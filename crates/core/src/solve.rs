//! Operating-point and sweep solutions of the time-domain model.
//!
//! Per engine speed: estimate the EVO state; pre-run the engine source into a short pipe to
//! get the mean flow and temperature for the initial state; march the full network cycle by
//! cycle, sampling outputs at `samples_per_cycle` equally spaced crank angles, until the last
//! cycle agrees with the one before (period 1) or with the one two back (period 2) to
//! `periodicity_tolerance` (at least `min_cycles`, at most `max_cycles`). The last four cycles
//! are Fourier-analysed; a solution of period P cycles has lines only at multiples of
//! `f_c / P`, `f_c = rpm/120` (engine order m/(2P)), so the spectrum is reported exactly at
//! those lines.
//!
//! Period 2 occurs when the tailpipe flow reverses and ingests ambient air: the cold slug left
//! in the tailpipe changes the next cycle's acoustics. Standard 1D practice (ambient inflow)
//! is kept; wall heat transfer damps it.
//!
//! Receiver: each outlet is a monopole of volume velocity `Q = v_face A` above a rigid ground
//! (`radiation::monopole_pressure`), in ambient air (ρ₀ = p/(287.05 T), c₀ = √(1.4·287.05 T)).
//! SPL in dB re 20 µPa of the harmonic's RMS pressure.

use rayon::prelude::*;
use rustfft::num_complex::Complex64;
use serde::Serialize;

use crate::engine::{EvoState, Load, estimate_evo_state};
use crate::error::{Error, Result};
use crate::gas::Gas;
use crate::gas1d::{Duct, Limiter, Network, Node, Port, Simulation};
use crate::geometry::{Vec3, add, scale, unit};
use crate::metrics::{self, Metrics};
use crate::model::{Model, SourceInputs, build};
use crate::project::{Project, WallThermal};
use crate::radiation::{a_weighting_db, monopole_pressure};
use crate::spectrum::fft;
use crate::thermal::{self, ThermalInputs};

const P_REF: f64 = 20e-6;
const R_AIR: f64 = 287.05;

/// Which solver produced a result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SolverKind {
    TimeDomain,
    FourPole,
}

/// Where the in-cylinder state at EVO came from.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EvoSource {
    /// Ideal Otto-cycle estimate at this manifold pressure.
    Estimated { map_kpa: f64 },
    /// `operating.evo_override`.
    Given,
}

/// How a result was produced; attached to every result.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Provenance {
    pub solver: SolverKind,
    pub core_version: String,
    pub project_hash: String,
    pub dx_target_mm: f64,
    pub cells: usize,
    pub dx_min_mm: f64,
    pub dx_max_mm: f64,
    pub gas_table: String,
    /// Wall and gas temperature model, with a BLAKE3 hash of its inputs.
    pub thermal_profile: String,
    pub evo: EvoState,
    pub evo_source: EvoSource,
    /// Time-domain run details; absent for four-pole results.
    pub time_domain: Option<TimeDomainRun>,
    /// Four-pole result of a network with elements that are nonlinear at exhaust amplitudes
    /// (valves, orifices): valid for small signals only.
    pub small_signal: bool,
    /// Modelling limitations that affect this result.
    pub warnings: Vec<String>,
}

impl Provenance {
    /// Four-pole results are direct solves; time-domain results must meet the periodicity
    /// tolerance.
    pub fn converged(&self) -> bool {
        self.time_domain.as_ref().is_none_or(|t| t.converged)
    }
}

/// Scheme and convergence of a time-domain run.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TimeDomainRun {
    pub cfl: f64,
    pub cfl_max: f64,
    pub limiter: Limiter,
    pub converged: bool,
    /// Period of the solution in engine cycles (1 or 2).
    pub period_cycles: u32,
    pub cycles: u32,
    /// Largest relative change of the monitored signals between the last cycle and the cycle
    /// one period earlier.
    pub periodicity_residual: f64,
    pub periodicity_tolerance: f64,
}

/// One spectral line at the receiver.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Line {
    pub frequency_hz: f64,
    /// Engine order (multiples of ½ for a four-stroke).
    pub order: f64,
    pub spl_db: f64,
    pub spl_dba: f64,
    /// Phase of the receiver pressure, rad (`e^{jωt}`, t = 0 at cylinder 1 firing TDC).
    pub phase_rad: f64,
}

