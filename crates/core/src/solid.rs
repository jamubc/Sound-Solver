//! The swept tube solids of the pipe runs, for export: STEP (B-rep through the `truck` kernel)
//! and binary STL. Every straight and bend of a route is its own solid, as it is its own part
//! on the bench: an annulus of the tube's outside and inside diameters swept along a straight
//! (translation) or a bend (revolution about the bend's axis). Millimetres, vehicle
//! coordinates. Element envelopes (bought-in cans) are left out.

use std::f64::consts::TAU;

use truck_modeling::{Face, Point3, Rad, Solid, Vector3, builder};
use truck_stepio::out::{CompleteStepDisplay, StepHeaderDescriptor, StepModels};

use crate::error::{Error, Result};
use crate::geometry::{Piece, Vec3, add, centreline, cross, dot, norm, scale, sub, unit};
use crate::project::Project;

/// Facets around a tube in the STL, and the largest arc step along a bend, degrees.
const AROUND: usize = 32;
const ARC_STEP_DEG: f64 = 5.0;

/// A piece of centreline in mm: a straight from `from` along `dir`, or a bend of `angle`
/// about `axis` through `centre`.
enum Sweep {
    Straight {
        from: Vec3,
        to: Vec3,
    },
    Bend {
        from: Vec3,
        dir: Vec3,
        centre: Vec3,
        axis: Vec3,
        angle: f64,
    },
}

/// Every route's pieces with its tube radii (outside, inside), mm.
fn sweeps(project: &Project) -> Result<Vec<(f64, f64, Vec<Sweep>)>> {
    project
        .system
        .routes
        .iter()
        .map(|r| {
            let points = project.route_points(r)?;
            let line = centreline(&points, &project.route_radii(r))?;
            let mm = |p: Vec3| scale(p, 1e3);
            let pieces = line
                .pieces
                .iter()
                .filter(|p| p.length() > 1e-6)
                .map(|p| match p {
                    Piece::Straight { from, to, .. } => Sweep::Straight {
                        from: mm(*from),
                        to: mm(*to),
                    },
                    Piece::Arc {
                        radius,
                        vertex,
                        angle,
                        ..
                    } => {
                        let i = *vertex;
                        let d_in = unit(sub(points[i], points[i - 1]));
                        let d_out = unit(sub(points[i + 1], points[i]));
                        let turns = unit(sub(d_out, scale(d_in, dot(d_in, d_out))));
                        let from = sub(points[i], scale(d_in, radius * (angle / 2.0).tan()));
                        Sweep::Bend {
                            from: mm(from),
                            dir: d_in,
                            centre: mm(add(from, scale(turns, *radius))),
                            axis: unit(cross(d_in, turns)),
                            angle: *angle,
                        }
                    }
                })
                .collect();
            let ro = r.pipe.od_mm / 2.0;
            Ok((ro, ro - r.pipe.wall_mm, pieces))
        })
        .collect()
}

fn point(p: Vec3) -> Point3 {
    Point3::new(p[0], p[1], p[2])
}

fn vector(v: Vec3) -> Vector3 {
    Vector3::new(v[0], v[1], v[2])
}

