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
use exhaust_core::measure::{self, OrderTracks, Recording, RpmLog};
use exhaust_core::metrics::{Metrics, OrderDifference};
use exhaust_core::project::{CabinTf, CabinTfMethod, Project};
use exhaust_core::scan::{self, Clearance, Mesh};
use exhaust_core::solve::{self, PointOutcome, SolverKind, SweepResult};
use exhaust_core::tune::{self, Tuning};
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

/// Applies `f` to every file path the project holds: scan, recordings, engine-speed logs.
fn map_paths(project: &mut Project, f: impl Fn(&str) -> String) {
    let (scan, recordings) = (
        &mut project.fabrication.scan,
        &mut project.measurements.recordings,
    );
    let paths = scan.iter_mut().map(|s| &mut s.path).chain(
        recordings
            .iter_mut()
            .flat_map(|r| std::iter::once(&mut r.path).chain(r.rpm_log.as_mut())),
    );
    for p in paths {
        *p = f(p);
    }
}

/// The app holds file paths absolute; the file holds them relative to itself where it can.
#[tauri::command]
fn open_project(path: PathBuf) -> Result<Value, String> {
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut project = Project::from_json(&text).map_err(|e| e.to_string())?;
    map_paths(&mut project, |p| dir(&path).join(p).display().to_string());
    Ok(to_value(&project))
}

#[tauri::command]
fn save_project(path: PathBuf, project: Value) -> Result<(), String> {
    let mut project = parse(&project)?;
    map_paths(&mut project, |p| {
        Path::new(p)
            .strip_prefix(dir(&path))
            .map_or(p.to_string(), |inside| inside.display().to_string())
    });
    std::fs::write(&path, project.to_json()).map_err(|e| format!("{}: {e}", path.display()))
}

/// Sizes a stub's length or a Helmholtz resonator's volume onto a drone (`tune::tune`).
#[tauri::command]
async fn tune(
    project: Value,
    element: String,
    target_hz: f64,
    rpm: f64,
    stated_k: Option<[f64; 2]>,
) -> Result<Tuning, String> {
    let project = parse(&project)?;
    blocking(move || {
        tune::tune(&project, &element, target_hz, rpm, stated_k).map_err(|e| e.to_string())
    })
    .await
}

fn read_recording(file: &Path) -> Result<Recording, String> {
    let bytes = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
    Recording::parse(&file.to_string_lossy(), &bytes)
        .map_err(|e| format!("{}: {e}", file.display()))
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map_or(path.into(), |n| n.to_string_lossy().into())
}

