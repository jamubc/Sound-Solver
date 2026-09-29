//! Grid convergence and determinism on the full engine-to-tailpipe chain
//! (`tests/cases/straight_pipe.json`).
//!
//! Grid: the operating point is solved at Δx = 15 mm and 7.5 mm; halving Δx must move the
//! 2nd-order receiver SPL by less than 0.5 dB and the downpipe-flange backpressure by less than
//! 1 %. Determinism: the same project solved twice gives bit-identical results.

use super::Check;
use crate::error::{Error, Result};
use crate::project::Project;
use crate::solve::{PointResult, solve_point, solve_sweep};

/// The plain-pipe reference case, compiled in so the suite runs anywhere.
pub const STRAIGHT_PIPE: &str = include_str!("../../../../tests/cases/straight_pipe.json");

const RPMS: [f64; 2] = [3000.0, 4500.0];

fn second_order(r: &PointResult) -> Result<f64> {
    r.orders
        .iter()
        .find(|o| o.0 == 2)
        .map(|o| o.2)
        .ok_or_else(|| Error::solver("no 2nd-order line"))
}

pub fn run() -> Result<Vec<Check>> {
    let base = Project::from_json(STRAIGHT_PIPE)?;
    let mut fine = base.clone();
    fine.solver.dx_mm = base.solver.dx_mm / 2.0;
    let mut checks = Vec::new();
    for rpm in RPMS {
        let (a, b) = (solve_point(&base, rpm)?, solve_point(&fine, rpm)?);
        for (r, dx) in [(&a, base.solver.dx_mm), (&b, fine.solver.dx_mm)] {
            if !r.provenance.converged {
                return Err(Error::solver(format!(
                    "grid case did not converge at {rpm} rpm, Δx = {dx} mm"
                )));
            }
        }
        let case = "Grid convergence";
        checks.push(Check::new(
            case,
            format!("2nd-order SPL change, dB ({rpm} rpm)"),
            (second_order(&a)? - second_order(&b)?).abs(),
            0.5,
        ));
        checks.push(Check::new(
            case,
            format!("backpressure relative change ({rpm} rpm)"),
            ((a.backpressure_pa - b.backpressure_pa) / b.backpressure_pa).abs(),
            0.01,
        ));
    }
    Ok(checks)
}

pub fn run_determinism() -> Result<Vec<Check>> {
    let project = Project::from_json(STRAIGHT_PIPE)?;
    let rpms = [2200.0, 5000.0];
    let first = serde_json::to_string(&solve_sweep(&project, &rpms)).expect("serialises");
    let second = serde_json::to_string(&solve_sweep(&project, &rpms)).expect("serialises");
    let differing = first
        .bytes()
        .zip(second.bytes())
        .filter(|(a, b)| a != b)
        .count()
        + first.len().abs_diff(second.len());
    Ok(vec![Check::new(
        "Determinism",
        "differing bytes between two identical sweeps",
        differing as f64,
        0.0,
    )])
}
