//! Linear acoustic validation of the network elements in both solvers.
//!
//! Test rig (impedance-tube style): a `Characteristic` inlet sends an incident wave into a
//! 1 m inlet pipe, the element under test follows, and a 1 m outlet pipe ends in an anechoic
//! `Characteristic` node. Gas at rest. Transmission loss `TL = 10 log₁₀(W_inc/W_trans)`:
//! - four-pole: `fourpole::transmission_loss`;
//! - time domain: a 2 Pa Gaussian incident wave (σ = 50 µs, flat to within 1 dB up to 1.5 kHz),
//!   Δx = 5 mm, recorded until the element's ringing has left through the anechoic ends;
//!   `TL(ω) = 20 log₁₀|W_in(ω)/W_out(ω)| + 10 log₁₀(S_in/S_out)` from Fourier integrals.
//!
//! References: sudden area change `TL = 10 log₁₀((1 + m)²/(4m))` (Munjal 2014, §3.2);
//! simple expansion chamber `TL = 10 log₁₀[1 + ¼(m − 1/m)² sin²(kL)]` (Davis et al. 1954,
//! NACA Report 1192); closed side branch notch at `c_b/(4L)` with `c_b` the branch gas;
//! Helmholtz resonator `f₀ = (c/2π)√(S_n/(V l_eff))`; T-junction of equal pipes
//! (Benson 1982): reflection −1/3, transmission 2/3.

use std::f64::consts::PI;

use super::Check;
use crate::error::Result;
use crate::fourpole::{self, MeanState};
use crate::gas::Gas;
use crate::gas1d::{
    CharacteristicBc, Duct, JunctionBc, JunctionKind, Limiter, Network, Node, Port, Signal,
    Simulation,
};
use crate::spectrum::fourier_integral;

const P0: f64 = 101_325.0;
const T_AIR: f64 = 293.15;
const DX: f64 = 0.005;
const SIGMA: f64 = 5e-5;

fn area(d: f64) -> f64 {
    PI / 4.0 * d * d
}

/// Inlet pipe (duct 0) and outlet pipe (duct 1) with their characteristic end nodes; the
/// element joins `Port::end(0)` to `Port::start(1)`.
struct Rig {
    net: Network,
    inlet: usize,
    outlets: Vec<usize>,
}

fn rig(
    gas: Gas,
    d_in: f64,
    d_out: f64,
    mut ducts: Vec<Duct>,
    mut nodes: Vec<Node>,
    t_ref: f64,
) -> Rig {
    let mut all = vec![
        Duct::conical("inlet", 1.0, d_in, d_in, DX),
        Duct::conical("outlet", 1.0, d_out, d_out, DX),
    ];
    all.append(&mut ducts);
    let incident = Signal::Gaussian {
        amplitude: 2.0,
        t0: 6.0 * SIGMA,
        sigma: SIGMA,
    };
    let inlet = nodes.len();
    nodes.push(Node::Characteristic(CharacteristicBc {
        port: Port::start(0),
        p_ref: P0,
        t_ref,
        incoming: incident,
    }));
    let outlet = nodes.len();
    nodes.push(Node::Characteristic(CharacteristicBc {
        port: Port::end(1),
        p_ref: P0,
        t_ref,
        incoming: Signal::Zero,
    }));
    Rig {
        net: Network {
            gas,
            ducts: all,
            nodes,
            limiter: Limiter::default(),
            cfl: 0.8,
        },
        inlet,
        outlets: vec![outlet],
    }
}

/// Time-domain response of a rig to its incident pulse: times, incident, returning and
/// transmitted characteristics (Pa).
struct Record {
    t: Vec<f64>,
    w_in: Vec<f64>,
    w_back: Vec<f64>,
    w_out: Vec<Vec<f64>>,
}

fn pulse(rig: &Rig, temps: &[f64], t_end: f64) -> Result<Record> {
    let gas = rig.net.gas.clone();
    let mut sim = Simulation::new(rig.net.clone(), |d, _| (P0 / (gas.r() * temps[d]), 0.0, P0))?;
    let dt = sim.stable_dt()?;
    let Node::Characteristic(src) = &rig.net.nodes[rig.inlet] else {
        unreachable!()
    };
    let mut rec = Record {
        t: Vec::new(),
        w_in: Vec::new(),
        w_back: Vec::new(),
        w_out: vec![Vec::new(); rig.outlets.len()],
    };
    while sim.t < t_end {
        sim.evaluate_faces()?;
        let w_in = src.incoming.at(sim.t);
        rec.t.push(sim.t);
        rec.w_in.push(w_in);
        rec.w_back
            .push(2.0 * (sim.nodes[rig.inlet].faces[0].p - P0) - w_in);
        for (k, &o) in rig.outlets.iter().enumerate() {
            rec.w_out[k].push(2.0 * (sim.nodes[o].faces[0].p - P0));
        }
        sim.step(dt)?;
    }
    Ok(rec)
}

