<script lang="ts">
  import { onMount } from 'svelte';
  import { backend } from './lib/backend';
  import Browser from './lib/Browser.svelte';
  import Dock from './lib/Dock.svelte';
  import Inspector from './lib/Inspector.svelte';
  import Ribbon from './lib/Ribbon.svelte';
  import Splitter from './lib/Splitter.svelte';
  import { app, load } from './lib/state.svelte';
  import StatusBar from './lib/StatusBar.svelte';
  import { persist, ui } from './lib/ui.svelte';
  import Viewport from './lib/Viewport.svelte';

  let unavailable = $state<string | null>(null);

  onMount(async () => {
    try {
      app.manifests = await backend.manifests();
      load(await backend.stockProject(), null);
    } catch (e) {
      unavailable = String(e);
    }
  });

  const resized = (apply: () => void, done: boolean) => {
    apply();
    if (done) persist();
  };
</script>

<div class="shell" style:--browser="{ui.browserWidth}px" style:--inspector="{ui.inspectorWidth}px">
  <Ribbon />
  <div class="banners">
    {#if unavailable}
      <div class="banner bad">Solver backend unavailable: {unavailable}</div>
    {/if}
    {#if app.invalid}
      <div class="banner warn" data-testid="invalid">Not solvable as edited: {app.invalid}</div>
    {/if}
  </div>
  <div class="work">
    <aside class="browser">
      <Browser />
    </aside>
    <Splitter
      axis="x"
      size={ui.browserWidth}
      min={180}
      max={420}
      onresize={(s, done) => resized(() => (ui.browserWidth = s), done)}
    />
    <main class="centre">
      <div class="view">
        <Viewport />
      </div>
      {#if ui.dockOpen}
        <Splitter
          axis="y"
          size={ui.dockHeight}
          min={160}
          max={720}
          invert
          onresize={(s, done) => resized(() => (ui.dockHeight = s), done)}
        />
      {/if}
      <div class="dock" style:height={ui.dockOpen ? `${ui.dockHeight}px` : 'auto'}>
        <Dock />
      </div>
    </main>
    <Splitter
      axis="x"
      size={ui.inspectorWidth}
      min={280}
      max={560}
      invert
      onresize={(s, done) => resized(() => (ui.inspectorWidth = s), done)}
    />
    <aside class="inspector">
      <Inspector />
    </aside>
  </div>
  <StatusBar />
</div>

<style>
  .shell {
    display: grid;
    grid-template-rows: auto auto minmax(0, 1fr) auto;
    grid-template-areas:
      'ribbon'
      'banner'
      'work'
      'status';
    height: 100%;
  }

  .banners {
    grid-area: banner;
  }

  .banner {
    padding: 5px 12px;
    border-bottom: 1px solid var(--line);
  }

  .banner.bad {
    background: #3a1d22;
  }

  .banner.warn {
    background: #3a321d;
  }

  .work {
    grid-area: work;
    display: grid;
    grid-template-columns: var(--browser) auto minmax(0, 1fr) auto var(--inspector);
    min-height: 0;
  }

  .browser,
  .inspector {
    min-width: 0;
    min-height: 0;
    background: var(--panel);
  }

  .centre {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  .view {
    position: relative;
    flex: 1;
    min-height: 120px;
  }

  .dock {
    flex: none;
    min-height: 0;
  }
</style>
