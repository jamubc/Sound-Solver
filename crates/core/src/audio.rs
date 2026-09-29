//! Hearing the prediction (directive, "Sound evaluation": playback by inverse transform of the
//! solved spectrum with its solved phase). The receiver pressure is resynthesised from the
//! solved spectrum lines, each a sinusoid of its engine order at its RMS pressure
//! `p₀ 10^(L/20)` and solved phase; nothing is added: no flow noise, nothing above the
//! solver's `max_frequency_hz`. Inside the cabin each line takes the measured cabin transfer
//! function's gain at its frequency; lines outside the measured range are left out and counted.
//!
//! The engine speed runs from `from_rpm` to `to_rpm` linearly. Each order's complex amplitude
//! is interpolated between the solved speeds on either side and its phase advances with the
//! crank angle `θ(t) = 2π ∫ rpm/60 dt`, so the orders stay locked to the firing as in a real
//! run-up. At a single speed the sound is a loop holding a whole, even number of engine cycles,
//! with the sample rate set so the loop is exactly that long: it repeats without a seam.

use std::f64::consts::PI;

use rustfft::num_complex::Complex64;

use crate::error::{Error, Result};
use crate::solve::Line;

/// Output sample rate, Hz: the solved lines stop at `max_frequency_hz` (2 kHz by default).
const SAMPLE_RATE: f64 = 22_050.0;
/// Samples synthesised at one engine speed and amplitude set.
const BLOCK: usize = 64;
const P_REF: f64 = 20e-6;

/// Receiver pressure, Pa, sampled at `sample_rate`.
#[derive(Clone, Debug)]
pub struct Sound {
    pub sample_rate: f64,
    pub samples: Vec<f32>,
    pub peak_pa: f64,
    /// Engine orders left out somewhere because the cabin transfer function is not measured
    /// at their frequency.
    pub dropped: usize,
}

/// The lines of one solved point as `(engine order, complex peak amplitude, Pa)`.
fn orders(lines: &[Line]) -> Vec<(f64, Complex64)> {
    lines
        .iter()
        .map(|l| {
            let peak = std::f64::consts::SQRT_2 * P_REF * 10f64.powf(l.spl_db / 20.0);
            (l.order, Complex64::from_polar(peak, l.phase_rad))
        })
        .collect()
}

/// Sound of an engine speed ramp from `from_rpm` to `to_rpm` over `seconds` (a loop of at
/// least `seconds` when the two are equal), from solved `points` of `(rpm, lines)` in
/// increasing speed, each line through `gain_db(frequency)` (`None`: left out).
pub fn synthesize(
    points: &[(f64, Vec<Line>)],
    from_rpm: f64,
    to_rpm: f64,
    seconds: f64,
    gain_db: &dyn Fn(f64) -> Option<f64>,
) -> Result<Sound> {
    if points.is_empty() {
        return Err(Error::invalid("no solved engine speed to listen to"));
    }
    if !(from_rpm > 0.0 && to_rpm > 0.0 && seconds > 0.0) {
        return Err(Error::invalid(
            "engine speeds and duration must be positive",
        ));
    }
    let solved: Vec<(f64, Vec<(f64, Complex64)>)> = points
        .iter()
        .map(|(rpm, lines)| (*rpm, orders(lines)))
        .collect();
    // A single speed loops over a whole, even number of cycles (period-2 lines are half-cycle
    // harmonics).
    let (duration, sample_rate) = if from_rpm == to_rpm {
        let cycle = 120.0 / from_rpm;
        let cycles = 2.0 * (seconds / cycle / 2.0).ceil();
        let duration = cycles * cycle;
        (duration, (duration * SAMPLE_RATE).round() / duration)
    } else {
        (seconds, SAMPLE_RATE)
    };
    let n = (duration * sample_rate).round() as usize;
    let rpm_at = |t: f64| from_rpm + (to_rpm - from_rpm) * t / duration;
    // Crank angle, rad: the integral of the linear ramp.
    let theta_at =
        |t: f64| 2.0 * PI / 60.0 * (from_rpm * t + (to_rpm - from_rpm) * t * t / (2.0 * duration));
    let mut samples = vec![0f32; n];
    let mut dropped: Vec<f64> = Vec::new();
    for start in (0..n).step_by(BLOCK) {
        let len = BLOCK.min(n - start);
        let t0 = start as f64 / sample_rate;
        let rpm = rpm_at(t0 + 0.5 * len as f64 / sample_rate);
        let (theta0, step) = (theta_at(t0), 2.0 * PI * rpm / 60.0 / sample_rate);
        for (order, amplitude) in interpolate(&solved, rpm) {
            let a = match gain_db(order * rpm / 60.0) {
                Some(g) => amplitude * 10f64.powf(g / 20.0),
                None => {
                    if !dropped.contains(&order) {
                        dropped.push(order);
                    }
                    continue;
                }
            };
            let mut z = Complex64::from_polar(1.0, order * theta0);
            let w = Complex64::from_polar(1.0, order * step);
            for s in &mut samples[start..start + len] {
                *s += (a * z).re as f32;
                z *= w;
            }
        }
    }
    let peak_pa = samples.iter().fold(0.0, |m: f64, s| m.max(s.abs() as f64));
    Ok(Sound {
        sample_rate,
        samples,
        peak_pa,
        dropped: dropped.len(),
    })
}

