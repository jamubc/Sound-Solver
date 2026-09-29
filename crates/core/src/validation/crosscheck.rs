//! Time-domain against four-pole on linear reference cases.
//!
//! A rigid mass-flow source (mean 0.02 kg/s at 800 K, harmonics of 66.7 Hz — the firing
//! frequency at 2000 rpm — at 1 % of the mean) drives exhaust gas with mean flow through a
//! pipe system to a radiating outlet. Walls adiabatic, friction on. The time-domain solver
//! runs to periodicity; the four-pole solver is linearised about the time-averaged state of
//! that run. The radiated level at each harmonic up to 1 kHz must agree within 2 dB
//! (directive: on linear cases disagreement is a bug). Systems: a plain 3 m pipe; the pipe with
//! an expansion chamber; the pipe with a quarter-wave stub.

use std::f64::consts::PI;

use rustfft::num_complex::Complex64;

use super::Check;
use crate::error::Result;
use crate::fourpole::{self, CellMean, Drive, MeanState};
use crate::gas::Gas;
use crate::gas1d::{
    Duct, JunctionBc, JunctionKind, Limiter, MassFlowBc, Network, Node, Port, RadiationBc, Signal,
    Simulation,
};
use crate::spectrum::fft;

const P0: f64 = 101_325.0;
const T_GAS: f64 = 800.0;
const MDOT: f64 = 0.02;
const F0: f64 = 2000.0 / 60.0 * 2.0;
const HARMONICS: usize = 15;
const DX: f64 = 0.01;

fn pipe(label: &str, length: f64, d: f64) -> Duct {
    let mut duct = Duct::conical(label, length, d, d, DX);
    duct.friction = true;
    duct.roughness = 3e-5;
    duct
}

/// Harmonic `k` of the source: amplitude 1 % of the mean, phase `0.7 k` rad.
fn source() -> Signal {
    Signal::Harmonics {
        mean: MDOT,
        omega: 2.0 * PI * F0,
        terms: (1..=HARMONICS)
            .map(|k| (0.01 * MDOT, 0.7 * k as f64))
            .collect(),
    }
}

/// Network with source at `Port::start(0)` and outlet at `outlet`.
fn network(mut ducts: Vec<Duct>, mut nodes: Vec<Node>, outlet: Port) -> (Network, usize, usize) {
    let src = nodes.len();
    nodes.push(Node::MassFlow(MassFlowBc {
        port: Port::start(0),
        mdot: source(),
        t0: T_GAS,
    }));
    let out = nodes.len();
    let radius = (ducts[outlet.duct].end_area(outlet.end) / PI).sqrt();
    nodes.push(Node::Radiation(RadiationBc {
        port: outlet,
        p_amb: P0,
        t_amb: 293.15,
        radius,
    }));
    ducts.iter_mut().for_each(|d| d.friction = true);
    let net = Network {
        gas: Gas::exhaust(1.87, 1.0).expect("valid composition"),
        ducts,
        nodes,
        limiter: Limiter::default(),
        cfl: 0.8,
    };
    (net, src, out)
}

