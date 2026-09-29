//! Quasi-one-dimensional unsteady compressible flow in a network of ducts.
//!
//! Governing equations per duct, conservative form, area `A(x)`, hydraulic diameter `D(x)`:
//! ```text
//! ∂(ρA)/∂t + ∂(ρuA)/∂x                  = 0
//! ∂(ρuA)/∂t + ∂((ρu² + p)A)/∂x          = p dA/dx − τ_w (4/D) A − Σₖ Kₖ ½ρu|u| δ(x − xₖ) A
//! ∂(EA)/∂t  + ∂(u(E + p)A)/∂x           = h (T_w − T) (4/D) A
//! E = ρ(e(T) + u²/2),   p = ρ R T,   τ_w: `wall::shear_stress`,   h = Nu k / D: `wall::nusselt`
//! ```
//! Wall shear does no work on the gas (the wall is stationary), so friction appears only in the
//! momentum equation. `Kₖ` are minor losses (bends) spread over the cells they cover.
//!
//! Discretisation: cell-centred finite volumes. Interface fluxes from the HLLC approximate
//! Riemann solver (Toro 2009, §10.4) with MUSCL reconstruction of `(ρ, u, p)` and a TVD limiter;
//! SSP-RK2 (Heun) in time; `Δt = CFL · min Δx/(|u| + c)`, CFL ≤ 0.8. The `p dA/dx` term is
//! `pᵢ (A_{i+½} − A_{i−½})`, which balances the interface pressure fluxes exactly for gas at
//! rest. Cells next to a duct end use zero slope (first order); by Gustafsson's theorem this
//! keeps second-order global accuracy. Duct ends couple through [`Node`]s, which return the
//! boundary-face state from the interior state (exact Riemann wave curves or linear
//! characteristics, see `nodes`).

pub mod nodes;
pub mod porous;
pub mod scheme;
pub mod stokes;
pub mod wall;

use crate::error::{Error, Result};
use crate::gas::Gas;
pub use nodes::{
    CharacteristicBc, CylPhase, JunctionBc, JunctionKind, Manifold, MassFlowBc, Node, RadiationBc,
    Signal, SourceBc,
};
pub use porous::Porous;

/// Most duct ends one node may join.
pub const MAX_PORTS: usize = 8;

/// TVB bound of `Limiter::VanLeerTvb`, as a fraction of the cell's ρ, c and p.
pub const TVB_BOUND: f64 = 1e-3;
pub use scheme::Limiter;
use scheme::{State, hllc};

/// Which end of a duct a node attaches to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// `x = 0`.
    Start,
    /// `x = L`.
    End,
}

/// A duct end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Port {
    pub duct: usize,
    pub end: End,
}

impl Port {
    pub fn start(duct: usize) -> Self {
        Self {
            duct,
            end: End::Start,
        }
    }

    pub fn end(duct: usize) -> Self {
        Self {
            duct,
            end: End::End,
        }
    }
}

/// One finite-volume duct with uniform cell length.
#[derive(Clone, Debug)]
pub struct Duct {
    pub label: String,
    pub dx: f64,
    /// Area at the `n + 1` cell faces, m².
    pub area_face: Vec<f64>,
    /// Cell volumes, m³.
    pub volume: Vec<f64>,
    /// Hydraulic diameter at cell centres, m.
    pub diameter: Vec<f64>,
    /// Wall temperature per cell, K; `None` is an adiabatic wall.
    pub t_wall: Option<Vec<f64>>,
    /// Absolute wall roughness, m; used only when `friction` is on.
    pub roughness: f64,
    pub friction: bool,
    /// Minor-loss coefficient per cell, referred to the cell velocity.
    pub k_loss: Vec<f64>,
    /// Laminar channel bundle (catalyst monolith) with this Poiseuille number `f·Re`: the
    /// wall shear is `τ = ½ (f Re) μ u / D_h` at every Reynolds number.
    pub channel: Option<f64>,
    /// Outer wall seen by the thermal model; `None` loses no heat to ambient.
    pub wall: Option<WallSpec>,
    /// Perforated wall coupling this duct, cell by cell, to a partner duct of the same grid
    /// (the annulus of an absorptive muffler). Set on the inner duct only.
    pub perforate: Option<Perforate>,
    /// Fibrous fill occupying the duct (the annulus of an absorptive muffler).
    pub porous: Option<Porous>,
}

/// Perforated tube wall between a duct and its partner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Perforate {
    pub partner: usize,
    /// Open-area ratio σ.
    pub porosity: f64,
    pub hole_diameter: f64,
    pub thickness: f64,
    /// Tube circumference over which the transverse velocity is averaged, m.
    pub perimeter: f64,
}

impl Perforate {
    /// Discharge coefficient of the sharp-edged holes for the nonlinear (jet) resistance.
    pub const DISCHARGE: f64 = 0.61;

