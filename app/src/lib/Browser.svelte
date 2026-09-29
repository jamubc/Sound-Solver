<script lang="ts">
  import { fileName, mm } from './format';
  import Icon from './Icon.svelte';
  import { app, manifestOf } from './state.svelte';
  import { enter, show } from './ui.svelte';

  const project = $derived(app.project);
  const routes = $derived(project?.system.routes ?? []);
  const elements = $derived(project?.system.elements ?? []);
  const recordings = $derived(project?.measurements?.recordings ?? []);
  const scan = $derived(project?.fabrication?.scan ?? null);
  let open = $state({ routes: true, elements: true, measurements: true });

  const length = (id: string) => app.layout?.routes.find((r) => r.id === id)?.length_mm;
  const pickedRoute = (id: string) => {
    const s = app.selection;
    return (s?.kind === 'route' && s.id === id) || (s?.kind === 'vertex' && s.route === id);
  };
</script>

{#snippet group(key: 'routes' | 'elements' | 'measurements', label: string, count: number)}
  <button class="group" onclick={() => (open[key] = !open[key])} aria-expanded={open[key]}>
    <span class="caret" class:open={open[key]}><Icon name="chevron" size={12} /></span>
    {label}
    <span class="count">{count}</span>
  </button>
{/snippet}

<nav class="browser" data-testid="browser">
  <div class="head">Model</div>
  <div class="tree">
    <button class="item root" class:picked={app.selection === null} onclick={() => (app.selection = null)} title="Project settings">
      <Icon name="project" />
      <span class="name">{project?.name ?? 'No project'}</span>
    </button>

    {@render group('routes', 'Pipes', routes.length)}
    {#if open.routes}
      <ul>
        {#each routes as r (r.id)}
          {@const l = length(r.id)}
          <li>
            <button class="item" class:picked={pickedRoute(r.id)} onclick={() => (app.selection = { kind: 'route', id: r.id })}>
              <Icon name="route" />
              <span class="name">{r.id}</span>
              <span class="meta mono">{r.pipe.od_mm.toFixed(1)}{l !== undefined ? ` · ${mm(l, 0)}` : ''}</span>
            </button>
          </li>
        {/each}
      </ul>
    {/if}

    {@render group('elements', 'Elements', elements.length)}
    {#if open.elements}
      <ul data-testid="elements">
        {#each elements as e (e.id)}
          <li>
            <button
              class="item"
              class:picked={app.selection?.kind === 'element' && app.selection.id === e.id}
              onclick={() => (app.selection = { kind: 'element', id: e.id })}
            >
              <Icon name="element" />
              <span class="name">{e.id}</span>
              <span class="meta">{manifestOf(e.type)?.name ?? e.type}</span>
            </button>
          </li>
        {/each}
      </ul>
    {/if}

    {@render group('measurements', 'Measurements', recordings.length + (project?.measurements?.cabin_tf ? 1 : 0) + (scan ? 1 : 0))}
    {#if open.measurements}
      <ul>
        {#each recordings as r, i (i)}
          <li>
            <button class="item" onclick={() => enter('measure')}>
              <Icon name="mic" />
              <span class="name">{fileName(r.path)}</span>
              <span class="meta">{r.position}</span>
            </button>
          </li>
        {/each}
        {#if project?.measurements?.cabin_tf}
          <li>
            <button class="item" onclick={() => (enter('measure'), show('measure'))}>
              <Icon name="cabin" />
              <span class="name">Cabin transfer</span>
              <span class="meta">{project.measurements.cabin_tf.method.replace('_', ' ')}</span>
            </button>
          </li>
        {/if}
        {#if scan}
          <li>
            <button class="item" onclick={() => enter('fabricate')}>
              <Icon name="scan" />
              <span class="name">{fileName(scan.path)}</span>
              <span class="meta">scan</span>
            </button>
          </li>
        {/if}
        {#if !recordings.length && !scan && !project?.measurements?.cabin_tf}
          <li class="empty muted">none yet</li>
        {/if}
      </ul>
    {/if}
  </div>
</nav>

<style>
  .browser {
    display: flex;
    flex-direction: column;
    min-height: 0;
    height: 100%;
  }

  .head {
    padding: 7px 10px;
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--muted);
    border-bottom: 1px solid var(--line);
  }

  .tree {
    overflow-y: auto;
    padding: 4px 0 10px;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  button {
    border: none;
    border-radius: 0;
    background: none;
    min-height: 22px;
  }

  .group {
    display: flex;
    align-items: center;
    gap: 4px;
    width: 100%;
    padding: 6px 8px 3px;
    color: var(--muted);
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .caret {
    display: flex;
    transition: transform 0.12s;
  }

  .caret.open {
    transform: rotate(90deg);
  }

  .count {
    margin-left: auto;
    color: var(--faint);
    font-weight: 400;
  }

  .item {
    display: flex;
    align-items: center;
    gap: 7px;
    width: 100%;
    padding: 2px 10px 2px 22px;
    text-align: left;
    color: var(--text);
    border-left: 2px solid transparent;
  }

  .item.root {
    padding-left: 10px;
    font-weight: 600;
  }

  .item:hover {
    background: var(--panel-2);
  }

  .item.picked {
    background: var(--accent-soft);
    border-left-color: var(--accent);
  }

  .item :global(svg) {
    color: var(--muted);
  }

  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .meta {
    margin-left: auto;
    color: var(--faint);
    font-size: 11px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 50%;
  }

  .empty {
    padding: 2px 22px;
  }
</style>
