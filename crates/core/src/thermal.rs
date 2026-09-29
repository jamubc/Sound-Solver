//! Quasi-steady thermal model: mean gas and wall temperature along every duct.
//!
//! Flowing ducts, marched along the mean flow from the source (mixing by mass at junctions):
//! ```text
//! ṁ c_p dT/dx = −q',   q' = h_i π D_i (T − T_wi) = (T_wi − T_wo) 2π k / ln(D_o/D_i)
//!                          = π D_o [h_o (T_wo − T_∞) + ε σ (T_wo⁴ − T_∞⁴)]
//! ```
//! `h_i`: Gnielinski at the mean-flow Reynolds number (`gas1d::wall::nusselt`); pulsation
//! enhancement is not modelled and sits inside the uncertainty band. `h_o`: cross-flow over a
//! cylinder, Churchill–Bernstein (1977) forced convection at the underbody air speed combined
//! with Churchill–Chu (1975) natural convection as `Nu = (Nu_f³ + Nu_n³)^{1/3}`; air at the
//! film temperature. Radiation to surroundings at ambient temperature (view factor 1, no heat
//! shields). Ducts without a wall (catalyst bricks in their insulated can, chamber annuli
//! inside the shell) lose no heat.
//!
//! Dead-ended branches (stubs, Helmholtz necks and cavities) hold stagnant gas at their own
//! wall temperature, set by conduction along the branch wall from the tee, where it takes the
//! main pipe's wall temperature, against external losses: a fin with adiabatic tip,
//! `T − T_∞ = (T_root − T_∞) cosh(m(L − y))/cosh(mL)`, `m = √(h_e P / (k A_c))`, `h_e` the
//! convective plus linearised radiative coefficient. Heat carried in by the oscillating flow at
//! the mouth is not modelled.
//!
//! Uncertainty: every heat-transfer coefficient scaled by `1 ± 0.3` gives the band reported
//! with the profile (`t_gas_band`), and through `c = √(γRT)` the band on tuned lengths.

use std::collections::VecDeque;
use std::f64::consts::PI;

use crate::error::{Error, Result};
use crate::gas1d::{End, Network, Node, WallSpec, wall};

const STEFAN_BOLTZMANN: f64 = 5.670_374_419e-8;
const G: f64 = 9.806_65;
/// Relative scaling of all heat-transfer coefficients for the uncertainty band.
pub const H_UNCERTAINTY: f64 = 0.3;

/// Operating conditions of the thermal solve.
#[derive(Clone, Copy, Debug)]
pub struct ThermalInputs {
    /// Mean engine mass flow, kg/s.
    pub mass_flow: f64,
    /// Mean stagnation temperature leaving the source, K.
    pub t_source: f64,
    pub p_amb: f64,
    pub t_amb: f64,
    /// Air speed across the pipes, m/s.
    pub air_speed: f64,
}

/// Mean temperatures of every cell.
#[derive(Clone, Debug)]
pub struct ThermalProfile {
    /// Gas temperature, K, per duct per cell.
    pub t_gas: Vec<Vec<f64>>,
    /// Inner wall surface temperature, K.
    pub t_wall: Vec<Vec<f64>>,
    /// Gas temperature with heat transfer 30 % weaker (hot) and stronger (cold).
    pub t_gas_band: [Vec<Vec<f64>>; 2],
    /// Mean mass flow along each duct's +x, kg/s.
    pub mass_flow: Vec<f64>,
    /// BLAKE3 of the inputs and the nominal profile.
    pub hash: String,
}

/// Air properties at `t`: (kinematic viscosity, conductivity, Prandtl).
fn air(t: f64, p: f64) -> (f64, f64, f64) {
    const MU_REF: f64 = 1.716e-5;
    let mu = MU_REF * (t / 273.15).powf(1.5) * (273.15 + 110.4) / (t + 110.4);
    let pr = 0.71;
    (mu / (p / (287.05 * t)), mu * 1007.0 / pr, pr)
}

