//! Sod (1978) shock tube against the exact Riemann solution.
//!
//! γ = 1.4, left (ρ, u, p) = (1, 0, 1), right (0.125, 0, 0.1), diaphragm at x = 0.5 of a unit
//! tube, closed ends, 400 cells, t = 0.2 (waves have not reached the ends). Errors are relative
//! L1 norms over the tube: `Σ|q − q_exact| / Σ|q_exact|` for ρ and p, and
//! `Σ|u − u_exact| / (N max|u_exact|)` for u. Conservation: total mass change relative to the
//! initial mass.

use super::Check;
use crate::error::Result;
use crate::gas::Gas;
use crate::gas1d::scheme::wave_f;
use crate::gas1d::{Duct, Limiter, Network, Node, Port, Simulation};

const GAMMA: f64 = 1.4;
const LEFT: (f64, f64, f64) = (1.0, 0.0, 1.0);
const RIGHT: (f64, f64, f64) = (0.125, 0.0, 0.1);

/// Exact solution of the Riemann problem `(ρ, u, p)_{L,R}` at `ξ = x/t` for a perfect gas
/// (Toro 2009, §4.2–4.5).
pub fn exact_riemann(l: (f64, f64, f64), r: (f64, f64, f64), g: f64, xi: f64) -> (f64, f64, f64) {
    let c = |s: (f64, f64, f64)| (g * s.2 / s.0).sqrt();
    let (cl, cr) = (c(l), c(r));
    // Star pressure: f_L(p) + f_R(p) + u_R − u_L = 0 by Newton from the two-rarefaction guess.
    let mut p = ((cl + cr - 0.5 * (g - 1.0) * (r.1 - l.1))
        / (cl / l.2.powf((g - 1.0) / (2.0 * g)) + cr / r.2.powf((g - 1.0) / (2.0 * g))))
    .powf(2.0 * g / (g - 1.0));
    for _ in 0..100 {
        let (fl, dl) = wave_f(p, l.0, l.2, cl, g);
        let (fr, dr) = wave_f(p, r.0, r.2, cr, g);
        let step = (fl + fr + r.1 - l.1) / (dl + dr);
        p = (p - step).max(1e-12);
        if step.abs() < 1e-14 * p {
            break;
        }
    }
    let u_star =
        0.5 * (l.1 + r.1) + 0.5 * (wave_f(p, r.0, r.2, cr, g).0 - wave_f(p, l.0, l.2, cl, g).0);
    let g1 = (g - 1.0) / (g + 1.0);
    // Sample one side; `sign` = −1 for the left wave, +1 for the right wave.
    let side = |k: (f64, f64, f64), ck: f64, sign: f64| -> (f64, f64, f64) {
        let (rho, u, pk) = k;
        if p > pk {
            let s = u + sign * ck * ((g + 1.0) / (2.0 * g) * p / pk + (g - 1.0) / (2.0 * g)).sqrt();
            if sign * (xi - s) >= 0.0 {
                k
            } else {
                (rho * (p / pk + g1) / (g1 * p / pk + 1.0), u_star, p)
            }
        } else {
            let head = u + sign * ck;
            let c_star = ck * (p / pk).powf((g - 1.0) / (2.0 * g));
            let tail = u_star + sign * c_star;
            if sign * (xi - head) >= 0.0 {
                k
            } else if sign * (xi - tail) <= 0.0 {
                (rho * (p / pk).powf(1.0 / g), u_star, p)
            } else {
                let cf = 2.0 / (g + 1.0) * (ck - sign * (g - 1.0) / 2.0 * (u - xi));
                let uf = 2.0 / (g + 1.0) * (-sign * ck + (g - 1.0) / 2.0 * u + xi);
                (
                    rho * (cf / ck).powf(2.0 / (g - 1.0)),
                    uf,
                    pk * (cf / ck).powf(2.0 * g / (g - 1.0)),
                )
            }
        }
    };
    if xi <= u_star {
        side(l, cl, -1.0)
    } else {
        side(r, cr, 1.0)
    }
}

pub fn run() -> Result<Vec<Check>> {
    let cells = 400;
    let gas = Gas::perfect(GAMMA, 1.0);
    let duct = Duct::conical("sod", 1.0, 1.0, 1.0, 1.0 / cells as f64);
    let net = Network {
        gas,
        ducts: vec![duct],
        nodes: vec![Node::Wall(Port::start(0)), Node::Wall(Port::end(0))],
        limiter: Limiter::VanLeer,
        cfl: 0.8,
    };
    let mut sim = Simulation::new(net, |_, x| if x < 0.5 { LEFT } else { RIGHT })?;
    let m0 = sim.duct_mass();
    let t_end = 0.2;
    while sim.t < t_end {
        let dt = sim.stable_dt()?.min(t_end - sim.t);
        sim.step(dt)?;
    }
    let (mut e_rho, mut n_rho, mut e_p, mut n_p, mut e_u, mut u_max) =
        (0.0, 0.0, 0.0, 0.0, 0.0, 0.0f64);
    for i in 0..cells {
        let x = (i as f64 + 0.5) / cells as f64;
        let (rho, u, p) = exact_riemann(LEFT, RIGHT, GAMMA, (x - 0.5) / t_end);
        let s = sim.prim(0, i);
        e_rho += (s.rho - rho).abs();
        n_rho += rho.abs();
        e_p += (s.p - p).abs();
        n_p += p.abs();
        e_u += (s.u - u).abs();
        u_max = u_max.max(u.abs());
    }
    let mass_error = ((sim.duct_mass() - m0) / m0).abs();
    let l1 = |name: &str, v: f64| {
        Check::new(
            "Sod shock tube",
            format!("{name} relative L1 error"),
            v,
            0.02,
        )
    };
    Ok(vec![
        l1("density", e_rho / n_rho),
        l1("pressure", e_p / n_p),
        l1("velocity", e_u / (cells as f64 * u_max)),
        Check::new("Sod shock tube", "relative mass change", mass_error, 1e-10),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_solution_star_state() {
        // Toro (2009) Table 4.3, test 1: p* = 0.30313, u* = 0.92745.
        let (_, u, p) = exact_riemann(LEFT, RIGHT, GAMMA, 0.5);
        assert!(
            (p - 0.30313).abs() < 1e-5 && (u - 0.92745).abs() < 1e-5,
            "{p} {u}"
        );
    }
}