    /// Effective hole length `t + 0.75 d` (Sullivan & Crocker 1978).
    pub fn effective_length(&self) -> f64 {
        self.thickness + 0.75 * self.hole_diameter
    }

    /// Normalised linear resistance of the perforate, referred to the area-averaged transverse
    /// velocity: Sullivan & Crocker's 0.006 without flow, plus the grazing-flow term of Rao &
    /// Munjal (1986), `0.006 + 0.53 M`, divided by σ.
    pub fn resistance(&self, grazing_mach: f64) -> f64 {
        (0.006 + 0.53 * grazing_mach.abs()) / self.porosity
    }
}

/// Pipe or shell wall between the gas and ambient air.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WallSpec {
    /// Outer diameter, m.
    pub outer_diameter: f64,
    pub thickness: f64,
    /// W/(m K).
    pub conductivity: f64,
    /// Outer-surface emissivity.
    pub emissivity: f64,
}

impl Duct {
    /// Circular duct whose diameter varies linearly from `d_in` to `d_out` (straight when
    /// equal). `n = ⌈L/Δx_target⌉` cells; cell volumes are exact frustum volumes.
    pub fn conical(
        label: impl Into<String>,
        length: f64,
        d_in: f64,
        d_out: f64,
        dx_target: f64,
    ) -> Self {
        Self::from_profile(label, &[(length, d_in, d_out)], dx_target)
    }

    /// Circular duct made of consecutive segments `(length, d_in, d_out)` with linear diameter
    /// in each (pipes and cones joined without a step), on one uniform grid. Cell volumes are
    /// exact: each cell is integrated segment by segment as frustums.
    pub fn from_profile(
        label: impl Into<String>,
        segments: &[(f64, f64, f64)],
        dx_target: f64,
    ) -> Self {
        let length: f64 = segments.iter().map(|s| s.0).sum();
        let n = ((length / dx_target).ceil() as usize).max(2);
        let dx = length / n as f64;
        let diameter_at = |x: f64| {
            let mut s0 = 0.0;
            for &(len, a, b) in segments {
                if x <= s0 + len {
                    return a + (b - a) * ((x - s0) / len).clamp(0.0, 1.0);
                }
                s0 += len;
            }
            segments[segments.len() - 1].2
        };
        let frustum =
            |len: f64, a: f64, b: f64| std::f64::consts::PI * len / 12.0 * (a * a + a * b + b * b);
        let volume_between = |x0: f64, x1: f64| {
            let mut v = 0.0;
            let mut s0 = 0.0;
            for &(len, _, _) in segments {
                let (lo, hi) = (x0.max(s0), x1.min(s0 + len));
                if hi > lo {
                    v += frustum(hi - lo, diameter_at(lo), diameter_at(hi));
                }
                s0 += len;
            }
            v
        };
        let d_face: Vec<f64> = (0..=n).map(|i| diameter_at(i as f64 * dx)).collect();
        let area_face = d_face
            .iter()
            .map(|d| std::f64::consts::PI / 4.0 * d * d)
            .collect();
        let volume = (0..n)
            .map(|i| volume_between(i as f64 * dx, (i + 1) as f64 * dx))
            .collect();
        let diameter = (0..n).map(|i| diameter_at((i as f64 + 0.5) * dx)).collect();
        Self {
            label: label.into(),
            dx,
            area_face,
            volume,
            diameter,
            t_wall: None,
            roughness: 0.0,
            friction: false,
            k_loss: vec![0.0; n],
            channel: None,
            wall: None,
            perforate: None,
            porous: None,
        }
    }

    pub fn n(&self) -> usize {
        self.volume.len()
    }

    pub fn length(&self) -> f64 {
        self.dx * self.n() as f64
    }

    /// Enables wall friction with absolute roughness `roughness` (m).
    pub fn with_friction(mut self, roughness: f64) -> Self {
        self.friction = true;
        self.roughness = roughness;
        self
    }

    /// Spreads a minor loss `k` over `[x0, x1]` in proportion to cell overlap. A zero-length
    /// interval puts it in the cell containing `x0`.
    pub fn add_loss(&mut self, x0: f64, x1: f64, k: f64) {
        let n = self.n();
        if x1 <= x0 {
            let i = ((x0 / self.dx) as usize).min(n - 1);
            self.k_loss[i] += k;
            return;
        }
        for i in 0..n {
            let (a, b) = (i as f64 * self.dx, (i + 1) as f64 * self.dx);
            let overlap = (b.min(x1) - a.max(x0)).max(0.0);
            self.k_loss[i] += k * overlap / (x1 - x0);
        }
    }

    /// Area at the end a port refers to.
    pub fn end_area(&self, end: End) -> f64 {
        match end {
            End::Start => self.area_face[0],
            End::End => self.area_face[self.n()],
        }
    }

    pub fn end_cell(&self, end: End) -> usize {
        match end {
            End::Start => 0,
            End::End => self.n() - 1,
        }
    }
}

