<script lang="ts">
  import { onMount } from 'svelte';
  import Icon from './Icon.svelte';
  import { play, player, pointsOf, setVolume, soundingRpm, stop, type Which } from './player.svelte';
  import { app } from './state.svelte';

  const sweep = $derived(app.project?.operating.sweep_rpm ?? [1000, 6500, 50]);
  const cabin = $derived(app.project?.measurements?.cabin_tf ?? null);
  const current = $derived(pointsOf('current'));
  const baseline = $derived(app.baseline ? pointsOf('baseline') : null);
  const maxHz = $derived(app.project?.solver.max_frequency_hz ?? 2000);

  // Defaults: the drone's speed, and the whole sweep for a run-up.
  $effect(() => {
    const metrics = app.timeDomain.result?.metrics ?? app.preview?.metrics;
    const drone = metrics?.drone_interior ?? metrics?.drone_exterior;
    if (player.rpm === null && (drone || app.rpm)) {
      const [start, , step] = sweep;
      const target = app.rpm ?? drone!.peak_rpm;
      player.rpm = start + Math.round((target - start) / step) * step;
    }
    player.from ??= sweep[0];
    player.to ??= sweep[1];
  });
  $effect(() => {
    if (!cabin) player.interior = false;
  });

  // The engine speed sounding now, while a run-up plays.
  let now = $state<number | null>(null);
  onMount(() => {
    let frame = requestAnimationFrame(function tick() {
      now = soundingRpm();
      frame = requestAnimationFrame(tick);
    });
    return () => {
      cancelAnimationFrame(frame);
      stop();
    };
  });

  const num = (e: Event) => Number((e.currentTarget as HTMLInputElement).value);
  const ready = $derived(
    !!current && (player.mode === 'steady' ? player.rpm !== null : player.from !== null && player.to !== null),
  );

  // Waveform of the take playing (or last played): min/max per pixel column.
  let canvas = $state<HTMLCanvasElement>();
  $effect(() => {
    const t = player.takes[player.playing ?? 'current'];
    if (!canvas || !t) return;
    const { samples } = t.sound;
    const peak = Math.max(player.takes.current?.sound.peakPa ?? 0, player.takes.baseline?.sound.peakPa ?? 0, 1e-9);
    const g = canvas.getContext('2d')!;
    const [w, h] = [(canvas.width = canvas.clientWidth * 2), (canvas.height = canvas.clientHeight * 2)];
    g.clearRect(0, 0, w, h);
    g.fillStyle = player.playing === 'baseline' ? '#8a94a6' : '#ff9f43';
    const per = Math.max(1, Math.floor(samples.length / w));
    for (let x = 0; x < w; x++) {
      let [lo, hi] = [0, 0];
      for (let i = x * per; i < Math.min(samples.length, (x + 1) * per); i++) {
        lo = Math.min(lo, samples[i]);
        hi = Math.max(hi, samples[i]);
      }
      const y = (v: number) => h / 2 - (v / peak) * (h / 2 - 2);
      g.fillRect(x, y(hi), 1, Math.max(1, y(lo) - y(hi)));
    }
  });

  const button = (which: Which) => (player.playing === which ? stop() : play(which));
</script>

