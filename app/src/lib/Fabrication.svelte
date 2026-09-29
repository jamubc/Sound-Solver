<script lang="ts">
  import { open } from '@tauri-apps/plugin-dialog';
  import { untrack } from 'svelte';
  import { backend } from './backend';
  import { fileName, fromShown, JOINT, lengthUnit, mm, od, shown, toShown, vec } from './format';
  import { app, editFabrication, REFERENCE_COLOURS } from './state.svelte';
  import type { Package, Part } from './types/api';
  import type { Fabrication, Project } from './types/project';

  type Vec3 = [number, number, number];
  const UNITS = [
    { label: 'metres', mm: 1000 },
    { label: 'centimetres', mm: 10 },
    { label: 'millimetres', mm: 1 },
    { label: 'inches', mm: 25.4 },
  ];

  const fab = $derived(app.project?.fabrication);
  const scan = $derived(fab?.scan);

  let pkg = $state<Package | null>(null);
  let pkgError = $state<string | null>(null);
  let written = $state<string[]>([]);
  let failure = $state<string | null>(null);
  let exporting = $state(false);
  let run = 0;

  // The parts list follows every edit the backend accepts.
  $effect(() => {
    if (!app.layout) return;
    const project = untrack(() => $state.snapshot(app.project)) as Project;
    const mine = ++run;
    backend.fabrication(project).then(
      (p) => {
        if (mine === run) [pkg, pkgError] = [p, null];
      },
      (e) => {
        if (mine === run) [pkg, pkgError] = [null, String(e)];
      },
    );
  });

  const num = (e: Event) => Number((e.currentTarget as HTMLInputElement).value);
  /** A length entered in the unit lengths show in, in millimetres. */
  const len = (e: Event) => fromShown(num(e));
  const setFab =(change: (f: Fabrication) => void) =>
    editFabrication((p) => {
      p.fabrication ??= {};
      change(p.fabrication);
    });
  const zeros = (): [Vec3, Vec3, Vec3] => [
    [0, 0, 0],
    [0, 0, 0],
    [0, 0, 0],
  ];

  async function attempt(action: () => Promise<void>) {
    failure = null;
    try {
      await action();
    } catch (e) {
      failure = String(e);
    }
  }

  const loadScan = () =>
    attempt(async () => {
      const filters = [{ name: 'Underbody scan', extensions: ['stl', 'obj'] }];
      const path = await open({ filters, multiple: false, directory: false });
      if (typeof path !== 'string') return;
      // A new scan keeps the jack-pad coordinates; its points are picked afresh.
      setFab((f) => {
        f.scan = {
          path,
          unit_mm: f.scan?.unit_mm ?? 1000,
          scan_points: zeros(),
          vehicle_points_mm: f.scan?.vehicle_points_mm ?? zeros(),
        };
      });
    });

  /** Saves the project, then writes the package beside it. */
  const exportPackage = () =>
    attempt(async () => {
      if (!app.project || !app.path) return;
      exporting = true;
      written = [];
      try {
        const project = $state.snapshot(app.project) as Project;
        if (app.dirty) {
          await backend.saveProject(app.path, project);
          app.dirty = false;
        }
        written = await backend.exportPackage(project, app.path);
      } finally {
        exporting = false;
      }
    });

  function describe(p: Part) {
    switch (p.kind) {
      case 'straight':
        return `straight ${mm(p.length_mm, 0)}`;
      case 'branch':
        return (
          `cut ${mm(p.length_mm, 1)}, one end coped ${mm(p.saddle_mm, 1)} deep to saddle ` +
          `the ${mm(p.main_od_mm, 1)} pipe at ${p.tee_angle_deg.toFixed(0)}°`
        );
      case 'bend':
        return (
          `bend ${p.angle_deg.toFixed(1)}° at ${mm(p.clr_mm, 0)} CLR, ` +
          (p.stock_as_is ? `stock ${p.stock_deg}° as is` : `cut from stock ${p.stock_deg}°`) +
          (p.rotation_deg != null ? `, rotate ${p.rotation_deg.toFixed(0)}°` : '')
        );
    }
  }
  const joints = $derived(
    Object.entries(
      (pkg?.welds ?? []).reduce<Record<string, number>>((n, w) => ({ ...n, [w.joint]: (n[w.joint] ?? 0) + 1 }), {}),
    ),
  );
  const noted = $derived((pkg?.welds ?? []).filter((w) => w.note));
