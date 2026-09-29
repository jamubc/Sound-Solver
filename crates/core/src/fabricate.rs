//! Fabrication package: what a welder needs to build the system as modelled, from the same
//! centrelines the solvers use. Per route, its straights and bends in order; the straights
//! nested into stock lengths; a bend schedule as a bender reads it (feed, rotation between
//! bends, bend angle, centreline radius); a weld map; hanger positions. CSV and Markdown
//! writers produce the files placed beside the project.
//!
//! A bend is cut from the smallest stock mandrel bend (45°, 90°, 180°) that covers it, and uses
//! a stock bend whole when within `fabrication.stock_bend_tolerance_deg` of one. The rotation
//! between bends is the angle, about the straight joining them, from one bend's plane to the
//! next (right-handed about the flow direction). The first bend is placed by the direction it
//! turns toward, in vehicle coordinates. Lengths mm, angles degrees.

use schemars::JsonSchema;
use serde::Serialize;

use crate::error::{Error, Result};
use crate::geometry::{Piece, Vec3, add, centreline, cross, dot, norm, scale, sub, unit};
use crate::layout::CAN_WALL_MM;
use crate::project::{Element, ElementKind, Joint, MaterialFamily, PortRef, Project, Route};

/// Stock mandrel bend angles, degrees.
const STOCK_ANGLES: [f64; 3] = [45.0, 90.0, 180.0];
/// Stock centreline radii as multiples of the tube OD.
const STOCK_RADII: [f64; 3] = [1.0, 1.5, 2.0];
/// Straights shorter than this are bend-to-bend joints, mm.
const MIN_STRAIGHT_MM: f64 = 0.5;

#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct Package {
    pub routes: Vec<RouteParts>,
    /// Straights nested into stock lengths.
    pub sticks: Vec<Stick>,
    pub welds: Vec<Weld>,
    pub warnings: Vec<String>,
}

/// The parts of a route, or of an element's own tube (`stub/tube`, `helmholtz/neck`,
/// `helmholtz/can`), from its start.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct RouteParts {
    pub route: String,
    pub od_mm: f64,
    pub wall_mm: f64,
    pub material: String,
    pub start: Vec3,
    /// Flow direction at the start.
    pub start_direction: Vec3,
    pub end: Vec3,
    /// Centreline length.
    pub length_mm: f64,
    pub parts: Vec<Part>,
}

#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Part {
    Straight {
        id: String,
        length_mm: f64,
        /// What each end is welded or joined to.
        from: String,
        to: String,
        /// Hangers, mm from this straight's start.
        hangers_mm: Vec<f64>,
    },
    Bend {
        id: String,
        angle_deg: f64,
        clr_mm: f64,
        /// Stock bend it is cut from, degrees.
        stock_deg: f64,
        /// Used whole: no cut.
        stock_as_is: bool,
        /// From the previous bend's plane; absent for a route's first bend.
        rotation_deg: Option<f64>,
        /// Direction the tube turns toward (first bend's placement), unit vector.
        turns_toward: Vec3,
        /// Hangers, mm of arc from this bend's start.
        hangers_mm: Vec<f64>,
    },
    /// A tube off the main pipe (a stub, a Helmholtz neck), one end coped to saddle the main
    /// pipe. Cut to `length_mm` (its long side); the cope reaches `saddle_mm` below the crown,
    /// where the branch axis leaves the main pipe's outer wall and the modelled length starts.
    Branch {
        id: String,
        length_mm: f64,
        saddle_mm: f64,
        /// The main pipe the cope fits, and the angle from its flow direction to the branch.
        main_od_mm: f64,
        tee_angle_deg: f64,
        from: String,
        to: String,
    },
}

impl Part {
    pub fn id(&self) -> &str {
        match self {
            Part::Straight { id, .. } | Part::Bend { id, .. } | Part::Branch { id, .. } => id,
        }
    }
}

/// One stock length of tube and the straights cut from it.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct Stick {
    pub material: String,
    pub od_mm: f64,
    pub wall_mm: f64,
    pub cuts: Vec<String>,
    pub used_mm: f64,
    pub offcut_mm: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct Weld {
    pub id: String,
    pub route: String,
    pub joint: Joint,
    /// The two things joined (part ids or `element.port`).
    pub between: [String; 2],
    pub at: Vec3,
    /// Filler or method needed for the metals meeting here.
    pub note: Option<String>,
}

