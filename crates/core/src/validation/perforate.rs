//! Perforate impedance: Sullivan & Crocker (1978) concentric-tube resonator, configuration 1.
//!
//! Geometry as reported for configuration 1 in the secondary literature: inner tube 49 mm,
//! shell 164 mm, length 257 mm, wall 0.81 mm, holes 2.49 mm, porosity 3.9 %; air at 293 K, no
//! flow. Reference: Sullivan & Crocker's coupled equations solved exactly — for
//! `y = (p₁, u₁, p₂, u₂)`
//! ```text
//! dp₁/dx = −jωρ u₁,   du₁/dx = −jω p₁/(ρc²) − (4/d₁) u_t,
//! dp₂/dx = −jωρ u₂,   du₂/dx = −jω p₂/(ρc²) + (π d₁/S₂) u_t,   u_t = (p₁ − p₂)/z_p,
//! z_p = ρc (0.006 + j k (t + 0.75 d_h)) / σ
//! ```
//! so `y(L) = e^{AL} y(0)`, with rigid annulus ends, a unit incident wave and an anechoic
//! outlet. (Their published curve was not available to this build; their own model evaluated
//! exactly stands in for it.) The four-pole solver (per-cell coupling) and the time-domain
//! solver (per-cell implicit perforate flow) must reproduce its TL within 2 dB below 1 kHz.
//! With the annulus packed (100 kg/m³ of 12 µm fibre, `gas1d::porous`), which the reference
//! does not cover, the two solvers must agree with each other within the same 2 dB.

use std::f64::consts::PI;

use rustfft::num_complex::Complex64 as C;

use super::Check;
use crate::elements::fill_properties;
use crate::error::Result;
use crate::fourpole::{self, MeanState};
use crate::gas::Gas;
use crate::gas1d::{
    CharacteristicBc, Duct, JunctionBc, JunctionKind, Limiter, Network, Node, Perforate, Porous,
    Port, Signal, Simulation,
};
use crate::spectrum::fourier_integral;

const P0: f64 = 101_325.0;
const T_AIR: f64 = 293.15;
const D1: f64 = 0.049;
const D2: f64 = 0.164;
const LENGTH: f64 = 0.257;
const WALL: f64 = 0.81e-3;
const HOLE: f64 = 2.49e-3;
const POROSITY: f64 = 0.039;
const DX: f64 = 0.004;

type M4 = [[C; 4]; 4];

fn mat_mul(a: &M4, b: &M4) -> M4 {
    let mut c = [[C::new(0.0, 0.0); 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            for k in 0..4 {
                c[i][j] += a[i][k] * b[k][j];
            }
        }
    }
    c
}

/// `e^M` by scaling and squaring with a 24-term Taylor series.
fn expm(m: &M4) -> M4 {
    let norm = m.iter().flatten().map(|z| z.norm()).sum::<f64>();
    let s = (norm.max(1.0).log2().ceil() as i32 + 1).max(0);
    let scale = 0.5f64.powi(s);
    let a: M4 = m.map(|row| row.map(|z| z * scale));
    let mut term = [[C::new(0.0, 0.0); 4]; 4];
    let mut sum = [[C::new(0.0, 0.0); 4]; 4];
    for k in 0..4 {
        term[k][k] = C::new(1.0, 0.0);
        sum[k][k] = C::new(1.0, 0.0);
    }
    for n in 1..24 {
        term = mat_mul(&term, &a).map(|row| row.map(|z| z / n as f64));
        for i in 0..4 {
            for j in 0..4 {
                sum[i][j] += term[i][j];
            }
        }
    }
    for _ in 0..s {
        sum = mat_mul(&sum, &sum);
    }
    sum
}

