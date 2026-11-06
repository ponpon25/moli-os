// Which theme the house shows. Kept by the house (`prefs.json`, key
// `maison-affichage`), like what each theme arranged under its own keys:
// changing theme loses nothing of the other.

import { onPrefs, savePref } from './home.svelte.js';

const KEY = 'maison-affichage';

const read = () => {
  try {
    return JSON.parse(localStorage.getItem(KEY) ?? 'null');
  } catch {
    return null;
  }
};

export const look = $state({ id: read() });

onPrefs((kept) => {
  if (typeof kept[KEY] === 'string') look.id = kept[KEY];
});

export function setLook(id) {
  look.id = id;
  savePref(KEY, JSON.stringify(id));
}
