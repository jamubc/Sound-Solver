<script lang="ts">
  import { onMount } from 'svelte';
  import { backend } from './lib/backend';
  import Fabrication from './lib/Fabrication.svelte';
  import Measurements from './lib/Measurements.svelte';
  import Plots from './lib/Plots.svelte';
  import Properties from './lib/Properties.svelte';
  import { app, load } from './lib/state.svelte';
  import StatusBar from './lib/StatusBar.svelte';
  import Toolbar from './lib/Toolbar.svelte';
  import Tree from './lib/Tree.svelte';
  import Viewport from './lib/Viewport.svelte';

  let unavailable = $state<string | null>(null);
  const TABS = { acoustics: 'Acoustics', measurements: 'Measurements', fabrication: 'Fabrication' } as const;
  let tab = $state<keyof typeof TABS>('acoustics');

  onMount(async () => {
    try {
      app.manifests = await backend.manifests();
      load(await backend.stockProject(), null);
    } catch (e) {
      unavailable = String(e);
    }
  });
</script>

<div class="shell">
  <Toolbar />
  <div class="banners">
    {#if unavailable}
      <div class="banner bad">Solver backend unavailable: {unavailable}</div>
    {/if}
    {#if app.invalid}
      <div class="banner warn" data-testid="invalid">Not solvable as edited: {app.invalid}</div>
    {/if}
  </div>
  <aside class="side">
    <Tree />
    <Properties />
  </aside>
  <main class="view">
    <Viewport />
  </main>
  <section class="plots">
    <div class="tabs" role="tablist">
      {#each Object.entries(TABS) as [t, name]}
        <button role="tab" aria-selected={tab === t} class:on={tab === t} onclick={() => (tab = t as keyof typeof TABS)}>
          {name}
        </button>
      {/each}
    </div>
    {#if tab === 'acoustics'}
      <Plots />
    {:else if tab === 'measurements'}
      <Measurements />
    {:else}
      <Fabrication />
    {/if}
  </section>
  <StatusBar />
</div>

<style>
  .shell {
    display: grid;
    grid-template-columns: 340px minmax(0, 1fr) 560px;
    grid-template-rows: auto auto minmax(0, 1fr) auto;
    grid-template-areas:
      'top top top'
      'banner banner banner'
      'side view plots'
      'status status status';
    height: 100%;
  }

  .banners {
    grid-area: banner;
  }

  .banner {
    padding: 6px 12px;
    border-bottom: 1px solid var(--line);
  }

  .banner.bad {
    background: #3a1d22;
  }

  .banner.warn {
    background: #3a321d;
  }

  .side {
    grid-area: side;
    display: flex;
    flex-direction: column;
    min-height: 0;
    border-right: 1px solid var(--line);
    background: var(--panel);
  }

  .view {
    grid-area: view;
    min-width: 0;
    min-height: 0;
    position: relative;
  }

  .plots {
    grid-area: plots;
    min-height: 0;
    overflow-y: auto;
    border-left: 1px solid var(--line);
    background: var(--panel);
  }

  .tabs {
    position: sticky;
    top: 0;
    z-index: 1;
    display: flex;
    gap: 4px;
    padding: 8px 12px 0;
    border-bottom: 1px solid var(--line);
    background: var(--panel);
  }

  .tabs button {
    border-bottom: none;
    border-radius: 4px 4px 0 0;
    background: none;
  }

  .tabs button.on {
    background: var(--panel-2);
    border-color: var(--accent);
  }
</style>
