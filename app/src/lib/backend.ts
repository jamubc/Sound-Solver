// Typed calls into the Rust backend (app/src-tauri/src/lib.rs); every number the UI shows
// comes back through here.
import { Channel, invoke } from '@tauri-apps/api/core';
import type { Layout, Manifest, PointOutcome, SweepResult } from './types/api';
import type { Project } from './types/project';

export const backend = {
  stockProject: () => invoke<Project>('stock_project'),
  openProject: (path: string) => invoke<Project>('open_project', { path }),
  saveProject: (path: string, project: Project) => invoke<void>('save_project', { path, project }),
  /** Validates the project; resolves to its layout or rejects with the reason. */
  check: (project: Project) => invoke<Layout>('check', { project }),
  manifests: () => invoke<Manifest[]>('manifests'),
  preview: (project: Project) => invoke<SweepResult>('preview', { project }),
  solve(project: Project, onPoint: (point: PointOutcome) => void) {
    const channel = new Channel<PointOutcome>();
    channel.onmessage = onPoint;
    return invoke<SweepResult>('solve', { project, onPoint: channel });
  },
  cancel: () => invoke<void>('cancel'),
};
