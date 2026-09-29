//! Sound radiation from an open pipe end.
//!
//! Levine & Schwinger (1948), "On the radiation of sound from an unflanged circular pipe",
//! Phys. Rev. 73, 383. For a pipe of radius `a` with no mean flow, in the plane-wave range
//! (`ka < 1.84`), the pressure reflection coefficient at the open end is
//!
//! ```text
//! R(ka) = −|R| exp(−2 j k l),
//! |R|   = exp[ −(2ka/π) ∫₀^{ka} atan(−J₁(x)/Y₁(x)) / (x √(k²a² − x²)) dx ]
//! l/a   = (1/π) ∫₀^{ka} ln[π J₁(x) √(J₁² + Y₁²)] / (x √(k²a² − x²)) dx
//!       + (1/π) ∫₀^∞  ln[1 / (2 I₁(x) K₁(x))]   / (x √(x² + k²a²)) dx
//! ```
//!
//! (convention `p ∝ e^{jωt}`, `R = p⁻/p⁺` at the pipe end plane). The low-frequency limits are
//! `|R| → 1 − (ka)²/2` and `l/a → 0.6133`. The two `ka·ln ka` terms of the integrals cancel,
//! which fixes the sign of the first log.

use std::f64::consts::PI;

use rustfft::num_complex::Complex64;

use crate::math::{bessel_i1_scaled, bessel_j1, bessel_k1_scaled, bessel_y1, integrate};

/// Magnitude of the Levine–Schwinger reflection coefficient, unflanged pipe, `0 < ka < 3.83`.
pub fn ls_reflection_magnitude(ka: f64) -> f64 {
    // x = ka·sin φ removes the endpoint singularity: dx / √(k²a² − x²) = dφ.
    let theta_over_x = |phi: f64| {
        let x = ka * phi.sin();
        bessel_j1(x).atan2(-bessel_y1(x)) / x
    };
    let i = integrate(theta_over_x, 0.0, PI / 2.0, 16, 16);
    (-(2.0 * ka / PI) * i).exp()
}

/// Levine–Schwinger end correction `l/a`, unflanged pipe, `0 < ka < 3.83`.
pub fn ls_end_correction(ka: f64) -> f64 {
    let first = |phi: f64| {
        let x = ka * phi.sin();
        let j1 = bessel_j1(x);
        let y1 = bessel_y1(x);
        (PI * j1 * (j1 * j1 + y1 * y1).sqrt()).ln() / x
    };
    // x = ka·sinh t: dx / √(x² + k²a²) = dt; the integrand decays like t·e^{−t}.
    let t_max = (1e9 / ka).asinh();
    let second = |t: f64| {
        let x = ka * t.sinh();
        -(2.0 * bessel_i1_scaled(x) * bessel_k1_scaled(x)).ln() / x
    };
    (integrate(first, 0.0, PI / 2.0, 16, 16) + integrate(second, 0.0, t_max, 256, 8)) / PI
}

/// Complex Levine–Schwinger reflection coefficient `R = −|R| e^{−2jkl}`.
pub fn ls_reflection(ka: f64) -> Complex64 {
    let phase = -2.0 * ka * ls_end_correction(ka);
    -ls_reflection_magnitude(ka) * Complex64::from_polar(1.0, phase)
}

/// Normalised radiation impedance `Z/(ρc) = (1 + R)/(1 − R)` seen by plane waves in the pipe.
pub fn ls_impedance(ka: f64) -> Complex64 {
    let r = ls_reflection(ka);
    (1.0 + r) / (1.0 - r)
}

/// Causal rational fit of the Levine–Schwinger reflection coefficient for time-domain use.
///
/// In the dimensionless Laplace variable `s̃ = s a / c`,
/// ```text
/// R(s̃) = −(1 + 1.9480630 s̃ + 0.4124807 s̃² + 0.1094815 s̃³)
///        / (1 + 3.1726359 s̃ + 3.0534404 s̃² + 1.1321698 s̃³ + 0.1695408 s̃⁴)
///      = Σᵢ rᵢ / (s̃ − pᵢ)
/// ```
/// fitted by Sanathanan–Koerner iteration to [`ls_reflection`] on `0 < ka ≤ 2.2` (relative
/// weighting). Properties checked by the tests below: `R(0) = −1` exactly (so the mean exit
/// pressure is ambient), all poles in the left half-plane (causal, stable),
/// `|R(jω)| < 1` for all ω (passive: the boundary never injects energy), strictly proper
/// (no algebraic loop with the incident wave). Error against the exact integrals for
/// `ka ≤ 1.5`: |R| < 0.02 %, l/a < 0.1 %.
///
/// Implemented in modal form: `dzᵢ/dτ = pᵢ zᵢ + W⁺(τ)`, `W⁻ = Σ rᵢ zᵢ`, `τ = t c / a`,
/// integrated exactly for an incident wave `W⁺` that is linear over each step.
#[derive(Clone, Copy, Debug, Default)]
pub struct LsFilter {
    z_pair: Complex64,
    z_real: [f64; 2],
}

