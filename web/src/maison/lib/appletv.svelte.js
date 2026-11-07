// The Apple TV, as the dashboard shows it: found among the devices, its
// apps, the CANAL+ channels it opens live, and where its sound goes.
import { hub, value, act } from './home.svelte.js';
import { t } from '../../lib/i18n.svelte.js';

/** The Apple TV (driver `appletv`). */
export function appleTv() {
  return Object.values(hub.devices).find(isAppleTv);
}

export function isAppleTv(d) {
  return !!d && d.manufacturer === 'Apple' && d.model === 'Apple TV' && d.points.some((p) => p.key === 'key');
}

/** The apps opened from the dashboard (bundle identifiers), CANAL+ first. */
export const APPLE_APPS = [
  { id: 'com.canalplusdistrib.mycanal.prod', name: 'CANAL+' },
  { id: 'com.netflix.Netflix', name: 'Netflix' },
  { id: 'com.google.ios.youtube', name: 'YouTube' },
  { id: 'com.disney.disneyplus', name: 'Disney+' },
  { id: 'com.amazon.aiv.AIVApp', name: 'Prime Video' },
  { id: 'com.apple.TVWatchList', name: 'Apple TV' },
];

/** The channels its `canal_channel` point offers (the house's grid order). */
export function canalChannels(id) {
  return hub.devices[id]?.points.find((p) => p.key === 'canal_channel')?.kind?.values ?? [];
}

/** The ones shown as buttons: the first of the grid (the free channels and
 *  CANAL+); the others in a list. */
export const CANAL_FIRST = 7;

export function watch(id, channel) {
  return act(`${id}/canal_channel`, channel, t('salon.atv.chaine', { chaine: channel }));
}

/** Its sound switches: the AirPlay speakers, then its own speakers. */
export function soundSwitches(id) {
  const points = hub.devices[id]?.points ?? [];
  return [...points.filter((p) => p.key.startsWith('airplay_')), ...points.filter((p) => p.key === 'tv_speakers')];
}

/** A Sonos of Moli behind an AirPlay switch (`airplay_aabbccddeeff`): its
 *  id carries the same address (`sonos:RINCON_AABBCCDDEEFF01400`). */
export function sonosOf(key) {
  const hex = key.replace('airplay_', '').toUpperCase();
  return Object.values(hub.devices).find((d) => d.id.startsWith('sonos:') && d.id.includes(`RINCON_${hex}`))?.id ?? null;
}

/** The Sonos that plays the Apple TV now, if one does: its volume is the
 *  one that matters. */
export function soundSpeaker(id) {
  const on = soundSwitches(id).find((p) => p.key.startsWith('airplay_') && value(id, p.key) === true);
  return on ? sonosOf(on.key) : null;
}
