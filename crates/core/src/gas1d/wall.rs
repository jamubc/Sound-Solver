//! Wall friction and heat-transfer correlations for fully developed pipe flow, applied
//! quasi-steadily to the instantaneous cell state (unsteady-friction and pulsation
//! enhancement of heat transfer are not modelled).

/// Reynolds numbers bounding the laminar–turbulent blend.
const RE_LAMINAR: f64 = 2300.0;
const RE_TURBULENT: f64 = 4000.0;
const RE_GNIELINSKI: f64 = 3000.0;

/// Darcy friction factor from the Haaland (1983) explicit approximation of Colebrook:
/// `1/√f_D = −1.8 log₁₀[(ε/D / 3.7)^1.11 + 6.9/Re]`.
#[inline]
pub fn haaland_darcy(re: f64, rel_roughness: f64) -> f64 {
    let x = -1.8 * ((rel_roughness / 3.7).powf(1.11) + 6.9 / re).log10();
    1.0 / (x * x)
}

/// Fanning friction factor `f = τ_w / (½ρu²)`: laminar `16/Re` below Re = 2300, Haaland/4
/// above Re = 4000, linear in Re between.
#[inline]
pub fn fanning(re: f64, rel_roughness: f64) -> f64 {
    if re <= RE_LAMINAR {
        16.0 / re
    } else if re >= RE_TURBULENT {
        0.25 * haaland_darcy(re, rel_roughness)
    } else {
        let w = (re - RE_LAMINAR) / (RE_TURBULENT - RE_LAMINAR);
        (1.0 - w) * 16.0 / RE_LAMINAR + w * 0.25 * haaland_darcy(RE_TURBULENT, rel_roughness)
    }
}

/// Wall shear stress with the sign of `u`, Pa. Laminar branch written as Hagen–Poiseuille
/// `τ = 8μu/D`, finite as `u → 0`.
#[inline]
pub fn shear_stress(rho: f64, u: f64, mu: f64, d: f64, roughness: f64) -> f64 {
    let re = rho * u.abs() * d / mu;
    if re <= RE_LAMINAR {
        8.0 * mu * u / d
    } else {
        fanning(re, roughness / d) * 0.5 * rho * u * u.abs()
    }
}

/// Nusselt number: 3.66 (laminar, uniform wall temperature) below Re = 2300; Gnielinski (1976)
/// `Nu = (f/8)(Re − 1000)Pr / (1 + 12.7 √(f/8) (Pr^{2/3} − 1))` with Petukhov
/// `f = (0.790 ln Re − 1.64)⁻²` from Re = 3000; linear in Re between.
#[inline]
pub fn nusselt(re: f64, pr: f64) -> f64 {
    let gnielinski = |re: f64| {
        let f = (0.790 * re.ln() - 1.64).powi(-2);
        (f / 8.0) * (re - 1000.0) * pr
            / (1.0 + 12.7 * (f / 8.0).sqrt() * (pr.powf(2.0 / 3.0) - 1.0))
    };
    if re <= RE_LAMINAR {
        3.66
    } else if re >= RE_GNIELINSKI {
        gnielinski(re)
    } else {
        let w = (re - RE_LAMINAR) / (RE_GNIELINSKI - RE_LAMINAR);
        (1.0 - w) * 3.66 + w * gnielinski(RE_GNIELINSKI)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn friction_reference_values() {
        // Moody chart: smooth pipe Re = 1e5 → f_D ≈ 0.0180; ε/D = 1e-3, Re = 1e6 → f_D ≈ 0.0199.
        assert!((haaland_darcy(1e5, 0.0) - 0.0180).abs() < 4e-4);
        assert!((haaland_darcy(1e6, 1e-3) - 0.0199).abs() < 4e-4);
        assert!((fanning(1000.0, 0.0) - 0.016).abs() < 1e-12);
    }

    #[test]
    fn gnielinski_close_to_dittus_boelter() {
        // Dittus–Boelter Nu = 0.023 Re^0.8 Pr^0.4 agrees with Gnielinski to ≈10 %.
        for re in [1e4, 1e5] {
            let db = 0.023 * f64::powf(re, 0.8) * f64::powf(0.7, 0.4);
            assert!((nusselt(re, 0.7) / db - 1.0).abs() < 0.12, "Re {re}");
        }
    }
}