/// Solution at one engine speed.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PointResult {
    pub rpm: f64,
    pub provenance: Provenance,
    pub spectrum: Vec<Line>,
    /// Engine orders 1–8: `(order, frequency_hz, spl_db)`.
    pub orders: Vec<(u32, f64, f64)>,
    pub overall_db: f64,
    pub overall_dba: f64,
    /// Mean static pressure above ambient at the downpipe flange, Pa (time domain only).
    pub backpressure_pa: Option<f64>,
    pub mass_flow_kg_s: f64,
    /// |outlet mass − source mass| / source mass over the analysed cycles (time domain only).
    pub mass_balance_error: Option<f64>,
    pub turbine_power_w: f64,
    /// Mean gas temperature leaving the reference outlet, K.
    pub outlet_temperature_k: f64,
}

/// A point either solved or failed; failures carry the reason and no numbers.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PointOutcome {
    Solved(Box<PointResult>),
    Failed { rpm: f64, error: String },
}

#[derive(Clone, Debug, Serialize)]
pub struct SweepResult {
    pub project: String,
    pub project_hash: String,
    pub solver: SolverKind,
    pub points: Vec<PointOutcome>,
    pub metrics: Metrics,
}

/// Solves every engine speed in parallel (one Rayon task per point; points are independent,
/// so results do not depend on thread count or scheduling).
pub fn solve_sweep(project: &Project, rpms: &[f64]) -> SweepResult {
    sweep(project, rpms, SolverKind::TimeDomain)
}

/// Sweep with either solver.
pub fn sweep(project: &Project, rpms: &[f64], solver: SolverKind) -> SweepResult {
    let points = rpms
        .par_iter()
        .map(|&rpm| {
            let r = match solver {
                SolverKind::TimeDomain => solve_point(project, rpm),
                SolverKind::FourPole => crate::preview::solve_point(project, rpm),
            };
            match r {
                Ok(r) => PointOutcome::Solved(Box::new(r)),
                Err(e) => PointOutcome::Failed {
                    rpm,
                    error: e.to_string(),
                },
            }
        })
        .collect::<Vec<_>>();
    SweepResult {
        project: project.name.clone(),
        project_hash: project.hash(),
        solver,
        metrics: metrics::evaluate(project, &points),
        points,
    }
}

/// Spectrum at the receiver.
pub struct ReceiverSpectrum {
    pub lines: Vec<Line>,
    /// Engine orders 1–8: `(order, frequency_hz, spl_db)`.
    pub orders: Vec<(u32, f64, f64)>,
    pub overall_db: f64,
    pub overall_dba: f64,
}

/// Receiver spectrum from the complex volume velocity of each outlet at each line: lines
/// spaced `f_line` until `volume_velocity` returns `None` or `max_frequency_hz`; the order of
/// line `m` is `m f_line / (rpm/60)`.
pub fn receiver_spectrum(
    project: &Project,
    model: &Model,
    rpm: f64,
    f_line: f64,
    mut volume_velocity: impl FnMut(usize) -> Option<Vec<Complex64>>,
) -> Result<ReceiverSpectrum> {
    let p_amb = project.ambient.pressure_pa;
    let t_amb = project.ambient.temperature_k;
    let rho0 = p_amb / (R_AIR * t_amb);
    let c0 = (1.4 * R_AIR * t_amb).sqrt();
    let positions: Vec<Vec3> = model.outlets.iter().map(|o| o.position).collect();
    let receiver = receiver_position(project, model, reference_outlet(project, model)?);
    let ground_z = project.ambient.ground_z_mm * 1e-3;
    let mut spectrum = Vec::new();
    let mut m = 1;
    while m as f64 * f_line <= project.solver.max_frequency_hz {
        let Some(q) = volume_velocity(m) else { break };
        let f = m as f64 * f_line;
        let p = monopole_pressure(
            2.0 * std::f64::consts::PI * f,
            rho0,
            c0,
            &positions,
            &q,
            receiver,
            ground_z,
        );
        let spl = 20.0 * (p.norm() / (std::f64::consts::SQRT_2 * P_REF)).log10();
        spectrum.push(Line {
            frequency_hz: f,
            order: f / (rpm / 60.0),
            spl_db: spl,
            spl_dba: spl + a_weighting_db(f),
            phase_rad: p.arg(),
        });
        m += 1;
    }
    let orders = (1..=8u32)
        .filter_map(|o| {
            spectrum
                .iter()
                .find(|l| (l.order - o as f64).abs() < 1e-6)
                .map(|l| (o, l.frequency_hz, l.spl_db))
        })
        .collect();
    let energy_sum = |f: fn(&Line) -> f64| {
        10.0 * spectrum
            .iter()
            .map(|l| 10f64.powf(f(l) / 10.0))
            .sum::<f64>()
            .log10()
    };
    Ok(ReceiverSpectrum {
        overall_db: energy_sum(|l| l.spl_db),
        overall_dba: energy_sum(|l| l.spl_dba),
        lines: spectrum,
        orders,
    })
}

