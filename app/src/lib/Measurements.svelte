<script lang="ts">
  import { open } from '@tauri-apps/plugin-dialog';
  import { untrack } from 'svelte';
  import type uPlot from 'uplot';
  import { addRecording } from './actions.svelte';
  import { backend } from './backend';
  import { db, fileName, ORDER_COLOURS, rpm } from './format';
  import Plot from './Plot.svelte';
  import Validation from './Validation.svelte';
  import type { Chip } from './provenance';
  import { app, editMeasurements } from './state.svelte';
  import type { DroneReport, OrderTracks } from './types/api';
  import type { CabinTf, Project, RecordingRef } from './types/project';

  const recordings = $derived(app.project?.measurements?.recordings ?? []);
  const tf = $derived(app.project?.measurements?.cabin_tf ?? null);
  const firing = $derived((app.project?.engine.geometry.firing_order.length ?? 4) / 2);
  /** The prediction to set measurements against: the reference once complete. */
  const predicted = $derived(
    app.timeDomain.result
      ? { metrics: app.timeDomain.result.metrics, label: 'time domain' }
      : app.preview
        ? { metrics: app.preview.metrics, label: 'four-pole preview' }
        : null,
  );

  let tracks = $state<(OrderTracks | string | null)[]>([]);
  let run = 0;
  // Tracks follow the recordings and the sweep they are binned on.
  $effect(() => {
    void JSON.stringify([recordings, app.project?.operating]);
    const project = untrack(() => $state.snapshot(app.project)) as Project;
    const mine = ++run;
    tracks = recordings.map(() => null);
    recordings.forEach((_, i) =>
      backend.orderTracks(project, i).then(
        (t) => {
          if (mine === run) tracks[i] = t;
        },
        (e) => {
          if (mine === run) tracks[i] = String(e);
        },
      ),
    );
  });

  let failure = $state<string | null>(null);
  async function attempt(action: () => Promise<void>) {
    failure = null;
    try {
      await action();
    } catch (e) {
      failure = String(e);
    }
  }
  const pickFile = async (name: string, extensions: string[]) => {
    const path = await open({ filters: [{ name, extensions }], multiple: false, directory: false });
    return typeof path === 'string' ? path : null;
  };
  const setRecordings = (change: (list: RecordingRef[]) => void) =>
    editMeasurements((p) => {
      p.measurements ??= {};
      p.measurements.recordings ??= [];
      change(p.measurements.recordings);
    });

  const addLog = (i: number) =>
    attempt(async () => {
      const path = await pickFile('Engine-speed log', ['csv', 'txt']);
      if (path) setRecordings((list) => (list[i].rpm_log = path));
    });

  const tracked = (t: OrderTracks | string | null | undefined): t is OrderTracks => !!t && typeof t !== 'string';
  const plotData = (t: OrderTracks): uPlot.AlignedData => [t.rpm, ...t.orders.map((o) => o.level_db)];
  const series = (t: OrderTracks): uPlot.Series[] =>
    t.orders.map((o, i) => ({
      label: `${o.order}`,
      stroke: ORDER_COLOURS[i % ORDER_COLOURS.length],
      width: o.order === firing ? 2.5 : 1,
    }));
  const chips = (r: RecordingRef, t: OrderTracks): Chip[] => [
    { text: 'measured', detail: r.path },
    t.rpm_estimated
      ? {
          text: 'rpm estimated from the recording',
          tone: 'warn',
          detail: 'No engine-speed log: the speed whose orders 1–8 carry the most power',
        }
      : { text: `rpm from ${fileName(r.rpm_log ?? '')}`, detail: `log offset ${r.log_offset_s ?? 0} s` },
    t.calibrated
      ? { text: 'calibrated', detail: `a full-scale sine is ${r.calibration_db} dB re 20 µPa` }
      : { text: 'uncalibrated: dB re full scale', tone: 'warn' },
    { text: `${t.frames[1]}/${t.frames[0]} frames in the sweep` },
  ];
  const hz = (r: number) => (firing * r) / 60;

  // Cabin transfer function.
  let method = $state<'order_ratio' | 'impulse'>('order_ratio');
  let exterior = $state(0);
  let interior = $state(1);
  let impulse = $state<{ exterior: string | null; interior: string | null }>({ exterior: null, interior: null });
  let measuring = $state(false);
  $effect(() => {
    const ext = recordings.findIndex((r) => r.position === 'exterior');
    const int = recordings.findIndex((r) => r.position === 'interior');
    untrack(() => {
      if (ext >= 0) exterior = ext;
      if (int >= 0) interior = int;
    });
  });
  const store = (cabin: CabinTf | null) =>
    editMeasurements((p) => {
      p.measurements ??= {};
      if (cabin) p.measurements.cabin_tf = cabin;
      else delete p.measurements.cabin_tf;
    });
  const measure = () =>
    attempt(async () => {
      if (!app.project) return;
      measuring = true;
      try {
        const project = $state.snapshot(app.project) as Project;
        await store(
          method === 'order_ratio'
            ? await backend.cabinTfOrders(project, exterior, interior)
            : await backend.cabinTfImpulse(impulse.exterior!, impulse.interior!),
        );
      } finally {
        measuring = false;
      }
    });
