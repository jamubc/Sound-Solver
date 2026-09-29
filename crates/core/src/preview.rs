//! Four-pole preview: the linear frequency-domain solution of the same network, fast enough
//! to rerun on every edit.
//!
//! Per engine speed: the engine source's Norton equivalent from a fixed-pressure run of the
//! cylinders, manifolds and turbine into ambient pressure (`SourceBc::norton`); the mean state
//! from the thermal model at that flow and temperature (uniform source temperature when the
//! project's walls are adiabatic or fixed); at each harmonic of the cycle frequency the network
//! is solved with the Norton drive (`fourpole::solve`), and the outlets' volume velocities
//! radiate to the receiver exactly as in the time-domain result. Everything nonlinear is
//! absent: source–pipe interaction beyond the linear admittance, loss nonlinearity, wave
//! steepening, tailpipe inflow. Networks with orifices (valves) are flagged small-signal.
//!
//! The source interaction dominates near pipe resonances. On the stock W205 the network's
//! flange-to-tailpipe transfer peaks at 2800 rpm in both solvers, but there the time domain's
//! 2nd-order flange flow falls 8 dB against the Norton source's 2 dB (whichever back pressure
//! the Norton run discharges into), so the preview's 2nd-order peak sits ~200 rpm above the
//! time domain's. Size against time-domain results.

use std::f64::consts::PI;

use rustfft::num_complex::Complex64;

use crate::error::Result;
use crate::fourpole::{self, CellMean, Drive, MeanState};
use crate::gas::Gas;
use crate::gas1d::nodes::NortonSource;
use crate::gas1d::{JunctionKind, Node};
use crate::model::{Model, SourceInputs, build};
use crate::project::{Project, WallThermal};
use crate::solve::{
    PointResult, Provenance, ReceiverSpectrum, SolverKind, evo_state, grid_summary,
    receiver_spectrum, thermal_inputs,
};
use crate::thermal;

/// Cycles of the fixed-pressure source run (the manifolds settle in two) and samples per cycle.
const SOURCE_CYCLES: usize = 4;
const SOURCE_SAMPLES: usize = 512;

/// Mean state from the thermal model (or a uniform source temperature), with a description
/// for provenance.
pub fn mean_state(
    project: &Project,
    model: &Model,
    mass_flow: f64,
    t0: f64,
) -> Result<(MeanState, String)> {
    let net = &model.network;
    let p = project.ambient.pressure_pa;
    let (temps, desc): (Vec<Vec<f64>>, String) = match project.solver.wall_thermal {
        WallThermal::Computed => {
            let pr = thermal::solve(
                net,
                model.source_node,
                &thermal_inputs(project, mass_flow, t0),
            )?;
            let desc = format!(
                "thermal model; source {t0:.1} K, {mass_flow:.4} kg/s [{}]",
                pr.hash
            );
            (pr.t_gas, desc)
        }
        _ => (
            net.ducts.iter().map(|d| vec![t0; d.n()]).collect(),
            format!("uniform {t0:.1} K (walls not computed)"),
        ),
    };
    let flows = thermal::mean_flows(net, model.source_node, mass_flow)?;
    let cells = net
        .ducts
        .iter()
        .enumerate()
        .map(|(d, duct)| {
            let mdot = flows[d]
                .map(|(m, fwd)| if fwd { m } else { -m })
                .unwrap_or(0.0);
            (0..duct.n())
                .map(|i| {
                    let t = temps[d][i];
                    let rho = p / (net.gas.r() * t);
                    CellMean {
                        rho,
                        c: net.gas.sound_speed(t),
                        u: mdot / (rho * duct.volume[i] / duct.dx),
                        t,
                    }
                })
                .collect()
        })
        .collect();
    Ok((MeanState { cells }, desc))
}

/// The engine source's Norton equivalent at the model's speed: a run into ambient pressure.
pub fn norton(model: &Model, gas: &Gas, p_amb: f64) -> Result<NortonSource> {
    let Node::Source(src) = &model.network.nodes[model.source_node] else {
        unreachable!("model records its source node")
    };
    src.norton(gas, p_amb, 900.0, SOURCE_CYCLES, SOURCE_SAMPLES)
}

pub fn solve_point(project: &Project, rpm: f64) -> Result<PointResult> {
    let gas = Gas::exhaust(project.gas.fuel_h_to_c, project.gas.lambda)?;
    let (evo, evo_source) = evo_state(project, &gas, project.operating.map_at(rpm));
    let p_amb = project.ambient.pressure_pa;
    let model = build(
        project,
        gas.clone(),
        project.solver.dx_mm * 1e-3,
        SourceInputs {
            rpm,
            evo,
            p_init: 1.2 * p_amb,
            t_init: 900.0,
        },
    )?;
    let norton = norton(&model, &gas, p_amb)?;
    let (mean, thermal_desc) = mean_state(project, &model, norton.mean_mass_flow, norton.mean_t0)?;
    let net = &model.network;
    let f_cycle = rpm / 120.0;
    let mut failure = None;
    let ReceiverSpectrum {
        lines: spectrum,
        orders,
        overall_db,
        overall_dba,
    } = receiver_spectrum(project, &model, rpm, f_cycle, |m| {
        if m >= SOURCE_SAMPLES / 2 {
            return None;
        }
        let omega = 2.0 * PI * m as f64 * f_cycle;
        let drive = Drive::Norton {
            q: norton.strength(m),
            y: norton.admittance(omega),
        };
        match fourpole::solve(net, &mean, omega, &[(model.source_node, drive)]) {
            Ok(sol) => Some(
                model
                    .outlets
                    .iter()
                    .map(|o| sol.volume_velocity_out(net, &mean, net.nodes[o.node].ports()[0]))
                    .collect::<Vec<Complex64>>(),
            ),
            Err(e) => {
                failure = Some(e);
                None
            }
        }
    })?;
    if let Some(e) = failure {
        return Err(e);
    }
    let small_signal = net
        .nodes
        .iter()
        .any(|n| matches!(n, Node::Junction(j) if matches!(j.kind, JunctionKind::Orifice { .. })));
    let (cells, dx_min_mm, dx_max_mm) = grid_summary(net);
    let outlet_port = net.nodes[model.outlets[0].node].ports()[0];
    let outlet_cell = net.ducts[outlet_port.duct].end_cell(outlet_port.end);
    let mut warnings = model.warnings.clone();
    warnings.push(
        "four-pole preview: linear, small-signal; the time-domain solution is the reference".into(),
    );
    Ok(PointResult {
        rpm,
        provenance: Provenance {
            solver: SolverKind::FourPole,
            core_version: env!("CARGO_PKG_VERSION").to_string(),
            project_hash: project.hash(),
            dx_target_mm: project.solver.dx_mm,
            cells,
            dx_min_mm,
            dx_max_mm,
            gas_table: gas.label().to_string(),
            thermal_profile: thermal_desc,
            evo,
            evo_source,
            time_domain: None,
            small_signal,
            warnings,
        },
        spectrum,
        orders,
        overall_db,
        overall_dba,
        backpressure_pa: None,
        mass_flow_kg_s: norton.mean_mass_flow,
        mass_balance_error: None,
        turbine_power_w: norton.mean_power,
        outlet_temperature_k: mean.cells[outlet_port.duct][outlet_cell].t,
    })
}
