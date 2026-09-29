//! Builds the time-domain network from a project. Routes joined end to start through a
//! continuous-area element (a tapered area change, or one between equal bores) form one
//! finite-volume duct with a piecewise diameter profile, so cones sit in the second-order
//! interior of the scheme; every other element becomes a node. Bends add Idelchik loss
//! coefficients over their arcs.

use std::collections::HashMap;

use crate::engine::EvoState;
use crate::error::{Error, Result};
use crate::gas::Gas;
use crate::gas1d::nodes::JunctionKind;
use crate::gas1d::{Duct, End, JunctionBc, Manifold, Network, Node, Port, RadiationBc, SourceBc};
use crate::geometry::{Piece, Vec3, bend_loss, centreline, scale};
use crate::project::{ElementKind, PortRef, Project, WallThermal};

/// Network plus the bookkeeping needed to read results back onto the project.
#[derive(Clone, Debug)]
pub struct Model {
    pub network: Network,
    /// Duct index and start offset (m) of each route, in project order.
    pub route_duct: Vec<(usize, f64)>,
    pub source_node: usize,
    pub outlets: Vec<OutletInfo>,
    /// Steady mass-flow share of each duct (fraction of the engine flow), for initial conditions.
    pub flow_share: Vec<f64>,
    /// Modelling limitations that affect accuracy, reported with every result.
    pub warnings: Vec<String>,
}

/// Cones resolved by fewer cells than this carry a warning: the grid study in
/// `validation::grid` shows mean-pressure errors near 1 % below it.
const MIN_CONE_CELLS: f64 = 6.0;

#[derive(Clone, Debug)]
pub struct OutletInfo {
    pub element: String,
    pub node: usize,
    /// Tip position, m.
    pub position: Vec3,
    /// Flow direction leaving the tip.
    pub axis: Vec3,
    /// Inner area at the tip, m².
    pub area: f64,
}

/// Engine operating inputs the network needs.
#[derive(Clone, Copy, Debug)]
pub struct SourceInputs {
    pub rpm: f64,
    pub evo: EvoState,
    /// Initial manifold pressure, Pa, and temperature, K.
    pub p_init: f64,
    pub t_init: f64,
}

