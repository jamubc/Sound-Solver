//! `exhaustctl`: headless front end to `exhaust-core`. Anything the app solves, this solves
//! with identical output.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use exhaust_core::layout::layout;
use exhaust_core::project::{Project, sweep_points};
use exhaust_core::scan::{self, Mesh};
use exhaust_core::solve::{SolverKind, sweep};
use exhaust_core::tune::tune;
use exhaust_core::validation::{PENDING, cases};
use exhaust_core::{fabricate, manifest, solid};

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
