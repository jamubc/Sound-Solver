<script lang="ts">
  import { untrack } from 'svelte';
  import { backend } from './backend';
  import { rpm } from './format';
  import { app, pinBaseline } from './state.svelte';
  import type { OrderDifference } from './types/api';
  import type { Project } from './types/project';

  const base = $derived(app.baseline);
  const firing = $derived((app.project?.engine.geometry.firing_order.length ?? 4) / 2);
  /** Like against like: time domain when both have it, else the previews. */
  const pair = $derived.by(() => {
    if (base?.timeDomain && app.timeDomain.result) {
      return { a: base.timeDomain.points, b: app.timeDomain.result.points, label: 'time domain', tone: 'td' };
    }
    if (base?.preview && app.preview) {
      return { a: base.preview.points, b: app.preview.points, label: 'four-pole preview', tone: 'fp' };
    }
    return null;
  });

  let differences = $state<OrderDifference[] | null>(null);
  let failure = $state<string | null>(null);
  let run = 0;
  $effect(() => {
    const p = pair;
    differences = null;
    if (!p) return;
    const project = untrack(() => $state.snapshot(app.project)) as Project;
    const mine = ++run;
    backend.compare(project, $state.snapshot(p.a), $state.snapshot(p.b)).then(
      (d) => {
        if (mine === run) [differences, failure] = [d, null];
      },
      (e) => {
        if (mine === run) failure = String(e);
      },
    );
  });
  const signed = (x: number) => `${x >= 0 ? '+' : ''}${x.toFixed(1)} dB`;
</script>

<section data-testid="compare">
  <h2>
    Compare
    {#if pair && differences}<span class="chip {pair.tone}">{pair.label}</span>{/if}
  </h2>
  <div class="line">
    <button onclick={pinBaseline} disabled={!app.preview}>Pin as baseline</button>
    {#if base}
      <span class="muted">baseline: {base.name}</span>
      <button onclick={() => (app.baseline = null)}>Clear</button>
    {/if}
  </div>
  {#if !base}
    <p class="muted">Pin these results, then edit or open another configuration: every plot overlays the baseline, dashed.</p>
  {:else if !pair}
    <p class="muted">Solve this configuration with the baseline's solver to compare them.</p>
  {:else if differences}
    <table data-testid="differences">
      <thead>
        <tr><th>Order</th><th>Cruise band</th><th>Largest</th><th>Peak: baseline → now</th></tr>
      </thead>
      <tbody>
        {#each differences as d (d.order)}
          <tr class:firing={d.order === firing}>
            <td>{d.order}</td>
            <td class="mono">{d.cruise_db == null ? '—' : signed(d.cruise_db)}</td>
            <td class="mono">{d.largest ? `${signed(d.largest[1])} at ${rpm(d.largest[0])}` : '—'}</td>
            <td class="mono">{d.peak_db.map((x) => (x == null ? '—' : x.toFixed(1))).join(' → ')}</td>
          </tr>
        {/each}
      </tbody>
    </table>
    <p class="muted note">Now minus baseline at the receiver, dB; cruise band mean over speeds both solved.</p>
  {/if}
  {#if failure}<p class="failure">{failure}</p>{/if}
</section>

<style>
  section {
    margin-bottom: 14px;
    padding-bottom: 10px;
    border-bottom: 1px solid var(--line);
  }

  .line {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  table {
    width: 100%;
    border-collapse: collapse;
    margin-top: 6px;
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

  tr.firing td {
    color: var(--td);
  }

  .note {
    font-size: 11px;
  }

  .failure {
    color: var(--bad);
  }
</style>
