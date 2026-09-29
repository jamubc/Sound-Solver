<script lang="ts">
  import { addHanger, addVia, inline, placeElement, removeElement, removeVia, setHangers, spot, straights } from './actions.svelte';
  import { describeInput, fromShown, JOINT, lengthUnit, mm, od, shown, toShown } from './format';
  import Section from './Section.svelte';
  import { app, edit, editFabrication, manifestOf } from './state.svelte';
  import Tune from './Tune.svelte';
  import type { Element, Joint, Project, Route } from './types/project';

  type Vec3 = [number, number, number];
  /** Stock mandrel-bend radii as multiples of the pipe OD; the solver defaults to 1.5D. */
  const STOCK = [1, 1.5, 2];

  const selection = $derived(app.selection);
  const element = $derived(
    selection?.kind === 'element' ? app.project?.system.elements.find((e) => e.id === selection.id) : undefined,
  );
  const routeId = $derived(
    selection?.kind === 'route' ? selection.id : selection?.kind === 'vertex' ? selection.route : null,
  );
  const route = $derived(app.project?.system.routes.find((r) => r.id === routeId));
  const manifest = $derived(element ? manifestOf(element.type) : undefined);
  const input = (path: string) => app.project?.inputs?.[path];

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

  const twoPort = (e: Element) => manifestOf(e.type)?.ports.join() === 'in,out';
  let placing = $state('quarter_wave_stub');
  // A click on the pipe sets where to place; otherwise the middle of its longest straight run.
  let placeAt = $state<number | null>(null);
  $effect(() => {
    placeAt = routeId ? spot(routeId) : null;
  });
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

<div class="properties" data-testid="properties">
  {#if element && manifest}
    <Section title={manifest.name}>
      <p class="muted">{manifest.summary}</p>
      {#if input(`system.elements.${element.id}`)}
        <p class="basis">Values here: {describeInput(input(`system.elements.${element.id}`)!)}</p>
      {/if}
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
            <select value={field(element, p.key)} onchange={(e) => setField(element.id, p.key, e.currentTarget.value)}>
              {#each app.project?.materials ?? [] as m (m.id)}<option value={m.id}>{m.name}</option>{/each}
            </select>
          </label>
        {:else if p.kind === 'direction'}
          {@render vec3(p.label, (field(element, p.key) as Vec3 | undefined) ?? [0, 0, -1], (v) => setField(element.id, p.key, v), false)}
        {:else}
          <div class="row muted">{p.label}: {((field(element, p.key) as unknown[] | undefined) ?? []).length}</div>
        {/if}
      {/each}
    </Section>
    <Section title="Placement">
      {@render vec3('Position', element.position_mm, (v) => setField(element.id, 'position_mm', v))}
      {@render vec3('Flow axis', element.axis ?? [-1, 0, 0], (v) => setField(element.id, 'axis', v), false)}
      {#if twoPort(element)}
        <button class="danger" onclick={() => removeElement(element.id)} title="Join the pipes either side through where it was">
          Remove {element.id}
        </button>
      {/if}
    </Section>
    {#if element.type === 'quarter_wave_stub' || element.type === 'helmholtz'}
      <Section title="Tune to a drone">
        {#key element.id}<Tune {element} />{/key}
      </Section>
    {/if}
  {:else if route}
    {@const layout = app.layout?.routes.find((r) => r.id === route.id)}
    <Section title="Pipe">
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
        <select value={route.pipe.material} onchange={(e) => edit((p) => (findRoute(p, route.id).pipe.material = e.currentTarget.value))}>
          {#each app.project?.materials ?? [] as m (m.id)}<option value={m.id}>{m.name}</option>{/each}
        </select>
      </label>
    </Section>
    <Section title="Bend points">
      {#each route.via_mm ?? [] as v, i}
        {@const stock = stockOf(route, i)}
        <div class="via" class:picked={selection?.kind === 'vertex' && selection.index === i} data-testid="via">
          {@render vec3(`#${i + 1}`, v, (nv) => edit((p) => (findRoute(p, route.id).via_mm![i] = nv)))}
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
            <button title="Remove this bend point" onclick={() => removeVia(route.id, i)}>×</button>
          </div>
        </div>
      {/each}
      <button onclick={() => addVia(route.id)}>Add bend point</button>
    </Section>
    <Section title="Place an element">
      <div class="row place">
        <select bind:value={placing} data-testid="place-kind">
          {#each inline() as m (m.type)}<option value={m.type}>{m.name}</option>{/each}
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
        <button onclick={() => placeElement(route.id, placing, placeAt ?? spot(route.id))}>Place</button>
      </div>
      <div class="range muted">
        straight runs: {straights(route.id).map(([a, b]) => `${shown(a)}–${shown(b)}`).join(', ')} {lengthUnit()};
        click the pipe to pick a spot
      </div>
    </Section>
    <Section title="Hangers">
      {#each route.hangers_mm ?? [] as h, i}
        <div class="row hanger" data-testid="hanger">
          <span class="label">#{i + 1}</span>
          <input type="number" step="any" min="0" value={toShown(h)} onchange={(e) => setHangers(route.id, (hs) => (hs[i] = len(e)))} />
          <span class="unit">{lengthUnit()} along</span>
          <button title="Remove this hanger" onclick={() => setHangers(route.id, (hs) => hs.splice(i, 1))}>×</button>
        </div>
      {/each}
      <button onclick={() => addHanger(route.id, placeAt ?? spot(route.id))}>Add hanger at {mm(placeAt ?? 0, 0)}</button>
    </Section>
    <Section title="Joints">
      {#each [['start_joint', route.from.element], ['end_joint', route.to.element]] as const as [end, at]}
        <label class="row">
          <span class="label">At {at}</span>
          <select value={route[end] ?? ''} onchange={(e) => setJoint(route.id, end, e.currentTarget.value)}>
            <option value="">default: {defaultJoint(at)}</option>
            {#each Object.entries(JOINT) as [j, name]}<option value={j}>{name}</option>{/each}
          </select>
        </label>
      {/each}
    </Section>
  {:else}
    <p class="muted empty">Select a pipe or element, in the model tree or the viewport.</p>
  {/if}
</div>

<style>
  .row {
    display: grid;
    grid-template-columns: 104px repeat(3, minmax(0, 1fr)) 26px;
    align-items: center;
    gap: 4px;
    margin: 3px 0;
  }

  label.row {
    grid-template-columns: 104px minmax(0, 1fr) 26px;
  }

  .via .row:last-child {
    grid-template-columns: 104px 84px minmax(0, 1fr) 26px auto;
  }

  .label {
    color: var(--muted);
  }

  .unit {
    color: var(--faint);
    font-size: 11px;
  }

  .range {
    font-size: 11px;
    margin: -2px 0 4px 108px;
  }

  .basis {
    color: var(--warn);
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
    grid-template-columns: 104px minmax(0, 1fr) auto auto;
  }

  .empty {
    padding: 10px;
  }
</style>
