//! Desktop shell: a thin Tauri backend over `exhaust-core`. The frontend holds the project as
//! its JSON (the project schema) and passes it whole to every command, so the core is the only
//! place a number comes from.
//!
//! Time-domain points and renders run as jobs (`exhaust_jobs`) on this machine, cached on disk
//! under their job keys (the project's solved content, the job's parameters and the solver
//! build), so an unchanged project never solves twice and a changed solver never reuses an old
//! result. Sweeps run point by point on Rayon, each point sent to the frontend as it finishes;
//! a newer sweep supersedes an older one: points not yet started are dropped.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use exhaust_core::audio;
use exhaust_core::fabricate::{self, Package};
use exhaust_core::layout::{Layout, layout};
use exhaust_core::manifest::{self, Manifest};
use exhaust_core::measure::{self, HoldComparison, OrderTracks, Recording, RpmLog};
use exhaust_core::metrics::{Metrics, OrderDifference};
use exhaust_core::project::{CabinTf, CabinTfMethod, Project};
use exhaust_core::render::{RenderProgress, Scene};
use exhaust_core::scan::{self, Clearance, Mesh};
use exhaust_core::solve::{self, CycleProgress, Line, PointOutcome, SolverKind, SweepResult};
use exhaust_core::tune::{self, Tuning};
use exhaust_core::validation::{self, CaseReport};
use exhaust_core::{edit, metrics};
use exhaust_jobs::{Job, Output, Progress, Runner};
use rayon::prelude::*;
use serde_json::Value;
use tauri::ipc::{Channel, Response};
use tauri::{AppHandle, Manager};

/// The reference project the app opens with.
const STOCK_W205: &str = include_str!("../../../tests/cases/w205_stock.json");

/// Bumped by every sweep and by `cancel`; a sweep stops starting points once it is stale.
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// Bumped by every render and by `cancel_render`; a render stops once it is stale.
static RENDER: AtomicU64 = AtomicU64::new(0);

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

