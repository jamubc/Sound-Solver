<script lang="ts">
  import { notice } from './actions.svelte';
  import { lengthUnit, mm, od, vec } from './format';
  import { app, manifestOf } from './state.svelte';

  const td = $derived(app.timeDomain);
  const inFlight = $derived(Object.values(td.inFlight).sort((a, b) => a.rpm - b.rpm));
  const maxCycles = $derived(app.project?.solver.max_cycles ?? 1);

  // A clock for the elapsed time while the sweep runs.
  let now = $state(Date.now());
  $effect(() => {
    if (!td.running) return;
    const tick = setInterval(() => (now = Date.now()), 500);
    return () => clearInterval(tick);
  });
  const clock = (ms: number) => {
    const s = Math.round(ms / 1000);
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
  };
  /** Points done, counting each point in flight by its share of the cycle limit. */
  const fraction = $derived(
    td.total ? (td.points.length + inFlight.reduce((sum, p) => sum + Math.min(p.cycle / maxCycles, 1), 0)) / td.total : 0,
  );
  const elapsed = $derived(Math.max(0, now - td.startedAt));
  const left = $derived(fraction > 0.02 ? (elapsed * (1 - fraction)) / fraction : null);
  const running = $derived(
    inFlight
      .map((p) => `${p.rpm} rpm: cycle ${p.cycle}/${maxCycles}${p.residual == null ? '' : `, residual ${(100 * p.residual).toFixed(1)} %`}`)
      .join('\n'),
  );
  const converged = $derived(
    td.points.filter((p) => p.status === 'solved' && p.provenance.time_domain?.converged).length,
  );

  const selection = $derived.by(() => {
    const s = app.selection;
    const project = app.project;
    if (!s || !project) return 'Nothing selected: click a pipe, a bend point or an element';
    if (s.kind === 'element') {
      const e = project.system.elements.find((x) => x.id === s.id);
      return e ? `${e.id} · ${manifestOf(e.type)?.name ?? e.type} at (${vec(e.position_mm)}) ${lengthUnit()}` : s.id;
    }
    const id = s.kind === 'route' ? s.id : s.route;
    const r = project.system.routes.find((x) => x.id === id);
    const layout = app.layout?.routes.find((x) => x.id === id);
    if (!r) return id;
    const pipe = `${od(r.pipe.od_mm)} × ${mm(r.pipe.wall_mm)} ${r.pipe.material}`;
    if (s.kind === 'vertex') {
      return `${id} · bend point ${s.index + 1} at (${vec(r.via_mm?.[s.index] ?? [0, 0, 0])}) ${lengthUnit()} · ${pipe}`;
    }
    return `${id} · ${pipe}${layout ? ` · ${mm(layout.length_mm, 0)} centreline` : ''}`;
  });

  const preview = $derived(
    app.invalid
      ? 'preview unavailable'
      : app.previewError
        ? `preview failed: ${app.previewError}`
        : app.previewMs !== null
          ? `four-pole preview ${Math.round(app.previewMs)} ms`
          : 'previewing…',
  );
</script>

<footer data-testid="status">
  <span class="selection" title={selection}>{selection}</span>
  {#if notice.text}<span class="notice" class:bad={notice.bad} data-testid="notice">{notice.text}</span>{/if}
  <span class="solver">
    {#if td.running}
      <span class="progress" title={running || 'starting'} data-testid="td-progress">
        <span class="bar"><span style:width="{(100 * fraction).toFixed(1)}%"></span></span>
        time domain {td.points.length}/{td.total} · {inFlight.length} running · {clock(elapsed)}
        {#if left !== null}· ~{clock(left)} left{/if}
      </span>
    {:else if td.error}
      <span class="bad">time domain: {td.error}</span>
    {:else if td.result}
      <span>time domain: {td.points.length} speeds, {converged} converged</span>
    {/if}
    <span class="muted">{preview}</span>
  </span>
</footer>

<style>
  footer {
    grid-area: status;
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 3px 12px;
    border-top: 1px solid var(--line);
    background: var(--panel);
    font-size: 12px;
    min-width: 0;
  }

  .selection {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .notice {
    max-width: 40%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--good);
  }

  .solver {
    display: flex;
    align-items: center;
    gap: 14px;
    white-space: nowrap;
  }

  .progress {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--td);
  }

  .bar {
    width: 140px;
    height: 6px;
    border-radius: 3px;
    background: var(--panel-2);
    border: 1px solid var(--line);
    overflow: hidden;
  }

  .bar span {
    display: block;
    height: 100%;
    background: var(--td);
    transition: width 0.4s ease-out;
  }

  .bad {
    color: var(--bad);
  }
</style>
