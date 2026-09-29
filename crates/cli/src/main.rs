//! `exhaustctl`: headless front end to `exhaust-core`. Anything the app solves, this solves
//! with identical output.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use exhaust_core::audio;
use exhaust_core::layout::layout;
use exhaust_core::measure::{self, OrderTracks, Recording, RpmLog};
use exhaust_core::project::{CabinTf, CabinTfMethod, Project, RecordingRef, sweep_points};
use exhaust_core::render::{RenderProgress, Scene, render};
use exhaust_core::scan::{self, Mesh};
use exhaust_core::solve::{Line, PointOutcome, SolverKind, sweep};
use exhaust_core::tune::tune;
use exhaust_core::validation::{PENDING, cases};
use exhaust_core::{fabricate, manifest, metrics, solid};

#[derive(Clone, Copy, ValueEnum)]
enum Solver {
    /// Nonlinear time-domain gas dynamics (the reference result).
    TimeDomain,
    /// Linear four-pole preview.
    FourPole,
}

#[derive(Clone, Copy, ValueEnum)]
enum Export {
    /// Route centrelines and element envelopes, as the app draws them (JSON).
    Layout,
    /// Element manifests: parameters with units and allowed ranges (JSON).
    Manifests,
    /// Straights, bends, stock sticks and joints, as the app lists them (JSON).
    Fabrication,
    /// Straights and bends with tube, stock stick and neighbours (CSV).
    CutList,
    /// Feed, rotation, angle and radius of every bend (CSV).
    Bends,
    /// Numbered joints with type and filler notes (Markdown).
    Welds,
    /// Pipe clearance to the project's underbody scan (Markdown).
    Clearance,
    /// Pipe runs as B-rep solids (STEP).
    Step,
    /// Pipe runs as a mesh (binary STL).
    Stl,
    /// All of the fabrication files, written beside the project file or into `--out`.
    Package,
}

