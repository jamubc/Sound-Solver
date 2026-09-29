// Application state. The project is the schema's JSON, edited in place; after every edit the
// backend re-validates and lays it out, and the four-pole preview reruns. Time-domain results
// belong to the project they were solved for, so an edit clears them.
import { backend, type ScanMesh } from './backend';
import type { Clearance, Layout, Manifest, PointOutcome, PointResult, SweepResult } from './types/api';
import type { Project } from './types/project';

export type Selection =
  | { kind: 'element'; id: string }
  /** A route, and where along its centreline it was picked, mm. */
  | { kind: 'route'; id: string; s_mm?: number }
  /** A via point of a route (index into `via_mm`). */
  | { kind: 'vertex'; route: string; index: number };

interface TimeDomain {
  running: boolean;
  points: PointOutcome[];
  result: SweepResult | null;
  error: string | null;
  total: number;
}

const idle = (): TimeDomain => ({ running: false, points: [], result: null, error: null, total: 0 });

/** Colours of the three scan reference points, in the panel and the viewport. */
export const REFERENCE_COLOURS = ['#4fd1c5', '#ff79c6', '#c792ea'];

export const app = $state({
  project: null as Project | null,
  path: null as string | null,
  dirty: false,
  manifests: [] as Manifest[],
  layout: null as Layout | null,
  /** Why the current project cannot be solved; the last valid layout stays on screen. */
  invalid: null as string | null,
  preview: null as SweepResult | null,
  previewMs: null as number | null,
  previewError: null as string | null,
  timeDomain: idle(),
  selection: null as Selection | null,
  /** Engine speed whose spectrum is shown. */
  rpm: null as number | null,
  scanMesh: null as ScanMesh | null,
  scanError: null as string | null,
  clearance: null as Clearance | null,
  /** Why the scan cannot be placed or measured. */
  clearanceError: null as string | null,
  /** The scan reference point (0–2) the next click on the scan sets. */
  picking: null as number | null,
});

let timer: ReturnType<typeof setTimeout> | undefined;
let refreshRun = 0;
let solveRun = 0;

/** Re-validates, lays out and previews the project, after `delay` ms of quiet. */
export function refresh(delay = 150) {
  clearTimeout(timer);
  timer = setTimeout(runRefresh, delay);
}

async function runRefresh() {
  if (!app.project) return;
  const run = ++refreshRun;
  const project = $state.snapshot(app.project) as Project;
  try {
    const layout = await backend.check(project);
    if (run !== refreshRun) return;
    app.layout = layout;
    app.invalid = null;
  } catch (e) {
    if (run === refreshRun) {
      app.invalid = String(e);
      app.preview = null;
    }
    return;
  }
  void measureClearance(project, run);
  const start = performance.now();
  try {
    const preview = await backend.preview(project);
    if (run !== refreshRun) return;
    app.preview = preview;
    app.previewMs = performance.now() - start;
    app.previewError = null;
    app.rpm ??= defaultRpm(project);
  } catch (e) {
    if (run === refreshRun) {
      app.preview = null;
      app.previewError = String(e);
    }
  }
}

async function measureClearance(project: Project, run: number) {
  const scan = project.fabrication?.scan;
  if (!scan) app.scanMesh = null;
  else if (app.scanMesh?.file !== scan.path) void loadScanMesh(scan.path);
  let clearance: Clearance | null = null;
  let error: string | null = null;
  if (scan) {
    try {
      clearance = await backend.clearance(project);
    } catch (e) {
      error = String(e);
    }
  }
  if (run !== refreshRun) return;
  app.clearance = clearance;
  app.clearanceError = error;
}

let scanLoading: string | null = null;

async function loadScanMesh(file: string) {
  if (scanLoading === file) return;
  scanLoading = file;
  app.scanError = null;
  try {
    const mesh = await backend.scanMesh(file);
    if (app.project?.fabrication?.scan?.path === file) app.scanMesh = mesh;
  } catch (e) {
    app.scanError = String(e);
  } finally {
    scanLoading = null;
  }
}

function defaultRpm(project: Project) {
  const [lo, hi] = project.operating.cruise_band_rpm;
  const [start, , step] = project.operating.sweep_rpm;
  return start + Math.round(((lo + hi) / 2 - start) / step) * step;
}

export function load(project: Project, path: string | null) {
  if (app.timeDomain.running) void backend.cancel();
  solveRun++;
  app.project = project;
  app.path = path;
  app.dirty = false;
  app.selection = null;
  app.rpm = null;
  app.layout = null;
  app.preview = null;
  app.timeDomain = idle();
  app.scanMesh = null;
  app.clearance = null;
  app.clearanceError = null;
  app.picking = null;
  refresh(0);
}

/** Applies an edit; the previous time-domain results no longer describe the project. */
export function edit(change: (project: Project) => void) {
  if (!app.project) return;
  change(app.project);
  app.dirty = true;
  if (app.timeDomain.running) void backend.cancel();
  solveRun++;
  app.timeDomain = idle();
  refresh();
}

/** Applies an edit to fabrication data (stock, scan, hangers, joints), which no result depends on. */
export function editFabrication(change: (project: Project) => void) {
  if (!app.project) return;
  change(app.project);
  app.dirty = true;
  refresh();
}

/** Replaces the project with a structurally edited one (an element placed or removed). */
export function commit(project: Project, selection: Selection | null) {
  edit(() => (app.project = project));
  app.selection = selection;
}

export async function solveTimeDomain() {
  if (!app.project || app.invalid) return;
  const project = $state.snapshot(app.project) as Project;
  const run = ++solveRun;
  const [start, stop, step] = project.operating.sweep_rpm;
  app.timeDomain = { ...idle(), running: true, total: Math.floor((stop - start) / step + 1e-9) + 1 };
  try {
    const result = await backend.solve(project, (point) => {
      if (run === solveRun) app.timeDomain.points.push(point);
    });
    if (run === solveRun) app.timeDomain.result = result;
  } catch (e) {
    if (run === solveRun) app.timeDomain.error = String(e);
  } finally {
    if (run === solveRun) app.timeDomain.running = false;
  }
}

export function cancelTimeDomain() {
  void backend.cancel();
}

/** Solved points in engine-speed order. */
export function solved(points: readonly PointOutcome[]): PointResult[] {
  return points
    .filter((p): p is PointResult & { status: 'solved' } => p.status === 'solved')
    .sort((a, b) => a.rpm - b.rpm);
}

export function manifestOf(type: string) {
  return app.manifests.find((m) => m.type === type);
}