/// Ducts, the nodes that join their ends, and scheme settings.
#[derive(Clone, Debug)]
pub struct Network {
    pub gas: Gas,
    pub ducts: Vec<Duct>,
    pub nodes: Vec<Node>,
    pub limiter: Limiter,
    pub cfl: f64,
}

impl Network {
    /// Every duct end must belong to exactly one node.
    pub fn validate(&self) -> Result<()> {
        if !(self.cfl > 0.0 && self.cfl <= 0.8) {
            return Err(Error::invalid("CFL must be in (0, 0.8]"));
        }
        let mut count = vec![[0usize; 2]; self.ducts.len()];
        for node in &self.nodes {
            if node.ports().len() > MAX_PORTS {
                return Err(Error::invalid(format!(
                    "a node joins more than {MAX_PORTS} ducts"
                )));
            }
            for p in node.ports() {
                if p.duct >= self.ducts.len() {
                    return Err(Error::invalid(format!(
                        "node refers to missing duct {}",
                        p.duct
                    )));
                }
                count[p.duct][(p.end == End::End) as usize] += 1;
            }
        }
        for (i, c) in count.iter().enumerate() {
            if c[0] != 1 || c[1] != 1 {
                return Err(Error::invalid(format!(
                    "duct '{}' ends must each join exactly one node (start {}, end {})",
                    self.ducts[i].label, c[0], c[1]
                )));
            }
        }
        for duct in &self.ducts {
            if let Some(perf) = &duct.perforate {
                let ok = self.ducts.get(perf.partner).is_some_and(|o| {
                    o.n() == duct.n() && (o.dx - duct.dx).abs() < 1e-12 && o.perforate.is_none()
                });
                if !ok {
                    return Err(Error::invalid(format!(
                        "duct '{}': perforate partner must exist, share its grid and not be coupled itself",
                        duct.label
                    )));
                }
            }
        }
        Ok(())
    }
}

/// Primitive state of one cell.
#[derive(Clone, Copy, Debug, Default)]
pub struct Prim {
    pub rho: f64,
    pub u: f64,
    pub p: f64,
    pub t: f64,
    pub c: f64,
    /// Total energy per unit volume.
    pub en: f64,
}

impl Prim {
    fn state(&self) -> State {
        State {
            rho: self.rho,
            u: self.u,
            p: self.p,
            en: self.en,
            c: self.c,
        }
    }
}

/// Boundary-face state a node imposes on one of its ports.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FaceState {
    /// Mass flux toward the node, kg/(m² s).
    pub mass_flux: f64,
    /// Static pressure, Pa.
    pub p: f64,
    /// Velocity toward the node, m/s.
    pub v: f64,
    /// Stagnation enthalpy carried by the mass flux, J/kg.
    pub h0: f64,
    /// Density, kg/m³.
    pub rho: f64,
}

impl FaceState {
    /// Flux per unit area in the duct's +x direction.
    fn x_flux(&self, end: End) -> [f64; 3] {
        let mom = self.mass_flux * self.v + self.p;
        match end {
            End::End => [self.mass_flux, mom, self.mass_flux * self.h0],
            End::Start => [-self.mass_flux, mom, -self.mass_flux * self.h0],
        }
    }
}

/// Interior state next to a port, with `v` the velocity toward the node.
#[derive(Clone, Copy, Debug, Default)]
pub struct PortState {
    pub rho: f64,
    pub v: f64,
    pub p: f64,
    pub t: f64,
    pub c: f64,
    pub gamma: f64,
    pub area: f64,
}

/// Mutable per-node state: ODE variables (engine source), the radiation filter, the last
/// face states and time integrals of what crossed each port.
#[derive(Clone, Debug, Default)]
pub struct NodeState {
    pub ode: Vec<f64>,
    ode_n: Vec<f64>,
    deriv: Vec<f64>,
    pub filter: crate::radiation::LsFilter,
    filter_pred: crate::radiation::LsFilter,
    w_plus_n: f64,
    pub cyl_phase: Vec<CylPhase>,
    /// Face states from the latest evaluation, one per port.
    pub faces: Vec<FaceState>,
    /// Incident characteristic `W⁺` at the latest evaluation (radiation nodes), Pa.
    pub w_plus: f64,
    /// Reflected characteristic `W⁻` at the latest evaluation (radiation nodes), Pa.
    pub w_minus: f64,
    /// Time-integrated mass that crossed each port toward the node, kg.
    pub mass_to_node: Vec<f64>,
    /// Time-integrated stagnation enthalpy that crossed each port toward the node, J.
    pub energy_to_node: Vec<f64>,
    /// Latest turbine power (engine source), W.
    pub turbine_power: f64,
    /// Time-integrated turbine work, J.
    pub turbine_work: f64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    One,
    Two,
}