fn td_tl(rig: &Rig, rec: &Record, f: f64) -> f64 {
    let w = 2.0 * PI * f;
    let s_in = rig.net.ducts[0].area_face[0];
    let s_out = *rig.net.ducts[1].area_face.last().unwrap();
    let ratio = fourier_integral(&rec.t, &rec.w_in, w) / fourier_integral(&rec.t, &rec.w_out[0], w);
    20.0 * ratio.norm().log10() + 10.0 * (s_in / s_out).log10()
}

fn fp_tl(rig: &Rig, mean: &MeanState, f: f64) -> Result<f64> {
    fourpole::transmission_loss(&rig.net, mean, 2.0 * PI * f, rig.inlet, rig.outlets[0])
}

/// Frequency of the TL maximum near `guess`, by golden-section search on `tl`.
fn tl_peak(mut tl: impl FnMut(f64) -> f64, guess: f64) -> f64 {
    let (mut a, mut b) = (0.8 * guess, 1.2 * guess);
    let g = 0.5 * (5f64.sqrt() - 1.0);
    let (mut x1, mut x2) = (b - g * (b - a), a + g * (b - a));
    let (mut f1, mut f2) = (tl(x1), tl(x2));
    for _ in 0..60 {
        if f1 > f2 {
            b = x2;
            x2 = x1;
            f2 = f1;
            x1 = b - g * (b - a);
            f1 = tl(x1);
        } else {
            a = x1;
            x1 = x2;
            f1 = f2;
            x2 = a + g * (b - a);
            f2 = tl(x2);
        }
    }
    0.5 * (a + b)
}

fn junction(ports: Vec<Port>, kind: JunctionKind) -> Node {
    Node::Junction(JunctionBc { ports, kind })
}

pub fn area_change() -> Result<Vec<Check>> {
    let (d1, d2) = (0.05, 0.1);
    let gas = Gas::perfect(1.4, 287.05);
    let r = rig(
        gas,
        d1,
        d2,
        vec![],
        vec![junction(
            vec![Port::end(0), Port::start(1)],
            JunctionKind::AreaChange,
        )],
        T_AIR,
    );
    let m = area(d2) / area(d1);
    let exact = 10.0 * ((1.0 + m).powi(2) / (4.0 * m)).log10();
    let mean = MeanState::quiescent(&r.net, P0, T_AIR);
    let rec = pulse(&r, &[T_AIR; 2], 0.03)?;
    let freqs = [100.0, 400.0, 800.0, 1200.0];
    let fp = freqs
        .iter()
        .map(|&f| fp_tl(&r, &mean, f).map(|v| (v - exact).abs()))
        .collect::<Result<Vec<_>>>()?;
    let td = freqs
        .iter()
        .map(|&f| (td_tl(&r, &rec, f) - exact).abs())
        .fold(0.0, f64::max);
    let case = "Sudden area change TL (Munjal §3.2)";
    Ok(vec![
        Check::new(
            case,
            "four-pole max |TL error|, dB (100–1200 Hz)",
            fp.iter().copied().fold(0.0, f64::max),
            0.3,
        ),
        Check::new(
            case,
            "time-domain max |TL error|, dB (100–1200 Hz)",
            td,
            0.3,
        ),
    ])
}

