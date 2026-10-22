<script>
  /** A line that does not fit shows its start, waits, scrolls slowly to its
   *  end, waits, and comes back: nothing is cut for good. */
  let { text = '', hold = 1800, speed = 30 } = $props();
  let box = $state();
  let inner = $state();
  let over = $state(false);

  $effect(() => {
    text;
    if (!box || !inner) return;
    let anim = null;
    const measure = () => {
      anim?.cancel();
      anim = null;
      const hidden = inner.scrollWidth - box.clientWidth;
      over = hidden > 2;
      if (!over || matchMedia('(prefers-reduced-motion: reduce)').matches) return;
      const go = (hidden / speed) * 1000;
      const back = 600;
      const total = hold + go + hold + back;
      const far = `translateX(-${hidden + 4}px)`;
      anim = inner.animate(
        [
          { transform: 'translateX(0)', offset: 0, easing: 'linear' },
          { transform: 'translateX(0)', offset: hold / total, easing: 'linear' },
          { transform: far, offset: (hold + go) / total, easing: 'linear' },
          { transform: far, offset: (hold + go + hold) / total, easing: 'ease-in-out' },
          { transform: 'translateX(0)', offset: 1 },
        ],
        { duration: total, iterations: Infinity },
      );
    };
    measure();
    const watch = new ResizeObserver(measure);
    watch.observe(box);
    return () => {
      watch.disconnect();
      anim?.cancel();
    };
  });
</script>

<span class="mq" class:over bind:this={box}><span class="in" bind:this={inner}>{text}</span></span>

<style>
  .mq {
    display: block;
    overflow: hidden;
    white-space: nowrap;
    min-width: 0;
  }

  .mq.over {
    mask-image: linear-gradient(90deg, #000 calc(100% - 22px), transparent);
  }

  .in {
    display: inline-block;
    will-change: transform;
  }
</style>
