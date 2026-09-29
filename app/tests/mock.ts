// Stands in for the Tauri backend in a plain browser: every command answers with what the
// real core returned for the reference project (fixtures from `npm run fixtures`).
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
