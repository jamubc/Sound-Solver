//! Thermodynamic and transport properties of exhaust gas.
//!
//! Equation of state (ideal gas): `p = ρ R T`.
//! Thermally perfect gas: `cp = cp(T)`, `cv = cp − R`, `γ(T) = cp/cv`, `c = sqrt(γ R T)`.
//! Internal energy `e(T) = ∫₀ᵀ cv dT'`, with `cv` piecewise linear between table nodes and held
//! constant outside the table, so `e` is continuous and strictly increasing and `T(e)` is unique.
//!
//! Mixture `cp` is the mole-fraction-weighted sum of species molar heat capacities from the
//! NIST-JANAF Thermochemical Tables (Chase 1998, 4th ed.) divided by the mixture molar mass.
//! Composition is frozen complete combustion of a `CH_y` fuel in dry air at air-excess ratio
//! `λ ≥ 1`. Assumptions: no dissociation (valid below ≈1800 K), water stays vapour, composition
//! does not change along the pipe.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Universal gas constant, J/(mol K).
pub const R_UNIVERSAL: f64 = 8.314_462_618;

/// Identifies the species data and interpolation scheme; part of every result's provenance.
pub const GAS_TABLE_VERSION: &str = "nist-janaf-4ed/linear-cp/100K/v1";

const JANAF_T0: f64 = 200.0;
const JANAF_DT: f64 = 100.0;
// Molar heat capacity Cp°, J/(mol K), at T = 200, 300, …, 2000 K (NIST-JANAF, 4th ed.).
const CP_N2: [f64; 19] = [
    29.107, 29.125, 29.249, 29.580, 30.110, 30.754, 31.433, 32.090, 32.697, 33.241, 33.723, 34.147,
    34.518, 34.843, 35.128, 35.380, 35.603, 35.803, 35.971,
];
const CP_O2: [f64; 19] = [
    29.126, 29.385, 30.106, 31.091, 32.090, 32.981, 33.733, 34.355, 34.870, 35.300, 35.667, 35.988,
    36.277, 36.544, 36.796, 37.040, 37.277, 37.510, 37.741,
];
const CP_CO2: [f64; 19] = [
    32.359, 37.221, 41.325, 44.627, 47.321, 49.564, 51.434, 52.999, 54.308, 55.409, 56.342, 57.137,
    57.802, 58.379, 58.886, 59.317, 59.701, 60.049, 60.350,
];
const CP_H2O: [f64; 19] = [
    33.349, 33.596, 34.262, 35.226, 36.325, 37.495, 38.721, 39.987, 41.268, 42.536, 43.768, 44.945,
    46.054, 47.090, 48.050, 48.935, 49.749, 50.496, 51.180,
];
const CP_AR: f64 = 20.786;

// Molar masses, kg/mol.
const W_N2: f64 = 28.0134e-3;
const W_O2: f64 = 31.9988e-3;
const W_CO2: f64 = 44.0095e-3;
const W_H2O: f64 = 18.0153e-3;
const W_AR: f64 = 39.948e-3;
const W_C: f64 = 12.011e-3;
const W_H: f64 = 1.008e-3;

// Dry air per mole O2 (mole fractions O2 0.20946, N2 0.78084, Ar 0.00934; CO2 neglected).
const AIR_N2_PER_O2: f64 = 3.7279;
const AIR_AR_PER_O2: f64 = 0.04459;

/// Mole fractions of the burned gas.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Composition {
    pub n2: f64,
    pub o2: f64,
    pub co2: f64,
    pub h2o: f64,
    pub ar: f64,
}

impl Composition {
    /// Complete combustion of `CH_y` in dry air at air-excess ratio `lambda`:
    /// `CH_y + λ a (O2 + 3.7279 N2 + 0.04459 Ar) → CO2 + y/2 H2O + (λ−1) a O2 + …`, `a = 1 + y/4`.
    pub fn combustion_products(h_to_c: f64, lambda: f64) -> Result<Self> {
        if !(h_to_c > 0.0 && h_to_c < 4.0) {
            return Err(Error::invalid("gas.fuel_h_to_c must be in (0, 4)"));
        }
        if !(1.0..=10.0).contains(&lambda) {
            return Err(Error::invalid(
                "gas.lambda must be in [1, 10]: rich products (CO, H2) are not modelled",
            ));
        }
        let a = 1.0 + h_to_c / 4.0;
        let (co2, h2o, o2) = (1.0, h_to_c / 2.0, (lambda - 1.0) * a);
        let (n2, ar) = (AIR_N2_PER_O2 * lambda * a, AIR_AR_PER_O2 * lambda * a);
        let total = co2 + h2o + o2 + n2 + ar;
        Ok(Self {
            n2: n2 / total,
            o2: o2 / total,
            co2: co2 / total,
            h2o: h2o / total,
            ar: ar / total,
        })
    }

