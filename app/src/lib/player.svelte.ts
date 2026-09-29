// Plays the predicted receiver pressure (the backend's `listen`: the solved spectrum lines
// resynthesised) through Web Audio. The current design and the pinned baseline are scaled
// alike, so an A/B comparison keeps their difference in level.
import { backend, type Sound } from './backend';
import { app } from './state.svelte';
import type { PointOutcome } from './types/api';
import type { Project } from './types/project';

export type Which = 'current' | 'baseline';

export interface Take {
  sound: Sound;
  /** Equivalent continuous and peak level at the receiver, dB re 20 µPa. */
  leqDb: number;
  peakDb: number;
  solver: string;
}

export const player = $state({
  mode: 'steady' as 'steady' | 'runup',
  rpm: null as number | null,
  from: null as number | null,
  to: null as number | null,
  seconds: 12,
  interior: false,
  volume: 0.7,
  playing: null as Which | null,
  loading: false,
  error: null as string | null,
  takes: {} as Partial<Record<Which, Take>>,
  /** Audio-clock time the playback started, s, and its length. */
  startedAt: 0,
  duration: 0,
});

let context: AudioContext | null = null;
let source: AudioBufferSourceNode | null = null;
let gain: GainNode | null = null;

/** The solved points a design is heard from: the time domain once complete, else the preview. */
export function pointsOf(which: Which): { points: PointOutcome[]; solver: string } | null {
  const results =
    which === 'current'
      ? { td: app.timeDomain.result, fp: app.preview }
      : { td: app.baseline?.timeDomain ?? null, fp: app.baseline?.preview ?? null };
  if (results.td) return { points: results.td.points, solver: 'time domain' };
  if (results.fp) return { points: results.fp.points, solver: 'four-pole preview' };
  return null;
}

const level = (pa: number) => 20 * Math.log10(Math.max(pa, 1e-9) / 20e-6);

async function take(which: Which): Promise<Take | null> {
  const from = pointsOf(which);
  if (!from || !app.project) return null;
  const [lo, hi] = player.mode === 'steady' ? [player.rpm!, player.rpm!] : [player.from!, player.to!];
  const sound = await backend.listen(
    $state.snapshot(app.project) as Project,
    $state.snapshot(from.points) as PointOutcome[],
    lo,
    hi,
    player.mode === 'steady' ? 2 : player.seconds,
    player.interior,
  );
  let square = 0;
  for (const s of sound.samples) square += s * s;
  return {
    sound,
    leqDb: level(Math.sqrt(square / Math.max(sound.samples.length, 1))),
    peakDb: level(sound.peakPa),
    solver: from.solver,
  };
}

/** Plays `which`, building both takes first so they share one scale. */
export async function play(which: Which) {
  stop();
  player.error = null;
  player.loading = true;
  try {
    const current = await take('current');
    const baseline = app.baseline ? await take('baseline') : null;
    player.takes = { ...(current ? { current } : {}), ...(baseline ? { baseline } : {}) };
    const chosen = player.takes[which];
    if (!chosen) throw new Error(which === 'baseline' ? 'no baseline pinned' : 'nothing solved yet');
    const peak = Math.max(current?.sound.peakPa ?? 0, baseline?.sound.peakPa ?? 0);
    if (peak === 0) throw new Error('silent: every engine order was left out');
    context ??= new AudioContext();
    await context.resume();
    const { sound } = chosen;
    const buffer = context.createBuffer(1, sound.samples.length, sound.sampleRate);
    const channel = buffer.getChannelData(0);
    const scale = 0.9 / peak;
    for (let i = 0; i < channel.length; i++) channel[i] = sound.samples[i] * scale;
    gain = context.createGain();
    gain.gain.value = player.volume;
    gain.connect(context.destination);
    source = context.createBufferSource();
    source.buffer = buffer;
    source.loop = player.mode === 'steady';
    source.connect(gain);
    source.onended = () => {
      if (player.playing === which && !source?.loop) player.playing = null;
    };
    source.start();
    player.playing = which;
    player.startedAt = context.currentTime;
    player.duration = buffer.duration;
  } catch (e) {
    player.error = String(e);
  } finally {
    player.loading = false;
  }
}

export function stop() {
  source?.stop();
  source?.disconnect();
  source = null;
  player.playing = null;
}

export function setVolume(volume: number) {
  player.volume = volume;
  if (gain) gain.gain.value = volume;
}

/** Engine speed now sounding: the run-up's position, or the steady speed. */
export function soundingRpm(): number | null {
  if (!player.playing || !context) return null;
  if (player.mode === 'steady') return player.rpm;
  const x = Math.min(1, (context.currentTime - player.startedAt) / player.duration);
  return player.from! + (player.to! - player.from!) * x;
}