/// Jobs on this machine, cached in the app's cache directory.
fn runner(app: &AppHandle) -> Result<Runner, String> {
    let dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("jobs");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(Runner::local(dir))
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

/// The project's recording `index` and its engine-speed log, if it has one.
fn recording(project: &Project, index: usize) -> Result<(Recording, Option<RpmLog>), String> {
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
    Ok((rec, log))
}

/// Order tracks of the project's recording `index`.
fn recording_tracks(project: &Project, index: usize) -> Result<OrderTracks, String> {
    let (rec, log) = recording(project, index)?;
    let spec = &project.measurements.recordings[index];
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

/// The receiver sound of solved `points` as the engine goes from `from_rpm` to `to_rpm` over
/// `seconds` (a seamless loop at one speed), outside or through the cabin transfer function
/// (`audio::synthesize`): sample rate and peak pressure (f64), orders left out (u32), then the
/// pressure (f32, Pa), little-endian.
#[tauri::command]
async fn listen(
    project: Value,
    points: Vec<PointOutcome>,
    from_rpm: f64,
    to_rpm: f64,
    seconds: f64,
    interior: bool,
) -> Result<Response, String> {
    let project = parse(&project)?;
    blocking(move || {
        let mut solved: Vec<(f64, Vec<Line>)> = points
            .into_iter()
            .filter_map(|p| match p {
                PointOutcome::Solved(r) => Some((r.rpm, r.spectrum)),
                PointOutcome::Failed { .. } => None,
            })
            .collect();
        solved.sort_by(|a, b| a.0.total_cmp(&b.0));
        let cabin = project.measurements.cabin_tf.as_ref();
        if interior && cabin.is_none() {
            return Err(
                "no measured cabin transfer function: the cabin is never synthesised".into(),
            );
        }
        let gain = |f: f64| match cabin {
            Some(tf) if interior => tf.at(f),
            _ => Some(0.0),
        };
        let sound = audio::synthesize(&solved, from_rpm, to_rpm, seconds, &gain)
            .map_err(|e| e.to_string())?;
        let mut out = Vec::with_capacity(20 + 4 * sound.samples.len());
        out.extend(sound.sample_rate.to_le_bytes());
        out.extend(sound.peak_pa.to_le_bytes());
        out.extend((sound.dropped as u32).to_le_bytes());
        sound
            .samples
            .iter()
            .for_each(|s| out.extend(s.to_le_bytes()));
        Ok(Response::new(out))
    })
    .await
}

/// Marches the project through `scene` (`render::render`), streaming its progress through
/// `on_progress`: the render's provenance as JSON after its byte length (u32; padded with
/// spaces to a multiple of 4), then the channel count and samples per channel (u32 each), then
/// each channel's pressure (f32, Pa), little-endian. A newer render or `cancel_render` stops it.
#[tauri::command]
async fn render(
    app: AppHandle,
    project: Value,
    scene: Scene,
    on_progress: Channel<RenderProgress>,
) -> Result<Response, String> {
    let project = parse(&project)?;
    let runner = runner(&app)?;
    let generation = RENDER.fetch_add(1, Ordering::SeqCst) + 1;
    blocking(move || {
        let job = Job::Render {
            project: Box::new(project),
            scene,
        };
        let out = runner.run(&job, &mut |p| {
            if let Progress::Render(p) = p {
                let _ = on_progress.send(p);
            }
            RENDER.load(Ordering::SeqCst) == generation
        })?;
        let Output::Render { info, channels } = out else {
            return Err("a render job made no render".into());
        };
        let mut info = serde_json::to_vec(&info).map_err(|e| e.to_string())?;
        info.resize(info.len().next_multiple_of(4), b' ');
        let n = channels.first().map_or(0, Vec::len);
        let mut out = Vec::with_capacity(12 + info.len() + 4 * n * channels.len());
        out.extend((info.len() as u32).to_le_bytes());
        out.extend(&info);
        out.extend((channels.len() as u32).to_le_bytes());
        out.extend((n as u32).to_le_bytes());
        for s in channels.iter().flatten() {
            out.extend(s.to_le_bytes());
        }
        Ok(Response::new(out))
    })
    .await
}

/// A steady hold of the project's recording `index` (`window_s`, s from its start) against a
/// render of the same engine speed where the recording was made (`measure::compare_hold`),
/// the render through the job runner with its progress streamed; `background`, a recording of
/// the background taken with the same phone at the same gain. Answers the comparison as JSON
/// after its byte length (u32; padded with spaces to a multiple of 4), then the recording's
/// and the render's pressure over the hold, each as its sample rate (f64), its length (u32)
/// and its samples (f32, Pa), little-endian. `cancel_render` stops it.
#[tauri::command]
async fn validate_hold(
    app: AppHandle,
    project: Value,
    index: usize,
    window_s: [f64; 2],
    background: Option<PathBuf>,
    on_progress: Channel<RenderProgress>,
) -> Result<Response, String> {
    let project = parse(&project)?;
    let runner = runner(&app)?;
    let generation = RENDER.fetch_add(1, Ordering::SeqCst) + 1;
    blocking(move || {
        let (rec, log) = recording(&project, index)?;
        let spec = &project.measurements.recordings[index];
        let cal = spec
            .calibration_db
            .ok_or("calibrate the recording first: the comparison is in dB re 20 µPa")?;
        let [start, stop, _] = project.operating.sweep_rpm;
        let (rpm, rpm_spread, rpm_estimated) = measure::hold_rpm(
            &rec,
            log.as_ref(),
            spec.log_offset_s,
            window_s,
            [start, stop],
        )
        .map_err(|e| e.to_string())?;
        let job = Job::Render {
            project: Box::new(project.clone()),
            scene: measure::hold_scene(rpm, window_s, spec.position),
        };
        let out = runner.run(&job, &mut |p| {
            if let Progress::Render(p) = p {
                let _ = on_progress.send(p);
            }
            RENDER.load(Ordering::SeqCst) == generation
        })?;
        let Output::Render { info, channels } = out else {
            return Err("a render job made no render".into());
        };
        let background = background.map(|p| read_recording(&p)).transpose()?;
        let resolved: Vec<bool> = info.bands.iter().map(|b| b.resolved).collect();
        let comparison = HoldComparison {
            window_s,
            rpm,
            rpm_spread,
            rpm_estimated,
            bands: measure::compare_hold(
                &rec,
                cal,
                window_s,
                background.as_ref().map(|b| (b, cal)),
                &channels[0],
                info.sample_rate,
                &resolved,
            ),
        };
        let mut json = serde_json::to_vec(&comparison).map_err(|e| e.to_string())?;
        json.resize(json.len().next_multiple_of(4), b' ');
        let mut out = (json.len() as u32).to_le_bytes().to_vec();
        out.extend(&json);
        for (fs, samples) in [
            (rec.sample_rate, measure::pascals(&rec, cal, window_s)),
            (info.sample_rate, channels[0].clone()),
        ] {
            out.extend(fs.to_le_bytes());
            out.extend((samples.len() as u32).to_le_bytes());
            samples.iter().for_each(|s| out.extend(s.to_le_bytes()));
        }
        Ok(Response::new(out))
    })
    .await
}

/// Runs the analytic verification suite, sending each case as it finishes.
#[tauri::command]
async fn verify(on_case: Channel<CaseReport>) -> Result<Vec<CaseReport>, String> {
    blocking(move || {
        Ok(validation::cases()
            .par_iter()
            .map(|(name, run)| {
                let (checks, error) = match run() {
                    Ok(checks) => (checks, None),
                    Err(e) => (Vec::new(), Some(e.to_string())),
                };
                let report = CaseReport {
                    case: name.to_string(),
                    subsystem: validation::subsystem(name).into(),
                    checks,
                    error,
                };
                let _ = on_case.send(report.clone());
                report
            })
            .collect())
    })
    .await
}

/// Stops the running render.
#[tauri::command]
fn cancel_render() {
    RENDER.fetch_add(1, Ordering::SeqCst);
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

/// Time-domain sweep over the project's sweep, streaming every engine cycle of the points in
/// flight through `on_cycle` and each point through `on_point`; returns the sweep with its
/// metrics, or an error if a newer sweep (or `cancel`) superseded it, which also stops the
/// points in flight.
#[tauri::command]
async fn solve(
    app: AppHandle,
    project: Value,
    on_point: Channel<PointOutcome>,
    on_cycle: Channel<CycleProgress>,
) -> Result<SweepResult, String> {
    let project = parse(&project)?;
    let runner = runner(&app)?;
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    tauri::async_runtime::spawn_blocking(move || {
        let hash = project.hash();
        let rpms = project.operating.sweep();
        let current = || GENERATION.load(Ordering::SeqCst) == generation;
        let points: Vec<Option<PointOutcome>> = rpms
            .par_iter()
            .map(|&rpm| {
                if !current() {
                    return None;
                }
                let job = Job::Point {
                    project: Box::new(project.clone()),
                    rpm,
                };
                let run = runner.run(&job, &mut |p| {
                    if let Progress::Cycle(c) = p {
                        let _ = on_cycle.send(c);
                    }
                    current()
                });
                let outcome = match run {
                    Ok(Output::Point(outcome)) => outcome,
                    // Stopped by a newer sweep: neither cached nor reported.
                    Err(_) if !current() => return None,
                    Err(error) => PointOutcome::Failed { rpm, error },
                    Ok(_) => PointOutcome::Failed {
                        rpm,
                        error: "a point job made no point".into(),
                    },
                };
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
            compare,
            listen,
            render,
            cancel_render,
            validate_hold,
            verify
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
                compare,
                listen
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
        // Two seconds or more of the preview at 2000 rpm: header, then the pressure.
        let args = json!({ "project": project, "points": preview["points"], "fromRpm": 2000.0, "toRpm": 2000.0, "seconds": 2.0, "interior": false });
        let Ok(InvokeResponseBody::Raw(sound)) =
            get_ipc_response(&webview, request("listen", args))
        else {
            panic!("sound as bytes");
        };
        let rate = f64::from_le_bytes(sound[..8].try_into().unwrap());
        let peak = f64::from_le_bytes(sound[8..16].try_into().unwrap());
        assert!(
            (sound.len() - 20) / 4 >= (2.0 * rate) as usize && peak > 1.0,
            "{rate} Hz, {peak} Pa"
        );
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