pub fn package(project: &Project) -> Result<Package> {
    let mut warnings = Vec::new();
    let mut routes = Vec::new();
    let mut welds: Vec<Weld> = Vec::new();
    for route in &project.system.routes {
        let parts = route_parts(project, route, &mut warnings)?;
        let port = |p: &PortRef| format!("{}.{}", p.element, p.port);
        let joint_at = |p: &PortRef, given: Option<Joint>| {
            given.unwrap_or(match project.element(&p.element).map(|e| &e.kind) {
                Some(ElementKind::Source) => Joint::Flange,
                _ => Joint::Butt,
            })
        };
        let note = |p: &PortRef| element_note(project, &p.element, &route.pipe.material);
        let mut add_weld = |route: &str, joint, between: [String; 2], at, note| {
            welds.push(Weld {
                id: format!("W{}", welds.len() + 1),
                route: route.into(),
                joint,
                between,
                at,
                note,
            })
        };
        let first = parts
            .parts
            .first()
            .map_or(port(&route.to), |p| p.id().to_string());
        add_weld(
            &route.id,
            joint_at(&route.from, route.start_joint),
            [port(&route.from), first],
            parts.start,
            note(&route.from),
        );
        for (pair, at) in parts.parts.windows(2).zip(joints_between(project, route)?) {
            add_weld(
                &route.id,
                Joint::Butt,
                [pair[0].id().into(), pair[1].id().into()],
                at,
                None,
            );
        }
        let last = parts
            .parts
            .last()
            .map_or(port(&route.from), |p| p.id().to_string());
        add_weld(
            &route.id,
            joint_at(&route.to, route.end_joint),
            [last, port(&route.to)],
            parts.end,
            note(&route.to),
        );
        routes.push(parts);
    }
    for el in &project.system.elements {
        for (tube, joints) in element_tubes(project, el)? {
            for (between, at, note) in joints {
                welds.push(Weld {
                    id: format!("W{}", welds.len() + 1),
                    route: tube.route.clone(),
                    joint: Joint::Butt,
                    between,
                    at,
                    note,
                });
            }
            routes.push(tube);
        }
    }
    let sticks = nest(project, &routes, &mut warnings);
    Ok(Package {
        routes,
        sticks,
        welds,
        warnings,
    })
}