impl LsFilter {
    const P_PAIR: Complex64 = Complex64::new(-2.443_159_988_249_841, 1.612_003_367_282_311);
    const R_PAIR: Complex64 = Complex64::new(0.747_240_024_315_667, -0.903_254_419_139_656);
    const P_REAL: [f64; 2] = [-0.558_188_424_302_027, -1.233_350_137_229_074];
    const R_REAL: [f64; 2] = [-0.031_372_511_498_935, -2.108_860_662_810_9];

    /// Reflected wave `W⁻` for the current state.
    #[inline]
    pub fn output(&self) -> f64 {
        2.0 * (Self::R_PAIR * self.z_pair).re
            + Self::R_REAL[0] * self.z_real[0]
            + Self::R_REAL[1] * self.z_real[1]
    }

    /// State after a step of dimensionless length `h = Δt c / a` during which the incident
    /// wave goes linearly from `w0` to `w1`:
    /// `z(h) = e^{ph} z + h[(φ₁ − φ₂) w0 + φ₂ w1]`, `φ₁(x) = (eˣ − 1)/x`, `φ₂(x) = (eˣ − 1 − x)/x²`.
    pub fn advanced(&self, h: f64, w0: f64, w1: f64) -> Self {
        let (e, p1, p2) = phi_complex(Self::P_PAIR * h);
        let z_pair = e * self.z_pair + h * ((p1 - p2) * w0 + p2 * w1);
        let mut z_real = [0.0; 2];
        for (i, z) in z_real.iter_mut().enumerate() {
            let (e, p1, p2) = phi_real(Self::P_REAL[i] * h);
            *z = e * self.z_real[i] + h * ((p1 - p2) * w0 + p2 * w1);
        }
        Self { z_pair, z_real }
    }

    /// Frequency response of the fit at `ka`.
    pub fn response(ka: f64) -> Complex64 {
        let s = Complex64::new(0.0, ka);
        Self::R_PAIR / (s - Self::P_PAIR)
            + Self::R_PAIR.conj() / (s - Self::P_PAIR.conj())
            + Self::R_REAL[0] / (s - Self::P_REAL[0])
            + Self::R_REAL[1] / (s - Self::P_REAL[1])
    }
}

fn phi_real(x: f64) -> (f64, f64, f64) {
    let e = x.exp();
    if x.abs() < 1e-4 {
        (e, 1.0 + x / 2.0 + x * x / 6.0, 0.5 + x / 6.0 + x * x / 24.0)
    } else {
        (e, (e - 1.0) / x, (e - 1.0 - x) / (x * x))
    }
}

fn phi_complex(x: Complex64) -> (Complex64, Complex64, Complex64) {
    let e = x.exp();
    if x.norm() < 1e-4 {
        (e, 1.0 + x / 2.0 + x * x / 6.0, 0.5 + x / 6.0 + x * x / 24.0)
    } else {
        (e, (e - 1.0) / x, (e - 1.0 - x) / (x * x))
    }
}

/// Complex acoustic pressure at `receiver` from point monopoles of complex volume-velocity
/// amplitude `q[i]` (m³/s) at `sources[i]`, above a rigid ground plane `z = ground_z`
/// (image sources, reflection coefficient +1). Free-field Green's function with `e^{jωt}`:
/// `p = jωρ₀ Q e^{−jkr} / (4π r)`, `k = ω/c₀`, ambient `ρ₀`, `c₀`. Positions in metres.
pub fn monopole_pressure(
    omega: f64,
    rho0: f64,
    c0: f64,
    sources: &[[f64; 3]],
    q: &[Complex64],
    receiver: [f64; 3],
    ground_z: f64,
) -> Complex64 {
    let k = omega / c0;
    let mut p = Complex64::new(0.0, 0.0);
    for (s, &qi) in sources.iter().zip(q) {
        let image = [s[0], s[1], 2.0 * ground_z - s[2]];
        for src in [*s, image] {
            let r = ((receiver[0] - src[0]).powi(2)
                + (receiver[1] - src[1]).powi(2)
                + (receiver[2] - src[2]).powi(2))
            .sqrt();
            p += Complex64::new(0.0, omega * rho0) * qi * Complex64::from_polar(1.0, -k * r)
                / (4.0 * PI * r);
        }
    }
    p
}

