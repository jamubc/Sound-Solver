// Plays the predicted pressure at a listener through Web Audio: the preview (the backend's
// `listen`: the solved spectrum lines resynthesised, instant) or a render (the backend's
// `render`: the time-domain solver marched through the scene, its own waveform). The current
// design and the pinned baseline are scaled alike, so an A/B comparison keeps their difference
// in level.
import { backend, type Rendered } from './backend';
import { app } from './state.svelte';
import type { Listener, PointOutcome, RenderInfo, RenderProgress, Scene } from './types/api';
import type { Project } from './types/project';

export type Which = 'current' | 'baseline';
export type ListenerKind = 'receiver' | 'stereo' | 'pass_by' | 'cabin';

export interface Take {
  /** Pressure of each channel, Pa. */
  channels: Float32Array[];
  sampleRate: number;
  peakPa: number;
  /** Equivalent continuous level of each channel, dB re 20 µPa. */
  leqDb: number[];
  peakDb: number;
  solver: string;
  /** Engine orders outside the cabin measurement left out (preview only). */
  dropped: number;
  /** How a render was made (render only). */
  info: RenderInfo | null;
}

/** ISO 362: microphone 7.5 m from the vehicle's path, 1.2 m above the ground. */
const PASS_BY_DISTANCE_MM = 7500;
const PASS_BY_HEIGHT_MM = 1200;

export const player = $state({
  source: 'preview' as 'preview' | 'render',
  mode: 'steady' as 'steady' | 'runup',
  rpm: null as number | null,
  from: null as number | null,
  to: null as number | null,
  seconds: 12,
  listener: 'receiver' as ListenerKind,
  /** Intake pressure against time instead of the project's load line (render only). */
  throttle: false,
  /** `t:kPa` pairs, s and kPa. */
  throttleTrace: '0:40, 1:100, 6:100, 6.5:30',
  passBySpeedKmh: 50,
  passBySide: 'right' as 'left' | 'right',
  earSpacingMm: 175,
  /** Head turned left from facing the tailpipe, degrees (90: the tailpipe at the right ear). */
  headTurnDeg: 90,
  volume: 0.7,
  playing: null as Which | null,
  loading: false,
  /** What a render in flight is doing. */
  progress: null as string | null,
  error: null as string | null,
  takes: {} as Partial<Record<Which, Take>>,
  /** Audio-clock time the playback started, s, and its length. */
  startedAt: 0,
  duration: 0,
});

let context: AudioContext | null = null;
let source: AudioBufferSourceNode | null = null;
let gain: GainNode | null = null;
/** Renders already made, by project and scene. */
const rendered = new Map<string, Rendered>();

/** The solved points a design is previewed from: the time domain once complete, else the preview. */
export function pointsOf(which: Which): { points: PointOutcome[]; solver: string } | null {
  const results =
    which === 'current'
      ? { td: app.timeDomain.result, fp: app.preview }
      : { td: app.baseline?.timeDomain ?? null, fp: app.baseline?.preview ?? null };
  if (results.td) return { points: results.td.points, solver: 'time domain' };
  if (results.fp) return { points: results.fp.points, solver: 'four-pole preview' };
  return null;
}

/** The project a design is rendered from. */
function projectOf(which: Which): Project | null {
  return which === 'current' ? ($state.snapshot(app.project) as Project | null) : (app.baseline?.project ?? null);
}

const level = (pa: number) => 20 * Math.log10(Math.max(pa, 1e-9) / 20e-6);

/** `t:kPa, t:kPa, …` as trace points; throws on anything else. */
export function parseTrace(text: string): [number, number][] {
  const points = text.split(',').map((pair) => {
    const [t, v] = pair.split(':').map((s) => Number(s.trim()));
    if (!Number.isFinite(t) || !Number.isFinite(v)) throw new Error(`throttle trace: '${pair.trim()}' is not t:kPa`);
    return [t, v] as [number, number];
  });
  if (points.some((p, i) => i > 0 && p[0] <= points[i - 1][0])) throw new Error('throttle trace: times must increase');
  return points;
}

