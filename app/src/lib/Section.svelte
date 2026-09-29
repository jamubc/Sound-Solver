<script lang="ts">
  import { untrack, type Snippet } from 'svelte';
  import Icon from './Icon.svelte';

  let { title, open = true, children }: { title: string; open?: boolean; children: Snippet } = $props();
  let shown = $state(untrack(() => open));
</script>

<section class="section">
  <button class="title" onclick={() => (shown = !shown)} aria-expanded={shown}>
    <span class="caret" class:open={shown}><Icon name="chevron" size={12} /></span>
    {title}
  </button>
  {#if shown}<div class="content">{@render children()}</div>{/if}
</section>

<style>
  .section {
    border-bottom: 1px solid var(--line);
  }

  .title {
    display: flex;
    align-items: center;
    gap: 5px;
    width: 100%;
    padding: 7px 10px;
    border: none;
    border-radius: 0;
    background: none;
    color: var(--muted);
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    text-align: left;
  }

  .title:hover {
    color: var(--text);
  }

  .caret {
    display: flex;
    transition: transform 0.12s;
  }

  .caret.open {
    transform: rotate(90deg);
  }

  .content {
    padding: 0 10px 10px;
  }
</style>