/// External film coefficient of a horizontal cylinder of diameter `d` at surface temperature
/// `tw`, W/(m² K).
fn h_outer(d: f64, tw: f64, inp: &ThermalInputs) -> f64 {
    let tf = 0.5 * (tw + inp.t_amb);
    let (nu, k, pr) = air(tf, inp.p_amb);
    let re = inp.air_speed * d / nu;
    let nu_f = if re > 0.0 {
        0.3 + 0.62 * re.sqrt() * pr.cbrt() / (1.0 + (0.4 / pr).powf(2.0 / 3.0)).powf(0.25)
            * (1.0 + (re / 282_000.0).powf(0.625)).powf(0.8)
    } else {
        0.0
    };
    let ra = G * (tw - inp.t_amb).abs() / tf * d.powi(3) / (nu * nu / pr);
    let nu_n = (0.60
        + 0.387 * ra.powf(1.0 / 6.0) / (1.0 + (0.559 / pr).powf(9.0 / 16.0)).powf(8.0 / 27.0))
    .powi(2);
    (nu_f.powi(3) + nu_n.powi(3)).cbrt() * k / d
}

/// Heat loss per unit length and wall temperatures `(q', T_wi, T_wo)` for gas at `tg`.
fn wall_balance(
    tg: f64,
    h_i: f64,
    di: f64,
    w: &WallSpec,
    inp: &ThermalInputs,
    scale: f64,
) -> (f64, f64, f64) {
    let r_cond = (w.outer_diameter / di).ln() / (2.0 * PI * w.conductivity);
    let loss_out = |two: f64| {
        PI * w.outer_diameter
            * (scale * h_outer(w.outer_diameter, two, inp) * (two - inp.t_amb)
                + w.emissivity * STEFAN_BOLTZMANN * (two.powi(4) - inp.t_amb.powi(4)))
    };
    // Outer wall temperature by bisection: the loss outward must equal the flow through the
    // inner film and the wall, both monotone in T_wo.
    let (mut lo, mut hi) = (inp.t_amb, tg);
    for _ in 0..80 {
        let two = 0.5 * (lo + hi);
        let q = loss_out(two);
        let twi = two + q * r_cond;
        let q_in = h_i * PI * di * (tg - twi);
        if q_in > q { lo = two } else { hi = two }
    }
    let two = 0.5 * (lo + hi);
    let q = loss_out(two);
    (q, two + q * r_cond, two)
}

/// Solves the profile. `source` is the node the mean flow starts from.
pub fn solve(net: &Network, source: usize, inp: &ThermalInputs) -> Result<ThermalProfile> {
    let flow = mean_flows(net, source, inp.mass_flow)?;
    let run = |scale: f64| march(net, source, inp, &flow, scale);
    let (t_gas, t_wall) = run(1.0);
    let hot = run(1.0 - H_UNCERTAINTY).0;
    let cold = run(1.0 + H_UNCERTAINTY).0;
    let mass_flow = flow
        .iter()
        .map(|f| {
            f.map(|(m, forward)| if forward { m } else { -m })
                .unwrap_or(0.0)
        })
        .collect();
    let summary = format!(
        "{inp:?}{:?}",
        t_gas
            .iter()
            .map(|d| d
                .iter()
                .map(|t| (t * 10.0).round() as i64)
                .collect::<Vec<_>>())
            .collect::<Vec<_>>()
    );
    Ok(ThermalProfile {
        t_gas,
        t_wall,
        t_gas_band: [hot, cold],
        mass_flow,
        hash: blake3::hash(summary.as_bytes()).to_hex()[..16].to_string(),
    })
}