/// Time-marching state of a [`Network`].
pub struct Simulation {
    pub net: Network,
    pub t: f64,
    q: Vec<Vec<[f64; 3]>>,
    q_n: Vec<Vec<[f64; 3]>>,
    prim: Vec<Vec<Prim>>,
    rhs: Vec<Vec<[f64; 3]>>,
    slope: Vec<[[f64; 3]; 2]>,
    pub nodes: Vec<NodeState>,
    /// Largest Courant number of any step taken.
    pub max_cfl: f64,
    pub steps: u64,
    stokes: stokes::HalfDerivative,
    /// Stokes-layer memory states per duct: for each cell, the velocity states then the
    /// temperature states. Empty for ducts without wall friction and for channel bundles.
    phi: Vec<Vec<f64>>,
    /// Velocity and temperature of each cell at the start of the step.
    start: Vec<Vec<(f64, f64)>>,
    /// Transverse velocity through each cell's perforate (inner ducts of coupled pairs).
    perforate_u: Vec<Vec<f64>>,
    /// Diffusive representation behind the fills' `√(s + a)` operators.
    fill_rep: stokes::HalfDerivative,
    /// Fill memory states per porous duct: for each cell, the drag states then the heat
    /// exchange states.
    fill: Vec<Vec<f64>>,
    /// Fibre temperature of each porous cell: its initial gas temperature.
    fill_t: Vec<Vec<f64>>,
    /// Momentum removed and energy added by the fill in each porous cell's stage-1 predictor.
    fill_pred: Vec<Vec<[f64; 2]>>,
}

impl Simulation {
    /// Initialises every cell from `init(duct, x) = (ρ, u, p)` at the cell centre.
    pub fn new(net: Network, init: impl Fn(usize, f64) -> (f64, f64, f64)) -> Result<Self> {
        net.validate()?;
        let mut q: Vec<Vec<[f64; 3]>> = Vec::with_capacity(net.ducts.len());
        for (d, duct) in net.ducts.iter().enumerate() {
            q.push(
                (0..duct.n())
                    .map(|i| {
                        let (rho, u, p) = init(d, (i as f64 + 0.5) * duct.dx);
                        let s = State::from_primitive(&net.gas, rho, u, p);
                        [s.rho, s.rho * s.u, s.en]
                    })
                    .collect(),
            );
        }
        let prim = net
            .ducts
            .iter()
            .map(|d| vec![Prim::default(); d.n()])
            .collect();
        let rhs = net.ducts.iter().map(|d| vec![[0.0; 3]; d.n()]).collect();
        let max_n = net.ducts.iter().map(Duct::n).max().unwrap_or(0);
        let nodes = net
            .nodes
            .iter()
            .map(|node| {
                let ports = node.ports().len();
                let mut st = NodeState {
                    faces: vec![FaceState::default(); ports],
                    mass_to_node: vec![0.0; ports],
                    energy_to_node: vec![0.0; ports],
                    ..Default::default()
                };
                if let Node::Source(src) = node {
                    src.init_state(&net.gas, &mut st);
                }
                st
            })
            .collect();
        let stokes = stokes::HalfDerivative::default();
        let phi = net
            .ducts
            .iter()
            .map(|d| {
                if d.friction && d.channel.is_none() {
                    vec![0.0; 2 * stokes.states() * d.n()]
                } else {
                    Vec::new()
                }
            })
            .collect();
        let start = net.ducts.iter().map(|d| vec![(0.0, 0.0); d.n()]).collect();
        let perforate_u = net
            .ducts
            .iter()
            .map(|d| vec![0.0; if d.perforate.is_some() { d.n() } else { 0 }])
            .collect();
        let fill_rep = stokes::HalfDerivative::band(porous::BAND.0, porous::BAND.1);
        let np = fill_rep.states();
        let (mut fill, mut fill_t) = (Vec::new(), Vec::new());
        for (duct, q) in net.ducts.iter().zip(&q) {
            let (mut states, mut temps) = (Vec::new(), Vec::new());
            if let Some(f) = &duct.porous {
                for c in q {
                    let p = to_prim(&net.gas, *c);
                    // Drag memory in equilibrium with the initial velocity.
                    let wv = f.rates(&net.gas, p.rho, p.t).0;
                    states.extend(fill_rep.poles.iter().map(|s| p.u / (s + wv)));
                    states.extend(std::iter::repeat_n(0.0, np));
                    temps.push(p.t);
                }
            }
            fill.push(states);
            fill_t.push(temps);
        }
        let fill_pred = fill_t.iter().map(|t| vec![[0.0; 2]; t.len()]).collect();
        Ok(Self {
            q_n: q.clone(),
            q,
            prim,
            rhs,
            slope: vec![[[0.0; 3]; 2]; max_n],
            nodes,
            net,
            t: 0.0,
            max_cfl: 0.0,
            steps: 0,
            stokes,
            phi,
            start,
            perforate_u,
            fill_rep,
            fill,
            fill_t,
            fill_pred,
        })
    }

