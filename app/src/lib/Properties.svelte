<script lang="ts">
  import { backend } from './backend';
  import { fromShown, JOINT, lengthUnit, mm, od, shown, toShown } from './format';
  import { app, commit, edit, editFabrication, manifestOf } from './state.svelte';
  import Tune from './Tune.svelte';
  import type { Element, Joint, Project, Route } from './types/project';

  type Vec3 = [number, number, number];
  /** Stock mandrel-bend radii as multiples of the pipe OD; the solver defaults to 1.5D. */
  const STOCK = [1, 1.5, 2];

  const selection = $derived(app.selection);
  const element = $derived(
    selection?.kind === 'element'
      ? app.project?.system.elements.find((e) => e.id === selection.id)
      : undefined,
  );
  const routeId = $derived(
    selection?.kind === 'route' ? selection.id : selection?.kind === 'vertex' ? selection.route : null,
  );
  const route = $derived(app.project?.system.routes.find((r) => r.id === routeId));
  const manifest = $derived(element ? manifestOf(element.type) : undefined);
  const basis = (path: string) => app.project?.basis?.[path];

  const findElement = (p: Project, id: string) => p.system.elements.find((e) => e.id === id)!;
  const findRoute = (p: Project, id: string) => p.system.routes.find((r) => r.id === id)!;
  const field = (e: Element, key: string) => (e as unknown as Record<string, unknown>)[key];
  const setField = (id: string, key: string, value: unknown) =>
    edit((p) => {
      (findElement(p, id) as unknown as Record<string, unknown>)[key] = value;
    });
  const num = (e: Event) => Number((e.currentTarget as HTMLInputElement).value);
  /** A length entered in the unit lengths show in, in millimetres. */
  const len = (e: Event) => fromShown(num(e));

  function stockOf(r: Route, i: number): number | null {
    const radius = r.bend_radius_mm?.[i] ?? 1.5 * r.pipe.od_mm;
    return STOCK.find((k) => Math.abs(radius - k * r.pipe.od_mm) < 0.5) ?? null;
  }

  function setBend(id: string, i: number, radius: number | null) {
    edit((p) => {
      const r = findRoute(p, id);
      const radii = r.bend_radius_mm ?? [];
      while (radii.length < r.via_mm!.length) radii.push(null);
      radii[i] = radius;
      r.bend_radius_mm = radii;
    });
  }

  function addVia(id: string) {
    edit((p) => {
      const r = findRoute(p, id);
      const pts = app.layout?.routes.find((l) => l.id === id)?.points;
      if (!pts) return;
      // Split the last segment at its midpoint.
      const [a, b] = [pts[pts.length - 2], pts[pts.length - 1]];
      const mid: Vec3 = [0, 1, 2].map((k) => Math.round((a[k] + b[k]) / 2)) as Vec3;
      r.via_mm = [...(r.via_mm ?? []), mid];
      r.bend_radius_mm = [...(r.bend_radius_mm ?? []), null].slice(0, r.via_mm.length);
    });
  }

  /** Element types that sit in a pipe run: one inlet, one outlet. */
  const inline = $derived(app.manifests.filter((m) => m.ports.join() === 'in,out'));
  const twoPort = (e: Element) => manifestOf(e.type)?.ports.join() === 'in,out';
  let placing = $state('quarter_wave_stub');
  let placeAt = $state<number | null>(null);
  let failure = $state<string | null>(null);
  const straights = $derived(
    (app.layout?.routes.find((r) => r.id === routeId)?.pieces ?? []).flatMap((p) =>
      p.kind === 'straight' ? [[p.s0_mm, p.s0_mm + Math.hypot(...p.to.map((x, k) => x - p.from[k]))]] : [],
    ),
  );
  // A click on the pipe sets where to place; otherwise the middle of the longest straight run.
  $effect(() => {
    const picked = selection?.kind === 'route' ? selection.s_mm : undefined;
    const longest = straights.reduce((a, b) => (b[1] - b[0] > a[1] - a[0] ? b : a), [0, 0]);
    placeAt = Math.round(picked ?? (longest[0] + longest[1]) / 2);
  });

  async function place(id: string) {
    failure = null;
    try {
      const done = await backend.insertElement($state.snapshot(app.project) as Project, id, placeAt ?? 0, placing);
      commit(done.project, { kind: 'element', id: done.id });
    } catch (e) {
      failure = String(e);
    }
  }

  async function takeOut(id: string) {
    failure = null;
    try {
      commit(await backend.removeElement($state.snapshot(app.project) as Project, id), null);
    } catch (e) {
      failure = String(e);
    }
  }

  function setHangers(id: string, change: (hangers: number[]) => void) {
    editFabrication((p) => {
      const r = findRoute(p, id);
      const hangers = [...(r.hangers_mm ?? [])];
      change(hangers);
      r.hangers_mm = hangers.sort((a, b) => a - b);
    });
  }

  function setJoint(id: string, end: 'start_joint' | 'end_joint', joint: string) {
    editFabrication((p) => {
      const r = findRoute(p, id);
      if (joint) r[end] = joint as Joint;
      else delete r[end];
    });
  }

  /** The joint the fabrication package assumes where none is given. */
  const defaultJoint = (element: string) =>
    app.project?.system.elements.find((e) => e.id === element)?.type === 'source' ? JOINT.flange : JOINT.butt;

  function removeVia(id: string, i: number) {
    edit((p) => {
      const r = findRoute(p, id);
      r.via_mm = r.via_mm!.filter((_, k) => k !== i);
      r.bend_radius_mm = r.bend_radius_mm?.filter((_, k) => k !== i);
    });
    app.selection = { kind: 'route', id };
  }
