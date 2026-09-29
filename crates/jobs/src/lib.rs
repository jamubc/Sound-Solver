//! Jobs: work described by a spec that fixes its result, run on a backend with the memory for
//! it, and cached under the spec's key. The spec and the result are the same on every backend,
//! so a result made on one host serves any other.
//!
//! The key covers everything the result depends on: the project's solved content
//! (`Project::hash`), the job's parameters, and the solver that makes it (the hash of the core's
//! source, `exhaust_core::SOURCE`, or a container image pinned by digest). A changed solver
//! never reuses an old result.
//!
//! Backends: [`Local`] runs native jobs in this process and container jobs through Docker on
//! this machine; [`Cloud`] is not built yet. A backend refuses nothing itself: the [`Runner`]
//! sends each job to the first backend whose memory holds the job's peak estimate, and refuses
//! the job, naming the memory it needs, when none does.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use exhaust_core::project::Project;
use exhaust_core::render::{RenderInfo, RenderProgress, Scene, render};
use exhaust_core::sensitivity::{Sensitivity, sensitivity};
use exhaust_core::solve::{CycleProgress, PointOutcome, solve_point_with};
use serde::{Deserialize, Serialize};

/// Work to do.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Job {
    /// A time-domain operating point.
    Point { project: Box<Project>, rpm: f64 },
    /// A scene marched by the time-domain solver.
    Render { project: Box<Project>, scene: Scene },
    /// Every estimated or derived input swept across its range at one engine speed.
    Sensitivity { project: Box<Project>, rpm: f64 },
    /// A program in a container image (the offline field solvers).
    Container(ContainerJob),
}

/// A program run in a container image with files in and files out.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContainerJob {
    /// Image pinned by digest, `name@sha256:…`.
    pub image: String,
    pub args: Vec<String>,
    /// Files written to the working directory before the run: name and contents.
    pub inputs: Vec<(String, Vec<u8>)>,
    /// Files the run leaves in the working directory that make up its result.
    pub outputs: Vec<String>,
    /// Peak memory the program needs, bytes (from its degrees of freedom).
    pub memory_bytes: u64,
}

/// What a job made.
#[derive(Clone, Debug, PartialEq)]
pub enum Output {
    Point(PointOutcome),
    Render {
        info: Box<RenderInfo>,
        /// Pressure of each listener channel, Pa.
        channels: Vec<Vec<f32>>,
    },
    Sensitivity(Box<Sensitivity>),
    /// Output files: name and contents.
    Files(Vec<(String, Vec<u8>)>),
}

/// A job in progress.
#[derive(Clone, Copy, Debug)]
pub enum Progress {
    Cycle(CycleProgress),
    Render(RenderProgress),
    /// Runs done of runs in all.
    Runs(usize, usize),
}

/// Stopped by its progress callback: nothing to cache or report.
pub const STOPPED: &str = "stopped";

impl Job {
    /// Cache key: BLAKE3 of the job kind, the solver that makes it and what its result depends
    /// on.
    pub fn key(&self) -> String {
        let body = match self {
            Job::Point { project, rpm } => {
                format!("point|{}|{}|{rpm}", exhaust_core::SOURCE, project.hash())
            }
            Job::Render { project, scene } => format!(
                "render|{}|{}|{}",
                exhaust_core::SOURCE,
                project.hash(),
                serde_json::to_string(scene).expect("scenes serialise")
            ),
            // Unlike a solve, the sweep depends on the input records.
            Job::Sensitivity { project, rpm } => format!(
                "sensitivity|{}|{}|{}|{rpm}",
                exhaust_core::SOURCE,
                project.hash(),
                serde_json::to_string(&project.inputs).expect("records serialise")
            ),
            Job::Container(c) => {
                let mut h = blake3::Hasher::new();
                for (name, bytes) in &c.inputs {
                    h.update(name.as_bytes());
                    h.update(&(bytes.len() as u64).to_le_bytes());
                    h.update(bytes);
                }
                format!(
                    "container|{}|{:?}|{:?}|{}",
                    c.image,
                    c.args,
                    c.outputs,
                    h.finalize()
                )
            }
        };
        blake3::hash(body.as_bytes()).to_hex().to_string()
    }

