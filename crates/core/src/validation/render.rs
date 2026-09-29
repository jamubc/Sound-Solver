//! Render against the periodic solution (`tests/cases/straight_pipe.json`, adiabatic walls):
//! - a steady scene marched at 48 kHz and radiated in the time domain must reproduce, line by
//!   line, the receiver spectrum the periodic solve reports for the same operating point (the
//!   march, the outlet recording and the time-domain radiation together);
//! - a scene that ramps engine speed and intake pressure and then holds must end on the
//!   periodic solution of the new point (crank angle from the speed trace, each event's EVO
//!   state from the intake pressure of its moment).

use super::Check;
use super::grid::STRAIGHT_PIPE;
use crate::error::{Error, Result};
use crate::project::Project;
use crate::render::{Listener, Scene, render};
use crate::solve::solve_point;
use crate::spectrum::fft;

const RPM: f64 = 3000.0;
const MAP_KPA: f64 = 90.0;
/// Lines this far below the strongest are left out: their levels are set by the periodicity
/// tolerance, not by the physics both paths share.
const RANGE_DB: f64 = 40.0;

pub fn run() -> Result<Vec<Check>> {
    let project = Project::from_json(STRAIGHT_PIPE)?;
    let steady = Scene {
        duration_s: 0.6,
        rpm: vec![[0.0, RPM]],
        map_kpa: None,
        listener: Listener::Receiver,
    };
    let mut loaded = project.clone();
    loaded.operating.map_kpa = vec![[RPM, MAP_KPA]];
    let ramp = Scene {
        duration_s: 1.2,
        rpm: vec![[0.0, 2500.0], [0.2, RPM]],
        map_kpa: Some(vec![[0.0, loaded_from(&project)], [0.2, MAP_KPA]]),
        listener: Listener::Receiver,
    };
    Ok(vec![
        Check::new(
            "Render vs periodic solve",
            format!("steady: line level, max |Δ| dB, lines within {RANGE_DB} dB ({RPM} rpm)"),
            worst_line(&project, &steady, &project)?,
            0.5,
        ),
        Check::new(
            "Render vs periodic solve",
            format!(
                "after a speed and load ramp: line level, max |Δ| dB ({RPM} rpm, {MAP_KPA} kPa)"
            ),
            worst_line(&project, &ramp, &loaded)?,
            0.5,
        ),
    ])
}

/// Intake pressure of `project`'s load line at 2500 rpm, kPa.
fn loaded_from(project: &Project) -> f64 {
    project.operating.map_at(2500.0)
}

/// Largest level difference between the last whole periods of `scene` rendered on `project`
/// and the periodic receiver spectrum of `reference` at `RPM`, over the lines within
/// `RANGE_DB` of the strongest.
fn worst_line(project: &Project, scene: &Scene, reference: &Project) -> Result<f64> {
    let point = solve_point(reference, RPM)?;
    let period = point
        .provenance
        .time_domain
        .as_ref()
        .map_or(1, |t| t.period_cycles) as usize;
    let r = render(project, scene, &mut |_| true)?;
    let fs = r.info.sample_rate;
    // The last half second, in whole periods.
    let per_cycle = (fs * 120.0 / RPM).round() as usize;
    let cycles = ((0.5 * fs) as usize / per_cycle / period * period).max(period);
    let x: Vec<f64> = r.channels[0][r.channels[0].len() - cycles * per_cycle..]
        .iter()
        .map(|&s| s as f64)
        .collect();
    let spec = fft(&x);
    let strongest = point
        .spectrum
        .iter()
        .map(|l| l.spl_db)
        .fold(f64::NEG_INFINITY, f64::max);
    let mut worst: f64 = 0.0;
    for (m, line) in point.spectrum.iter().enumerate() {
        if line.spl_db < strongest - RANGE_DB {
            continue;
        }
        // Line m + 1 of spacing f_c/P falls on bin (m + 1)·cycles/P.
        let bin = (m + 1) * cycles / period;
        let rms = spec
            .get(bin)
            .ok_or_else(|| Error::solver("render too short for the solved lines"))?
            .norm()
            * std::f64::consts::SQRT_2
            / x.len() as f64;
        worst = worst.max((20.0 * (rms / 20e-6).log10() - line.spl_db).abs());
    }
    Ok(worst)
}
