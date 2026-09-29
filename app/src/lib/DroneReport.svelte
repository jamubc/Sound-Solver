<script lang="ts">
  import { db, rpm } from './format';
  import { app } from './state.svelte';
  import type { DroneReport } from './types/api';

  /** The reference once it is complete, the preview until then. */
  const source = $derived(
    app.timeDomain.result
      ? { metrics: app.timeDomain.result.metrics, label: 'time domain', tone: 'td' }
      : app.preview
        ? { metrics: app.preview.metrics, label: 'four-pole preview', tone: 'fp' }
        : null,
  );
  const receiver = $derived(app.project?.receiver);
  const band = $derived(app.project?.operating.cruise_band_rpm);
  const range = (d: DroneReport) =>
    d.minus_3db_rpm.map((x, i) => (x === null ? (i ? 'beyond sweep' : 'below sweep') : rpm(x))).join(' – ');
</script>

{#snippet report(d: DroneReport)}
  <dl>
    <dt>Peak</dt>
    <dd class="mono" data-testid="drone-peak">{rpm(d.peak_rpm)} · {db(d.peak_db)}</dd>
    <dt>−3 dB</dt>
    <dd class="mono">{range(d)}</dd>
    <dt>Cruise band</dt>
    <dd class="mono">
      {#if d.cruise_peak}{db(d.cruise_peak[1])} at {rpm(d.cruise_peak[0])}{:else}no solved speed in band{/if}
    </dd>
    {#if d.unconverged_rpm.length}
      <dt>Unconverged</dt>
      <dd class="warn">{d.unconverged_rpm.length} speeds</dd>
    {/if}
  </dl>
{/snippet}

<section data-testid="drone">
  <h2>
    Drone
    {#if source}<span class="chip {source.tone}">{source.label}</span>{/if}
  </h2>
  {#if source?.metrics.drone_exterior}
    <div class="grid">
      <div>
        <h3>Exterior, receiver {receiver?.distance_m} m at {receiver?.angle_deg}°</h3>
        {@render report(source.metrics.drone_exterior)}
      </div>
      <div>
        <h3>Interior (driver's ear)</h3>
        {#if source.metrics.drone_interior}
          {@render report(source.metrics.drone_interior)}
        {:else}
          <p class="unavailable" data-testid="interior-unavailable">
            Unavailable: no measured cabin transfer function. The cabin is never synthesised.
          </p>
        {/if}
      </div>
    </div>
    <dl class="extra">
      <dt title="Firing-order level over every other line, mean over the cruise band ({band?.[0]}–{band?.[1]} rpm)">
        Boominess
      </dt>
      <dd class="mono">{source.metrics.boominess_db == null ? 'no speed in band' : db(source.metrics.boominess_db)}</dd>
      <dt title="Energy above 800 Hz over energy below 300 Hz, 3000–5000 rpm: a proxy, not a perceptual metric">
        Rasp proxy
      </dt>
      <dd class="mono">{source.metrics.rasp_proxy_db == null ? 'no speed in range' : db(source.metrics.rasp_proxy_db)}</dd>
    </dl>
  {:else}
    <p class="muted">No solution yet.</p>
  {/if}
</section>

<style>
  section {
    margin-bottom: 14px;
    padding-bottom: 10px;
    border-bottom: 1px solid var(--line);
  }

  h3 {
    font-size: 11px;
    font-weight: 500;
    color: var(--muted);
    margin: 0 0 4px;
  }

  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
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

  .extra {
    margin-top: 8px;
  }

  .warn {
    color: var(--warn);
  }

  .unavailable {
    color: var(--muted);
    opacity: 0.7;
    margin: 0;
  }

  .chip {
    font-size: 10.5px;
    font-weight: 400;
    text-transform: none;
    letter-spacing: 0;
    padding: 0 6px;
    border-radius: 8px;
    border: 1px solid var(--line);
    margin-left: 6px;
  }

  .chip.td {
    border-color: var(--td);
    color: var(--td);
  }

  .chip.fp {
    border-color: var(--fp);
    color: var(--fp);
  }
</style>