    /// Peak memory the job needs, bytes. Native jobs: the network and its buffers (tens of
    /// megabytes) plus a render's recorded outlet flow and listener channels.
    pub fn memory_bytes(&self) -> u64 {
        const NETWORK: u64 = 64 << 20;
        match self {
            Job::Point { .. } => NETWORK,
            Job::Render { scene, .. } => {
                // Outlet flow (f64) and each channel (f64 while made, f32 kept), at most eight each.
                let samples =
                    (scene.duration_s.max(0.0) * exhaust_core::render::SAMPLE_RATE) as u64;
                NETWORK + samples * 8 * (8 + 8 + 4)
            }
            // One network per worker thread.
            Job::Sensitivity { .. } => {
                NETWORK * std::thread::available_parallelism().map_or(1, |n| n.get()) as u64
            }
            Job::Container(c) => c.memory_bytes,
        }
    }
}

/// Somewhere jobs run.
pub trait Backend: Send + Sync {
    fn name(&self) -> &str;
    /// Memory a job may take here without the host swapping, bytes.
    fn capacity_bytes(&self) -> u64;
    /// Runs `job`, reporting to `progress`, which stops it by returning `false` (the run then
    /// fails with [`STOPPED`]).
    fn run(&self, job: &Job, progress: &mut dyn FnMut(Progress) -> bool) -> Result<Output, String>;
}

/// This machine: native jobs in this process, container jobs through Docker.
pub struct Local {
    capacity: u64,
}

impl Local {
    /// Four fifths of physical memory, the rest left to the system and the app.
    pub fn new() -> Self {
        Self {
            capacity: physical_memory().unwrap_or(0) / 5 * 4,
        }
    }
}

impl Default for Local {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for Local {
    fn name(&self) -> &str {
        "this machine"
    }

    fn capacity_bytes(&self) -> u64 {
        self.capacity
    }

    fn run(&self, job: &Job, progress: &mut dyn FnMut(Progress) -> bool) -> Result<Output, String> {
        let mut stopped = false;
        match job {
            Job::Point { project, rpm } => {
                let run = solve_point_with(project, *rpm, &mut |p| {
                    stopped = !progress(Progress::Cycle(p));
                    !stopped
                });
                match run {
                    Err(_) if stopped => Err(STOPPED.into()),
                    Ok(r) => Ok(Output::Point(PointOutcome::Solved(Box::new(r)))),
                    Err(e) => Ok(Output::Point(PointOutcome::Failed {
                        rpm: *rpm,
                        error: e.to_string(),
                    })),
                }
            }
            Job::Render { project, scene } => {
                let r = render(project, scene, &mut |p| {
                    stopped = !progress(Progress::Render(p));
                    !stopped
                });
                match r {
                    Err(_) if stopped => Err(STOPPED.into()),
                    Err(e) => Err(e.to_string()),
                    Ok(r) => Ok(Output::Render {
                        info: Box::new(r.info),
                        channels: r.channels,
                    }),
                }
            }
            Job::Sensitivity { project, rpm } => {
                // The sweep reports from its worker threads; `progress` stays on this one.
                let (done, total, stop) = (
                    AtomicUsize::new(0),
                    AtomicUsize::new(1),
                    AtomicBool::new(false),
                );
                let result = std::thread::scope(|s| {
                    let sweep = s.spawn(|| {
                        sensitivity(project, *rpm, &|d, t| {
                            done.fetch_max(d, Ordering::Relaxed);
                            total.store(t, Ordering::Relaxed);
                            !stop.load(Ordering::Relaxed)
                        })
                    });
                    while !sweep.is_finished() {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        let p = Progress::Runs(
                            done.load(Ordering::Relaxed),
                            total.load(Ordering::Relaxed),
                        );
                        if !progress(p) {
                            stop.store(true, Ordering::Relaxed);
                        }
                    }
                    sweep.join().expect("the sweep does not panic")
                });
                match result {
                    Err(_) if stop.load(Ordering::Relaxed) => Err(STOPPED.into()),
                    Err(e) => Err(e.to_string()),
                    Ok(r) => Ok(Output::Sensitivity(Box::new(r))),
                }
            }
            Job::Container(c) => run_container(c),
        }
    }
}

/// Runs `c` in Docker with a fresh working directory mounted at `/work`.
fn run_container(c: &ContainerJob) -> Result<Output, String> {
    if Command::new("docker").arg("--version").output().is_err() {
        return Err("Docker is not installed on this machine".into());
    }
    let work = std::env::temp_dir().join(format!(
        "exhaust-job-{}",
        &Job::Container(c.clone()).key()[..16]
    ));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    for (name, bytes) in &c.inputs {
        std::fs::write(work.join(name), bytes).map_err(|e| format!("{name}: {e}"))?;
    }
    let out = Command::new("docker")
        .args(["run", "--rm", "-v"])
        .arg(format!("{}:/work", work.display()))
        .args(["-w", "/work", &c.image])
        .args(&c.args)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "{} failed: {}",
            c.image,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let files = c
        .outputs
        .iter()
        .map(|name| {
            std::fs::read(work.join(name))
                .map(|b| (name.clone(), b))
                .map_err(|e| format!("{name}: {e}"))
        })
        .collect::<Result<_, _>>()?;
    let _ = std::fs::remove_dir_all(&work);
    Ok(Output::Files(files))
}

/// Physical memory of this machine, bytes.
fn physical_memory() -> Option<u64> {
    if cfg!(target_os = "macos") {
        let out = Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    } else {
        let info = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kb: u64 = info
            .lines()
            .find(|l| l.starts_with("MemTotal:"))?
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()?;
        Some(kb * 1024)
    }
}

/// A remote cloud host. Not built yet: every job it is given fails.
pub struct Cloud;

impl Backend for Cloud {
    fn name(&self) -> &str {
        "cloud"
    }

