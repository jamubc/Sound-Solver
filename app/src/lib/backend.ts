// Typed calls into the Rust backend (app/src-tauri/src/lib.rs); every number the UI shows
// comes back through here.
import { Channel, invoke } from '@tauri-apps/api/core';
import type {
  Clearance,
  Layout,
  Manifest,
  Metrics,
  OrderDifference,
  OrderTracks,
  Package,
  PointOutcome,
  SweepResult,
  Tuning,
} from './types/api';
import type { CabinTf, Project } from './types/project';

/** A scan mesh in scan units: xyz per vertex, three vertex indices per triangle. */
export interface ScanMesh {
  file: string;
  positions: Float32Array;
  index: Uint32Array;
}

export const backend = {
  stockProject: () => invoke<Project>('stock_project'),
  openProject: (path: string) => invoke<Project>('open_project', { path }),
  saveProject: (path: string, project: Project) => invoke<void>('save_project', { path, project }),
  /** Validates the project; resolves to its layout or rejects with the reason. */
  check: (project: Project) => invoke<Layout>('check', { project }),
  manifests: () => invoke<Manifest[]>('manifests'),
  /** Places a new element on a route, `sMm` along it; rejects with the reason if it cannot. */
  insertElement: (project: Project, route: string, sMm: number, kind: string) =>
    invoke<{ project: Project; id: string }>('insert_element', { project, route, sMm, kind }),
  /** Takes a two-port element out, joining its pipes. */
  removeElement: (project: Project, id: string) => invoke<Project>('remove_element', { project, id }),
  preview: (project: Project) => invoke<SweepResult>('preview', { project }),
  solve(project: Project, onPoint: (point: PointOutcome) => void) {
    const channel = new Channel<PointOutcome>();
    channel.onmessage = onPoint;
    return invoke<SweepResult>('solve', { project, onPoint: channel });
  },
  cancel: () => invoke<void>('cancel'),
  /** Straights, bends, stock sticks and joints of the project as drawn. */
  fabrication: (project: Project) => invoke<Package>('fabrication', { project }),
  /** Writes the fabrication files beside the project file `path`; resolves to their paths. */
  exportPackage: (project: Project, path: string) => invoke<string[]>('export_package', { project, path }),
  async scanMesh(file: string): Promise<ScanMesh> {
    const bytes = await invoke<ArrayBuffer>('scan_mesh', { file });
    const [vertices, triangles] = new Uint32Array(bytes, 0, 2);
    return {
      file,
      positions: new Float32Array(bytes, 8, 3 * vertices),
      index: new Uint32Array(bytes, 8 + 12 * vertices, 3 * triangles),
    };
  },
  /** Pipe clearance to the project's scan; rejects with the reason if it cannot be placed. */
  clearance: (project: Project) => invoke<Clearance>('clearance', { project }),
  /** The stub length or Helmholtz volume that puts the branch's resonance at `targetHz`. */
  tune: (project: Project, element: string, targetHz: number, rpm: number, statedK: [number, number] | null) =>
    invoke<Tuning>('tune', { project, element, targetHz, rpm, statedK }),
  /** Engine-order tracks of the project's recording `index`. */
  orderTracks: (project: Project, index: number) => invoke<OrderTracks>('order_tracks', { project, index }),
  cabinTfOrders: (project: Project, exterior: number, interior: number) =>
    invoke<CabinTf>('cabin_tf_orders', { project, exterior, interior }),
  cabinTfImpulse: (exterior: string, interior: string) => invoke<CabinTf>('cabin_tf_impulse', { exterior, interior }),
  /** Metrics of solved points under the project's measurements as they are now. */
  evaluate: (project: Project, points: PointOutcome[]) => invoke<Metrics>('evaluate', { project, points }),
  /** Engine orders of `b` against `a`. */
  compare: (project: Project, a: PointOutcome[], b: PointOutcome[]) =>
    invoke<OrderDifference[]>('compare', { project, a, b }),
};
