// The dashboard's themes. A theme is only how the house is shown: a `Frame`
// (the window, the way between pages, the routing) and its own pages where it
// has any; the devices, automations and settings are the same underneath.
// Another one: a folder here, with its `Frame.svelte`, and a line below.
// `label` and `desc` are catalogue keys, `preview` two colours.

export const THEMES = [
  { id: 'ciel', label: 'maison.theme.ciel', desc: 'maison.theme.ciel_desc', preview: ['#101a33', '#4f9ae9'], load: () => import('./ciel/Frame.svelte') },
  { id: 'classique', label: 'maison.theme.classique', desc: 'maison.theme.classique_desc', preview: ['#edf0f5', '#ffffff'], load: () => import('./classique/Frame.svelte') },
];

/** The first one is the default, for a house that has not chosen. */
export const themeOf = (id) => THEMES.find((x) => x.id === id) ?? THEMES[0];