/// Mean mass flow of each duct and whether it runs along +x: a tree walk from the source that
/// splits flow in proportion to the outlets reachable through each branch. `None`: dead end.
pub fn mean_flows(net: &Network, source: usize, total: f64) -> Result<Vec<Option<(f64, bool)>>> {
    let node_at = |duct: usize, end: End| {
        net.nodes
            .iter()
            .position(|n| n.ports().iter().any(|p| p.duct == duct && p.end == end))
            .expect("validated network")
    };
    fn outlets(
        net: &Network,
        node: usize,
        from: usize,
        node_at: &dyn Fn(usize, End) -> usize,
        depth: usize,
    ) -> usize {
        if matches!(net.nodes[node], Node::Radiation(_)) {
            return 1;
        }
        if depth > net.ducts.len() {
            return 0;
        }
        net.nodes[node]
            .ports()
            .iter()
            .filter(|p| p.duct != from)
            .map(|p| {
                let far = node_at(
                    p.duct,
                    if p.end == End::Start {
                        End::End
                    } else {
                        End::Start
                    },
                );
                outlets(net, far, p.duct, node_at, depth + 1)
            })
            .sum()
    }
    let mut flow = vec![None; net.ducts.len()];
    let total_outlets = outlets(net, source, usize::MAX, &node_at, 0);
    if total_outlets == 0 {
        return Err(Error::invalid("no outlet is reachable from the source"));
    }
    let mut queue = VecDeque::from([(source, usize::MAX)]);
    while let Some((node, from)) = queue.pop_front() {
        for p in net.nodes[node].ports() {
            if p.duct == from || flow[p.duct].is_some() {
                continue;
            }
            let far = node_at(
                p.duct,
                if p.end == End::Start {
                    End::End
                } else {
                    End::Start
                },
            );
            let n = outlets(net, far, p.duct, &node_at, 0);
            if n > 0 {
                flow[p.duct] = Some((total * n as f64 / total_outlets as f64, p.end == End::Start));
                queue.push_back((far, p.duct));
            }
        }
    }
    Ok(flow)
}

type Profile = (Vec<Vec<f64>>, Vec<Vec<f64>>);