#[derive(Parser)]
#[command(
    name = "exhaustctl",
    version,
    about = "Exhaust gas-dynamics and acoustics solver"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Solve a project over an engine-speed sweep; prints JSON.
    Solve {
        /// Project file (JSON).
        project: PathBuf,
        /// Engine speeds as `start:stop:step` rpm; defaults to the project's sweep.
        #[arg(long)]
        sweep: Option<String>,
        #[arg(long, value_enum, default_value = "time-domain")]
        solver: Solver,
        /// Write the result here instead of standard output.
        #[arg(long, short)]
        out: Option<PathBuf>,
    },
    /// Export data derived from a project.
    Export {
        #[arg(value_enum)]
        what: Export,
        /// Project file (JSON); not needed for manifests.
        project: Option<PathBuf>,
        /// Write here instead of standard output (a directory for `package`).
        #[arg(long, short)]
        out: Option<PathBuf>,
    },
    /// Size a stub's length or a Helmholtz resonator's volume to put its resonance on a
    /// drone, with the thermal band; prints JSON.
    Tune {
        /// Project file (JSON).
        project: PathBuf,
        /// The stub or Helmholtz resonator element.
        #[arg(long)]
        element: String,
        /// Engine speed of the drone: the branch gas takes its thermal state.
        #[arg(long)]
        rpm: f64,
        /// Target, Hz; defaults to the firing order's frequency at `--rpm`.
        #[arg(long)]
        frequency: Option<f64>,
        /// Measured branch gas temperature as `lowest:highest`, K, instead of the thermal
        /// model's.
        #[arg(long)]
        branch_temperature: Option<String>,
    },
    /// Engine-order tracks of one of the project's recordings (JSON).
    Tracks {
        /// Project file (JSON).
        project: PathBuf,
        /// Index into `measurements.recordings`.
        #[arg(long, default_value_t = 0)]
        recording: usize,
    },
    /// Measure the cabin transfer function; prints it as `measurements.cabin_tf` (JSON).
    CabinTf {
        /// Project file (JSON).
        project: PathBuf,
        /// Order ratio: exterior and interior recordings of the same drive, as indices into
        /// `measurements.recordings`.
        #[arg(long, num_args = 2, value_names = ["EXTERIOR", "INTERIOR"])]
        orders: Option<Vec<usize>>,
        /// Impulse: recordings (WAV or CAF) of the same impulses at the exterior receiver and
        /// at the driver's ear.
        #[arg(long, num_args = 2, value_names = ["EXTERIOR", "INTERIOR"], conflicts_with = "orders")]
        impulse: Option<Vec<PathBuf>>,
    },
    /// Engine orders of two solved sweeps (`solve` output), the second against the first (JSON).
    Compare {
        /// Project file (JSON) whose cruise band applies.
        project: PathBuf,
        a: PathBuf,
        b: PathBuf,
    },
    /// Write the predicted sound at the receiver to a WAV: a steady engine speed, or a run-up.
    Listen {
        /// Project file (JSON).
        project: PathBuf,
        /// A steady engine speed, rpm.
        #[arg(long, conflicts_with_all = ["from", "to"])]
        rpm: Option<f64>,
        /// Run-up from this speed (default: the project's sweep start), rpm.
        #[arg(long)]
        from: Option<f64>,
        /// Run-up to this speed (default: the project's sweep end), rpm.
        #[arg(long)]
        to: Option<f64>,
        /// Length, s (a steady sound is at least this long, in whole engine cycles).
        #[arg(long, default_value_t = 10.0)]
        seconds: f64,
        #[arg(long, value_enum, default_value = "time-domain")]
        solver: Solver,
        /// Through the project's measured cabin transfer function.
        #[arg(long)]
        interior: bool,
        /// WAV file to write (16-bit, peak at 0.9 of full scale).
        #[arg(long, short)]
        out: PathBuf,
    },
    /// Render a scene by marching the time-domain solver: a WAV of every listener channel
    /// (16-bit, peak at 0.9 of full scale) and the render's provenance as JSON.
    Render {
        /// Project file (JSON).
        project: PathBuf,
        /// Scene file (JSON): engine speed and intake pressure against time, and the listener.
        #[arg(long)]
        scene: PathBuf,
        /// WAV file to write.
        #[arg(long, short)]
        out: PathBuf,
        /// Write the provenance here instead of standard output.
        #[arg(long)]
        info: Option<PathBuf>,
    },
    /// Check a steady hold of one of the project's calibrated recordings against a render of
    /// the same engine speed where it was made, band by band (JSON).
    Hold {
        /// Project file (JSON).
        project: PathBuf,
        /// Index into `measurements.recordings`.
        #[arg(long, default_value_t = 0)]
        recording: usize,
        /// Start of the hold, s from the recording's start.
        #[arg(long)]
        from: f64,
        /// End of the hold, s.
        #[arg(long)]
        to: f64,
        /// A recording of the background, same phone at the same gain.
        #[arg(long)]
        background: Option<PathBuf>,
    },
    /// Sweep every estimated or derived input across its range at one engine speed: the change
    /// in each receiver ⅓-octave band per input, ranked, and the combined band (JSON).
    Sensitivity {
        /// Project file (JSON).
        project: PathBuf,
        /// Engine speed, rpm.
        #[arg(long)]
        rpm: f64,
        /// Write the result here instead of standard output.
        #[arg(long, short)]
        out: Option<PathBuf>,
    },
    /// Run the analytic validation suite; exits non-zero if any check fails.
    Validate {
        /// Print the checks as JSON instead of a table.
        #[arg(long)]
        json: bool,
        /// Run only these cases (default: all).
        #[arg(long = "case")]
        cases: Vec<String>,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("exhaustctl: {e}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode, String> {
    match cli.command {
        Command::Solve {
            project,
            sweep: rpm_sweep,
            solver,
            out,
        } => {
            let project = read_project(&project)?;
            let rpms = match rpm_sweep {
                Some(s) => parse_sweep(&s)?,
                None => project.operating.sweep(),
            };
            let kind = match solver {
                Solver::TimeDomain => SolverKind::TimeDomain,
                Solver::FourPole => SolverKind::FourPole,
            };
            let result = sweep(&project, &rpms, kind);
            emit(&result, out)?;
            let failed = result
                .points
                .iter()
                .filter(|p| matches!(p, exhaust_core::solve::PointOutcome::Failed { .. }))
                .count();
            if failed > 0 {
                eprintln!(
                    "exhaustctl: {failed} of {} operating points failed; see their `error` fields",
                    rpms.len()
                );
                return Ok(ExitCode::FAILURE);
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Export { what, project, out } => {
            if let Export::Manifests = what {
                emit(manifest::all(), out)?;
                return Ok(ExitCode::SUCCESS);
            }
            let path = project.ok_or("this export needs a project file")?;
            let project = read_project(&path)?;
            let text = |s: String| write(out.as_deref(), s.as_bytes());
            let err = |e: exhaust_core::Error| e.to_string();
            match what {
                Export::Manifests => unreachable!(),
                Export::Layout => emit(&layout(&project).map_err(err)?, out)?,
                Export::Fabrication => emit(&fabricate::check(&project).map_err(err)?, out)?,
                Export::CutList => text(fabricate::cut_list_csv(
                    &fabricate::check(&project).map_err(err)?,
                ))?,
                Export::Bends => text(fabricate::bend_schedule_csv(
                    &fabricate::check(&project).map_err(err)?,
                ))?,
                Export::Welds => {
                    let package = fabricate::check(&project).map_err(err)?;
                    text(fabricate::weld_map_markdown(&project, &package))?
                }
                Export::Clearance => {
                    let mesh = read_scan(&path, &project)?.ok_or("the project has no scan")?;
                    let report = scan::clearance(&project, &mesh).map_err(err)?;
                    text(scan::report_markdown(&project, &report))?
                }
                Export::Step => text(solid::step(&project).map_err(err)?)?,
                Export::Stl => write(out.as_deref(), &solid::stl(&project).map_err(err)?)?,
                Export::Package => {
                    let dir = match out {
                        Some(dir) => dir,
                        None => path.parent().map(Path::to_path_buf).unwrap_or_default(),
                    };
                    for file in package(&path, &project)? {
                        let target = dir.join(&file.0);
                        write(Some(&target), &file.1)?;
                        eprintln!("wrote {}", target.display());
                    }
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Tune {
            project,
            element,
            rpm,
            frequency,
            branch_temperature,
        } => {
            let project = read_project(&project)?;
            let firing = project.engine.geometry.firing_order.len() as f64 / 2.0 * rpm / 60.0;
            let stated = match branch_temperature {
                Some(s) => match s.split_once(':').map(|(a, b)| (a.parse(), b.parse())) {
                    Some((Ok(lo), Ok(hi))) if lo > 0.0 && hi >= lo => Some([lo, hi]),
                    _ => {
                        return Err(format!(
                            "--branch-temperature takes lowest:highest K, got '{s}'"
                        ));
                    }
                },
                None => None,
            };
            let tuning = tune(&project, &element, frequency.unwrap_or(firing), rpm, stated)
                .map_err(|e| e.to_string())?;
            emit(&tuning, None)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Tracks {
            project: path,
            recording,
        } => {
            let project = read_project(&path)?;
            let spec = project
                .measurements
                .recordings
                .get(recording)
                .ok_or_else(|| format!("the project has no recording {recording}"))?;
            emit(&tracks(&path, &project, spec)?, None)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::CabinTf {
            project: path,
            orders,
            impulse,
        } => {
            let project = read_project(&path)?;
            let tf = match (orders, impulse) {
                (Some(pair), _) => {
                    let spec = |i: usize| {
                        project
                            .measurements
                            .recordings
                            .get(i)
                            .ok_or_else(|| format!("the project has no recording {i}"))
                    };
                    let (ext, int) = (spec(pair[0])?, spec(pair[1])?);
                    let gain_db = measure::cabin_tf_orders(
                        &tracks(&path, &project, ext)?,
                        &tracks(&path, &project, int)?,
                    )
                    .map_err(|e| e.to_string())?;
                    CabinTf {
                        method: CabinTfMethod::OrderRatio,
                        source: format!(
                            "order ratio: {} (exterior), {} (interior)",
                            file_name(Path::new(&ext.path)),
                            file_name(Path::new(&int.path))
                        ),
                        gain_db,
                    }
                }
                (None, Some(files)) => {
                    let rec = |file: &Path| -> Result<Recording, String> {
                        let bytes =
                            std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
                        Recording::parse(&file.to_string_lossy(), &bytes)
                            .map_err(|e| format!("{}: {e}", file.display()))
                    };
                    CabinTf {
                        method: CabinTfMethod::Impulse,
                        source: format!(
                            "impulse: {} (exterior), {} (interior)",
                            file_name(&files[0]),
                            file_name(&files[1])
                        ),
                        gain_db: measure::cabin_tf_impulse(&rec(&files[0])?, &rec(&files[1])?)
                            .map_err(|e| e.to_string())?,
                    }
                }
                (None, None) => return Err("cabin-tf needs --orders or --impulse".into()),
            };
            emit(&tf, None)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Compare { project, a, b } => {
            let project = read_project(&project)?;
            let points = |file: &Path| -> Result<Vec<PointOutcome>, String> {
                let text = std::fs::read_to_string(file)
                    .map_err(|e| format!("{}: {e}", file.display()))?;
                let mut sweep: serde_json::Value =
                    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))?;
                serde_json::from_value(sweep["points"].take())
                    .map_err(|e| format!("{}: {e}", file.display()))
            };
            emit(
                &metrics::compare(&project, &points(&a)?, &points(&b)?),
                None,
            )?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Listen {
            project,
            rpm,
            from,
            to,
            seconds,
            solver,
            interior,
            out,
        } => {
            let project = read_project(&project)?;
            let [start, stop, step] = project.operating.sweep_rpm;
            let (lo, hi) = match rpm {
                Some(r) => (r, r),
                None => (from.unwrap_or(start), to.unwrap_or(stop)),
            };
            // The solved speeds the sound needs: the steady one, or the sweep across the run-up
            // and one step beyond each end.
            let rpms: Vec<f64> = match rpm {
                Some(r) => vec![r],
                None => project
                    .operating
                    .sweep()
                    .into_iter()
                    .filter(|r| *r >= lo.min(hi) - step && *r <= lo.max(hi) + step)
                    .collect(),
            };
            let kind = match solver {
                Solver::TimeDomain => SolverKind::TimeDomain,
                Solver::FourPole => SolverKind::FourPole,
            };
            let mut solved: Vec<(f64, Vec<Line>)> = sweep(&project, &rpms, kind)
                .points
                .into_iter()
                .filter_map(|p| match p {
                    PointOutcome::Solved(r) => Some((r.rpm, r.spectrum)),
                    PointOutcome::Failed { .. } => None,
                })
                .collect();
            solved.sort_by(|a, b| a.0.total_cmp(&b.0));
            let cabin = project.measurements.cabin_tf.as_ref();
            if interior && cabin.is_none() {
                return Err("the project has no measured cabin transfer function".into());
            }
            let gain = |f: f64| match cabin {
                Some(tf) if interior => tf.at(f),
                _ => Some(0.0),
            };
            let sound =
                audio::synthesize(&solved, lo, hi, seconds, &gain).map_err(|e| e.to_string())?;
            write_wav(&out, sound.sample_rate, &[&sound.samples], sound.peak_pa)?;
            let square: f64 = sound.samples.iter().map(|&s| (s as f64).powi(2)).sum();
            let level = |pa: f64| 20.0 * (pa / 20e-6).log10();
            eprintln!(
                "wrote {}: {:.1} s, Leq {:.1} dB, peak {:.1} dB re 20 µPa{}",
                out.display(),
                sound.samples.len() as f64 / sound.sample_rate,
                level((square / sound.samples.len().max(1) as f64).sqrt()),
                level(sound.peak_pa),
                if sound.dropped > 0 {
                    format!(
                        ", {} orders outside the cabin measurement left out",
                        sound.dropped
                    )
                } else {
                    String::new()
                }
            );
            Ok(ExitCode::SUCCESS)
        }
        Command::Render {
            project,
            scene,
            out,
            info,
        } => {
            let project = read_project(&project)?;
            let text =
                std::fs::read_to_string(&scene).map_err(|e| format!("{}: {e}", scene.display()))?;
            let scene: Scene =
                serde_json::from_str(&text).map_err(|e| format!("{}: {e}", scene.display()))?;
            let mut last = String::new();
            let r = render(&project, &scene, &mut |p| {
                let line = match p {
                    RenderProgress::Settle { cycle, .. } => format!("settling: cycle {cycle}"),
                    RenderProgress::March { fraction } => {
                        format!("marching: {:.0} %", 100.0 * fraction)
                    }
                };
                if line != last {
                    eprintln!("{line}");
                    last = line;
                }
                true
            })
            .map_err(|e| e.to_string())?;
            let channels: Vec<&[f32]> = r.channels.iter().map(Vec::as_slice).collect();
            write_wav(&out, r.info.sample_rate, &channels, r.info.peak_pa)?;
            emit(
                &serde_json::json!({
                    "wav": out,
                    "full_scale_pa": r.info.peak_pa / 0.9,
                    "render": r.info,
                }),
                info,
            )?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Hold {
            project: path,
            recording,
            from,
            to,
            background,
        } => {
            let project = read_project(&path)?;
            let spec = project
                .measurements
                .recordings
                .get(recording)
                .ok_or_else(|| format!("the project has no recording {recording}"))?;
            let cal = spec
                .calibration_db
                .ok_or("calibrate the recording first: the comparison is in dB re 20 µPa")?;
            let dir = path.parent().unwrap_or(Path::new("."));
            let read_rec = |file: &Path| -> Result<Recording, String> {
                let bytes = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
                Recording::parse(&file.to_string_lossy(), &bytes)
                    .map_err(|e| format!("{}: {e}", file.display()))
            };
            let rec = read_rec(&dir.join(&spec.path))?;
            let log = match &spec.rpm_log {
                Some(name) => {
                    let text = std::fs::read_to_string(dir.join(name))
                        .map_err(|e| format!("{name}: {e}"))?;
                    Some(RpmLog::parse(&text).map_err(|e| format!("{name}: {e}"))?)
                }
                None => None,
            };
            let [start, stop, _] = project.operating.sweep_rpm;
            let window = [from, to];
            let (rpm, rpm_spread, rpm_estimated) =
                measure::hold_rpm(&rec, log.as_ref(), spec.log_offset_s, window, [start, stop])
                    .map_err(|e| e.to_string())?;
            let scene = measure::hold_scene(rpm, window, spec.position);
            let r = render(&project, &scene, &mut |_| true).map_err(|e| e.to_string())?;
            let background = background.as_deref().map(read_rec).transpose()?;
            let resolved: Vec<bool> = r.info.bands.iter().map(|b| b.resolved).collect();
            emit(
                &measure::HoldComparison {
                    window_s: window,
                    rpm,
                    rpm_spread,
                    rpm_estimated,
                    bands: measure::compare_hold(
                        &rec,
                        cal,
                        window,
                        background.as_ref().map(|b| (b, cal)),
                        &r.channels[0],
                        r.info.sample_rate,
                        &resolved,
                    ),
                },
                None,
            )?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Sensitivity { project, rpm, out } => {
            let project = read_project(&project)?;
            let result = exhaust_core::sensitivity::sensitivity(&project, rpm, &|done, total| {
                eprint!("\r{done}/{total} runs");
                true
            })
            .map_err(|e| e.to_string())?;
            eprintln!();
            emit(&result, out)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Validate { json, cases: only } => {
            let mut all = Vec::new();
            let mut ok = true;
            for (name, case) in cases() {
                if !only.is_empty() && !only.iter().any(|c| c == name) {
                    continue;
                }
                match case() {
                    Ok(checks) => all.extend(checks),
                    Err(e) => {
                        ok = false;
                        eprintln!("case {name} did not run: {e}");
                    }
                }
            }
            ok &= all.iter().all(|c| c.pass);
            if json {
                let pending: Vec<_> = PENDING
                    .iter()
                    .map(|(case, needs)| serde_json::json!({ "case": case, "needs": needs }))
                    .collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "checks": all,
                        "pending": pending,
                    }))
                    .expect("checks serialise")
                );
            } else {
                for c in &all {
                    println!(
                        "{:<4} {:<26} {:<50} {:>11.4e} <= {:.3e}",
                        if c.pass { "ok" } else { "FAIL" },
                        c.case,
                        c.metric,
                        c.value,
                        c.limit
                    );
                }
                if only.is_empty() {
                    for (case, needs) in PENDING {
                        println!("pend {case:<26} needs {needs}");
                    }
                }
                println!(
                    "{}",
                    if ok {
                        "validation passed"
                    } else {
                        "VALIDATION FAILED"
                    }
                );
            }
            Ok(if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
    }
}

fn read_project(path: &Path) -> Result<Project, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Project::from_json(&text).map_err(|e| e.to_string())
}

/// The project's underbody scan (path relative to the project file), if it has one.
fn read_scan(project_path: &Path, project: &Project) -> Result<Option<Mesh>, String> {
    let Some(scan) = &project.fabrication.scan else {
        return Ok(None);
    };
    let file = project_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(&scan.path);
    let bytes = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    Mesh::parse(&scan.path, &bytes)
        .map(Some)
        .map_err(|e| format!("{}: {e}", file.display()))
}

/// 16-bit WAV of equal-length `channels` of pressure, Pa, interleaved, `peak_pa` at 0.9 of full
/// scale; the sample rate rounded to 1 Hz.
fn write_wav(
    path: &Path,
    sample_rate: f64,
    channels: &[&[f32]],
    peak_pa: f64,
) -> Result<(), String> {
    let scale = if peak_pa > 0.0 { 0.9 / peak_pa } else { 0.0 };
    let (rate, nc) = (sample_rate.round() as u32, channels.len() as u32);
    let n = channels.first().map_or(0, |c| c.len()) as u32;
    let bytes = 2 * nc * n;
    let mut b = b"RIFF".to_vec();
    b.extend((36 + bytes).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend([1u16, nc as u16].iter().flat_map(|x| x.to_le_bytes()));
    b.extend([rate, 2 * nc * rate].iter().flat_map(|x| x.to_le_bytes()));
    b.extend([2 * nc as u16, 16].iter().flat_map(|x| x.to_le_bytes()));
    b.extend(b"data");
    b.extend(bytes.to_le_bytes());
    for i in 0..n as usize {
        for c in channels {
            b.extend(((c[i] as f64 * scale * 32767.0).round() as i16).to_le_bytes());
        }
    }
    std::fs::write(path, b).map_err(|e| format!("{}: {e}", path.display()))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or(path.display().to_string(), |n| n.to_string_lossy().into())
}

/// Order tracks of a recording, its files relative to the project file.
fn tracks(
    project_path: &Path,
    project: &Project,
    spec: &RecordingRef,
) -> Result<OrderTracks, String> {
    let dir = project_path.parent().unwrap_or(Path::new("."));
    let read = |name: &str| {
        let file = dir.join(name);
        std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))
    };
    let rec = Recording::parse(&spec.path, &read(&spec.path)?)
        .map_err(|e| format!("{}: {e}", spec.path))?;
    let log = match &spec.rpm_log {
        Some(name) => Some(
            RpmLog::parse(&String::from_utf8_lossy(&read(name)?))
                .map_err(|e| format!("{name}: {e}"))?,
        ),
        None => None,
    };
    measure::order_tracks(
        project,
        &rec,
        log.as_ref(),
        spec.log_offset_s,
        spec.calibration_db,
    )
    .map_err(|e| e.to_string())
}

/// The fabrication files, named after the project file.
fn package(project_path: &Path, project: &Project) -> Result<Vec<(String, Vec<u8>)>, String> {
    let stem = project_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("project");
    let scan = read_scan(project_path, project)?;
    fabricate::files(project, stem, scan.as_ref()).map_err(|e| e.to_string())
}

/// Writes `bytes` to `out`, or to standard output.
fn write(out: Option<&Path>, bytes: &[u8]) -> Result<(), String> {
    match out {
        Some(path) => std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display())),
        None => {
            use std::io::Write;
            std::io::stdout()
                .write_all(bytes)
                .map_err(|e| e.to_string())
        }
    }
}

/// Writes `value` as pretty JSON to `out`, or to standard output.
fn emit(value: &(impl serde::Serialize + ?Sized), out: Option<PathBuf>) -> Result<(), String> {
    let json = serde_json::to_string_pretty(value).expect("results serialise");
    match out {
        Some(path) => std::fs::write(&path, json).map_err(|e| format!("{}: {e}", path.display())),
        None => {
            println!("{json}");
            Ok(())
        }
    }
}

fn parse_sweep(s: &str) -> Result<Vec<f64>, String> {
    let parts: Vec<f64> = s
        .split(':')
        .map(|p| p.trim().parse::<f64>())
        .collect::<Result<_, _>>()
        .map_err(|_| format!("bad --sweep '{s}'"))?;
    match parts.as_slice() {
        [start, stop, step] if *step > 0.0 && stop >= start && *start > 0.0 => {
            Ok(sweep_points(*start, *stop, *step))
        }
        [rpm] if *rpm > 0.0 => Ok(vec![*rpm]),
        _ => Err(format!(
            "--sweep takes start:stop:step or a single rpm, got '{s}'"
        )),
    }
}
