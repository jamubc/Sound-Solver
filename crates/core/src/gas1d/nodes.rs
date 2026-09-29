//! Nodes: the boundary conditions and junctions that join duct ends.
//!
//! Each node returns the state on the boundary face of each duct end it owns, given the
//! interior cell next to it. On the duct side the face state lies on the exact Riemann wave
//! curve of the interior state, `v_face = v − f(p_face)` (Toro 2009, eqs. 4.6–4.7; `v` is the
//! velocity toward the node), except at the radiating outlet and at area changes, which use
//! linear characteristics `p ± ρc v` because wave amplitudes there are a few percent of `p`.

use std::slice;

use super::scheme::{wave_f, wave_pressure, wave_rho};
use super::{FaceState, NodeState, Port, PortState};
use crate::engine::{EngineGeometry, EvoState, ExhaustValves, nozzle_mass_flux};
use crate::error::{Error, Result};
use crate::gas::Gas;
use crate::math::brent;

/// A node joining one or more duct ends.
#[derive(Clone, Debug)]
pub enum Node {
    /// Rigid closed end: `v_face = 0`.
    Wall(Port),
    /// Ideal open end to a reservoir at static pressure `p` (outflow) or stagnation state
    /// `(p, t)` (inflow): reflection coefficient −1 for small waves.
    Pressure { port: Port, p: f64, t: f64 },
    /// Unflanged open end radiating per Levine–Schwinger.
    Radiation(RadiationBc),
    /// Two or more duct ends meeting.
    Junction(JunctionBc),
    /// Engine cylinders → manifold volumes → turbine → this duct end.
    Source(Box<SourceBc>),
    /// Non-reflecting end with an optional incident wave.
    Characteristic(CharacteristicBc),
    /// Prescribed mass flow into the duct.
    MassFlow(MassFlowBc),
}

impl Node {
    pub fn ports(&self) -> &[Port] {
        match self {
            Node::Wall(p) => slice::from_ref(p),
            Node::Pressure { port, .. } => slice::from_ref(port),
            Node::Radiation(bc) => slice::from_ref(&bc.port),
            Node::Junction(j) => &j.ports,
            Node::Source(s) => slice::from_ref(&s.port),
            Node::Characteristic(bc) => slice::from_ref(&bc.port),
            Node::MassFlow(bc) => slice::from_ref(&bc.port),
        }
    }
}

/// Closed end. The face pressure solves `f(p) = v` (the mirror-state Riemann problem).
pub(crate) fn wall(ps: &PortState) -> Result<FaceState> {
    let p = wave_pressure(ps.v, ps.rho, ps.p, ps.c, ps.gamma).ok_or_else(|| {
        Error::solver("cavitation at a closed end: gas leaves the wall faster than 2c/(γ−1)")
    })?;
    Ok(FaceState {
        mass_flux: 0.0,
        p,
        v: 0.0,
        h0: 0.0,
        rho: wave_rho(p, ps.rho, ps.p, ps.gamma),
    })
}

/// Ideal open end to a reservoir. Outflow leaves at the reservoir pressure; inflow accelerates
/// isentropically from the reservoir stagnation state `(p0, t0)`.
pub(crate) fn reservoir(gas: &Gas, ps: &PortState, p0: f64, t0: f64) -> Result<FaceState> {
    let v_out = ps.v - wave_f(p0, ps.rho, ps.p, ps.c, ps.gamma).0;
    if v_out >= 0.0 {
        let rho = wave_rho(p0, ps.rho, ps.p, ps.gamma);
        let t = p0 / (rho * gas.r());
        return Ok(FaceState {
            mass_flux: rho * v_out,
            p: p0,
            v: v_out,
            h0: gas.h(t) + 0.5 * v_out * v_out,
            rho,
        });
    }
    let g0 = gas.gamma(t0);
    let h00 = gas.h(t0);
    let inflow_velocity = |pb: f64| -> (f64, f64) {
        let tb = t0 * (pb / p0).powf((g0 - 1.0) / g0);
        (-(2.0 * (h00 - gas.h(tb))).max(0.0).sqrt(), tb)
    };
    let resid =
        |pb: f64| (ps.v - wave_f(pb, ps.rho, ps.p, ps.c, ps.gamma).0) - inflow_velocity(pb).0;
    let pb = brent(resid, 0.02 * p0.min(ps.p), p0, 1e-10 * p0, 200)
        .ok_or_else(|| Error::solver("reservoir inflow: no face pressure found"))?;
    let (v, tb) = inflow_velocity(pb);
    let rho = pb / (gas.r() * tb);
    Ok(FaceState {
        mass_flux: rho * v,
        p: pb,
        v,
        h0: h00,
        rho,
    })
}

