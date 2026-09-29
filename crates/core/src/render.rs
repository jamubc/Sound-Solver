//! Hearing the solver directly: the time-domain network is marched through a scene (engine
//! speed and intake pressure against time) while the volume velocity leaving every outlet is
//! recorded at 48 kHz and radiated to the listeners in the time domain
//! (`radiation::listener_pressure`). The waveform is the solver's; nothing is resynthesised.
//!
//! Sequence: settle the network at the scene's first speed and load (`solve::settle`); hold
//! that state for a pre-roll as long as the longest propagation delay, so every listener hears
//! the engine from t = 0; then march the scene, the crank angle integrating the engine speed
//! and each exhaust event taking its EVO state at the intake pressure of its moment. Walls keep
//! the thermal profile of the scene's start: their time constant (minutes) is long against a
//! scene.
//!
//! Every ⅓-octave band is reported resolved or unresolved, never low-passed:
//! - cross-modes: above `1.8412 c √(1 − M²) / (π D)` in a duct (its lowest sound speed and
//!   highest Mach number during the scene, `D` its section) plane waves are not the only
//!   propagating field, which the 1D model assumes;
//! - grid: fewer than 8 cells per wavelength at a duct's lowest sound speed;
//! - radiation: above `k a = 0.5` at an outlet (ambient `k`) the far field is not the
//!   omnidirectional one the point monopole assumes;
//! - refinement: at the scene's fastest moment the outlet flow in the band moves by more than
//!   1 dB when Δx is halved, or the band is above that run's Nyquist frequency.

use std::sync::atomic::{AtomicBool, Ordering};

use rustfft::FftPlanner;
use rustfft::num_complex::Complex64;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::gas1d::{EvoSupply, Limiter, MapSchedule, Node};
use crate::geometry::{Vec3, add, norm, scale, sub, unit};
use crate::math::Trace;
use crate::project::{CabinTf, Project};
use crate::radiation::{Motion, listener_pressure};
use crate::solve::{
    CycleProgress, Settled, grid_summary, load, receiver_position, reference_outlet, settle,
};
use crate::spectrum::fft;

/// Output sample rate, Hz.
pub const SAMPLE_RATE: f64 = 48_000.0;
const R_AIR: f64 = 287.05;
/// First non-planar mode of a circular duct: `J₁′(x) = 0` at x = 1.8412.
const J1P_ROOT: f64 = 1.841_18;
const CELLS_PER_WAVELENGTH: f64 = 8.0;
const KA_MONOPOLE: f64 = 0.5;
const REFINEMENT_DB: f64 = 1.0;

/// What the render plays: engine speed and load against time, and who hears it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Scene {
    /// Length of the scene, s.
    pub duration_s: f64,
    /// Engine speed against time, `[[t_s, rpm], …]`, linear between points, held outside.
    pub rpm: Vec<[f64; 2]>,
    /// Intake manifold pressure against time, `[[t_s, kPa], …]` (a throttle trace); absent:
    /// the project's load line `operating.map_kpa` at the engine speed of the moment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_kpa: Option<Vec<[f64; 2]>>,
    pub listener: Listener,
}

/// Who hears the scene. Positions are in the vehicle frame (X forward, Y left, Z up, origin at
/// the downpipe flange), mm.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Listener {
    /// The project's receiver: one channel.
    Receiver,
    /// A head at the receiver facing the reference outlet, ears `ear_spacing_mm` apart on the
    /// horizontal line across that direction: two channels, left ear first. Each ear is a
    /// free-field point; the head's shadow is not modelled.
    Stereo { ear_spacing_mm: f64 },
    /// Microphones at rest beside the vehicle: one channel each.
    Points { positions_mm: Vec<Vec3> },
    /// The vehicle drives along its +x axis past a microphone at rest. `mic_mm` is in the
    /// frame the vehicle frame coincides with after travelling 0; the vehicle frame starts at
    /// `start_mm` along x and moves at `speed_kmh` against time, `[[t_s, km/h], …]`.
    PassBy {
        mic_mm: Vec3,
        start_mm: f64,
        speed_kmh: Vec<[f64; 2]>,
    },
    /// The driver's ear: the receiver's pressure through the measured cabin transfer function.
    Cabin,
}

/// A render in progress.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, JsonSchema)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum RenderProgress {
    /// Settling the network at the scene's start.
    Settle { cycle: u32, residual: Option<f64> },
    /// Marching the scene: fraction of the recorded time done.
    March { fraction: f64 },
}