    fn capacity_bytes(&self) -> u64 {
        0
    }

    fn run(&self, _: &Job, _: &mut dyn FnMut(Progress) -> bool) -> Result<Output, String> {
        Err("the cloud backend is not built yet".into())
    }
}

/// Results on disk, one directory per key.
pub struct Cache {
    dir: PathBuf,
}

/// What a cached result's `result.json` holds besides its binary files.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Stored {
    Point {
        outcome: PointOutcome,
    },
    Sensitivity {
        result: Box<Sensitivity>,
    },
    Render {
        info: serde_json::Value,
        channels: usize,
        samples: usize,
    },
    Files {
        names: Vec<String>,
    },
}

impl Cache {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn get(&self, key: &str) -> Option<Output> {
        let dir = self.dir.join(key);
        let stored: Stored =
            serde_json::from_slice(&std::fs::read(dir.join("result.json")).ok()?).ok()?;
        Some(match stored {
            Stored::Point { outcome } => Output::Point(outcome),
            Stored::Sensitivity { result } => Output::Sensitivity(result),
            Stored::Render {
                info,
                channels,
                samples,
            } => {
                let bytes = std::fs::read(dir.join("channels.f32")).ok()?;
                if bytes.len() != 4 * channels * samples {
                    return None;
                }
                let values: Vec<f32> = bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| f32::from_le_bytes(*b))
                    .collect();
                Output::Render {
                    info: serde_json::from_value(info).ok()?,
                    channels: values.chunks(samples.max(1)).map(<[f32]>::to_vec).collect(),
                }
            }
            Stored::Files { names } => Output::Files(
                names
                    .into_iter()
                    .map(|n| std::fs::read(dir.join("files").join(&n)).map(|b| (n, b)))
                    .collect::<Result<_, _>>()
                    .ok()?,
            ),
        })
    }

    /// Stores `output` under `key`: written aside, then moved into place, so a reader never
    /// sees half a result.
    pub fn put(&self, key: &str, output: &Output) -> std::io::Result<()> {
        let staging = self.dir.join(format!(".{key}.{}", std::process::id()));
        std::fs::create_dir_all(&staging)?;
        let stored = match output {
            Output::Point(outcome) => Stored::Point {
                outcome: outcome.clone(),
            },
            Output::Sensitivity(result) => Stored::Sensitivity {
                result: result.clone(),
            },
            Output::Render { info, channels } => {
                let samples = channels.first().map_or(0, Vec::len);
                let bytes: Vec<u8> = channels
                    .iter()
                    .flatten()
                    .flat_map(|s| s.to_le_bytes())
                    .collect();
                std::fs::write(staging.join("channels.f32"), bytes)?;
                Stored::Render {
                    info: serde_json::to_value(info).map_err(std::io::Error::other)?,
                    channels: channels.len(),
                    samples,
                }
            }
            Output::Files(files) => {
                std::fs::create_dir_all(staging.join("files"))?;
                for (name, bytes) in files {
                    std::fs::write(staging.join("files").join(name), bytes)?;
                }
                Stored::Files {
                    names: files.iter().map(|f| f.0.clone()).collect(),
                }
            }
        };
        std::fs::write(
            staging.join("result.json"),
            serde_json::to_vec(&stored).map_err(std::io::Error::other)?,
        )?;
        let target = self.dir.join(key);
        let _ = std::fs::remove_dir_all(&target);
        std::fs::rename(&staging, &target)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

/// Runs jobs on the first backend that holds them, through the cache.
pub struct Runner {
    pub backends: Vec<Box<dyn Backend>>,
    pub cache: Cache,
}

impl Runner {
    /// This machine only, caching under `dir`.
    pub fn local(dir: impl Into<PathBuf>) -> Self {
        Self {
            backends: vec![Box::new(Local::new())],
            cache: Cache::new(dir),
        }
    }

    /// The cached result of `job`, or `job` run on the first backend with the memory for it.
    pub fn run(
        &self,
        job: &Job,
        progress: &mut dyn FnMut(Progress) -> bool,
    ) -> Result<Output, String> {
        let key = job.key();
        if let Some(out) = self.cache.get(&key) {
            return Ok(out);
        }
        let need = job.memory_bytes();
        let backend = self
            .backends
            .iter()
            .find(|b| b.capacity_bytes() >= need)
            .ok_or_else(|| {
                format!(
                    "needs a host with at least {:.1} GB free for the job; the largest has {:.1} GB",
                    need as f64 / 1e9,
                    self.backends
                        .iter()
                        .map(|b| b.capacity_bytes())
                        .max()
                        .unwrap_or(0) as f64
                        / 1e9
                )
            })?;
        let out = backend.run(job, progress)?;
        // A result that cannot be stored is still a result.
        let _ = self.cache.put(&key, &out);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STRAIGHT_PIPE: &str = include_str!("../../../tests/cases/straight_pipe.json");

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("exhaust-jobs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A point runs once: the second request is the cached result, bit for bit.
    #[test]
    fn a_point_runs_once_then_comes_from_the_cache() {
        let dir = scratch("point");
        let runner = Runner::local(&dir);
        let job = Job::Point {
            project: Box::new(Project::from_json(STRAIGHT_PIPE).unwrap()),
            rpm: 3000.0,
        };
        let mut cycles = 0;
        let first = runner
            .run(&job, &mut |_| {
                cycles += 1;
                true
            })
            .unwrap();
        assert!(cycles > 0);
        let second = runner.run(&job, &mut |_| panic!("ran again")).unwrap();
        assert_eq!(first, second);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A stopped job fails and leaves nothing in the cache, a point or a sweep whose progress
    /// comes from other threads.
    #[test]
    fn a_stopped_job_is_not_cached() {
        let dir = scratch("stopped");
        let runner = Runner::local(&dir);
        let project = Box::new(Project::from_json(STRAIGHT_PIPE).unwrap());
        for job in [
            Job::Point {
                project: project.clone(),
                rpm: 3000.0,
            },
            Job::Sensitivity {
                project,
                rpm: 3000.0,
            },
        ] {
            assert_eq!(runner.run(&job, &mut |_| false), Err(STOPPED.to_string()));
            assert!(runner.cache.get(&job.key()).is_none());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A render runs once and comes back from the cache channel for channel.
    #[test]
    fn a_render_runs_once_then_comes_from_the_cache() {
        let dir = scratch("render");
        let runner = Runner::local(&dir);
        let job = Job::Render {
            project: Box::new(Project::from_json(STRAIGHT_PIPE).unwrap()),
            scene: serde_json::from_str(
                r#"{ "duration_s": 0.1, "rpm": [[0, 3000]], "listener": { "kind": "stereo", "ear_spacing_mm": 175, "turn_deg": 90 } }"#,
            )
            .unwrap(),
        };
        let first = runner.run(&job, &mut |_| true).unwrap();
        let Output::Render { channels, .. } = &first else {
            panic!("a render");
        };
        assert_eq!((channels.len(), channels[0].len()), (2, 4800));
        let second = runner.run(&job, &mut |_| panic!("ran again")).unwrap();
        assert_eq!(first, second);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A job too big for every backend is refused with the memory it needs.
    #[test]
    fn oversized_jobs_are_refused() {
        let dir = scratch("oversized");
        let runner = Runner {
            backends: vec![Box::new(Cloud)],
            cache: Cache::new(&dir),
        };
        let job = Job::Container(ContainerJob {
            image: "solvers@sha256:0".into(),
            args: vec![],
            inputs: vec![],
            outputs: vec![],
            memory_bytes: 40 << 30,
        });
        let e = runner.run(&job, &mut |_| true).unwrap_err();
        assert!(e.starts_with("needs a host with at least 42.9 GB"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The key follows the project's solved content and the job's parameters.
    #[test]
    fn keys_follow_what_the_result_depends_on() {
        let project = Project::from_json(STRAIGHT_PIPE).unwrap();
        let point = |p: &Project, rpm| Job::Point {
            project: Box::new(p.clone()),
            rpm,
        };
        let mut renamed = project.clone();
        renamed.name = "another name".into();
        let mut longer = project.clone();
        longer.solver.dx_mm = 10.0;
        let base = point(&project, 3000.0).key();
        assert_eq!(base, point(&renamed, 3000.0).key());
        assert_ne!(base, point(&project, 3050.0).key());
        assert_ne!(base, point(&longer, 3000.0).key());
    }
}
