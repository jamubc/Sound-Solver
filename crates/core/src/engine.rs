//! Engine exhaust source: single-zone cylinders discharging through the exhaust valves into
//! lumped manifold volumes (one per turbine scroll), which discharge through the turbine into
//! the downpipe (see `gas1d::nodes`).
//!
//! Cylinder model after exhaust-valve opening (EVO), per cylinder:
//! ```text
//! V(θ) = V_c + (π B²/4) (l + a − a cos θ − √(l² − a² sin² θ)),  a = S/2, V_c = V_d/(CR − 1)
//! dm/dt = −ṁ_valve
//! p = p_EVO (ρ/ρ_EVO)ⁿ,  ρ = m/V,  T = p/(ρR)       (polytropic expansion of the gas left in
//!                                                   the cylinder; n < γ accounts for heat loss)
//! ṁ_valve = C_d A_v(θ) ψ(p_cyl, T_cyl → p_manifold)  (isentropic orifice, reverse flow allowed)
//! ```
//! The in-cylinder state at EVO comes from [`estimate_evo_state`] (ideal Otto cycle) or is
//! entered directly. Combustion, gas exchange on the intake side and the residual fraction are
//! outside the model (directive non-goal).

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::gas::{Composition, Gas};

/// Slider-crank geometry and firing order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EngineGeometry {
    /// Bore, mm.
    pub bore_mm: f64,
    /// Stroke, mm.
    pub stroke_mm: f64,
    /// Connecting-rod centre distance, mm.
    pub rod_mm: f64,
    /// Geometric compression ratio.
    pub compression_ratio: f64,
    /// Firing order, 1-based cylinder numbers. Four-stroke: cylinders fire at equal intervals
    /// of 720°/n.
    pub firing_order: Vec<usize>,
}

impl EngineGeometry {
    pub fn cylinders(&self) -> usize {
        self.firing_order.len()
    }

    /// Swept volume of one cylinder, m³.
    pub fn displacement(&self) -> f64 {
        std::f64::consts::PI / 4.0 * (self.bore_mm * 1e-3).powi(2) * self.stroke_mm * 1e-3
    }

    /// Clearance volume, m³.
    pub fn clearance_volume(&self) -> f64 {
        self.displacement() / (self.compression_ratio - 1.0)
    }

    /// Cylinder volume at crank angle `theta_deg` after firing TDC, m³.
    pub fn volume(&self, theta_deg: f64) -> f64 {
        let a = 0.5 * self.stroke_mm * 1e-3;
        let l = self.rod_mm * 1e-3;
        let th = theta_deg.to_radians();
        let x = l + a - a * th.cos() - (l * l - (a * th.sin()).powi(2)).sqrt();
        self.clearance_volume() + std::f64::consts::PI / 4.0 * (self.bore_mm * 1e-3).powi(2) * x
    }

    /// Crank-angle offset of each cylinder's firing TDC from cylinder 1, indexed by
    /// 0-based cylinder number.
    pub fn firing_offsets_deg(&self) -> Vec<f64> {
        let n = self.cylinders();
        let mut off = vec![0.0; n];
        for (slot, &cyl) in self.firing_order.iter().enumerate() {
            off[cyl - 1] = slot as f64 * 720.0 / n as f64;
        }
        off
    }

    pub fn validate(&self) -> Result<()> {
        let n = self.cylinders();
        let mut seen = vec![false; n];
        for &c in &self.firing_order {
            if c == 0 || c > n || seen[c - 1] {
                return Err(Error::invalid(
                    "engine.firing_order must be a permutation of 1..=n",
                ));
            }
            seen[c - 1] = true;
        }
        if !(self.bore_mm > 0.0 && self.stroke_mm > 0.0 && self.compression_ratio > 1.0) {
            return Err(Error::invalid(
                "engine bore, stroke must be > 0 and compression ratio > 1",
            ));
        }
        if self.rod_mm <= 0.5 * self.stroke_mm {
            return Err(Error::invalid("engine.rod_mm must exceed half the stroke"));
        }
        Ok(())
    }
}

/// Exhaust valve train of one cylinder.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ExhaustValves {
    pub valves_per_cylinder: usize,
    /// Valve head diameter, mm (curtain area π d L).
    pub valve_diameter_mm: f64,
    /// Port throat (inner seat) diameter, mm; caps the flow area at π d²/4 per valve.
    pub throat_diameter_mm: f64,
    /// Discharge coefficient referred to the geometric flow area.
    pub discharge_coefficient: f64,
    /// Exhaust valve opening, crank degrees after firing TDC.
    pub evo_deg: f64,
    /// Exhaust valve closing, crank degrees after firing TDC (e.g. 370 = 10° after gas-exchange TDC).
    pub evc_deg: f64,
    /// Peak lift, mm, of the raised-cosine profile `L = L_max (1 − cos 2πφ)/2`,
    /// `φ = (θ − EVO)/(EVC − EVO)`. Ignored when `lift_table` is given.
    pub max_lift_mm: f64,
    /// Measured lift curve: (crank degrees after firing TDC, lift mm), linearly interpolated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lift_table: Option<Vec<[f64; 2]>>,
}