</script>

{#snippet drone(label: string, d: DroneReport | null | undefined, unit: string)}
  <dt>{label}</dt>
  <dd class="mono">
    {#if d}
      {rpm(d.peak_rpm)} ({hz(d.peak_rpm).toFixed(1)} Hz) · {db(d.peak_db)}{unit}
    {:else}
      unavailable
    {/if}
  </dd>
{/snippet}

<div class="measurements" data-testid="measurements">
  <section>
    <h2>Recordings</h2>
    <p class="muted">
      WAV or CAF of a slow run-up, at the receiver outside or at the driver's ear, with the OBD engine-speed log
      (CSV) taken on the same drive. Without a log the engine speed is estimated from the recording.
    </p>
    {#each recordings as r, i (i)}
      {@const t = tracks[i]}
      <div class="recording" data-testid="recording">
        <div class="line">
          <strong title={r.path}>{fileName(r.path)}</strong>
          <select
            value={r.position}
            onchange={(e) => setRecordings((list) => (list[i].position = e.currentTarget.value as RecordingRef['position']))}
          >
            <option value="exterior">exterior, at the receiver</option>
            <option value="interior">interior, driver's ear</option>
          </select>
          <button onclick={() => setRecordings((list) => list.splice(i, 1))}>Remove</button>
        </div>
        <div class="line">
          <span class="muted">engine speed: {r.rpm_log ? fileName(r.rpm_log) : 'estimated from the recording'}</span>
          <button onclick={() => addLog(i)}>Log…</button>
          {#if r.rpm_log}
            <button title="Estimate from the recording" onclick={() => setRecordings((list) => delete list[i].rpm_log)}>×</button>
          {/if}
        </div>
        <label class="row">
          <span class="label">Log at start</span>
          <input
            type="number"
            step="0.1"
            value={r.log_offset_s ?? 0}
            onchange={(e) => setRecordings((list) => (list[i].log_offset_s = Number(e.currentTarget.value)))}
          />
          <span class="unit">s on the log's clock</span>
        </label>
        <label class="row">
          <span class="label">Calibration</span>
          <input
            type="number"
            step="0.1"
            placeholder="uncalibrated"
            value={r.calibration_db ?? ''}
            onchange={(e) => {
              const v = e.currentTarget.value;
              setRecordings((list) => {
                if (v === '') delete list[i].calibration_db;
                else list[i].calibration_db = Number(v);
              });
            }}
          />
          <span class="unit">dB SPL of a full-scale sine</span>
        </label>
        {#if tracked(t)}
          <Plot
            title={`Measured engine orders: ${fileName(r.path)}`}
            provenance={chips(r, t)}
            data={plotData(t)}
            series={series(t)}
            x="rpm"
            y={t.calibrated ? 'dB re 20 µPa' : 'dB re full scale'}
            floor={60}
          />
          <dl data-testid="measured-drone">
            {@render drone('Measured drone', t.drone, t.calibrated ? '' : ' re FS')}
            {@render drone(
              `Predicted (${predicted?.label ?? 'none yet'})`,
              r.position === 'exterior' ? predicted?.metrics.drone_exterior : predicted?.metrics.drone_interior,
              '',
            )}
            {#if t.drone && predicted}
              {@const p = r.position === 'exterior' ? predicted.metrics.drone_exterior : predicted.metrics.drone_interior}
              {#if p}
                <dt>Apart</dt>
                <dd class="mono">
                  {Math.abs(p.peak_rpm - t.drone.peak_rpm).toFixed(0)} rpm,
                  {Math.abs(hz(p.peak_rpm) - hz(t.drone.peak_rpm)).toFixed(1)} Hz
                </dd>
              {/if}
            {/if}
          </dl>
        {:else if typeof t === 'string'}
          <p class="failure">{t}</p>
        {:else}
          <p class="muted">Analysing…</p>
        {/if}
      </div>
    {/each}
    <button onclick={addRecording}>Add recording…</button>
  </section>

  <section data-testid="cabin-tf">
    <h2>Cabin transfer function</h2>
    {#if tf}
      <Plot
        title="Cabin transfer function"
        provenance={[{ text: tf.method === 'impulse' ? 'measured: impulse' : 'measured: order ratio', detail: tf.source }]}
        data={[tf.gain_db.map((p) => p[0]), tf.gain_db.map((p) => p[1])]}
        series={[{ label: 'interior − exterior', stroke: '#c792ea', width: 2, points: { show: true, size: 4 } }]}
        x="Hz"
        y="dB"
        floor={40}
      />
      <div class="line">
        <span class="muted">{tf.source}</span>
        <button onclick={() => attempt(() => store(null))}>Remove</button>
      </div>
    {:else}
      <p class="muted">None measured: the interior drone is unavailable.</p>
    {/if}
    <h3>Measure</h3>
    <div class="line">
      <label><input type="radio" bind:group={method} value="order_ratio" /> order ratio</label>
      <label><input type="radio" bind:group={method} value="impulse" /> impulse</label>
    </div>
    {#if method === 'order_ratio'}
      <p class="muted">
        Two recordings of the same drive (a run-up, or a steady cruise): outside at the receiver and at the driver's
        ear, both calibrated or on the same phone at the same gain.
      </p>
      <label class="row">
        <span class="label">Exterior</span>
        <select bind:value={exterior}>
          {#each recordings as r, i}<option value={i}>{fileName(r.path)}</option>{/each}
        </select>
      </label>
      <label class="row">
        <span class="label">Interior</span>
        <select bind:value={interior}>
          {#each recordings as r, i}<option value={i}>{fileName(r.path)}</option>{/each}
        </select>
      </label>
      <button onclick={measure} disabled={measuring || recordings.length < 2 || exterior === interior}>Measure</button>
    {:else}
      <p class="muted">
        Claps or balloon pops at the tailpipe, recorded at the receiver outside and at the driver's ear.
      </p>
      {#each ['exterior', 'interior'] as const as side}
        <div class="line">
          <button
            onclick={() =>
              attempt(async () => {
                impulse[side] = (await pickFile('Recording', ['wav', 'caf'])) ?? impulse[side];
              })}>{side === 'exterior' ? 'Exterior…' : 'Interior…'}</button
          >
          <span class="muted">{impulse[side] ? fileName(impulse[side]) : 'none'}</span>
        </div>
      {/each}
      <button onclick={measure} disabled={measuring || !impulse.exterior || !impulse.interior}>Measure</button>
    {/if}
  </section>
  <Validation />
  {#if failure}<p class="failure">{failure}</p>{/if}
</div>

<style>
  /* Sections side by side in the wide dock. */
  .measurements {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(460px, 1fr));
    align-items: start;
    gap: 0 22px;
    padding: 10px 14px;
  }

  section {
    margin-bottom: 14px;
    min-width: 0;
  }

  h3 {
    font-size: 12px;
    margin: 10px 0 4px;
  }

  .recording {
    border-left: 2px solid var(--line);
    padding-left: 8px;
    margin-bottom: 12px;
  }

  .line {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    margin: 4px 0;
  }

  .row {
    display: grid;
    grid-template-columns: 110px minmax(0, 1fr) auto;
    align-items: center;
    gap: 4px;
    margin: 3px 0;
  }

  .label {
    color: var(--muted);
  }

  .unit {
    color: var(--muted);
    font-size: 11px;
  }

  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 2px 10px;
    margin: 0 0 6px;
  }

  dt {
    color: var(--muted);
  }

  dd {
    margin: 0;
  }

  .failure {
    color: var(--bad);
  }
</style>