<div class="listen" data-testid="listen">
  <div class="controls">
    <fieldset>
      <legend>Hear</legend>
      <label>
        <input type="radio" bind:group={player.interior} value={false} /> outside, at the receiver
        {app.project?.receiver.distance_m} m, {app.project?.receiver.angle_deg}°
      </label>
      <label class:off={!cabin} title={cabin ? cabin.source : 'Measure a cabin transfer function first'}>
        <input type="radio" bind:group={player.interior} value={true} disabled={!cabin} /> inside, through the measured
        cabin transfer function
      </label>
    </fieldset>
    <fieldset>
      <legend>Engine</legend>
      <label>
        <input type="radio" bind:group={player.mode} value="steady" /> steady at
        <input
          class="rpm"
          type="number"
          step={sweep[2]}
          min={sweep[0]}
          max={sweep[1]}
          value={player.rpm}
          onchange={(e) => (player.rpm = num(e))}
        /> rpm
      </label>
      <label>
        <input type="radio" bind:group={player.mode} value="runup" /> run-up
        <input class="rpm" type="number" step={sweep[2]} value={player.from} onchange={(e) => (player.from = num(e))} />
        →
        <input class="rpm" type="number" step={sweep[2]} value={player.to} onchange={(e) => (player.to = num(e))} />
        rpm over
        <input class="short" type="number" min="1" max="60" value={player.seconds} onchange={(e) => (player.seconds = num(e))} />
        s
      </label>
    </fieldset>
    <div class="transport">
      <button class="primary" onclick={() => button('current')} disabled={!ready || player.loading} data-testid="play">
        <Icon name={player.playing === 'current' ? 'stop' : 'play'} />
        {player.playing === 'current' ? 'Stop' : 'Play design'}
      </button>
      <button onclick={() => button('baseline')} disabled={!baseline || !ready || player.loading} title={baseline ? app.baseline?.name : 'Pin a baseline to compare with'}>
        <Icon name={player.playing === 'baseline' ? 'stop' : 'play'} />
        {player.playing === 'baseline' ? 'Stop' : 'Play baseline'}
      </button>
      <label class="volume" title="Playback volume (the levels below are the prediction's)">
        <Icon name="speaker" />
        <input type="range" min="0" max="1" step="0.01" value={player.volume} oninput={(e) => setVolume(num(e))} />
      </label>
    </div>
  </div>

  <div class="readout">
    {#if !current}
      <p class="muted">Nothing solved yet: the four-pole preview appears as soon as the project is valid.</p>
    {:else}
      <div class="now">
        <span class="big mono">{now === null ? '—' : `${Math.round(now)} rpm`}</span>
        <span class="muted">{player.loading ? 'synthesising…' : player.playing ? `playing ${player.playing}` : 'stopped'}</span>
      </div>
      <canvas bind:this={canvas}></canvas>
      <dl>
        {#each ['current', 'baseline'] as const as which}
          {@const t = player.takes[which]}
          {#if t}
            <dt>{which === 'current' ? 'Design' : `Baseline (${app.baseline?.name})`}</dt>
            <dd class="mono" data-testid="level-{which}">
              L<sub>eq</sub> {t.leqDb.toFixed(1)} dB · peak {t.peakDb.toFixed(1)} dB re 20 µPa · {t.solver}{t.sound.dropped
                ? ` · ${t.sound.dropped} orders outside the cabin measurement left out`
                : ''}
            </dd>
          {/if}
        {/each}
      </dl>
      {#if player.error}<p class="bad">{player.error}</p>{/if}
      <p class="note muted">
        The predicted pressure at the listening point, rebuilt from the solved engine-order lines (up to {maxHz} Hz) with
        their solved phases; no flow noise is added. Both takes are scaled alike, so their difference in loudness is kept;
        the volume control sets playback only.
      </p>
    {/if}
  </div>
</div>

<style>
  .listen {
    display: grid;
    grid-template-columns: minmax(360px, 1fr) minmax(320px, 1.2fr);
    gap: 16px;
    padding: 10px 14px;
    height: 100%;
    overflow: auto;
  }

  fieldset {
    border: 1px solid var(--line);
    border-radius: var(--radius);
    margin: 0 0 8px;
    padding: 6px 10px 8px;
  }

  legend {
    color: var(--muted);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    padding: 0 4px;
  }

  label {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 4px 0;
    white-space: nowrap;
  }

  label.off {
    opacity: 0.5;
  }

  .rpm {
    width: 62px;
  }

  .short {
    width: 42px;
  }

  .transport {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .transport button {
    display: flex;
    align-items: center;
    gap: 6px;
    white-space: nowrap;
  }

  .volume {
    margin-left: auto;
    color: var(--muted);
  }

  .now {
    display: flex;
    align-items: baseline;
    gap: 10px;
  }

  .big {
    font-size: 22px;
    font-weight: 600;
    color: var(--td);
  }

  canvas {
    width: 100%;
    height: 70px;
    margin: 6px 0;
    background: var(--bg);
    border: 1px solid var(--line);
    border-radius: var(--radius);
  }

  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 2px 10px;
    margin: 0;
  }

  dt {
    color: var(--muted);
  }

  dd {
    margin: 0;
  }

  .note {
    font-size: 11px;
  }

  .bad {
    color: var(--bad);
  }
</style>