impl ExhaustValves {
    /// Valve lift at cylinder crank angle `theta` (0–720° after firing TDC), m.
    pub fn lift(&self, theta: f64) -> f64 {
        if let Some(table) = &self.lift_table {
            return interp_table(table, theta) * 1e-3;
        }
        if theta <= self.evo_deg || theta >= self.evc_deg {
            return 0.0;
        }
        let phi = (theta - self.evo_deg) / (self.evc_deg - self.evo_deg);
        0.5 * self.max_lift_mm * 1e-3 * (1.0 - (2.0 * std::f64::consts::PI * phi).cos())
    }

    /// Effective flow area `C_d · n · min(π d L, π d_t²/4)`, m².
    pub fn effective_area(&self, theta: f64) -> f64 {
        let lift = self.lift(theta);
        if lift <= 0.0 {
            return 0.0;
        }
        let curtain = std::f64::consts::PI * self.valve_diameter_mm * 1e-3 * lift;
        let throat = std::f64::consts::PI / 4.0 * (self.throat_diameter_mm * 1e-3).powi(2);
        self.discharge_coefficient * self.valves_per_cylinder as f64 * curtain.min(throat)
    }

    /// True while the valve is off its seat.
    pub fn is_open(&self, theta: f64) -> bool {
        theta > self.evo_deg && theta < self.evc_deg
    }

    pub fn validate(&self) -> Result<()> {
        if !(self.evo_deg > 0.0 && self.evo_deg < self.evc_deg && self.evc_deg < 720.0) {
            return Err(Error::invalid(
                "exhaust valve timing must satisfy 0 < EVO < EVC < 720",
            ));
        }
        if !(self.valve_diameter_mm > 0.0
            && self.throat_diameter_mm > 0.0
            && self.discharge_coefficient > 0.0)
        {
            return Err(Error::invalid(
                "exhaust valve diameters and discharge coefficient must be > 0",
            ));
        }
        if let Some(t) = &self.lift_table
            && (t.len() < 2 || t.windows(2).any(|w| w[1][0] <= w[0][0]))
        {
            return Err(Error::invalid(
                "lift_table needs ≥ 2 points with increasing crank angle",
            ));
        }
        Ok(())
    }
}

fn interp_table(table: &[[f64; 2]], x: f64) -> f64 {
    if x <= table[0][0] || x >= table[table.len() - 1][0] {
        return 0.0;
    }
    let k = table.partition_point(|p| p[0] <= x) - 1;
    let (a, b) = (table[k], table[k + 1]);
    a[1] + (b[1] - a[1]) * (x - a[0]) / (b[0] - a[0])
}

/// Engine load used to estimate the in-cylinder state at EVO.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Load {
    /// Intake manifold absolute pressure, kPa.
    pub map_kpa: f64,
    /// Intake manifold temperature, K.
    pub intake_temperature_k: f64,
    /// Volumetric efficiency referred to manifold density.
    pub volumetric_efficiency: f64,
    /// Fuel lower heating value, MJ/kg.
    pub lhv_mj_per_kg: f64,
    /// Fraction of the fuel's heating value that is in the cylinder gas at TDC after
    /// combustion inefficiency and wall heat loss.
    pub heat_retained: f64,
    /// Polytropic exponent of compression.
    pub n_compression: f64,
    /// Polytropic exponent of expansion from TDC to EVO.
    pub n_expansion: f64,
}

/// In-cylinder state at exhaust valve opening.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EvoState {
    /// Pa.
    pub pressure_pa: f64,
    /// K.
    pub temperature_k: f64,
}

/// Ideal Otto-cycle estimate of the EVO state:
/// ```text
/// m_air = η_v p_map V_d / (R_air T_im),  m_f = m_air / (λ AFR_st),  m = m_air + m_f
/// 1 (BDC):  V₁ = V_d + V_c,  T₁ = T_im,  p₁ = m R T₁ / V₁
/// 1→2 polytropic compression to TDC:  T₂ = T₁ CRⁿᶜ⁻¹, p₂ = p₁ CRⁿᶜ
/// 2→3 constant-volume heat addition:  e(T₃) = e(T₂) + χ m_f LHV / m,   p₃ = p₂ T₃/T₂
/// 3→EVO polytropic expansion:  T = T₃ (V_c/V_EVO)ⁿᵉ⁻¹,  p = p₃ (V_c/V_EVO)ⁿᵉ
/// ```
/// Every state holds the same mass `m`, so the cylinder model starts each exhaust event with
/// `p_EVO V_EVO/(R T_EVO) = m`. Burned-gas `R` and `e(T)` from `gas`; `R_air = 287.05` J/(kg K).
/// Residual gas is not tracked. An estimate, labelled as such in every result that uses it.
pub fn estimate_evo_state(
    geometry: &EngineGeometry,
    valves: &ExhaustValves,
    gas: &Gas,
    load: &Load,
    fuel_h_to_c: f64,
    lambda: f64,
) -> EvoState {
    const R_AIR: f64 = 287.05;
    let m_air = load.volumetric_efficiency * load.map_kpa * 1e3 * geometry.displacement()
        / (R_AIR * load.intake_temperature_k);
    let m_fuel = m_air / (lambda * Composition::stoichiometric_afr(fuel_h_to_c));
    let m = m_air + m_fuel;
    let cr = geometry.compression_ratio;
    let p1 = m * gas.r() * load.intake_temperature_k / geometry.volume(180.0);
    let t2 = load.intake_temperature_k * cr.powf(load.n_compression - 1.0);
    let p2 = p1 * cr.powf(load.n_compression);
    let e3 = gas.e(t2) + load.heat_retained * m_fuel * load.lhv_mj_per_kg * 1e6 / m;
    let t3 = gas.t_from_e(e3);
    let p3 = p2 * t3 / t2;
    let ratio = geometry.clearance_volume() / geometry.volume(valves.evo_deg);
    EvoState {
        pressure_pa: p3 * ratio.powf(load.n_expansion),
        temperature_k: t3 * ratio.powf(load.n_expansion - 1.0),
    }
}

