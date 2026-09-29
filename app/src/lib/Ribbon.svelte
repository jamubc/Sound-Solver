<script lang="ts">
  import {
    addHanger,
    addRecording,
    addVia,
    canDelete,
    deleteSelected,
    inline,
    loadScan,
    openProject,
    placeElement,
    reference,
    saveProject,
    selectedRoute,
  } from './actions.svelte';
  import Icon from './Icon.svelte';
  import { play, player, stop } from './player.svelte';
  import { app, cancelTimeDomain, pinBaseline, redo, solveTimeDomain, undo } from './state.svelte';
  import { enter, persist, show, ui, view, WORKSPACES } from './ui.svelte';

  let placing = $state('quarter_wave_stub');
  const route = $derived(selectedRoute());
  const scan = $derived(app.project?.fabrication?.scan ?? null);

  // Keyboard: ⌘O ⌘S ⇧⌘S ⌘Z ⇧⌘Z, Delete, and ? for the list. Fields keep their own keys.
  function keys(e: KeyboardEvent) {
    const typing = e.target instanceof HTMLElement && !!e.target.closest('input, select, textarea');
    const run = (action: () => void) => {
      e.preventDefault();
      action();
    };
    const key = e.key.toLowerCase();
    if (e.metaKey || e.ctrlKey) {
      if (key === 'o') run(openProject);
      else if (key === 's') run(() => saveProject(e.shiftKey));
      else if (key === 'z' && !typing) run(e.shiftKey ? redo : undo);
      else if (key === 'y' && !typing) run(redo);
      return;
    }
    if (typing) return;
    if (key === 'delete' || key === 'backspace') run(deleteSelected);
    else if (key === '?') run(() => (ui.help = !ui.help));
    else if (key === ' ' && ui.dock === 'listen') run(() => (player.playing ? stop() : play('current')));
  }

  const SHORTCUTS: [string, string][] = [
    ['⌘O / ⌘S / ⇧⌘S', 'Open, save, save as'],
    ['⌘Z / ⇧⌘Z', 'Undo, redo'],
    ['Delete', 'Remove the selected bend point or in-line element'],
    ['F', 'Fit everything in view'],
    ['Esc', 'Clear the selection'],
    ['Drag / right-drag / scroll', 'Orbit, pan, zoom at the cursor'],
    ['Space', 'Play or stop, on the Listen tab'],
    ['?', 'This list'],
  ];
</script>

<svelte:window onkeydown={keys} />

