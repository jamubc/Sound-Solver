<script lang="ts">
  import { open, save } from '@tauri-apps/plugin-dialog';
  import { backend } from './backend';
  import { app, cancelTimeDomain, load, solveTimeDomain } from './state.svelte';
  import type { Project } from './types/project';

  const filters = [{ name: 'Exhaust project', extensions: ['json'] }];
  let failure = $state<string | null>(null);

  async function attempt(action: () => Promise<void>) {
    failure = null;
    try {
      await action();
    } catch (e) {
      failure = String(e);
    }
  }

  const openFile = () =>
    attempt(async () => {
      const path = await open({ filters, multiple: false, directory: false });
      if (typeof path === 'string') load(await backend.openProject(path), path);
    });

  const saveAs = (path: string | null) =>
    attempt(async () => {
      if (!app.project) return;
      const target = path ?? (await save({ filters, defaultPath: `${app.project.name}.json` }));
      if (!target) return;
      await backend.saveProject(target, $state.snapshot(app.project) as Project);
      app.path = target;
      app.dirty = false;
    });

  const reference = () => attempt(async () => load(await backend.stockProject(), null));

  const previewStatus = $derived(
    app.invalid
      ? 'preview unavailable'
      : app.previewError
        ? `preview failed: ${app.previewError}`
        : app.previewMs !== null
          ? `preview ${Math.round(app.previewMs)} ms`
          : 'previewing…',
  );
</script>

<header>
  <div class="title">
    <strong data-testid="project-name">{app.project?.name ?? 'No project'}</strong>
    {#if app.dirty}<span class="muted">(edited)</span>{/if}
    <span class="muted path">{app.path ?? 'not saved'}</span>
  </div>
  <div class="actions">
    <button onclick={reference}>Reference W205</button>
    <button onclick={openFile}>Open…</button>
    <button onclick={() => saveAs(app.path)} disabled={!app.project}>Save</button>
    <button onclick={() => saveAs(null)} disabled={!app.project}>Save as…</button>
    {#if app.timeDomain.running}
      <span class="mono" data-testid="td-progress">
        time domain {app.timeDomain.points.length}/{app.timeDomain.total}
      </span>
      <button onclick={cancelTimeDomain}>Cancel</button>
    {:else}
      <button
        class="primary"
        onclick={solveTimeDomain}
        disabled={!app.project || !!app.invalid}
        title="Nonlinear time-domain sweep: the reference result">Solve time domain</button
      >
    {/if}
    <span class="muted mono status">{previewStatus}</span>
  </div>
  {#if failure}<div class="failure">{failure}</div>{/if}
</header>

<style>
  header {
    grid-area: top;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 16px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--line);
    background: var(--panel);
  }

  .title {
    display: flex;
    align-items: baseline;
    gap: 8px;
    min-width: 0;
    flex: 1;
  }

  .path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .status {
    min-width: 130px;
  }

  .failure {
    flex-basis: 100%;
    color: var(--bad);
  }
</style>
