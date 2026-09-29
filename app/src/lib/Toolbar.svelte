<script lang="ts">
  import { open, save } from '@tauri-apps/plugin-dialog';
  import { backend } from './backend';
  import { app, cancelTimeDomain, load, redo, solveTimeDomain, undo } from './state.svelte';
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

  // ⌘O open, ⌘S save, ⇧⌘S save as, ⌘Z undo, ⇧⌘Z redo; typing in a field keeps its own undo.
  function shortcut(e: KeyboardEvent) {
    if (!(e.metaKey || e.ctrlKey)) return;
    const key = e.key.toLowerCase();
    const typing = e.target instanceof HTMLElement && e.target.closest('input, select, textarea');
    const run = (action: () => void) => {
      e.preventDefault();
      action();
    };
    if (key === 'o') run(openFile);
    else if (key === 's') run(() => saveAs(e.shiftKey ? null : app.path));
    else if (key === 'z' && !typing) run(e.shiftKey ? redo : undo);
    else if (key === 'y' && !typing) run(redo);
  }
</script>

<svelte:window onkeydown={shortcut} />

<header>
  <div class="title">
    <strong data-testid="project-name">{app.project?.name ?? 'No project'}</strong>
    {#if app.dirty}<span class="muted">(edited)</span>{/if}
    <span class="muted path">{app.path ?? 'not saved'}</span>
  </div>
  <div class="actions">
    <div class="group" aria-label="File">
      <button onclick={reference} title="Open the reference W205 project">Reference W205</button>
      <button onclick={openFile} title="Open a project (⌘O)">Open…</button>
      <button onclick={() => saveAs(app.path)} disabled={!app.project} title="Save (⌘S)">Save</button>
      <button onclick={() => saveAs(null)} disabled={!app.project} title="Save as (⇧⌘S)">Save as…</button>
    </div>
    <div class="group" aria-label="Edit">
      <button onclick={undo} disabled={!app.history.undo} title="Undo (⌘Z)" data-testid="undo">Undo</button>
      <button onclick={redo} disabled={!app.history.redo} title="Redo (⇧⌘Z)" data-testid="redo">Redo</button>
    </div>
    <div class="group">
      <button data-testid="units" title="Show lengths in millimetres or inches" onclick={() => (app.inches = !app.inches)}>
        {app.inches ? 'inches' : 'mm'}
      </button>
    </div>
    {#if app.timeDomain.running}
      <button onclick={cancelTimeDomain} title="Stop the sweep, points in flight included">Cancel sweep</button>
    {:else}
      <button
        class="primary"
        onclick={solveTimeDomain}
        disabled={!app.project || !!app.invalid}
        title="Nonlinear time-domain sweep: the reference result">Solve time domain</button
      >
    {/if}
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
    gap: 12px;
  }

  .group {
    display: flex;
    gap: 4px;
    padding-right: 12px;
    border-right: 1px solid var(--line);
  }

  .failure {
    flex-basis: 100%;
    color: var(--bad);
  }
</style>
