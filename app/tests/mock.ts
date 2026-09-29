// Stands in for the Tauri backend in a plain browser: every command answers with what the
// real core returned for the reference project (fixtures from `npm run fixtures`). The file
// dialog picks a synthetic underbody scan.
import { readFileSync } from 'node:fs';
import type { Page } from '@playwright/test';

const fixture = (name: string): unknown =>
  JSON.parse(readFileSync(new URL(`./fixtures/${name}.json`, import.meta.url), 'utf8'));

export async function mockBackend(page: Page) {
  const answers = {
    project: fixture('project'),
    layout: fixture('layout'),
    manifests: fixture('manifests'),
    preview: fixture('preview'),
    fabrication: fixture('fabrication'),
    solve: fixture('solve') as { points: unknown[] },
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
        a.solve.points.forEach((point, index) => channel({ index, message: point }));
        channel({ index: a.solve.points.length, end: true });
        return a.solve;
      },
      fabrication: () => a.fabrication,
      'plugin:dialog|open': () => '/scans/floor.stl',
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