    /// Mixture molar mass, kg/mol.
    pub fn molar_mass(&self) -> f64 {
        self.n2 * W_N2 + self.o2 * W_O2 + self.co2 * W_CO2 + self.h2o * W_H2O + self.ar * W_AR
    }

    /// Stoichiometric air-fuel mass ratio of `CH_y` in the same dry air.
    pub fn stoichiometric_afr(h_to_c: f64) -> f64 {
        let a = 1.0 + h_to_c / 4.0;
        a * (W_O2 + AIR_N2_PER_O2 * W_N2 + AIR_AR_PER_O2 * W_AR) / (W_C + h_to_c * W_H)
    }
}

/// Thermally perfect ideal gas with tabulated `cp(T)`.
#[derive(Clone, Debug)]
pub struct Gas {
    r: f64,
    t0: f64,
    dt: f64,
    cv: Vec<f64>,
    e: Vec<f64>,
    label: String,
}

impl Gas {
    /// Calorically perfect gas (constant γ). Used by analytic validation cases.
    pub fn perfect(gamma: f64, r: f64) -> Self {
        let cv = r / (gamma - 1.0);
        Self::from_cv_table(
            r,
            0.0,
            1.0,
            vec![cv, cv],
            format!("perfect/gamma={gamma}/R={r}"),
        )
    }

    /// Frozen combustion products of `CH_y` fuel at air-excess ratio `lambda`.
    pub fn exhaust(h_to_c: f64, lambda: f64) -> Result<Self> {
        let x = Composition::combustion_products(h_to_c, lambda)?;
        let w = x.molar_mass();
        let r = R_UNIVERSAL / w;
        let cv = (0..CP_N2.len())
            .map(|i| {
                let cp_molar = x.n2 * CP_N2[i]
                    + x.o2 * CP_O2[i]
                    + x.co2 * CP_CO2[i]
                    + x.h2o * CP_H2O[i]
                    + x.ar * CP_AR;
                cp_molar / w - r
            })
            .collect();
        let label = format!("{GAS_TABLE_VERSION}/CH{h_to_c}/lambda={lambda}");
        Ok(Self::from_cv_table(r, JANAF_T0, JANAF_DT, cv, label))
    }

    fn from_cv_table(r: f64, t0: f64, dt: f64, cv: Vec<f64>, label: String) -> Self {
        let mut e = Vec::with_capacity(cv.len());
        e.push(cv[0] * t0);
        for k in 1..cv.len() {
            e.push(e[k - 1] + 0.5 * (cv[k - 1] + cv[k]) * dt);
        }
        Self {
            r,
            t0,
            dt,
            cv,
            e,
            label,
        }
    }

    /// Specific gas constant, J/(kg K).
    #[inline]
    pub fn r(&self) -> f64 {
        self.r
    }

    /// Provenance label: table version and composition.
    pub fn label(&self) -> &str {
        &self.label
    }

    #[inline]
    fn node(&self, t: f64) -> Option<(usize, f64)> {
        let s = (t - self.t0) / self.dt;
        if s < 0.0 || s >= (self.cv.len() - 1) as f64 {
            return None;
        }
        let k = s as usize;
        Some((k, t - (self.t0 + k as f64 * self.dt)))
    }

    /// Specific heat at constant volume, J/(kg K).
    #[inline]
    pub fn cv(&self, t: f64) -> f64 {
        match self.node(t) {
            Some((k, x)) => self.cv[k] + (self.cv[k + 1] - self.cv[k]) * x / self.dt,
            None if t < self.t0 => self.cv[0],
            None => self.cv[self.cv.len() - 1],
        }
    }

    /// Specific heat at constant pressure, J/(kg K).
    #[inline]
    pub fn cp(&self, t: f64) -> f64 {
        self.cv(t) + self.r
    }

    /// Ratio of specific heats.
    #[inline]
    pub fn gamma(&self, t: f64) -> f64 {
        let cv = self.cv(t);
        (cv + self.r) / cv
    }

    /// Isentropic speed of sound `c = sqrt(γ R T)`, m/s.
    #[inline]
    pub fn sound_speed(&self, t: f64) -> f64 {
        (self.gamma(t) * self.r * t).sqrt()
    }

