//! Steady pressure losses: the time-domain solution against each loss-carrying element's loss
//! model. Air at 293 K flows at a constant 0.06 kg/s (17 m/s in the 60.5 mm bore) through the
//! element — built from a project as in a solve (`tests/cases/straight_pipe.json`, its reducer
//! swapped for the element and both pipes 63.5 × 1.5 mm) — between frictionless pipes to a
//! non-reflecting outlet. Once steady, the stagnation-pressure drop between pipe cells either
//! side (five cells out where a junction's boundary cells sit) must match the model within 2 %:
//! - bend, 90°, R/D = 1.57 (the case's downpipe bend): `ΔP₀ = K ½ρu²`, Idelchik diagram 6-1
//!   (`geometry::bend_loss`);
//! - butterfly valve at 60° (δ = 30°): `ζ = 3.91` on the pipe velocity, Idelchik diagram 9-17
//!   (`elements::butterfly_loss`);
//! - catalyst, 400 cpsi, 6.5 mil walls, ∅100 × 100 mm brick, 50 mm cones: on the pipe velocity
//!   the cones' Crane losses (`geometry::cone_loss`); on the channel velocity, contraction into
//!   the channels `0.5 (1 − OFA)^0.75` (Idelchik), laminar friction `2 fRe μ u L/D_h²` with
//!   `f Re = 14.227` and entrance `K(∞) = 1.43` (Shah & London 1978), Borda–Carnot expansion
//!   `(1 − OFA)²` out.
//!
//! The steep catalyst cones (43° included, four cells at 15 mm) also check that the scheme
//! adds no total-pressure loss of its own there. Published catalyst pressure-drop data were not
//! available to this build, so the catalyst row checks the solver's implementation of the
//! model built from the cited laminar-channel results.

use std::f64::consts::{FRAC_PI_2, PI};

use serde_json::{Value, json};

use super::Check;
use super::grid::STRAIGHT_PIPE;
use crate::elements::{SQUARE_CHANNEL_FRE, SQUARE_CHANNEL_K_INF, butterfly_loss};
use crate::engine::EvoState;
use crate::error::Result;
use crate::gas::Gas;
use crate::gas1d::{CharacteristicBc, MassFlowBc, Node, Signal, Simulation};
use crate::geometry::{bend_loss, cone_loss};
use crate::model::{self, Model, SourceInputs};
use crate::project::Project;

const P0: f64 = 101_325.0;
const T0: f64 = 293.15;
const MDOT: f64 = 0.06;
/// Settling time, s: many transits of the 3.4 m system and one flow-through.
const SETTLE: f64 = 0.4;
/// Cells between an element and the pipe cell probed beside it.
const MARGIN: usize = 5;

/// The straight-pipe case, mid-pipe at the downpipe's bore, with its reducer replaced by
/// `element` if given.
pub(super) fn project(element: Option<Value>) -> Result<Project> {
    let mut p: Value = serde_json::from_str(STRAIGHT_PIPE).expect("valid case JSON");
    let sys = &mut p["system"];
    if let Some(mut el) = element {
        let els = sys["elements"].as_array_mut().expect("elements");
        let old = els
            .iter_mut()
            .find(|e| e["id"] == "reducer")
            .expect("reducer");
        for key in ["id", "position_mm", "axis"] {
            el[key] = old[key].clone();
        }
        *old = el;
    }
    sys["routes"][1]["pipe"] = sys["routes"][0]["pipe"].clone();
    Project::from_json(&p.to_string())
}

/// Solves the project's network to steady state under a constant air flow.
fn steady(project: &Project) -> Result<(Model, Simulation)> {
    let gas = Gas::perfect(1.4, 287.05);
    let src = SourceInputs {
        rpm: 3000.0,
        evo: EvoState {
            pressure_pa: 3e5,
            temperature_k: 1200.0,
        },
        p_init: P0,
        t_init: T0,
    };
    let mut m = model::build(project, gas.clone(), project.solver.dx_mm * 1e-3, src)?;
    let net = &mut m.network;
    for d in &mut net.ducts {
        d.friction &= d.channel.is_some();
    }
    let port = net.nodes[m.source_node].ports()[0];
    net.nodes[m.source_node] = Node::MassFlow(MassFlowBc {
        port,
        mdot: Signal::Harmonics {
            mean: MDOT,
            omega: 0.0,
            terms: Vec::new(),
        },
        t0: T0,
    });
    for o in &m.outlets {
        let port = net.nodes[o.node].ports()[0];
        net.nodes[o.node] = Node::Characteristic(CharacteristicBc {
            port,
            p_ref: P0,
            t_ref: T0,
            incoming: Signal::Zero,
        });
    }
    let rho = P0 / (gas.r() * T0);
    let (ducts, share) = (net.ducts.clone(), m.flow_share.clone());
    let mut sim = Simulation::new(net.clone(), |d, x| {
        let i = ((x / ducts[d].dx) as usize).min(ducts[d].n() - 1);
        let area = ducts[d].volume[i] / ducts[d].dx;
        (rho, share[d] * MDOT / (rho * area), P0)
    })?;
    while sim.t < SETTLE {
        let dt = sim.stable_dt()?;
        sim.step(dt)?;
    }
    Ok((m, sim))
}

