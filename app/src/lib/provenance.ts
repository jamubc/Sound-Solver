// Provenance chips: every plot states which solver, grid, gas-temperature profile and
// convergence its numbers come from.
import type { PointResult } from './types/api';

export interface Chip {
  text: string;
  detail?: string;
  tone?: 'td' | 'fp' | 'warn' | 'bad';
}

/** Chips for points from one solver; `total` counts a sweep still running. */
export function provenance(points: readonly PointResult[], total?: number): Chip[] {
  if (!points.length) return [];
  const p = points[0].provenance;
  const chips: Chip[] = [];
  if (p.solver === 'time_domain') {
    const converged = points.filter((x) => x.provenance.time_domain?.converged).length;
    chips.push(
      { text: 'time domain', tone: 'td', detail: 'Nonlinear time-domain gas dynamics: the reference result' },
      {
        text: `${converged}/${points.length} converged${total && points.length < total ? `, ${points.length}/${total} solved` : ''}`,
        tone: converged < points.length ? 'warn' : undefined,
        detail: 'Points that did not reach periodicity are drawn dashed',
      },
      {
        text: `Δx ${p.dx_target_mm} mm · CFL ${p.time_domain?.cfl}`,
        detail: `${p.cells} cells (${p.dx_min_mm.toFixed(1)}–${p.dx_max_mm.toFixed(1)} mm), limiter ${p.time_domain?.limiter}`,
      },
    );
  } else {
    chips.push(
      { text: 'four-pole preview', tone: 'fp', detail: 'Linear frequency-domain preview of the same network' },
      { text: `Δx ${p.dx_target_mm} mm`, detail: `${p.cells} cells` },
    );
    if (points.some((x) => x.provenance.small_signal)) {
      chips.push({ text: 'small-signal', tone: 'warn', detail: 'Valves or orifices: valid for small signals only' });
    }
  }
  chips.push({
    text: p.thermal_profile.startsWith('thermal model') ? 'thermal model' : p.thermal_profile.split(';')[0],
    detail: `Gas temperature profile (first point): ${p.thermal_profile}`,
  });
  const warnings = [...new Set(points.flatMap((x) => x.provenance.warnings))];
  if (warnings.length) {
    chips.push({ text: `${warnings.length} warning${warnings.length > 1 ? 's' : ''}`, tone: 'warn', detail: warnings.join('\n') });
  }
  return chips;
}