/// A route's straights and bends in flow order.
fn route_parts(project: &Project, route: &Route, warnings: &mut Vec<String>) -> Result<RouteParts> {
    let points = project.route_points(route)?;
    let line = centreline(&points, &project.route_radii(route))?;
    let od = route.pipe.od_mm;
    let tolerance = project.fabrication.stock_bend_tolerance_deg;
    let mut parts: Vec<Part> = Vec::new();
    let mut previous_normal: Option<Vec3> = None;
    let (mut straights, mut bends) = (0, 0);
    let hangers_in = |s0: f64, length: f64| -> Vec<f64> {
        route
            .hangers_mm
            .iter()
            .map(|h| h * 1e-3 - s0)
            .filter(|&d| d >= 0.0 && d <= length)
            .map(|d| d * 1e3)
            .collect()
    };
    for piece in &line.pieces {
        match piece {
            Piece::Straight { s0, length, .. } => {
                if length * 1e3 < MIN_STRAIGHT_MM {
                    continue;
                }
                straights += 1;
                parts.push(Part::Straight {
                    id: format!("{}/S{straights}", route.id),
                    length_mm: length * 1e3,
                    from: String::new(),
                    to: String::new(),
                    hangers_mm: hangers_in(*s0, *length),
                });
            }
            Piece::Arc {
                s0,
                length,
                angle,
                radius,
                vertex,
            } => {
                bends += 1;
                let i = *vertex;
                let d_in = unit(sub(points[i], points[i - 1]));
                let d_out = unit(sub(points[i + 1], points[i]));
                let normal = unit(cross(d_in, d_out));
                let turns = unit(sub(d_out, scale(d_in, dot(d_in, d_out))));
                let rotation = previous_normal.map(|n| {
                    dot(cross(n, normal), d_in)
                        .atan2(dot(n, normal))
                        .to_degrees()
                });
                previous_normal = Some(normal);
                let deg = angle.to_degrees();
                let stock = STOCK_ANGLES
                    .iter()
                    .copied()
                    .find(|s| deg <= s + tolerance)
                    .unwrap_or(180.0);
                let id = format!("{}/B{bends}", route.id);
                let clr = radius * 1e3;
                if !STOCK_RADII.iter().any(|k| (clr - k * od).abs() <= 1.0) {
                    warnings.push(format!(
                        "{id}: centreline radius {clr:.0} mm is not a stock mandrel radius \
                         (1D, 1.5D, 2D = {:.0}, {:.0}, {:.0} mm)",
                        od,
                        1.5 * od,
                        2.0 * od
                    ));
                }
                parts.push(Part::Bend {
                    id,
                    angle_deg: deg,
                    clr_mm: clr,
                    stock_deg: stock,
                    stock_as_is: (deg - stock).abs() <= tolerance,
                    rotation_deg: rotation,
                    turns_toward: turns,
                    hangers_mm: hangers_in(*s0, *length),
                });
            }
        }
    }
    // What each straight mates to: its neighbours, or the element ports at the route's ends.
    let ids: Vec<String> = parts.iter().map(|p| p.id().to_string()).collect();
    let ends = [
        format!("{}.{}", route.from.element, route.from.port),
        format!("{}.{}", route.to.element, route.to.port),
    ];
    let n = parts.len();
    for (k, part) in parts.iter_mut().enumerate() {
        if let Part::Straight { from, to, .. } = part {
            *from = if k == 0 {
                ends[0].clone()
            } else {
                ids[k - 1].clone()
            };
            *to = if k + 1 == n {
                ends[1].clone()
            } else {
                ids[k + 1].clone()
            };
        }
    }
    let mm = |p: Vec3| scale(p, 1e3);
    Ok(RouteParts {
        route: route.id.clone(),
        od_mm: od,
        wall_mm: route.pipe.wall_mm,
        material: route.pipe.material.clone(),
        start: mm(points[0]),
        start_direction: unit(sub(points[1], points[0])),
        end: mm(points[points.len() - 1]),
        length_mm: line.length * 1e3,
        parts,
    })
}

/// Positions of the butt joints between consecutive parts of a route: every straight's ends
/// inside the route, bend-to-bend joints included.
fn joints_between(project: &Project, route: &Route) -> Result<Vec<Vec3>> {
    let points = project.route_points(route)?;
    let line = centreline(&points, &project.route_radii(route))?;
    let mut boundaries = Vec::new();
    let mut s = Vec::new();
    for piece in &line.pieces {
        let kept =
            !matches!(piece, Piece::Straight { length, .. } if length * 1e3 < MIN_STRAIGHT_MM);
        if kept {
            s.push(piece.s0() + piece.length());
        }
    }
    s.pop();
    // Arc-length positions back to points along the centreline.
    for target in s {
        boundaries.push(scale(line.point_at(target), 1e3));
    }
    Ok(boundaries)
}

/// Filler or method for the metals meeting at an element: the route's and the other routes'
/// materials there.
fn element_note(project: &Project, element: &str, material: &str) -> Option<String> {
    project
        .system
        .routes
        .iter()
        .filter(|r| r.from.element == element || r.to.element == element)
        .map(|r| r.pipe.material.as_str())
        .filter(|m| *m != material)
        .find_map(|m| weld_note(project, material, m))
}

/// The two things a joint joins, where, and its filler note.
type Joined = ([String; 2], Vec3, Option<String>);