/// Radiating open end (tailpipe outlet), unflanged, Levine–Schwinger.
#[derive(Clone, Debug)]
pub struct RadiationBc {
    pub port: Port,
    /// Ambient pressure, Pa.
    pub p_amb: f64,
    /// Ambient temperature for inflow, K.
    pub t_amb: f64,
    /// Pipe inner radius at the outlet, m.
    pub radius: f64,
}

/// Outlet: the Levine–Schwinger radiation impedance in series with the separation loss of
/// inflow at a sharp-edged pipe end.
///
/// Linear characteristics with `Z = ρc`: `W⁺ = (p − p_amb) + Z v` arrives from the pipe. Just
/// outside the exit plane the radiation impedance sees `p_r = p + Δ(v)`, where
/// `Δ = ρ_amb v²` during inflow (`v < 0`: separated sink flow into a thin-walled pipe loses
/// its dynamic head plus a Borda loss of 1 × ½ρv²) and 0 during outflow (the jet leaves at the
/// radiation pressure). The radiation side obeys `W⁻_r = R ∗ W⁺_r` with `W⁺_r = W⁺ + Δ(v)`
/// (`LsFilter`; strictly proper, so `W⁻_r` is known from past input), and the face velocity
/// solves `2Z v − Δ(v) = W⁺ − W⁻_r`; then `p = p_amb + W⁺ − Z v`. `Δ` only ever removes
/// energy. For outflow or small amplitude this is the linear Levine–Schwinger boundary; at high
/// amplitude `Δ` is the vortex-shedding loss of open ends (Ingard & Ising 1967; Disselhorst &
/// van Wijngaarden 1980). `R(0) = −1` makes the mean exit pressure ambient. Mean-flow effects on
/// radiation (Munt 1990) and the temperature jump between jet and ambient air are neglected.
/// Inflowing air takes the exhaust gas properties at ambient temperature.
///
/// Returns the face state and `W⁺_r`, the input of the radiation filter.
pub(crate) fn radiation(
    gas: &Gas,
    ps: &PortState,
    bc: &RadiationBc,
    w_plus: f64,
    w_minus_r: f64,
) -> (FaceState, f64) {
    let z = ps.rho * ps.c;
    let rho_amb = bc.p_amb / (gas.r() * bc.t_amb);
    let b = w_plus - w_minus_r;
    let v = if b >= 0.0 {
        b / (2.0 * z)
    } else {
        // ρ v² − 2Z v + b = 0, root continuous with b/(2Z), in cancellation-free form.
        b / (z + (z * z - rho_amb * b).sqrt())
    };
    let loss = if v < 0.0 { rho_amb * v * v } else { 0.0 };
    let p = bc.p_amb + w_plus - z * v;
    let face = if v >= 0.0 {
        let rho = ps.rho * (p / ps.p).powf(1.0 / ps.gamma);
        let t = p / (rho * gas.r());
        FaceState {
            mass_flux: rho * v,
            p,
            v,
            h0: gas.h(t) + 0.5 * v * v,
            rho,
        }
    } else {
        let h0 = gas.h(bc.t_amb);
        let rho = p / (gas.r() * gas.t_from_h(h0 - 0.5 * v * v));
        FaceState {
            mass_flux: rho * v,
            p,
            v,
            h0,
            rho,
        }
    };
    (face, w_plus + loss)
}

