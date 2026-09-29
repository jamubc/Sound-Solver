// Display formatting. The project holds lengths in millimetres; they show in millimetres or
// inches (`app.inches`), pipe diameters always in both.
import { app } from './state.svelte';
import type { Input, Uncertainty } from './types/project';

const MM_PER_IN = 25.4;

/** The unit lengths show in. */
export const lengthUnit = () => (app.inches ? 'in' : 'mm');

/** A length held in millimetres, in the unit it shows in (inches with one more decimal). */
export const mm = (x: number, digits = 1) =>
  app.inches ? `${(x / MM_PER_IN).toFixed(digits + 1)} in` : `${x.toFixed(digits)} mm`;

/** A length held in millimetres as an input shows it; `fromShown` takes it back. */
export const toShown = (x: number) => (app.inches ? +(x / MM_PER_IN).toFixed(3) : x);
export const fromShown = (x: number) => (app.inches ? x * MM_PER_IN : x);

/** Pipe outside diameter, always in both units. */
export const od = (x: number) =>
  app.inches
    ? `${(x / MM_PER_IN).toFixed(2)} in (${x.toFixed(1)} mm)`
    : `${x.toFixed(1)} mm (${(x / MM_PER_IN).toFixed(2)} in)`;

export const db = (x: number) => `${x.toFixed(1)} dB`;

export const rpm = (x: number) => `${Math.round(x)} rpm`;

/** A length held in millimetres as a bare number in the unit it shows in, to 1 mm or 0.1 in. */
export const shown = (x: number) => (app.inches ? (x / MM_PER_IN).toFixed(1) : x.toFixed(0));

/** A point held in millimetres, in the unit lengths show in. */
export const vec = (v: readonly number[]) => v.map(shown).join(', ');

export const JOINT: Record<string, string> = { butt: 'butt weld', slip: 'slip joint', v_band: 'V-band', flange: 'flange' };

/** Engine orders 1–8. */
export const ORDER_COLOURS = ['#9aa6b8', '#ff9f43', '#6aa9ff', '#5fd08a', '#c792ea', '#f5c451', '#ff6b6b', '#4fd1c5'];

export const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path;

/** A range for people: "1.2–1.4", "±5 %", "±10", "×0.5–×2". */
export function describeRange(u: Uncertainty): string {
  return u.kind === 'range'
    ? `${u.low}–${u.high}`
    : u.kind === 'relative'
      ? `±${+(100 * u.fraction).toFixed(3)} %`
      : u.kind === 'absolute'
        ? `±${u.plus_minus}`
        : `×${+(1 / u.factor).toFixed(3)}–×${u.factor}`;
}

/** A recorded input for people: provenance, the stated range or "default range", and its source. */
export function describeInput(input: Input): string {
  const u = input.uncertainty;
  const range = u
    ? describeRange(u)
    : input.provenance === 'estimated' || input.provenance === 'derived'
      ? 'default range for its class'
      : 'no range stated';
  return `${input.provenance}, ${range}: ${input.source}`;
}

/** Dot class of a recorded input: its provenance, or `default` for an estimate on its class's default range. */
export function inputClass(input: Input): string {
  return !input.uncertainty && (input.provenance === 'estimated' || input.provenance === 'derived')
    ? 'default'
    : input.provenance;
}
