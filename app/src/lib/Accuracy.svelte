<script lang="ts">
  import { backend } from './backend';
  import { describeRange } from './format';
  import { app } from './state.svelte';
  import type { Project } from './types/project';

  const sweep = $derived(app.project?.operating.sweep_rpm ?? [1000, 6500, 50]);
  const estimated = $derived(
    Object.entries(app.project?.inputs ?? {}).filter(
      ([path, i]) =>
        (i.provenance === 'estimated' || i.provenance === 'derived') &&
        !path.startsWith('fabrication.') &&
        !path.startsWith('measurements.'),
    ),
  );
  let rpm = $state<number | null>(null);
  $effect(() => {
    if (rpm !== null) return;
    const metrics = app.timeDomain.result?.metrics ?? app.preview?.metrics;
    const drone = metrics?.drone_interior ?? metrics?.drone_exterior;
    const [cruise0, cruise1] = app.project?.operating.cruise_band_rpm ?? [2000, 2000];
    rpm = app.rpm ?? drone?.peak_rpm ?? Math.round((cruise0 + cruise1) / 2);
  });

  let running = $state(false);
  let progress = $state<string | null>(null);
  let failure = $state<string | null>(null);
  const current = $derived(
    app.sensitivity && app.project && app.sensitivity.project === JSON.stringify(app.project) ? app.sensitivity.result : null,
  );
  const stale = $derived(!!app.sensitivity && !current);

  async function run() {
    if (!app.project || rpm === null) return;
    failure = null;
    running = true;
    const project = $state.snapshot(app.project) as Project;
    try {
      const result = await backend.sensitivity(project, rpm, (done, total) => (progress = `${done} of ${total} runs`));
      app.sensitivity = { result, project: JSON.stringify(project) };
    } catch (e) {
      failure = String(e);
    } finally {
      running = false;
      progress = null;
    }
  }

  const shown = $derived.by(() => {
    const s = app.sensitivity?.result;
    if (!s) return [];
    const loud = Math.max(...s.base_db.filter((v): v is number => v !== null && v !== undefined));
    return s.bands_hz
      .map((hz, b) => ({ hz, base: s.base_db[b], combined: s.combined_db[b], resolved: hz * 10 ** 0.05 <= s.limit_hz }))
      .filter((b) => b.base !== null && b.base !== undefined && b.base > loud - 40);
  });
  const hz = (f: number) => (f >= 1000 ? `${+(f / 1000).toFixed(1)} k` : `${Math.round(f)}`);
  const f1 = (v: number | null | undefined) => (v === null || v === undefined ? '—' : v.toFixed(1));
</script>

<div class="accuracy" data-testid="accuracy">
  <section>
    <h2>Uncertainty from the inputs</h2>
    <p class="muted">
      Each estimated or derived input goes to both ends of its range with everything else held; the receiver's ⅓-octave
      levels move by what the table shows. The band combines them root-sum-square, taking the inputs as independent and
      their effects as linear. {estimated.length} inputs, two time-domain runs each.
    </p>
    <div class="line">
      <label>
        at
        <input class="rpm" type="number" step={sweep[2]} min={sweep[0]} max={sweep[1]} bind:value={rpm} /> rpm
      </label>
      <button class="primary" onclick={run} disabled={running || !app.project || !!app.invalid} data-testid="run-sensitivity">
        Sweep the inputs
      </button>
      {#if running}
        <button onclick={() => backend.cancelSensitivity()}>Cancel</button><span class="muted">{progress ?? 'starting…'}</span>
      {/if}
    </div>
    {#if stale}<p class="warn">The project has changed since this sweep: run it again.</p>{/if}
    {#if failure}<p class="bad">{failure}</p>{/if}
    {#if app.sensitivity}
      {@const s = app.sensitivity.result}
      <p class="mono">
        {Math.round(s.rpm)} rpm · the model resolves bands below {Math.round(s.limit_hz)} Hz · {s.inputs.filter((i) => i.error).length}
        inputs outside the band
      </p>
      <table class="mono" data-testid="combined">
        <thead><tr><th>Hz</th><th>level, dB</th><th>±</th><th></th></tr></thead>
        <tbody>
          {#each shown as b}
            <tr class:off={!b.resolved}>
              <td>{hz(b.hz)}</td><td>{f1(b.base)}</td><td>{f1(b.combined)}</td><td class="muted">{b.resolved ? '' : 'beyond the model'}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  </section>

  {#if app.sensitivity}
    {@const s = app.sensitivity.result}
    <section data-testid="improve">
      <h2>Improve accuracy</h2>
      <p class="muted">
        Supply these first: inputs on their class's default range, then by the largest change they make in the bands the model
        resolves. A measured or published value replaces an estimate without any other change.
      </p>
      <table class="mono inputs">
        <thead><tr><th>input</th><th>as</th><th>range</th><th>impact, dB</th><th></th></tr></thead>
        <tbody>
          {#each s.inputs as i}
            <tr>
              <td>{i.path}</td>
              <td class:bad={i.default_range}>{i.provenance}{i.default_range ? ', default range' : ''}</td>
              <td>{describeRange(i.range)}</td>
              <td>{i.error ? '—' : i.impact_db.toFixed(1)}</td>
              <td class="muted">{i.error ?? i.note ?? ''}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </section>
  {/if}
</div>

<style>
  .accuracy {
    display: grid;
    grid-template-columns: minmax(320px, 1fr) minmax(420px, 1.4fr);
    gap: 18px;
    padding: 10px 14px;
    height: 100%;
    overflow: auto;
    align-items: start;
  }

  section {
    min-width: 0;
  }

  .line {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }

  .rpm {
    width: 62px;
  }

  table {
    border-collapse: collapse;
    font-size: 11px;
    margin-top: 6px;
  }

  td,
  th {
    padding: 1px 10px 1px 0;
    text-align: right;
    vertical-align: top;
  }

  th {
    color: var(--muted);
    font-weight: 500;
  }

  .inputs td:first-child,
  .inputs th:first-child,
  .inputs td:last-child {
    text-align: left;
  }

  .inputs td:last-child {
    max-width: 360px;
  }

  tr.off td {
    opacity: 0.55;
  }

  .warn {
    color: var(--warn);
  }

  .bad {
    color: var(--bad);
  }
</style>
