// Display formatting. Lengths are millimetres throughout; pipe diameters also in inches.

export const mm = (x: number, digits = 1) => `${x.toFixed(digits)} mm`;

/** Pipe outside diameter, always in both units. */
export const od = (x: number) => `${x.toFixed(1)} mm (${(x / 25.4).toFixed(2)} in)`;

export const db = (x: number) => `${x.toFixed(1)} dB`;

export const rpm = (x: number) => `${Math.round(x)} rpm`;

export const vec = (v: readonly number[]) => v.map((x) => x.toFixed(0)).join(', ');