/// Mass flux per unit effective area of an isentropic nozzle from stagnation `(p0, t0)` to
/// static back pressure `p`, kg/(m² s):
/// `ψ = p0/√(R T0) · √(2γ/(γ−1) [π^{2/γ} − π^{(γ+1)/γ}])`, `π = max(p/p0, π*)`,
/// `π* = (2/(γ+1))^{γ/(γ−1)}` (choked).
#[inline]
pub fn nozzle_mass_flux(p0: f64, t0: f64, p: f64, gamma: f64, r: f64) -> f64 {
    let crit = (2.0 / (gamma + 1.0)).powf(gamma / (gamma - 1.0));
    let pr = (p / p0).clamp(crit, 1.0);
    p0 / (r * t0).sqrt()
        * (2.0 * gamma / (gamma - 1.0) * (pr.powf(2.0 / gamma) - pr.powf((gamma + 1.0) / gamma)))
            .sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m274() -> (EngineGeometry, ExhaustValves) {
        (
            EngineGeometry {
                bore_mm: 83.0,
                stroke_mm: 92.0,
                rod_mm: 145.0,
                compression_ratio: 9.8,
                firing_order: vec![1, 3, 4, 2],
            },
            ExhaustValves {
                valves_per_cylinder: 2,
                valve_diameter_mm: 26.0,
                throat_diameter_mm: 22.6,
                discharge_coefficient: 0.65,
                evo_deg: 130.0,
                evc_deg: 370.0,
                max_lift_mm: 8.0,
                lift_table: None,
            },
        )
    }

    #[test]
    fn slider_crank_volumes() {
        let (g, _) = m274();
        assert!((g.displacement() * 4.0 - 1.991e-3).abs() < 2e-6);
        assert!((g.volume(0.0) - g.clearance_volume()).abs() < 1e-12);
        assert!((g.volume(180.0) - g.clearance_volume() - g.displacement()).abs() < 1e-12);
        assert_eq!(g.firing_offsets_deg(), vec![0.0, 540.0, 180.0, 360.0]);
    }

    #[test]
    fn evo_state_is_plausible_for_boosted_si_engine() {
        let (g, v) = m274();
        let gas = Gas::exhaust(1.87, 1.0).unwrap();
        let load = Load {
            map_kpa: 100.0,
            intake_temperature_k: 313.0,
            volumetric_efficiency: 0.9,
            lhv_mj_per_kg: 43.0,
            heat_retained: 0.75,
            n_compression: 1.32,
            n_expansion: 1.30,
        };
        let s = estimate_evo_state(&g, &v, &gas, &load, 1.87, 1.0);
        // Heywood (1988) ch. 6: blowdown starts at 3–6 bar, 1100–1500 K at full load.
        assert!(s.pressure_pa > 3e5 && s.pressure_pa < 7e5, "{s:?}");
        assert!(
            s.temperature_k > 1100.0 && s.temperature_k < 1600.0,
            "{s:?}"
        );
        // Mass at EVO equals the trapped charge.
        let m_evo = s.pressure_pa * g.volume(v.evo_deg) / (gas.r() * s.temperature_k);
        let m_air = 0.9 * 100e3 * g.displacement() / (287.05 * 313.0);
        let m = m_air * (1.0 + 1.0 / Composition::stoichiometric_afr(1.87));
        assert!((m_evo / m - 1.0).abs() < 1e-12, "{m_evo} vs {m}");
    }

    #[test]
    fn nozzle_chokes() {
        let g = 1.4;
        let a = nozzle_mass_flux(2e5, 300.0, 0.5e5, g, 287.0);
        let b = nozzle_mass_flux(2e5, 300.0, 1.0e5, g, 287.0);
        assert!(
            (a - b).abs() < 1e-9 * a,
            "choked flux must not depend on back pressure"
        );
        assert_eq!(nozzle_mass_flux(2e5, 300.0, 2e5, g, 287.0), 0.0);
    }
}