/// Tubes an element adds off the main pipe, with their joints: a stub's tube, coped onto the
/// main pipe and capped; a Helmholtz neck, coped, into its cavity can (a tube of the cavity
/// bore between end plates, `CAN_WALL_MM` thick, the main pipe's material, as the solver has
/// it). Lengths as modelled start at the crown (`Part::Branch`).
fn element_tubes(project: &Project, el: &Element) -> Result<Vec<(RouteParts, Vec<Joined>)>> {
    let (direction, bore, length) = match &el.kind {
        ElementKind::QuarterWaveStub(s) => (s.direction, s.id_mm, s.length_mm),
        ElementKind::Helmholtz(h) => (h.direction, h.neck_id_mm, h.neck_length_mm),
        _ => return Ok(Vec::new()),
    };
    let main = project
        .system
        .routes
        .iter()
        .find(|r| r.to.element == el.id || r.from.element == el.id)
        .map(|r| &r.pipe)
        .ok_or_else(|| Error::invalid(format!("element '{}' is on no pipe", el.id)))?;
    let (wall, material) = match &el.kind {
        ElementKind::QuarterWaveStub(s) => (s.wall_mm, s.material.as_str()),
        _ => (main.wall_mm, main.material.as_str()),
    };
    let (a, d) = (unit(el.axis), unit(direction));
    let od = bore + 2.0 * wall;
    let (crown, lowest) = saddle(a, d, od / 2.0, main.od_mm / 2.0).ok_or_else(|| {
        Error::invalid(format!(
            "element '{}': a {od:.1} mm branch along {direction:?} cannot saddle the {:.1} mm pipe",
            el.id, main.od_mm
        ))
    })?;
    let at = |t: f64| add(el.position_mm, scale(d, t));
    let stub = matches!(el.kind, ElementKind::QuarterWaveStub(_));
    let name = format!("{}/{}", el.id, if stub { "tube" } else { "neck" });
    let end = if stub {
        "cap".to_string()
    } else {
        format!("{}/can", el.id)
    };
    let tube = RouteParts {
        route: name.clone(),
        od_mm: od,
        wall_mm: wall,
        material: material.into(),
        start: at(lowest),
        start_direction: d,
        end: at(crown + length),
        length_mm: length,
        parts: vec![Part::Branch {
            id: name.clone(),
            length_mm: crown + length - lowest,
            saddle_mm: crown - lowest,
            main_od_mm: main.od_mm,
            tee_angle_deg: dot(a, d).clamp(-1.0, 1.0).acos().to_degrees(),
            from: el.id.clone(),
            to: end.clone(),
        }],
    };
    let joints = vec![
        (
            [el.id.clone(), name.clone()],
            at(crown),
            weld_note(project, &main.material, material),
        ),
        ([name, end], at(crown + length), None),
    ];
    let mut tubes = vec![(tube, joints)];
    if let ElementKind::Helmholtz(h) = &el.kind {
        let can = format!("{}/can", el.id);
        let can_length =
            h.volume_l * 1e6 / (std::f64::consts::PI / 4.0 * h.cavity_diameter_mm.powi(2));
        let (near, far) = (crown + length, crown + length + can_length);
        let plate = || String::from("end plate");
        tubes.push((
            RouteParts {
                route: can.clone(),
                od_mm: h.cavity_diameter_mm + 2.0 * CAN_WALL_MM,
                wall_mm: CAN_WALL_MM,
                material: main.material.clone(),
                start: at(near),
                start_direction: d,
                end: at(far),
                length_mm: can_length,
                parts: vec![Part::Straight {
                    id: can.clone(),
                    length_mm: can_length,
                    from: plate(),
                    to: plate(),
                    hangers_mm: Vec::new(),
                }],
            },
            vec![
                ([can.clone(), plate()], at(near), None),
                ([can, plate()], at(far), None),
            ],
        ));
    }
    Ok(tubes)
}

/// Where a tube of outside radius `r`, its axis leaving the main pipe's axis (unit `a`) along
/// unit `d`, meets a main pipe of outside radius `big_r`: distances along `d` to the main
/// pipe's outer wall on the tube's axis (the crown) and on its lowest side line. `None` when
/// the tube runs along the main pipe or is wider than it.
fn saddle(a: Vec3, d: Vec3, r: f64, big_r: f64) -> Option<(f64, f64)> {
    let across = |x: Vec3| sub(x, scale(a, dot(x, a)));
    let d_across = across(d);
    let q = dot(d_across, d_across);
    if q < 1e-6 {
        return None;
    }
    let crown = big_r / q.sqrt();
    let u = unit(cross(d, a));
    let v = cross(d, u);
    let mut lowest = crown;
    // Each side line meets the main pipe where |across(w + t d)| = R.
    for k in 0..360 {
        let phi = (k as f64).to_radians();
        let w = across(add(scale(u, r * phi.cos()), scale(v, r * phi.sin())));
        let (b, c) = (dot(w, d_across), dot(w, w) - big_r * big_r);
        let disc = b * b - q * c;
        if disc < 0.0 {
            return None;
        }
        lowest = lowest.min((-b + disc.sqrt()) / q);
    }
    Some((crown, lowest))
}

