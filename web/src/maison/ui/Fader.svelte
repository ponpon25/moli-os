<script>
  /** A tall slider, one gesture for a light or a shutter. A slide sets it to
   *  the unit, its value shown meanwhile, and the order leaves when the finger
   *  lifts. A press switches a light (`tap`)
   *  or sends a shutter where it was touched. */
  let { value = 0, label, word = '', kind = 'light', disabled = false, onchange, ontap = null } = $props();

  let drag = $state(null);
  const shown = $derived(drag ?? value);
  let start = null;
  let el = $state();

  const at = (e) => {
    const r = el.getBoundingClientRect();
    return Math.round(Math.max(0, Math.min(1, 1 - (e.clientY - r.top) / r.height)) * 100);
  };
  function down(e) {
    if (disabled) return;
    el.setPointerCapture(e.pointerId);
    start = { y: e.clientY, moved: false };
  }
  function move(e) {
    if (!start) return;
    if (Math.abs(e.clientY - start.y) > 5) start.moved = true;
    if (start.moved) drag = at(e);
  }
  function up(e) {
    if (!start) return;
    const { moved } = start;
    const final = moved ? drag : at(e);
    start = null;
    drag = null;
    if (!moved && ontap) ontap();
    else if (final !== value) onchange?.(final);
  }
  function cancel() {
    start = null;
    drag = null;
  }
  function key(e) {
    if (disabled) return;
    const step = { ArrowUp: 1, ArrowRight: 1, ArrowDown: -1, ArrowLeft: -1, PageUp: 10, PageDown: -10 }[e.key];
    if (step) {
      e.preventDefault();
      onchange?.(Math.max(0, Math.min(100, value + step)));
    } else if ((e.key === ' ' || e.key === 'Enter') && ontap) {
      e.preventDefault();
      ontap();
    }
  }
</script>

<div class="wrap">
  <div
    bind:this={el}
    class="fader {kind}"
    class:on={shown > 0}
    class:disabled
    role="slider"
    tabindex={disabled ? -1 : 0}
    aria-label={label}
    aria-valuemin="0"
    aria-valuemax="100"
    aria-valuenow={shown}
    aria-valuetext={drag != null ? `${drag} %` : word}
    aria-disabled={disabled}
    style:--v="{shown}%"
    onpointerdown={down}
    onpointermove={move}
    onpointerup={up}
    onpointercancel={cancel}
    onkeydown={key}
  >
    <span class="fill"></span>
    <span class="val">{drag != null ? `${drag} %` : word}</span>
  </div>
  <b>{label}</b>
</div>

<style>
  .wrap {
    display: grid;
    gap: 8px;
    justify-items: center;
    width: 94px;
    text-align: center;
  }

  .fader {
    position: relative;
    width: 100%;
    height: 190px;
    overflow: hidden;
    border-radius: 24px;
    background: var(--surface-3);
    cursor: ns-resize;
    touch-action: none;
    user-select: none;
  }

  .fader.disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .fader:focus-visible {
    outline: 2px solid var(--warm);
    outline-offset: 3px;
  }

  .fill {
    position: absolute;
    inset: auto 0 0 0;
    height: var(--v);
    transition: height 0.14s;
  }

  .light .fill {
    background: linear-gradient(180deg, #ffd27a, #f5a93c);
    box-shadow: 0 -12px 34px -4px var(--warm-glow);
  }

  /* What is filled is the daylight coming in; above it, the slats. */
  .cover .fill {
    background: linear-gradient(180deg, #bfdcf4, #e4f0f6);
  }

  .cover::before {
    content: '';
    position: absolute;
    inset: 0;
    background: repeating-linear-gradient(180deg, transparent 0 10px, color-mix(in srgb, var(--ink) 8%, transparent) 10px 12px);
  }

  .val {
    position: absolute;
    inset: auto 4px 12px;
    color: var(--ink-2);
    font-size: 13px;
    font-weight: 620;
    line-height: 1.15;
    pointer-events: none;
  }

  .fader.on .val {
    color: #2d2108;
  }

  .cover.on .val {
    color: #213544;
  }

  b {
    font-size: 14px;
    font-weight: 620;
    line-height: 1.2;
    overflow-wrap: anywhere;
  }
</style>
