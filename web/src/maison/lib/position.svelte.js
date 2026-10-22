// Where a speaker is in its track, followed by the browser's own clock.
//
// The hub tells a position only when it moves (a new track, a jump, a pause):
// while it plays, the room follows by its clock. After a pause, the position
// is not told again when it resumes (it has not moved), so counting from the
// time it was told would add the whole pause. The count starts again from the
// moment the speaker is seen to play.

import { untrack } from 'svelte';
import { value, since } from './home.svelte.js';

/** `lead`, `playing` and `duration` are functions (they follow the card).
 *  Returns an object whose `pos` is the position in seconds, or null. */
export function createPosition(lead, playing, duration, every = 500) {
  let now = $state(Date.now());
  let anchor = $state({ p: null, at: 0 });
  let was = null;

  $effect(() => {
    const timer = setInterval(() => (now = Date.now()), every);
    return () => clearInterval(timer);
  });
  // What the hub told, and when.
  $effect(() => {
    const p = value(lead(), 'position');
    const at = since(lead(), 'position');
    anchor = { p: typeof p === 'number' ? p : null, at: at ?? Date.now() };
  });
  // Seen to start playing: count from now, from where it stood.
  $effect(() => {
    const is = playing();
    if (is && was === false) {
      const p = untrack(() => anchor.p);
      if (p != null) anchor = { p, at: Date.now() };
    }
    was = is;
  });

  const pos = $derived.by(() => {
    const total = duration();
    if (anchor.p == null || !total) return null;
    const moved = playing() ? Math.max(0, (now - anchor.at) / 1000) : 0;
    return Math.min(total, anchor.p + moved);
  });
  return {
    get pos() {
      return pos;
    },
  };
}
