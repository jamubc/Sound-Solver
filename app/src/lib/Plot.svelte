<script lang="ts">
  import { onMount } from 'svelte';
  import uPlot from 'uplot';
  import type { Chip } from './provenance';

  interface Props {
    title: string;
    /** Where the numbers come from; a plot without provenance is a bug. */
    provenance: Chip[];
    data: uPlot.AlignedData | null;
    series: uPlot.Series[];
    x: string;
    y: string;
    /** Why nothing is shown, when a quantity cannot be computed. */
    unavailable?: string | null;
    height?: number;
    bars?: boolean;
    /** Show only the top `floor` of the y range (dB levels whose quiet lines would dwarf it). */
    floor?: number;
    /** Called with the x value under the cursor on click. */
    onpick?: (x: number) => void;
  }

  let {
    title,
    provenance,
    data,
    series,
    x,
    y,
    unavailable = null,
    height = 190,
    bars = false,
    floor,
    onpick,
  }: Props = $props();

  let host: HTMLDivElement;
  let plot: uPlot | null = null;
  const axis = (label: string): uPlot.Axis => ({
    label,
    stroke: '#8a94a6',
    grid: { stroke: '#232a36', width: 1 },
    ticks: { stroke: '#2b3342', width: 1 },
  });

  function draw() {
    plot?.destroy();
    plot = null;
    if (!host || !data || unavailable) return;
    plot = new uPlot(
      {
        width: host.clientWidth,
        height,
        scales: {
          x: { time: false },
          y: floor === undefined ? {} : { range: (_u, min, max) => [Math.max(min, max - floor), max + 3] },
        },
        series: [
          { label: x },
          ...series.map((s) =>
            bars
              ? {
                  ...s,
                  paths: uPlot.paths.bars!({ size: [0.9, 6] }),
                  fill: s.stroke as string,
                  fillTo: (u: uPlot) => u.scales.y.min ?? 0,
                }
              : s,
          ),
        ],
        axes: [axis(x), axis(y)],
        legend: { live: true },
        hooks: {
          ready: [
            (u) =>
              u.over.addEventListener('click', () => {
                const i = u.cursor.idx;
                if (i != null) onpick?.(u.data[0][i]);
              }),
          ],
        },
      },
      data,
      host,
    );
  }

  $effect(draw);

  onMount(() => {
    const observer = new ResizeObserver(() => plot?.setSize({ width: host.clientWidth, height }));
    observer.observe(host);
    return () => {
      observer.disconnect();
      plot?.destroy();
    };
  });
</script>

<figure data-testid="plot">
  <figcaption>
    <span class="title">{title}</span>
    <span class="chips" data-testid="provenance">
      {#each provenance as c}
        <span class="chip {c.tone ?? ''}" title={c.detail ?? c.text}>{c.text}</span>
      {/each}
      {#if data && !unavailable && !provenance.length}
        <span class="chip bad" title="Numbers without a stated source are a bug">no provenance</span>
      {/if}
    </span>
  </figcaption>
  {#if unavailable}
    <div class="unavailable" style:height="{height}px">{unavailable}</div>
  {/if}
  <div bind:this={host} class:hidden={!!unavailable}></div>
</figure>

<style>
  figure {
    margin: 0 0 14px;
  }

  figcaption {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 4px 8px;
    margin-bottom: 4px;
  }

  .title {
    font-weight: 600;
  }

  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }

  .chip {
    font-size: 10.5px;
    padding: 0 6px;
    border-radius: 8px;
    border: 1px solid var(--line);
    color: var(--muted);
    white-space: nowrap;
  }

  .chip.td {
    border-color: var(--td);
    color: var(--td);
  }

  .chip.fp {
    border-color: var(--fp);
    color: var(--fp);
  }

  .chip.warn {
    border-color: var(--warn);
    color: var(--warn);
  }

  .chip.bad {
    border-color: var(--bad);
    color: var(--bad);
  }

  .unavailable {
    display: grid;
    place-items: center;
    color: var(--muted);
    border: 1px dashed var(--line);
    border-radius: 4px;
    padding: 12px;
    text-align: center;
  }

  .hidden {
    display: none;
  }

  figure :global(.u-legend) {
    font-size: 11px;
    color: var(--muted);
  }
</style>
