//! Sizing a side branch onto a drone (directive goal 3): the quarter-wave stub length, or the
//! Helmholtz cavity volume, that puts the branch's resonance on a target frequency.
//!
//! The resonance is the lowest frequency at which the branch's input reactance at the tee
//! vanishes, compliant (`Im Z < 0`) below it: `Z = p/m' = −T₂₂/T₂₁`, `T` the four-pole transfer
//! matrix of the branch ducts from the tee to the closed end, where `m' = 0`. There the branch
//! shorts the main pipe: the notch it cuts in the transmission. The ducts are the solvers' own
//! (end corrections included), with the branch gas at a stated temperature range (measured:
//! a thermocouple at the branch), or else at the thermal model's temperatures for the dead-end
//! branch at the operating point: conduction along its wall only, from the tee. The model's
//! band (`thermal::ThermalProfile::t_gas_band`) runs from heat transfer 30 % stronger up to the
//! main flow's temperature at the tee, the most the unmodelled heat carried in at the mouth
//! could give the branch. With walls not computed the branch gas is the source's, as in the
//! four-pole preview, unbanded. The size is found by fixed point on `f ∝ 1/L` (stub) or
//! `f ∝ 1/√V` (Helmholtz), at the nominal temperature and at both ends of the band.

use std::f64::consts::PI;

use rustfft::num_complex::Complex64 as C64;
use schemars::JsonSchema;
use serde::Serialize;

use crate::error::{Error, Result};
use crate::fourpole::{CellMean, M2, MeanState, duct_matrix, mul};
use crate::gas::Gas;
use crate::gas1d::Network;
use crate::manifest;
use crate::model::{SourceInputs, build};
use crate::preview::norton;
use crate::project::{ElementKind, Project, WallThermal};
use crate::solve::{evo_state, thermal_inputs};
use crate::thermal;

#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct Tuning {
    pub element: String,
    /// The parameter sized: `length_mm` (stub) or `volume_l` (Helmholtz).
    pub parameter: String,
    pub target_hz: f64,
    /// Engine speed whose thermal state the branch gas takes.
    pub rpm: f64,
    /// The branch's resonance as drawn.
    pub resonance_hz: f64,
    pub value: f64,
    /// The value for the hottest and for the coolest branch gas; absent when the walls are not
    /// computed and no temperature is stated.
    pub band: Option<[f64; 2]>,
    /// Mean branch gas temperature at each value (nominal, then hottest and coolest), K.
    pub branch_temperature_k: Vec<f64>,
    /// Where the branch gas temperature comes from.
    pub basis: String,
}

