<script lang="ts">
  import { onMount } from 'svelte';
  import Icon from './Icon.svelte';
  import { cancelRender, play, player, pointsOf, setVolume, soundingRpm, stop, type Which } from './player.svelte';
  import { app } from './state.svelte';

  const sweep = $derived(app.project?.operating.sweep_rpm ?? [1000, 6500, 50]);
  const cabin = $derived(app.project?.measurements?.cabin_tf ?? null);
  const current = $derived(pointsOf('current'));
  const baseline = $derived(app.baseline ? pointsOf('baseline') : null);
  const maxHz = $derived(app.project?.solver.max_frequency_hz ?? 2000);
  const rendering = $derived(player.source === 'render');

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
  // Listeners the chosen source cannot give fall back to the receiver.
  $effect(() => {
    if (player.listener === 'cabin' && !cabin) player.listener = 'receiver';
    if (!rendering && (player.listener === 'stereo' || player.listener === 'pass_by')) player.listener = 'receiver';
    if (!rendering) player.throttle = false;
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
    (rendering ? !!app.project && !app.invalid : !!current) &&
      (player.mode === 'steady' ? player.rpm !== null : player.from !== null && player.to !== null),
  );

  // Waveform of the take playing (or last played): min/max per pixel column of its first channel.
  let canvas = $state<HTMLCanvasElement>();
  $effect(() => {
    const t = player.takes[player.playing ?? 'current'];
    if (!canvas || !t) return;
    const samples = t.channels[0];
    const peak = Math.max(player.takes.current?.peakPa ?? 0, player.takes.baseline?.peakPa ?? 0, 1e-9);
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

  // How the current render was made: its bands, and why the unresolved ones are.
  const info = $derived(player.takes.current?.info ?? null);
  const reasons = $derived(
    info ? [...new Set(info.bands.flatMap((b) => b.reasons.map((r) => r.replace(/[\d.]+ dB/, '… dB'))))] : [],
  );
  const hz = (f: number) => (f >= 1000 ? `${+(f / 1000).toFixed(f >= 10000 ? 0 : 1)} k` : `${Math.round(f)}`);

  const button = (which: Which) => (player.playing === which ? stop() : play(which));
</script>

<div class="listen" data-testid="listen">
  <div class="controls">
    <fieldset>
      <legend>Sound</legend>
      <label>
        <input type="radio" bind:group={player.source} value="preview" /> preview: the solved engine-order lines, instant
      </label>
      <label title="Marches the time-domain solver through the scene and radiates its outlet flow: several times slower than real time">
        <input type="radio" bind:group={player.source} value="render" data-testid="source-render" /> render: the solver's own
        waveform
      </label>
    </fieldset>
    <fieldset>
      <legend>Hear</legend>
      <label>
        <input type="radio" bind:group={player.listener} value="receiver" /> outside, at the receiver
        {app.project?.receiver.distance_m} m, {app.project?.receiver.angle_deg}°
      </label>
      <label class:off={!rendering} title={rendering ? 'Two ears at the receiver, facing the tailpipe' : 'Render only'}>
        <input type="radio" bind:group={player.listener} value="stereo" disabled={!rendering} /> stereo head at the receiver,
        ears
        <input class="short" type="number" min="100" max="250" value={player.earSpacingMm} onchange={(e) => (player.earSpacingMm = num(e))} />
        mm apart
      </label>
      <label class:off={!rendering} title={rendering ? 'ISO 362 microphone: 7.5 m from the path, 1.2 m up' : 'Render only'}>
        <input type="radio" bind:group={player.listener} value="pass_by" disabled={!rendering} /> pass-by at
        <input class="short" type="number" min="5" max="200" value={player.passBySpeedKmh} onchange={(e) => (player.passBySpeedKmh = num(e))} />
        km/h, microphone on the
        <select bind:value={player.passBySide}>
          <option value="right">right</option>
          <option value="left">left</option>
        </select>
      </label>
      <label class:off={!cabin} title={cabin ? cabin.source : 'Measure a cabin transfer function first'}>
        <input type="radio" bind:group={player.listener} value="cabin" disabled={!cabin} /> inside, through the measured cabin
        transfer function
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
        rpm
      </label>
      {#if rendering || player.mode === 'runup'}
        <label>
          over
          <input class="short" type="number" min="0.5" max="60" step="0.5" value={player.seconds} onchange={(e) => (player.seconds = num(e))} />
          s
        </label>
      {/if}
      <label class:off={!rendering} title={rendering ? 'Intake manifold pressure against time, as t:kPa pairs' : 'Render only'}>
        <input type="checkbox" bind:checked={player.throttle} disabled={!rendering} /> throttle trace
        <input class="trace mono" type="text" bind:value={player.throttleTrace} disabled={!player.throttle} /> s:kPa
      </label>
    </fieldset>
    <div class="transport">
      <button class="primary" onclick={() => button('current')} disabled={!ready || player.loading} data-testid="play">
        <Icon name={player.playing === 'current' ? 'stop' : 'play'} />
        {player.playing === 'current' ? 'Stop' : 'Play design'}
      </button>
      <button onclick={() => button('baseline')} disabled={!app.baseline || (!rendering && !baseline) || !ready || player.loading} title={app.baseline ? app.baseline.name : 'Pin a baseline to compare with'}>
        <Icon name={player.playing === 'baseline' ? 'stop' : 'play'} />
        {player.playing === 'baseline' ? 'Stop' : 'Play baseline'}
      </button>
      {#if player.loading && rendering}
        <button onclick={cancelRender} data-testid="cancel-render">Cancel render</button>
      {/if}
      <label class="volume" title="Playback volume (the levels below are the prediction's)">
        <Icon name="speaker" />
        <input type="range" min="0" max="1" step="0.01" value={player.volume} oninput={(e) => setVolume(num(e))} />
      </label>
    </div>
  </div>

  <div class="readout">
    {#if !current && !rendering}
      <p class="muted">Nothing solved yet: the four-pole preview appears as soon as the project is valid.</p>
    {:else}
      <div class="now">
        <span class="big mono">{now === null ? '—' : `${Math.round(now)} rpm`}</span>
        <span class="muted">
          {player.loading ? (player.progress ?? (rendering ? 'rendering…' : 'synthesising…')) : player.playing ? `playing ${player.playing}` : 'stopped'}
        </span>
      </div>
      <canvas bind:this={canvas}></canvas>
      <dl>
        {#each ['current', 'baseline'] as const as which}
          {@const t = player.takes[which]}
          {#if t}
            <dt>{which === 'current' ? 'Design' : `Baseline (${app.baseline?.name})`}</dt>
            <dd class="mono" data-testid="level-{which}">
              L<sub>eq</sub> {t.leqDb.map((l) => l.toFixed(1)).join(' / ')} dB · peak {t.peakDb.toFixed(1)} dB re 20 µPa · {t.solver}{t.dropped
                ? ` · ${t.dropped} orders outside the cabin measurement left out`
                : ''}
            </dd>
          {/if}
        {/each}
      </dl>
      {#if player.error}<p class="bad">{player.error}</p>{/if}
      {#if info}
        <div class="bands" data-testid="bands" aria-label="Third-octave bands the model resolves">
          {#each info.bands as b}
            <span class:resolved={b.resolved} title="{hz(b.center_hz)}Hz: {b.resolved ? 'resolved' : b.reasons.join('; ')}"></span>
          {/each}
        </div>
        <div class="axis mono muted"><span>{hz(info.bands[0].center_hz)}Hz</span><span>{hz(info.bands[info.bands.length - 1].center_hz)}Hz</span></div>
        <p class="note">
          <span class="key resolved"></span> resolved <span class="key"></span> unresolved: the render keeps these bands, the model
          cannot vouch for them.
        </p>
        <ul class="note muted">
          {#each reasons as r}<li>{r}</li>{/each}
          {#each info.warnings as w}<li>{w}</li>{/each}
        </ul>
        <p class="note muted mono">
          start {Math.round(info.start.rpm)} rpm, {info.start.map_kpa.toFixed(0)} kPa: {info.start.cycles} cycles, {info.start.converged
            ? 'converged'
            : 'NOT converged'} · {info.cells} cells, Δx {info.dx_mm} mm, CFL ≤ {info.cfl_max.toFixed(2)} · grid check at {Math.round(
            info.refinement.rpm,
          )} rpm, Δx/2 · {info.source}
        </p>
      {:else}
        <p class="note muted">
          {#if rendering}
            A render marches the time-domain solver through the scene and radiates each outlet's flow to the listener: the
            solver's own waveform, every band labelled resolved or unresolved.
          {:else}
            The predicted pressure at the listening point, rebuilt from the solved engine-order lines (up to {maxHz} Hz) with
            their solved phases; no flow noise is added.
          {/if}
          Both takes are scaled alike, so their difference in loudness is kept; the volume control sets playback only.
        </p>
      {/if}
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

  .trace {
    width: 170px;
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

  .bands {
    display: grid;
    grid-auto-flow: column;
    grid-auto-columns: 1fr;
    gap: 1px;
    height: 12px;
    margin-top: 10px;
  }

  .bands span,
  .key {
    background: var(--line-2);
    border-radius: 1px;
  }

  .bands span.resolved,
  .key.resolved {
    background: var(--good);
  }

  .key {
    display: inline-block;
    width: 10px;
    height: 10px;
    vertical-align: -1px;
  }

  .axis {
    display: flex;
    justify-content: space-between;
    font-size: 10px;
  }

  .note {
    font-size: 11px;
  }

  ul.note {
    margin: 4px 0;
    padding-left: 16px;
  }

  .bad {
    color: var(--bad);
  }
</style>