/// Receiver pressure of every listener channel, Pa, and how it was made.
#[derive(Clone, Debug)]
pub struct Render {
    pub channels: Vec<Vec<f32>>,
    pub info: RenderInfo,
}

#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct RenderInfo {
    pub core_version: String,
    pub project_hash: String,
    pub scene: Scene,
    pub sample_rate: f64,
    /// Name of each channel.
    pub channels: Vec<String>,
    pub peak_pa: f64,
    /// Equivalent continuous level of each channel, dB re 20 µPa.
    pub leq_db: Vec<f64>,
    pub dx_mm: f64,
    pub cells: usize,
    pub cfl: f64,
    /// Largest Courant number of any step of the scene.
    pub cfl_max: f64,
    pub limiter: Limiter,
    pub gas_table: String,
    /// Wall and initial gas temperatures.
    pub thermal: String,
    /// Engine source model and where its EVO states come from.
    pub source: String,
    /// The periodic state the scene starts from.
    pub start: StartState,
    /// Engine held at the start state before t = 0 so every listener hears it from t = 0, s.
    pub pre_roll_s: f64,
    /// Where the grid-refinement check ran.
    pub refinement: Refinement,
    pub bands: Vec<Band>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct StartState {
    pub rpm: f64,
    pub map_kpa: f64,
    pub cycles: u32,
    pub periodicity_residual: f64,
    pub converged: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct Refinement {
    pub rpm: f64,
    pub map_kpa: f64,
    pub dx_mm: f64,
    pub fine_dx_mm: f64,
}

/// One ⅓-octave band (IEC 61260 base-ten centres) and whether the model resolves it.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct Band {
    pub center_hz: f64,
    pub lower_hz: f64,
    pub upper_hz: f64,
    pub resolved: bool,
    /// Why it is not resolved; empty when it is.
    pub reasons: Vec<String>,
}