/// What joining two materials needs, if anything beyond a like-for-like weld.
pub fn weld_note(project: &Project, a: &str, b: &str) -> Option<String> {
    let family = |id: &str| project.material(id).map(|m| m.family);
    let (fa, fb) = (family(a)?, family(b)?);
    use MaterialFamily::*;
    match (fa, fb) {
        (Titanium, Titanium) => None,
        (Titanium, _) | (_, Titanium) => Some(format!(
            "{a} to {b}: titanium does not fusion-weld to steel; use a V-band or slip joint"
        )),
        (FerriticStainless, AusteniticStainless) | (AusteniticStainless, FerriticStainless) => {
            Some(format!("{a} to {b}: dissimilar stainless, 309L filler"))
        }
        _ => None,
    }
}

/// Straights nested into stock lengths, first-fit decreasing per tube size and material.
fn nest(project: &Project, routes: &[RouteParts], warnings: &mut Vec<String>) -> Vec<Stick> {
    let f = &project.fabrication;
    let mut cuts: Vec<(&str, f64, f64, &str, f64)> = routes
        .iter()
        .flat_map(|r| {
            r.parts.iter().filter_map(move |p| match p {
                Part::Straight { id, length_mm, .. } | Part::Branch { id, length_mm, .. } => {
                    Some((
                        r.material.as_str(),
                        r.od_mm,
                        r.wall_mm,
                        id.as_str(),
                        *length_mm,
                    ))
                }
                Part::Bend { .. } => None,
            })
        })
        .collect();
    cuts.sort_by(|a, b| b.4.total_cmp(&a.4));
    let mut sticks: Vec<Stick> = Vec::new();
    for (material, od, wall, id, length) in cuts {
        if length > f.stock_length_mm {
            warnings.push(format!(
                "{id}: {length:.0} mm is longer than the {:.0} mm stock; join two lengths",
                f.stock_length_mm
            ));
            continue;
        }
        let fits = sticks.iter_mut().find(|s| {
            s.material == material
                && s.od_mm == od
                && s.wall_mm == wall
                && s.used_mm + f.kerf_mm + length <= f.stock_length_mm
        });
        match fits {
            Some(s) => {
                s.used_mm += f.kerf_mm + length;
                s.cuts.push(id.into());
            }
            None => sticks.push(Stick {
                material: material.into(),
                od_mm: od,
                wall_mm: wall,
                cuts: vec![id.into()],
                used_mm: length,
                offcut_mm: 0.0,
            }),
        }
    }
    for s in &mut sticks {
        s.offcut_mm = f.stock_length_mm - s.used_mm;
    }
    sticks
}

/// End point and flow direction of a route rebuilt from its parts as a bender would: feed each
/// straight, rotate by the rotation between bends, bend by the angle on the radius. The first
/// bend turns toward its `turns_toward`.
pub fn rebuild(parts: &RouteParts) -> (Vec3, Vec3) {
    let (mut at, mut dir) = (parts.start, parts.start_direction);
    let mut normal: Option<Vec3> = None;
    for part in &parts.parts {
        match part {
            Part::Straight { length_mm, .. } | Part::Branch { length_mm, .. } => {
                at = add(at, scale(dir, *length_mm))
            }
            Part::Bend {
                angle_deg,
                clr_mm,
                rotation_deg,
                turns_toward,
                ..
            } => {
                let n = match (normal, rotation_deg) {
                    (Some(n), Some(r)) => rotate(n, dir, r.to_radians()),
                    _ => unit(cross(dir, *turns_toward)),
                };
                let turns = cross(n, dir);
                let theta = angle_deg.to_radians();
                at = add(
                    at,
                    add(
                        scale(dir, clr_mm * theta.sin()),
                        scale(turns, clr_mm * (1.0 - theta.cos())),
                    ),
                );
                dir = unit(add(scale(dir, theta.cos()), scale(turns, theta.sin())));
                normal = Some(n);
            }
        }
    }
    (at, dir)
}

