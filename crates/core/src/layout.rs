//! What the 3D editor draws, from the centrelines the solvers use: every route's pipe as its
//! straights and arcs, and every element's ports and outer envelope. Millimetres, vehicle
//! coordinates (X forward, Y left, Z up, origin at the downpipe flange face).

use schemars::JsonSchema;
use serde::Serialize;

use crate::error::Result;
use crate::geometry::{Piece, Vec3, add, centreline, dot, scale, sub, unit};
use crate::project::{ElementKind, PipeSpec, PortRef, Project};

#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct Layout {
    pub routes: Vec<RouteLayout>,
    pub elements: Vec<ElementLayout>,
}

#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct RouteLayout {
    pub id: String,
    pub od_mm: f64,
    pub wall_mm: f64,
    pub material: String,
    /// Start port, via points, end port.
    pub points: Vec<Vec3>,
    /// Centreline radius of the bend at each via point.
    pub bend_radius_mm: Vec<f64>,
    pub pieces: Vec<PieceLayout>,
    /// Centreline length: the flow solver's x-axis along this route.
    pub length_mm: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PieceLayout {
    Straight {
        from: Vec3,
        to: Vec3,
    },
    /// Constant-radius arc from `from` to `to` about `centre`.
    Arc {
        from: Vec3,
        to: Vec3,
        centre: Vec3,
        radius_mm: f64,
        angle_deg: f64,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct ElementLayout {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub ports: Vec<PortLayout>,
    /// Outer envelope as coaxial frustums (cones and cylinders).
    pub body: Vec<Frustum>,
}

#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct PortLayout {
    pub name: String,
    pub position: Vec3,
}

#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct Frustum {
    pub from: Vec3,
    pub to: Vec3,
    pub d_from_mm: f64,
    pub d_to_mm: f64,
}

/// Outer shell of welded cans without a shell parameter, mm.
const CAN_WALL_MM: f64 = 1.2;

pub fn layout(project: &Project) -> Result<Layout> {
    let routes = project
        .system
        .routes
        .iter()
        .map(|r| {
            let points = project.route_points(r)?;
            let radii = project.route_radii(r);
            let line = centreline(&points, &radii)?;
            let mm = |p: Vec3| scale(p, 1e3);
            let pieces = line
                .pieces
                .iter()
                .map(|piece| match piece {
                    Piece::Straight { from, to, .. } => PieceLayout::Straight {
                        from: mm(*from),
                        to: mm(*to),
                    },
                    Piece::Arc {
                        angle,
                        radius,
                        vertex,
                        ..
                    } => {
                        let (p, i) = (points[*vertex], *vertex);
                        let d_in = unit(sub(p, points[i - 1]));
                        let d_out = unit(sub(points[i + 1], p));
                        let t = radius * (angle / 2.0).tan();
                        let from = sub(p, scale(d_in, t));
                        let inward = unit(sub(d_out, scale(d_in, dot(d_in, d_out))));
                        PieceLayout::Arc {
                            from: mm(from),
                            to: mm(add(p, scale(d_out, t))),
                            centre: mm(add(from, scale(inward, *radius))),
                            radius_mm: radius * 1e3,
                            angle_deg: angle.to_degrees(),
                        }
                    }
                })
                .collect();
            Ok(RouteLayout {
                id: r.id.clone(),
                od_mm: r.pipe.od_mm,
                wall_mm: r.pipe.wall_mm,
                material: r.pipe.material.clone(),
                points: points.iter().map(|&p| mm(p)).collect(),
                bend_radius_mm: radii.iter().map(|r| r * 1e3).collect(),
                pieces,
                length_mm: line.length * 1e3,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let elements = project
        .system
        .elements
        .iter()
        .map(|el| {
            let pipe_od =
                |port: &str| pipe_at(project, &el.id, port).map_or(50.0, |p: &PipeSpec| p.od_mm);
            let axis = unit(el.axis);
            let at = |len: f64| add(el.position_mm, scale(axis, len));
            let cyl = |from: Vec3, to: Vec3, d: f64| Frustum {
                from,
                to,
                d_from_mm: d,
                d_to_mm: d,
            };
            let body = match &el.kind {
                ElementKind::AreaChange { taper_length_mm } if *taper_length_mm > 0.0 => {
                    vec![Frustum {
                        from: el.position_mm,
                        to: at(*taper_length_mm),
                        d_from_mm: pipe_od("in"),
                        d_to_mm: pipe_od("out"),
                    }]
                }
                ElementKind::ExpansionChamber(c) => vec![cyl(
                    el.position_mm,
                    at(c.length_mm),
                    c.diameter_mm + 2.0 * c.shell_mm,
                )],
                ElementKind::Absorptive(a) => vec![cyl(
                    el.position_mm,
                    at(a.length_mm),
                    a.case_diameter_mm + 2.0 * a.shell_mm,
                )],
                ElementKind::Catalyst(c) => {
                    let can = c.brick_diameter_mm + 2.0 * CAN_WALL_MM;
                    let (a, b) = (c.inlet_cone_mm, c.inlet_cone_mm + c.brick_length_mm);
                    let end = b + c.outlet_cone_mm;
                    vec![
                        Frustum {
                            from: el.position_mm,
                            to: at(a),
                            d_from_mm: pipe_od("in"),
                            d_to_mm: can,
                        },
                        cyl(at(a), at(b), can),
                        Frustum {
                            from: at(b),
                            to: at(end),
                            d_from_mm: can,
                            d_to_mm: pipe_od("out"),
                        },
                    ]
                }
                ElementKind::QuarterWaveStub(s) => {
                    let dir = unit(s.direction);
                    let end = add(el.position_mm, scale(dir, s.length_mm));
                    vec![cyl(el.position_mm, end, s.id_mm + 2.0 * s.wall_mm)]
                }
                ElementKind::Helmholtz(h) => {
                    let dir = unit(h.direction);
                    let neck = add(el.position_mm, scale(dir, h.neck_length_mm));
                    let area = std::f64::consts::PI / 4.0 * h.cavity_diameter_mm.powi(2);
                    let cavity = add(neck, scale(dir, h.volume_l * 1e6 / area));
                    let wall = pipe_at(project, &el.id, "in").map_or(1.5, |p| p.wall_mm);
                    vec![
                        cyl(el.position_mm, neck, h.neck_id_mm + 2.0 * wall),
                        cyl(neck, cavity, h.cavity_diameter_mm + 2.0 * CAN_WALL_MM),
                    ]
                }
                ElementKind::Valve(_) => {
                    let d = pipe_od("in");
                    vec![cyl(at(-0.25 * d), at(0.25 * d), 1.2 * d)]
                }
                _ => Vec::new(),
            };
            ElementLayout {
                id: el.id.clone(),
                kind: serde_json::to_value(&el.kind).expect("elements serialise")["type"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                ports: el
                    .kind
                    .port_names()
                    .into_iter()
                    .filter_map(|name| {
                        el.port_position(&name)
                            .map(|position| PortLayout { name, position })
                    })
                    .collect(),
                body,
            }
        })
        .collect();
    Ok(Layout { routes, elements })
}

/// The pipe of the route joined at `element.port`.
fn pipe_at<'a>(project: &'a Project, element: &str, port: &str) -> Option<&'a PipeSpec> {
    let at = PortRef {
        element: element.into(),
        port: port.into(),
    };
    project
        .system
        .routes
        .iter()
        .find(|r| r.from == at || r.to == at)
        .map(|r| &r.pipe)
}