/// Junction model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum JunctionKind {
    /// Two collinear ducts of different area meeting at a step.
    AreaChange,
    /// Two ducts joined through an orifice of effective area `C_d A_o` (m²): a throttle or a
    /// butterfly valve.
    Orifice { effective_area: f64 },
    /// Any number of ducts sharing one static pressure (Benson's constant-pressure junction).
    ConstantPressure,
}

/// Duct ends meeting at a point.
#[derive(Clone, Debug)]
pub struct JunctionBc {
    pub ports: Vec<Port>,
    pub kind: JunctionKind,
}

impl JunctionBc {
    pub(crate) fn eval(
        &self,
        gas: &Gas,
        ports: &[PortState],
        faces: &mut [FaceState],
    ) -> Result<()> {
        match self.kind {
            JunctionKind::AreaChange => {
                (faces[0], faces[1]) = step_junction(gas, &ports[0], &ports[1], None);
            }
            JunctionKind::Orifice { effective_area } => {
                (faces[0], faces[1]) =
                    step_junction(gas, &ports[0], &ports[1], Some(effective_area));
            }
            JunctionKind::ConstantPressure => constant_pressure(gas, ports, faces),
        }
        Ok(())
    }
}

/// Loss coefficient referred to the upstream velocity `v_u` for flow from area `a_u` to
/// `a_d`, optionally through an orifice of effective area `a_e`.
///
/// Expansion: Borda–Carnot `(1 − A_u/A_d)²`, identical to the momentum balance with base
/// pressure equal to the jet pressure. Contraction: `0.5 (1 − A_d/A_u)^0.75` on `v_d`
/// (Idelchik 1986, sharp-edged, Re > 10⁴). Orifice: the jet leaves the vena contracta at
/// `v_u A_u/A_e` and mixes out in the downstream pipe, `(A_u/A_e − A_u/A_d)²`.
pub fn step_loss(a_u: f64, a_d: f64, a_e: Option<f64>) -> f64 {
    let alpha = a_u / a_d;
    match a_e {
        Some(a_e) if a_e < a_u.min(a_d) => (a_u / a_e - alpha).powi(2),
        _ if a_d >= a_u => (1.0 - alpha).powi(2),
        _ => 0.5 * (1.0 - 1.0 / alpha).powf(0.75) * alpha * alpha,
    }
}

/// Sudden area change or orifice between two ducts. With `v` toward the junction and
/// `W = p + ρc v` the wave arriving from each side, the upstream velocity `v_u` solves
/// ```text
/// (W_u − Z_u v_u) + ½ρ v_u² − (W_d − Z_d v_d) − ½ρ v_d² = K ½ρ v_u²,   v_d = −(A_u/A_d) v_u
/// ```
/// (stagnation-pressure loss across the step, [`step_loss`]). Mass and stagnation enthalpy
/// pass through unchanged, so both are conserved exactly.
fn step_junction(
    gas: &Gas,
    a: &PortState,
    b: &PortState,
    orifice: Option<f64>,
) -> (FaceState, FaceState) {
    let wa = a.p + a.rho * a.c * a.v;
    let wb = b.p + b.rho * b.c * b.v;
    let flip = wb > wa;
    let (up, dn) = if flip { (b, a) } else { (a, b) };
    let (wu, wd) = if flip { (wb, wa) } else { (wa, wb) };
    let (zu, zd) = (up.rho * up.c, dn.rho * dn.c);
    let alpha = up.area / dn.area;
    let k_eff = step_loss(up.area, dn.area, orifice);
    let c0 = wu - wd;
    let c1 = -(zu + alpha * zd);
    let c2 = 0.5 * up.rho * (1.0 - alpha * alpha - k_eff);
    let disc = c1 * c1 - 4.0 * c2 * c0;
    // Root continuous with the linear solution c0/(Z_u + αZ_d); past the incompressible
    // flow maximum (disc < 0) the maximum is taken.
    let vu = if disc >= 0.0 {
        2.0 * c0 / (-c1 + disc.sqrt())
    } else {
        -c1 / (2.0 * c2)
    };
    let pu = wu - zu * vu;
    let rho_u = up.rho * (pu / up.p).powf(1.0 / up.gamma);
    let mdot = rho_u * up.area * vu;
    let h0 = gas.h(pu / (rho_u * gas.r())) + 0.5 * vu * vu;
    let vd_est = -alpha * vu;
    let pd = wd - zd * vd_est;
    let rho_d = pd / (gas.r() * gas.t_from_h(h0 - 0.5 * vd_est * vd_est));
    let fu = FaceState {
        mass_flux: mdot / up.area,
        p: pu,
        v: vu,
        h0,
        rho: rho_u,
    };
    let fd = FaceState {
        mass_flux: -mdot / dn.area,
        p: pd,
        v: -mdot / (rho_d * dn.area),
        h0,
        rho: rho_d,
    };
    if flip { (fd, fu) } else { (fu, fd) }
}