/// `v` rotated by `angle` about unit axis `k` (Rodrigues).
fn rotate(v: Vec3, k: Vec3, angle: f64) -> Vec3 {
    add(
        add(scale(v, angle.cos()), scale(cross(k, v), angle.sin())),
        scale(k, dot(k, v) * (1.0 - angle.cos())),
    )
}

/// Cut list CSV: every straight and bend with its tube, stock and neighbours.
pub fn cut_list_csv(p: &Package) -> String {
    let stick_of = |id: &str| {
        p.sticks
            .iter()
            .position(|s| s.cuts.iter().any(|c| c == id))
            .map_or(String::from("over-length"), |i| format!("#{}", i + 1))
    };
    let mut out = String::from(
        "part,kind,length_mm,length_in,angle_deg,clr_mm,od_mm,od_in,wall_mm,material,stock,from,to,hangers_mm,saddle_mm,saddle_od_mm\n",
    );
    for r in &p.routes {
        for part in &r.parts {
            let row = match part {
                Part::Straight {
                    id,
                    length_mm,
                    from,
                    to,
                    hangers_mm,
                } => format!(
                    "{id},straight,{length_mm:.1},{:.2},,,{od:.1},{:.2},{wall:.2},{mat},stick {},{from},{to},{},,",
                    length_mm / 25.4,
                    r.od_mm / 25.4,
                    stick_of(id),
                    join(hangers_mm),
                    od = r.od_mm,
                    wall = r.wall_mm,
                    mat = r.material,
                ),
                Part::Branch {
                    id,
                    length_mm,
                    saddle_mm,
                    main_od_mm,
                    tee_angle_deg,
                    from,
                    to,
                } => format!(
                    "{id},branch,{length_mm:.1},{:.2},{tee_angle_deg:.1},,{od:.1},{:.2},{wall:.2},{mat},stick {},{from},{to},,{saddle_mm:.1},{main_od_mm:.1}",
                    length_mm / 25.4,
                    r.od_mm / 25.4,
                    stick_of(id),
                    od = r.od_mm,
                    wall = r.wall_mm,
                    mat = r.material,
                ),
                Part::Bend {
                    id,
                    angle_deg,
                    clr_mm,
                    stock_deg,
                    stock_as_is,
                    hangers_mm,
                    ..
                } => format!(
                    "{id},bend,,,{angle_deg:.1},{clr_mm:.1},{od:.1},{:.2},{wall:.2},{mat},{stock_deg:.0}° stock {},,,{},,",
                    r.od_mm / 25.4,
                    if *stock_as_is { "as-is" } else { "cut" },
                    join(hangers_mm),
                    od = r.od_mm,
                    wall = r.wall_mm,
                    mat = r.material,
                ),
            };
            out.push_str(&row);
            out.push('\n');
        }
    }
    out
}

/// Bend schedule CSV: per route in flow order, feed before each bend, rotation, angle, radius.
pub fn bend_schedule_csv(p: &Package) -> String {
    let mut out = String::from(
        "bend,route,feed_mm,rotation_deg,angle_deg,clr_mm,clr_d,stock,first_bend_turns_toward\n",
    );
    for r in &p.routes {
        let mut feed = 0.0;
        for part in &r.parts {
            match part {
                Part::Straight { length_mm, .. } | Part::Branch { length_mm, .. } => {
                    feed += length_mm
                }
                Part::Bend {
                    id,
                    angle_deg,
                    clr_mm,
                    stock_deg,
                    stock_as_is,
                    rotation_deg,
                    turns_toward,
                    ..
                } => {
                    let toward = if rotation_deg.is_none() {
                        format!(
                            "{:.3} {:.3} {:.3}",
                            turns_toward[0], turns_toward[1], turns_toward[2]
                        )
                    } else {
                        String::new()
                    };
                    out.push_str(&format!(
                        "{id},{},{feed:.1},{},{angle_deg:.1},{clr_mm:.1},{:.2},{stock_deg:.0}° {},{toward}\n",
                        r.route,
                        rotation_deg.map_or(String::new(), |x| format!("{x:.1}")),
                        clr_mm / r.od_mm,
                        if *stock_as_is { "as-is" } else { "cut" },
                    ));
                    feed = 0.0;
                }
            }
        }
    }
    out
}

