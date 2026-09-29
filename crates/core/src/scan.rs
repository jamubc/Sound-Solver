//! Underbody scan and pipe clearance. A scan (STL or OBJ from a phone LiDAR or photogrammetry
//! app) is placed on the car by three points picked on it whose vehicle coordinates are known
//! (jack pads): the rigid transform fitting them (Kabsch, least squares) maps the scan into
//! vehicle coordinates, and its residual shows a scan out of scale or points picked badly.
//!
//! Clearance is the distance from the pipe surface to the nearest scan triangle, sampled along
//! every route's centreline (distance from the centreline minus the tube radius), against the
//! gap the project asks for at that position along the car. The core parses bytes; callers
//! read the files.

use nalgebra::{Matrix3, Vector3};
use schemars::JsonSchema;
use serde::Serialize;

use crate::error::{Error, Result};
use crate::geometry::{Vec3, add, centreline, dot, scale, sub};
use crate::project::{Project, Scan};

/// Spacing of clearance samples along a centreline, mm.
const SAMPLE_MM: f64 = 10.0;

/// Triangle mesh.
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub vertices: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
}

impl Mesh {
    /// Reads an OBJ when `file_name` ends in `.obj`, else STL.
    pub fn parse(file_name: &str, bytes: &[u8]) -> Result<Self> {
        if file_name.to_ascii_lowercase().ends_with(".obj") {
            Self::from_obj(&String::from_utf8_lossy(bytes))
        } else {
            Self::from_stl(bytes)
        }
    }

    /// Reads binary or ASCII STL.
    pub fn from_stl(bytes: &[u8]) -> Result<Self> {
        let ascii = bytes.starts_with(b"solid")
            && std::str::from_utf8(&bytes[..bytes.len().min(512)])
                .is_ok_and(|h| h.contains("facet"));
        if ascii {
            let text = std::str::from_utf8(bytes).map_err(|_| Error::invalid("STL: not UTF-8"))?;
            let mut mesh = Mesh::default();
            let mut corner = Vec::with_capacity(3);
            for line in text.lines() {
                let mut words = line.split_whitespace();
                if words.next() == Some("vertex") {
                    let v: Vec<f64> = words.filter_map(|w| w.parse().ok()).collect();
                    if v.len() != 3 {
                        return Err(Error::invalid("STL: bad vertex line"));
                    }
                    corner.push([v[0], v[1], v[2]]);
                    if corner.len() == 3 {
                        mesh.push(&corner);
                        corner.clear();
                    }
                }
            }
            return Ok(mesh);
        }
        if bytes.len() < 84 {
            return Err(Error::invalid("STL: too short"));
        }
        let count = u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize;
        if bytes.len() < 84 + count * 50 {
            return Err(Error::invalid("STL: truncated"));
        }
        let mut mesh = Mesh::default();
        for k in 0..count {
            let at = 84 + k * 50 + 12;
            let f = |i: usize| {
                f32::from_le_bytes(bytes[at + 4 * i..at + 4 * i + 4].try_into().unwrap()) as f64
            };
            mesh.push(&[[f(0), f(1), f(2)], [f(3), f(4), f(5)], [f(6), f(7), f(8)]]);
        }
        Ok(mesh)
    }

    /// Reads the vertices and faces of a Wavefront OBJ (polygons fanned into triangles).
    pub fn from_obj(text: &str) -> Result<Self> {
        let mut mesh = Mesh::default();
        for line in text.lines() {
            let mut words = line.split_whitespace();
            match words.next() {
                Some("v") => {
                    let v: Vec<f64> = words.take(3).filter_map(|w| w.parse().ok()).collect();
                    if v.len() != 3 {
                        return Err(Error::invalid("OBJ: bad vertex line"));
                    }
                    mesh.vertices.push([v[0], v[1], v[2]]);
                }
                Some("f") => {
                    let n = mesh.vertices.len() as i64;
                    let idx: Vec<u32> = words
                        .filter_map(|w| w.split('/').next()?.parse::<i64>().ok())
                        .map(|i| if i < 0 { n + i } else { i - 1 })
                        .filter(|&i| i >= 0 && i < n)
                        .map(|i| i as u32)
                        .collect();
                    for k in 1..idx.len().saturating_sub(1) {
                        mesh.triangles.push([idx[0], idx[k], idx[k + 1]]);
                    }
                }
                _ => {}
            }
        }
        Ok(mesh)
    }