/// Grid summary for provenance: (cells, smallest and largest Δx in mm).
pub fn grid_summary(net: &Network) -> (usize, f64, f64) {
    let dxs = net.ducts.iter().map(|d| d.dx * 1e3);
    (
        net.ducts.iter().map(Duct::n).sum(),
        dxs.clone().fold(f64::INFINITY, f64::min),
        dxs.fold(0.0, f64::max),
    )
}

/// EVO state for `rpm` and where it came from.
pub fn evo_state(project: &Project, gas: &Gas, rpm: f64) -> (EvoState, EvoSource) {
    if let Some(evo) = project.operating.evo_override {
        return (evo, EvoSource::Given);
    }
    let op = &project.operating;
    let map_kpa = op.map_at(rpm);
    let load = Load {
        map_kpa,
        intake_temperature_k: op.intake_temperature_k,
        volumetric_efficiency: op.volumetric_efficiency,
        lhv_mj_per_kg: op.lhv_mj_per_kg,
        heat_retained: op.heat_retained,
        n_compression: op.n_compression,
        n_expansion: op.n_expansion,
    };
    let evo = estimate_evo_state(
        &project.engine.geometry,
        &project.engine.valves,
        gas,
        &load,
        project.gas.fuel_h_to_c,
        project.gas.lambda,
    );
    (evo, EvoSource::Estimated { map_kpa })
}