pub fn expansion_chamber() -> Result<Vec<Check>> {
    let (d, dc, l) = (0.05, 0.15, 0.3);
    let gas = Gas::perfect(1.4, 287.05);
    let c = gas.sound_speed(T_AIR);
    let chamber = Duct::conical("chamber", l, dc, dc, DX);
    let r = rig(
        gas,
        d,
        d,
        vec![chamber],
        vec![
            junction(vec![Port::end(0), Port::start(2)], JunctionKind::AreaChange),
            junction(vec![Port::end(2), Port::start(1)], JunctionKind::AreaChange),
        ],
        T_AIR,
    );
    let m = area(dc) / area(d);
    let davis = |f: f64| {
        let k = 2.0 * PI * f / c;
        10.0 * (1.0 + 0.25 * (m - 1.0 / m).powi(2) * (k * l).sin().powi(2)).log10()
    };
    let mean = MeanState::quiescent(&r.net, P0, T_AIR);
    let rec = pulse(&r, &[T_AIR; 3], 0.25)?;
    let (mut fp_err, mut td_err) = (0.0f64, 0.0f64);
    let mut f = 50.0;
    while f <= 1500.0 {
        fp_err = fp_err.max((fp_tl(&r, &mean, f)? - davis(f)).abs());
        td_err = td_err.max((td_tl(&r, &rec, f) - davis(f)).abs());
        f += 10.0;
    }
    let case = "Expansion chamber TL (Davis, NACA 1192)";
    Ok(vec![
        Check::new(
            case,
            "four-pole max |TL error|, dB (50–1500 Hz)",
            fp_err,
            1.0,
        ),
        Check::new(
            case,
            "time-domain max |TL error|, dB (50–1500 Hz)",
            td_err,
            1.0,
        ),
    ])
}

/// Main pipe with a closed side branch of length `l` at temperature `t_branch`.
fn stub_rig(gas: Gas, t_main: f64, l: f64) -> Rig {
    let d = 0.05;
    let stub = Duct::conical("stub", l, d, d, DX);
    rig(
        gas,
        d,
        d,
        vec![stub],
        vec![
            junction(
                vec![Port::end(0), Port::start(1), Port::start(2)],
                JunctionKind::ConstantPressure,
            ),
            Node::Wall(Port::end(2)),
        ],
        t_main,
    )
}

pub fn quarter_wave_stub() -> Result<Vec<Check>> {
    let gas = Gas::exhaust(1.87, 1.0)?;
    let l = 0.4;
    let mut checks = Vec::new();
    let case = "Quarter-wave stub notch";
    for (t_main, t_branch) in [(700.0, 700.0), (700.0, 450.0)] {
        let r = stub_rig(gas.clone(), t_main, l);
        let expected = gas.sound_speed(t_branch) / (4.0 * l);
        let mut mean = MeanState::quiescent(&r.net, P0, t_main);
        mean.cells[2] = MeanState::quiescent(&r.net, P0, t_branch).cells[2].clone();
        let fp = tl_peak(|f| fp_tl(&r, &mean, f).unwrap_or(f64::NAN), expected);
        let rec = pulse(&r, &[t_main, t_main, t_branch], 0.12)?;
        let td = tl_peak(|f| td_tl(&r, &rec, f), expected);
        let label = format!("main {t_main} K, branch {t_branch} K ({expected:.1} Hz)");
        checks.push(Check::new(
            case,
            format!("four-pole relative error, {label}"),
            (fp / expected - 1.0).abs(),
            0.01,
        ));
        checks.push(Check::new(
            case,
            format!("time-domain relative error, {label}"),
            (td / expected - 1.0).abs(),
            0.01,
        ));
    }
    Ok(checks)
}

pub fn helmholtz() -> Result<Vec<Check>> {
    let gas = Gas::perfect(1.4, 287.05);
    let c = gas.sound_speed(T_AIR);
    let d = 0.05;
    let mut checks = Vec::new();
    // (neck diameter, effective neck length, cavity volume, cavity diameter), m and m³.
    for (dn, ln, v, dc) in [
        (0.025, 0.05, 1.5e-3, 0.2),
        (0.035, 0.08, 3.0e-3, 0.25),
        (0.02, 0.04, 0.8e-3, 0.16),
    ] {
        let lc = v / area(dc);
        let neck = Duct::conical("neck", ln, dn, dn, 0.002);
        let cavity = Duct::conical("cavity", lc, dc, dc, 0.002);
        let r = rig(
            gas.clone(),
            d,
            d,
            vec![neck, cavity],
            vec![
                junction(
                    vec![Port::end(0), Port::start(1), Port::start(2)],
                    JunctionKind::ConstantPressure,
                ),
                junction(vec![Port::end(2), Port::start(3)], JunctionKind::AreaChange),
                Node::Wall(Port::end(3)),
            ],
            T_AIR,
        );
        let f0 = c / (2.0 * PI) * (area(dn) / (v * ln)).sqrt();
        let mean = MeanState::quiescent(&r.net, P0, T_AIR);
        let fp = tl_peak(|f| fp_tl(&r, &mean, f).unwrap_or(f64::NAN), f0);
        let rec = pulse(&r, &[T_AIR; 4], 0.25)?;
        let td = tl_peak(|f| td_tl(&r, &rec, f), f0);
        let case = "Helmholtz resonator f₀";
        let label = format!(
            "V = {:.1} L, neck {:.0} mm × {:.0} mm ({f0:.1} Hz)",
            v * 1e3,
            dn * 1e3,
            ln * 1e3
        );
        checks.push(Check::new(
            case,
            format!("four-pole relative error, {label}"),
            (fp / f0 - 1.0).abs(),
            0.01,
        ));
        checks.push(Check::new(
            case,
            format!("time-domain relative error, {label}"),
            (td / f0 - 1.0).abs(),
            0.01,
        ));
    }
    Ok(checks)
}