    fn push(&mut self, corners: &[Vec3]) {
        let base = self.vertices.len() as u32;
        self.vertices.extend_from_slice(corners);
        self.triangles.push([base, base + 1, base + 2]);
    }

    fn corners(&self, t: usize) -> [Vec3; 3] {
        self.triangles[t].map(|i| self.vertices[i as usize])
    }
}

/// Rigid transform `x ↦ R x + t`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, JsonSchema)]
pub struct Placement {
    pub rotation: [[f64; 3]; 3],
    pub translation: Vec3,
    /// RMS distance between the reference points as placed and as given, mm.
    pub residual_mm: f64,
}

impl Placement {
    pub fn apply(&self, x: Vec3) -> Vec3 {
        let r = &self.rotation;
        let p = [0, 1, 2].map(|i| r[i][0] * x[0] + r[i][1] * x[1] + r[i][2] * x[2]);
        add(p, self.translation)
    }
}

/// Least-squares rigid fit (Kabsch) of the scan's reference points, scaled to mm, onto their
/// vehicle coordinates.
pub fn place(scan: &Scan) -> Result<Placement> {
    let src: Vec<Vector3<f64>> = scan
        .scan_points
        .iter()
        .map(|p| Vector3::from(*p) * scan.unit_mm)
        .collect();
    let dst: Vec<Vector3<f64>> = scan
        .vehicle_points_mm
        .iter()
        .map(|p| Vector3::from(*p))
        .collect();
    let spread = (src[1] - src[0]).cross(&(src[2] - src[0])).norm();
    if spread < 1e3 {
        return Err(Error::invalid(
            "scan reference points are (nearly) in a line",
        ));
    }
    let (cs, cd) = (
        src.iter().sum::<Vector3<f64>>() / 3.0,
        dst.iter().sum::<Vector3<f64>>() / 3.0,
    );
    let h: Matrix3<f64> = src
        .iter()
        .zip(&dst)
        .map(|(s, d)| (s - cs) * (d - cd).transpose())
        .sum();
    let svd = h.svd(true, true);
    let (u, v_t) = (svd.u.expect("U"), svd.v_t.expect("Vᵀ"));
    let mut d = Matrix3::identity();
    d[(2, 2)] = (v_t.transpose() * u.transpose()).determinant().signum();
    let r = v_t.transpose() * d * u.transpose();
    let t = cd - r * cs;
    let residual = (src
        .iter()
        .zip(&dst)
        .map(|(s, d)| (r * s + t - d).norm_squared())
        .sum::<f64>()
        / 3.0)
        .sqrt();
    Ok(Placement {
        rotation: [0, 1, 2].map(|i| [0, 1, 2].map(|j| r[(i, j)])),
        translation: [t.x, t.y, t.z],
        residual_mm: residual,
    })
}

/// Axis-aligned bounding-box tree over triangles, for nearest-point queries.
struct Tree {
    nodes: Vec<Node>,
    order: Vec<u32>,
}

struct Node {
    lo: Vec3,
    hi: Vec3,
    /// Leaf (`count > 0`): triangles `order[first..first + count]`; inner: children `first`
    /// and `second`.
    first: u32,
    second: u32,
    count: u32,
}

const LEAF: usize = 8;

impl Tree {
    fn new(mesh: &Mesh) -> Self {
        let mut order: Vec<u32> = (0..mesh.triangles.len() as u32).collect();
        let centres: Vec<Vec3> = (0..mesh.triangles.len())
            .map(|t| {
                let [a, b, c] = mesh.corners(t);
                scale(add(add(a, b), c), 1.0 / 3.0)
            })
            .collect();
        let mut tree = Tree {
            nodes: Vec::new(),
            order: Vec::new(),
        };
        tree.build(mesh, &centres, &mut order[..], 0);
        tree.order = order;
        tree
    }