</script>

{#snippet vec3(label: string, value: Vec3, onchange: (v: Vec3) => void, length = true)}
  <div class="row">
    <span class="label">{label}</span>
    {#each value as x, k}
      <input
        type="number"
        step="any"
        value={length ? toShown(x) : x}
        onchange={(e) => onchange(value.map((v, j) => (j === k ? (length ? len(e) : num(e)) : v)) as Vec3)}
      />
    {/each}
    <span class="unit">{length ? lengthUnit() : ''}</span>
  </div>
{/snippet}

<section data-testid="properties">
  {#if element && manifest}
    <h2>{element.id} · {manifest.name}</h2>
    <p class="muted">{manifest.summary}</p>
    {#if basis(`system.elements.${element.id}`)}
      <p class="basis">Values here are {basis(`system.elements.${element.id}`)}</p>
    {/if}
    {@render vec3('Position', element.position_mm, (v) => setField(element.id, 'position_mm', v))}
    {@render vec3('Axis', element.axis ?? [-1, 0, 0], (v) => setField(element.id, 'axis', v), false)}
    {#each manifest.param ?? [] as p (p.key)}
      {#if p.kind === 'number'}
        {@const length = p.unit === 'mm'}
        {@const show = (x: number) => (length ? toShown(x) : x)}
        <label class="row">
          <span class="label">{p.label}</span>
          <input
            type="number"
            step="any"
            min={p.min == null ? undefined : show(p.min)}
            max={p.max == null ? undefined : show(p.max)}
            value={show(Number(field(element, p.key) ?? p.default))}
            onchange={(e) => setField(element.id, p.key, length ? len(e) : num(e))}
          />
          <span class="unit">{length ? lengthUnit() : p.unit}</span>
        </label>
        {#if p.min != null && p.max != null}
          <div class="range muted">{show(p.min)}–{show(p.max)} {length ? lengthUnit() : p.unit}</div>
        {/if}
      {:else if p.kind === 'material'}
        <label class="row">
          <span class="label">{p.label}</span>
          <select
            value={field(element, p.key)}
            onchange={(e) => setField(element.id, p.key, e.currentTarget.value)}
          >
            {#each app.project?.materials ?? [] as m (m.id)}
              <option value={m.id}>{m.name}</option>
            {/each}
          </select>
        </label>
      {:else if p.kind === 'direction'}
        {@render vec3(
          p.label,
          (field(element, p.key) as Vec3 | undefined) ?? [0, 0, -1],
          (v) => setField(element.id, p.key, v),
          false,
        )}
      {:else}
        <div class="row muted">
          {p.label}: {((field(element, p.key) as unknown[] | undefined) ?? []).length}
        </div>
      {/if}
    {/each}
    {#if element.type === 'quarter_wave_stub' || element.type === 'helmholtz'}
      {#key element.id}<Tune {element} />{/key}
    {/if}
    {#if twoPort(element)}
      <button class="gap" onclick={() => takeOut(element.id)} title="Join the pipes either side through where it was">
        Remove {element.id}
      </button>
    {/if}
    {#if failure}<p class="failure">{failure}</p>{/if}
  {:else if route}
    {@const layout = app.layout?.routes.find((r) => r.id === route.id)}
    <h2>{route.id}</h2>
    <p class="muted">
      {route.from.element}.{route.from.port} → {route.to.element}.{route.to.port}
      {#if layout}· centreline {mm(layout.length_mm, 0)}{/if}
    </p>
    <label class="row">
      <span class="label">Outside diameter</span>
      <input
        type="number"
        step="any"
        min={toShown(10)}
        value={toShown(route.pipe.od_mm)}
        onchange={(e) => edit((p) => (findRoute(p, route.id).pipe.od_mm = len(e)))}
      />
      <span class="unit">{lengthUnit()}</span>
    </label>
    <div class="range muted">{od(route.pipe.od_mm)}</div>
    <label class="row">
      <span class="label">Wall</span>
      <input
        type="number"
        step="any"
        min={toShown(0.3)}
        value={toShown(route.pipe.wall_mm)}
        onchange={(e) => edit((p) => (findRoute(p, route.id).pipe.wall_mm = len(e)))}
      />
      <span class="unit">{lengthUnit()}</span>
    </label>
    <label class="row">
      <span class="label">Material</span>
      <select
        value={route.pipe.material}
        onchange={(e) => edit((p) => (findRoute(p, route.id).pipe.material = e.currentTarget.value))}
      >
        {#each app.project?.materials ?? [] as m (m.id)}
          <option value={m.id}>{m.name}</option>
        {/each}
      </select>
    </label>
    <h2 class="gap">Bends at via points</h2>
    {#each route.via_mm ?? [] as v, i}
      {@const stock = stockOf(route, i)}
      <div
        class="via"
        class:picked={selection?.kind === 'vertex' && selection.index === i}
        data-testid="via"
      >
        {@render vec3(`#${i + 1}`, v, (nv) =>
          edit((p) => (findRoute(p, route.id).via_mm![i] = nv)),
        )}
        <div class="row">
          <span class="label">Radius</span>
          <select
            value={stock === null ? 'custom' : String(stock)}
            onchange={(e) => {
              const k = e.currentTarget.value;
              if (k !== 'custom') setBend(route.id, i, k === '1.5' ? null : Number(k) * route.pipe.od_mm);
            }}
          >
            {#each STOCK as k}<option value={String(k)}>{k}D</option>{/each}
            <option value="custom">custom</option>
          </select>
          <input
            type="number"
            step="any"
            min={toShown(1)}
            value={toShown(route.bend_radius_mm?.[i] ?? 1.5 * route.pipe.od_mm)}
            onchange={(e) => setBend(route.id, i, len(e))}
          />
          <span class="unit">{lengthUnit()}</span>
          <button title="Remove this via point" onclick={() => removeVia(route.id, i)}>×</button>
        </div>
      </div>
    {/each}
    <button onclick={() => addVia(route.id)}>Add via point</button>
    <h2 class="gap">Place an element on this pipe</h2>
    <div class="row place">
      <select bind:value={placing} data-testid="place-kind">
        {#each inline as m (m.type)}<option value={m.type}>{m.name}</option>{/each}
      </select>
      <input
        type="number"
        step="any"
        min="0"
        value={placeAt === null ? '' : toShown(placeAt)}
        onchange={(e) => (placeAt = len(e))}
        data-testid="place-at"
      />
      <span class="unit">{lengthUnit()} along</span>
      <button onclick={() => place(route.id)}>Place</button>
    </div>
    <div class="range muted">
      straight runs: {straights.map(([a, b]) => `${shown(a)}–${shown(b)}`).join(', ')} {lengthUnit()}; click
      the pipe to pick a spot
    </div>
    {#if failure}<p class="failure">{failure}</p>{/if}
    <h2 class="gap">Hangers</h2>
    {#each route.hangers_mm ?? [] as h, i}
      <div class="row hanger" data-testid="hanger">
        <span class="label">#{i + 1}</span>
        <input
          type="number"
          step="any"
          min="0"
          value={toShown(h)}
          onchange={(e) => setHangers(route.id, (hs) => (hs[i] = len(e)))}
        />
        <span class="unit">{lengthUnit()} along</span>
        <button title="Remove this hanger" onclick={() => setHangers(route.id, (hs) => hs.splice(i, 1))}>×</button>
      </div>
    {/each}
    <button onclick={() => setHangers(route.id, (hs) => hs.push(placeAt ?? 0))}>
      Add hanger at {mm(placeAt ?? 0, 0)}
    </button>
    <h2 class="gap">Joints</h2>
    {#each [['start_joint', route.from.element], ['end_joint', route.to.element]] as const as [end, at]}
      <label class="row">
        <span class="label">At {at}</span>
        <select value={route[end] ?? ''} onchange={(e) => setJoint(route.id, end, e.currentTarget.value)}>
          <option value="">default: {defaultJoint(at)}</option>
          {#each Object.entries(JOINT) as [j, name]}<option value={j}>{name}</option>{/each}
        </select>
      </label>
    {/each}
  {:else}
    <p class="muted">Select a route or element, here or in the viewport.</p>
  {/if}
</section>

<style>
  section {
    padding: 10px;
    overflow-y: auto;
    flex: 1;
  }

  .row {
    display: grid;
    grid-template-columns: 110px repeat(3, minmax(0, 1fr)) auto;
    align-items: center;
    gap: 4px;
    margin: 3px 0;
  }

  label.row {
    grid-template-columns: 110px minmax(0, 1fr) auto;
  }

  .via .row:last-child {
    grid-template-columns: 110px 88px minmax(0, 1fr) auto auto;
  }

  .label {
    color: var(--muted);
  }

  .unit {
    color: var(--muted);
    font-size: 11px;
  }

  .range {
    font-size: 11px;
    margin: -2px 0 4px 114px;
  }

  .basis {
    color: var(--warn);
  }

  .gap {
    margin-top: 12px;
  }

  .via {
    border-left: 2px solid transparent;
    padding-left: 4px;
    margin-bottom: 6px;
  }

  .via.picked {
    border-left-color: var(--accent);
  }

  .row.place {
    grid-template-columns: minmax(0, 1fr) 70px auto auto;
  }

  .row.hanger {
    grid-template-columns: 110px minmax(0, 1fr) auto auto;
  }

  .failure {
    color: var(--bad);
  }
</style>
