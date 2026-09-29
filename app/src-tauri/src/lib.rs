//! Desktop shell: a thin Tauri backend over `exhaust-core`. The frontend holds the project as
//! its JSON (the project schema) and passes it whole to every command, so the core is the only
//! place a number comes from.
//!
//! Time-domain sweeps run point by point on Rayon, each point sent to the frontend as it
//! finishes and cached on disk under the project's BLAKE3 hash, so an unchanged project never
//! solves twice. A newer sweep supersedes an older one: points not yet started are dropped.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use exhaust_core::layout::{Layout, layout};
use exhaust_core::manifest::{self, Manifest};
use exhaust_core::metrics;
use exhaust_core::project::Project;
use exhaust_core::solve::{self, PointOutcome, SolverKind, SweepResult};
use rayon::prelude::*;
use serde_json::Value;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager};

/// The reference project the app opens with.
const STOCK_W205: &str = include_str!("../../../tests/cases/w205_stock.json");

/// Bumped by every sweep and by `cancel`; a sweep stops starting points once it is stale.
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn parse(project: &Value) -> Result<Project, String> {
    Project::from_json(&project.to_string()).map_err(|e| e.to_string())
}

fn to_value(project: &Project) -> Value {
    serde_json::from_str(&project.to_json()).expect("projects serialise")
}

#[tauri::command]
fn stock_project() -> Value {
    serde_json::from_str(STOCK_W205).expect("reference case is JSON")
}

#[tauri::command]
fn open_project(path: PathBuf) -> Result<Value, String> {
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let project = Project::from_json(&text).map_err(|e| e.to_string())?;
    Ok(to_value(&project))
}

#[tauri::command]
fn save_project(path: PathBuf, project: Value) -> Result<(), String> {
    let project = parse(&project)?;
    std::fs::write(&path, project.to_json()).map_err(|e| format!("{}: {e}", path.display()))
}

/// Validates the project and lays it out for the viewport.
#[tauri::command]
fn check(project: Value) -> Result<Layout, String> {
    layout(&parse(&project)?).map_err(|e| e.to_string())
}

#[tauri::command]
fn manifests() -> Vec<Manifest> {
    manifest::all().to_vec()
}

/// Four-pole preview over the project's sweep.
#[tauri::command]
async fn preview(project: Value) -> Result<SweepResult, String> {
    let project = parse(&project)?;
    tauri::async_runtime::spawn_blocking(move || {
        solve::sweep(&project, &project.operating.sweep(), SolverKind::FourPole)
    })
    .await
    .map_err(|e| e.to_string())
}

/// Time-domain sweep over the project's sweep, streaming each point through `on_point`;
/// returns the sweep with its metrics, or an error if a newer sweep superseded it.
#[tauri::command]
async fn solve(
    app: AppHandle,
    project: Value,
    on_point: Channel<PointOutcome>,
) -> Result<SweepResult, String> {
    let project = parse(&project)?;
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let cache = app
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("time-domain");
    std::fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let hash = project.hash();
        let rpms = project.operating.sweep();
        let points: Vec<Option<PointOutcome>> = rpms
            .par_iter()
            .map(|&rpm| {
                if GENERATION.load(Ordering::SeqCst) != generation {
                    return None;
                }
                let file = cache.join(format!("{hash}-{rpm}.json"));
                let cached = std::fs::read_to_string(&file)
                    .ok()
                    .and_then(|text| serde_json::from_str(&text).ok());
                let outcome = cached.unwrap_or_else(|| {
                    let outcome = match solve::solve_point(&project, rpm) {
                        Ok(r) => PointOutcome::Solved(Box::new(r)),
                        Err(e) => PointOutcome::Failed {
                            rpm,
                            error: e.to_string(),
                        },
                    };
                    if let Ok(json) = serde_json::to_string(&outcome) {
                        let _ = std::fs::write(&file, json);
                    }
                    outcome
                });
                let _ = on_point.send(outcome.clone());
                Some(outcome)
            })
            .collect();
        let points: Option<Vec<PointOutcome>> = points.into_iter().collect();
        let points = points.ok_or("superseded by a newer sweep")?;
        Ok(SweepResult {
            project: project.name.clone(),
            project_hash: hash,
            solver: SolverKind::TimeDomain,
            metrics: metrics::evaluate(&project, &points),
            points,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Stops the running sweep from starting further points.
#[tauri::command]
fn cancel() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            stock_project,
            open_project,
            save_project,
            check,
            manifests,
            preview,
            solve,
            cancel
        ])
        .run(tauri::generate_context!())
        .expect("error while running the app");
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{INVOKE_KEY, get_ipc_response, mock_builder, mock_context, noop_assets};
    use tauri::webview::InvokeRequest;

    use super::*;

    /// The calls app/src/lib/backend.ts makes, with its arguments, through the IPC layer.
    #[test]
    fn commands_answer_the_frontends_calls() {
        let app = mock_builder()
            .invoke_handler(tauri::generate_handler![
                stock_project,
                check,
                manifests,
                preview
            ])
            .build(mock_context(noop_assets()))
            .expect("app builds");
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("webview");
        let call = |cmd: &str, args: Value| {
            let request = InvokeRequest {
                cmd: cmd.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: "tauri://localhost".parse().unwrap(),
                body: InvokeBody::Json(args),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.to_string(),
            };
            get_ipc_response(&webview, request).map(|b| b.deserialize::<Value>().unwrap())
        };
        let project = call("stock_project", json!({})).expect("reference project");
        let layout =
            call("check", json!({ "project": project })).expect("reference project checks");
        assert_eq!(layout["routes"].as_array().unwrap().len(), 6);
        let manifests = call("manifests", json!({})).unwrap();
        assert_eq!(manifests.as_array().unwrap().len(), manifest::all().len());
        let preview = call("preview", json!({ "project": project })).expect("preview");
        assert_eq!(preview["solver"], "four_pole");
        // An unsolvable project comes back as the reason.
        let mut bad = project.clone();
        bad["system"]["elements"][3]["diameter_mm"] = json!(1e4);
        let reason = call("check", json!({ "project": bad })).unwrap_err();
        assert!(
            reason.as_str().unwrap().contains("must be 40–600 mm"),
            "{reason}"
        );
    }
}
