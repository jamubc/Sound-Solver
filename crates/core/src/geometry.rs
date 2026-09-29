//! Pipe centrelines: straight segments joined by constant-radius arcs, which is what a mandrel
//! bender produces. Vehicle coordinates: X forward, Y left, Z up, origin at the downpipe
//! flange face; millimetres in the project file, metres here.
//!
//! At an interior vertex with deflection angle θ (between incoming and outgoing directions) and
//! centreline radius R the arc replaces a tangent length `t = R tan(θ/2)` of each adjacent
//! segment and has length `R θ`. The route length (the 1D solver's x-axis) is the sum of the
//! remaining straights and the arcs.

use crate::error::{Error, Result};

pub type Vec3 = [f64; 3];

pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn scale(a: Vec3, s: f64) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

pub fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn norm(a: Vec3) -> f64 {
    dot(a, a).sqrt()
}

pub fn unit(a: Vec3) -> Vec3 {
    scale(a, 1.0 / norm(a))
}

/// One piece of a centreline, positioned by arc length `s0` from the route start (m).
#[derive(Clone, Debug, PartialEq)]
pub enum Piece {
    Straight {
        s0: f64,
        length: f64,
        from: Vec3,
        to: Vec3,
    },
    Arc {
        s0: f64,
        length: f64,
        angle: f64,
        radius: f64,
        vertex: usize,
    },
}

impl Piece {
    pub fn s0(&self) -> f64 {
        match self {
            Piece::Straight { s0, .. } | Piece::Arc { s0, .. } => *s0,
        }
    }

    pub fn length(&self) -> f64 {
        match self {
            Piece::Straight { length, .. } | Piece::Arc { length, .. } => *length,
        }
    }
}

/// Resolved centreline of one route.
#[derive(Clone, Debug)]
pub struct Centreline {
    /// All vertices including the end points, m.
    pub points: Vec<Vec3>,
    pub pieces: Vec<Piece>,
    /// Total centreline length, m.
    pub length: f64,
}

impl Centreline {
    /// Direction of flow leaving the last vertex.
    pub fn end_direction(&self) -> Vec3 {
        let n = self.points.len();
        unit(sub(self.points[n - 1], self.points[n - 2]))
    }
}

/// Builds the centreline through `points` (m) with bend radius `radii[i]` (m) at interior
/// vertex `i + 1`.
pub fn centreline(points: &[Vec3], radii: &[f64]) -> Result<Centreline> {
    if points.len() < 2 {
        return Err(Error::invalid("a route needs at least two points"));
    }
    if radii.len() != points.len() - 2 {
        return Err(Error::invalid(
            "one bend radius per interior vertex is required",
        ));
    }
    let n = points.len();
    let mut angle = vec![0.0; n];
    let mut tangent = vec![0.0; n];
    for i in 1..n - 1 {
        let d1 = sub(points[i], points[i - 1]);
        let d2 = sub(points[i + 1], points[i]);
        if norm(d1) < 1e-9 || norm(d2) < 1e-9 {
            return Err(Error::invalid("route has coincident points"));
        }
        let c = dot(unit(d1), unit(d2)).clamp(-1.0, 1.0);
        angle[i] = c.acos();
        if angle[i] > std::f64::consts::PI - 1e-6 {
            return Err(Error::invalid("route doubles back on itself (180° bend)"));
        }
        tangent[i] = radii[i - 1] * (angle[i] / 2.0).tan();
    }
    let mut pieces = Vec::new();
    let mut s = 0.0;
    for i in 0..n - 1 {
        let seg = sub(points[i + 1], points[i]);
        let len = norm(seg);
        let straight = len - tangent[i] - tangent[i + 1];
        if straight < -1e-9 {
            return Err(Error::invalid(format!(
                "segment {i} is {:.1} mm long but its bends need {:.1} mm of tangent",
                len * 1e3,
                (tangent[i] + tangent[i + 1]) * 1e3
            )));
        }
        let dir = unit(seg);
        let from = add(points[i], scale(dir, tangent[i]));
        let to = sub(points[i + 1], scale(dir, tangent[i + 1]));
        pieces.push(Piece::Straight {
            s0: s,
            length: straight.max(0.0),
            from,
            to,
        });
        s += straight.max(0.0);
        if i + 1 < n - 1 && angle[i + 1] > 1e-9 {
            let length = radii[i] * angle[i + 1];
            pieces.push(Piece::Arc {
                s0: s,
                length,
                angle: angle[i + 1],
                radius: radii[i],
                vertex: i + 1,
            });
            s += length;
        }
    }
    Ok(Centreline {
        points: points.to_vec(),
        pieces,
        length: s,
    })
}