/// Stagnation pressure, density and velocity of a cell.
fn stagnation(sim: &Simulation, duct: usize, cell: usize) -> (f64, f64, f64) {
    let p = sim.prim(duct, cell);
    (p.p + 0.5 * p.rho * p.u * p.u, p.rho, p.u)
}

/// Relative error of the drop from route 0's duct to route 1's, `MARGIN` cells either side of
/// the element, against `expected(ρ, u)` with the upstream state.
fn across_element(m: &Model, sim: &Simulation, expected: impl Fn(f64, f64) -> f64) -> f64 {
    let (a, b) = (m.route_duct[0].0, m.route_duct[1].0);
    let (p_a, rho, u) = stagnation(sim, a, sim.net.ducts[a].n() - 1 - MARGIN);
    let (p_b, _, _) = stagnation(sim, b, MARGIN);
    ((p_a - p_b) / expected(rho, u) - 1.0).abs()
}

pub fn run() -> Result<Vec<Check>> {
    let case = "Steady element losses (air, 0.06 kg/s)";
    let mut checks = Vec::new();

    // Bend: the merged downpipe/mid-pipe duct, probes next to the arc's loss cells (no
    // junction there).
    let bend = project(None)?;
    let (m, sim) = steady(&bend)?;
    let d = m.route_duct[0].0;
    let k = &sim.net.ducts[d].k_loss;
    let first = k.iter().position(|&x| x > 0.0).expect("bend loss cells");
    let last = k.iter().rposition(|&x| x > 0.0).expect("bend loss cells");
    let (p_a, rho, u) = stagnation(&sim, d, first - 1);
    let (p_b, _, _) = stagnation(&sim, d, last + 1);
    let route = &bend.system.routes[0];
    let r_over_d = route.bend_radius_mm[0].expect("case bend radius") * 1e-3 / route.pipe.id_m();
    let expected = bend_loss(FRAC_PI_2, r_over_d) * 0.5 * rho * u * u;
    checks.push(Check::new(
        case,
        format!("bend 90°, R/D = {r_over_d:.2}: ΔP₀ relative error"),
        ((p_a - p_b) / expected - 1.0).abs(),
        0.02,
    ));

    let valve = project(Some(json!({ "type": "valve", "angle_deg": 60.0 })))?;
    let (m, sim) = steady(&valve)?;
    checks.push(Check::new(
        case,
        "butterfly valve at 60°: ΔP₀ relative error",
        across_element(&m, &sim, |rho, u| butterfly_loss(60.0) * 0.5 * rho * u * u),
        0.02,
    ));

    let (cpsi, wall, length, diameter, cone) = (400.0, 0.1651e-3, 0.1, 0.1, 0.05);
    let cat = project(Some(json!({
        "type": "catalyst",
        "cpsi": cpsi,
        "cell_wall_mm": wall * 1e3,
        "brick_length_mm": length * 1e3,
        "brick_diameter_mm": diameter * 1e3,
        "inlet_cone_mm": cone * 1e3,
        "outlet_cone_mm": cone * 1e3
    })))?;
    let (m, sim) = steady(&cat)?;
    let d_pipe = cat.system.routes[0].pipe.id_m();
    let pitch = 25.4e-3 / f64::sqrt(cpsi);
    let w = pitch - wall;
    let ofa = (w / pitch).powi(2);
    let mu = sim.net.gas.viscosity(T0);
    let k_cones = cone_loss(d_pipe, diameter, cone) + cone_loss(diameter, d_pipe, cone);
    checks.push(Check::new(
        case,
        format!("catalyst {cpsi} cpsi, OFA {ofa:.3}: ΔP₀ relative error"),
        across_element(&m, &sim, |rho, u_pipe| {
            let u = MDOT / (rho * ofa * PI / 4.0 * diameter * diameter);
            let k = 0.5 * (1.0 - ofa).powf(0.75) + SQUARE_CHANNEL_K_INF + (1.0 - ofa).powi(2);
            k_cones * 0.5 * rho * u_pipe * u_pipe
                + k * 0.5 * rho * u * u
                + 2.0 * SQUARE_CHANNEL_FRE * mu * u * length / (w * w)
        }),
        0.02,
    ));
    Ok(checks)
}
