//! `exhaustctl`: headless front end to `exhaust-core`. Anything the app solves, this solves
//! with identical output.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use exhaust_core::project::{Project, sweep_points};
use exhaust_core::solve::{SolverKind, sweep};
use exhaust_core::validation::{PENDING, cases};

#[derive(Clone, Copy, ValueEnum)]
enum Solver {
    /// Nonlinear time-domain gas dynamics (the reference result).
    TimeDomain,
    /// Linear four-pole preview.
    FourPole,
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
            let text = std::fs::read_to_string(&project)
                .map_err(|e| format!("{}: {e}", project.display()))?;
            let project = Project::from_json(&text).map_err(|e| e.to_string())?;
            let rpms = match rpm_sweep {
                Some(s) => parse_sweep(&s)?,
                None => project.operating.sweep(),
            };
            let kind = match solver {
                Solver::TimeDomain => SolverKind::TimeDomain,
                Solver::FourPole => SolverKind::FourPole,
            };
            let result = sweep(&project, &rpms, kind);
            let json = serde_json::to_string_pretty(&result).expect("results serialise");
            match out {
                Some(path) => {
                    std::fs::write(&path, json).map_err(|e| format!("{}: {e}", path.display()))?
                }
                None => println!("{json}"),
            }
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