/// Constant-pressure junction (Benson 1982). With linear characteristics
/// `vₖ = (Wₖ − p_J)/Zₖ` and `Σ ρₖ Aₖ vₖ = 0`, the common pressure is
/// `p_J = Σ(Aₖ Wₖ/cₖ) / Σ(Aₖ/cₖ)`. Gas arriving from the ducts (`vₖ > 0`) mixes; the mixed
/// stagnation enthalpy leaves through the other ports, and the arriving mass is shared between
/// them in proportion to their volume flow, so mass and energy are conserved exactly.
fn constant_pressure(gas: &Gas, ports: &[PortState], faces: &mut [FaceState]) {
    let (num, den) = ports.iter().fold((0.0, 0.0), |(n, d), s| {
        let w = s.p + s.rho * s.c * s.v;
        (n + s.area * w / s.c, d + s.area / s.c)
    });
    let pj = num / den;
    let (mut m_in, mut e_in, mut vol_out) = (0.0, 0.0, 0.0);
    let mut v = [0.0; super::MAX_PORTS];
    for (k, s) in ports.iter().enumerate() {
        v[k] = (s.p + s.rho * s.c * s.v - pj) / (s.rho * s.c);
        if v[k] > 0.0 {
            let rho = s.rho * (pj / s.p).powf(1.0 / s.gamma);
            let h0 = gas.h(pj / (rho * gas.r())) + 0.5 * v[k] * v[k];
            let mdot = rho * s.area * v[k];
            m_in += mdot;
            e_in += mdot * h0;
            faces[k] = FaceState {
                mass_flux: rho * v[k],
                p: pj,
                v: v[k],
                h0,
                rho,
            };
        } else {
            vol_out -= s.area * v[k];
        }
    }
    let h0_mix = if m_in > 0.0 { e_in / m_in } else { 0.0 };
    for (k, s) in ports.iter().enumerate() {
        if v[k] > 0.0 {
            continue;
        }
        if m_in == 0.0 || vol_out == 0.0 {
            faces[k] = FaceState {
                mass_flux: 0.0,
                p: pj,
                v: 0.0,
                h0: 0.0,
                rho: s.rho,
            };
            continue;
        }
        let mdot = m_in * s.area * (-v[k]) / vol_out;
        let rho = pj / (gas.r() * gas.t_from_h(h0_mix - 0.5 * v[k] * v[k]));
        faces[k] = FaceState {
            mass_flux: -mdot / s.area,
            p: pj,
            v: -mdot / (rho * s.area),
            h0: h0_mix,
            rho,
        };
    }
}