pub fn solve_point(project: &Project, rpm: f64) -> Result<PointResult> {
    if rpm.is_nan() || rpm <= 0.0 {
        return Err(Error::invalid("engine speed must be positive"));
    }
    let gas = Gas::exhaust(project.gas.fuel_h_to_c, project.gas.lambda)?;
    let (evo, evo_source) = evo_state(project, &gas, rpm);
    let settings = &project.solver;
    let dx = settings.dx_mm * 1e-3;
    let p_amb = project.ambient.pressure_pa;
    let n_samples = settings.samples_per_cycle as usize;

    let pre = prerun(project, &gas, rpm, evo, dx)?;
    let mut model = build(
        project,
        gas.clone(),
        dx,
        SourceInputs {
            rpm,
            evo,
            p_init: pre.manifold_p,
            t_init: pre.manifold_t,
        },
    )?;
    let profile = match project.solver.wall_thermal {
        WallThermal::Computed => Some(thermal::solve(
            &model.network,
            model.source_node,
            &thermal_inputs(project, pre.mass_flow, pre.t0),
        )?),
        _ => None,
    };
    if let Some(pr) = &profile {
        for (d, duct) in model.network.ducts.iter_mut().enumerate() {
            if duct.wall.is_some() {
                duct.t_wall = Some(pr.t_wall[d].clone());
            }
        }
    }
    let t_uniform = match project.solver.wall_thermal {
        WallThermal::Fixed { temperature_k } => temperature_k.max(0.5 * pre.t0),
        _ => pre.t0,
    };
    let shares = model.flow_share.clone();
    let ducts = model.network.ducts.clone();
    let mut sim = Simulation::new(model.network.clone(), |d, x| {
        let t = match &profile {
            Some(pr) => pr.t_gas[d][((x / ducts[d].dx) as usize).min(ducts[d].n() - 1)],
            None => t_uniform,
        };
        let rho = p_amb / (gas.r() * t);
        (
            rho,
            shares[d] * pre.mass_flow / (rho * ducts[d].area_face[0]),
            p_amb,
        )
    })?;

    let cycle_time = 120.0 / rpm;
    let mut rec = Recorder::new(model.outlets.len(), n_samples);
    let mut cycles = 0u32;
    let (mut residual, mut period): (f64, u32);
    let mut mass_marks = Vec::new();
    let tol = settings.periodicity_tolerance;
    loop {
        mass_marks.push(Marks::read(&sim, &model));
        run_cycle(&mut sim, cycle_time, n_samples, |sim| {
            rec.sample(sim, &model)
        })?;
        cycles += 1;
        let r1 = if cycles >= 2 {
            rec.periodicity_residual(1)
        } else {
            f64::INFINITY
        };
        let r2 = if cycles >= 3 {
            rec.periodicity_residual(2)
        } else {
            f64::INFINITY
        };
        (residual, period) = if r1 <= tol || r1 <= r2 {
            (r1, 1)
        } else {
            (r2, 2)
        };
        if (cycles >= settings.min_cycles && residual <= tol) || cycles >= settings.max_cycles {
            break;
        }
    }
    mass_marks.push(Marks::read(&sim, &model));
    let converged = residual <= tol;

    // Analysis over the last four cycles.
    let analysed = 4.min(cycles as usize);
    let span = analysed as f64 * cycle_time;
    let (m0, m1) = (
        &mass_marks[mass_marks.len() - 1 - analysed],
        &mass_marks[mass_marks.len() - 1],
    );
    let source_mass = -(m1.source - m0.source);
    let outlet_mass: f64 = m1.outlets.iter().zip(&m0.outlets).map(|(a, b)| a - b).sum();
    let ref_idx = reference_outlet(project, &model)?;
    let outlet_energy = m1.outlet_energy[ref_idx] - m0.outlet_energy[ref_idx];
    let ref_mass = m1.outlets[ref_idx] - m0.outlets[ref_idx];
    let outlet_h0 = if ref_mass > 0.0 {
        outlet_energy / ref_mass
    } else {
        f64::NAN
    };

    let q_spectra: Vec<Vec<Complex64>> = (0..model.outlets.len())
        .map(|o| fft(&rec.last_cycles(&rec.outlet_q[o], analysed)))
        .collect();
    let n_total = analysed * n_samples;
    // Lines at multiples of f_c/P fall on every (analysed/P)-th FFT bin.
    let bins_per_line = analysed / period as usize;
    let ReceiverSpectrum {
        lines: spectrum,
        orders,
        overall_db,
        overall_dba,
    } = receiver_spectrum(project, &model, rpm, rpm / 120.0 / period as f64, |m| {
        let bin = bins_per_line * m;
        (bin < n_total / 2).then(|| {
            q_spectra
                .iter()
                .map(|s| 2.0 * s[bin] / n_total as f64)
                .collect()
        })
    })?;
    let flange = rec.last_cycles(&rec.flange_p, analysed);
    let backpressure = flange.iter().sum::<f64>() / flange.len() as f64 - p_amb;
    let (turbine0, turbine1) = (m0.turbine_work, m1.turbine_work);

    let (cells, dx_min_mm, dx_max_mm) = grid_summary(&model.network);
    let thermal = match (&project.solver.wall_thermal, &profile) {
        (WallThermal::Computed, Some(pr)) => format!(
            "thermal model walls and initial gas; source {:.1} K, {:.4} kg/s [{}]",
            pre.t0, pre.mass_flow, pr.hash
        ),
        (WallThermal::Fixed { temperature_k }, _) => {
            format!("fixed wall {temperature_k:.1} K; initial gas {t_uniform:.1} K")
        }
        _ => format!("adiabatic walls; initial gas {t_uniform:.1} K from source pre-run"),
    };
    let thermal_hash = blake3::hash(thermal.as_bytes()).to_hex()[..16].to_string();
    Ok(PointResult {
        rpm,
        provenance: Provenance {
            solver: SolverKind::TimeDomain,
            core_version: env!("CARGO_PKG_VERSION").to_string(),
            project_hash: project.hash(),
            dx_target_mm: settings.dx_mm,
            cells,
            dx_min_mm,
            dx_max_mm,
            gas_table: gas.label().to_string(),
            thermal_profile: format!("{thermal} [{thermal_hash}]"),
            evo,
            evo_source,
            time_domain: Some(TimeDomainRun {
                cfl: settings.cfl,
                cfl_max: sim.max_cfl,
                limiter: settings.limiter,
                converged,
                period_cycles: period,
                cycles,
                periodicity_residual: residual,
                periodicity_tolerance: settings.periodicity_tolerance,
            }),
            small_signal: false,
            warnings: model.warnings.clone(),
        },
        overall_db,
        overall_dba,
        spectrum,
        orders,
        backpressure_pa: Some(backpressure),
        mass_flow_kg_s: source_mass / span,
        mass_balance_error: Some(((outlet_mass - source_mass) / source_mass).abs()),
        turbine_power_w: (turbine1 - turbine0) / span,
        outlet_temperature_k: sim.net.gas.t_from_h(outlet_h0),
    })
}