/// Between two solved speeds: level and phase each linear (the phase along the shorter arc),
/// so an order whose phase turns between them keeps its level. A silent side takes the other's
/// phase.
fn blend(va: Complex64, vb: Complex64, x: f64) -> Complex64 {
    let (ra, pa) = va.to_polar();
    let (rb, pb) = vb.to_polar();
    let (pa, pb) = match (ra == 0.0, rb == 0.0) {
        (true, _) => (pb, pb),
        (_, true) => (pa, pa),
        _ => (pa, pb),
    };
    let turn = (pb - pa + PI).rem_euclid(2.0 * PI) - PI;
    Complex64::from_polar(ra + (rb - ra) * x, pa + turn * x)
}

/// Complex amplitude of every order at `rpm`: `blend` between the solved speeds on either
/// side (an order missing at one of them counts as silent there), the end speeds beyond the
/// range.
fn interpolate(solved: &[(f64, Vec<(f64, Complex64)>)], rpm: f64) -> Vec<(f64, Complex64)> {
    let k = solved.partition_point(|p| p.0 < rpm);
    if k == 0 {
        return solved[0].1.clone();
    }
    if k == solved.len() {
        return solved[k - 1].1.clone();
    }
    let ((ra, a), (rb, b)) = (&solved[k - 1], &solved[k]);
    let x = (rpm - ra) / (rb - ra);
    let at = |set: &[(f64, Complex64)], order: f64| {
        set.iter()
            .find(|(o, _)| (o - order).abs() < 1e-6)
            .map_or(Complex64::new(0.0, 0.0), |p| p.1)
    };
    let mut out: Vec<(f64, Complex64)> = a
        .iter()
        .map(|&(o, va)| (o, blend(va, at(b, o), x)))
        .collect();
    out.extend(
        b.iter()
            .filter(|(o, _)| !a.iter().any(|(oa, _)| (oa - o).abs() < 1e-6))
            .map(|&(o, vb)| (o, vb * x)),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(order: f64, rpm: f64, spl_db: f64) -> Line {
        let frequency_hz = order * rpm / 60.0;
        Line {
            frequency_hz,
            order,
            spl_db,
            spl_dba: spl_db,
            phase_rad: 0.3,
        }
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    /// 94 dB is 1 Pa RMS; a steady loop holds whole cycles; through a −20 dB cabin, 0.1 Pa.
    #[test]
    fn steady_loop_at_the_solved_level() {
        let points = vec![(1500.0, vec![line(2.0, 1500.0, 94.0)])];
        let sound = synthesize(&points, 1500.0, 1500.0, 1.0, &|_| Some(0.0)).unwrap();
        let seconds = sound.samples.len() as f64 / sound.sample_rate;
        assert!(
            (seconds * 50.0 - (seconds * 50.0).round()).abs() < 1e-9,
            "{seconds} s"
        );
        assert!(
            (rms(&sound.samples) - 1.0).abs() < 0.01,
            "{}",
            rms(&sound.samples)
        );
        let inside = synthesize(&points, 1500.0, 1500.0, 1.0, &|_| Some(-20.0)).unwrap();
        assert!((rms(&inside.samples) - 0.1).abs() < 0.001);
        let outside_tf = synthesize(&points, 1500.0, 1500.0, 1.0, &|_| None).unwrap();
        assert_eq!((outside_tf.peak_pa, outside_tf.dropped), (0.0, 1));
    }

    /// Mid-way through a 1000 → 3000 rpm run-up the firing order sounds at 2 × 2000/60 Hz.
    #[test]
    fn run_up_follows_the_engine_speed() {
        let points: Vec<(f64, Vec<Line>)> = [1000.0, 3000.0]
            .iter()
            .map(|&rpm| (rpm, vec![line(2.0, rpm, 94.0)]))
            .collect();
        let sound = synthesize(&points, 1000.0, 3000.0, 4.0, &|_| Some(0.0)).unwrap();
        let fs = sound.sample_rate;
        let window = &sound.samples[(1.75 * fs) as usize..(2.25 * fs) as usize];
        let crossings = window
            .windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count() as f64;
        let expected = 2.0 * 2000.0 / 60.0 * 0.5;
        assert!(
            (crossings - expected).abs() <= 1.0,
            "{crossings} vs {expected}"
        );
        assert!((rms(window) - 1.0).abs() < 0.02, "{}", rms(window));
    }

    /// An order whose phase turns 2.5 rad between two solved speeds keeps its level between
    /// them (a straight line between the two complex amplitudes dips to cos 1.25, −10 dB).
    #[test]
    fn run_up_keeps_level_while_phase_turns() {
        let points: Vec<(f64, Vec<Line>)> = [(1000.0, 0.0), (3000.0, 2.5)]
            .iter()
            .map(|&(rpm, phase_rad)| {
                let l = line(2.0, rpm, 94.0);
                (rpm, vec![Line { phase_rad, ..l }])
            })
            .collect();
        let sound = synthesize(&points, 1000.0, 3000.0, 4.0, &|_| Some(0.0)).unwrap();
        let fs = sound.sample_rate;
        let window = &sound.samples[(1.75 * fs) as usize..(2.25 * fs) as usize];
        assert!((rms(window) - 1.0).abs() < 0.02, "{}", rms(window));
    }
}
