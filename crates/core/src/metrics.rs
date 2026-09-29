//! Sound metrics over a solved sweep (directive, "Sound evaluation").
//!
//! - Drone: the firing-order line (`N_cyl/2` for a four-stroke, `f = N_cyl · rpm/120`) at the
//!   receiver against engine speed; its peak (parabola through the highest point and its
//!   neighbours, unless its vertex rises 3 dB above them: then the sweep does not resolve it
//!   and the highest point stands), the speeds where it has fallen 3 dB either side (linear
//!   between points), and
//!   its highest level in the cruise band. Interior drone adds the measured cabin transfer
//!   function at each line's frequency; without one it is unavailable, never synthesised.
//! - Boominess: firing-order level minus the level of every other line, dB, averaged over the
//!   cruise band.
//! - Rasp proxy: energy of lines above 800 Hz over energy below 300 Hz, summed over
//!   3000–5000 rpm, dB. A proxy for harshness, not a perceptual metric.
//!
//! Failed points are skipped; points whose time-domain run did not converge are used and
//! listed.

use schemars::JsonSchema;
use serde::Serialize;

use crate::project::Project;
use crate::solve::{PointOutcome, PointResult};

/// Rasp proxy bands, Hz, and speed range, rpm.
const RASP_HIGH_HZ: f64 = 800.0;
const RASP_LOW_HZ: f64 = 300.0;
const RASP_RPM: [f64; 2] = [3000.0, 5000.0];

#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct Metrics {
    /// Engine order of firing.
    pub firing_order: f64,
    pub drone_exterior: Option<DroneReport>,
    /// Through the measured cabin transfer function; `None` without one.
    pub drone_interior: Option<DroneReport>,
    /// Exterior, dB.
    pub boominess_db: Option<f64>,
    /// Exterior, dB.
    pub rasp_proxy_db: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct DroneReport {
    /// Firing-order level per solved speed: `[rpm, dB]`.
    pub track: Vec<[f64; 2]>,
    pub peak_rpm: f64,
    pub peak_db: f64,
    /// Speeds below and above the peak where the track is 3 dB down; `None` where it stays
    /// within 3 dB to the end of the sweep.
    pub minus_3db_rpm: [Option<f64>; 2],
    /// Highest level in the cruise band, `[rpm, dB]`; `None` without points in the band.
    pub cruise_peak: Option<[f64; 2]>,
    /// Speeds in the track whose time-domain run did not reach periodicity.
    pub unconverged_rpm: Vec<f64>,
}

pub fn evaluate(project: &Project, points: &[PointOutcome]) -> Metrics {
    let firing_order = project.engine.geometry.firing_order.len() as f64 / 2.0;
    let solved: Vec<&PointResult> = points
        .iter()
        .filter_map(|p| match p {
            PointOutcome::Solved(r) => Some(r.as_ref()),
            PointOutcome::Failed { .. } => None,
        })
        .collect();
    let band = project.operating.cruise_band_rpm;
    let in_band = |rpm: f64| rpm >= band[0] && rpm <= band[1];
    // Level and frequency of the firing-order line.
    let firing = |r: &PointResult| {
        r.spectrum
            .iter()
            .find(|l| (l.order - firing_order).abs() < 1e-9)
            .map(|l| (l.spl_db, l.frequency_hz))
    };
    let sample = |r: &PointResult, level: f64| (r.rpm, level, r.provenance.converged());
    let exterior: Vec<(f64, f64, bool)> = solved
        .iter()
        .filter_map(|r| firing(r).map(|(level, _)| sample(r, level)))
        .collect();
    let interior: Vec<(f64, f64, bool)> = match &project.measurements.cabin_tf {
        Some(tf) => solved
            .iter()
            .filter_map(|r| {
                let (level, f) = firing(r)?;
                Some(sample(r, level + tf.at(f)?))
            })
            .collect(),
        None => Vec::new(),
    };
    let energy = |r: &PointResult, keep: &dyn Fn(f64, f64) -> bool| -> f64 {
        r.spectrum
            .iter()
            .filter(|l| keep(l.order, l.frequency_hz))
            .map(|l| 10f64.powf(l.spl_db / 10.0))
            .sum()
    };
    let boom: Vec<f64> = solved
        .iter()
        .filter(|r| in_band(r.rpm))
        .filter_map(|r| {
            let (level, _) = firing(r)?;
            let rest = energy(r, &|order, _| (order - firing_order).abs() >= 1e-9);
            Some(level - 10.0 * rest.log10())
        })
        .collect();
    let rasp_points = solved
        .iter()
        .filter(|r| r.rpm >= RASP_RPM[0] && r.rpm <= RASP_RPM[1]);
    let (high, low) = rasp_points.fold((0.0, 0.0), |(h, l), r| {
        (
            h + energy(r, &|_, f| f > RASP_HIGH_HZ),
            l + energy(r, &|_, f| f < RASP_LOW_HZ),
        )
    });
    Metrics {
        firing_order,
        drone_exterior: drone(&exterior, band),
        drone_interior: drone(&interior, band),
        boominess_db: (!boom.is_empty()).then(|| boom.iter().sum::<f64>() / boom.len() as f64),
        rasp_proxy_db: (low > 0.0).then(|| 10.0 * (high / low).log10()),
    }
}

/// An engine order of two solved configurations, `b − a`.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct OrderDifference {
    pub order: u32,
    /// Mean level difference over the cruise band, at speeds both solved, dB.
    pub cruise_db: Option<f64>,
    /// The largest difference either way over the sweep, `[rpm, dB]`.
    pub largest: Option<[f64; 2]>,
    /// Highest level over the sweep, of `a` and of `b`, dB.
    pub peak_db: [Option<f64>; 2],
}

