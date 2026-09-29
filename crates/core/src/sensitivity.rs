//! How much the predicted sound depends on each input the project records as estimated or
//! derived. Each is set to the low and to the high end of its range (the stated one, else its
//! class's default: `inputs`) with everything else held, and the receiver's ⅓-octave levels at
//! one operating point are set against those with every input as recorded. The changes combine
//! root-sum-square into an uncertainty band, which takes the inputs as independent and their
//! effects as linear across their ranges.
//!
//! Levels come from the settled time-domain cycle (`solve::settle`): every harmonic its samples
//! resolve, radiated to the receiver (`radiation::monopole_pressure`), not only those up to
//! `max_frequency_hz`.
//!
//! How a range applies: a number takes the range's ends; a table (`[[x, y], …]`) scales its
//! values `y`; a list scales every entry; an element (`system.elements.<id>`) scales its
//! lengths and volumes, less any the project records on their own; `system.routes` makes the
//! layout geometrically similar about the flange (positions, bend radii, and each element's
//! lengths along its axis: every pipe length) and scales every pipe's diameter.
//!
//! An input's impact is its largest change in the bands the model resolves (below cross-mode
//! cut-on, 8 cells per wavelength and outlet ka = 0.5 in the unperturbed state:
//! `render::Limits`) within 40 dB of the loudest. Where one end of a range cannot be built
//! (a longer element that no longer fits its pipes), the other end's change stands for both,
//! and the input says so.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rayon::prelude::*;
use rustfft::num_complex::Complex64;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::inputs::{Provenance, Uncertainty, value_at};
use crate::project::Project;
use crate::radiation::monopole_pressure;
use crate::render::{Limits, third_octaves};
use crate::solve::{Settled, receiver_position, reference_outlet, settle};
use crate::spectrum::fft;

