<script lang="ts">
  import type uPlot from 'uplot';
  import Compare from './Compare.svelte';
  import DroneReport from './DroneReport.svelte';
  import { ORDER_COLOURS } from './format';
  import Plot from './Plot.svelte';
  import { type Chip, provenance } from './provenance';
  import { app, solved } from './state.svelte';
  import type { PointResult } from './types/api';

  const fp = $derived(app.preview ? solved(app.preview.points) : []);
  const td = $derived(solved(app.timeDomain.points));
  const tdComplete = $derived(app.timeDomain.result !== null);
  /** Plots other than the firing-order comparison use the reference once it is complete. */
  const best = $derived(tdComplete ? td : fp);
  const firingOrder = $derived(
    app.preview?.metrics.firing_order ?? (app.project?.engine.geometry.firing_order.length ?? 4) / 2,
  );
  // The pinned baseline, drawn dashed on every plot: its reference once complete, like the rest.
  const base = $derived(app.baseline);
  const baseFp = $derived(base?.preview ? solved(base.preview.points) : []);
  const baseTd = $derived(base?.timeDomain ? solved(base.timeDomain.points) : []);
  const baseBest = $derived(baseTd.length ? baseTd : baseFp);
  const BASE = '#8a94a6';
  const baseChips = (points: PointResult[]): Chip[] =>
    base && points.length
      ? [
          {
            text: `baseline: ${base.name}`,
            detail: provenance(points)
              .map((c) => c.text)
              .join(' · '),
          },
        ]
      : [];
  const bestChips = $derived([...provenance(best, app.timeDomain.total), ...baseChips(baseBest)]);
  const rpms = $derived([...new Set([...fp, ...td, ...baseFp, ...baseTd].map((p) => p.rpm))].sort((a, b) => a - b));
  /** `f` of the baseline point at each speed of `points`. */
  const baselineAt = (points: PointResult[], f: (p: PointResult) => number | null) => {
    const byRpm = new Map(baseBest.map((p) => [p.rpm, p]));
    return points.map((p) => {
      const b = byRpm.get(p.rpm);
      return b ? f(b) : null;
    });
  };

  const converged = (p: PointResult) => p.provenance.time_domain?.converged ?? true;
  const firing = (p: PointResult) =>
    p.spectrum.find((l) => Math.abs(l.order - firingOrder) < 1e-9)?.spl_db ?? null;
  const along = (points: PointResult[], f: (p: PointResult) => number | null) => {
    const byRpm = new Map(points.map((p) => [p.rpm, p]));
    return rpms.map((r) => {
      const p = byRpm.get(r);
      return p ? f(p) : null;
    });
  };

  const firingData = $derived.by((): uPlot.AlignedData | null => {
    if (!rpms.length) return null;
    const ok = along(td, (p) => (converged(p) ? firing(p) : null));
    // Unconverged points with their neighbours, so the dashed stretch joins the solid line.
    const all = along(td, firing);
    const bad = along(td, (p) => (converged(p) ? null : 1));
    const dashed = all.map((v, i) => (bad[i] || bad[i - 1] || bad[i + 1] ? v : null));
    const baseline = base ? [along(baseFp, firing), along(baseTd, firing)] : [];
    return [rpms, along(fp, firing), ok, dashed, ...baseline];
  });
  // Time-domain points arrive one by one across the sweep: markers, joined across unsolved speeds.
  const firingSeries = $derived<uPlot.Series[]>([
    { label: 'four-pole', stroke: '#6aa9ff', width: 1.5 },
    { label: 'time domain', stroke: '#ff9f43', width: 2, spanGaps: true, points: { show: true, size: 5 } },
    { label: 'unconverged', stroke: '#ff9f43', width: 1.5, dash: [5, 4], points: { show: true, size: 5 } },
    ...(base
      ? [
          { label: 'baseline four-pole', stroke: BASE, width: 1.5, dash: [6, 4], spanGaps: true },
          { label: 'baseline time domain', stroke: '#c9a27a', width: 1.5, dash: [6, 4], spanGaps: true },
        ]
      : []),
  ]);
  const firingChips = $derived([
    ...provenance(fp),
    ...provenance(td, app.timeDomain.total),
    ...baseChips([...baseFp, ...baseTd]),
  ]);

  const order = (p: PointResult, k: number) => p.orders.find((o) => o[0] === k)?.[2] ?? null;
  const ordersData = $derived.by((): uPlot.AlignedData | null =>
    best.length
      ? [
          best.map((p) => p.rpm),
          ...[1, 2, 3, 4, 5, 6, 7, 8].map((k) => best.map((p) => order(p, k))),
          ...(base ? [baselineAt(best, (p) => order(p, firingOrder))] : []),
        ]
      : null,
  );
  const ordersSeries = $derived<uPlot.Series[]>([
    ...ORDER_COLOURS.map((c, i) => ({ label: `${i + 1}`, stroke: c, width: i + 1 === firingOrder ? 2.5 : 1 })),
    ...(base ? [{ label: `baseline ${firingOrder}`, stroke: BASE, width: 1.5, dash: [6, 4] }] : []),
  ]);

  const spectrumPoint = $derived(
    app.rpm === null ? undefined : (td.find((p) => p.rpm === app.rpm) ?? fp.find((p) => p.rpm === app.rpm)),
  );
  const baseSpectrum = $derived(base && spectrumPoint ? baseBest.find((p) => p.rpm === spectrumPoint.rpm) : undefined);
  const spectrumData = $derived.by((): uPlot.AlignedData | null => {
    if (!spectrumPoint) return null;
    const lines = (p: PointResult | undefined) => new Map((p?.spectrum ?? []).map((l) => [l.frequency_hz, l.spl_db]));
    const [now, then] = [lines(spectrumPoint), lines(baseSpectrum)];
    const hz = [...new Set([...now.keys(), ...then.keys()])].sort((a, b) => a - b);
    return [hz, hz.map((f) => now.get(f) ?? null), ...(baseSpectrum ? [hz.map((f) => then.get(f) ?? null)] : [])];
  });

  const levelsData = $derived.by((): uPlot.AlignedData | null =>
    best.length
      ? [
          best.map((p) => p.rpm),
          best.map((p) => p.overall_dba),
          best.map((p) => p.overall_db),
          ...(base ? [baselineAt(best, (p) => p.overall_dba)] : []),
        ]
      : null,
  );

  const backpressureData = $derived.by((): uPlot.AlignedData | null =>
    td.length
      ? [
          td.map((p) => p.rpm),
          td.map((p) => p.backpressure_pa ?? null),
          ...(baseTd.length ? [baselineAt(td, (p) => p.backpressure_pa ?? null)] : []),
        ]
      : null,
  );

  const pick = (rpm: number) => (app.rpm = rpm);