    /// Specific internal energy, J/kg.
    #[inline]
    pub fn e(&self, t: f64) -> f64 {
        match self.node(t) {
            Some((k, x)) => {
                let slope = (self.cv[k + 1] - self.cv[k]) / self.dt;
                self.e[k] + self.cv[k] * x + 0.5 * slope * x * x
            }
            None if t < self.t0 => self.cv[0] * t,
            None => {
                let n = self.cv.len() - 1;
                self.e[n] + self.cv[n] * (t - (self.t0 + n as f64 * self.dt))
            }
        }
    }

    /// Specific enthalpy `h = e + R T`, J/kg.
    #[inline]
    pub fn h(&self, t: f64) -> f64 {
        self.e(t) + self.r * t
    }

    /// Inverse of `e(T)`.
    #[inline]
    pub fn t_from_e(&self, e: f64) -> f64 {
        let n = self.cv.len() - 1;
        if e <= self.e[0] {
            return e / self.cv[0];
        }
        if e >= self.e[n] {
            return self.t0 + n as f64 * self.dt + (e - self.e[n]) / self.cv[n];
        }
        let k = self.e.partition_point(|&ek| ek <= e) - 1;
        let de = e - self.e[k];
        let slope = (self.cv[k + 1] - self.cv[k]) / self.dt;
        // Root of ½ s x² + cv_k x − de = 0 in the cancellation-free form.
        let x = 2.0 * de / (self.cv[k] + (self.cv[k] * self.cv[k] + 2.0 * slope * de).sqrt());
        self.t0 + k as f64 * self.dt + x
    }

    /// Inverse of `h(T)`.
    pub fn t_from_h(&self, h: f64) -> f64 {
        // h = e(T) + R T is strictly increasing; Newton from the constant-cp estimate converges
        // in a few steps because cp varies slowly.
        let mut t = h / self.cp(1000.0);
        for _ in 0..50 {
            let f = self.h(t) - h;
            let step = f / self.cp(t);
            t -= step;
            if step.abs() < 1e-10 * t {
                break;
            }
        }
        t
    }

    /// Dynamic viscosity, Pa s. Sutherland's law with air constants
    /// (μ_ref = 1.716e-5 Pa s at 273.15 K, S = 110.4 K); exhaust gas is ≈73 % N2, so air values
    /// are used (White, Viscous Fluid Flow, 3rd ed., Table 1-2).
    #[inline]
    pub fn viscosity(&self, t: f64) -> f64 {
        const MU_REF: f64 = 1.716e-5;
        const T_REF: f64 = 273.15;
        const S: f64 = 110.4;
        MU_REF * (t / T_REF).powf(1.5) * (T_REF + S) / (t + S)
    }

    /// Prandtl number; taken constant at 0.70 for combustion products.
    #[inline]
    pub fn prandtl(&self) -> f64 {
        0.70
    }

    /// Thermal conductivity `k = μ cp / Pr`, W/(m K).
    #[inline]
    pub fn conductivity(&self, t: f64) -> f64 {
        self.viscosity(t) * self.cp(t) / self.prandtl()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stoichiometric_gasoline_exhaust_properties() {
        let gas = Gas::exhaust(1.87, 1.0).unwrap();
        assert!((gas.r() - 286.6).abs() < 0.5, "R = {}", gas.r());
        assert!(
            (gas.gamma(300.0) - 1.3715).abs() < 1e-3,
            "γ(300) = {}",
            gas.gamma(300.0)
        );
        assert!(
            (gas.gamma(900.0) - 1.3027).abs() < 1e-3,
            "γ(900) = {}",
            gas.gamma(900.0)
        );
        assert!((Composition::stoichiometric_afr(1.87) - 14.6).abs() < 0.05);
    }

    #[test]
    fn energy_inverse_round_trips_inside_and_outside_table() {
        let gas = Gas::exhaust(1.87, 1.0).unwrap();
        for t in [50.0, 199.0, 200.0, 250.0, 777.7, 1999.0, 2000.0, 2600.0] {
            let back = gas.t_from_e(gas.e(t));
            assert!((back - t).abs() < 1e-9 * t, "T = {t}, back = {back}");
            assert!((gas.t_from_h(gas.h(t)) - t).abs() < 1e-7 * t);
        }
    }

    #[test]
    fn perfect_gas_is_linear() {
        let gas = Gas::perfect(1.4, 287.0);
        let cv = 287.0 / 0.4;
        for t in [0.5, 1.0, 300.0, 3000.0] {
            assert!((gas.e(t) - cv * t).abs() < 1e-9 * cv * t);
            assert!((gas.gamma(t) - 1.4).abs() < 1e-12);
        }
    }
}
