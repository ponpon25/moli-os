<script>
  /** A level set to the unit, its value shown while the finger moves. The
   *  order leaves when it lifts, once. `unit`: said after the value. */
  let { value = 0, label, min = 0, max = 100, unit = ' %', disabled = false, onchange } = $props();

  let drag = $state(null);
  const shown = $derived(drag ?? value);
  const fill = $derived(max > min ? ((shown - min) / (max - min)) * 100 : 0);
</script>

<div class="level" class:disabled>
  <input
    type="range"
    {min}
    {max}
    step="1"
    value={shown}
    {disabled}
    aria-label={label}
    style:--p="{fill}%"
    oninput={(e) => (drag = Number(e.currentTarget.value))}
    onchange={(e) => {
      const v = Number(e.currentTarget.value);
      drag = null;
      if (v !== value) onchange?.(v);
    }}
  />
  <output class="num" class:moving={drag != null}>{shown}{unit}</output>
</div>

<style>
  .level {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    margin-top: 2px;
  }

  .level.disabled {
    opacity: 0.45;
  }

  input {
    flex: 1;
    min-width: 0;
    height: 30px;
    margin: 0;
    appearance: none;
    background: transparent;
    cursor: pointer;
  }

  input:disabled {
    cursor: not-allowed;
  }

  input:focus-visible {
    outline: 2px solid var(--warm);
    outline-offset: 2px;
    border-radius: 8px;
  }

  input::-webkit-slider-runnable-track {
    height: 6px;
    border-radius: 6px;
    background: linear-gradient(90deg, var(--warm) var(--p), var(--surface-3) var(--p));
  }

  input::-moz-range-track {
    height: 6px;
    border-radius: 6px;
    background: var(--surface-3);
  }

  input::-moz-range-progress {
    height: 6px;
    border-radius: 6px;
    background: var(--warm);
  }

  input::-webkit-slider-thumb {
    appearance: none;
    width: 20px;
    height: 20px;
    margin-top: -7px;
    border-radius: 50%;
    background: var(--ink);
  }

  input::-moz-range-thumb {
    width: 20px;
    height: 20px;
    border: 0;
    border-radius: 50%;
    background: var(--ink);
  }

  output {
    min-width: 44px;
    color: var(--ink-3);
    font-size: 13.5px;
    font-weight: 600;
    text-align: right;
  }

  output.moving {
    color: var(--ink);
  }
</style>