/// Marches one engine cycle, calling `sample` at each of `n` equally spaced instants
/// (before advancing). Each sample interval is split into equal steps no longer than the CFL
/// limit at its start, so samples fall exactly on the crank-angle grid.
fn run_cycle(
    sim: &mut Simulation,
    cycle_time: f64,
    n: usize,
    mut sample: impl FnMut(&Simulation),
) -> Result<()> {
    let ts = cycle_time / n as f64;
    for _ in 0..n {
        sim.evaluate_faces()?;
        sample(sim);
        let sub = (ts / sim.stable_dt()?).ceil().max(1.0);
        let dt = ts / sub;
        for _ in 0..sub as usize {
            sim.step(dt)?;
        }
    }
    Ok(())
}

/// Cumulative counters at a cycle boundary.
struct Marks {
    source: f64,
    outlets: Vec<f64>,
    outlet_energy: Vec<f64>,
    turbine_work: f64,
}

impl Marks {
    fn read(sim: &Simulation, model: &Model) -> Self {
        let src = &sim.nodes[model.source_node];
        Self {
            source: src.mass_to_node[0],
            outlets: model
                .outlets
                .iter()
                .map(|o| sim.nodes[o.node].mass_to_node[0])
                .collect(),
            outlet_energy: model
                .outlets
                .iter()
                .map(|o| sim.nodes[o.node].energy_to_node[0])
                .collect(),
            turbine_work: src.turbine_work,
        }
    }
}

/// Sampled signals, one entry per sample, cycles appended.
struct Recorder {
    n: usize,
    outlet_q: Vec<Vec<f64>>,
    flange_p: Vec<f64>,
}

impl Recorder {
    fn new(outlets: usize, n: usize) -> Self {
        Self {
            n,
            outlet_q: vec![Vec::new(); outlets],
            flange_p: Vec::new(),
        }
    }

    fn sample(&mut self, sim: &Simulation, model: &Model) {
        for (o, out) in model.outlets.iter().enumerate() {
            self.outlet_q[o].push(sim.nodes[out.node].faces[0].v * out.area);
        }
        self.flange_p.push(sim.nodes[model.source_node].faces[0].p);
    }

    fn last_cycles(&self, s: &[f64], cycles: usize) -> Vec<f64> {
        s[s.len() - cycles * self.n..].to_vec()
    }

    /// Largest over monitored signals of RMS(last − cycle `lag` back) / RMS(last − mean(last)).
    fn periodicity_residual(&self, lag: usize) -> f64 {
        let n = self.n;
        let rel = |s: &[f64]| {
            let (prev, last) = (
                &s[s.len() - (lag + 1) * n..s.len() - lag * n],
                &s[s.len() - n..],
            );
            let mean = last.iter().sum::<f64>() / n as f64;
            let diff: f64 = last.iter().zip(prev).map(|(a, b)| (a - b).powi(2)).sum();
            let var: f64 = last.iter().map(|a| (a - mean).powi(2)).sum();
            (diff / var.max(1e-300)).sqrt()
        };
        self.outlet_q
            .iter()
            .map(|s| rel(s))
            .fold(rel(&self.flange_p), f64::max)
    }
}

fn reference_outlet(project: &Project, model: &Model) -> Result<usize> {
    match &project.receiver.outlet {
        None => Ok(0),
        Some(id) => model
            .outlets
            .iter()
            .position(|o| &o.element == id)
            .ok_or_else(|| Error::invalid(format!("receiver.outlet '{id}' is not an outlet"))),
    }
}

/// Thermal-model inputs for a mean engine flow and source temperature.
pub fn thermal_inputs(project: &Project, mass_flow: f64, t_source: f64) -> ThermalInputs {
    let a = &project.ambient;
    ThermalInputs {
        mass_flow,
        t_source,
        p_amb: a.pressure_pa,
        t_amb: a.temperature_k,
        air_speed: a.vehicle_speed_kmh / 3.6 * a.underbody_air_factor,
    }
}