    /// Conserved variables `(ρ, ρu, E)` of a cell.
    pub fn conserved(&self, duct: usize, cell: usize) -> [f64; 3] {
        self.q[duct][cell]
    }

    /// Primitive state of a cell.
    pub fn prim(&self, duct: usize, cell: usize) -> Prim {
        to_prim(&self.net.gas, self.q[duct][cell])
    }

    /// Total mass in all ducts, kg.
    pub fn duct_mass(&self) -> f64 {
        self.q
            .iter()
            .zip(&self.net.ducts)
            .map(|(q, d)| q.iter().zip(&d.volume).map(|(c, v)| c[0] * v).sum::<f64>())
            .sum()
    }

    /// Total energy in all ducts, J.
    pub fn duct_energy(&self) -> f64 {
        self.q
            .iter()
            .zip(&self.net.ducts)
            .map(|(q, d)| q.iter().zip(&d.volume).map(|(c, v)| c[2] * v).sum::<f64>())
            .sum()
    }

    /// Largest stable step for the current state.
    pub fn stable_dt(&mut self) -> Result<f64> {
        self.update_prims()?;
        Ok(self.net.cfl / self.max_wave_rate())
    }

    fn max_wave_rate(&self) -> f64 {
        let mut rate: f64 = 0.0;
        for (duct, prim) in self.net.ducts.iter().zip(&self.prim) {
            for p in prim {
                rate = rate.max((p.u.abs() + p.c) / duct.dx);
            }
        }
        rate
    }

    /// Evaluates every node on the current state without advancing (face states for output).
    pub fn evaluate_faces(&mut self) -> Result<()> {
        self.update_prims()?;
        self.eval_nodes(self.t, 0.0, Stage::One)
    }

    /// Advances by `dt` with SSP-RK2.
    pub fn step(&mut self, dt: f64) -> Result<()> {
        let t0 = self.t;
        self.update_prims()?;
        self.max_cfl = self.max_cfl.max(dt * self.max_wave_rate());
        self.q_n.clone_from(&self.q);
        for (d, start) in self.start.iter_mut().enumerate() {
            if !self.phi[d].is_empty() {
                for (s, p) in start.iter_mut().zip(&self.prim[d]) {
                    *s = (p.u, p.t);
                }
            }
        }

        // Stage 1: q¹ = qⁿ + Δt L(qⁿ).
        self.eval_nodes(t0, dt, Stage::One)?;
        self.accumulate(0.5 * dt);
        self.compute_rhs();
        for (q, r) in self.q.iter_mut().zip(&self.rhs) {
            for (c, d) in q.iter_mut().zip(r) {
                for k in 0..3 {
                    c[k] += dt * d[k];
                }
            }
        }
        for st in &mut self.nodes {
            st.ode_n.clone_from(&st.ode);
            for (y, k) in st.ode.iter_mut().zip(&st.deriv) {
                *y += dt * k;
            }
        }
        self.fills(dt, Stage::One);

        // Stage 2: qⁿ⁺¹ = ½qⁿ + ½(q¹ + Δt L(q¹)).
        self.update_prims()?;
        self.eval_nodes(t0 + dt, dt, Stage::Two)?;
        self.accumulate(0.5 * dt);
        self.compute_rhs();
        for ((q, qn), r) in self.q.iter_mut().zip(&self.q_n).zip(&self.rhs) {
            for ((c, cn), d) in q.iter_mut().zip(qn).zip(r) {
                for k in 0..3 {
                    c[k] = 0.5 * (cn[k] + c[k] + dt * d[k]);
                }
            }
        }
        for (st, node) in self.nodes.iter_mut().zip(&self.net.nodes) {
            for ((y, yn), k) in st.ode.iter_mut().zip(&st.ode_n).zip(&st.deriv) {
                *y = 0.5 * (yn + *y + dt * k);
            }
            if matches!(node, Node::Radiation(_)) {
                st.filter = st.filter_pred;
            }
        }

        self.t = t0 + dt;
        self.steps += 1;
        self.stokes_layers(dt);
        self.perforates(dt);
        self.fills(dt, Stage::Two);
        for (st, node) in self.nodes.iter_mut().zip(&self.net.nodes) {
            if let Node::Source(src) = node {
                src.update_phases(self.t, &self.net.gas, st);
            }
        }
        Ok(())
    }