/// A unit vector perpendicular to `d`.
fn across(d: Vec3) -> Vec3 {
    let helper = if d[2].abs() < 0.9 {
        [0.0, 0.0, 1.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    unit(cross(d, helper))
}

/// Tube cross-section at `at` facing `dir`: the annulus between the two radii.
fn annulus(at: Vec3, dir: Vec3, ro: f64, ri: f64) -> Result<Face> {
    let (c, d, u) = (point(at), vector(dir), across(dir));
    let circle = |r: f64| {
        builder::rsweep(
            &builder::vertex(point(add(at, scale(u, r)))),
            c,
            d,
            Rad(TAU),
        )
    };
    builder::try_attach_plane(&[circle(ro), circle(ri).inverse()])
        .map_err(|e| Error::solver(format!("tube section: {e}")))
}

/// STEP (AP214) of every straight and bend as a solid.
pub fn step(project: &Project) -> Result<String> {
    let mut solids: Vec<Solid> = Vec::new();
    for (ro, ri, pieces) in sweeps(project)? {
        for piece in pieces {
            solids.push(match piece {
                Sweep::Straight { from, to } => {
                    let d = sub(to, from);
                    builder::tsweep(&annulus(from, unit(d), ro, ri)?, vector(d))
                }
                Sweep::Bend {
                    from,
                    dir,
                    centre,
                    axis,
                    angle,
                } => builder::rsweep(
                    &annulus(from, dir, ro, ri)?,
                    point(centre),
                    vector(axis),
                    Rad(angle),
                ),
            });
        }
    }
    let compressed: Vec<_> = solids.iter().map(Solid::compress).collect();
    let models: StepModels<_, _, _> = compressed.iter().collect();
    Ok(CompleteStepDisplay::new(
        models,
        StepHeaderDescriptor {
            file_name: format!("{}.step", project.name),
            organization_system: "exhaust-core".into(),
            ..Default::default()
        },
    )
    .to_string())
}

/// Binary STL of every route as one watertight tube: outer and inner walls, annular ends.
pub fn stl(project: &Project) -> Result<Vec<u8>> {
    let mut triangles: Vec<[Vec3; 3]> = Vec::new();
    for (ro, ri, pieces) in sweeps(project)? {
        // Centreline stations with a rotation-minimising frame (no twist between rings).
        let mut stations: Vec<(Vec3, Vec3)> = Vec::new();
        for piece in &pieces {
            match piece {
                Sweep::Straight { from, to } => {
                    let d = unit(sub(*to, *from));
                    stations.push((*from, d));
                    stations.push((*to, d));
                }
                Sweep::Bend {
                    from,
                    dir,
                    centre,
                    axis,
                    angle,
                } => {
                    let n = (angle.to_degrees() / ARC_STEP_DEG).ceil().max(1.0) as usize;
                    let v0 = sub(*from, *centre);
                    for k in 0..=n {
                        let t = angle * k as f64 / n as f64;
                        let v = add(scale(v0, t.cos()), scale(cross(*axis, v0), t.sin()));
                        let d = add(scale(*dir, t.cos()), scale(cross(*axis, *dir), t.sin()));
                        stations.push((add(*centre, v), unit(d)));
                    }
                }
            }
        }
        stations.dedup_by(|a, b| norm(sub(a.0, b.0)) < 1e-6);
        if stations.len() < 2 {
            continue;
        }
        let mut u = across(stations[0].1);
        let rings: Vec<(Vec<Vec3>, Vec<Vec3>)> = stations
            .iter()
            .map(|&(at, d)| {
                u = unit(sub(u, scale(d, dot(u, d))));
                let w = cross(d, u);
                let ring = |r: f64| {
                    (0..AROUND)
                        .map(|k| {
                            let a = TAU * k as f64 / AROUND as f64;
                            add(at, add(scale(u, r * a.cos()), scale(w, r * a.sin())))
                        })
                        .collect::<Vec<_>>()
                };
                (ring(ro), ring(ri))
            })
            .collect();
        // Counter-clockwise seen from outside the wall (rings run counter-clockwise about `d`).
        for pair in rings.windows(2) {
            let ((o0, i0), (o1, i1)) = (&pair[0], &pair[1]);
            for k in 0..AROUND {
                let j = (k + 1) % AROUND;
                triangles.push([o0[k], o1[j], o1[k]]);
                triangles.push([o0[k], o0[j], o1[j]]);
                triangles.push([i0[k], i1[k], i1[j]]);
                triangles.push([i0[k], i1[j], i0[j]]);
            }
        }
        let ((o0, i0), (o1, i1)) = (&rings[0], &rings[rings.len() - 1]);
        for k in 0..AROUND {
            let j = (k + 1) % AROUND;
            triangles.push([o0[k], i0[j], o0[j]]);
            triangles.push([o0[k], i0[k], i0[j]]);
            triangles.push([o1[k], o1[j], i1[j]]);
            triangles.push([o1[k], i1[j], i1[k]]);
        }
    }
    let mut out = Vec::with_capacity(84 + 50 * triangles.len());
    let mut header = format!("exhaust-core STL, mm: {}", project.name).into_bytes();
    header.resize(80, b' ');
    out.extend(header);
    out.extend((triangles.len() as u32).to_le_bytes());
    for [a, b, c] in &triangles {
        let n = unit(cross(sub(*b, *a), sub(*c, *a)));
        for v in [n, *a, *b, *c] {
            for x in v {
                out.extend((x as f32).to_le_bytes());
            }
        }
        out.extend([0u8; 2]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::Mesh;
    use crate::validation::grid::STRAIGHT_PIPE;

    /// The STL tube is closed and holds the tube's volume: π(ro² − ri²) × centreline length.
    #[test]
    fn stl_is_a_closed_tube_of_the_right_volume() {
        let project = Project::from_json(STRAIGHT_PIPE).unwrap();
        let mesh = Mesh::from_stl(&stl(&project).unwrap()).unwrap();
        let volume: f64 = mesh
            .triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| mesh.vertices[i as usize]);
                dot(a, cross(b, c)) / 6.0
            })
            .sum();
        let expected: f64 = project
            .system
            .routes
            .iter()
            .map(|r| {
                let line =
                    centreline(&project.route_points(r).unwrap(), &project.route_radii(r)).unwrap();
                let (ro, ri) = (r.pipe.od_mm / 2.0, r.pipe.od_mm / 2.0 - r.pipe.wall_mm);
                std::f64::consts::PI * (ro * ro - ri * ri) * line.length * 1e3
            })
            .sum();
        // Facets undercut the circles by (1 − sin(2π/n)/(2π/n)) ≈ 0.6 %.
        assert!(
            (volume / expected - 1.0).abs() < 0.01,
            "{volume} vs {expected}"
        );
    }

    /// A STEP reader finds one closed shell per straight and bend.
    #[test]
    fn step_reads_back_a_shell_per_piece() {
        let project = Project::from_json(STRAIGHT_PIPE).unwrap();
        let text = step(&project).unwrap();
        let exchange = ruststep::parser::parse(&text).expect("STEP parses");
        let table = truck_stepio::r#in::Table::from_data_section(&exchange.data[0]);
        let pieces: usize = sweeps(&project).unwrap().iter().map(|s| s.2.len()).sum();
        assert_eq!(table.shell.len(), pieces);
        for shell in table.shell.values() {
            table.to_compressed_shell(shell).expect("shell rebuilds");
        }
    }
}
