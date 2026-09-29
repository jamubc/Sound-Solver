//! Levine–Schwinger outlet in the time-domain solver.
//!
//! A right-running Gaussian pulse (1 Pa, σ = 10 mm) in air at 293.15 K travels down a 2 m
//! pipe of radius 25 mm (Δx = 2 mm) to the radiating outlet. The incident and reflected
//! characteristics `W⁺`, `W⁻` at the outlet face are recorded until just before the wave
//! reflected from the far end returns, and `R(ω) = ∫W⁻e^{−jωt}dt / ∫W⁺e^{−jωt}dt`. Magnitude
//! and end correction `l/a = −arg(−R)/(2ka)` are compared with the Levine–Schwinger integrals
//! (`radiation::ls_*`) at ka = 0.1, 0.5, 1.0.

use super::Check;
use crate::error::Result;
use crate::gas::Gas;
use crate::gas1d::{Duct, Limiter, Network, Node, Port, RadiationBc, Simulation};
use crate::radiation::{ls_end_correction, ls_reflection_magnitude};
use crate::spectrum::fourier_integral;

pub fn run() -> Result<Vec<Check>> {
    let (radius, length, dx) = (0.025, 2.0, 0.002);
    let (p0, t0) = (101_325.0, 293.15);
    let gas = Gas::perfect(1.4, 287.05);
    let rho0 = p0 / (gas.r() * t0);
    let c0 = gas.sound_speed(t0);
    let net = Network {
        gas,
        ducts: vec![Duct::conical(
            "pipe",
            length,
            2.0 * radius,
            2.0 * radius,
            dx,
        )],
        nodes: vec![
            Node::Wall(Port::start(0)),
            Node::Radiation(RadiationBc {
                port: Port::end(0),
                p_amb: p0,
                t_amb: t0,
                radius,
            }),
        ],
        limiter: Limiter::VanLeer,
        cfl: 0.8,
    };
    let (x0, sigma, amp) = (1.5, 0.01, 1.0);
    let mut sim = Simulation::new(net, |_, x| {
        let dp = amp * (-0.5 * ((x - x0) / sigma).powi(2)).exp();
        (rho0 + dp / (c0 * c0), dp / (rho0 * c0), p0 + dp)
    })?;
    // First reflection is back at the outlet after (length − x0 + 2·length)/c0.
    let t_end = 0.95 * (length - x0 + 2.0 * length) / c0;
    let dt = sim.stable_dt()?;
    let (mut t, mut wp, mut wm) = (Vec::new(), Vec::new(), Vec::new());
    while sim.t < t_end {
        sim.evaluate_faces()?;
        t.push(sim.t);
        wp.push(sim.nodes[1].w_plus);
        wm.push(sim.nodes[1].w_minus);
        sim.step(dt)?;
    }
    let mut checks = Vec::new();
    for ka in [0.1, 0.5, 1.0] {
        let omega = ka * c0 / radius;
        let r = fourier_integral(&t, &wm, omega) / fourier_integral(&t, &wp, omega);
        let l = -(-r).arg() / (2.0 * ka);
        let case = "Levine–Schwinger outlet";
        checks.push(Check::new(
            case,
            format!("|R| relative error at ka = {ka}"),
            (r.norm() / ls_reflection_magnitude(ka) - 1.0).abs(),
            0.01,
        ));
        checks.push(Check::new(
            case,
            format!("l/a relative error at ka = {ka}"),
            (l / ls_end_correction(ka) - 1.0).abs(),
            0.01,
        ));
    }
    Ok(checks)
}
