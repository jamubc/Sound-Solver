<script lang="ts">
  import Icon from './Icon.svelte';
  import ProjectSettings from './ProjectSettings.svelte';
  import Properties from './Properties.svelte';
  import { app, manifestOf } from './state.svelte';

  const heading = $derived.by((): [string, string, string] => {
    const s = app.selection;
    if (!s) return ['gear', 'Project settings', 'nothing selected'];
    if (s.kind === 'element') {
      const e = app.project?.system.elements.find((x) => x.id === s.id);
      return ['element', s.id, manifestOf(e?.type ?? '')?.name ?? 'element'];
    }
    if (s.kind === 'route') return ['route', s.id, 'pipe'];
    return ['bend', `${s.route} · bend point ${s.index + 1}`, 'bend point'];
  });
</script>

<aside class="inspector" data-testid="inspector">
  <div class="head">
    <Icon name={heading[0]} />
    <span class="title">{heading[1]}</span>
    <span class="kind">{heading[2]}</span>
  </div>
  <div class="body">
    {#if app.project && !app.selection}
      <ProjectSettings />
    {:else}
      <Properties />
    {/if}
  </div>
</aside>

<style>
  .inspector {
    display: flex;
    flex-direction: column;
    min-height: 0;
    height: 100%;
  }

  .head {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 7px 10px;
    border-bottom: 1px solid var(--line);
    color: var(--accent);
  }

  .title {
    color: var(--text);
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .kind {
    margin-left: auto;
    color: var(--faint);
    font-size: 11px;
    white-space: nowrap;
  }

  .body {
    overflow-y: auto;
    min-height: 0;
    flex: 1;
  }
</style>