/// A time signal driving a boundary.
#[derive(Clone, Debug, PartialEq)]
pub enum Signal {
    Zero,
    /// `a exp(−½((t − t₀)/σ)²)`.
    Gaussian {
        amplitude: f64,
        t0: f64,
        sigma: f64,
    },
    /// `mean + Σₖ aₖ cos(k ω t + φₖ)`, terms `(aₖ, φₖ)` for k = 1, 2, …
    Harmonics {
        mean: f64,
        omega: f64,
        terms: Vec<(f64, f64)>,
    },
}

impl Signal {
    pub fn at(&self, t: f64) -> f64 {
        match self {
            Signal::Zero => 0.0,
            Signal::Gaussian {
                amplitude,
                t0,
                sigma,
            } => amplitude * (-0.5 * ((t - t0) / sigma).powi(2)).exp(),
            Signal::Harmonics { mean, omega, terms } => {
                mean + terms
                    .iter()
                    .enumerate()
                    .map(|(k, (a, phi))| a * ((k + 1) as f64 * omega * t + phi).cos())
                    .sum::<f64>()
            }
        }
    }
}

/// Characteristic (non-reflecting) end: the wave leaving the duct passes out unreflected and
/// the wave entering it is `incoming(t)`, both as linear characteristics about `(p_ref, t_ref)`:
/// `p = p_ref + (W_out + W_in)/2`, `v = (W_out − W_in)/(2ρc)`. An anechoic termination with
/// `Signal::Zero`; an incident-wave source otherwise.
#[derive(Clone, Debug)]
pub struct CharacteristicBc {
    pub port: Port,
    pub p_ref: f64,
    pub t_ref: f64,
    pub incoming: Signal,
}

pub(crate) fn characteristic(
    gas: &Gas,
    ps: &PortState,
    bc: &CharacteristicBc,
    t: f64,
) -> FaceState {
    let z = ps.rho * ps.c;
    let w_out = (ps.p - bc.p_ref) + z * ps.v;
    let w_in = bc.incoming.at(t);
    let p = bc.p_ref + 0.5 * (w_out + w_in);
    let v = (w_out - w_in) / (2.0 * z);
    if v >= 0.0 {
        let rho = ps.rho * (p / ps.p).powf(1.0 / ps.gamma);
        let tf = p / (rho * gas.r());
        FaceState {
            mass_flux: rho * v,
            p,
            v,
            h0: gas.h(tf) + 0.5 * v * v,
            rho,
        }
    } else {
        let h0 = gas.h(bc.t_ref);
        let rho = p / (gas.r() * gas.t_from_h(h0 - 0.5 * v * v));
        FaceState {
            mass_flux: rho * v,
            p,
            v,
            h0,
            rho,
        }
    }
}

/// Prescribed mass flow into the duct (a rigid, infinite-impedance source) at stagnation
/// temperature `t0`. The face pressure solves `ρ_face v_face A = −ṁ(t)` with `v_face` on the
/// interior wave curve.
#[derive(Clone, Debug)]
pub struct MassFlowBc {
    pub port: Port,
    /// Mass flow into the duct, kg/s.
    pub mdot: Signal,
    pub t0: f64,
}

pub(crate) fn mass_flow(gas: &Gas, ps: &PortState, bc: &MassFlowBc, t: f64) -> Result<FaceState> {
    let mdot = bc.mdot.at(t);
    let h00 = gas.h(bc.t0);
    let face = |pb: f64| {
        let v = ps.v - wave_f(pb, ps.rho, ps.p, ps.c, ps.gamma).0;
        let rho = if mdot > 0.0 {
            pb / (gas.r() * gas.t_from_h(h00 - 0.5 * v * v))
        } else {
            wave_rho(pb, ps.rho, ps.p, ps.gamma)
        };
        (v, rho)
    };
    let pb = brent(
        |pb| {
            let (v, rho) = face(pb);
            rho * v * ps.area + mdot
        },
        0.05 * ps.p,
        20.0 * ps.p,
        1e-10 * ps.p,
        200,
    )
    .ok_or_else(|| Error::solver("mass-flow boundary: no face pressure found"))?;
    let (v, rho) = face(pb);
    let h0 = if mdot > 0.0 {
        h00
    } else {
        gas.h(pb / (rho * gas.r())) + 0.5 * v * v
    };
    Ok(FaceState {
        mass_flux: -mdot / ps.area,
        p: pb,
        v,
        h0,
        rho,
    })
}