/// Transmission loss of the resonator from the exact coupled solution, dB.
pub fn reference_tl(f: f64) -> f64 {
    let gas = Gas::perfect(1.4, 287.05);
    let rho = P0 / (gas.r() * T_AIR);
    let c = gas.sound_speed(T_AIR);
    let w = 2.0 * PI * f;
    let k = w / c;
    let j = C::new(0.0, 1.0);
    let zp = rho * c * (0.006 + j * k * (WALL + 0.75 * HOLE)) / POROSITY;
    let s2 = PI / 4.0 * (D2 * D2 - D1 * D1);
    let (g1, g2) = (4.0 / D1, PI * D1 / s2);
    let zero = C::new(0.0, 0.0);
    let a: M4 = [
        [zero, -j * w * rho, zero, zero],
        [-j * w / (rho * c * c) - g1 / zp, zero, g1 / zp, zero],
        [zero, zero, zero, -j * w * rho],
        [g2 / zp, zero, -j * w / (rho * c * c) - g2 / zp, zero],
    ];
    let e = expm(&a.map(|row| row.map(|z| z * LENGTH)));
    // Unknowns p₁(0), u₁(0), p₂(0); u₂(0) = 0. Conditions: incident wave p₁ + ρc u₁ = 2 at
    // x = 0, anechoic p₁ − ρc u₁ = 0 and rigid u₂ = 0 at x = L.
    let z0 = rho * c;
    let row = |r: usize| [e[r][0], e[r][1], e[r][2]];
    let (r0, r1, r3) = (row(0), row(1), row(3));
    let m = [
        [C::new(1.0, 0.0), C::new(z0, 0.0), zero],
        [r0[0] - z0 * r1[0], r0[1] - z0 * r1[1], r0[2] - z0 * r1[2]],
        r3,
    ];
    let rhs = [C::new(2.0, 0.0), zero, zero];
    let x = solve3(m, rhs);
    let p_out = r0[0] * x[0] + r0[1] * x[1] + r0[2] * x[2];
    -20.0 * p_out.norm().log10()
}

fn solve3(mut m: [[C; 3]; 3], mut b: [C; 3]) -> [C; 3] {
    for col in 0..3 {
        let piv = (col..3)
            .max_by(|&a, &b| m[a][col].norm().total_cmp(&m[b][col].norm()))
            .expect("rows");
        m.swap(col, piv);
        b.swap(col, piv);
        for r in col + 1..3 {
            let f = m[r][col] / m[col][col];
            let pivot = m[col];
            for (x, v) in m[r][col..].iter_mut().zip(&pivot[col..]) {
                *x -= f * v;
            }
            let v = b[col];
            b[r] -= f * v;
        }
    }
    let mut x = [C::new(0.0, 0.0); 3];
    for r in (0..3).rev() {
        let s: C = (r + 1..3).map(|c| m[r][c] * x[c]).sum();
        x[r] = (b[r] - s) / m[r][r];
    }
    x
}

fn rig(fill: Option<Porous>) -> (Network, usize, usize) {
    let gas = Gas::perfect(1.4, 287.05);
    let s2 = fill.map_or(1.0, |f| f.porosity) * PI / 4.0 * (D2 * D2 - D1 * D1);
    let mut inner = Duct::conical("perforated tube", LENGTH, D1, D1, DX);
    inner.perforate = Some(Perforate {
        partner: 3,
        porosity: POROSITY,
        hole_diameter: HOLE,
        thickness: WALL,
        perimeter: PI * D1,
    });
    let d_ann = (4.0 * s2 / PI).sqrt();
    let mut annulus = Duct::conical("annulus", LENGTH, d_ann, d_ann, DX);
    annulus.porous = fill;
    let pulse = Signal::Gaussian {
        amplitude: 2.0,
        t0: 6.0 * 1e-4,
        sigma: 1e-4,
    };
    let net = Network {
        gas,
        ducts: vec![
            Duct::conical("inlet", 1.0, D1, D1, DX),
            Duct::conical("outlet", 1.0, D1, D1, DX),
            inner,
            annulus,
        ],
        nodes: vec![
            Node::Junction(JunctionBc {
                ports: vec![Port::end(0), Port::start(2)],
                kind: JunctionKind::AreaChange,
            }),
            Node::Junction(JunctionBc {
                ports: vec![Port::end(2), Port::start(1)],
                kind: JunctionKind::AreaChange,
            }),
            Node::Wall(Port::start(3)),
            Node::Wall(Port::end(3)),
            Node::Characteristic(CharacteristicBc {
                port: Port::start(0),
                p_ref: P0,
                t_ref: T_AIR,
                incoming: pulse,
            }),
            Node::Characteristic(CharacteristicBc {
                port: Port::end(1),
                p_ref: P0,
                t_ref: T_AIR,
                incoming: Signal::Zero,
            }),
        ],
        limiter: Limiter::default(),
        cfl: 0.8,
    };
    (net, 4, 5)
}