/// Marches `project` through `scene`, reporting to `on_progress`, which stops the render by
/// returning `false`.
pub fn render(
    project: &Project,
    scene: &Scene,
    on_progress: &mut dyn FnMut(RenderProgress) -> bool,
) -> Result<Render> {
    let fs = SAMPLE_RATE;
    if !(scene.duration_s.is_finite() && scene.duration_s > 0.0) {
        return Err(Error::invalid("scene.duration_s must be positive"));
    }
    let speed = trace("scene.rpm", &scene.rpm, 1.0)?;
    let map = match &scene.map_kpa {
        Some(m) => Some(trace("scene.map_kpa", m, 1.0)?),
        None => None,
    };
    let map_at = |t: f64| match &map {
        Some(m) => m.at(t),
        None => project.operating.map_at(speed.at(t)),
    };
    let cabin = match &scene.listener {
        Listener::Cabin => Some(project.measurements.cabin_tf.as_ref().ok_or_else(|| {
            Error::invalid("the cabin listener needs a measured cabin transfer function")
        })?),
        _ => None,
    };
    let a = &project.ambient;
    let (rho0, c0) = (
        a.pressure_pa / (R_AIR * a.temperature_k),
        (1.4 * R_AIR * a.temperature_k).sqrt(),
    );
    let dx = project.solver.dx_mm * 1e-3;
    let (rpm0, map0) = (speed.at(0.0), map_at(0.0));

    // The fastest moment, for the refinement check.
    let t_fast = speed
        .knots()
        .iter()
        .map(|k| k[0].clamp(0.0, scene.duration_s))
        .chain([0.0, scene.duration_s])
        .fold(0.0, |best, t| {
            if speed.at(t) > speed.at(best) {
                t
            } else {
                best
            }
        });
    let (rpm_f, map_f) = (speed.at(t_fast), map_at(t_fast));
    // The refinement check runs beside the scene; a steady scene shares its Δx run.
    let same = rpm_f == rpm0 && map_f == map0;
    let stop = AtomicBool::new(false);
    let (played, checked) = std::thread::scope(|s| {
        let check = s.spawn(|| -> Result<(Option<Levels>, Levels)> {
            let mut go = |_: CycleProgress| !stop.load(Ordering::Relaxed);
            let fine = band_levels(&settle(project, rpm_f, map_f, 0.5 * dx, &mut go)?, rpm_f);
            let coarse = match same {
                true => None,
                false => Some(band_levels(
                    &settle(project, rpm_f, map_f, dx, &mut go)?,
                    rpm_f,
                )),
            };
            Ok((coarse, fine))
        });
        let played = play(project, scene, &speed, map.as_ref(), dx, c0, on_progress);
        if played.is_err() {
            stop.store(true, Ordering::Relaxed);
        }
        (
            played,
            check.join().expect("the refinement thread does not panic"),
        )
    });
    let Played {
        st,
        start,
        listeners,
        motion,
        sources,
        n_pre,
        q,
        c_min,
        mach_max,
    } = played?;
    let refined = checked
        .map(|(coarse, fine)| (coarse.unwrap_or_else(|| band_levels(&st, rpm0)), fine))
        .map_err(|e| e.to_string());
    let ground_z = a.ground_z_mm * 1e-3;
    let n_out = (scene.duration_s * fs).round() as usize;
    let t_q0 = -(n_pre as f64) / fs;
    let mut channels: Vec<Vec<f64>> = listeners
        .iter()
        .map(|(_, l)| {
            listener_pressure(
                fs, n_out, rho0, c0, &sources, &q, t_q0, &motion, *l, ground_z,
            )
        })
        .collect();
    if let Some(tf) = cabin {
        channels = channels.iter().map(|x| through_cabin(x, fs, tf)).collect();
    }

    let peak_pa = channels
        .iter()
        .flatten()
        .fold(0.0, |m: f64, s| m.max(s.abs()));
    let leq_db = channels
        .iter()
        .map(|x| {
            let ms = x.iter().map(|s| s * s).sum::<f64>() / x.len().max(1) as f64;
            10.0 * (ms.max(1e-300) / 4e-10).log10()
        })
        .collect();
    let bands = band_status(&st, &c_min, &mach_max, c0, &refined, rpm_f);
    let (cells, _, _) = grid_summary(&st.model.network);
    let mut warnings = st.model.warnings.clone();
    warnings.push("walls hold the thermal profile of the scene's start".into());
    if matches!(scene.listener, Listener::PassBy { .. }) {
        warnings.push(format!(
            "air flow round the moving vehicle is not modelled; the thermal model takes \
             ambient.vehicle_speed_kmh = {}",
            a.vehicle_speed_kmh
        ));
    }
    if let Some(tf) = cabin {
        let (lo, hi) = (tf.gain_db[0][0], tf.gain_db[tf.gain_db.len() - 1][0]);
        warnings.push(format!(
            "cabin: measured magnitude applied with zero phase over {lo:.0}–{hi:.0} Hz; \
             nothing outside the measured range reaches the driver ({})",
            tf.source
        ));
    }
    let info = RenderInfo {
        core_version: env!("CARGO_PKG_VERSION").to_string(),
        project_hash: project.hash(),
        scene: scene.clone(),
        sample_rate: fs,
        channels: listeners.into_iter().map(|(n, _)| n).collect(),
        peak_pa,
        leq_db,
        dx_mm: project.solver.dx_mm,
        cells,
        cfl: project.solver.cfl,
        cfl_max: st.sim.max_cfl,
        limiter: project.solver.limiter,
        gas_table: st.gas.label().to_string(),
        thermal: st.thermal.clone(),
        source: if project.operating.evo_override.is_some() {
            "single-zone exhaust-valve source from the given EVO state; every cycle alike".into()
        } else {
            "single-zone exhaust-valve source; each event's EVO state from the ideal \
             Otto-cycle estimate at the intake pressure of its moment; cylinders and cycles \
             otherwise alike"
                .into()
        },
        start,
        pre_roll_s: n_pre as f64 / fs,
        refinement: Refinement {
            rpm: rpm_f,
            map_kpa: map_f,
            dx_mm: project.solver.dx_mm,
            fine_dx_mm: 0.5 * project.solver.dx_mm,
        },
        bands,
        warnings,
    };
    Ok(Render {
        channels: channels
            .into_iter()
            .map(|x| x.into_iter().map(|s| s as f32).collect())
            .collect(),
        info,
    })
}

/// Band levels of each outlet (`band_levels`).
type Levels = Vec<Vec<Option<f64>>>;

/// The scene marched: the settled start, the listeners and the recorded outlet flow.
struct Played {
    st: Settled,
    start: StartState,
    listeners: Vec<(String, Vec3)>,
    motion: Motion,
    sources: Vec<Vec3>,
    n_pre: usize,
    q: Vec<Vec<f64>>,
    c_min: Vec<f64>,
    mach_max: Vec<f64>,
}