pub fn tee_junction() -> Result<Vec<Check>> {
    let gas = Gas::perfect(1.4, 287.05);
    let d = 0.05;
    let branch = Duct::conical("branch", 1.0, d, d, DX);
    let mut r = rig(
        gas,
        d,
        d,
        vec![branch],
        vec![
            junction(
                vec![Port::end(0), Port::start(1), Port::start(2)],
                JunctionKind::ConstantPressure,
            ),
            Node::Characteristic(CharacteristicBc {
                port: Port::end(2),
                p_ref: P0,
                t_ref: T_AIR,
                incoming: Signal::Zero,
            }),
        ],
        T_AIR,
    );
    r.outlets.push(1);
    let rec = pulse(&r, &[T_AIR; 3], 0.012)?;
    // The same rig with a plain pipe in place of the tee: dividing by its transmission removes
    // the 2 m of propagation. Reflected and transmitted waves also travel equal distances, so
    // R/T = −1/2 carries no propagation either.
    let plain = rig(
        Gas::perfect(1.4, 287.05),
        d,
        d,
        vec![],
        vec![junction(
            vec![Port::end(0), Port::start(1)],
            JunctionKind::AreaChange,
        )],
        T_AIR,
    );
    let rec_plain = pulse(&plain, &[T_AIR; 2], 0.012)?;
    let mut worst = (0.0f64, 0.0f64);
    for f in [200.0, 600.0, 1200.0] {
        let w = 2.0 * PI * f;
        let refl = fourier_integral(&rec.t, &rec.w_back, w);
        let trans = fourier_integral(&rec.t, &rec.w_out[0], w);
        let through = fourier_integral(&rec_plain.t, &rec_plain.w_out[0], w);
        worst.0 = worst.0.max((refl / trans + 0.5).norm() / 0.5);
        worst.1 = worst.1.max((trans / through - 2.0 / 3.0).norm() * 1.5);
    }
    let case = "T-junction of equal pipes (Benson)";
    Ok(vec![
        Check::new(
            case,
            "|R/T + 1/2| / (1/2), R = −1/3, T = 2/3",
            worst.0,
            0.01,
        ),
        Check::new(case, "|T/T_plain − 2/3| / (2/3)", worst.1, 0.01),
    ])
}

/// Four-pole natural frequencies of the open–open pipe of `duct::run`: minima of the system
/// determinant against the Levine–Schwinger-corrected reference.
pub fn open_pipe_fourpole() -> Result<Vec<Check>> {
    let (radius, length) = (0.025, 1.0);
    let gas = Gas::perfect(1.4, 287.05);
    let c = gas.sound_speed(T_AIR);
    let outlet = |port| {
        Node::Radiation(crate::gas1d::RadiationBc {
            port,
            p_amb: P0,
            t_amb: T_AIR,
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
            DX,
        )],
        nodes: vec![outlet(Port::start(0)), outlet(Port::end(0))],
        limiter: Limiter::default(),
        cfl: 0.8,
    };
    let mean = MeanState::quiescent(&net, P0, T_AIR);
    let expected = super::duct::open_open_resonances(length, radius, c, 5);
    let mut checks = Vec::new();
    for (i, e) in expected.iter().enumerate() {
        let f = tl_peak(
            |f| {
                -fourpole::determinant(&net, &mean, 2.0 * PI * f)
                    .map(|d| d.norm().ln())
                    .unwrap_or(f64::NAN)
            },
            *e,
        );
        checks.push(Check::new(
            "Open–open duct resonance",
            format!("four-pole mode {} relative error ({e:.2} Hz)", i + 1),
            (f / e - 1.0).abs(),
            0.005,
        ));
    }
    Ok(checks)
}