    fn build(&mut self, mesh: &Mesh, centres: &[Vec3], tris: &mut [u32], offset: usize) -> usize {
        let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for &t in tris.iter() {
            for p in mesh.corners(t as usize) {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
        }
        let index = self.nodes.len();
        self.nodes.push(Node {
            lo,
            hi,
            first: offset as u32,
            second: 0,
            count: tris.len() as u32,
        });
        if tris.len() > LEAF {
            let axis = (0..3)
                .max_by(|&a, &b| (hi[a] - lo[a]).total_cmp(&(hi[b] - lo[b])))
                .unwrap();
            let mid = tris.len() / 2;
            tris.select_nth_unstable_by(mid, |a, b| {
                centres[*a as usize][axis].total_cmp(&centres[*b as usize][axis])
            });
            let (left, right) = tris.split_at_mut(mid);
            let l = self.build(mesh, centres, left, offset);
            let r = self.build(mesh, centres, right, offset + mid);
            let node = &mut self.nodes[index];
            (node.first, node.second, node.count) = (l as u32, r as u32, 0);
        }
        index
    }

    /// Nearest point of the mesh to `p`.
    fn nearest(&self, mesh: &Mesh, p: Vec3) -> (f64, Vec3) {
        let mut best = (f64::INFINITY, p);
        let mut stack = vec![0usize];
        while let Some(i) = stack.pop() {
            let node = &self.nodes[i];
            if box_distance_sq(node, p) >= best.0 {
                continue;
            }
            if node.count > 0 {
                for &t in &self.order[node.first as usize..(node.first + node.count) as usize] {
                    let q = closest_on_triangle(p, mesh.corners(t as usize));
                    let d = dot(sub(q, p), sub(q, p));
                    if d < best.0 {
                        best = (d, q);
                    }
                }
            } else {
                let (l, r) = (node.first as usize, node.second as usize);
                // Nearer child last, so it is searched first.
                let (dl, dr) = (
                    box_distance_sq(&self.nodes[l], p),
                    box_distance_sq(&self.nodes[r], p),
                );
                if dl < dr {
                    stack.push(r);
                    stack.push(l);
                } else {
                    stack.push(l);
                    stack.push(r);
                }
            }
        }
        (best.0.sqrt(), best.1)
    }
}

fn box_distance_sq(node: &Node, p: Vec3) -> f64 {
    (0..3)
        .map(|k| {
            let d = (node.lo[k] - p[k]).max(p[k] - node.hi[k]).max(0.0);
            d * d
        })
        .sum()
}

/// Closest point on triangle `abc` to `p` (Ericson, Real-Time Collision Detection, §5.1.5).
fn closest_on_triangle(p: Vec3, [a, b, c]: [Vec3; 3]) -> Vec3 {
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = sub(p, b);
    let (d3, d4) = (dot(ab, bp), dot(ac, bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return add(a, scale(ab, d1 / (d1 - d3)));
    }
    let cp = sub(p, c);
    let (d5, d6) = (dot(ab, cp), dot(ac, cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return add(a, scale(ac, d2 / (d2 - d6)));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return add(b, scale(sub(c, b), (d4 - d3) / ((d4 - d3) + (d5 - d6))));
    }
    let denom = 1.0 / (va + vb + vc);
    add(a, add(scale(ab, vb * denom), scale(ac, vc * denom)))
}

#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct Clearance {
    pub placement: Placement,
    /// Closest approach of each route.
    pub routes: Vec<RouteClearance>,
    /// Stretches closer than the gap wanted there.
    pub contacts: Vec<Contact>,
}

#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct RouteClearance {
    pub route: String,
    pub min_mm: f64,
    pub at_mm: f64,
    pub at: Vec3,
}

/// A stretch of pipe closer to the underbody than wanted.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct Contact {
    pub route: String,
    /// Along the centreline from the route's start, mm.
    pub from_mm: f64,
    pub to_mm: f64,
    pub min_mm: f64,
    pub wanted_mm: f64,
    /// Where it is closest, on the centreline and on the scan.
    pub at: Vec3,
    pub scan_point: Vec3,
    /// The clearance zone that set the gap, if one did.
    pub zone: Option<String>,
}

