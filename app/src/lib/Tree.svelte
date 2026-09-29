<script lang="ts">
  import { mm, od } from './format';
  import { app, manifestOf } from './state.svelte';

  const length = (id: string) => app.layout?.routes.find((r) => r.id === id)?.length_mm;
  const picked = (kind: 'route' | 'element', id: string) => {
    const s = app.selection;
    if (!s) return false;
    if (kind === 'element') return s.kind === 'element' && s.id === id;
    return (s.kind === 'route' && s.id === id) || (s.kind === 'vertex' && s.route === id);
  };
</script>

<nav>
  <h2>Routes</h2>
  <ul>
    {#each app.project?.system.routes ?? [] as r (r.id)}
      {@const l = length(r.id)}
      <li>
        <button
          class:picked={picked('route', r.id)}
          onclick={() => (app.selection = { kind: 'route', id: r.id })}
        >
          <span>{r.id}</span>
          <span class="muted">{od(r.pipe.od_mm)}{l !== undefined ? ` · ${mm(l, 0)}` : ''}</span>
        </button>
      </li>
    {/each}
  </ul>
  <h2>Elements</h2>
  <ul data-testid="elements">
    {#each app.project?.system.elements ?? [] as e (e.id)}
      <li>
        <button
          class:picked={picked('element', e.id)}
          onclick={() => (app.selection = { kind: 'element', id: e.id })}
        >
          <span>{e.id}</span>
          <span class="muted">{manifestOf(e.type)?.name ?? e.type}</span>
        </button>
      </li>
    {/each}
  </ul>
</nav>

<style>
  nav {
    padding: 10px;
    overflow-y: auto;
    max-height: 45%;
    border-bottom: 1px solid var(--line);
  }

  ul {
    list-style: none;
    margin: 0 0 10px;
    padding: 0;
  }

  button {
    display: flex;
    flex-direction: column;
    width: 100%;
    text-align: left;
    background: none;
    border-color: transparent;
    padding: 3px 6px;
  }

  button .muted {
    font-size: 11px;
  }

  button.picked {
    background: var(--panel-2);
    border-color: var(--accent);
  }

  span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