/// Engine orders 1–8 of configuration `b` against `a`, the cruise band `project`'s.
pub fn compare(project: &Project, a: &[PointOutcome], b: &[PointOutcome]) -> Vec<OrderDifference> {
    let band = project.operating.cruise_band_rpm;
    let solved = |points: &[PointOutcome]| -> Vec<PointResult> {
        points
            .iter()
            .filter_map(|p| match p {
                PointOutcome::Solved(r) => Some(r.as_ref().clone()),
                PointOutcome::Failed { .. } => None,
            })
            .collect()
    };
    let (a, b) = (solved(a), solved(b));
    let level = |r: &PointResult, k: u32| r.orders.iter().find(|o| o.0 == k).map(|o| o.2);
    (1..=8)
        .map(|k| {
            let diffs: Vec<(f64, f64)> = a
                .iter()
                .filter_map(|ra| {
                    let rb = b.iter().find(|rb| rb.rpm == ra.rpm)?;
                    Some((ra.rpm, level(rb, k)? - level(ra, k)?))
                })
                .collect();
            let cruise: Vec<f64> = diffs
                .iter()
                .filter(|d| d.0 >= band[0] && d.0 <= band[1])
                .map(|d| d.1)
                .collect();
            let peak = |points: &[PointResult]| {
                points
                    .iter()
                    .filter_map(|r| level(r, k))
                    .max_by(f64::total_cmp)
            };
            OrderDifference {
                order: k,
                cruise_db: (!cruise.is_empty())
                    .then(|| cruise.iter().sum::<f64>() / cruise.len() as f64),
                largest: diffs
                    .iter()
                    .max_by(|x, y| x.1.abs().total_cmp(&y.1.abs()))
                    .map(|d| [d.0, d.1]),
                peak_db: [peak(&a), peak(&b)],
            }
        })
        .collect()
}

/// Drone report of a track, `(rpm, level, converged)` in increasing speed.
pub(crate) fn drone(track: &[(f64, f64, bool)], band: [f64; 2]) -> Option<DroneReport> {
    let pts: Vec<[f64; 2]> = track.iter().map(|&(rpm, level, _)| [rpm, level]).collect();
    let k = (0..pts.len()).max_by(|&a, &b| pts[a][1].total_cmp(&pts[b][1]))?;
    let [mut peak_rpm, mut peak_db] = pts[k];
    if k > 0 && k + 1 < pts.len() {
        // Vertex of the parabola through the three points around the maximum.
        let ([x0, y0], [x1, y1], [x2, y2]) = (pts[k - 1], pts[k], pts[k + 1]);
        let d = (x0 - x1) * (x0 - x2) * (x1 - x2);
        let a = (x2 * (y1 - y0) + x1 * (y0 - y2) + x0 * (y2 - y1)) / d;
        let b = (x2 * x2 * (y0 - y1) + x1 * x1 * (y2 - y0) + x0 * x0 * (y1 - y2)) / d;
        let x = -b / (2.0 * a);
        let y = y1 + (x - x1) * (a * (x + x1) + b);
        // A vertex far above the sampled maximum means the sweep step does not resolve it.
        if a < 0.0 && y - y1 < 3.0 {
            (peak_rpm, peak_db) = (x, y);
        }
    }
    let down = peak_db - 3.0;
    let crossing = |range: &mut dyn Iterator<Item = usize>| {
        let mut prev = k;
        for i in range {
            let ([xa, ya], [xb, yb]) = (pts[prev], pts[i]);
            if yb < down {
                return Some(xa + (xb - xa) * (ya - down) / (ya - yb));
            }
            prev = i;
        }
        None
    };
    let cruise_peak = pts
        .iter()
        .filter(|p| p[0] >= band[0] && p[0] <= band[1])
        .max_by(|a, b| a[1].total_cmp(&b[1]))
        .copied();
    let unconverged_rpm = track.iter().filter(|s| !s.2).map(|s| s.0).collect();
    Some(DroneReport {
        minus_3db_rpm: [
            crossing(&mut (0..k).rev()),
            crossing(&mut (k + 1..pts.len())),
        ],
        track: pts,
        peak_rpm,
        peak_db,
        cruise_peak,
        unconverged_rpm,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drone_peak_and_width_of_a_resonance() {
        // Lorentzian resonance at 2830 rpm, 3.01 dB down at ±100 rpm, sampled every 50 rpm.
        let track: Vec<(f64, f64, bool)> = (0..=60)
            .map(|i| {
                let rpm = 1500.0 + 50.0 * i as f64;
                let x = (rpm - 2830.0) / 100.0;
                (rpm, 100.0 - 10.0 * (1.0 + x * x).log10(), rpm != 2850.0)
            })
            .collect();
        let d = drone(&track, [1800.0, 2400.0]).expect("track");
        assert!((d.peak_rpm - 2830.0).abs() < 5.0, "peak at {}", d.peak_rpm);
        assert!((d.peak_db - 100.0).abs() < 0.2, "peak level {}", d.peak_db);
        let [lo, hi] = d.minus_3db_rpm.map(|x| x.expect("both sides fall 3 dB"));
        assert!(
            (lo - 2730.0).abs() < 10.0 && (hi - 2930.0).abs() < 10.0,
            "−3 dB at {lo}, {hi}"
        );
        assert_eq!(d.cruise_peak.map(|p| p[0]), Some(2400.0));
        assert_eq!(d.unconverged_rpm, vec![2850.0]);
    }
}