fn march(
    net: &Network,
    source: usize,
    inp: &ThermalInputs,
    flow: &[Option<(f64, bool)>],
    scale: f64,
) -> Profile {
    let gas = &net.gas;
    let mut t_gas: Vec<Vec<f64>> = net.ducts.iter().map(|d| vec![f64::NAN; d.n()]).collect();
    let mut t_wall = t_gas.clone();
    // Outlet temperature and flow arriving at each node from marched ducts.
    let mut arriving: Vec<(f64, f64)> = vec![(0.0, 0.0); net.nodes.len()];
    let node_at = |duct: usize, end: End| {
        net.nodes
            .iter()
            .position(|n| n.ports().iter().any(|p| p.duct == duct && p.end == end))
            .expect("validated")
    };
    let mut queue = VecDeque::from([source]);
    let mut done = vec![false; net.ducts.len()];
    while let Some(node) = queue.pop_front() {
        let t_in = if node == source {
            inp.t_source
        } else {
            arriving[node].0 / arriving[node].1
        };
        for p in net.nodes[node].ports() {
            let Some((mdot, forward)) = flow[p.duct] else {
                continue;
            };
            // Leaving this node along the duct?
            if done[p.duct] || forward != (p.end == End::Start) {
                continue;
            }
            done[p.duct] = true;
            let duct = &net.ducts[p.duct];
            let n = duct.n();
            let mut tg = t_in;
            for step in 0..n {
                let i = if forward { step } else { n - 1 - step };
                let di = duct.diameter[i];
                let area = duct.volume[i] / duct.dx;
                let (q, twi) = match &duct.wall {
                    Some(w) if mdot > 0.0 => {
                        let mu = gas.viscosity(tg);
                        let re = mdot * di / (area * mu);
                        let h_i =
                            scale * wall::nusselt(re, gas.prandtl()) * gas.conductivity(tg) / di;
                        let (q, twi, _) = wall_balance(tg, h_i, di, w, inp, scale);
                        (q, twi)
                    }
                    _ => (0.0, tg),
                };
                let dt = q * duct.dx / (mdot * gas.cp(tg));
                t_gas[p.duct][i] = tg - 0.5 * dt;
                t_wall[p.duct][i] = twi;
                tg -= dt;
            }
            let far = node_at(
                p.duct,
                if p.end == End::Start {
                    End::End
                } else {
                    End::Start
                },
            );
            arriving[far].0 += mdot * tg;
            arriving[far].1 += mdot;
            queue.push_back(far);
        }
    }
    // Dead ends: fins rooted at the wall temperature where they meet marched ducts.
    let mut pending: Vec<usize> = (0..net.ducts.len())
        .filter(|&d| flow[d].is_none())
        .collect();
    let mut guard = 0;
    while !pending.is_empty() && guard < 4 * net.ducts.len() + 4 {
        guard += 1;
        pending.retain(|&d| {
            let duct = &net.ducts[d];
            // Root: the neighbour at either end that is already known.
            for end in [End::Start, End::End] {
                let node = node_at(d, end);
                let root = net.nodes[node]
                    .ports()
                    .iter()
                    .filter(|q| q.duct != d)
                    .find_map(|q| {
                        let k = net.ducts[q.duct].end_cell(q.end);
                        let (tw, tg) = (t_wall[q.duct][k], t_gas[q.duct][k]);
                        (!tw.is_nan()).then_some((tw, tg))
                    });
                let Some((tw_root, tg_root)) = root else {
                    continue;
                };
                let n = duct.n();
                let temps = match &duct.wall {
                    Some(w) => fin(
                        duct.dx * n as f64,
                        duct.diameter[0],
                        w,
                        tw_root,
                        inp,
                        scale,
                        n,
                    ),
                    None => vec![tg_root; n],
                };
                for (step, t) in temps.iter().enumerate() {
                    let i = if end == End::Start {
                        step
                    } else {
                        n - 1 - step
                    };
                    t_gas[d][i] = *t;
                    t_wall[d][i] = *t;
                }
                return false;
            }
            true
        });
    }
    // Anything unreachable (should not happen in a validated network) stays at ambient.
    for d in 0..net.ducts.len() {
        for i in 0..net.ducts[d].n() {
            if t_gas[d][i].is_nan() {
                t_gas[d][i] = inp.t_amb;
                t_wall[d][i] = inp.t_amb;
            }
        }
    }
    (t_gas, t_wall)
}

/// Wall temperature at the `n` cell centres of a branch of length `l`: fin with adiabatic tip,
/// iterated so the radiative coefficient matches the local temperature.
fn fin(
    l: f64,
    di: f64,
    w: &WallSpec,
    t_root: f64,
    inp: &ThermalInputs,
    scale: f64,
    n: usize,
) -> Vec<f64> {
    let d_o = w.outer_diameter;
    let a_c = PI / 4.0 * (d_o * d_o - di * di);
    let per = PI * d_o;
    let mut t_ref = 0.5 * (t_root + inp.t_amb);
    let mut out = vec![t_root; n];
    for _ in 0..4 {
        let h_rad = w.emissivity
            * STEFAN_BOLTZMANN
            * (t_ref * t_ref + inp.t_amb * inp.t_amb)
            * (t_ref + inp.t_amb);
        let h_e = scale * h_outer(d_o, t_ref, inp) + h_rad;
        let m = (h_e * per / (w.conductivity * a_c)).sqrt();
        for (k, t) in out.iter_mut().enumerate() {
            let y = (k as f64 + 0.5) * l / n as f64;
            *t = inp.t_amb + (t_root - inp.t_amb) * (m * (l - y)).cosh() / (m * l).cosh();
        }
        t_ref = out.iter().sum::<f64>() / n as f64;
    }
    out
}