/// Weld map, Markdown: every joint, numbered, with its type, place and filler notes.
pub fn weld_map_markdown(project: &Project, p: &Package) -> String {
    let mut out = format!(
        "# Weld map: {}\n\nVehicle coordinates, mm (X forward, Y left, Z up, origin at the downpipe flange face).\n\n\
         | Joint | Route | Type | Between | x | y | z | Note |\n|---|---|---|---|---:|---:|---:|---|\n",
        project.name
    );
    for w in &p.welds {
        out.push_str(&format!(
            "| {} | {} | {} | {} – {} | {:.0} | {:.0} | {:.0} | {} |\n",
            w.id,
            w.route,
            match w.joint {
                Joint::Butt => "butt weld",
                Joint::Slip => "slip joint",
                Joint::VBand => "V-band",
                Joint::Flange => "flange",
            },
            w.between[0],
            w.between[1],
            w.at[0],
            w.at[1],
            w.at[2],
            w.note.as_deref().unwrap_or(""),
        ));
    }
    if !p.warnings.is_empty() {
        out.push_str("\n## Warnings\n\n");
        for w in &p.warnings {
            out.push_str(&format!("- {w}\n"));
        }
    }
    out
}

fn join(values: &[f64]) -> String {
    values
        .iter()
        .map(|v| format!("{v:.0}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The fabrication files, named `{stem}.{kind}`: cut list and bend schedule (CSV), weld map
/// (Markdown), the pipe runs as STEP and STL, and with a scan the clearance report.
pub fn files(
    project: &Project,
    stem: &str,
    scan: Option<&crate::scan::Mesh>,
) -> Result<Vec<(String, Vec<u8>)>> {
    let p = check(project)?;
    let mut files = vec![
        (
            format!("{stem}.cut-list.csv"),
            cut_list_csv(&p).into_bytes(),
        ),
        (
            format!("{stem}.bends.csv"),
            bend_schedule_csv(&p).into_bytes(),
        ),
        (
            format!("{stem}.welds.md"),
            weld_map_markdown(project, &p).into_bytes(),
        ),
        (
            format!("{stem}.step"),
            crate::solid::step(project)?.into_bytes(),
        ),
        (format!("{stem}.stl"), crate::solid::stl(project)?),
    ];
    if let Some(mesh) = scan {
        let report = crate::scan::clearance(project, mesh)?;
        files.push((
            format!("{stem}.clearance.md"),
            crate::scan::report_markdown(project, &report).into_bytes(),
        ));
    }
    Ok(files)
}

/// The package, or why the project cannot be built as drawn.
pub fn check(project: &Project) -> Result<Package> {
    let p = package(project)?;
    for r in &p.routes {
        let (end, _) = rebuild(r);
        let miss = norm(sub(end, r.end));
        if miss > 1.0 {
            return Err(Error::solver(format!(
                "route '{}' rebuilds {miss:.1} mm from its end",
                r.route
            )));
        }
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::grid::STRAIGHT_PIPE;

    /// The bend schedule, as written (0.1 mm, 0.1°), rebuilds every route of the reference car
    /// to within 5 mm of its end (acceptance: a cut list round-trips within 5 mm).
    #[test]
    fn schedule_rebuilds_the_routes() {
        let text = include_str!("../../../tests/cases/w205_stock.json");
        for project in [
            Project::from_json(text).unwrap(),
            Project::from_json(STRAIGHT_PIPE).unwrap(),
        ] {
            let p = package(&project).unwrap();
            for r in &p.routes {
                let mut written = r.clone();
                for part in &mut written.parts {
                    let round = |x: &mut f64| *x = (*x * 10.0).round() / 10.0;
                    match part {
                        Part::Straight { length_mm, .. } | Part::Branch { length_mm, .. } => {
                            round(length_mm)
                        }
                        Part::Bend {
                            angle_deg,
                            clr_mm,
                            rotation_deg,
                            ..
                        } => {
                            round(angle_deg);
                            round(clr_mm);
                            if let Some(x) = rotation_deg {
                                round(x);
                            }
                        }
                    }
                }
                let (end, _) = rebuild(&written);
                let miss = norm(sub(end, r.end));
                assert!(miss < 5.0, "{}: rebuilt end {miss:.2} mm off", r.route);
            }
        }
    }

    /// Acceptance: a stub's cut list, as written, gives back its modelled length within 5 mm,
    /// the cope included, for a tee square to the pipe and one leaning 30° downstream.
    #[test]
    fn stub_cut_list_round_trips() {
        let text = include_str!("../../../tests/cases/w205_stock.json");
        let mut project = Project::from_json(text).unwrap();
        let id = crate::edit::insert(&mut project, "mid-pipe", 300.0, "quarter_wave_stub").unwrap();
        for lean in [0.0, 30f64.to_radians()] {
            let el = project
                .system
                .elements
                .iter_mut()
                .find(|e| e.id == id)
                .unwrap();
            let flow = unit(el.axis);
            let ElementKind::QuarterWaveStub(s) = &mut el.kind else {
                unreachable!()
            };
            s.direction = add(scale([0.0, 0.0, -1.0], lean.cos()), scale(flow, lean.sin()));
            let length = s.length_mm;
            let csv = cut_list_csv(&check(&project).unwrap());
            let header: Vec<&str> = csv.lines().next().unwrap().split(',').collect();
            let row: Vec<&str> = csv
                .lines()
                .find(|l| l.starts_with(&format!("{id}/tube,")))
                .unwrap()
                .split(',')
                .collect();
            let column = |name: &str| {
                row[header.iter().position(|h| *h == name).unwrap()]
                    .parse::<f64>()
                    .unwrap()
            };
            let saddle = column("saddle_mm");
            assert!(
                saddle > 5.0,
                "the cope matters at this tolerance: {saddle} mm"
            );
            let built = column("length_mm") - saddle;
            assert!(
                (built - length).abs() < 5.0,
                "{built} mm built, {length} mm modelled"
            );
        }
    }

    /// A tube square to a pipe saddles it to `R − √(R² − r²)`.
    #[test]
    fn saddle_depth() {
        let (crown, lowest) = saddle([-1.0, 0.0, 0.0], [0.0, 0.0, -1.0], 26.5, 35.0).unwrap();
        assert!((crown - 35.0).abs() < 1e-12);
        assert!(
            (crown - lowest - (35.0 - (35.0f64.powi(2) - 26.5f64.powi(2)).sqrt())).abs() < 1e-9
        );
        assert!(
            saddle([-1.0, 0.0, 0.0], [0.0, 0.0, -1.0], 40.0, 35.0).is_none(),
            "wider"
        );
    }

    #[test]
    fn joints_and_notes() {
        let mut project = Project::from_json(STRAIGHT_PIPE).unwrap();
        project.system.routes[1].pipe.material = "304".into();
        let p = package(&project).unwrap();
        assert_eq!(
            p.welds[0].joint,
            Joint::Flange,
            "downpipe flange at the source"
        );
        let reducer: Vec<_> = p
            .welds
            .iter()
            .filter(|w| w.between.iter().any(|b| b.starts_with("reducer.")))
            .collect();
        assert_eq!(reducer.len(), 2);
        assert!(
            reducer
                .iter()
                .all(|w| w.note.as_deref().is_some_and(|n| n.contains("309L")))
        );
        // Every straight is in a stick, and sticks hold no more than a stock length.
        let straights = p
            .routes
            .iter()
            .flat_map(|r| &r.parts)
            .filter(|x| matches!(x, Part::Straight { .. }))
            .count();
        assert_eq!(
            p.sticks.iter().map(|s| s.cuts.len()).sum::<usize>(),
            straights
        );
        assert!(
            p.sticks
                .iter()
                .all(|s| s.used_mm <= project.fabrication.stock_length_mm)
        );
    }
}
