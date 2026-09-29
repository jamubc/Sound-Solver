//! HLLC flux, MUSCL slope limiters and exact Riemann wave curves.

use serde::{Deserialize, Serialize};

use crate::gas::Gas;

/// TVD slope limiter for MUSCL reconstruction of primitive variables.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Limiter {
    /// `minmod(a, b)`: most dissipative TVD limiter.
    Minmod,
    /// van Leer (1974): `2ab/(a + b)` when `ab > 0`.
    #[default]
    VanLeer,
}

impl Limiter {
    #[inline]
    pub fn slope(self, a: f64, b: f64) -> f64 {
        if a * b <= 0.0 {
            return 0.0;
        }
        match self {
            Limiter::Minmod => {
                if a.abs() < b.abs() {
                    a
                } else {
                    b
                }
            }
            Limiter::VanLeer => 2.0 * a * b / (a + b),
        }
    }
}

/// Cell or face state in the variables the flux needs.
#[derive(Clone, Copy, Debug, Default)]
pub struct State {
    pub rho: f64,
    pub u: f64,
    pub p: f64,
    /// Total energy per unit volume `ρ(e + u²/2)`.
    pub en: f64,
    /// Sound speed.
    pub c: f64,
}

impl State {
    /// Completes a primitive `(ρ, u, p)` with the thermally perfect equation of state.
    #[inline]
    pub fn from_primitive(gas: &Gas, rho: f64, u: f64, p: f64) -> Self {
        let t = p / (rho * gas.r());
        Self {
            rho,
            u,
            p,
            en: rho * (gas.e(t) + 0.5 * u * u),
            c: gas.sound_speed(t),
        }
    }
}

/// HLLC approximate Riemann solver (Toro 2009, §10.4, eqs. 10.37–10.40), Davis wave-speed
/// estimates `S_L = min(u_L − c_L, u_R − c_R)`, `S_R = max(u_L + c_L, u_R + c_R)`.
/// Returns the interface flux per unit area of (mass, momentum, energy).
#[inline]
pub fn hllc(l: &State, r: &State) -> [f64; 3] {
    let sl = (l.u - l.c).min(r.u - r.c);
    let sr = (l.u + l.c).max(r.u + r.c);
    let fl = [l.rho * l.u, l.rho * l.u * l.u + l.p, l.u * (l.en + l.p)];
    if sl >= 0.0 {
        return fl;
    }
    let fr = [r.rho * r.u, r.rho * r.u * r.u + r.p, r.u * (r.en + r.p)];
    if sr <= 0.0 {
        return fr;
    }
    let s_star = (r.p - l.p + l.rho * l.u * (sl - l.u) - r.rho * r.u * (sr - r.u))
        / (l.rho * (sl - l.u) - r.rho * (sr - r.u));
    let star = |k: &State, sk: f64, fk: [f64; 3]| {
        let m = k.rho * (sk - k.u) / (sk - s_star);
        let u_star = [
            m,
            m * s_star,
            m * (k.en / k.rho + (s_star - k.u) * (s_star + k.p / (k.rho * (sk - k.u)))),
        ];
        [
            fk[0] + sk * (u_star[0] - k.rho),
            fk[1] + sk * (u_star[1] - k.rho * k.u),
            fk[2] + sk * (u_star[2] - k.en),
        ]
    };
    if s_star >= 0.0 {
        star(l, sl, fl)
    } else {
        star(r, sr, fr)
    }
}

/// Toro (2009) eqs. 4.6–4.7: velocity change `f(p)` across the wave that connects the
/// state `(ρₖ, pₖ, cₖ)` to pressure `p` (shock for `p > pₖ`, rarefaction otherwise), and `df/dp`.
/// A node joined to a duct end satisfies `v_face = v − f(p_face)`, with `v` the interior
/// velocity toward the node.
#[inline]
pub fn wave_f(p: f64, rho_k: f64, p_k: f64, c_k: f64, g: f64) -> (f64, f64) {
    if p > p_k {
        let a = 2.0 / ((g + 1.0) * rho_k);
        let b = (g - 1.0) / (g + 1.0) * p_k;
        let q = (a / (p + b)).sqrt();
        ((p - p_k) * q, q * (1.0 - 0.5 * (p - p_k) / (b + p)))
    } else {
        let ratio = p / p_k;
        let f = 2.0 * c_k / (g - 1.0) * (ratio.powf((g - 1.0) / (2.0 * g)) - 1.0);
        let df = ratio.powf(-(g + 1.0) / (2.0 * g)) / (rho_k * c_k);
        (f, df)
    }
}

/// Density behind the wave of [`wave_f`] (Toro eqs. 4.50, 4.53, 4.57, 4.60).
#[inline]
pub fn wave_rho(p: f64, rho_k: f64, p_k: f64, g: f64) -> f64 {
    let ratio = p / p_k;
    if p > p_k {
        let g6 = (g - 1.0) / (g + 1.0);
        rho_k * (ratio + g6) / (g6 * ratio + 1.0)
    } else {
        rho_k * ratio.powf(1.0 / g)
    }
}

/// Pressure at which the wave curve of state `k` reaches velocity change `target`
/// (`f(p) = target`), by safeguarded Newton iteration. `f` is increasing in `p`, bounded below
/// by `−2c/(γ−1)`; a target below that bound means cavitation and returns `None`.
pub fn wave_pressure(target: f64, rho_k: f64, p_k: f64, c_k: f64, g: f64) -> Option<f64> {
    if target <= -2.0 * c_k / (g - 1.0) {
        return None;
    }
    let mut p = (p_k + rho_k * c_k * target).max(1e-3 * p_k);
    for _ in 0..60 {
        let (f, df) = wave_f(p, rho_k, p_k, c_k, g);
        let step = (f - target) / df;
        let next = (p - step).max(0.1 * p);
        if (next - p).abs() <= 1e-12 * p {
            return Some(next);
        }
        p = next;
    }
    Some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hllc_is_consistent_for_equal_states() {
        let gas = Gas::perfect(1.4, 287.0);
        let s = State::from_primitive(&gas, 1.2, 35.0, 1.1e5);
        let f = hllc(&s, &s);
        let exact = [s.rho * s.u, s.rho * s.u * s.u + s.p, s.u * (s.en + s.p)];
        for k in 0..3 {
            assert!((f[k] - exact[k]).abs() < 1e-9 * exact[k].abs().max(1.0));
        }
    }

    #[test]
    fn wave_pressure_inverts_wave_f() {
        let (rho, p, c, g) = (1.0, 1e5, 374.0, 1.4);
        for target in [-300.0, -20.0, 0.0, 15.0, 400.0] {
            let ps = wave_pressure(target, rho, p, c, g).unwrap();
            assert!((wave_f(ps, rho, p, c, g).0 - target).abs() < 1e-8);
        }
    }
}
