//! Fibrous fill as a rigid-frame equivalent fluid: the Johnson–Champoux–Allard model with the
//! parameters of a fibre bed (Allard & Champoux 1992): tortuosity 1, viscous length
//! `Λ = √(8μ/(σφ))` and thermal length `Λ' = 2Λ`. For the pore gas (velocity `u`, pressure
//! `p`, density `ρ`):
//! ```text
//! jω ρ̃ u = −∂p/∂x,   ρ̃ = ρ + (σφ/jω) √(1 + jω/ω_v),                  ω_v = 2σφ/ρ
//! jω p = −K̃ ∂u/∂x,   K̃ = γp (jω + L)/(jω + γL),   L = ω_t √(1 + jω/(2ω_t)),   ω_t = σφ/(4 Pr ρ)
//! ```
//! Darcy flow and isothermal compression at low frequency, boundary layers on the fibres at
//! high; causal and passive at every frequency. Where Delany & Bazley's fit holds
//! (0.01 < f/σ < 1) it follows that fit's causal form (Miki 1990) within 15 %; below it, where
//! exhaust packings sit at drone frequencies, the power laws extrapolate while this keeps the
//! Darcy and isothermal limits. `σ(T) = σ μ(T)/μ(293.15 K)`: resistivity is viscous.
//!
//! `K̃` is the gas exchanging heat `q = −ρc_p L(T − T_f)` with fibres held at `T_f`. In the time
//! domain both square roots are `√(s + a)` operators (`s = d/dt`): the drag per unit pore
//! volume is `(σφ/√ω_v) √(s + ω_v) u` and the heat exchange `ρc_p √(ω_t/2) √(s + 2ω_t) (T − T_f)`,
//! represented diffusively like the Stokes layers (`stokes`) with every pole shifted by `a`.

use rustfft::num_complex::Complex64 as C64;

use super::stokes::HalfDerivative;
use crate::gas::Gas;

/// Temperature at which `Porous::resistivity` is given, K.
const T_REF: f64 = 293.15;

/// Rigid-frame fibrous fill.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Porous {
    /// Static flow resistivity σ at 293.15 K, Pa s/m² (pressure gradient per superficial
    /// velocity).
    pub resistivity: f64,
    /// Volume fraction open to the gas, φ.
    pub porosity: f64,
}

impl Porous {
    /// `σφ` at gas temperature `t`: static drag per unit pore volume and pore velocity, Pa s/m².
    pub fn drag(&self, gas: &Gas, t: f64) -> f64 {
        self.resistivity * self.porosity * gas.viscosity(t) / gas.viscosity(T_REF)
    }

    /// Viscous and thermal rates `(ω_v, ω_t)`, 1/s, at gas density `rho` and temperature `t`.
    pub fn rates(&self, gas: &Gas, rho: f64, t: f64) -> (f64, f64) {
        let d = self.drag(gas, t) / rho;
        (2.0 * d, d / (4.0 * gas.prandtl()))
    }

    /// Dynamic density `ρ̃` and bulk modulus `K̃` of the pore gas at `(rho, t)`.
    pub fn equivalent_fluid(&self, gas: &Gas, rho: f64, t: f64, omega: f64) -> (C64, C64) {
        let (wv, wt) = self.rates(gas, rho, t);
        let jw = C64::new(0.0, omega);
        let rho_e = rho + self.drag(gas, t) * (1.0 + jw / wv).sqrt() / jw;
        let l = wt * (1.0 + jw / (2.0 * wt)).sqrt();
        let gamma = gas.gamma(t);
        (
            rho_e,
            gamma * rho * gas.r() * t * (jw + l) / (jw + gamma * l),
        )
    }
}

/// Band of the time-domain representation, rad/s: `|s + a|` for audio `s` and any fill's rates.
pub(super) const BAND: (f64, f64) = (1e2, 1e7);

/// Relaxes `x` over a step `dt` under `dx/dt = −c √(s + a) x` by backward Euler, from its value
/// after the rest of the step. `√(s + a) x = Σ wₖ(x − sₖψₖ)` with `dψₖ/dt = x − (sₖ + a)ψₖ`,
/// each `ψₖ` integrated exactly with `x` held at its new value; `pole_decay[k] = e^{−sₖ dt}`.
/// The memory is kept (a predictor) unless `advance`.
#[allow(clippy::too_many_arguments)]
pub(super) fn relax(
    rep: &HalfDerivative,
    pole_decay: &[f64],
    psi: &mut [f64],
    x: f64,
    a: f64,
    c: f64,
    dt: f64,
    advance: bool,
) -> f64 {
    let ea = (-a * dt).exp();
    let (mut sa, mut sb) = (0.0, 0.0);
    for (((&s, &w), &e), &p) in rep
        .poles
        .iter()
        .zip(&rep.weights)
        .zip(pole_decay)
        .zip(&*psi)
    {
        let d = e * ea;
        // w (1 − s g) with g = (1 − d)/(s + a), without cancellation.
        sa += w * (a + s * d) / (s + a);
        sb += w * s * d * p;
    }
    let x1 = (x + c * dt * sb) / (1.0 + c * dt * sa);
    if advance {
        for ((&s, &e), p) in rep.poles.iter().zip(pole_decay).zip(psi.iter_mut()) {
            let d = e * ea;
            *p = d * *p + (1.0 - d) / (s + a) * x1;
        }
    }
    x1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elements::fill_properties;

    /// Miki (1990): `Z_c` on the bulk area and `k`, against `f/σ`.
    fn miki(x: f64) -> (C64, C64) {
        let (az, ak) = (x.powf(-0.632), x.powf(-0.618));
        (
            C64::new(1.0 + 0.070 * az, -0.107 * az),
            C64::new(1.0 + 0.109 * ak, -0.160 * ak),
        )
    }

    #[test]
    fn follows_miki_where_delany_bazley_holds() {
        let gas = Gas::perfect(1.4, 287.05);
        let t = T_REF;
        let rho = 101_325.0 / (gas.r() * t);
        let c = gas.sound_speed(t);
        for bulk in [25.0, 50.0, 100.0, 150.0, 200.0, 250.0] {
            let fill = fill_properties(bulk, 12e-6);
            for i in 0..=40 {
                let x = 0.01 * 100f64.powf(i as f64 / 40.0);
                let omega = 2.0 * std::f64::consts::PI * x * fill.resistivity;
                let (rho_e, k_e) = fill.equivalent_fluid(&gas, rho, t, omega);
                let z = (rho_e * k_e).sqrt() / fill.porosity / (rho * c);
                let k = (rho_e / k_e).sqrt() * c;
                let (zm, km) = miki(x);
                assert!(
                    (z / zm - 1.0).norm() < 0.15,
                    "ρ_b {bulk}, f/σ {x}: Z {z} vs {zm}"
                );
                assert!(
                    (k / km - 1.0).norm() < 0.15,
                    "ρ_b {bulk}, f/σ {x}: k {k} vs {km}"
                );
            }
        }
    }
}
