<script lang="ts">
  import { backend } from './backend';
  import { lengthUnit, rpm as rpmText, shown as shownLength } from './format';
  import { app, edit, manifestOf } from './state.svelte';
  import type { Tuning } from './types/api';
  import type { Element, Project } from './types/project';

  let { element }: { element: Element } = $props();

  const firing = $derived((app.project?.engine.geometry.firing_order.length ?? 4) / 2);
  /** Where the drone is: the interior report when there is one, from the reference once complete. */
  const drone = $derived.by(() => {
    const metrics = app.timeDomain.result?.metrics ?? app.preview?.metrics;
    const where = metrics?.drone_interior ? 'interior' : 'exterior';
    const d = metrics?.drone_interior ?? metrics?.drone_exterior;
    const solver = app.timeDomain.result ? 'time domain' : 'four-pole preview';
    return d ? { rpm: Math.round(d.peak_rpm / 10) * 10, label: `${where} drone peak, ${solver}` } : null;
  });

  let rpm = $state<number | null>(null);
  let hz = $state<number | null>(null);
  let lowest = $state<number | null>(null);
  let highest = $state<number | null>(null);
  let result = $state<Tuning | null>(null);
  let failure = $state<string | null>(null);
  let working = $state(false);

  const setRpm = (value: number) => {
    rpm = value;
    hz = Math.round(((firing * value) / 60) * 10) / 10;
  };
  $effect(() => {
    if (rpm === null && drone) setRpm(drone.rpm);
  });

  const volume = $derived(result?.parameter === 'volume_l');
  const unit = $derived(volume ? 'L' : lengthUnit());
  const shown = (x: number) => (volume ? x.toFixed(2) : shownLength(x));
  const label = $derived(
    manifestOf(element.type)?.param?.find((p) => p.key === result?.parameter)?.label ?? result?.parameter,
  );

  async function tune() {
    if (!app.project || rpm === null || hz === null) return;
    [failure, result, working] = [null, null, true];
    try {
      const stated: [number, number] | null =
        lowest !== null && highest !== null ? [Math.min(lowest, highest), Math.max(lowest, highest)] : null;
      result = await backend.tune($state.snapshot(app.project) as Project, element.id, hz, rpm, stated);
    } catch (e) {
      failure = String(e);
    } finally {
      working = false;
    }
  }

  /** The tuned value as stored: to 0.01 L, or to the millimetre. */
  function apply(tuned: Tuning) {
    const value = volume ? Number(tuned.value.toFixed(2)) : Math.round(tuned.value);
    edit((p) => {
      const e = p.system.elements.find((x) => x.id === element.id);
      if (e) (e as unknown as Record<string, number>)[tuned.parameter] = value;
    });
  }
</script>

<section data-testid="tune">
  <label class="row">
    <span class="label">Drone at</span>
    <input type="number" step="10" value={rpm} onchange={(e) => setRpm(Number(e.currentTarget.value))} />
    <span class="unit">rpm</span>
  </label>
  <div class="range muted">{drone ? `${drone.label}: ${rpmText(drone.rpm)}` : 'no drone report yet'}</div>
  <label class="row">
    <span class="label">Resonance at</span>
    <input type="number" step="0.1" bind:value={hz} />
    <span class="unit">Hz</span>
  </label>
  <div class="range muted">firing order {firing} at that speed</div>
  <div class="row pair">
    <span class="label">Branch gas</span>
    <input type="number" step="10" placeholder="model" bind:value={lowest} />
    <input type="number" step="10" placeholder="model" bind:value={highest} />
    <span class="unit">K</span>
  </div>
  <div class="range muted">measured lowest and highest (a thermocouple at the branch); blank: the thermal model</div>
  <button onclick={tune} disabled={working || rpm === null || hz === null}>{working ? 'Tuning…' : 'Tune'}</button>
  {#if result}
    {@const t = result.branch_temperature_k}
    <div class="result" data-testid="tuning">
      <p>
        <strong>{label} {shown(result.value)} {unit}</strong> puts the resonance at {result.target_hz.toFixed(1)} Hz
        (now {result.resonance_hz.toFixed(1)} Hz), branch gas {t[0].toFixed(0)} K.
      </p>
      {#if result.band}
        <p>
          {shown(Math.min(...result.band))}–{shown(Math.max(...result.band))}
          {unit} for branch gas {Math.min(t[1], t[2]).toFixed(0)}–{Math.max(t[1], t[2]).toFixed(0)} K.
        </p>
      {:else}
        <p class="warn">No band: walls are not computed and no temperature is stated.</p>
      {/if}
      <p class="muted">{result.basis}</p>
      <button onclick={() => apply(result!)}>Apply {shown(result.value)} {unit}</button>
    </div>
  {/if}
  {#if failure}<p class="failure">{failure}</p>{/if}
</section>

<style>
  .row {
    display: grid;
    grid-template-columns: 110px minmax(0, 1fr) auto;
    align-items: center;
    gap: 4px;
    margin: 3px 0;
  }

  .row.pair {
    grid-template-columns: 110px repeat(2, minmax(0, 1fr)) auto;
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

  .result {
    margin-top: 6px;
    padding: 6px 8px;
    border: 1px solid var(--line);
    border-radius: 4px;
  }

  .result p {
    margin: 0 0 4px;
  }

  .warn {
    color: var(--warn);
  }

  .failure {
    color: var(--bad);
  }
</style>