pub fn build(project: &Project, gas: Gas, dx: f64, src: SourceInputs) -> Result<Model> {
    let routes = &project.system.routes;
    let lines = routes
        .iter()
        .map(|r| {
            centreline(&project.route_points(r)?, &project.route_radii(r))
                .map_err(|e| Error::invalid(format!("route '{}': {e}", r.id)))
        })
        .collect::<Result<Vec<_>>>()?;
    let route_ending_at = |pr: &PortRef| routes.iter().position(|r| &r.to == pr);
    let route_starting_at = |pr: &PortRef| routes.iter().position(|r| &r.from == pr);

    // Continuous joints: route `a` ends at the element's `in`, route `b` starts at its `out`,
    // and the bore does not step.
    let mut next: Vec<Option<(usize, f64)>> = vec![None; routes.len()];
    let mut has_prev = vec![false; routes.len()];
    let mut merged = std::collections::HashSet::new();
    for el in &project.system.elements {
        if let ElementKind::AreaChange { taper_length_mm } = el.kind {
            let pin = PortRef {
                element: el.id.clone(),
                port: "in".into(),
            };
            let pout = PortRef {
                element: el.id.clone(),
                port: "out".into(),
            };
            if let (Some(a), Some(b)) = (route_ending_at(&pin), route_starting_at(&pout)) {
                let same_bore = (routes[a].pipe.id_m() - routes[b].pipe.id_m()).abs() < 1e-9;
                if taper_length_mm > 0.0 || same_bore {
                    next[a] = Some((b, taper_length_mm * 1e-3));
                    has_prev[b] = true;
                    merged.insert(el.id.clone());
                }
            }
        }
    }

    let mut warnings = Vec::new();
    for el in &project.system.elements {
        if let ElementKind::AreaChange { taper_length_mm } = el.kind
            && taper_length_mm > 0.0
            && taper_length_mm * 1e-3 / dx < MIN_CONE_CELLS
        {
            warnings.push(format!(
                "cone '{}' ({taper_length_mm} mm) spans {:.1} cells; mean pressure across it may be off by ~1 % at this grid",
                el.id,
                taper_length_mm * 1e-3 / dx
            ));
        }
    }
    let mut ducts = Vec::new();
    let mut route_duct = vec![(usize::MAX, 0.0); routes.len()];
    // Where each element port lands: (duct, end).
    let mut port_end: HashMap<PortRef, Port> = HashMap::new();
    for first in (0..routes.len()).filter(|&r| !has_prev[r]) {
        let idx = ducts.len();
        let mut segments = Vec::new();
        let mut losses = Vec::new();
        let mut labels = Vec::new();
        let (mut r, mut offset) = (first, 0.0);
        loop {
            let d = routes[r].pipe.id_m();
            route_duct[r] = (idx, offset);
            labels.push(routes[r].id.clone());
            segments.push((lines[r].length, d, d));
            for piece in &lines[r].pieces {
                if let Piece::Arc {
                    s0,
                    length,
                    angle,
                    radius,
                    ..
                } = piece
                {
                    losses.push((
                        offset + s0,
                        offset + s0 + length,
                        bend_loss(*angle, radius / d),
                    ));
                }
            }
            offset += lines[r].length;
            match next[r] {
                Some((b, taper)) if route_duct[b].0 == usize::MAX => {
                    if taper > 0.0 {
                        segments.push((taper, d, routes[b].pipe.id_m()));
                        offset += taper;
                    }
                    r = b;
                }
                _ => break,
            }
        }
        port_end.insert(routes[first].from.clone(), Port::start(idx));
        port_end.insert(routes[r].to.clone(), Port::end(idx));
        let mut duct = Duct::from_profile(labels.join(" + "), &segments, dx);
        if project.solver.friction {
            duct = duct.with_friction(project.solver.wall_roughness_mm * 1e-3);
        }
        for (x0, x1, k) in losses {
            duct.add_loss(x0, x1, k);
        }
        if let WallThermal::Fixed { temperature_k } = project.solver.wall_thermal {
            duct.t_wall = Some(vec![temperature_k; duct.n()]);
        }
        ducts.push(duct);
    }
    if route_duct.iter().any(|d| d.0 == usize::MAX) {
        return Err(Error::invalid(
            "routes joined through area changes form a closed loop",
        ));
    }
    let port_of = |element: &str, port: &str| {
        port_end[&PortRef {
            element: element.into(),
            port: port.into(),
        }]
    };

    let mut nodes = Vec::new();
    let mut outlets = Vec::new();
    let mut source_node = usize::MAX;
    for el in &project.system.elements {
        match &el.kind {
            ElementKind::Source => {
                let port = port_of(&el.id, "out");
                let t = &project.turbine;
                let manifolds = t
                    .scrolls
                    .iter()
                    .map(|s| Manifold {
                        cylinders: s.cylinders.iter().map(|c| c - 1).collect(),
                        volume: s.manifold_volume_l * 1e-3,
                        turbine_area: s.nozzle_area_cm2 * 1e-4,
                    })
                    .collect();
                if t.scrolls.iter().flat_map(|s| &s.cylinders).any(|&c| c == 0) {
                    return Err(Error::invalid("turbine scroll cylinders are 1-based"));
                }
                source_node = nodes.len();
                nodes.push(Node::Source(Box::new(SourceBc::new(
                    port,
                    project.engine.geometry.clone(),
                    project.engine.valves.clone(),
                    src.evo,
                    project.engine.polytropic_exponent,
                    src.rpm,
                    manifolds,
                    t.extraction_factor,
                    &gas,
                    src.p_init,
                    src.t_init,
                )?)));
            }
            ElementKind::Outlet => {
                let port = port_of(&el.id, "in");
                let route = project
                    .system
                    .routes
                    .iter()
                    .position(|r| r.to.element == el.id || r.from.element == el.id)
                    .expect("validated");
                let r = &project.system.routes[route];
                let line = centreline(&project.route_points(r)?, &project.route_radii(r))?;
                let axis = if r.to.element == el.id {
                    line.end_direction()
                } else {
                    scale(unit_start(&line.points), -1.0)
                };
                let area = ducts[port.duct].end_area(port.end);
                outlets.push(OutletInfo {
                    element: el.id.clone(),
                    node: nodes.len(),
                    position: scale(el.position_mm, 1e-3),
                    axis,
                    area,
                });
                nodes.push(Node::Radiation(RadiationBc {
                    port,
                    p_amb: project.ambient.pressure_pa,
                    t_amb: project.ambient.temperature_k,
                    radius: (area / std::f64::consts::PI).sqrt(),
                }));
            }
            ElementKind::Cap => nodes.push(Node::Wall(port_of(&el.id, "in"))),
            ElementKind::AreaChange { taper_length_mm } => {
                if merged.contains(&el.id) {
                    continue;
                }
                let (pin, pout) = (port_of(&el.id, "in"), port_of(&el.id, "out"));
                if *taper_length_mm > 0.0 {
                    // A cone whose pipes do not run in→out: its own duct between two joints.
                    let d_in = diameter(&ducts[pin.duct], pin.end);
                    let d_out = diameter(&ducts[pout.duct], pout.end);
                    let cone = ducts.len();
                    ducts.push(Duct::conical(
                        format!("{} (cone)", el.id),
                        taper_length_mm * 1e-3,
                        d_in,
                        d_out,
                        dx,
                    ));
                    nodes.push(junction(pin, Port::start(cone)));
                    nodes.push(junction(Port::end(cone), pout));
                } else {
                    nodes.push(junction(pin, pout));
                }
            }
        }
    }
    let flow_share = flow_shares(&ducts, &nodes, source_node);
    let network = Network {
        gas,
        ducts,
        nodes,
        limiter: project.solver.limiter,
        cfl: project.solver.cfl,
    };
    network.validate()?;
    Ok(Model {
        network,
        route_duct,
        source_node,
        outlets,
        flow_share,
        warnings,
    })
}