/// Settles at the scene's start, holds it for the pre-roll, then marches the scene.
fn play(
    project: &Project,
    scene: &Scene,
    speed: &Trace,
    map: Option<&Trace>,
    dx: f64,
    c0: f64,
    on_progress: &mut dyn FnMut(RenderProgress) -> bool,
) -> Result<Played> {
    let fs = SAMPLE_RATE;
    let a = &project.ambient;
    let rpm0 = speed.at(0.0);
    let map0 = map.map_or_else(|| project.operating.map_at(rpm0), |m| m.at(0.0));
    let mut st = settle(project, rpm0, map0, dx, &mut |p| {
        on_progress(RenderProgress::Settle {
            cycle: p.cycle,
            residual: p.residual,
        })
    })?;
    let start = StartState {
        rpm: rpm0,
        map_kpa: map0,
        cycles: st.cycles,
        periodicity_residual: st.residual,
        converged: st.residual <= project.solver.periodicity_tolerance,
    };

    // Listeners and the vehicle's motion.
    let receiver = || -> Result<Vec3> {
        let r = reference_outlet(project, &st.model)?;
        Ok(receiver_position(project, &st.model, r))
    };
    let (listeners, motion): (Vec<(String, Vec3)>, Motion) = match &scene.listener {
        Listener::Receiver => (vec![("receiver".into(), receiver()?)], Motion::at_rest()),
        Listener::Cabin => (
            vec![("driver's ear".into(), receiver()?)],
            Motion::at_rest(),
        ),
        Listener::Stereo { ear_spacing_mm } => {
            if !(ear_spacing_mm.is_finite() && *ear_spacing_mm > 0.0) {
                return Err(Error::invalid("the ear spacing must be positive"));
            }
            let r = reference_outlet(project, &st.model)?;
            let head = receiver_position(project, &st.model, r);
            let o = st.model.outlets[r].position;
            let facing = unit([o[0] - head[0], o[1] - head[1], 0.0]);
            let left = scale([-facing[1], facing[0], 0.0], 0.5 * ear_spacing_mm * 1e-3);
            (
                vec![
                    ("left ear".into(), add(head, left)),
                    ("right ear".into(), sub(head, left)),
                ],
                Motion::at_rest(),
            )
        }
        Listener::Points { positions_mm } => {
            if positions_mm.is_empty() {
                return Err(Error::invalid("the listener needs at least one position"));
            }
            let names = (1..=positions_mm.len()).map(|i| format!("mic {i}"));
            let points = positions_mm.iter().map(|p| scale(*p, 1e-3));
            (names.zip(points).collect(), Motion::at_rest())
        }
        Listener::PassBy {
            mic_mm,
            start_mm,
            speed_kmh,
        } => {
            let v = trace("listener.speed_kmh", speed_kmh, 0.0)?;
            let knots = v.knots().iter().map(|k| [k[0], k[1] / 3.6]).collect();
            (
                vec![("pass-by mic".into(), scale(*mic_mm, 1e-3))],
                Motion {
                    x0: start_mm * 1e-3,
                    speed: Trace::new(knots).expect("scaled from a valid trace"),
                },
            )
        }
    };
    let v_max = motion
        .speed
        .knots()
        .iter()
        .map(|k| k[1])
        .fold(0.0, f64::max);
    if v_max >= 0.5 * c0 {
        return Err(Error::invalid(
            "the vehicle must stay below half the speed of sound",
        ));
    }
    let sources: Vec<Vec3> = st.model.outlets.iter().map(|o| o.position).collect();
    let ground_z = a.ground_z_mm * 1e-3;
    let r_max = listeners
        .iter()
        .flat_map(|(_, l)| {
            sources.iter().flat_map(move |s| {
                [s[2], 2.0 * ground_z - s[2]].map(|z| norm(sub(*l, [s[0] + motion.x0, s[1], z])))
            })
        })
        .fold(0.0, f64::max);
    let n_pre = ((r_max / (c0 - v_max) + 0.005) * fs).ceil() as usize;

    // From here on the source follows the scene; scene time 0 is sim time `t0`.
    let t0 = st.sim.t + n_pre as f64 / fs;
    let shifted = |tr: &Trace| {
        Trace::new(tr.knots().iter().map(|k| [k[0] + t0, k[1]]).collect())
            .expect("shifted from a valid trace")
    };
    let given = project.operating.evo_override.is_some();
    if let Node::Source(src) = &mut st.sim.net.nodes[st.model.source_node] {
        src.speed = shifted(speed);
        if !given {
            src.evo = EvoSupply::Estimated {
                load: load(project, map0),
                map_kpa: match map {
                    Some(m) => MapSchedule::OfTime(shifted(m)),
                    None => MapSchedule::OfSpeed(
                        Trace::new(project.operating.map_kpa.to_vec())
                            .expect("validated load line"),
                    ),
                },
                fuel_h_to_c: project.gas.fuel_h_to_c,
                lambda: project.gas.lambda,
            };
        }
    }

    let (q, c_min, mach_max) = march(&mut st, fs, n_pre, scene.duration_s, on_progress)?;
    Ok(Played {
        st,
        start,
        listeners,
        motion,
        sources,
        n_pre,
        q,
        c_min,
        mach_max,
    })
}