/// Clearance of every route to a scan placed on the car per `project.fabrication.scan`.
pub fn clearance(project: &Project, scan: &Mesh) -> Result<Clearance> {
    let f = &project.fabrication;
    let spec = f
        .scan
        .as_ref()
        .ok_or_else(|| Error::invalid("the project has no scan"))?;
    let placement = place(spec)?;
    let placed = Mesh {
        vertices: scan
            .vertices
            .iter()
            .map(|&v| placement.apply(scale(v, spec.unit_mm)))
            .collect(),
        triangles: scan.triangles.clone(),
    };
    if placed.triangles.is_empty() {
        return Err(Error::invalid("the scan has no triangles"));
    }
    let tree = Tree::new(&placed);
    let wanted = |x: f64| {
        f.clearance_zones
            .iter()
            .filter(|z| x >= z.x_min_mm && x <= z.x_max_mm)
            .max_by(|a, b| a.clearance_mm.total_cmp(&b.clearance_mm))
            .map_or((f.clearance_mm, None), |z| {
                (z.clearance_mm.max(f.clearance_mm), Some(z.name.clone()))
            })
    };
    let mut routes = Vec::new();
    let mut contacts: Vec<Contact> = Vec::new();
    for r in &project.system.routes {
        let points = project.route_points(r)?;
        let line = centreline(&points, &project.route_radii(r))?;
        let n = ((line.length * 1e3 / SAMPLE_MM).ceil() as usize).max(1);
        let radius = r.pipe.od_mm / 2.0;
        let mut worst = RouteClearance {
            route: r.id.clone(),
            min_mm: f64::INFINITY,
            at_mm: 0.0,
            at: [0.0; 3],
        };
        let mut open: Option<Contact> = None;
        for k in 0..=n {
            let s = line.length * k as f64 / n as f64;
            let at = scale(line.point_at(s), 1e3);
            let (d, q) = tree.nearest(&placed, at);
            let gap = d - radius;
            if gap < worst.min_mm {
                worst = RouteClearance {
                    route: r.id.clone(),
                    min_mm: gap,
                    at_mm: s * 1e3,
                    at,
                };
            }
            let (need, zone) = wanted(at[0]);
            if gap < need {
                let c = open.get_or_insert(Contact {
                    route: r.id.clone(),
                    from_mm: s * 1e3,
                    to_mm: s * 1e3,
                    min_mm: gap,
                    wanted_mm: need,
                    at,
                    scan_point: q,
                    zone: zone.clone(),
                });
                c.to_mm = s * 1e3;
                if gap < c.min_mm {
                    (c.min_mm, c.at, c.scan_point, c.wanted_mm, c.zone) = (gap, at, q, need, zone);
                }
            } else if let Some(c) = open.take() {
                contacts.push(c);
            }
        }
        contacts.extend(open);
        routes.push(worst);
    }
    Ok(Clearance {
        placement,
        routes,
        contacts,
    })
}