/// Receiver at `distance` from the reference outlet, in the horizontal plane at outlet height,
/// `angle` from the outlet axis toward the outboard side (away from the vehicle centre plane).
pub fn receiver_position(project: &Project, model: &Model, outlet: usize) -> Vec3 {
    let o = &model.outlets[outlet];
    let mut axis = [o.axis[0], o.axis[1], 0.0];
    if axis[0].hypot(axis[1]) < 1e-6 {
        axis = [-1.0, 0.0, 0.0];
    }
    let axis = unit(axis);
    let outboard = if o.position[1] >= 0.0 { 1.0 } else { -1.0 };
    let mut perp = [-axis[1], axis[0], 0.0];
    if perp[1] * outboard < 0.0 {
        perp = scale(perp, -1.0);
    }
    let a = project.receiver.angle_deg.to_radians();
    add(
        o.position,
        scale(
            add(scale(axis, a.cos()), scale(perp, a.sin())),
            project.receiver.distance_m,
        ),
    )
}

/// Mean flow, stagnation temperature and manifold state from the engine source discharging
/// into a short pipe that ends at ambient pressure.
struct PreRun {
    mass_flow: f64,
    t0: f64,
    manifold_p: f64,
    manifold_t: f64,
}

fn prerun(project: &Project, gas: &Gas, rpm: f64, evo: EvoState, dx: f64) -> Result<PreRun> {
    let p_amb = project.ambient.pressure_pa;
    let src_route = project
        .system
        .routes
        .iter()
        .find(|r| {
            project
                .element(&r.from.element)
                .is_some_and(|e| e.kind == crate::project::ElementKind::Source)
        })
        .ok_or_else(|| Error::invalid("no route starts at the source"))?;
    let d = src_route.pipe.id_m();
    let model = build(
        project,
        gas.clone(),
        dx,
        SourceInputs {
            rpm,
            evo,
            p_init: 1.2 * p_amb,
            t_init: 900.0,
        },
    )?;
    let Node::Source(src) = &model.network.nodes[model.source_node] else {
        unreachable!()
    };
    let mut src = src.clone();
    src.port = Port::start(0);
    let net = Network {
        gas: gas.clone(),
        ducts: vec![Duct::conical("pre-run", 0.5, d, d, dx.max(0.02))],
        nodes: vec![
            Node::Source(src),
            Node::Pressure {
                port: Port::end(0),
                p: p_amb,
                t: project.ambient.temperature_k,
            },
        ],
        limiter: project.solver.limiter,
        cfl: project.solver.cfl,
    };
    let t_guess = 900.0;
    let rho = p_amb / (gas.r() * t_guess);
    let mut sim = Simulation::new(net, |_, _| (rho, 0.0, p_amb))?;
    let cycle_time = 120.0 / rpm;
    let cycles = 4;
    let (mut m_start, mut e_start) = (0.0, 0.0);
    let (mut pm, mut tm, mut count) = (0.0, 0.0, 0.0);
    for c in 0..cycles {
        if c == cycles - 1 {
            m_start = sim.nodes[0].mass_to_node[0];
            e_start = sim.nodes[0].energy_to_node[0];
        }
        let Node::Source(src) = &sim.net.nodes[0] else {
            unreachable!()
        };
        let src = src.clone();
        run_cycle(&mut sim, cycle_time, 256, |s| {
            if c == cycles - 1 {
                for j in 0..src.manifolds.len() {
                    let (p, t) = src.manifold_state(&s.net.gas, &s.nodes[0].ode, j);
                    pm += p;
                    tm += t;
                    count += 1.0;
                }
            }
        })?;
    }
    let mass = -(sim.nodes[0].mass_to_node[0] - m_start);
    let energy = -(sim.nodes[0].energy_to_node[0] - e_start);
    if mass.is_nan() || mass <= 0.0 {
        return Err(Error::solver(
            "engine source pre-run produced no net outflow",
        ));
    }
    Ok(PreRun {
        mass_flow: mass / cycle_time,
        t0: gas.t_from_h(energy / mass),
        manifold_p: pm / count,
        manifold_t: tm / count,
    })
}