/// A validated trace whose values are at least `min`.
fn trace(what: &str, knots: &[[f64; 2]], min: f64) -> Result<Trace> {
    Trace::new(knots.to_vec())
        .filter(|t| t.knots().iter().all(|k| k[1] >= min))
        .ok_or_else(|| {
            Error::invalid(format!(
                "{what} needs ≥ 1 point, times increasing, values ≥ {min}"
            ))
        })
}

/// Outlet volume velocities at `fs` over the pre-roll and the scene, and each duct's lowest
/// sound speed and highest Mach number, sampled every millisecond.
#[allow(clippy::type_complexity)]
fn march(
    st: &mut Settled,
    fs: f64,
    n_pre: usize,
    duration: f64,
    on_progress: &mut dyn FnMut(RenderProgress) -> bool,
) -> Result<(Vec<Vec<f64>>, Vec<f64>, Vec<f64>)> {
    let n = n_pre + (duration * fs).ceil() as usize + 1;
    let outlets = st.model.outlets.clone();
    let nd = st.sim.net.ducts.len();
    let (mut c_min, mut mach_max) = (vec![f64::INFINITY; nd], vec![0.0f64; nd]);
    let mut q = vec![Vec::with_capacity(n); outlets.len()];
    let every = (fs / 1000.0) as usize;
    let ts = 1.0 / fs;
    for j in 0..n {
        st.sim.evaluate_faces()?;
        for (qo, out) in q.iter_mut().zip(&outlets) {
            qo.push(st.sim.nodes[out.node].faces[0].v * out.area);
        }
        if j % every == 0 {
            for d in 0..nd {
                for i in 0..st.sim.net.ducts[d].n() {
                    let p = st.sim.prim(d, i);
                    c_min[d] = c_min[d].min(p.c);
                    mach_max[d] = mach_max[d].max(p.u.abs() / p.c);
                }
            }
        }
        if j % (100 * every) == 0
            && !on_progress(RenderProgress::March {
                fraction: j as f64 / n as f64,
            })
        {
            return Err(Error::solver("stopped"));
        }
        if j + 1 < n {
            let sub = (ts / st.sim.stable_dt()?).ceil().max(1.0);
            for _ in 0..sub as usize {
                st.sim.step(ts / sub)?;
            }
        }
    }
    Ok((q, c_min, mach_max))
}

/// ⅓-octave bands 10 Hz – 20 kHz: (centre, lower, upper), Hz.
fn third_octaves() -> Vec<(f64, f64, f64)> {
    (-20..=13)
        .map(|k| {
            let c = 1000.0 * 10f64.powf(k as f64 / 10.0);
            (c, c * 10f64.powf(-0.05), c * 10f64.powf(0.05))
        })
        .collect()
}

/// Level of each outlet's volume velocity in each ⅓-octave band over the settled state's last
/// four cycles, dB re an arbitrary common reference; `None` above the run's Nyquist frequency.
fn band_levels(s: &Settled, rpm: f64) -> Vec<Vec<Option<f64>>> {
    let analysed = 4.min(s.cycles as usize);
    let span = analysed as f64 * 120.0 / rpm;
    s.rec
        .outlet_q
        .iter()
        .map(|qo| {
            let x = s.rec.last_cycles(qo, analysed);
            let spec = fft(&x);
            let nyquist = 0.5 * x.len() as f64 / span;
            third_octaves()
                .iter()
                .map(|&(_, lo, hi)| {
                    (hi <= nyquist).then(|| {
                        let e: f64 = (1..x.len() / 2)
                            .filter(|&k| (lo..hi).contains(&(k as f64 / span)))
                            .map(|k| spec[k].norm_sqr())
                            .sum();
                        10.0 * e.max(1e-300).log10()
                    })
                })
                .collect()
        })
        .collect()
}