/// Phase of a cylinder's exhaust event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CylPhase {
    /// Valve closed; the cylinder takes no part.
    Closed,
    /// Between EVO and EVC; mass is integrated.
    Open,
    /// The simulation started inside this cylinder's valve event; it joins at its next EVO.
    Skip,
}

/// Turbine scroll fed by a lumped manifold volume.
#[derive(Clone, Debug)]
pub struct Manifold {
    /// 0-based cylinders discharging into this volume.
    pub cylinders: Vec<usize>,
    /// Volume of ports, runners and scroll, m³.
    pub volume: f64,
    /// Effective nozzle area `C_d A` of the scroll (plus open wastegate), m².
    pub turbine_area: f64,
}

/// Engine exhaust source feeding the start of the downpipe.
///
/// Manifolds are adiabatic filling-and-emptying volumes:
/// `dm/dt = Σ ṁ_valve − ṁ_turbine`, `dU/dt = Σ ṁ_valve h_in − ṁ_turbine h_out`,
/// `p = m R T / V`, `U = m e(T)`.
/// Each scroll discharges through a quasi-steady isentropic nozzle to the downpipe face
/// pressure `p_b`. The turbine removes the fraction `extraction` of the isentropic enthalpy drop
/// as shaft work: `h0_out = h(T_M) − η_x [h(T_M) − h(T_M (p_b/p_M)^{(γ−1)/γ})]`. Reverse flow
/// through a scroll carries downpipe gas and does no work.
#[derive(Clone, Debug)]
pub struct SourceBc {
    pub port: Port,
    pub geometry: EngineGeometry,
    pub valves: ExhaustValves,
    pub evo: EvoState,
    /// Polytropic exponent of the cylinder gas after EVO.
    pub n_poly: f64,
    pub rpm: f64,
    /// Crank angle of cylinder 1 after its firing TDC at `t = 0`, degrees.
    pub theta0_deg: f64,
    pub manifolds: Vec<Manifold>,
    /// Fraction of the isentropic enthalpy drop the turbine extracts.
    pub extraction: f64,
    /// Initial manifold pressure, Pa, and temperature, K.
    pub p_init: f64,
    pub t_init: f64,
    offsets: Vec<f64>,
    cyl_manifold: Vec<usize>,
    m_evo: f64,
    rho_evo: f64,
}

