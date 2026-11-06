// What the family arranged on a page: the order of its things and those it hid.
// Kept by the house (so every screen sees the same), one entry a list.

import { onPrefs, savePref } from './home.svelte.js';

const KEY = 'maison-ordre';

const read = () => {
  try {
    const kept = JSON.parse(localStorage.getItem(KEY) ?? '{}');
    return kept && typeof kept === 'object' && !Array.isArray(kept) ? kept : {};
  } catch {
    return {};
  }
};

export const arrangements = $state({ lists: read() });

onPrefs((kept) => {
  const lists = kept[KEY];
  if (lists && typeof lists === 'object' && !Array.isArray(lists)) arrangements.lists = lists;
});

const keep = () => savePref(KEY, JSON.stringify(arrangements.lists));
const entry = (list) => arrangements.lists[list] ?? { order: [], hidden: [] };

export const isHiddenIn = (list, id) => entry(list).hidden?.includes(id) ?? false;

/** The things in the family's order (new ones last, in the order given),
 *  without those hidden unless `all`. */
export function arranged(list, items, idOf, all = false) {
  const { order = [] } = entry(list);
  const rank = (x) => {
    const i = order.indexOf(idOf(x));
    return i < 0 ? order.length : i;
  };
  const sorted = items
    .map((x, i) => [x, i])
    .sort((a, b) => rank(a[0]) - rank(b[0]) || a[1] - b[1])
    .map(([x]) => x);
  return all ? sorted : sorted.filter((x) => !isHiddenIn(list, idOf(x)));
}

/** A new order for these things; those not named keep their place after them. */
export function setOrder(list, ids) {
  const { order = [], hidden = [] } = entry(list);
  arrangements.lists = { ...arrangements.lists, [list]: { hidden, order: [...ids, ...order.filter((id) => !ids.includes(id))] } };
  keep();
}

export function setHiddenIn(list, id, hide) {
  const { order = [], hidden = [] } = entry(list);
  const rest = hidden.filter((x) => x !== id);
  arrangements.lists = { ...arrangements.lists, [list]: { order, hidden: hide ? [...rest, id] : rest } };
  keep();
}

/** Back to how the page comes. */
export function resetLists(lists) {
  const next = { ...arrangements.lists };
  for (const list of lists) delete next[list];
  arrangements.lists = next;
  keep();
}