fn unit_start(points: &[Vec3]) -> Vec3 {
    crate::geometry::unit(crate::geometry::sub(points[1], points[0]))
}

fn diameter(duct: &Duct, end: End) -> f64 {
    (4.0 * duct.end_area(end) / std::f64::consts::PI).sqrt()
}

fn junction(a: Port, b: Port) -> Node {
    Node::Junction(JunctionBc {
        ports: vec![a, b],
        kind: JunctionKind::AreaChange,
    })
}

/// Steady mass-flow share of each duct on a tree rooted at the source: a duct carries the
/// fraction of outlets reachable through it. Dead ends carry nothing.
fn flow_shares(ducts: &[Duct], nodes: &[Node], source: usize) -> Vec<f64> {
    let mut node_of_port: HashMap<(usize, bool), usize> = HashMap::new();
    for (i, n) in nodes.iter().enumerate() {
        for p in n.ports() {
            node_of_port.insert((p.duct, p.end == End::End), i);
        }
    }
    // Outlets reachable from `node` without going back through `from_duct`.
    fn outlets_beyond(
        node: usize,
        from_duct: usize,
        nodes: &[Node],
        map: &HashMap<(usize, bool), usize>,
        count: &mut Vec<f64>,
        depth: usize,
    ) -> f64 {
        if depth > 10_000 {
            return 0.0;
        }
        if matches!(nodes[node], Node::Radiation(_)) {
            return 1.0;
        }
        let mut total = 0.0;
        for p in nodes[node].ports() {
            if p.duct == from_duct {
                continue;
            }
            let far = map[&(p.duct, p.end != End::End)];
            let n = outlets_beyond(far, p.duct, nodes, map, count, depth + 1);
            count[p.duct] = n;
            total += n;
        }
        total
    }
    let mut count = vec![0.0; ducts.len()];
    let total = outlets_beyond(source, usize::MAX, nodes, &node_of_port, &mut count, 0);
    count
        .iter()
        .map(|c| if total > 0.0 { c / total } else { 0.0 })
        .collect()
}