/// Order tracks of the project's recording `index`.
fn recording_tracks(project: &Project, index: usize) -> Result<OrderTracks, String> {
    let spec = project
        .measurements
        .recordings
        .get(index)
        .ok_or_else(|| format!("the project has no recording {index}"))?;
    let rec = read_recording(Path::new(&spec.path))?;
    let log = match &spec.rpm_log {
        Some(file) => {
            let text = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
            Some(RpmLog::parse(&text).map_err(|e| format!("{file}: {e}"))?)
        }
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

#[tauri::command]
async fn order_tracks(project: Value, index: usize) -> Result<OrderTracks, String> {
    let project = parse(&project)?;
    blocking(move || recording_tracks(&project, index)).await
}

/// Cabin transfer function by order ratio of two of the project's recordings of one drive.
#[tauri::command]
async fn cabin_tf_orders(
    project: Value,
    exterior: usize,
    interior: usize,
) -> Result<CabinTf, String> {
    let project = parse(&project)?;
    blocking(move || {
        let name = |i: usize| file_name(&project.measurements.recordings[i].path);
        let gain_db = measure::cabin_tf_orders(
            &recording_tracks(&project, exterior)?,
            &recording_tracks(&project, interior)?,
        )
        .map_err(|e| e.to_string())?;
        Ok(CabinTf {
            method: CabinTfMethod::OrderRatio,
            source: format!(
                "order ratio: {} (exterior), {} (interior)",
                name(exterior),
                name(interior)
            ),
            gain_db,
        })
    })
    .await
}

/// Cabin transfer function from recordings of the same impulses outside and inside.
#[tauri::command]
async fn cabin_tf_impulse(exterior: PathBuf, interior: PathBuf) -> Result<CabinTf, String> {
    blocking(move || {
        let gain_db =
            measure::cabin_tf_impulse(&read_recording(&exterior)?, &read_recording(&interior)?)
                .map_err(|e| e.to_string())?;
        Ok(CabinTf {
            method: CabinTfMethod::Impulse,
            source: format!(
                "impulse: {} (exterior), {} (interior)",
                file_name(&exterior.to_string_lossy()),
                file_name(&interior.to_string_lossy())
            ),
            gain_db,
        })
    })
    .await
}

/// Sound metrics of solved points under the project's measurements as they are now.
#[tauri::command]
fn evaluate(project: Value, points: Vec<PointOutcome>) -> Result<Metrics, String> {
    Ok(metrics::evaluate(&parse(&project)?, &points))
}

/// Engine orders of configuration `b` against `a`.
#[tauri::command]
fn compare(
    project: Value,
    a: Vec<PointOutcome>,
    b: Vec<PointOutcome>,
) -> Result<Vec<OrderDifference>, String> {
    Ok(metrics::compare(&parse(&project)?, &a, &b))
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
            clearance,
            tune,
            order_tracks,
            cabin_tf_orders,
            cabin_tf_impulse,
            evaluate,
            compare
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
                clearance,
                tune,
                order_tracks,
                cabin_tf_orders,
                cabin_tf_impulse,
                evaluate,
                compare
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

        // Recordings: 4 s of the firing order at 2100 rpm (70 Hz), outside and 20 dB down inside.
        let wav = |name: &str, gain: f64| {
            let (fs, n) = (8000u32, 32000u32);
            let mut b = b"RIFF".to_vec();
            b.extend((36 + 2 * n).to_le_bytes());
            b.extend(b"WAVEfmt ");
            b.extend(16u32.to_le_bytes());
            b.extend([1u16, 1].iter().flat_map(|x| x.to_le_bytes()));
            b.extend([fs, 2 * fs].iter().flat_map(|x| x.to_le_bytes()));
            b.extend([2u16, 16].iter().flat_map(|x| x.to_le_bytes()));
            b.extend(b"data");
            b.extend((2 * n).to_le_bytes());
            for i in 0..n {
                let s =
                    gain * 0.3 * (2.0 * std::f64::consts::PI * 70.0 * i as f64 / fs as f64).sin();
                b.extend(((s * 32767.0) as i16).to_le_bytes());
            }
            std::fs::write(dir.join(name), b).unwrap();
            dir.join(name)
        };
        scanned["measurements"] = json!({ "recordings": [
            { "path": wav("outside.wav", 1.0), "position": "exterior" },
            { "path": wav("inside.wav", 0.1), "position": "interior" },
        ]});
        let tracks =
            call("order_tracks", json!({ "project": scanned, "index": 0 })).expect("tracks");
        assert_eq!(tracks["rpm_estimated"], true);
        let tf = call(
            "cabin_tf_orders",
            json!({ "project": scanned, "exterior": 0, "interior": 1 }),
        )
        .expect("order ratio");
        assert_eq!(
            tf["source"],
            "order ratio: outside.wav (exterior), inside.wav (interior)"
        );
        let tf = call(
            "cabin_tf_impulse",
            json!({ "exterior": dir.join("outside.wav"), "interior": dir.join("inside.wav") }),
        )
        .expect("impulse");
        assert_eq!(tf["method"], "impulse");
        let metrics = call(
            "evaluate",
            json!({ "project": scanned, "points": preview["points"] }),
        )
        .expect("metrics");
        assert_eq!(metrics["firing_order"], 2.0);
        let differences = call(
            "compare",
            json!({ "project": project, "a": preview["points"], "b": preview["points"] }),
        )
        .expect("differences");
        assert_eq!(differences[1]["cruise_db"], 0.0);
        let stub = &inserted["project"];
        let tuning = call(
            "tune",
            json!({ "project": stub, "element": inserted["id"], "targetHz": 150.0, "rpm": 3000.0, "statedK": null }),
        )
        .expect("tuning");
        assert_eq!(tuning["parameter"], "length_mm");

        // The file holds paths relative to itself; the app holds them absolute.
        let path = dir.join("w205.json");
        call("save_project", json!({ "path": path, "project": scanned })).expect("saved");
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(
            saved.contains("\"path\": \"floor.stl\"") && saved.contains("\"path\": \"inside.wav\"")
        );
        let reopened = call("open_project", json!({ "path": path })).expect("reopened");
        assert_eq!(reopened["fabrication"]["scan"]["path"], json!(floor));
        assert_eq!(
            reopened["measurements"]["recordings"][1]["path"],
            json!(dir.join("inside.wav"))
        );
        let files = call(
            "export_package",
            json!({ "project": scanned, "path": path }),
        )
        .expect("package");
        assert_eq!(files.as_array().unwrap().len(), 6, "{files}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
