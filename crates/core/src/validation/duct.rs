//! Lossless duct standing waves: open–open pipe resonances.
//!
//! A 1 m pipe of radius 25 mm, both ends radiating (Levine–Schwinger), air at 293.15 K, no
//! friction or heat transfer, Δx = 5 mm. An off-centre 1 Pa Gaussian bump rings the pipe for
//! 1 s; resonance frequencies are the peaks of the pressure spectrum 0.1 m from one end.
//! Reference: `fₙ = n c / (2 (L + 2 l(kₙa)))` with the Levine–Schwinger end correction at each
//! end, solved by fixed-point iteration because `l` depends on `fₙ`.

use super::Check;
use crate::error::Result;
use crate::gas::Gas;
use crate::gas1d::{Duct, Limiter, Network, Node, Port, RadiationBc, Simulation};
use crate::radiation::ls_end_correction;
use crate::spectrum::peak_frequencies;

/// Open–open resonances with Levine–Schwinger end corrections at both ends.
pub fn open_open_resonances(length: f64, radius: f64, c: f64, modes: usize) -> Vec<f64> {
    (1..=modes)
        .map(|n| {
            let mut f = n as f64 * c / (2.0 * length);
            for _ in 0..50 {
                let ka = 2.0 * std::f64::consts::PI * f * radius / c;
                f = n as f64 * c / (2.0 * (length + 2.0 * radius * ls_end_correction(ka)));
            }
            f
        })
        .collect()
}

pub fn run() -> Result<Vec<Check>> {
    let (radius, length, dx) = (0.025, 1.0, 0.005);
    let (p0, t0) = (101_325.0, 293.15);
    let gas = Gas::perfect(1.4, 287.05);
    let rho0 = p0 / (gas.r() * t0);
    let c0 = gas.sound_speed(t0);
    let outlet = |port| {
        Node::Radiation(RadiationBc {
            port,
            p_amb: p0,
            t_amb: t0,
            radius,
        })
    };
    let net = Network {
        gas,
        ducts: vec![Duct::conical(
            "pipe",
            length,
            2.0 * radius,
            2.0 * radius,
            dx,
        )],
        nodes: vec![outlet(Port::start(0)), outlet(Port::end(0))],
        limiter: Limiter::default(),
        cfl: 0.8,
    };
    let mut sim = Simulation::new(net, |_, x| {
        let dp = (-0.5 * ((x - 0.3) / 0.02f64).powi(2)).exp();
        (rho0 + dp / (c0 * c0), 0.0, p0 + dp)
    })?;
    let dt = sim.stable_dt()?;
    let probe = (0.1 / dx) as usize;
    let steps = (1.0 / dt) as usize;
    let mut signal = Vec::with_capacity(steps);
    for _ in 0..steps {
        signal.push(sim.prim(0, probe).p);
        sim.step(dt)?;
    }
    let expected = open_open_resonances(length, radius, c0, 5);
    let found = peak_frequencies(&signal, dt, 8, &expected);
    Ok(expected
        .iter()
        .zip(&found)
        .enumerate()
        .map(|(i, (e, f))| {
            Check::new(
                "Open–open duct resonance",
                format!("mode {} relative error ({e:.2} Hz)", i + 1),
                (f / e - 1.0).abs(),
                0.005,
            )
        })
        .collect())
}