    /// Flow through perforated walls, applied after the step cell pair by cell pair.
    ///
    /// The area-averaged transverse velocity `u` (inner to outer) obeys the Sullivan–Crocker
    /// perforate impedance, `(ρ l_e/σ) du/dt + (ρc θ/σ + ρ|u|/(2C_d²σ²)) u = p_in − p_out`
    /// (`Perforate`; the quadratic jet term makes it nonlinear at high amplitude), and moves mass
    /// `ρ u A_s` across the tube surface `A_s = perimeter Δx`, which changes the pressure
    /// difference at `K = ρ A_s (c_in²/V_in + c_out²/V_out)` per unit `u`. That pair is
    /// integrated by the trapezoidal rule — the transverse resonance is too fast for the
    /// explicit step at high porosity. The transferred mass carries its upstream stagnation
    /// enthalpy (energy conserved) and takes the axial velocity of the channel it is in, the
    /// wall taking the difference: Sullivan–Crocker's momentum equation, which the four-pole
    /// model shares, has no transfer term.
    fn perforates(&mut self, dt: f64) {
        let gas = &self.net.gas;
        for d in 0..self.net.ducts.len() {
            let Some(perf) = self.net.ducts[d].perforate else {
                continue;
            };
            let o = perf.partner;
            let (inner, outer) = (&self.net.ducts[d], &self.net.ducts[o]);
            let l_e = perf.effective_length();
            for i in 0..inner.n() {
                let (pi, po) = (to_prim(gas, self.q[d][i]), to_prim(gas, self.q[o][i]));
                let (vi, vo) = (inner.volume[i], outer.volume[i]);
                let area = perf.perimeter * inner.dx;
                let u0 = self.perforate_u[d][i];
                let rho = if u0 >= 0.0 { pi.rho } else { po.rho };
                let mass = rho * l_e / perf.porosity;
                let resist = rho * pi.c * perf.resistance(pi.u / pi.c)
                    + rho * u0.abs() / (2.0 * (Perforate::DISCHARGE * perf.porosity).powi(2));
                let k = rho * area * (pi.c * pi.c / vi + po.c * po.c / vo);
                let a = dt / mass * (0.25 * k * dt + 0.5 * resist);
                let u1 = (u0 * (1.0 - a) + dt / mass * (pi.p - po.p)) / (1.0 + a);
                self.perforate_u[d][i] = u1;
                let dm = rho * area * dt * 0.5 * (u0 + u1);
                let (from, to, src) = if dm >= 0.0 { (d, o, pi) } else { (o, d, po) };
                let (v_from, v_to) = if dm >= 0.0 { (vi, vo) } else { (vo, vi) };
                let m = dm.abs();
                let h0 = src.en / src.rho + src.p / src.rho;
                for (duct, sign, v) in [(from, -1.0, v_from), (to, 1.0, v_to)] {
                    let u = if duct == d { pi.u } else { po.u };
                    let q = &mut self.q[duct][i];
                    q[0] += sign * m / v;
                    q[1] += sign * m * u / v;
                    q[2] += sign * m * h0 / v;
                }
            }
        }
    }

    /// Fibre drag and heat exchange of porous fills (`porous`), implicit: at dense packings the
    /// drag stops the pore gas far faster than the acoustic step. Stage 1's predictor is relaxed
    /// by backward Euler, so stage 2's fluxes carry fill velocities, not free-gas ones (which
    /// would move a factor `1 + λΔt/2` too much gas); after stage 2 the half of those terms the
    /// stage average holds is taken out and the whole step relaxed again, advancing the memory:
    /// the explicit trapezoid for the fluxes, backward Euler for the fill, statics exact. The
    /// drag keeps the total energy (the kinetic energy it removes becomes heat); the heat
    /// exchange moves the internal energy toward the fibre temperature.
    fn fills(&mut self, dt: f64, stage: Stage) {
        let gas = &self.net.gas;
        let np = self.fill_rep.states();
        let last = stage == Stage::Two;
        let decay: Vec<f64> = self
            .fill_rep
            .poles
            .iter()
            .map(|s| (-s * dt).exp())
            .collect();
        for (d, duct) in self.net.ducts.iter().enumerate() {
            let Some(fill) = &duct.porous else { continue };
            for i in 0..duct.n() {
                let (psi_u, psi_t) = self.fill[d][2 * np * i..2 * np * (i + 1)].split_at_mut(np);
                let q = &mut self.q[d][i];
                let pred = &mut self.fill_pred[d][i];
                if last {
                    q[1] += 0.5 * pred[0];
                    q[2] -= 0.5 * pred[1];
                }
                let p = to_prim(gas, *q);
                let (wv, wt) = fill.rates(gas, p.rho, p.t);
                let c = fill.drag(gas, p.t) / (p.rho * wv.sqrt());
                let u = porous::relax(&self.fill_rep, &decay, psi_u, p.u, wv, c, dt, last);
                q[1] = p.rho * u;
                let p2 = to_prim(gas, *q);
                let tf = self.fill_t[d][i];
                let c = gas.gamma(p2.t) * (0.5 * wt).sqrt();
                let theta = porous::relax(
                    &self.fill_rep,
                    &decay,
                    psi_t,
                    p2.t - tf,
                    2.0 * wt,
                    c,
                    dt,
                    last,
                );
                let de = p2.rho * (gas.e(tf + theta) - gas.e(p2.t));
                q[2] += de;
                // Momentum the drag removed and energy the fibres added.
                *pred = [p.rho * (p.u - u), de];
            }
        }
    }

