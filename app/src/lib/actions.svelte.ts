// Commands shared by the ribbon, the keyboard and the panels. Failures surface as a notice in
// the status bar.
import { open, save } from '@tauri-apps/plugin-dialog';
import { backend } from './backend';
import { app, commit, edit, editFabrication, load, manifestOf } from './state.svelte';
import type { Project } from './types/project';

type Vec3 = [number, number, number];

export const notice = $state({ text: null as string | null, bad: false });
let clearing: ReturnType<typeof setTimeout> | undefined;

/** A message in the status bar; failures stay until the next one. */
export function tell(text: string, bad = false) {
  clearTimeout(clearing);
  Object.assign(notice, { text, bad });
  if (!bad) clearing = setTimeout(() => (notice.text = null), 6000);
}

async function attempt(action: () => Promise<void>) {
  try {
    await action();
  } catch (e) {
    tell(String(e), true);
  }
}

const filters = [{ name: 'Exhaust project', extensions: ['json'] }];

export const reference = () => attempt(async () => load(await backend.stockProject(), null));

export const openProject = () =>
  attempt(async () => {
    const path = await open({ filters, multiple: false, directory: false });
    if (typeof path === 'string') {
      load(await backend.openProject(path), path);
      tell(`Opened ${path}`);
    }
  });

export const saveProject = (as = false) =>
  attempt(async () => {
    if (!app.project) return;
    const target = !as && app.path ? app.path : await save({ filters, defaultPath: `${app.project.name}.json` });
    if (!target) return;
    await backend.saveProject(target, $state.snapshot(app.project) as Project);
    app.path = target;
    app.dirty = false;
    tell(`Saved ${target}`);
  });

const routeOf = (p: Project, id: string) => p.system.routes.find((r) => r.id === id)!;

/** Straight runs of a route's centreline, `[start, end]` mm along it. */
export function straights(routeId: string): [number, number][] {
  return (app.layout?.routes.find((r) => r.id === routeId)?.pieces ?? []).flatMap((p) =>
    p.kind === 'straight' ? [[p.s0_mm, p.s0_mm + Math.hypot(...p.to.map((x, k) => x - p.from[k]))] as [number, number]] : [],
  );
}

/** Where along a route to act: the spot picked on it, else the middle of its longest straight. */
export function spot(routeId: string): number {
  const s = app.selection;
  if (s?.kind === 'route' && s.id === routeId && s.s_mm !== undefined) return Math.round(s.s_mm);
  const longest = straights(routeId).reduce((a, b) => (b[1] - b[0] > a[1] - a[0] ? b : a), [0, 0]);
  return Math.round((longest[0] + longest[1]) / 2);
}

/** The route the selection is on, if any. */
export function selectedRoute(): string | null {
  const s = app.selection;
  return s?.kind === 'route' ? s.id : s?.kind === 'vertex' ? s.route : null;
}

/** Splits the route's last segment with a new bend point. */
export function addVia(routeId: string) {
  edit((p) => {
    const r = routeOf(p, routeId);
    const pts = app.layout?.routes.find((l) => l.id === routeId)?.points;
    if (!pts) return;
    const [a, b] = [pts[pts.length - 2], pts[pts.length - 1]];
    const mid = [0, 1, 2].map((k) => Math.round((a[k] + b[k]) / 2)) as Vec3;
    r.via_mm = [...(r.via_mm ?? []), mid];
    r.bend_radius_mm = [...(r.bend_radius_mm ?? []), null].slice(0, r.via_mm.length);
  });
  app.selection = { kind: 'vertex', route: routeId, index: (routeOf(app.project!, routeId).via_mm?.length ?? 1) - 1 };
}

export function removeVia(routeId: string, i: number) {
  edit((p) => {
    const r = routeOf(p, routeId);
    r.via_mm = r.via_mm!.filter((_, k) => k !== i);
    r.bend_radius_mm = r.bend_radius_mm?.filter((_, k) => k !== i);
  });
  app.selection = { kind: 'route', id: routeId };
}

export function setHangers(routeId: string, change: (hangers: number[]) => void) {
  editFabrication((p) => {
    const r = routeOf(p, routeId);
    const hangers = [...(r.hangers_mm ?? [])];
    change(hangers);
    r.hangers_mm = hangers.sort((a, b) => a - b);
  });
}

export const addHanger = (routeId: string, sMm = spot(routeId)) => setHangers(routeId, (hs) => hs.push(sMm));

/** Element types that sit in a pipe run: one inlet, one outlet. */
export const inline = () => app.manifests.filter((m) => m.ports.join() === 'in,out');

export const placeElement = (routeId: string, kind: string, sMm = spot(routeId)) =>
  attempt(async () => {
    const done = await backend.insertElement($state.snapshot(app.project) as Project, routeId, sMm, kind);
    commit(done.project, { kind: 'element', id: done.id });
    tell(`Placed ${done.id} on ${routeId} at ${sMm} mm`);
  });

export const removeElement = (id: string) =>
  attempt(async () => {
    commit(await backend.removeElement($state.snapshot(app.project) as Project, id), null);
    tell(`Removed ${id}: its pipes joined`);
  });

const zeros = (): [Vec3, Vec3, Vec3] => [
  [0, 0, 0],
  [0, 0, 0],
  [0, 0, 0],
];

/** Loads an underbody scan; a new one keeps the jack-pad coordinates and is picked afresh. */
export const loadScan = () =>
  attempt(async () => {
    const path = await open({ filters: [{ name: 'Underbody scan', extensions: ['stl', 'obj'] }], multiple: false, directory: false });
    if (typeof path !== 'string') return;
    editFabrication((p) => {
      p.fabrication ??= {};
      p.fabrication.scan = {
        path,
        unit_mm: p.fabrication.scan?.unit_mm ?? 1000,
        scan_points: zeros(),
        vehicle_points_mm: p.fabrication.scan?.vehicle_points_mm ?? zeros(),
      };
    });
    tell('Scan loaded: pick its three reference points on it');
  });

/** Adds a WAV or CAF recording of the car, as heard outside unless changed. */
export const addRecording = () =>
  attempt(async () => {
    const path = await open({ filters: [{ name: 'Recording', extensions: ['wav', 'caf'] }], multiple: false, directory: false });
    if (typeof path !== 'string') return;
    editFabrication((p) => {
      p.measurements ??= {};
      p.measurements.recordings ??= [];
      p.measurements.recordings.push({ path, position: 'exterior', log_offset_s: 0 });
    });
  });

/** Deletes what is selected: a bend point, or an element that sits in a pipe run. */
export function deleteSelected() {
  const s = app.selection;
  if (s?.kind === 'vertex') removeVia(s.route, s.index);
  else if (s?.kind === 'element') {
    const e = app.project?.system.elements.find((x) => x.id === s.id);
    if (e && manifestOf(e.type)?.ports.join() === 'in,out') void removeElement(s.id);
    else tell(`${s.id} joins more than two pipes; it cannot be taken out`, true);
  }
}

export const canDelete = () => {
  const s = app.selection;
  if (s?.kind === 'vertex') return true;
  if (s?.kind !== 'element') return false;
  const e = app.project?.system.elements.find((x) => x.id === s.id);
  return !!e && manifestOf(e.type)?.ports.join() === 'in,out';
};
