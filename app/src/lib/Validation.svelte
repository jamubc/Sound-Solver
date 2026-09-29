<script lang="ts">
  import { open } from '@tauri-apps/plugin-dialog';
  import { backend, type Hold } from './backend';
  import { fileName } from './format';
  import { cancelRender, playClip, stop } from './player.svelte';
  import { app } from './state.svelte';
  import type { CaseReport } from './types/api';
  import type { Project } from './types/project';

  const recordings = $derived(app.project?.measurements?.recordings ?? []);
  const calibrated = $derived(
    recordings.map((r, i) => ({ r, i })).filter(({ r }) => r.calibration_db !== undefined && r.calibration_db !== null),
  );

  // A steady hold against a render of the same speed.
  let index = $state<number | null>(null);
  let from = $state(1);
  let to = $state(4);
  let background = $state<string | null>(null);
  let hold = $state<Hold | null>(null);
  let holding = $state(false);
  let progress = $state<string | null>(null);
  let failure = $state<string | null>(null);
  $effect(() => {
    if (index === null || !calibrated.some((c) => c.i === index)) index = calibrated[0]?.i ?? null;
  });

  async function check() {
    if (index === null || !app.project) return;
    failure = null;
    holding = true;
    try {
      hold = await backend.validateHold($state.snapshot(app.project) as Project, index, [from, to], background, (p) => {
        progress = p.stage === 'march' ? `rendering: ${Math.round(100 * p.fraction)} %` : `settling: cycle ${p.cycle}`;
      });
    } catch (e) {
      failure = String(e);
    } finally {
      holding = false;
      progress = null;
    }
  }

  const peak = $derived.by(() => {
    let m = 0;
    for (const c of hold ? [hold.measured, hold.predicted] : []) for (const s of c.samples) m = Math.max(m, Math.abs(s));
    return m;
  });
  const compared = $derived(hold?.comparison.bands.filter((b) => b.error_db !== null && b.error_db !== undefined) ?? []);
  const within = $derived(compared.filter((b) => Math.abs(b.error_db!) <= b.target_db).length);
  const fmt = (v: number | null | undefined, digits = 1) => (v === null || v === undefined ? '—' : v.toFixed(digits));
  const hz = (f: number) => (f >= 1000 ? `${+(f / 1000).toFixed(1)} k` : `${Math.round(f)}`);

  // The analytic verification suite, on demand.
  let reports = $state<CaseReport[]>([]);
  let verifying = $state(false);
  async function verify() {
    reports = [];
    verifying = true;
    try {
      await backend.verify((r) => (reports = [...reports, r]));
    } catch (e) {
      failure = String(e);
    } finally {
      verifying = false;
    }
  }
  const SUBSYSTEMS = [
    ['propagation', 'Propagation: the 1D solver and every element'],
    ['radiation', 'Radiation: the open end and the render'],
    ['source', 'Source: the engine and turbine'],
    ['flow noise', 'Flow noise'],
  ] as const;
  const status = (key: string) => {
    const mine = reports.filter((r) => r.subsystem === key);
    if (key === 'source') return { text: 'no analytic benchmark: waits on a measured pressure trace', tone: 'warn' };
    if (key === 'flow noise') return { text: 'not modelled', tone: 'warn' };
    if (!mine.length) return { text: verifying ? 'running…' : 'not run', tone: '' };
    const bad = mine.filter((r) => r.error || r.checks.some((c) => !c.pass));
    return bad.length
      ? { text: `FAILED: ${bad.map((r) => r.case).join(', ')}`, tone: 'bad' }
      : { text: `verified: ${mine.reduce((n, r) => n + r.checks.length, 0)} checks pass`, tone: 'good' };
  };
</script>