    /// Stokes-layer wall shear and heat flux (`stokes`), applied after the step from the
    /// change of each cell's velocity and temperature over it (first-order splitting; the
    /// terms change the momentum by ~10⁻³ per step).
    fn stokes_layers(&mut self, dt: f64) {
        let (decay, gain) = self.stokes.step_factors(dt);
        let np = self.stokes.states();
        let gas = &self.net.gas;
        for (d, duct) in self.net.ducts.iter().enumerate() {
            if self.phi[d].is_empty() {
                continue;
            }
            for i in 0..duct.n() {
                let p = to_prim(gas, self.q[d][i]);
                let (u0, t0) = self.start[d][i];
                let (phi_u, phi_t) = self.phi[d][2 * np * i..2 * np * (i + 1)].split_at_mut(np);
                let du = self.stokes.advance(phi_u, p.u - u0, &decay, &gain);
                let dtemp = self.stokes.advance(phi_t, p.t - t0, &decay, &gain);
                let tau = (p.rho * gas.viscosity(p.t)).sqrt() * du;
                let heat = -(p.rho * gas.cp(p.t) * gas.conductivity(p.t)).sqrt() * dtemp;
                let per = 4.0 / duct.diameter[i];
                let q = &mut self.q[d][i];
                q[1] -= dt * per * tau;
                q[2] += dt * per * heat;
            }
        }
    }

    fn update_prims(&mut self) -> Result<()> {
        let gas = &self.net.gas;
        for (d, (q, prim)) in self.q.iter().zip(self.prim.iter_mut()).enumerate() {
            for (i, (c, p)) in q.iter().zip(prim.iter_mut()).enumerate() {
                *p = to_prim(gas, *c);
                if !(p.rho > 0.0 && p.p > 0.0 && p.t.is_finite() && p.u.is_finite()) {
                    return Err(Error::solver(format!(
                        "non-physical state in duct '{}' cell {i} at t = {:.6e} s (ρ = {}, p = {})",
                        self.net.ducts[d].label, self.t, p.rho, p.p
                    )));
                }
            }
        }
        Ok(())
    }

    fn port_state(&self, port: Port) -> PortState {
        let duct = &self.net.ducts[port.duct];
        let p = self.prim[port.duct][duct.end_cell(port.end)];
        let gamma = self.net.gas.gamma(p.t);
        let v = match port.end {
            End::End => p.u,
            End::Start => -p.u,
        };
        PortState {
            rho: p.rho,
            v,
            p: p.p,
            t: p.t,
            c: p.c,
            gamma,
            area: duct.end_area(port.end),
        }
    }

    fn eval_nodes(&mut self, t: f64, dt: f64, stage: Stage) -> Result<()> {
        for i in 0..self.net.nodes.len() {
            let mut buf = [PortState::default(); MAX_PORTS];
            let node_ports = self.net.nodes[i].ports();
            for (k, &p) in node_ports.iter().enumerate() {
                buf[k] = self.port_state(p);
            }
            let ports = &buf[..node_ports.len()];
            let gas = &self.net.gas;
            let st = &mut self.nodes[i];
            match &self.net.nodes[i] {
                Node::Wall(_) => st.faces[0] = nodes::wall(&ports[0])?,
                Node::Pressure { p, t: t0, .. } => {
                    st.faces[0] = nodes::reservoir(gas, &ports[0], *p, *t0)?
                }
                Node::Radiation(bc) => {
                    let ps = &ports[0];
                    let w_plus = (ps.p - bc.p_amb) + ps.rho * ps.c * ps.v;
                    let (face, w_plus_r) = match stage {
                        Stage::One => {
                            let (face, w_plus_r) =
                                nodes::radiation(gas, ps, bc, w_plus, st.filter.output());
                            st.w_plus_n = w_plus_r;
                            (face, w_plus_r)
                        }
                        Stage::Two => {
                            // The filter state at t + Δt depends on its input at t + Δt, which
                            // depends on the inflow loss; two fixed-point passes.
                            let h = dt * ps.c / bc.radius;
                            let mut input = w_plus;
                            let mut out = (FaceState::default(), w_plus);
                            for _ in 0..2 {
                                st.filter_pred = st.filter.advanced(h, st.w_plus_n, input);
                                out =
                                    nodes::radiation(gas, ps, bc, w_plus, st.filter_pred.output());
                                input = out.1;
                            }
                            out
                        }
                    };
                    st.w_plus = w_plus_r;
                    st.w_minus = if stage == Stage::One {
                        st.filter.output()
                    } else {
                        st.filter_pred.output()
                    };
                    st.faces[0] = face;
                }
                Node::Junction(j) => j.eval(gas, ports, &mut st.faces)?,
                Node::Source(src) => src.eval(gas, t, dt, &ports[0], st)?,
                Node::Characteristic(bc) => {
                    st.faces[0] = nodes::characteristic(gas, &ports[0], bc, t)
                }
                Node::MassFlow(bc) => st.faces[0] = nodes::mass_flow(gas, &ports[0], bc, t)?,
            }
        }
        Ok(())
    }