</script>

<div class="plots">
  <DroneReport />
  <Compare />
  <Plot
    title={`Firing order (${firingOrder}) at the receiver`}
    provenance={firingChips}
    data={firingData}
    series={firingSeries}
    x="rpm"
    y="dB re 20 µPa"
    floor={60}
    unavailable={rpms.length ? null : app.invalid ? 'The project is not solvable as edited.' : 'Solving…'}
    onpick={pick}
  />
  <Plot
    title={spectrumPoint ? `Spectrum at ${spectrumPoint.rpm} rpm` : 'Spectrum'}
    provenance={spectrumPoint ? [...provenance([spectrumPoint]), ...baseChips(baseSpectrum ? [baseSpectrum] : [])] : []}
    data={spectrumData}
    series={[
      { label: 'SPL', stroke: spectrumPoint?.provenance.solver === 'time_domain' ? '#ff9f43' : '#6aa9ff' },
      ...(baseSpectrum ? [{ label: 'baseline', stroke: 'rgba(138, 148, 166, 0.55)' }] : []),
    ]}
    x="Hz"
    y="dB re 20 µPa"
    floor={60}
    bars
    unavailable={spectrumPoint ? null : 'Click an engine speed on a plot to show its spectrum.'}
  />
  <Plot
    title="Engine orders 1–8"
    provenance={bestChips}
    data={ordersData}
    series={ordersSeries}
    x="rpm"
    y="dB re 20 µPa"
    floor={60}
    unavailable={best.length ? null : 'No solution yet.'}
    onpick={pick}
  />
  <Plot
    title="Overall level"
    provenance={bestChips}
    data={levelsData}
    series={[
      { label: 'dB(A)', stroke: '#5fd08a', width: 2 },
      { label: 'dB(Lin)', stroke: '#9aa6b8', width: 1.5 },
      ...(base ? [{ label: 'baseline dB(A)', stroke: BASE, width: 1.5, dash: [6, 4] }] : []),
    ]}
    x="rpm"
    y="dB"
    floor={60}
    unavailable={best.length ? null : 'No solution yet.'}
    onpick={pick}
  />
  <Plot
    title="Mean backpressure at the downpipe flange"
    provenance={[...provenance(td, app.timeDomain.total), ...baseChips(baseTd)]}
    data={backpressureData}
    series={[
      { label: 'backpressure', stroke: '#ff9f43', width: 2 },
      ...(baseTd.length ? [{ label: 'baseline', stroke: BASE, width: 1.5, dash: [6, 4] }] : []),
    ]}
    x="rpm"
    y="Pa"
    unavailable={td.length
      ? null
      : 'Mean flow comes from the time-domain solution only: run “Solve time domain”.'}
    onpick={pick}
  />
</div>

<style>
  .plots {
    padding: 10px 12px;
  }
</style>
