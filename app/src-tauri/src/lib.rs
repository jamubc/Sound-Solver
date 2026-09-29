//! Desktop shell: a thin Tauri backend over `exhaust-core`. The frontend holds the project as
//! its JSON (the project schema) and passes it whole to every command, so the core is the only
//! place a number comes from.
//!
//! Time-domain sweeps run point by point on Rayon, each point sent to the frontend as it
//! finishes and cached on disk under the project's BLAKE3 hash, so an unchanged project never
//! solves twice. A newer sweep supersedes an older one: points not yet started are dropped.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use exhaust_core::fabricate::{self, Package};
use exhaust_core::layout::{Layout, layout};
use exhaust_core::manifest::{self, Manifest};
use exhaust_core::project::Project;
use exhaust_core::scan::{self, Clearance, Mesh};
use exhaust_core::solve::{self, PointOutcome, SolverKind, SweepResult};
use exhaust_core::{edit, metrics};
use rayon::prelude::*;
use serde_json::Value;
use tauri::ipc::{Channel, Response};
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

/// The directory of the project file `path`.
fn dir(path: &Path) -> &Path {
    path.parent().unwrap_or(Path::new("."))
}

fn read_mesh(file: &Path) -> Result<Mesh, String> {
    let bytes = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
    Mesh::parse(&file.to_string_lossy(), &bytes).map_err(|e| format!("{}: {e}", file.display()))
}

/// Runs CPU-heavy work off the main thread.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| e.to_string())?
}

/// Straights, bends, stock sticks and joints of the project as drawn.
#[tauri::command]
fn fabrication(project: Value) -> Result<Package, String> {
    fabricate::check(&parse(&project)?).map_err(|e| e.to_string())
}

/// Writes the fabrication files beside the project file `path`; answers their paths.
#[tauri::command]
async fn export_package(project: Value, path: PathBuf) -> Result<Vec<String>, String> {
    let project = parse(&project)?;
    blocking(move || {
        let scan = match &project.fabrication.scan {
            Some(s) => Some(read_mesh(&dir(&path).join(&s.path))?),
            None => None,
        };
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("project");
        let files = fabricate::files(&project, stem, scan.as_ref()).map_err(|e| e.to_string())?;
        files
            .into_iter()
            .map(|(name, bytes)| {
                let target = dir(&path).join(name);
                std::fs::write(&target, bytes).map_err(|e| format!("{}: {e}", target.display()))?;
                Ok(target.display().to_string())
            })
            .collect()
    })
    .await
}

/// A scan file as a mesh for the viewport, in scan units: vertex and triangle counts (u32),
/// then positions (3 × f32 per vertex) and corners (3 × u32 per triangle), little-endian.
#[tauri::command]
async fn scan_mesh(file: PathBuf) -> Result<Response, String> {
    blocking(move || {
        let mesh = read_mesh(&file)?;
        let mut out = Vec::with_capacity(8 + 12 * mesh.vertices.len() + 12 * mesh.triangles.len());
        out.extend((mesh.vertices.len() as u32).to_le_bytes());
        out.extend((mesh.triangles.len() as u32).to_le_bytes());
        for v in &mesh.vertices {
            v.iter().for_each(|x| out.extend((*x as f32).to_le_bytes()));
        }
        for t in &mesh.triangles {
            t.iter().for_each(|i| out.extend(i.to_le_bytes()));
        }
        Ok(Response::new(out))
    })
    .await
}

/// Pipe clearance to the project's scan, placed by its reference points.
#[tauri::command]
async fn clearance(project: Value) -> Result<Clearance, String> {
    let project = parse(&project)?;
    blocking(move || {
        let file = &project
            .fabrication
            .scan
            .as_ref()
            .ok_or("the project has no scan")?
            .path;
        scan::clearance(&project, &read_mesh(Path::new(file))?).map_err(|e| e.to_string())
    })
    .await
}

/// The app holds the scan path absolute; the file holds it relative to the project file.
#[tauri::command]
fn open_project(path: PathBuf) -> Result<Value, String> {
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut project = Project::from_json(&text).map_err(|e| e.to_string())?;
    if let Some(scan) = &mut project.fabrication.scan {
        scan.path = dir(&path).join(&scan.path).display().to_string();
    }
    Ok(to_value(&project))
}

#[tauri::command]
fn save_project(path: PathBuf, project: Value) -> Result<(), String> {
    let mut project = parse(&project)?;
    if let Some(scan) = &mut project.fabrication.scan
        && let Ok(inside) = Path::new(&scan.path).strip_prefix(dir(&path))
    {
        scan.path = inside.display().to_string();
    }
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

/// Places a new element of type `kind` on `route` at `s_mm` along it (`edit::insert`); answers
/// the edited project and the element's id.
#[tauri::command]
fn insert_element(project: Value, route: String, s_mm: f64, kind: String) -> Result<Value, String> {
    let mut project = parse(&project)?;
    let id = edit::insert(&mut project, &route, s_mm, &kind).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "project": to_value(&project), "id": id }))
}