</script>

{#snippet vec3(value: Vec3, onchange: (v: Vec3) => void)}
  {#each value as x, k}
    <input
      type="number"
      step="any"
      value={toShown(x)}
      onchange={(e) => onchange(value.map((v, j) => (j === k ? len(e) : v)) as Vec3)}
    />
  {/each}
{/snippet}

<div class="fabrication" data-testid="fabrication">
  <section>
    <h2>Package</h2>
    <div class="line">
      <button class="primary" onclick={exportPackage} disabled={!app.path || !!app.invalid || exporting}>
        {app.dirty ? 'Save and export package' : 'Export package'}
      </button>
      <span class="muted">
        {app.path
          ? 'cut list, bend schedule, weld map, STEP and STL (and the clearance report) beside the project file'
          : 'Save the project first: the files are written beside it.'}
      </span>
    </div>
    {#each written as file}<div class="mono muted">{file}</div>{/each}
    {#if failure}<p class="failure">{failure}</p>{/if}
  </section>

  <section data-testid="parts">
    <h2>Cut list and bends</h2>
    {#if pkgError}
      <p class="failure">{pkgError}</p>
    {:else if pkg}
      {#each pkg.warnings as w}<p class="warn">{w}</p>{/each}
      {#each pkg.routes as r (r.route)}
        <h3>{r.route} <span class="muted">{od(r.od_mm)} × {mm(r.wall_mm)} {r.material}, {mm(r.length_mm, 0)}</span></h3>
        <ol>
          {#each r.parts as p (p.id)}
            <li>
              <span class="mono">{p.id}</span>
              {describe(p)}
              {#if p.kind !== 'branch' && p.hangers_mm.length}
                <span class="muted">· hanger at {p.hangers_mm.map(shown).join(', ')} {lengthUnit()}</span>
              {/if}
            </li>
          {/each}
        </ol>
      {/each}
      <h3>Stock sticks <span class="muted">{mm(fab?.stock_length_mm ?? 3048, 0)} long, {mm(fab?.kerf_mm ?? 3, 1)} kerf</span></h3>
      <ol>
        {#each pkg.sticks as s}
          <li>
            {od(s.od_mm)} × {mm(s.wall_mm)} {s.material}: {s.cuts.join(', ')}
            <span class="muted">· offcut {mm(s.offcut_mm, 0)}</span>
          </li>
        {/each}
      </ol>
      <h3>Joints <span class="muted">{joints.map(([j, n]) => `${n} × ${JOINT[j] ?? j}`).join(', ')}</span></h3>
      {#each noted as w (w.id)}
        <p class="warn">{w.id} ({w.between.join(' – ')}): {w.note}</p>
      {/each}
    {:else}
      <p class="muted">{app.invalid ? 'The project is not solvable as edited.' : 'Working…'}</p>
    {/if}
  </section>

  <section data-testid="scan">
    <h2>Underbody scan</h2>
    {#if scan}
      <div class="line">
        <span class="mono" title={scan.path}>{fileName(scan.path)}</span>
        <button onclick={loadScan}>Replace…</button>
        <button onclick={() => setFab((f) => (f.scan = null))}>Remove</button>
      </div>
      {#if app.scanError}
        <p class="failure">{app.scanError}</p>
      {:else if app.scanMesh}
        <p class="muted">{(app.scanMesh.index.length / 3).toLocaleString()} triangles</p>
      {/if}
      <label class="row">
        <span class="label">Scan units</span>
        <select value={scan.unit_mm ?? 1} onchange={(e) => setFab((f) => (f.scan!.unit_mm = Number(e.currentTarget.value)))}>
          {#each UNITS as u}<option value={u.mm}>{u.label}</option>{/each}
        </select>
      </label>
      <p class="muted">
        Three points fix where the scan sits on the car: pick each on the scan (jack pads, say) and give its
        position on the car ({lengthUnit()} from the downpipe flange).
      </p>
      {#each scan.scan_points as p, k}
        <div class="point" data-testid="reference-point">
          <div class="row">
            <span class="label"><i style:background={REFERENCE_COLOURS[k]}></i> Point {k + 1}</span>
            <span class="mono muted coords">{p.map((x) => x.toFixed(3)).join(', ')}</span>
            <button class:on={app.picking === k} onclick={() => (app.picking = app.picking === k ? null : k)}>
              {app.picking === k ? 'Click the scan…' : 'Pick on scan'}
            </button>
          </div>
          <div class="row">
            <span class="label">on the car</span>
            {@render vec3(scan.vehicle_points_mm[k], (v) => setFab((f) => (f.scan!.vehicle_points_mm[k] = v)))}
            <span class="unit">{lengthUnit()}</span>
          </div>
        </div>
      {/each}
    {:else}
      <p class="muted">An STL or OBJ of the underbody (phone LiDAR or photogrammetry) to check pipe clearance against.</p>
      <button onclick={loadScan}>Load scan…</button>
    {/if}
  </section>

  <section data-testid="clearance">
    <h2>Clearance</h2>
    <label class="row">
      <span class="label">Gap wanted</span>
      <input
        type="number"
        step="any"
        min="0"
        value={toShown(fab?.clearance_mm ?? 25)}
        onchange={(e) => setFab((f) => (f.clearance_mm = len(e)))}
      />
      <span class="unit">{lengthUnit()}</span>
    </label>
    {#each fab?.clearance_zones ?? [] as z, i}
      <div class="row zone">
        <input value={z.name} onchange={(e) => setFab((f) => (f.clearance_zones![i].name = e.currentTarget.value))} />
        <input type="number" step="any" value={toShown(z.x_min_mm)} title="from x" onchange={(e) => setFab((f) => (f.clearance_zones![i].x_min_mm = len(e)))} />
        <input type="number" step="any" value={toShown(z.x_max_mm)} title="to x" onchange={(e) => setFab((f) => (f.clearance_zones![i].x_max_mm = len(e)))} />
        <input type="number" step="any" value={toShown(z.clearance_mm)} title="gap" onchange={(e) => setFab((f) => (f.clearance_zones![i].clearance_mm = len(e)))} />
        <button title="Remove this zone" onclick={() => setFab((f) => f.clearance_zones!.splice(i, 1))}>×</button>
      </div>
    {/each}
    {#if fab?.clearance_zones?.length}<div class="range muted">name · from x · to x · gap, {lengthUnit()}</div>{/if}
    <button
      onclick={() =>
        setFab((f) => (f.clearance_zones = [...(f.clearance_zones ?? []), { name: 'zone', x_min_mm: 0, x_max_mm: 0, clearance_mm: 40 }]))}
      >Add zone</button
    >
    {#if app.clearance}
      <p>
        Scan placed with a residual of {mm(app.clearance.placement.residual_mm)}
        <span class="muted">(RMS misfit of the three points: clearances are uncertain by about this much)</span>
      </p>
      {#if app.clearance.contacts.length}
        <table data-testid="contacts">
          <thead><tr><th>Route</th><th>Along, {lengthUnit()}</th><th>Closest</th><th>Wanted</th><th>Zone</th></tr></thead>
          <tbody>
            {#each app.clearance.contacts as c}
              <tr>
                <td>
                  <button
                    class="link"
                    title="Select this stretch"
                    onclick={() => (app.selection = { kind: 'route', id: c.route, s_mm: (c.from_mm + c.to_mm) / 2 })}
                    >{c.route}</button
                  >
                </td>
                <td>{shown(c.from_mm)}–{shown(c.to_mm)}</td>
                <td class="bad">{mm(c.min_mm, 0)}</td>
                <td>{mm(c.wanted_mm, 0)}</td>
                <td>{c.zone ?? ''}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      {:else}
        <p class="good">No stretch is closer than wanted.</p>
      {/if}
      <table>
        <thead>
          <tr><th>Route</th><th>Closest</th><th>Along, {lengthUnit()}</th><th>At (x, y, z), {lengthUnit()}</th></tr>
        </thead>
        <tbody>
          {#each app.clearance.routes as r}
            <tr><td>{r.route}</td><td>{mm(r.min_mm, 0)}</td><td>{shown(r.at_mm)}</td><td>{vec(r.at)}</td></tr>
          {/each}
        </tbody>
      </table>
    {:else if app.clearanceError}
      <p class="warn" data-testid="clearance-unavailable">Clearance unavailable: {app.clearanceError}</p>
    {:else if !scan}
      <p class="muted">Load a scan to check clearance.</p>
    {/if}
  </section>

  <section>
    <h2>Stock</h2>
    <label class="row">
      <span class="label">Stick length</span>
      <input
        type="number"
        step="any"
        min={toShown(1)}
        value={toShown(fab?.stock_length_mm ?? 3048)}
        onchange={(e) => setFab((f) => (f.stock_length_mm = len(e)))}
      />
      <span class="unit">{lengthUnit()}</span>
    </label>
    <label class="row">
      <span class="label">Saw kerf</span>
      <input
        type="number"
        step="any"
        min="0"
        value={toShown(fab?.kerf_mm ?? 3)}
        onchange={(e) => setFab((f) => (f.kerf_mm = len(e)))}
      />
      <span class="unit">{lengthUnit()}</span>
    </label>
    <label class="row">
      <span class="label">Stock bend within</span>
      <input
        type="number"
        step="0.1"
        min="0"
        value={fab?.stock_bend_tolerance_deg ?? 0.5}
        onchange={(e) => setFab((f) => (f.stock_bend_tolerance_deg = num(e)))}
      />
      <span class="unit">° of 45°/90°</span>
    </label>
  </section>
</div>

<style>
  .fabrication {
    padding: 10px 12px;
  }

  section {
    margin-bottom: 14px;
  }

  h3 {
    font-size: 12px;
    margin: 10px 0 4px;
  }

  ol {
    margin: 0;
    padding-left: 22px;
  }

  .line {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 4px 0;
  }

  .row {
    display: grid;
    grid-template-columns: 120px repeat(3, minmax(0, 1fr)) auto;
    align-items: center;
    gap: 4px;
    margin: 3px 0;
  }

  label.row {
    grid-template-columns: 120px minmax(0, 1fr) auto;
  }

  .point .row:first-child {
    grid-template-columns: 120px minmax(0, 1fr) auto;
  }

  .row.zone {
    grid-template-columns: minmax(0, 1.4fr) repeat(3, minmax(0, 1fr)) auto;
  }

  .point {
    margin-bottom: 6px;
  }

  .label {
    color: var(--muted);
  }

  .label i {
    display: inline-block;
    width: 9px;
    height: 9px;
    border-radius: 50%;
  }

  .unit,
  .range {
    color: var(--muted);
    font-size: 11px;
  }

  .coords {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  button.on {
    border-color: var(--accent);
    color: var(--accent);
  }

  table {
    width: 100%;
    border-collapse: collapse;
    margin: 6px 0;
  }

  th,
  td {
    text-align: left;
    padding: 2px 6px 2px 0;
    border-bottom: 1px solid var(--line);
  }

  th {
    color: var(--muted);
    font-weight: normal;
  }

  button.link {
    border: none;
    background: none;
    padding: 0;
    color: var(--accent);
  }

  .warn {
    color: var(--warn);
  }

  .bad,
  .failure {
    color: var(--bad);
  }

  .good {
    color: var(--good);
  }
</style>