/// The size of branch `element` that puts its resonance at `target_hz`, with the branch gas
/// at `stated_k` (lowest, highest), K, or in its thermal state at `rpm`.
pub fn tune(
    project: &Project,
    element: &str,
    target_hz: f64,
    rpm: f64,
    stated_k: Option<[f64; 2]>,
) -> Result<Tuning> {
    let el = project
        .element(element)
        .ok_or_else(|| Error::invalid(format!("no element '{element}'")))?;
    let (kind, parameter, now) = match &el.kind {
        ElementKind::QuarterWaveStub(s) => ("quarter_wave_stub", "length_mm", s.length_mm),
        ElementKind::Helmholtz(h) => ("helmholtz", "volume_l", h.volume_l),
        _ => {
            return Err(Error::invalid(format!(
                "'{element}' is not a stub or a Helmholtz resonator"
            )));
        }
    };
    let param = manifest::get(kind)
        .and_then(|m| m.params.iter().find(|p| p.key == parameter))
        .expect("manifests list the sized parameters");
    let (lo, hi) = (param.min.unwrap_or(0.0), param.max.unwrap_or(f64::INFINITY));
    let gas = Gas::exhaust(project.gas.fuel_h_to_c, project.gas.lambda)?;
    let p_amb = project.ambient.pressure_pa;
    let dx = project.solver.dx_mm * 1e-3;
    let inputs = SourceInputs {
        rpm,
        evo: evo_state(project, &gas, rpm).0,
        p_init: 1.2 * p_amb,
        t_init: 900.0,
    };
    let source = norton(&build(project, gas.clone(), dx, inputs)?, &gas, p_amb)?;
    let computed = project.solver.wall_thermal == WallThermal::Computed;
    let banded = computed || stated_k.is_some();
    // Resonance and mean gas temperature of the branch sized `value`, per thermal state.
    let branch = |value: f64| -> Result<Vec<(f64, f64)>> {
        let mut sized = project.clone();
        for e in &mut sized.system.elements {
            match &mut e.kind {
                ElementKind::QuarterWaveStub(s) if e.id == element => s.length_mm = value,
                ElementKind::Helmholtz(h) if e.id == element => h.volume_l = value,
                _ => {}
            }
        }
        let model = build(&sized, gas.clone(), dx, inputs)?;
        let net = &model.network;
        let uniform =
            |t: f64| -> Vec<Vec<f64>> { net.ducts.iter().map(|d| vec![t; d.n()]).collect() };
        let states = match stated_k {
            Some([lo, hi]) => vec![uniform(0.5 * (lo + hi)), uniform(hi), uniform(lo)],
            None if computed => {
                let profile = thermal::solve(
                    net,
                    model.source_node,
                    &thermal_inputs(&sized, source.mean_mass_flow, source.mean_t0),
                )?;
                let [hot, cold] = profile.t_gas_band;
                vec![profile.t_gas, hot, cold]
            }
            None => vec![uniform(source.mean_t0)],
        };
        let ducts: Vec<usize> = ["stub", "neck", "cavity"]
            .iter()
            .filter_map(|part| {
                let label = format!("{element} ({part})");
                net.ducts.iter().position(|d| d.label == label)
            })
            .collect();
        states
            .iter()
            .map(|temps| {
                let mean = MeanState {
                    cells: temps
                        .iter()
                        .map(|d| {
                            d.iter()
                                .map(|&t| CellMean {
                                    rho: p_amb / (net.gas.r() * t),
                                    c: net.gas.sound_speed(t),
                                    u: 0.0,
                                    t,
                                })
                                .collect()
                        })
                        .collect(),
                };
                let (heat, volume) = ducts.iter().fold((0.0, 0.0), |(h, v), &d| {
                    let cells = temps[d].iter().zip(&net.ducts[d].volume);
                    cells.fold((h, v), |(h, v), (t, dv)| (h + t * dv, v + dv))
                });
                Ok((resonance(net, &mean, &ducts)?, heat / volume))
            })
            .collect()
    };
    let resonance_hz = branch(now)?[0].0;
    let size = |state: usize| -> Result<(f64, f64)> {
        let mut value = now;
        for _ in 0..40 {
            let (f, t) = branch(value)?[state];
            if (f / target_hz - 1.0).abs() < 1e-5 {
                return Ok((value, t));
            }
            let ratio = f / target_hz;
            let next = match parameter {
                "length_mm" => value * ratio,
                _ => value * ratio * ratio,
            }
            .clamp(lo, hi);
            if next == value {
                break;
            }
            value = next;
        }
        Err(Error::invalid(format!(
            "no {} within {lo}–{hi} {} puts the resonance of '{element}' at {target_hz:.1} Hz",
            param.label.to_lowercase(),
            param.unit.as_deref().unwrap_or_default()
        )))
    };
    let (value, t) = size(0)?;
    let mut branch_temperature_k = vec![t];
    let band = if banded {
        let ((hot, t_hot), (cold, t_cold)) = (size(1)?, size(2)?);
        branch_temperature_k.extend([t_hot, t_cold]);
        Some([hot, cold])
    } else {
        None
    };
    let basis = match stated_k {
        Some([lo, hi]) => format!("stated {lo:.0}–{hi:.0} K"),
        None if computed => format!(
            "thermal model at {rpm:.0} rpm: conduction along the branch wall; up to the main \
             flow's temperature at the tee for heat carried in at the mouth (not modelled)"
        ),
        None => format!(
            "source temperature {:.0} K (walls not computed)",
            source.mean_t0
        ),
    };
    Ok(Tuning {
        element: element.into(),
        parameter: parameter.into(),
        target_hz,
        rpm,
        resonance_hz,
        value,
        band,
        branch_temperature_k,
        basis,
    })
}

