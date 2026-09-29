<script lang="ts">
  import Compare from './Compare.svelte';
  import DroneReport from './DroneReport.svelte';
  import Fabrication from './Fabrication.svelte';
  import Icon from './Icon.svelte';
  import Listen from './Listen.svelte';
  import Measurements from './Measurements.svelte';
  import { player } from './player.svelte';
  import Plots from './Plots.svelte';
  import { persist, RESULTS, show, ui, WORKSPACE_TAB } from './ui.svelte';

  const tabs = $derived([...(WORKSPACE_TAB[ui.workspace] ? [WORKSPACE_TAB[ui.workspace]!] : []), ...RESULTS]);
  let body = $state(0);
  /** Plot height: the dock less the plot's caption, chips and legend. */
  const plot = $derived(Math.max(120, body - 96));
</script>

<section class="dock" data-testid="dock">
  <div class="bar" role="tablist">
    {#each tabs as [id, label] (id)}
      <button role="tab" aria-selected={ui.dock === id} class:on={ui.dock === id} onclick={() => show(id)}>
        {#if id === 'listen'}<Icon name="speaker" size={13} />{/if}
        {label}
        {#if id === 'listen' && player.playing}<span class="live"></span>{/if}
      </button>
    {/each}
    <span class="spacer"></span>
    <button
      class="toggle"
      title={ui.dockOpen ? 'Hide the results' : 'Show the results'}
      onclick={() => {
        ui.dockOpen = !ui.dockOpen;
        persist();
      }}
    >
      <span class="caret" class:down={ui.dockOpen}><Icon name="chevron" size={13} /></span>
    </button>
  </div>
  {#if ui.dockOpen}
    <div class="body" bind:clientHeight={body}>
      {#if ui.dock === 'overview'}
        <div class="overview">
          <div class="side">
            <DroneReport />
            <Compare />
          </div>
          <div class="main"><Plots show="firing" height={plot} /></div>
        </div>
      {:else if ui.dock === 'listen'}
        <Listen />
      {:else if ui.dock === 'measure'}
        <Measurements />
      {:else if ui.dock === 'fabricate'}
        <Fabrication />
      {:else}
        <div class="single">
          <Plots show={ui.dock as 'spectrum' | 'orders' | 'level' | 'backpressure'} height={plot} />
        </div>
      {/if}
    </div>
  {/if}
</section>

<style>
  .dock {
    display: flex;
    flex-direction: column;
    min-height: 0;
    height: 100%;
    background: var(--panel);
  }

  .bar {
    display: flex;
    align-items: stretch;
    gap: 1px;
    padding: 0 6px;
    border-bottom: 1px solid var(--line);
    background: var(--bg);
    flex: none;
  }

  .bar button {
    display: flex;
    align-items: center;
    gap: 5px;
    border: none;
    border-bottom: 2px solid transparent;
    border-radius: 0;
    background: none;
    color: var(--muted);
    padding: 5px 10px;
    min-height: 28px;
  }

  .bar button.on {
    color: var(--text);
    border-bottom-color: var(--accent);
    background: var(--panel);
  }

  .live {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--good);
    box-shadow: 0 0 6px var(--good);
  }

  .spacer {
    flex: 1;
  }

  .caret {
    display: flex;
    transform: rotate(-90deg);
    transition: transform 0.12s;
  }

  .caret.down {
    transform: rotate(90deg);
  }

  .body {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }

  .overview {
    display: grid;
    grid-template-columns: minmax(340px, 0.9fr) minmax(0, 1.6fr);
    height: 100%;
  }

  .side {
    overflow-y: auto;
    padding: 10px 14px;
    border-right: 1px solid var(--line);
  }

  .main,
  .single {
    padding: 8px 14px 0;
    min-width: 0;
  }
</style>
