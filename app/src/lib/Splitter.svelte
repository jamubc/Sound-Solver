<script lang="ts">
  /** A drag handle that resizes the panel on one side of it. */
  let {
    axis,
    size,
    min,
    max,
    invert = false,
    onresize,
  }: {
    axis: 'x' | 'y';
    size: number;
    min: number;
    max: number;
    /** The panel grows as the pointer moves left or up. */
    invert?: boolean;
    onresize: (size: number, done: boolean) => void;
  } = $props();

  function start(e: PointerEvent) {
    const handle = e.currentTarget as HTMLElement;
    handle.setPointerCapture(e.pointerId);
    const [from, origin] = [size, axis === 'x' ? e.clientX : e.clientY];
    const at = (m: PointerEvent) => {
      const moved = (axis === 'x' ? m.clientX : m.clientY) - origin;
      return Math.min(max, Math.max(min, from + (invert ? -moved : moved)));
    };
    const move = (m: PointerEvent) => onresize(at(m), false);
    const end = (m: PointerEvent) => {
      onresize(at(m), true);
      handle.removeEventListener('pointermove', move);
      handle.removeEventListener('pointerup', end);
    };
    handle.addEventListener('pointermove', move);
    handle.addEventListener('pointerup', end);
  }
</script>

<div class="splitter {axis}" role="separator" aria-orientation={axis === 'x' ? 'vertical' : 'horizontal'} onpointerdown={start}></div>

<style>
  .splitter {
    position: relative;
    z-index: 5;
    background: var(--line);
  }

  .splitter.x {
    width: 1px;
    cursor: col-resize;
  }

  .splitter.y {
    height: 1px;
    cursor: row-resize;
  }

  /* A wider grip than the visible line. */
  .splitter::after {
    content: '';
    position: absolute;
    inset: -3px;
  }

  .splitter:hover,
  .splitter:active {
    background: var(--accent);
  }
</style>