impl SourceBc {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        port: Port,
        geometry: EngineGeometry,
        valves: ExhaustValves,
        evo: EvoState,
        n_poly: f64,
        rpm: f64,
        manifolds: Vec<Manifold>,
        extraction: f64,
        gas: &Gas,
        p_init: f64,
        t_init: f64,
    ) -> Result<Self> {
        geometry.validate()?;
        valves.validate()?;
        if manifolds.is_empty() || manifolds.len() > MAX_MANIFOLDS {
            return Err(Error::invalid(format!(
                "the engine needs 1 to {MAX_MANIFOLDS} manifolds (turbine scrolls)"
            )));
        }
        let n = geometry.cylinders();
        let mut cyl_manifold = vec![usize::MAX; n];
        for (j, m) in manifolds.iter().enumerate() {
            for &c in &m.cylinders {
                if c >= n || cyl_manifold[c] != usize::MAX {
                    return Err(Error::invalid(
                        "each cylinder must discharge into exactly one manifold",
                    ));
                }
                cyl_manifold[c] = j;
            }
        }
        if cyl_manifold.contains(&usize::MAX) {
            return Err(Error::invalid(
                "each cylinder must discharge into exactly one manifold",
            ));
        }
        let v_evo = geometry.volume(valves.evo_deg);
        let rho_evo = evo.pressure_pa / (gas.r() * evo.temperature_k);
        Ok(Self {
            port,
            offsets: geometry.firing_offsets_deg(),
            m_evo: rho_evo * v_evo,
            rho_evo,
            geometry,
            valves,
            evo,
            n_poly,
            rpm,
            theta0_deg: 0.0,
            manifolds,
            extraction,
            p_init,
            t_init,
            cyl_manifold,
        })
    }

    /// Cylinder `k`'s crank angle after its own firing TDC at time `t`.
    pub fn cylinder_angle(&self, k: usize, t: f64) -> f64 {
        (self.theta0_deg + 6.0 * self.rpm * t - self.offsets[k]).rem_euclid(720.0)
    }

    /// Mass trapped at EVO in one cylinder, kg.
    pub fn m_evo(&self) -> f64 {
        self.m_evo
    }

    pub(crate) fn init_state(&self, gas: &Gas, st: &mut NodeState) {
        let n = self.geometry.cylinders();
        st.ode = vec![0.0; n + 2 * self.manifolds.len()];
        for (j, m) in self.manifolds.iter().enumerate() {
            let mass = self.p_init * m.volume / (gas.r() * self.t_init);
            st.ode[n + 2 * j] = mass;
            st.ode[n + 2 * j + 1] = mass * gas.e(self.t_init);
        }
        st.deriv = vec![0.0; st.ode.len()];
        st.cyl_phase = (0..n)
            .map(|k| {
                if self.valves.is_open(self.cylinder_angle(k, 0.0)) {
                    CylPhase::Skip
                } else {
                    CylPhase::Closed
                }
            })
            .collect();
    }

    /// Discrete EVO/EVC events after a step: a cylinder entering its valve event is reset to
    /// the EVO mass. The lift is zero at EVO, so the step-size delay in the reset moves no mass.
    pub(crate) fn update_phases(&self, t: f64, _gas: &Gas, st: &mut NodeState) {
        for k in 0..self.geometry.cylinders() {
            let inside = self.valves.is_open(self.cylinder_angle(k, t));
            st.cyl_phase[k] = match (st.cyl_phase[k], inside) {
                (CylPhase::Closed, true) => {
                    st.ode[k] = self.m_evo;
                    CylPhase::Open
                }
                (CylPhase::Open, false) | (CylPhase::Skip, false) => CylPhase::Closed,
                (phase, _) => phase,
            };
        }
    }

    /// Manifold `j` state `(p, T)` from the ODE vector.
    pub fn manifold_state(&self, gas: &Gas, ode: &[f64], j: usize) -> (f64, f64) {
        let n = self.geometry.cylinders();
        let (m, u) = (ode[n + 2 * j], ode[n + 2 * j + 1]);
        let t = gas.t_from_e(u / m);
        (m * gas.r() * t / self.manifolds[j].volume, t)
    }

    pub(crate) fn eval(&self, gas: &Gas, t: f64, ps: &PortState, st: &mut NodeState) -> Result<()> {
        let n = self.geometry.cylinders();
        let r = gas.r();
        st.deriv.iter_mut().for_each(|d| *d = 0.0);
        let man: Vec<(f64, f64)> = (0..self.manifolds.len())
            .map(|j| self.manifold_state(gas, &st.ode, j))
            .collect();

        // Exhaust valves: cylinder ↔ manifold.
        for k in 0..n {
            if st.cyl_phase[k] != CylPhase::Open {
                continue;
            }
            let th = self.cylinder_angle(k, t);
            let area = self.valves.effective_area(th);
            if area == 0.0 {
                continue;
            }
            let j = self.cyl_manifold[k];
            let rho = st.ode[k] / self.geometry.volume(th);
            let pc = self.evo.pressure_pa * (rho / self.rho_evo).powf(self.n_poly);
            let tc = pc / (rho * r);
            let (pm, tm) = man[j];
            let (mdot, h) = if pc >= pm {
                (
                    area * nozzle_mass_flux(pc, tc, pm, gas.gamma(tc), r),
                    gas.h(tc),
                )
            } else {
                (
                    -area * nozzle_mass_flux(pm, tm, pc, gas.gamma(tm), r),
                    gas.h(tm),
                )
            };
            st.deriv[k] = -mdot;
            st.deriv[n + 2 * j] += mdot;
            st.deriv[n + 2 * j + 1] += mdot * h;
        }

        // Turbine nozzles → downpipe face. Unknown: face pressure p_b.
        let a_port = ps.area;
        let eta = self.extraction;
        let flows = |pb: f64| {
            let v_b = ps.v - wave_f(pb, ps.rho, ps.p, ps.c, ps.gamma).0;
            let rho_w = wave_rho(pb, ps.rho, ps.p, ps.gamma);
            let t_f = pb / (rho_w * r);
            let h0_f = gas.h(t_f) + 0.5 * v_b * v_b;
            let p0_f = pb + 0.5 * rho_w * v_b * v_b;
            let t0_f = t_f + 0.5 * v_b * v_b / gas.cp(t_f);
            let mut out = SourceFlows {
                v_b,
                h0_f,
                ..Default::default()
            };
            for (j, m) in self.manifolds.iter().enumerate() {
                let (pm, tm) = man[j];
                if pm >= pb {
                    let g = gas.gamma(tm);
                    let mdot = m.turbine_area * nozzle_mass_flux(pm, tm, pb, g, r);
                    let hm = gas.h(tm);
                    let h0 = hm - eta * (hm - gas.h(tm * (pb / pm).powf((g - 1.0) / g)));
                    out.mdot[j] = mdot;
                    out.h_man[j] = hm;
                    out.fwd_mass += mdot;
                    out.fwd_energy += mdot * h0;
                    out.power += mdot * (hm - h0);
                    out.energy += mdot * h0;
                } else {
                    let mdot =
                        -m.turbine_area * nozzle_mass_flux(p0_f, t0_f, pm, gas.gamma(t0_f), r);
                    out.mdot[j] = mdot;
                    out.h_man[j] = h0_f;
                    out.energy += mdot * h0_f;
                }
                out.net += out.mdot[j];
            }
            out.rho_b = if v_b < 0.0 && out.fwd_mass > 0.0 {
                let h0_in = out.fwd_energy / out.fwd_mass;
                pb / (r * gas.t_from_h(h0_in - 0.5 * v_b * v_b))
            } else {
                rho_w
            };
            out
        };
        let p_min = man.iter().map(|m| m.0).fold(ps.p, f64::min);
        let p_max = man.iter().map(|m| m.0).fold(ps.p, f64::max);
        let resid = |pb: f64| {
            let f = flows(pb);
            f.net + f.rho_b * f.v_b * a_port
        };
        let pb = brent(resid, 0.02 * p_min, 20.0 * p_max, 1e-10 * ps.p, 300).ok_or_else(|| {
            Error::solver(format!(
                "engine source: no downpipe face pressure at t = {t:.6e} s"
            ))
        })?;
        let f = flows(pb);
        for j in 0..self.manifolds.len() {
            st.deriv[n + 2 * j] -= f.mdot[j];
            st.deriv[n + 2 * j + 1] -= f.mdot[j] * f.h_man[j];
        }
        st.turbine_power = f.power;
        let h0 = if f.net.abs() > 0.0 {
            f.energy / f.net
        } else {
            f.h0_f
        };
        st.faces[0] = FaceState {
            mass_flux: -f.net / a_port,
            p: pb,
            v: f.v_b,
            h0,
            rho: f.rho_b,
        };
        Ok(())
    }
}

const MAX_MANIFOLDS: usize = 4;

#[derive(Default)]
struct SourceFlows {
    v_b: f64,
    rho_b: f64,
    h0_f: f64,
    mdot: [f64; MAX_MANIFOLDS],
    h_man: [f64; MAX_MANIFOLDS],
    net: f64,
    energy: f64,
    fwd_mass: f64,
    fwd_energy: f64,
    power: f64,
}