/** Who hears a render, from the controls. */
function listenerOf(project: Project): Listener {
  switch (player.listener) {
    case 'receiver':
      return { kind: 'receiver' };
    case 'cabin':
      return { kind: 'cabin' };
    case 'stereo':
      return { kind: 'stereo', ear_spacing_mm: player.earSpacingMm, turn_deg: player.headTurnDeg };
    case 'pass_by': {
      const side = player.passBySide === 'left' ? 1 : -1;
      return {
        kind: 'pass_by',
        mic_mm: [0, side * PASS_BY_DISTANCE_MM, project.ambient.ground_z_mm + PASS_BY_HEIGHT_MM],
        // Level with the microphone half-way through the scene.
        start_mm: -((player.passBySpeedKmh / 3.6) * 1000 * player.seconds) / 2,
        speed_kmh: [[0, player.passBySpeedKmh]],
      };
    }
  }
}

/** The scene a render plays, from the controls. */
export function scene(project: Project): Scene {
  const seconds = player.seconds;
  return {
    duration_s: seconds,
    rpm: player.mode === 'steady' ? [[0, player.rpm!]] : [[0, player.from!], [seconds, player.to!]],
    ...(player.throttle ? { map_kpa: parseTrace(player.throttleTrace) } : {}),
    listener: listenerOf(project),
  };
}

function describe(p: RenderProgress, which: Which): string {
  const who = which === 'current' ? 'design' : 'baseline';
  return p.stage === 'march' ? `rendering ${who}: ${Math.round(100 * p.fraction)} %` : `settling ${who}: cycle ${p.cycle}`;
}

function takeOf(channels: Float32Array[], sampleRate: number, solver: string, dropped: number, info: RenderInfo | null): Take {
  let peak = 0;
  const leqDb = channels.map((c) => {
    let square = 0;
    for (const s of c) {
      square += s * s;
      peak = Math.max(peak, Math.abs(s));
    }
    return level(Math.sqrt(square / Math.max(c.length, 1)));
  });
  return { channels, sampleRate, peakPa: peak, leqDb, peakDb: level(peak), solver, dropped, info };
}

async function take(which: Which): Promise<Take | null> {
  if (player.source === 'render') {
    const project = projectOf(which);
    if (!project) return null;
    const s = scene(project);
    const key = JSON.stringify([project, s]);
    let r = rendered.get(key);
    if (!r) {
      r = await backend.render(project, s, (p) => (player.progress = describe(p, which)));
      rendered.set(key, r);
      if (rendered.size > 4) rendered.delete(rendered.keys().next().value!);
    }
    return takeOf(r.channels, r.info.sample_rate, 'time-domain render', 0, r.info);
  }
  const from = pointsOf(which);
  if (!from || !app.project) return null;
  const [lo, hi] = player.mode === 'steady' ? [player.rpm!, player.rpm!] : [player.from!, player.to!];
  const sound = await backend.listen(
    $state.snapshot(app.project) as Project,
    $state.snapshot(from.points) as PointOutcome[],
    lo,
    hi,
    player.mode === 'steady' ? 2 : player.seconds,
    player.listener === 'cabin',
  );
  return takeOf([sound.samples], sound.sampleRate, from.solver, sound.dropped, null);
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
    const peak = Math.max(current?.peakPa ?? 0, baseline?.peakPa ?? 0);
    if (peak === 0) throw new Error('silent: every engine order was left out');
    context ??= new AudioContext();
    await context.resume();
    const { channels, sampleRate } = chosen;
    const buffer = context.createBuffer(channels.length, channels[0].length, sampleRate);
    const scale = 0.9 / peak;
    channels.forEach((c, i) => {
      const out = buffer.getChannelData(i);
      for (let k = 0; k < out.length; k++) out[k] = c[k] * scale;
    });
    gain = context.createGain();
    gain.gain.value = player.volume;
    gain.connect(context.destination);
    source = context.createBufferSource();
    source.buffer = buffer;
    source.loop = player.mode === 'steady' && player.source === 'preview';
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
    player.progress = null;
  }
}

export function stop() {
  source?.stop();
  source?.disconnect();
  source = null;
  player.playing = null;
}

/** Stops a render in flight. */
export function cancelRender() {
  void backend.cancelRender();
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