const R_AIR: f64 = 287.05;
/// Bands this far below the loudest do not set an input's impact.
const RANGE_DB: f64 = 40.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Sensitivity {
    pub rpm: f64,
    /// ⅓-octave band centres, Hz.
    pub bands_hz: Vec<f64>,
    /// Receiver level with every input as recorded, dB re 20 µPa; `None` above the samples'
    /// Nyquist frequency or where nothing sounds.
    pub base_db: Vec<Option<f64>>,
    /// Root-sum-square over the inputs of each one's larger change, dB.
    pub combined_db: Vec<Option<f64>>,
    /// Bands above this frequency, Hz, are beyond the model (`render::Limits`); impacts are
    /// taken below it.
    pub limit_hz: f64,
    /// Inputs on their class's default range first, then by impact.
    pub inputs: Vec<InputEffect>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct InputEffect {
    pub path: String,
    pub provenance: Provenance,
    pub range: Uncertainty,
    pub default_range: bool,
    /// The ends swept: values for a number, scale factors otherwise.
    pub ends: [f64; 2],
    pub scaled: bool,
    /// Level change at the low and the high end, per band, dB.
    pub low_db: Vec<Option<f64>>,
    pub high_db: Vec<Option<f64>>,
    /// Largest change in the resolved bands within 40 dB of the loudest, dB.
    pub impact_db: f64,
    /// One end could not be built: why, the other end standing for both.
    pub note: Option<String>,
    /// Neither end could be built: the input is outside the band.
    pub error: Option<String>,
}

/// Sweeps every estimated or derived input of `project` at `rpm` on its load line, reporting
/// runs done and runs in all to `on_progress`, which stops the sweep by returning `false`.
pub fn sensitivity(
    project: &Project,
    rpm: f64,
    on_progress: &(dyn Fn(usize, usize) -> bool + Sync),
) -> Result<Sensitivity> {
    let swept: Vec<(String, Uncertainty, bool, Provenance)> = project
        .inputs
        .iter()
        .filter(|(path, _)| !path.starts_with("fabrication.") && !path.starts_with("measurements."))
        .filter_map(|(path, input)| {
            input
                .range(path)
                .map(|(u, default)| (path.clone(), u, default, input.provenance))
        })
        .collect();
    let total = 1 + 2 * swept.len();
    let done = AtomicUsize::new(0);
    let c0 = (1.4 * R_AIR * project.ambient.temperature_k).sqrt();
    let stopped = AtomicBool::new(false);
    // Receiver levels, and the model's limit, of a settled state.
    let levels = |p: &Project| -> Result<(Vec<Option<f64>>, f64)> {
        if stopped.load(Ordering::Relaxed) {
            return Err(Error::solver("stopped"));
        }
        let s = settle(
            p,
            rpm,
            p.operating.map_at(rpm),
            p.solver.dx_mm * 1e-3,
            &mut |_| !stopped.load(Ordering::Relaxed),
        )?;
        if !on_progress(done.fetch_add(1, Ordering::Relaxed) + 1, total) {
            stopped.store(true, Ordering::Relaxed);
            return Err(Error::solver("stopped"));
        }
        Ok((
            receiver_levels(p, &s, rpm)?,
            Limits::of_state(&s, c0).lowest(),
        ))
    };
    let (base, limit_hz) = levels(project)?;
    let bands = third_octaves();
    let loudest = base
        .iter()
        .flatten()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    let counts =
        |b: usize| bands[b].2 <= limit_hz && base[b].is_some_and(|l| l > loudest - RANGE_DB);
    let mut inputs: Vec<InputEffect> = swept
        .par_iter()
        .map(|(path, range, default_range, provenance)| {
            let run = |end: usize| -> Result<(f64, bool, Vec<Option<f64>>)> {
                let (value, scaled, p) = perturbed(project, path, range, end)?;
                let l = levels(&p)?.0;
                Ok((
                    value,
                    scaled,
                    l.iter()
                        .zip(&base)
                        .map(|(a, b)| a.zip(*b).map(|(a, b)| a - b))
                        .collect(),
                ))
            };
            let mut effect = InputEffect {
                path: path.clone(),
                provenance: *provenance,
                range: *range,
                default_range: *default_range,
                ends: [f64::NAN; 2],
                scaled: false,
                low_db: Vec::new(),
                high_db: Vec::new(),
                impact_db: 0.0,
                note: None,
                error: None,
            };
            let (l, h) = match (run(0), run(1)) {
                (Ok(l), Ok(h)) => (l, h),
                (Ok(l), Err(e)) => {
                    effect.note = Some(format!(
                        "high end not built ({e}); the low end stands for both"
                    ));
                    (l.clone(), l)
                }
                (Err(e), Ok(h)) => {
                    effect.note = Some(format!(
                        "low end not built ({e}); the high end stands for both"
                    ));
                    (h.clone(), h)
                }
                (Err(e), Err(_)) => {
                    effect.error = Some(e.to_string());
                    return effect;
                }
            };
            effect.ends = [l.0, h.0];
            effect.scaled = l.1;
            effect.impact_db = (0..base.len())
                .filter(|&b| counts(b))
                .flat_map(|b| [l.2[b].map(f64::abs), h.2[b].map(f64::abs)])
                .flatten()
                .fold(0.0, f64::max);
            (effect.low_db, effect.high_db) = (l.2, h.2);
            effect
        })
        .collect();
    if stopped.load(Ordering::Relaxed) {
        return Err(Error::solver("stopped"));
    }
    let combined_db = (0..base.len())
        .map(|b| {
            base[b].map(|_| {
                inputs
                    .iter()
                    .filter_map(|i| {
                        let pick = |v: &[Option<f64>]| v.get(b).copied().flatten().map(f64::abs);
                        match (pick(&i.low_db), pick(&i.high_db)) {
                            (Some(a), Some(b)) => Some(a.max(b)),
                            (a, b) => a.or(b),
                        }
                    })
                    .map(|d| d * d)
                    .sum::<f64>()
                    .sqrt()
            })
        })
        .collect();
    inputs.sort_by(|a, b| {
        b.default_range
            .cmp(&a.default_range)
            .then(b.impact_db.total_cmp(&a.impact_db))
    });
    Ok(Sensitivity {
        rpm,
        bands_hz: bands.iter().map(|b| b.0).collect(),
        base_db: base,
        combined_db,
        limit_hz,
        inputs,
    })
}

/// Receiver ⅓-octave levels of a settled cycle: every FFT bin of each outlet's volume velocity
/// over the last four cycles, radiated as monopoles with the ground's image.
fn receiver_levels(project: &Project, s: &Settled, rpm: f64) -> Result<Vec<Option<f64>>> {
    let a = &project.ambient;
    let (rho0, c0) = (
        a.pressure_pa / (R_AIR * a.temperature_k),
        (1.4 * R_AIR * a.temperature_k).sqrt(),
    );
    let receiver = receiver_position(project, &s.model, reference_outlet(project, &s.model)?);
    let positions: Vec<[f64; 3]> = s.model.outlets.iter().map(|o| o.position).collect();
    let analysed = 4.min(s.cycles as usize);
    let span = analysed as f64 * 120.0 / rpm;
    let spectra: Vec<Vec<Complex64>> = s
        .rec
        .outlet_q
        .iter()
        .map(|q| fft(&s.rec.last_cycles(q, analysed)))
        .collect();
    let n = spectra[0].len();
    let nyquist = 0.5 * n as f64 / span;
    let mut power = vec![0.0; n / 2];
    for (k, p) in power.iter_mut().enumerate().skip(1) {
        let q: Vec<Complex64> = spectra.iter().map(|s| 2.0 * s[k] / n as f64).collect();
        let f = k as f64 / span;
        let pk = monopole_pressure(
            2.0 * std::f64::consts::PI * f,
            rho0,
            c0,
            &positions,
            &q,
            receiver,
            a.ground_z_mm * 1e-3,
        );
        *p = 0.5 * pk.norm_sqr();
    }
    Ok(third_octaves()
        .iter()
        .map(|&(_, lo, hi)| {
            (hi <= nyquist)
                .then(|| {
                    let ms: f64 = power
                        .iter()
                        .enumerate()
                        .filter(|(k, _)| (lo..hi).contains(&(*k as f64 / span)))
                        .map(|(_, p)| p)
                        .sum();
                    (ms > 0.0).then(|| 10.0 * (ms / 4e-10).log10())
                })
                .flatten()
        })
        .collect())
}

/// `project` with the input at `path` at the low (`end` 0) or high (1) end of `range`: the
/// value or scale factor used, whether it was scaled, and the project.
fn perturbed(
    project: &Project,
    path: &str,
    range: &Uncertainty,
    end: usize,
) -> Result<(f64, bool, Project)> {
    let mut v = serde_json::to_value(project).expect("projects serialise");
    let current = value_at(&v, path)
        .ok_or_else(|| Error::invalid(format!("the project has no field {path}")))?;
    let (used, scaled) = match current.as_f64() {
        Some(x) => {
            let (lo, hi) = range.bounds(x);
            let to = [lo, hi][end];
            update_at(&mut v, path, &mut |leaf| *leaf = to.into());
            (to, false)
        }
        None => {
            let (lo, hi) = range.scale().ok_or_else(|| {
                Error::invalid(format!("{path}: a group takes a relative or factor range"))
            })?;
            let k = [lo, hi][end];
            let own: Vec<String> = project
                .inputs
                .keys()
                .filter_map(|p| p.strip_prefix(&format!("{path}.")).map(str::to_string))
                .collect();
            if path == "system.routes" {
                for route in v
                    .pointer_mut("/system/routes")
                    .and_then(Value::as_array_mut)
                    .into_iter()
                    .flatten()
                {
                    for key in ["via_mm", "bend_radius_mm"] {
                        scale_numbers(route.get_mut(key), k);
                    }
                    scale_numbers(route.pointer_mut("/pipe/od_mm"), k);
                }
                for element in v
                    .pointer_mut("/system/elements")
                    .and_then(Value::as_array_mut)
                    .into_iter()
                    .flatten()
                {
                    for (key, field) in element.as_object_mut().into_iter().flatten() {
                        if key == "position_mm"
                            || key == "extra_outlets_mm"
                            || AXIAL.contains(&key.as_str())
                        {
                            scale_numbers(Some(field), k);
                        }
                    }
                }
            } else if path.starts_with("system.elements.") {
                update_at(&mut v, path, &mut |element| {
                    for (key, field) in element.as_object_mut().into_iter().flatten() {
                        if (key.ends_with("_mm") || key.ends_with("_l"))
                            && field.is_number()
                            && !own.contains(key)
                        {
                            scale_numbers(Some(field), k);
                        }
                    }
                });
            } else {
                update_at(&mut v, path, &mut |leaf| scale_table(leaf, k));
            }
            (k, true)
        }
    };
    let p = Project::from_json(&v.to_string())
        .map_err(|e| Error::invalid(format!("{path} at {used}: {e}")))?;
    Ok((used, scaled, p))
}

/// Element fields that place its ports along its axis.
const AXIAL: [&str; 7] = [
    "length_mm",
    "taper_length_mm",
    "brick_length_mm",
    "inlet_cone_mm",
    "outlet_cone_mm",
    "inlet_extension_mm",
    "outlet_extension_mm",
];

/// Multiplies every number in `v` (nested lists included) by `k`.
fn scale_numbers(v: Option<&mut Value>, k: f64) {
    match v {
        Some(Value::Number(n)) => *v.unwrap() = (n.as_f64().unwrap_or(0.0) * k).into(),
        Some(Value::Array(items)) => items.iter_mut().for_each(|i| scale_numbers(Some(i), k)),
        _ => {}
    }
}

/// Scales a number, a list's entries, or a table's values (`[[x, y], …]`: `y`) by `k`.
fn scale_table(v: &mut Value, k: f64) {
    match v {
        Value::Array(rows)
            if rows
                .iter()
                .all(|r| r.as_array().is_some_and(|r| r.len() == 2)) =>
        {
            rows.iter_mut().for_each(|r| scale_numbers(r.get_mut(1), k))
        }
        _ => scale_numbers(Some(v), k),
    }
}

/// Applies `f` to the value at `path` (`inputs::value_at`'s addressing), to each element's
/// field where a segment runs across a list.
fn update_at(v: &mut Value, path: &str, f: &mut dyn FnMut(&mut Value)) {
    let Some((seg, rest)) = path
        .split_once('.')
        .map_or(Some((path, None)), |(a, b)| Some((a, Some(b))))
    else {
        return;
    };
    let next: Vec<&mut Value> = match v {
        Value::Object(m) => m.get_mut(seg).into_iter().collect(),
        Value::Array(items) => {
            let by_id = items
                .iter()
                .position(|i| i.get("id").and_then(Value::as_str) == Some(seg));
            match by_id {
                Some(i) => vec![&mut items[i]],
                None => items.iter_mut().filter_map(|i| i.get_mut(seg)).collect(),
            }
        }
        _ => Vec::new(),
    };
    for n in next {
        match rest {
            Some(rest) => update_at(n, rest, f),
            None => f(n),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W205: &str = include_str!("../../../tests/cases/w205_stock.json");

    /// Each kind of input moves the field it names, and only it.
    #[test]
    fn ranges_apply_where_they_say() {
        let p = Project::from_json(W205).unwrap();
        let (v, scaled, q) = perturbed(
            &p,
            "engine.valves.evo_deg",
            &Uncertainty::Range {
                low: 110.0,
                high: 150.0,
            },
            1,
        )
        .unwrap();
        assert_eq!((v, scaled, q.engine.valves.evo_deg), (150.0, false, 150.0));
        let (_, _, q) = perturbed(
            &p,
            "operating.map_kpa",
            &Uncertainty::Relative { fraction: 0.2 },
            0,
        )
        .unwrap();
        assert_eq!(q.operating.map_kpa, vec![[1000.0, 44.0]]);
        let (_, _, q) = perturbed(
            &p,
            "turbine.scrolls.manifold_volume_l",
            &Uncertainty::Relative { fraction: 0.3 },
            1,
        )
        .unwrap();
        assert!(
            q.turbine
                .scrolls
                .iter()
                .all(|s| (s.manifold_volume_l - 0.455).abs() < 1e-12)
        );
        let (_, _, q) = perturbed(
            &p,
            "system.elements.cat",
            &Uncertainty::Relative { fraction: 0.1 },
            1,
        )
        .unwrap();
        let cat = |p: &Project| match &p.element("cat").unwrap().kind {
            crate::project::ElementKind::Catalyst(c) => c.clone(),
            _ => unreachable!(),
        };
        let (a, b) = (cat(&p), cat(&q));
        assert!((b.brick_length_mm - 1.1 * a.brick_length_mm).abs() < 1e-9);
        // Recorded on their own: not scaled with the element.
        assert_eq!((b.cell_wall_mm, b.cpsi), (a.cell_wall_mm, a.cpsi));
        assert_eq!(
            p.element("cat").unwrap().position_mm,
            q.element("cat").unwrap().position_mm
        );
        let (_, _, q) = perturbed(
            &p,
            "system.routes",
            &Uncertainty::Relative { fraction: 0.05 },
            1,
        )
        .unwrap();
        let tip = |p: &Project| p.element("tip-left").unwrap().position_mm;
        assert!((tip(&q)[0] - 1.05 * tip(&p)[0]).abs() < 1e-9);
        assert!(
            (q.system.routes[0].pipe.od_mm - 1.05 * p.system.routes[0].pipe.od_mm).abs() < 1e-9
        );
    }
}