{#snippet command(icon: string, label: string, action: () => void, options: { disabled?: boolean; title?: string; on?: boolean; primary?: boolean; testid?: string } = {})}
  <button
    class="command"
    class:on={options.on}
    class:primary={options.primary}
    onclick={action}
    disabled={options.disabled}
    title={options.title ?? label}
    data-testid={options.testid}
  >
    <Icon name={icon} size={18} />
    <span>{label}</span>
  </button>
{/snippet}

<header class="ribbon">
  <div class="titlebar">
    <span class="app">Exhaust</span>
    <span class="sep">/</span>
    <strong data-testid="project-name">{app.project?.name ?? 'No project'}</strong>
    {#if app.dirty}<span class="edited" title="Unsaved changes">●</span>{/if}
    <span class="path muted" title={app.path ?? ''}>{app.path ?? 'not saved'}</span>
    <span class="spacer"></span>
    <button class="flat" data-testid="units" title="Show lengths in millimetres or inches" onclick={() => (app.inches = !app.inches)}>
      {app.inches ? 'inches' : 'mm'}
    </button>
    <button class="flat" title="Keyboard shortcuts (?)" onclick={() => (ui.help = !ui.help)}><Icon name="help" /></button>
  </div>
  <div class="tabs" role="tablist">
    {#each WORKSPACES as [id, name] (id)}
      <button role="tab" aria-selected={ui.workspace === id} class:active={ui.workspace === id} onclick={() => enter(id)}>
        {name}
      </button>
    {/each}
  </div>
  <div class="commands">
    <div class="group">
      <div class="row">
        {@render command('new', 'Reference', reference, { title: 'Open the reference W205 project' })}
        {@render command('open', 'Open', openProject, { title: 'Open a project (⌘O)' })}
        {@render command('save', 'Save', () => saveProject(false), { disabled: !app.project, title: 'Save (⌘S)' })}
        {@render command('saveas', 'Save as', () => saveProject(true), { disabled: !app.project, title: 'Save as (⇧⌘S)' })}
      </div>
      <span class="caption">File</span>
    </div>
    <div class="group">
      <div class="row">
        {@render command('undo', 'Undo', undo, { disabled: !app.history.undo, title: 'Undo (⌘Z)', testid: 'undo' })}
        {@render command('redo', 'Redo', redo, { disabled: !app.history.redo, title: 'Redo (⇧⌘Z)', testid: 'redo' })}
        {@render command('trash', 'Delete', deleteSelected, { disabled: !canDelete(), title: 'Remove the selected bend point or in-line element (Delete)' })}
      </div>
      <span class="caption">Edit</span>
    </div>

    {#if ui.workspace === 'design'}
      <div class="group">
        <div class="row">
          {@render command('bend', 'Bend point', () => route && addVia(route), { disabled: !route, title: 'Add a bend point to the selected pipe' })}
          {@render command('hanger', 'Hanger', () => route && addHanger(route), { disabled: !route, title: 'Hang the selected pipe at the picked spot' })}
          <div class="place">
            <select bind:value={placing} title="Element to place" data-testid="ribbon-place-kind">
              {#each inline() as m (m.type)}<option value={m.type}>{m.name}</option>{/each}
            </select>
            <button class="small" disabled={!route} onclick={() => route && placeElement(route, placing)} title="Place it on the selected pipe at the picked spot">
              <Icon name="place" /> Place
            </button>
          </div>
        </div>
        <span class="caption">Pipe {route ? `· ${route}` : '· select a pipe'}</span>
      </div>
      <div class="group">
        <div class="row">
          {@render command('fit', 'Fit', () => view.fitAll(), { title: 'Fit everything in view (F)' })}
          {@render command('target', 'Selection', () => view.fitSelection(), { disabled: !app.selection })}
          {@render command(ui.ortho ? 'ortho' : 'persp', ui.ortho ? 'Orthographic' : 'Perspective', () => {
            ui.ortho = !ui.ortho;
            persist();
          })}
        </div>
        <span class="caption">View</span>
      </div>
    {:else if ui.workspace === 'simulate'}
      <div class="group">
        <div class="row">
          {#if app.timeDomain.running}
            {@render command('stop', 'Cancel sweep', cancelTimeDomain, { title: 'Stop the sweep, points in flight included' })}
          {:else}
            {@render command('play', 'Solve time domain', solveTimeDomain, {
              disabled: !app.project || !!app.invalid,
              primary: true,
              title: 'Nonlinear time-domain sweep: the reference result',
            })}
          {/if}
        </div>
        <span class="caption">Solve · the four-pole preview follows every edit</span>
      </div>
      <div class="group">
        <div class="row">
          {@render command('speaker', 'Listen', () => show('listen'), { on: ui.dock === 'listen', title: 'Hear the prediction' })}
          {@render command(player.playing ? 'stop' : 'play', player.playing ? 'Stop' : 'Play', () =>
            player.playing ? stop() : (show('listen'), play('current')),
          )}
        </div>
        <span class="caption">Sound</span>
      </div>
      <div class="group">
        <div class="row">
          {@render command('pin', 'Pin baseline', pinBaseline, { disabled: !app.preview, title: 'Keep these results to compare the next design with' })}
          {@render command('clear', 'Clear', () => (app.baseline = null), { disabled: !app.baseline })}
        </div>
        <span class="caption">Compare {app.baseline ? `· ${app.baseline.name}` : ''}</span>
      </div>
      <div class="group">
        <div class="row">
          {@render command('gear', 'Settings', () => (app.selection = null), { title: 'Engine, operating sweep, solver and receiver: shown in the inspector with nothing selected' })}
        </div>
        <span class="caption">Project</span>
      </div>
    {:else if ui.workspace === 'measure'}
      <div class="group">
        <div class="row">
          {@render command('mic', 'Add recording', () => (show('measure'), addRecording()), {
            title: 'A WAV or CAF recording of the car',
          })}
          {@render command('cabin', 'Cabin transfer', () => show('measure'), { title: 'Measure the cabin transfer function' })}
        </div>
        <span class="caption">Recordings</span>
      </div>
    {:else}
      <div class="group">
        <div class="row">
          {@render command('box', 'Package', () => show('fabricate'), { title: 'Cut list, bends, welds, STEP and STL' })}
        </div>
        <span class="caption">Fabricate</span>
      </div>
      <div class="group">
        <div class="row">
          {@render command('scan', scan ? 'Replace scan' : 'Load scan', () => (show('fabricate'), loadScan()))}
          {#each [0, 1, 2] as k}
            {@render command('point', `Point ${k + 1}`, () => (app.picking = app.picking === k ? null : k), {
              disabled: !app.scanMesh,
              on: app.picking === k,
              title: `Pick reference point ${k + 1} on the scan`,
            })}
          {/each}
        </div>
        <span class="caption">Underbody scan</span>
      </div>
    {/if}
  </div>
</header>

{#if ui.help}
  <div class="help" role="dialog" aria-label="Keyboard shortcuts">
    <h2>Keyboard</h2>
    <dl>
      {#each SHORTCUTS as [k, what]}<dt>{k}</dt><dd>{what}</dd>{/each}
    </dl>
    <button onclick={() => (ui.help = false)}>Close</button>
  </div>
{/if}

<style>
  .ribbon {
    grid-area: ribbon;
    background: var(--panel);
    border-bottom: 1px solid var(--line);
  }

  .titlebar {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 30px;
    padding: 0 10px;
    border-bottom: 1px solid var(--line);
    background: var(--bg);
  }

  .app {
    font-weight: 700;
    letter-spacing: 0.04em;
    color: var(--accent);
  }

  .sep {
    color: var(--faint);
  }

  .edited {
    color: var(--warn);
    font-size: 10px;
  }

  .path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }

  .spacer {
    flex: 1;
  }

  button.flat {
    background: none;
    border-color: transparent;
    display: flex;
    align-items: center;
    color: var(--muted);
  }

  .tabs {
    display: flex;
    gap: 2px;
    padding: 4px 10px 0;
  }

  .tabs button {
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    border-radius: 0;
    padding: 4px 12px;
    color: var(--muted);
    font-weight: 600;
    letter-spacing: 0.03em;
  }

  .tabs button.active {
    color: var(--text);
    border-bottom-color: var(--accent);
  }

  .commands {
    display: flex;
    align-items: stretch;
    gap: 0;
    padding: 4px 6px 2px;
    min-height: 66px;
    overflow-x: auto;
  }

  .group {
    display: flex;
    flex-direction: column;
    justify-content: space-between;
    padding: 0 8px;
    border-right: 1px solid var(--line);
  }

  .row {
    display: flex;
    align-items: flex-start;
    gap: 2px;
  }

  .caption {
    font-size: 10px;
    color: var(--faint);
    text-align: center;
    white-space: nowrap;
    padding-top: 2px;
  }

  .command {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
    min-width: 54px;
    padding: 4px 6px;
    background: none;
    border-color: transparent;
    font-size: 11px;
  }

  .command:hover:not(:disabled) {
    background: var(--panel-2);
    border-color: var(--line-2);
  }

  .command.on {
    background: var(--accent-soft);
    border-color: var(--accent);
  }

  .command.primary {
    background: #1f4a80;
    border-color: #2d65a8;
    min-width: 110px;
  }

  .command span {
    white-space: nowrap;
  }

  .place {
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding: 2px 4px;
  }

  .place select {
    width: 170px;
  }

  .small {
    display: flex;
    align-items: center;
    gap: 4px;
    justify-content: center;
  }

  .help {
    position: fixed;
    top: 110px;
    right: 20px;
    z-index: 20;
    padding: 12px 16px;
    background: var(--panel-2);
    border: 1px solid var(--line-2);
    border-radius: 6px;
    box-shadow: 0 10px 30px rgba(0, 0, 0, 0.5);
  }

  .help dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 4px 14px;
    margin: 0 0 10px;
  }

  .help dt {
    font-family: ui-monospace, Menlo, monospace;
    color: var(--accent);
  }

  .help dd {
    margin: 0;
  }
</style>