    fn accumulate(&mut self, w: f64) {
        for (st, node) in self.nodes.iter_mut().zip(&self.net.nodes) {
            for (k, port) in node.ports().iter().enumerate() {
                let area = self.net.ducts[port.duct].end_area(port.end);
                let f = st.faces[k];
                st.mass_to_node[k] += w * f.mass_flux * area;
                st.energy_to_node[k] += w * f.mass_flux * f.h0 * area;
            }
            st.turbine_work += w * st.turbine_power;
        }
    }

    fn compute_rhs(&mut self) {
        let gas = &self.net.gas;
        let limiter = self.net.limiter;
        // Boundary fluxes (+x, per unit area) at each duct's start and end.
        let mut bflux = vec![[[0.0; 3]; 2]; self.net.ducts.len()];
        for (st, node) in self.nodes.iter().zip(&self.net.nodes) {
            for (k, port) in node.ports().iter().enumerate() {
                bflux[port.duct][(port.end == End::End) as usize] = st.faces[k].x_flux(port.end);
            }
        }
        for (d, duct) in self.net.ducts.iter().enumerate() {
            let prim = &self.prim[d];
            let n = duct.n();
            // off[i] = (left-face, right-face) offsets of (ρ, u, p) from the cell average.
            let off = &mut self.slope[..n];
            off[0] = [[0.0; 3]; 2];
            off[n - 1] = [[0.0; 3]; 2];
            for i in 1..n - 1 {
                let (a, b, c) = (&prim[i - 1], &prim[i], &prim[i + 1]);
                let r = limiter.offsets(b.rho - a.rho, c.rho - b.rho, TVB_BOUND * b.rho);
                let u = limiter.offsets(b.u - a.u, c.u - b.u, TVB_BOUND * b.c);
                let p = limiter.offsets(b.p - a.p, c.p - b.p, TVB_BOUND * b.p);
                off[i] = [[r.0, u.0, p.0], [r.1, u.1, p.1]];
            }
            let face_state = |i: usize, sign: f64| -> State {
                let s = off[i][(sign > 0.0) as usize];
                if s == [0.0; 3] {
                    return prim[i].state();
                }
                let (rho, u, p) = (
                    prim[i].rho + sign * s[0],
                    prim[i].u + sign * s[1],
                    prim[i].p + sign * s[2],
                );
                if rho > 0.0 && p > 0.0 {
                    State::from_primitive(gas, rho, u, p)
                } else {
                    prim[i].state()
                }
            };
            let rhs = &mut self.rhs[d];
            let b = bflux[d];
            let mut f_prev = b[0].map(|f| f * duct.area_face[0]);
            for i in 0..n {
                let f_next = if i + 1 < n {
                    hllc(&face_state(i, 1.0), &face_state(i + 1, -1.0))
                        .map(|f| f * duct.area_face[i + 1])
                } else {
                    b[1].map(|f| f * duct.area_face[n])
                };
                let p = &prim[i];
                let vol = duct.volume[i];
                let mut r = [
                    f_prev[0] - f_next[0],
                    f_prev[1] - f_next[1],
                    f_prev[2] - f_next[2],
                ];
                r[1] += p.p * (duct.area_face[i + 1] - duct.area_face[i]);
                let dia = duct.diameter[i];
                if duct.friction {
                    let mu = gas.viscosity(p.t);
                    let tau = match duct.channel {
                        Some(fre) => 0.5 * fre * mu * p.u / dia,
                        None => wall::shear_stress(p.rho, p.u, mu, dia, duct.roughness),
                    };
                    r[1] -= tau * 4.0 * vol / dia;
                }
                if duct.k_loss[i] != 0.0 {
                    r[1] -= duct.k_loss[i] * 0.5 * p.rho * p.u * p.u.abs() * vol / duct.dx;
                }
                if let Some(tw) = &duct.t_wall {
                    let mu = gas.viscosity(p.t);
                    let re = p.rho * p.u.abs() * dia / mu;
                    let h = wall::nusselt(re, gas.prandtl()) * gas.conductivity(p.t) / dia;
                    r[2] += h * (tw[i] - p.t) * 4.0 * vol / dia;
                }
                rhs[i] = [r[0] / vol, r[1] / vol, r[2] / vol];
                f_prev = f_next;
            }
        }
    }
}

#[inline]
fn to_prim(gas: &Gas, c: [f64; 3]) -> Prim {
    let rho = c[0];
    let u = c[1] / rho;
    let e = c[2] / rho - 0.5 * u * u;
    let t = gas.t_from_e(e);
    Prim {
        rho,
        u,
        p: rho * gas.r() * t,
        t,
        c: gas.sound_speed(t),
        en: c[2],
    }
}
