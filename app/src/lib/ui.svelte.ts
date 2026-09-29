// Interface state: the workspace, what the dock shows, panel sizes and viewport display
// options. The project and its results live in state.svelte.ts. Sizes and display options are
// kept per viewer in local storage; the app works the same without it.

export type Workspace = 'design' | 'simulate' | 'measure' | 'fabricate';
export type Vec3 = [number, number, number];

export const WORKSPACES: [Workspace, string][] = [
  ['design', 'Design'],
  ['simulate', 'Simulate'],
  ['measure', 'Measure'],
  ['fabricate', 'Fabricate'],
];

/** Result tabs of the dock, in every workspace. */
export const RESULTS: [string, string][] = [
  ['overview', 'Drone & compare'],
  ['listen', 'Listen'],
  ['accuracy', 'Accuracy'],
  ['spectrum', 'Spectrum'],
  ['orders', 'Orders'],
  ['level', 'Level'],
  ['backpressure', 'Backpressure'],
];

/** The workspace's own dock tab, shown first. */
export const WORKSPACE_TAB: Partial<Record<Workspace, [string, string]>> = {
  measure: ['measure', 'Recordings & cabin'],
  fabricate: ['fabricate', 'Fabrication package'],
};

/** Standard views: where the camera looks from, in vehicle axes (X forward, Y left, Z up). */
export const VIEWS: [string, Vec3, string][] = [
  ['Iso', [0.25, -0.85, 0.45], 'Isometric: from the right, front and above'],
  ['Top', [0, -0.001, 1], 'From above'],
  ['Under', [0, -0.001, -1], 'From below: the underbody side'],
  ['Side', [0, -1, 0], 'From the right side'],
  ['Front', [1, 0, 0], 'From the front'],
  ['Rear', [-1, 0, 0], 'From behind'],
];

const KEY = 'exhaust.ui';
const saved: Record<string, unknown> = (() => {
  try {
    return JSON.parse(localStorage.getItem(KEY) ?? '{}');
  } catch {
    return {};
  }
})();
const kept = <T>(name: string, fallback: T): T => (typeof saved[name] === typeof fallback ? (saved[name] as T) : fallback);

export const ui = $state({
  workspace: 'design' as Workspace,
  dock: 'overview',
  dockOpen: kept('dockOpen', true),
  dockHeight: kept('dockHeight', 300),
  browserWidth: kept('browserWidth', 240),
  inspectorWidth: kept('inspectorWidth', 340),
  /** Dimension every pipe, not only the selected one. */
  allDimensions: kept('allDimensions', false),
  labels: kept('labels', true),
  centreline: kept('centreline', true),
  grid: kept('grid', true),
  ortho: kept('ortho', false),
  help: false,
});

export function persist() {
  const { dockOpen, dockHeight, browserWidth, inspectorWidth, allDimensions, labels, centreline, grid, ortho } = ui;
  try {
    localStorage.setItem(
      KEY,
      JSON.stringify({ dockOpen, dockHeight, browserWidth, inspectorWidth, allDimensions, labels, centreline, grid, ortho }),
    );
  } catch {
    // Storage unavailable: the layout just is not remembered.
  }
}

/** Switches workspace, bringing its dock tab forward. */
export function enter(workspace: Workspace) {
  ui.workspace = workspace;
  const own = WORKSPACE_TAB[workspace];
  if (own) ui.dock = own[0];
  else if (!RESULTS.some(([id]) => id === ui.dock)) ui.dock = 'overview';
  ui.dockOpen = true;
}

/** Brings a dock tab forward. */
export function show(tab: string) {
  ui.dock = tab;
  ui.dockOpen = true;
}

/** Viewport commands for the ribbon and shortcuts; the viewport fills them in when mounted. */
export const view = {
  fitAll: () => {},
  fitSelection: () => {},
  look: (_from: readonly number[]) => {},
};