/// Clearance report, Markdown.
pub fn report_markdown(project: &Project, c: &Clearance) -> String {
    let mut out = format!(
        "# Underbody clearance: {}\n\nScan placed by three reference points; residual {:.1} mm.\n\
         Gap wanted {:.0} mm{}.\n\n",
        project.name,
        c.placement.residual_mm,
        project.fabrication.clearance_mm,
        project
            .fabrication
            .clearance_zones
            .iter()
            .map(|z| format!(
                ", {:.0} mm {} (x {:.0}…{:.0})",
                z.clearance_mm, z.name, z.x_min_mm, z.x_max_mm
            ))
            .collect::<String>()
    );
    if c.contacts.is_empty() {
        out.push_str("No stretch is closer than wanted.\n\n");
    } else {
        out.push_str(
            "## Too close\n\n| Route | From mm | To mm | Closest mm | Wanted mm | x | y | z | Zone |\n\
             |---|---:|---:|---:|---:|---:|---:|---:|---|\n",
        );
        for k in &c.contacts {
            out.push_str(&format!(
                "| {} | {:.0} | {:.0} | {:.0} | {:.0} | {:.0} | {:.0} | {:.0} | {} |\n",
                k.route,
                k.from_mm,
                k.to_mm,
                k.min_mm,
                k.wanted_mm,
                k.at[0],
                k.at[1],
                k.at[2],
                k.zone.as_deref().unwrap_or("")
            ));
        }
        out.push('\n');
    }
    out.push_str("## Closest approach per route\n\n| Route | Clearance mm | Along mm | x | y | z |\n|---|---:|---:|---:|---:|---:|\n");
    for r in &c.routes {
        out.push_str(&format!(
            "| {} | {:.0} | {:.0} | {:.0} | {:.0} | {:.0} |\n",
            r.route, r.min_mm, r.at_mm, r.at[0], r.at[1], r.at[2]
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::grid::STRAIGHT_PIPE;

    /// A flat floor scanned in metres, rotated and shifted: placed back by its reference
    /// points, it sits at a known height above the pipes, and the tree finds the same nearest
    /// points as a search of every triangle.
    #[test]
    fn places_a_scan_and_measures_clearance() {
        let mut project = Project::from_json(STRAIGHT_PIPE).unwrap();
        // Floor plane z = +40 mm in vehicle coordinates, as a grid of triangles.
        let floor = |x: f64, y: f64| [x, y, 40.0];
        let (mut mesh, n) = (Mesh::default(), 40);
        for i in 0..n {
            for j in 0..n {
                let (x0, y0) = (-4000.0 + 120.0 * i as f64, -2400.0 + 120.0 * j as f64);
                mesh.push(&[floor(x0, y0), floor(x0 + 120.0, y0), floor(x0, y0 + 120.0)]);
                mesh.push(&[
                    floor(x0 + 120.0, y0),
                    floor(x0 + 120.0, y0 + 120.0),
                    floor(x0, y0 + 120.0),
                ]);
            }
        }
        // Scan frame: rotate 30° about z, shift, and store in metres.
        let (c, s) = (30f64.to_radians().cos(), 30f64.to_radians().sin());
        let to_scan = |p: Vec3| {
            scale(
                [
                    c * p[0] - s * p[1] + 500.0,
                    s * p[0] + c * p[1] - 200.0,
                    p[2] + 300.0,
                ],
                1e-3,
            )
        };
        let refs = [
            [-3500.0, -2000.0, 40.0],
            [-500.0, -2000.0, 40.0],
            [-2000.0, 2000.0, 40.0],
        ];
        let scan = Mesh {
            vertices: mesh.vertices.iter().map(|&v| to_scan(v)).collect(),
            triangles: mesh.triangles.clone(),
        };
        project.fabrication.scan = Some(Scan {
            path: "floor.stl".into(),
            unit_mm: 1000.0,
            scan_points: refs.map(to_scan),
            vehicle_points_mm: refs,
        });
        project.fabrication.clearance_mm = 150.0;
        let result = clearance(&project, &scan).unwrap();
        assert!(result.placement.residual_mm < 1e-6);
        // Downpipe starts at the flange (z = 0): 40 mm below the floor, less its radius.
        let down = &result.routes[0];
        assert!((down.min_mm - (40.0 - 63.5 / 2.0)).abs() < 1e-6, "{down:?}");
        assert!(
            result
                .contacts
                .iter()
                .any(|k| k.route == "downpipe" && k.from_mm == 0.0)
        );
        // The tree agrees with brute force.
        let placed = Mesh {
            vertices: scan
                .vertices
                .iter()
                .map(|&v| result.placement.apply(scale(v, 1000.0)))
                .collect(),
            triangles: scan.triangles.clone(),
        };
        let tree = Tree::new(&placed);
        for p in [
            [-1234.0, 56.0, -150.0],
            [300.0, 900.0, 80.0],
            [-4100.0, -2500.0, 10.0],
        ] {
            let brute = (0..placed.triangles.len())
                .map(|t| {
                    let q = closest_on_triangle(p, placed.corners(t));
                    dot(sub(q, p), sub(q, p)).sqrt()
                })
                .fold(f64::INFINITY, f64::min);
            assert!((tree.nearest(&placed, p).0 - brute).abs() < 1e-9);
        }
    }

    #[test]
    fn reads_stl_and_obj() {
        let ascii = b"solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid t\n";
        assert_eq!(Mesh::from_stl(ascii).unwrap().triangles.len(), 1);
        let mut binary = vec![0u8; 80];
        binary.extend(1u32.to_le_bytes());
        binary.extend([0u8; 12]);
        for v in [0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
            binary.extend(v.to_le_bytes());
        }
        binary.extend([0u8; 2]);
        let mesh = Mesh::from_stl(&binary).unwrap();
        assert_eq!(mesh.vertices[1], [1.0, 0.0, 0.0]);
        let quad =
            Mesh::from_obj("v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1/1 2/2 3/3 4/4\n").unwrap();
        assert_eq!(quad.triangles, vec![[0, 1, 2], [0, 2, 3]]);
    }
}