/// Takes a two-port element out, joining its pipes (`edit::remove`).
#[tauri::command]
fn remove_element(project: Value, id: String) -> Result<Value, String> {
    let mut project = parse(&project)?;
    edit::remove(&mut project, &id).map_err(|e| e.to_string())?;
    Ok(to_value(&project))
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
            insert_element,
            remove_element,
            preview,
            solve,
            cancel,
            fabrication,
            export_package,
            scan_mesh,
            clearance
        ])
        .run(tauri::generate_context!())
        .expect("error while running the app");
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tauri::ipc::{CallbackFn, InvokeBody, InvokeResponseBody};
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
                insert_element,
                remove_element,
                preview,
                open_project,
                save_project,
                fabrication,
                export_package,
                scan_mesh,
                clearance
            ])
            .build(mock_context(noop_assets()))
            .expect("app builds");
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("webview");
        let request = |cmd: &str, args: Value| InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "tauri://localhost".parse().unwrap(),
            body: InvokeBody::Json(args),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        };
        let call = |cmd: &str, args: Value| {
            get_ipc_response(&webview, request(cmd, args))
                .map(|b| b.deserialize::<Value>().unwrap())
        };
        let project = call("stock_project", json!({})).expect("reference project");
        let layout =
            call("check", json!({ "project": project })).expect("reference project checks");
        assert_eq!(layout["routes"].as_array().unwrap().len(), 6);
        let manifests = call("manifests", json!({})).unwrap();
        assert_eq!(manifests.as_array().unwrap().len(), manifest::all().len());
        let preview = call("preview", json!({ "project": project })).expect("preview");
        assert_eq!(preview["solver"], "four_pole");
        let args = json!({ "project": project, "route": "mid-pipe", "sMm": 300.0, "kind": "quarter_wave_stub" });
        let inserted = call("insert_element", args).expect("stub on the mid-pipe");
        assert_eq!(inserted["id"], "quarter_wave_stub");
        let without = call(
            "remove_element",
            json!({ "project": project, "id": "resonator" }),
        )
        .expect("resonator delete");
        assert_eq!(without["system"]["routes"].as_array().unwrap().len(), 5);
        let refused = call(
            "remove_element",
            json!({ "project": project, "id": "muffler" }),
        );
        assert!(refused.is_err(), "the dual-outlet muffler has three ports");
        // An unsolvable project comes back as the reason.
        let mut bad = project.clone();
        bad["system"]["elements"][3]["diameter_mm"] = json!(1e4);
        let reason = call("check", json!({ "project": bad })).unwrap_err();
        assert!(
            reason.as_str().unwrap().contains("must be 40–600 mm"),
            "{reason}"
        );

        let parts = call("fabrication", json!({ "project": project })).expect("parts");
        assert_eq!(parts["routes"].as_array().unwrap().len(), 6);
        // A flat underbody 200 mm above the flange, scanned in metres, beside a saved project.
        let dir = std::env::temp_dir().join(format!("exhaust-app-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let floor = dir.join("floor.stl");
        let facet = |c: [[f64; 2]; 3]| {
            let v: String = c
                .iter()
                .map(|[x, y]| format!("vertex {x} {y} 0.2\n"))
                .collect();
            format!("facet normal 0 0 1\nouter loop\n{v}endloop\nendfacet\n")
        };
        let stl = facet([[-5.0, -2.0], [2.0, -2.0], [2.0, 2.0]])
            + &facet([[-5.0, -2.0], [2.0, 2.0], [-5.0, 2.0]]);
        std::fs::write(&floor, format!("solid floor\n{stl}endsolid floor\n")).unwrap();
        let Ok(InvokeResponseBody::Raw(mesh)) =
            get_ipc_response(&webview, request("scan_mesh", json!({ "file": floor })))
        else {
            panic!("scan mesh as bytes");
        };
        assert_eq!(mesh.len(), 8 + 6 * 12 + 2 * 12);
        let mut scanned = project.clone();
        scanned["fabrication"]["scan"] = json!({
            "path": floor,
            "unit_mm": 1000.0,
            "scan_points": [[-4.0, -1.0, 0.2], [1.0, -1.0, 0.2], [-4.0, 1.0, 0.2]],
            "vehicle_points_mm": [[-4000.0, -1000.0, 200.0], [1000.0, -1000.0, 200.0], [-4000.0, 1000.0, 200.0]],
        });
        let clearance = call("clearance", json!({ "project": scanned })).expect("clearance");
        assert!(clearance["placement"]["residual_mm"].as_f64().unwrap() < 1e-6);
        assert_eq!(clearance["routes"].as_array().unwrap().len(), 6);
        // The file holds the scan path relative to itself; the app holds it absolute.
        let path = dir.join("w205.json");
        call("save_project", json!({ "path": path, "project": scanned })).expect("saved");
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("\"path\": \"floor.stl\"")
        );
        let reopened = call("open_project", json!({ "path": path })).expect("reopened");
        assert_eq!(reopened["fabrication"]["scan"]["path"], json!(floor));
        let files = call(
            "export_package",
            json!({ "project": scanned, "path": path }),
        )
        .expect("package");
        assert_eq!(files.as_array().unwrap().len(), 6, "{files}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
