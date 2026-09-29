//! Oscillating wall boundary layers (Stokes layers): the frequency-dependent part of wall
//! friction and heat transfer that quasi-steady correlations miss.
//!
//! For a thin acoustic boundary layer the wall shear and the heat flux into the gas are
//! half-order time derivatives of the core velocity and temperature (Stokes' second problem):
//! ```text
//! τ_w = √(ρμ) ∂^{1/2}u/∂t^{1/2},      q_w = −√(ρ c_p k) ∂^{1/2}T/∂t^{1/2}
//! ```
//! acting on the cell through the wetted perimeter `4/D` (momentum sink `τ_w 4/D`, heat
//! source `q_w 4/D`; the shear moves no energy, so the kinetic energy it removes becomes heat).
//! For `e^{jωt}` these give `√(jω)` factors, and in a plane wave they reproduce Kirchhoff's
//! attenuation `α = (1/(r c)) √(ων/2) (1 + (γ−1)/√Pr)` used by the four-pole model, so the two
//! solvers carry the same wall damping. The derivative is taken in the Caputo sense (of the
//! rate of change), so a steady state produces none: mean-flow friction and heat transfer stay
//! with the quasi-steady correlations and are not counted twice.
//!
//! Diffusive representation (Montseny 1998): `∂^{1/2}u = Σₖ wₖ φₖ`, `dφₖ/dt = −sₖ φₖ + du/dt`,
//! from `√(jω) = (jω/π) ∫₀^∞ s^{−½}/(jω + s) ds` with `s = eˣ` and the trapezoidal rule in `x`
//! (step `h` = 1.5, `x` spanning the audio band with twelve e-folds of margin each side):
//! `sₖ = e^{xₖ}`, `wₖ = h e^{xₖ/2}/π`. Each φₖ is advanced exactly for a rate constant over
//! the step.

/// Band the representation is accurate over, rad/s.
const OMEGA_MIN: f64 = 30.0;
const OMEGA_MAX: f64 = 3.0e4;

/// Poles and weights of the half-order derivative.
#[derive(Clone, Debug)]
pub struct HalfDerivative {
    pub poles: Vec<f64>,
    pub weights: Vec<f64>,
}

impl Default for HalfDerivative {
    fn default() -> Self {
        Self::band(OMEGA_MIN, OMEGA_MAX)
    }
}

impl HalfDerivative {
    /// Representation accurate for `|s|` from `omega_min` to `omega_max`, rad/s.
    pub fn band(omega_min: f64, omega_max: f64) -> Self {
        // Tails beyond the ends contribute ≈ (2/π)√(s_min/ω) and (2/π)√(ω/s_max): 12 e-folds of
        // margin keeps each under 0.2 %; the trapezoidal error at step 1.5 is ~e^{−π²/1.5}.
        let (x0, x1, h) = (omega_min.ln() - 12.0, omega_max.ln() + 12.0, 1.5);
        let n = ((x1 - x0) / h).ceil() as usize + 1;
        let xs: Vec<f64> = (0..n).map(|k| x0 + h * k as f64).collect();
        Self {
            poles: xs.iter().map(|x| x.exp()).collect(),
            weights: xs
                .iter()
                .map(|x| h * (x / 2.0).exp() / std::f64::consts::PI)
                .collect(),
        }
    }

    /// Number of memory states per input.
    pub fn states(&self) -> usize {
        self.poles.len()
    }

    /// Frequency response of the representation, ≈ √(jω).
    pub fn response(&self, omega: f64) -> rustfft::num_complex::Complex64 {
        let jw = rustfft::num_complex::Complex64::new(0.0, omega);
        self.poles
            .iter()
            .zip(&self.weights)
            .map(|(s, w)| w * jw / (jw + s))
            .sum()
    }

    /// Exact one-step factors for step `dt`: `φ ← decay φ + gain Δu` with
    /// `decay = e^{−s Δt}`, `gain = (1 − e^{−s Δt})/(s Δt)` (input rate constant over the step).
    pub fn step_factors(&self, dt: f64) -> (Vec<f64>, Vec<f64>) {
        self.poles
            .iter()
            .map(|s| {
                let a = s * dt;
                let decay = (-a).exp();
                (
                    decay,
                    if a > 1e-8 {
                        (1.0 - decay) / a
                    } else {
                        1.0 - 0.5 * a
                    },
                )
            })
            .unzip()
    }

    /// Advances `phi` for an input change `du` over the step and returns `Σ wₖ φₖ`.
    #[inline]
    pub fn advance(&self, phi: &mut [f64], du: f64, decay: &[f64], gain: &[f64]) -> f64 {
        let mut sum = 0.0;
        for (((f, w), d), g) in phi.iter_mut().zip(&self.weights).zip(decay).zip(gain) {
            *f = d * *f + g * du;
            sum += w * *f;
        }
        sum
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn represents_sqrt_jw_across_the_band() {
        let h = HalfDerivative::default();
        for i in 0..=60 {
            let omega = OMEGA_MIN * (OMEGA_MAX / OMEGA_MIN).powf(i as f64 / 60.0);
            let exact = rustfft::num_complex::Complex64::new(0.0, omega).sqrt();
            let err = (h.response(omega) - exact).norm() / exact.norm();
            assert!(err < 0.01, "ω = {omega}: relative error {err}");
        }
    }
}
