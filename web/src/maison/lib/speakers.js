// What the speaker cards and pages ask of a device, shared.

import { hub, value } from './home.svelte.js';

/** Whether a device has a point. */
export const has = (id, key) => hub.devices[id]?.points?.some((p) => p.key === key) ?? false;

/** A list a device keeps as JSON in a point (favorites, queue, results). */
export const list = (id, key) => {
  try {
    const v = JSON.parse(value(id, key) ?? '[]');
    return Array.isArray(v) ? v : [];
  } catch {
    return [];
  }
};

/** What a source lets you do: a queue or a station answers next and previous. */
export const steerable = (src) => src === 'music' || src === 'radio';

export const mmss = (sec) => {
  const s = Math.max(0, Math.round(sec));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
};
