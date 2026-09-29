<script lang="ts" module>
  // Line icons on a 24-unit grid, drawn with the current colour.
  const ICONS: Record<string, string> = {
    new: 'M6 3h8l4 4v14H6z M14 3v4h4 M12 11v6 M9 14h6',
    open: 'M3 7V5h6l2 2h10v12H3z M3 10h18',
    save: 'M5 3h12l4 4v14H5z M8 3v5h8V3 M8 21v-7h8v7',
    saveas: 'M5 3h12l4 4v6 M5 3v18h7 M8 3v5h8V3 M15 21l6-6-2-2-6 6v2z',
    undo: 'M9 14L4 9l5-5 M4 9h10a6 6 0 010 12h-3',
    redo: 'M15 14l5-5-5-5 M20 9H10a6 6 0 000 12h3',
    trash: 'M4 7h16 M9 7V4h6v3 M6 7l1 14h10l1-14 M10 11v6 M14 11v6',
    bend: 'M3 19h6a6 6 0 006-6V5 M12 8l3-3 3 3',
    hanger: 'M12 3v8 M7 11h10 M8 11a4 4 0 008 0 M5 21h14 M12 15v6',
    place: 'M3 12h5 M16 12h5 M8 7h8v10H8z M12 10v4 M10 12h4',
    fit: 'M4 9V4h5 M15 4h5v5 M20 15v5h-5 M9 20H4v-5 M9 9h6v6H9z',
    target: 'M12 3v4 M12 17v4 M3 12h4 M17 12h4 M12 12m-5 0a5 5 0 1010 0a5 5 0 10-10 0',
    cube: 'M12 3l8 4.5v9L12 21l-8-4.5v-9z M12 12l8-4.5 M12 12v9 M12 12L4 7.5',
    ortho: 'M4 4h16v16H4z M4 12h16 M12 4v16',
    persp: 'M3 20L9 4h6l6 16z M6 12h12',
    play: 'M7 4l13 8-13 8z',
    stop: 'M6 6h12v12H6z',
    pin: 'M9 3h6l-1 6 4 4H6l4-4z M12 13v8',
    clear: 'M6 6l12 12 M18 6L6 18',
    speaker: 'M4 9h4l5-4v14l-5-4H4z M16 9a4 4 0 010 6 M19 6a8 8 0 010 12',
    mic: 'M9 3h6v10H9z M5 11a7 7 0 0014 0 M12 18v3 M8 21h8',
    cabin: 'M5 21v-6l2-8h6l1 4h5v10 M5 15h14 M9 21v-3 M16 21v-3',
    box: 'M12 3l9 4.5v9L12 21 3 16.5v-9z M3 7.5l9 4.5 9-4.5 M12 12v9',
    scan: 'M4 8V4h4 M16 4h4v4 M20 16v4h-4 M8 20H4v-4 M7 12h10',
    point: 'M12 12m-3 0a3 3 0 106 0a3 3 0 10-6 0 M12 3v4 M12 17v4 M3 12h4 M17 12h4',
    gear:
      'M12 9a3 3 0 100 6a3 3 0 100-6 M19 12l2-1-1-3-2 .3-1.3-1.3.3-2-3-1-1 2h-2l-1-2-3 1 .3 2L6 7.3 4 7l-1 3 2 1v2l-2 1 1 3 2-.3L7.3 17 7 19l3 1 1-2h2l1 2 3-1-.3-2 1.3-1.3 2 .3 1-3-2-1z',
    ruler: 'M3 17L17 3l4 4L7 21z M7 13l2 2 M10 10l2 2 M13 7l2 2',
    tag: 'M3 12V4h8l10 10-8 8z M8 8h.01',
    axis: 'M3 12h18 M3 12l3-3 M3 12l3 3',
    grid: 'M4 4h16v16H4z M4 9.3h16 M4 14.6h16 M9.3 4v16 M14.6 4v16',
    help: 'M12 21a9 9 0 100-18 9 9 0 000 18 M9.5 9a2.5 2.5 0 015 .5c0 1.5-2.5 2-2.5 3.5 M12 17h.01',
    chevron: 'M9 6l6 6-6 6',
    route: 'M3 17h5a4 4 0 004-4V11a4 4 0 014-4h5',
    element: 'M4 8h16v8H4z M8 8v8 M16 8v8',
    project: 'M4 5h6l2 2h8v12H4z',
    wave: 'M2 12h3l2-6 3 12 3-9 2 6 2-3h5',
    wrench: 'M14 7a4 4 0 015 5l-8 8-3-3 8-8 M5 5l3 3',
    pencil: 'M4 20l4-1L19 8l-3-3L5 16z M14 7l3 3',
    activity: 'M3 12h4l3-8 4 16 3-8h4',
    dock: 'M3 4h18v16H3z M3 14h18',
    panel: 'M3 4h18v16H3z M9 4v16',
    measure: 'M3 12h18 M3 8v8 M21 8v8 M8 10v4 M13 10v4 M18 10v4',
  };
</script>

<script lang="ts">
  let { name, size = 16 }: { name: keyof typeof ICONS | string; size?: number } = $props();
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="1.6"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"><path d={ICONS[name] ?? ''} /></svg
>

<style>
  svg {
    flex: none;
    display: block;
  }
</style>