<section data-testid="hold-check">
  <h2>Check against a steady hold</h2>
  <p class="muted">
    A calibrated recording at the receiver (ISO 5130: 0.5 m, 45°, outlet height) or at the driver's ear, at a steady engine
    speed, set against a render of the same speed there. Recordings only validate: no input is ever fitted to one.
  </p>
  {#if calibrated.length}
    <label class="row">
      <span class="label">Recording</span>
      <select bind:value={index}>
        {#each calibrated as c}<option value={c.i}>{fileName(c.r.path)} ({c.r.position})</option>{/each}
      </select>
    </label>
    <label class="row">
      <span class="label">Hold</span>
      <input type="number" min="0" step="0.1" bind:value={from} /> to <input type="number" min="0" step="0.1" bind:value={to} /> s
    </label>
    <div class="line">
      <button
        onclick={async () => {
          const path = await open({ filters: [{ name: 'Background', extensions: ['wav', 'caf'] }], multiple: false, directory: false });
          if (typeof path === 'string') background = path;
        }}>Background…</button
      >
      <span class="muted">{background ? fileName(background) : 'no background sample'}</span>
      {#if background}<button onclick={() => (background = null)}>×</button>{/if}
    </div>
    <div class="line">
      <button class="primary" onclick={check} disabled={holding || index === null || !(to > from)} data-testid="check-hold">Check</button>
      {#if holding}<button onclick={cancelRender}>Cancel</button><span class="muted">{progress ?? 'reading…'}</span>{/if}
    </div>
  {:else}
    <p class="muted">Add a recording and enter its calibration first: the comparison is in dB re 20 µPa.</p>
  {/if}
  {#if hold}
    {@const c = hold.comparison}
    <p class="mono">
      {Math.round(c.rpm)} ± {Math.round(c.rpm_spread)} rpm{c.rpm_estimated ? ' (estimated from the recording)' : ''} ·
      {within} of {compared.length} bands within the indicative target
    </p>
    <div class="line">
      <button onclick={() => playClip(hold!.measured.samples, hold!.measured.sampleRate, peak)}>Play recording</button>
      <button onclick={() => playClip(hold!.predicted.samples, hold!.predicted.sampleRate, peak)}>Play render</button>
      <button onclick={stop}>Stop</button>
      <span class="muted">both at one scale</span>
    </div>
    <table class="bands mono" data-testid="hold-bands">
      <thead><tr><th>Hz</th><th>measured</th><th>render</th><th>error</th><th>target</th><th></th></tr></thead>
      <tbody>
        {#each c.bands.filter((b) => b.measured_db !== null || b.predicted_db !== null) as b}
          <tr class:off={!b.resolved}>
            <td>{hz(b.center_hz)}</td>
            <td>{fmt(b.measured_db)}</td>
            <td>{fmt(b.predicted_db)}</td>
            <td class:bad={b.error_db !== null && b.error_db !== undefined && Math.abs(b.error_db) > b.target_db}>{fmt(b.error_db)}</td>
            <td>±{b.target_db}</td>
            <td class="muted">{b.resolved ? '' : 'unresolved'}{b.note ? ` ${b.note}` : ''}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
  {#if failure}<p class="failure">{failure}</p>{/if}
</section>

<section data-testid="verification">
  <h2>Verification</h2>
  <p class="muted">The analytic benchmarks, run here as in CI: verified means every check of a subsystem passes.</p>
  <button onclick={verify} disabled={verifying}>{verifying ? 'Running…' : 'Run the benchmarks'}</button>
  <dl>
    {#each SUBSYSTEMS as [key, label]}
      {@const s = status(key)}
      <dt>{label}</dt>
      <dd class={s.tone}>{s.text}</dd>
    {/each}
  </dl>
  {#if reports.length}
    <details>
      <summary class="muted">{reports.reduce((n, r) => n + r.checks.length, 0)} checks in {reports.length} cases</summary>
      <table class="bands mono">
        <tbody>
          {#each reports.flatMap((r) => r.checks) as ch}
            <tr><td class={ch.pass ? 'good' : 'bad'}>{ch.pass ? 'ok' : 'FAIL'}</td><td>{ch.case}</td><td>{ch.metric}</td><td>{ch.value.toPrecision(3)} ≤ {ch.limit}</td></tr>
          {/each}
        </tbody>
      </table>
    </details>
  {/if}
</section>

<style>
  section {
    margin-bottom: 14px;
    min-width: 0;
  }

  .line {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    margin: 4px 0;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 4px 0;
  }

  .row input[type='number'] {
    width: 64px;
  }

  .label {
    width: 84px;
    color: var(--muted);
  }

  table.bands {
    border-collapse: collapse;
    font-size: 11px;
    margin-top: 6px;
  }

  table.bands td,
  table.bands th {
    padding: 1px 10px 1px 0;
    text-align: right;
  }

  table.bands th {
    color: var(--muted);
    font-weight: 500;
  }

  table.bands td:last-child {
    text-align: left;
  }

  tr.off td:not(:last-child) {
    opacity: 0.55;
  }

  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 2px 10px;
    font-size: 12px;
  }

  dt {
    color: var(--muted);
  }

  dd {
    margin: 0;
  }

  .good {
    color: var(--good);
  }

  .warn {
    color: var(--warn);
  }

  .bad,
  .failure {
    color: var(--bad);
  }
</style>
