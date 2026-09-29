<script lang="ts">
  import { mm } from './format';
  import { app, edit, manifestOf } from './state.svelte';
  import type { Element, Project, Route } from './types/project';

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

  function removeVia(id: string, i: number) {
    edit((p) => {
      const r = findRoute(p, id);
      r.via_mm = r.via_mm!.filter((_, k) => k !== i);
      r.bend_radius_mm = r.bend_radius_mm?.filter((_, k) => k !== i);
    });
    app.selection = { kind: 'route', id };
  }
</script>

{#snippet vec3(label: string, value: Vec3, onchange: (v: Vec3) => void)}
  <div class="row">
    <span class="label">{label}</span>
    {#each value as x, k}
      <input
        type="number"
        step="1"
        value={x}
        onchange={(e) => onchange(value.map((v, j) => (j === k ? num(e) : v)) as Vec3)}
      />
    {/each}
    <span class="unit">mm</span>
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
    {@render vec3('Axis', element.axis ?? [-1, 0, 0], (v) => setField(element.id, 'axis', v))}
    {#each manifest.param ?? [] as p (p.key)}
      {#if p.kind === 'number'}
        <label class="row">
          <span class="label">{p.label}</span>
          <input
            type="number"
            step="any"
            min={p.min}
            max={p.max}
            value={field(element, p.key) ?? p.default}
            onchange={(e) => setField(element.id, p.key, num(e))}
          />
          <span class="unit">{p.unit}</span>
        </label>
        <div class="range muted">{p.min}–{p.max} {p.unit}</div>
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
        {@render vec3(p.label, (field(element, p.key) as Vec3 | undefined) ?? [0, 0, -1], (v) =>
          setField(element.id, p.key, v),
        )}
      {:else}
        <div class="row muted">
          {p.label}: {((field(element, p.key) as unknown[] | undefined) ?? []).length}
        </div>
      {/if}
    {/each}
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
        step="0.1"
        min="10"
        value={route.pipe.od_mm}
        onchange={(e) => edit((p) => (findRoute(p, route.id).pipe.od_mm = num(e)))}
      />
      <span class="unit">mm</span>
    </label>
    <div class="range muted">{(route.pipe.od_mm / 25.4).toFixed(2)} in</div>
    <label class="row">
      <span class="label">Wall</span>
      <input
        type="number"
        step="0.1"
        min="0.3"
        value={route.pipe.wall_mm}
        onchange={(e) => edit((p) => (findRoute(p, route.id).pipe.wall_mm = num(e)))}
      />
      <span class="unit">mm</span>
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
            step="1"
            min="1"
            value={route.bend_radius_mm?.[i] ?? 1.5 * route.pipe.od_mm}
            onchange={(e) => setBend(route.id, i, num(e))}
          />
          <span class="unit">mm</span>
          <button title="Remove this via point" onclick={() => removeVia(route.id, i)}>×</button>
        </div>
      </div>
    {/each}
    <button onclick={() => addVia(route.id)}>Add via point</button>
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
    grid-template-columns: 110px 70px minmax(0, 1fr) auto auto;
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
</style>