/// Four-pole and time-domain TL of the rig at `freqs`.
fn tl(fill: Option<Porous>, freqs: &[f64]) -> Result<(Vec<f64>, Vec<f64>)> {
    let (net, inlet, outlet) = rig(fill);
    let mean = MeanState::quiescent(&net, P0, T_AIR);
    let fp = freqs
        .iter()
        .map(|&f| fourpole::transmission_loss(&net, &mean, 2.0 * PI * f, inlet, outlet))
        .collect::<Result<_>>()?;
    // Time domain: incident pulse, record until the resonator has rung down.
    let gas = net.gas.clone();
    let rho = P0 / (gas.r() * T_AIR);
    let mut sim = Simulation::new(net.clone(), |_, _| (rho, 0.0, P0))?;
    let dt = sim.stable_dt()?;
    let Node::Characteristic(src) = &net.nodes[inlet] else {
        unreachable!()
    };
    let (mut t, mut w_in, mut w_out) = (Vec::new(), Vec::new(), Vec::new());
    while sim.t < 0.15 {
        sim.evaluate_faces()?;
        t.push(sim.t);
        w_in.push(src.incoming.at(sim.t));
        w_out.push(2.0 * (sim.nodes[outlet].faces[0].p - P0));
        sim.step(dt)?;
    }
    let td = freqs
        .iter()
        .map(|&f| {
            let w = 2.0 * PI * f;
            20.0 * (fourier_integral(&t, &w_in, w) / fourier_integral(&t, &w_out, w))
                .norm()
                .log10()
        })
        .collect();
    Ok((fp, td))
}

pub fn run() -> Result<Vec<Check>> {
    let freqs: Vec<f64> = (10..=100).map(|k| 10.0 * k as f64).collect();
    let max_diff = |a: &[f64], b: &[f64]| {
        a.iter()
            .zip(b)
            .map(|(x, y)| (x - y).abs())
            .fold(0.0, f64::max)
    };
    let reference: Vec<f64> = freqs.iter().map(|&f| reference_tl(f)).collect();
    let (fp, td) = tl(None, &freqs)?;
    let (fp_fill, td_fill) = tl(Some(fill_properties(100.0, 12e-6)), &freqs)?;
    let case = "Perforate (Sullivan–Crocker config. 1)";
    Ok(vec![
        Check::new(
            case,
            "four-pole max |TL error|, dB (100–1000 Hz)",
            max_diff(&fp, &reference),
            2.0,
        ),
        Check::new(
            case,
            "time-domain max |TL error|, dB (100–1000 Hz)",
            max_diff(&td, &reference),
            2.0,
        ),
        Check::new(
            case,
            "packed: time-domain vs four-pole max |ΔTL|, dB (100–1000 Hz)",
            max_diff(&td_fill, &fp_fill),
            2.0,
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expm_of_diagonal_and_nilpotent_matrices() {
        let z = C::new(0.0, 0.0);
        let mut a: M4 = [[z; 4]; 4];
        a[0][0] = C::new(0.3, 2.0);
        a[1][1] = C::new(-4.0, 0.5);
        a[2][3] = C::new(7.0, -1.0);
        let e = expm(&a);
        assert!((e[0][0] - a[0][0].exp()).norm() < 1e-12);
        assert!((e[1][1] - a[1][1].exp()).norm() < 1e-12);
        // Nilpotent block: e^N = I + N.
        assert!((e[2][3] - a[2][3]).norm() < 1e-12 && (e[2][2] - 1.0).norm() < 1e-12);
    }
}