fn band_status(
    st: &Settled,
    c_min: &[f64],
    mach_max: &[f64],
    c0: f64,
    refined: &std::result::Result<(Levels, Levels), String>,
    rpm_f: f64,
) -> Vec<Band> {
    let ducts = &st.sim.net.ducts;
    let lowest = |f: &dyn Fn(usize) -> f64| {
        (0..ducts.len())
            .map(|d| (f(d), d))
            .fold((f64::INFINITY, 0), |a, b| if b.0 < a.0 { b } else { a })
    };
    let (f_mode, d_mode) = lowest(&|d| {
        J1P_ROOT * c_min[d] * (1.0 - mach_max[d].powi(2)).max(0.0).sqrt()
            / (std::f64::consts::PI * ducts[d].section_diameter)
    });
    let (f_grid, d_grid) = lowest(&|d| c_min[d] / (CELLS_PER_WAVELENGTH * ducts[d].dx));
    let (f_rad, o_rad) = st
        .model
        .outlets
        .iter()
        .map(|o| {
            let a = (o.area / std::f64::consts::PI).sqrt();
            KA_MONOPOLE * c0 / (2.0 * std::f64::consts::PI * a)
        })
        .enumerate()
        .fold(
            (f64::INFINITY, 0),
            |a, (i, f)| if f < a.0 { (f, i) } else { a },
        );
    third_octaves()
        .into_iter()
        .enumerate()
        .map(|(b, (center_hz, lower_hz, upper_hz))| {
            let mut reasons = Vec::new();
            if upper_hz > f_mode {
                reasons.push(format!(
                    "cross-modes cut on above {f_mode:.0} Hz in '{}' (⌀{:.0} mm, c ≥ {:.0} m/s)",
                    ducts[d_mode].label,
                    ducts[d_mode].section_diameter * 1e3,
                    c_min[d_mode]
                ));
            }
            if upper_hz > f_grid {
                reasons.push(format!(
                    "under 8 cells per wavelength above {f_grid:.0} Hz in '{}' (Δx {:.1} mm)",
                    ducts[d_grid].label,
                    ducts[d_grid].dx * 1e3
                ));
            }
            if upper_hz > f_rad {
                reasons.push(format!(
                    "outlet '{}' beyond ka = {KA_MONOPOLE} above {f_rad:.0} Hz: directivity not \
                     modelled",
                    st.model.outlets[o_rad].element
                ));
            }
            match refined {
                Err(e) => reasons.push(format!("grid-refinement check did not run: {e}")),
                Ok((coarse, fine)) => {
                    let shift = coarse
                        .iter()
                        .zip(fine)
                        .map(|(c, f)| match (c[b], f[b]) {
                            (Some(c), Some(f)) => Some((f - c).abs()),
                            _ => None,
                        })
                        .try_fold(0.0f64, |m, d| d.map(|d| m.max(d)));
                    match shift {
                        None => reasons.push(format!(
                            "above the refinement run's Nyquist frequency at {rpm_f:.0} rpm"
                        )),
                        Some(d) if d > REFINEMENT_DB => reasons.push(format!(
                            "not grid-converged: outlet flow moves {d:.1} dB when Δx is halved"
                        )),
                        _ => {}
                    }
                }
            }
            Band {
                center_hz,
                lower_hz,
                upper_hz,
                resolved: reasons.is_empty(),
                reasons,
            }
        })
        .collect()
}

/// `x` through the measured cabin gain, zero phase (the measurement has no phase), nothing
/// outside the measured range.
fn through_cabin(x: &[f64], fs: f64, tf: &CabinTf) -> Vec<f64> {
    let pad = (0.5 * fs) as usize;
    let n = (x.len() + 2 * pad).next_power_of_two();
    let mut buf = vec![Complex64::new(0.0, 0.0); n];
    for (b, &v) in buf[pad..].iter_mut().zip(x) {
        *b = Complex64::new(v, 0.0);
    }
    let mut planner = FftPlanner::new();
    planner.plan_fft_forward(n).process(&mut buf);
    for (k, b) in buf.iter_mut().enumerate() {
        let f = k.min(n - k) as f64 * fs / n as f64;
        *b *= tf.at(f).map_or(0.0, |g| 10f64.powf(g / 20.0));
    }
    planner.plan_fft_inverse(n).process(&mut buf);
    buf[pad..pad + x.len()]
        .iter()
        .map(|c| c.re / n as f64)
        .collect()
}
