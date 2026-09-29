// Display formatting. Lengths are millimetres throughout; pipe diameters also in inches.

export const mm = (x: number, digits = 1) => `${x.toFixed(digits)} mm`;

/** Pipe outside diameter, always in both units. */
export const od = (x: number) => `${x.toFixed(1)} mm (${(x / 25.4).toFixed(2)} in)`;

export const db = (x: number) => `${x.toFixed(1)} dB`;

export const rpm = (x: number) => `${Math.round(x)} rpm`;

export const vec = (v: readonly number[]) => v.map((x) => x.toFixed(0)).join(', ');

export const JOINT: Record<string, string> = { butt: 'butt weld', slip: 'slip joint', v_band: 'V-band', flange: 'flange' };

/** Engine orders 1–8. */
export const ORDER_COLOURS = ['#9aa6b8', '#ff9f43', '#6aa9ff', '#5fd08a', '#c792ea', '#f5c451', '#ff6b6b', '#4fd1c5'];

export const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path;