/// Lowest frequency at which the reactance of `ducts` (in order from the tee, closed at the
/// far end) turns from compliant, Hz.
fn resonance(net: &Network, mean: &MeanState, ducts: &[usize]) -> Result<f64> {
    let (zero, one) = (C64::new(0.0, 0.0), C64::new(1.0, 0.0));
    let reactance = |f: f64| {
        let omega = 2.0 * PI * f;
        let t = ducts
            .iter()
            .fold([[one, zero], [zero, one]], |acc: M2, &d| {
                mul(&duct_matrix(net, mean, d, omega), &acc)
            });
        (-t[1][1] / t[1][0]).im
    };
    let (mut lo, mut hi) = (1.0, 1.0);
    while reactance(hi) < 0.0 {
        lo = hi;
        hi *= 1.05;
        if hi > 5000.0 {
            return Err(Error::solver("the branch has no resonance below 5 kHz"));
        }
    }
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        if reactance(mid) < 0.0 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Ok(0.5 * (lo + hi))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::grid::STRAIGHT_PIPE;

    /// Lossless (no wall friction) and with walls not computed, the stub gas is the source's
    /// and the tuned length the quarter wave less the end correction, `c/(4f) − 0.6 r`.
    /// Computed walls band it, longer for hotter gas.
    #[test]
    fn stub_length_for_a_frequency() {
        let mut project = Project::from_json(STRAIGHT_PIPE).unwrap();
        let id = crate::edit::insert(&mut project, "mid-pipe", 500.0, "quarter_wave_stub").unwrap();
        let bore = match &project.element(&id).unwrap().kind {
            ElementKind::QuarterWaveStub(s) => s.id_mm,
            _ => unreachable!(),
        };
        project.solver.friction = false;
        let t = tune(&project, &id, 150.0, 3000.0, None).unwrap();
        let gas = Gas::exhaust(project.gas.fuel_h_to_c, project.gas.lambda).unwrap();
        let c = gas.sound_speed(t.branch_temperature_k[0]);
        let quarter = (c / (4.0 * 150.0) - 0.6 * bore / 2.0 * 1e-3) * 1e3;
        assert!(
            (t.value / quarter - 1.0).abs() < 1e-3,
            "{} mm vs {quarter} mm",
            t.value
        );
        assert!(t.band.is_none());
        // A stated range bands it the same way: c ∝ √T.
        let t = tune(&project, &id, 150.0, 3000.0, Some([400.0, 500.0])).unwrap();
        let [hot, cold] = t.band.unwrap();
        let length = |t_k: f64| (gas.sound_speed(t_k) / 600.0 - 0.3 * bore * 1e-3) * 1e3;
        for (tuned, t_k) in [(t.value, 450.0), (hot, 500.0), (cold, 400.0)] {
            assert!(
                (tuned / length(t_k) - 1.0).abs() < 1e-3,
                "{tuned} mm at {t_k} K"
            );
        }

        project.solver.friction = true;
        project.solver.wall_thermal = WallThermal::Computed;
        let t = tune(&project, &id, 150.0, 3000.0, None).unwrap();
        let [hot, cold] = t.band.unwrap();
        assert!(
            hot > t.value && t.value > cold,
            "{hot} > {} > {cold}",
            t.value
        );
        let [nominal, t_hot, t_cold] = t.branch_temperature_k[..] else {
            unreachable!()
        };
        assert!(t_hot > nominal && nominal > t_cold);
    }
}