/// Local loss coefficient of a smooth circular bend (Idelchik 1986, diagram 6-1):
/// `K = A₁ B₁`, `A₁ = 0.9 sin θ` (θ ≤ 70°), `1.0` (θ = 90°), `0.7 + 0.35 θ/90°` (θ ≥ 100°),
/// linear between; `B₁ = 0.21 (R/D)^{-2.5}` (R/D < 1), `0.21 (R/D)^{-0.5}` (R/D ≥ 1).
/// Wall friction along the arc is computed separately by the solver.
pub fn bend_loss(angle_rad: f64, radius_over_d: f64) -> f64 {
    let deg = angle_rad.to_degrees();
    let a1 = |d: f64| {
        if d <= 70.0 {
            0.9 * d.to_radians().sin()
        } else if d >= 100.0 {
            0.7 + 0.35 * d / 90.0
        } else if d <= 90.0 {
            let a70 = 0.9 * 70f64.to_radians().sin();
            a70 + (1.0 - a70) * (d - 70.0) / 20.0
        } else {
            let a100 = 0.7 + 0.35 * 100.0 / 90.0;
            1.0 + (a100 - 1.0) * (d - 90.0) / 10.0
        }
    };
    let b1 = if radius_over_d < 1.0 {
        0.21 * radius_over_d.powf(-2.5)
    } else {
        0.21 / radius_over_d.sqrt()
    };
    a1(deg) * b1
}

/// Loss coefficient of a conical transition for flow from diameter `d_in` to `d_out` over
/// `length`, on the small-end velocity (Crane TP-410, 1988, p. A-26): with included angle θ
/// and `β = d_small/d_large`, a diffuser loses `2.6 sin(θ/2) (1 − β²)²` up to θ = 45° and the
/// sudden expansion's `(1 − β²)²` beyond; a nozzle `0.8 sin(θ/2) (1 − β²)` up to 45° and
/// `0.5 √sin(θ/2) (1 − β²)` beyond. The flow solver's cones are otherwise lossless.
pub fn cone_loss(d_in: f64, d_out: f64, length: f64) -> f64 {
    let (small, large) = (d_in.min(d_out), d_in.max(d_out));
    let half = ((large - small) / (2.0 * length)).atan();
    let area = 1.0 - (small / large).powi(2);
    let wide = half > 22.5f64.to_radians();
    if d_out > d_in {
        area * area * if wide { 1.0 } else { 2.6 * half.sin() }
    } else {
        area * if wide {
            0.5 * half.sin().sqrt()
        } else {
            0.8 * half.sin()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn right_angle_bend_lengths() {
        // 1 m legs, R = 0.1 m: straights 0.9 m each, arc π/2 · 0.1.
        let c = centreline(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]], &[0.1]).unwrap();
        let expected = 1.8 + std::f64::consts::FRAC_PI_2 * 0.1;
        assert!((c.length - expected).abs() < 1e-12);
        assert_eq!(c.pieces.len(), 3);
    }

    #[test]
    fn bend_that_does_not_fit_is_rejected() {
        assert!(
            centreline(
                &[[0.0, 0.0, 0.0], [0.05, 0.0, 0.0], [0.05, 1.0, 0.0]],
                &[0.1]
            )
            .is_err()
        );
    }

    #[test]
    fn idelchik_bend_values() {
        assert!((bend_loss(std::f64::consts::FRAC_PI_2, 1.0) - 0.21).abs() < 1e-12);
        assert!((bend_loss(std::f64::consts::FRAC_PI_2, 2.0) - 0.1485).abs() < 1e-4);
    }
}