/// A-weighting in dB (IEC 61672-1:2013, Annex E).
pub fn a_weighting_db(f: f64) -> f64 {
    let f2 = f * f;
    let ra = 12194.0_f64.powi(2) * f2 * f2
        / ((f2 + 20.6_f64.powi(2))
            * ((f2 + 107.7_f64.powi(2)) * (f2 + 737.9_f64.powi(2))).sqrt()
            * (f2 + 12194.0_f64.powi(2)));
    20.0 * ra.log10() + 2.00
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_frequency_limits() {
        // Levine & Schwinger report l/a = 0.6127 at ka → 0; later evaluations give 0.6133.
        assert!(
            (ls_end_correction(0.01) - 0.6130).abs() < 5e-4,
            "{}",
            ls_end_correction(0.01)
        );
        let ka = 0.05;
        assert!((ls_reflection_magnitude(ka) - (-(ka * ka) / 2.0).exp()).abs() < 1e-5);
    }

    /// Independent cross-check against the (non-causal) Padé fits of Silva, Guillemain,
    /// Kergomard, Mallaroni & Norris (2009), J. Sound Vib. 322, Table 1, which approximate
    /// their own evaluation of the Levine–Schwinger solution to about 1–2 %.
    #[test]
    fn agrees_with_silva_2009_fits() {
        for ka in [0.1, 0.3, 0.5, 0.7, 1.0, 1.5, 2.0] {
            let x2: f64 = ka * ka;
            let r_silva =
                (1.0 + 0.800 * x2) / (1.0 + 1.300 * x2 + 0.266 * x2 * x2 + 0.0263 * x2 * x2 * x2);
            let l_silva = 0.6133 * (1.0 + 0.0599 * x2)
                / (1.0 + 0.238 * x2 - 0.0153 * x2 * x2 + 0.00150 * x2 * x2 * x2);
            let (r, l) = (ls_reflection_magnitude(ka), ls_end_correction(ka));
            assert!(
                (r / r_silva - 1.0).abs() < 0.01,
                "ka {ka}: |R| {r} vs {r_silva}"
            );
            assert!(
                (l / l_silva - 1.0).abs() < 0.015,
                "ka {ka}: l/a {l} vs {l_silva}"
            );
        }
    }

    #[test]
    fn rational_fit_matches_exact_and_is_passive() {
        assert!((LsFilter::response(0.0) + 1.0).norm() < 1e-12);
        for ka in [0.1, 0.25, 0.5, 0.75, 1.0, 1.25, 1.5] {
            let fit = LsFilter::response(ka);
            let l_fit = -(-fit).arg() / (2.0 * ka);
            assert!(
                (fit.norm() / ls_reflection_magnitude(ka) - 1.0).abs() < 2e-4,
                "ka {ka}"
            );
            assert!(
                (l_fit / ls_end_correction(ka) - 1.0).abs() < 1e-3,
                "ka {ka}"
            );
        }
        for i in 0..=4000 {
            let ka = 1e-4 * 1e8f64.powf(i as f64 / 4000.0);
            assert!(
                LsFilter::response(ka).norm() < 1.0,
                "not passive at ka {ka}"
            );
        }
    }

    /// The exponential integrator reproduces the fit's frequency response for a sampled sine.
    #[test]
    fn filter_time_integration_matches_response() {
        let ka = 0.8;
        let h = 0.05;
        let mut f = LsFilter::default();
        let (mut sum_c, mut sum_s) = (Complex64::new(0.0, 0.0), Complex64::new(0.0, 0.0));
        let steps = 40_000;
        for i in 0..steps {
            let (t0, t1) = (i as f64 * h, (i + 1) as f64 * h);
            f = f.advanced(h, (ka * t0).cos(), (ka * t1).cos());
            if i > steps / 2 {
                let w = Complex64::from_polar(1.0, -ka * t1);
                sum_c += f.output() * w;
                sum_s += (ka * t1).cos() * w;
            }
        }
        let measured = sum_c / sum_s;
        assert!(
            (measured - LsFilter::response(ka)).norm() < 2e-3,
            "{measured} vs {}",
            LsFilter::response(ka)
        );
    }

    #[test]
    fn a_weighting_reference_points() {
        assert!(a_weighting_db(1000.0).abs() < 0.01);
        assert!((a_weighting_db(100.0) + 19.1).abs() < 0.1);
        assert!((a_weighting_db(50.0) + 30.2).abs() < 0.1);
    }
}