/// Largest |ΔL| over the harmonics between the solvers' outlet volume velocities (the
/// receiver level is proportional to it harmonic by harmonic).
fn compare(net: &Network, src: usize, out: usize, flow_share: &[f64]) -> Result<f64> {
    let gas = net.gas.clone();
    let rho = P0 / (gas.r() * T_GAS);
    let mut sim = Simulation::new(net.clone(), |d, _| {
        (
            rho,
            flow_share[d] * MDOT / (rho * net.ducts[d].area_face[0]),
            P0,
        )
    })?;
    let period = 1.0 / F0;
    let n = 512;
    let ts = period / n as f64;
    let port = net.nodes[out].ports()[0];
    let area = net.ducts[port.duct].end_area(port.end);
    // Settle for 60 periods (≈ 12 flow-through times), then record 8 periods and average the
    // cell states over them for the linearisation point.
    let (settle, record) = (60, 8);
    let mut q = Vec::with_capacity(record * n);
    let mut sums: Vec<Vec<[f64; 4]>> = net.ducts.iter().map(|d| vec![[0.0; 4]; d.n()]).collect();
    for p in 0..settle + record {
        for _ in 0..n {
            sim.evaluate_faces()?;
            if p >= settle {
                q.push(sim.nodes[out].faces[0].v * area);
                for (d, cells) in sums.iter_mut().enumerate() {
                    for (i, s) in cells.iter_mut().enumerate() {
                        let c = sim.prim(d, i);
                        s[0] += c.rho;
                        s[1] += c.u;
                        s[2] += c.t;
                        s[3] += 1.0;
                    }
                }
            }
            let sub = (ts / sim.stable_dt()?).ceil();
            for _ in 0..sub as usize {
                sim.step(ts / sub)?;
            }
        }
    }
    let spec = fft(&q);
    let total = q.len() as f64;
    let mean = MeanState {
        cells: sums
            .iter()
            .map(|d| {
                d.iter()
                    .map(|s| {
                        let t = s[2] / s[3];
                        CellMean {
                            rho: s[0] / s[3],
                            c: gas.sound_speed(t),
                            u: s[1] / s[3],
                            t,
                        }
                    })
                    .collect()
            })
            .collect(),
    };
    let mut worst = 0.0f64;
    for k in 1..=HARMONICS {
        let td = 2.0 * spec[record * k] / total;
        let omega = 2.0 * PI * F0 * k as f64;
        let drive = Drive::Norton {
            q: Complex64::from_polar(0.01 * MDOT, 0.7 * k as f64),
            y: Complex64::new(0.0, 0.0),
        };
        let sol = fourpole::solve(net, &mean, omega, &[(src, drive)])?;
        let fp = sol.volume_velocity_out(net, &mean, port);
        worst = worst.max(20.0 * (td.norm() / fp.norm()).log10().abs());
    }
    Ok(worst)
}

pub fn run() -> Result<Vec<Check>> {
    let d = 0.06;
    let case = "Time vs four-pole, linear cases";
    let mut checks = Vec::new();

    let (net, s, o) = network(vec![pipe("pipe", 3.0, d)], vec![], Port::end(0));
    checks.push(Check::new(
        case,
        "straight pipe: max |ΔL| over harmonics, dB",
        compare(&net, s, o, &[1.0])?,
        2.0,
    ));

    let (net, s, o) = network(
        vec![
            pipe("inlet", 1.5, d),
            pipe("chamber", 0.4, 0.16),
            pipe("outlet", 1.1, d),
        ],
        vec![
            Node::Junction(JunctionBc {
                ports: vec![Port::end(0), Port::start(1)],
                kind: JunctionKind::AreaChange,
            }),
            Node::Junction(JunctionBc {
                ports: vec![Port::end(1), Port::start(2)],
                kind: JunctionKind::AreaChange,
            }),
        ],
        Port::end(2),
    );
    checks.push(Check::new(
        case,
        "expansion chamber: max |ΔL| over harmonics, dB",
        compare(&net, s, o, &[1.0; 3])?,
        2.0,
    ));

    let (net, s, o) = network(
        vec![
            pipe("inlet", 1.5, d),
            pipe("outlet", 1.5, d),
            pipe("stub", 0.5, d),
        ],
        vec![
            Node::Junction(JunctionBc {
                ports: vec![Port::end(0), Port::start(1), Port::start(2)],
                kind: JunctionKind::ConstantPressure,
            }),
            Node::Wall(Port::end(2)),
        ],
        Port::end(1),
    );
    checks.push(Check::new(
        case,
        "quarter-wave stub: max |ΔL| over harmonics, dB",
        compare(&net, s, o, &[1.0, 1.0, 0.0])?,
        2.0,
    ));
    Ok(checks)
}
