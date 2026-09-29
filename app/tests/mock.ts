// Stands in for the Tauri backend in a plain browser: every command answers with what the
// real core returned for the reference project (fixtures from `npm run fixtures`), or for the
// fixture `project` names. The file dialog picks a synthetic scan, recording or log.
import { readFileSync } from 'node:fs';
import type { Page } from '@playwright/test';

const fixture = (name: string): unknown =>
  JSON.parse(readFileSync(new URL(`./fixtures/${name}.json`, import.meta.url), 'utf8'));

export async function mockBackend(page: Page, project: 'project' | 'project-stub' = 'project') {
  const answers = {
    project: fixture(project),
    layout: fixture(project === 'project' ? 'layout' : 'layout-stub'),
    manifests: fixture('manifests'),
    preview: fixture('preview') as { metrics: unknown },
    fabrication: fixture('fabrication'),
    solve: fixture('solve') as { points: unknown[] },
    tune: fixture('tune'),
    tracks: fixture('tracks'),
    cabinTf: fixture('cabin-tf'),
    compare: fixture('compare'),
    render: fixture('render') as { render: unknown },
    sensitivity: fixture('sensitivity'),
  };
  await page.addInitScript((a) => {
    const callbacks = new Map<number, (message: unknown) => void>();
    let next = 1;
    const replies: Record<string, (args: Record<string, unknown>) => unknown> = {
      stock_project: () => a.project,
      manifests: () => a.manifests,
      check: () => a.layout,
      preview: () => a.preview,
      cancel: () => null,
      solve: (args) => {
        const channel = callbacks.get((args.onPoint as { id: number }).id)!;
        const cycles = callbacks.get((args.onCycle as { id: number }).id)!;
        a.solve.points.forEach((point, index) => {
          const rpm = (point as { rpm: number }).rpm;
          cycles({ index, message: { rpm, cycle: 1, residual: null } });
          channel({ index, message: point });
        });
        channel({ index: a.solve.points.length, end: true });
        cycles({ index: a.solve.points.length, end: true });
        return a.solve;
      },
      fabrication: () => a.fabrication,
      'plugin:dialog|open': (args) => {
        const { filters } = args.options as { filters?: { extensions: string[] }[] };
        const kinds = filters?.[0]?.extensions ?? [];
        return kinds.includes('stl') ? '/scans/floor.stl' : kinds.includes('wav') ? '/recordings/run.wav' : '/recordings/obd.csv';
      },
      tune: () => a.tune,
      order_tracks: () => a.tracks,
      cabin_tf_orders: () => a.cabinTf,
      cabin_tf_impulse: () => a.cabinTf,
      evaluate: () => a.preview.metrics,
      compare: () => a.compare,
      // One second of a 70 Hz tone at 1 Pa peak: the header, then the samples.
      listen: () => {
        const n = 22050;
        const bytes = new ArrayBuffer(20 + 4 * n);
        const header = new DataView(bytes);
        header.setFloat64(0, 22050, true);
        header.setFloat64(8, 1, true);
        header.setUint32(16, 0, true);
        new Float32Array(bytes, 20, n).set(Array.from({ length: n }, (_, i) => Math.sin((2 * Math.PI * 70 * i) / 22050)));
        return bytes;
      },
      // The render's provenance, then half a second of a 100 Hz tone at 1 Pa peak.
      render: (args) => {
        const progress = callbacks.get((args.onProgress as { id: number }).id)!;
        progress({ index: 0, message: { stage: 'settle', cycle: 1, residual: null } });
        progress({ index: 1, message: { stage: 'march', fraction: 0.5 } });
        progress({ index: 2, end: true });
        const utf8 = new TextEncoder().encode(JSON.stringify(a.render.render));
        const info = new Uint8Array(Math.ceil(utf8.length / 4) * 4).fill(32);
        info.set(utf8);
        const n = 24000;
        const bytes = new ArrayBuffer(12 + info.length + 4 * n);
        const view = new DataView(bytes);
        view.setUint32(0, info.length, true);
        new Uint8Array(bytes, 4, info.length).set(info);
        view.setUint32(4 + info.length, 1, true);
        view.setUint32(8 + info.length, n, true);
        new Float32Array(bytes, 12 + info.length, n).set(Array.from({ length: n }, (_, i) => Math.sin((2 * Math.PI * 100 * i) / 48000)));
        return bytes;
      },
      cancel_render: () => null,
      sensitivity: (args) => {
        const progress = callbacks.get((args.onProgress as { id: number }).id)!;
        progress({ index: 0, message: [1, 57] });
        progress({ index: 1, end: true });
        return a.sensitivity;
      },
      // Two benchmark cases, sent as they finish.
      verify: (args) => {
        const send = callbacks.get((args.onCase as { id: number }).id)!;
        const reports = [
          { case: 'sod', subsystem: 'propagation', checks: [{ case: 'Sod shock tube', metric: 'density error', value: 0.004, limit: 0.02, pass: true }], error: null },
          { case: 'outlet', subsystem: 'radiation', checks: [{ case: 'Levine–Schwinger outlet', metric: '|R| error', value: 0.001, limit: 0.01, pass: true }], error: null },
        ];
        reports.forEach((message, index) => send({ index, message }));
        send({ index: reports.length, end: true });
        return reports;
      },
      // A flat underbody 200 mm above the flange, in metres: 4 vertices, 2 triangles.
      scan_mesh: () => {
        const v = [-4, -1, 0.2, 1, -1, 0.2, 1, 1, 0.2, -4, 1, 0.2];
        const bytes = new ArrayBuffer(8 + 4 * v.length + 4 * 6);
        new Uint32Array(bytes, 0, 2).set([4, 2]);
        new Float32Array(bytes, 8, v.length).set(v);
        new Uint32Array(bytes, 8 + 4 * v.length, 6).set([0, 1, 2, 0, 2, 3]);
        return bytes;
      },
      // The core's answer until three reference points are picked.
      clearance: () => {
        throw 'scan reference points are (nearly) in a line';
      },
    };
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
      transformCallback: (callback: (message: unknown) => void) => {
        callbacks.set(next, callback);
        return next++;
      },
      unregisterCallback: (id: number) => callbacks.delete(id),
      invoke: async (cmd: string, args: Record<string, unknown>) => {
        const reply = replies[cmd];
        if (!reply) throw new Error(`no mock for ${cmd}`);
        return structuredClone(reply(args));
      },
      metadata: { currentWindow: { label: 'main' }, currentWebview: { windowLabel: 'main', label: 'main' } },
    };
  }, answers);
}
