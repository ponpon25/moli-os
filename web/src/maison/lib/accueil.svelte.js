// What the home page says of the house: one sentence, what is going on, and
// the house seen by use or room by room. Reading only: the orders go through
// `act` and the real cards, so the guard and the household's code still hold.

import {
  home, hub, device, value, isOn, deviceKind, roomList, roomOf, hidden, nameOf, reachable, doorOpen, leak, lowBattery, clock, num,
  isCoverGroup, isCharger, isSmoke, isThermo, chargerPhase, levelKey, powerKey, dimKey, shownPercent, levelAt, act, setPower, unpowered, savePref,
} from './home.svelte.js';
import { layout, onHome } from './layout.svelte.js';
import { isPrinter, phase } from './printers.js';
import { isAppleTv } from './appletv.svelte.js';
import { t } from '../../lib/i18n.svelte.js';

/** The house's alarm: a device that takes a `mode` among which « disarmed ». */
export const isAlarm = (d) => !!d?.points?.some((p) => p.key === 'mode' && p.kind?.values?.includes('disarmed'));

/** A helper (a presence, a mode, an input of an automation) is no equipment. */
const isHelper = (d) => hub.drivers?.find((x) => x.instance === d.instance)?.kind === 'helpers';

const collator = () => new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });
const byName = (a, b) => collator().compare(nameOf(a.id), nameOf(b.id));
/** What makes up a garage door (its relay, its sensor) is the door. */
const inGarage = () => new Set(Object.values(hub.devices).filter((d) => deviceKind(d) === 'garage').flatMap((d) => d.members ?? []));
const all = () => {
  const parts = inGarage();
  return Object.values(hub.devices).filter((d) => !hidden(d.id) && onHome(d.id) && !isHelper(d) && !parts.has(d.id));
};
const ofKind = (...kinds) => all().filter((d) => kinds.includes(deviceKind(d)) && !isCharger(d));

export const lights = () => ofKind('light').sort(byName);
export const covers = () => ofKind('cover').filter((d) => !isCoverGroup(d)).sort(byName);
export const coverGroups = () => ofKind('cover').filter(isCoverGroup).sort(byName);
export const clims = () => ofKind('climate').sort(byName);
export const speakers = () => ofKind('speaker').sort((a, b) => Number(value(b.id, 'playing') === true) - Number(value(a.id, 'playing') === true) || byName(a, b));
export const cameras = () => ofKind('camera').sort(byName);
export const alarms = () => all().filter(isAlarm);
export const garages = () => ofKind('garage').sort(byName);
export const smokes = () => all().filter(isSmoke).sort(byName);
export const chargers = () => all().filter(isCharger).sort(byName);
export const tvs = () => ofKind('tv').sort(byName);
/** A thermometer of the house, not the weather or the garden's. */
const isOutdoor = (d) => OUTSIDE.test(plain(roomOf(d))) || (home.config?.outdoor && Object.values(home.config.outdoor).includes(d.id));
const thermos = () => all().filter(isThermo);

/** A name without the room it is already under: « Store fenêtre Salon » in
 *  the Salon is « Store fenêtre ». A name that says nothing else stays. */
export function shortName(d, room) {
  const name = nameOf(d.id);
  if (!room) return name;
  // A whole word only (« Caméra extérieure » is not in « Extérieur »), with
  // the small word that led to it (« Hotte de cuisine »).
  const quoted = room.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const left = name
    .replace(new RegExp(`(?:(?<![\\p{L}\\p{N}])\\p{L}{1,3}[\\s\\u2019']+)?(?<![\\p{L}\\p{N}])${quoted}(?![\\p{L}\\p{N}])`, 'iu'), '')
    .replace(/\s+/g, ' ')
    .trim();
  if (!left) return name;
  return left.charAt(0).toUpperCase() + left.slice(1);
}

// ---- a light, a shutter ------------------------------------------------------------

/** What a light is doing, in the percent people see. */
export function lightOf(d) {
  const key = dimKey(d);
  // Its own top: a Z-Wave level to 99, a Hue brightness to 100, a Zigbee one to 254.
  const top = d.points.find((p) => p.key === key)?.kind?.max ?? 100;
  // What was asked shows until the lamp says it took it: it does not fall back
  // to its old state meanwhile.
  const askedLevel = key ? hub.pending[`${d.id}/${key}`] : undefined;
  const askedPower = hub.pending[`${d.id}/${powerKey(d)}`];
  const raw = Number(askedLevel ?? value(d.id, key));
  const on = askedPower != null ? askedPower === true || askedPower === 'ON' : askedLevel != null ? Number(askedLevel) > 0 : isOn(d.id);
  const pct = !on ? 0 : !key ? 100 : Math.max(1, Math.round(shownPercent(d, raw, top))) || 100;
  const cut = unpowered(d.id);
  return { id: d.id, on, pct, key, top, cut, ok: reachable(d.id) || levelKey(d) != null, word: cut ? t('commun.lampe.coupee') : !reachable(d.id) && !levelKey(d) ? t('commun.lampe.injoignable') : on ? (key ? t('commun.pourcent', { n: pct }) : t('commun.lampe.allumee')) : t('commun.lampe.eteinte') };
}

/** Sets a light to a percent: 0 is off. Off, the level lights a dimmer by
 *  itself; a lamp whose switch is apart is switched on too. */
export function setLight(d, pct) {
  const label = nameOf(d.id);
  const l = lightOf(d);
  if (pct <= 0) return setPower(d, false, label);
  if (!l.key) return setPower(d, true, label);
  act(`${d.id}/${l.key}`, levelAt(d, pct, l.top), label);
  if (!l.on && !levelKey(d)) setPower(d, true, label);
}

export function allLightsOff() {
  for (const d of lights()) if (isOn(d.id)) setPower(d, false, nameOf(d.id));
}

/** Where a shutter is: `pct` null when it does not know (it has no position). */
export function coverOf(d) {
  const raw = hub.pending[`${d.id}/percent_control`] ?? value(d.id, 'percent_control');
  const pct = raw == null ? null : Math.max(0, Math.min(100, Math.round(Number(raw))));
  const settable = d.points.some((p) => p.key === 'percent_control' && p.access?.write);
  const offers = d.points.find((p) => p.key === 'control')?.kind?.values ?? [];
  const word = pct == null ? '—' : pct === 0 ? t('maison.accueil.store_ferme') : pct === 100 ? t('maison.accueil.store_ouvert') : t('commun.pourcent', { n: pct });
  return { id: d.id, pct, settable, offers, open: (pct ?? 0) > 0, word };
}

export function coversWord(list = covers()) {
  const open = list.filter((d) => coverOf(d).open).length;
  if (!list.length) return '';
  if (!open) return t('maison.accueil.stores_fermes');
  if (open === list.length) return t('maison.accueil.stores_ouverts');
  return t('maison.accueil.stores_ouverts_sur', { count: open, total: list.length });
}

// ---- a speaker ---------------------------------------------------------------------

export const speakerOf = (d) => {
  const state = value(d.id, 'state');
  const title = value(d.id, 'title');
  return {
    id: d.id,
    playing: value(d.id, 'playing') === true,
    paused: state === 'paused_playback' && !!title,
    title,
    artist: value(d.id, 'artist'),
    art: value(d.id, 'art'),
    volume: Number(hub.pending[`${d.id}/volume`] ?? value(d.id, 'volume') ?? 0),
  };
};

// ---- the words of the house --------------------------------------------------------

const word = (key, fallback) => {
  const w = t(key);
  return w === key ? String(fallback) : w;
};

export const alarmWord = (d) => word(`maison.alarme.etat.${value(d.id, 'state')}`, value(d.id, 'state') ?? t('maison.alarme.inconnu'));
export const alarmArmed = (d) => {
  const m = value(d.id, 'mode');
  return !!m && m !== 'disarmed';
};
export const garageWord = (d) => word(`maison.accueil.garage.${value(d.id, 'state')}`, value(d.id, 'state') ?? '—');
export const garageOpen = (d) => {
  const s = value(d.id, 'state');
  return !!s && s !== 'closed';
};
export const chargerWord = (d) => t(`maison.accueil.borne.${chargerPhase(d.id)}`);

/** The rooms that are outside: their thermometer says nothing of the indoors. */
const OUTSIDE = /exterieur|dehors|jardin|terrasse|outside|outdoor|garden|terrace/i;
const plain = (s) => String(s ?? '').normalize('NFD').replace(/\p{M}/gu, '');

/** The mean of the thermometers indoors (null: none). */
export function indoorTemp() {
  const list = thermos()
    .filter((d) => !isOutdoor(d))
    .map((d) => Number(value(d.id, 'temperature')))
    .filter(Number.isFinite);
  return list.length ? list.reduce((a, b) => a + b, 0) / list.length : null;
}

// ---- what deserves a word ----------------------------------------------------------

/** `tone`: `alert` and `warm` take the sentence, `cool` is only a chip.
 *  Silence when all is well. */
export function notices() {
  const list = [];
  const cfg = home.config ?? {};
  const out = cfg.outdoor ?? {};
  const devices = Object.values(hub.devices);
  for (const d of devices) {
    if (value(d.id, 'smoke') === true) list.push({ tone: 'alert', icon: 'fire', text: t('maison.accueil.fumee', { name: nameOf(d.id) }), href: '#/pieces' });
    if (leak(d.id)) list.push({ tone: 'alert', icon: 'water', text: t('maison.accueil.fuite', { name: nameOf(d.id) }), href: '#/pieces' });
  }
  for (const a of devices.filter(isAlarm)) if (/trigger|alarm/i.test(String(value(a.id, 'state') ?? ''))) list.push({ tone: 'alert', icon: 'shield-lock', text: alarmWord(a), panel: 'security' });
  const bell = (cfg.favorites?.cameras ?? [])[0]?.id;
  if (bell && value(bell, 'doorbell') === true) list.push({ tone: 'alert', icon: 'bell', text: t('maison.accueil.sonnette'), href: '#/cameras' });
  if (bell && value(bell, 'person') === true) list.push({ tone: 'warm', icon: 'account', text: t('maison.accueil.quelquun'), href: '#/cameras' });
  for (const door of cfg.doors ?? []) if (device(door.id) && doorOpen(door.id) === true) list.push({ tone: 'warm', icon: 'door-open', text: t('maison.accueil.porte_ouverte', { name: door.name }), href: '#/pieces' });
  for (const g of garages()) if (value(g.id, 'state') === 'open') list.push({ tone: 'warm', icon: 'gate', text: t('maison.accueil.garage_ouvert', { name: nameOf(g.id) }), panel: 'security' });
  if (hub.approvals?.length) list.push({ tone: 'cool', icon: 'lock', text: t('maison.accueil.demandes', { count: hub.approvals.length }), href: '#/reglages/securite' });
  for (const p of devices.filter(isPrinter)) {
    if (p.online === false) continue;
    const st = phase(value(p.id, 'state'));
    const left = value(p.id, 'remaining');
    const pct = Math.round(Number(value(p.id, 'progress') ?? 0));
    if (st === 'printing') list.push({ tone: 'cool', icon: 'printer3d', href: '#/impression', text: typeof left === 'number' ? t('maison.accueil.impression_fin', { name: nameOf(p.id), pct, time: clock(home.now + left * 60_000) }) : t('maison.accueil.impression', { name: nameOf(p.id), pct }) });
    else if (st === 'paused') list.push({ tone: 'warm', icon: 'printer3d', text: t('maison.accueil.impression_pause', { name: nameOf(p.id), pct }), href: '#/impression' });
    else if (st === 'error') list.push({ tone: 'alert', icon: 'printer3d', text: t('maison.accueil.impression_erreur', { name: nameOf(p.id) }), href: '#/impression' });
  }
  if (value('energie:maison', 'over_budget') === true) list.push({ tone: 'warm', icon: 'bolt', text: t('maison.accueil.facture', { bill: num(value('energie:maison', 'bill_projection')) }), href: '#/energie' });
  // A dead battery takes its sensor offline: count it all the same.
  const flat = devices.filter((d) => lowBattery(d.id));
  if (flat.length) list.push({ tone: 'cool', icon: 'alert', href: '#/pieces', text: flat.length <= 3 ? t('maison.accueil.piles_liste', { count: flat.length, names: flat.map((d) => nameOf(d.id)).join(', ') }) : t('maison.accueil.piles', { count: flat.length }) });
  if (out.pool && value(out.pool, 'action_required') === true) list.push({ tone: 'cool', icon: 'pool', text: t('maison.accueil.piscine_oeil'), href: '#/dehors' });
  return list;
}

/** What is going on, only what is on: nothing to see when nothing happens. */
export function activity() {
  const list = [];
  const seen = new Set();
  for (const d of speakers()) {
    const s = speakerOf(d);
    if (!s.playing) continue;
    // A group plays once: its members say the same.
    const key = `${value(d.id, 'group_leader') ?? d.id}:${s.title}`;
    if (seen.has(key)) continue;
    seen.add(key);
    list.push({ key: d.id, icon: 'speaker', title: s.title ?? t('maison.accueil.musique'), detail: nameOf(d.id), panel: 'music', cover: s.art ? { id: d.id, art: s.art } : null });
  }
  // With an Apple TV, the TV only shows it: one line, the Apple TV's.
  const watched = tvs().some(isAppleTv) ? tvs().filter(isAppleTv) : tvs();
  for (const d of watched) if (value(d.id, 'power') === true) list.push({ key: d.id, icon: 'tv', title: t('maison.accueil.tele_en_marche'), detail: roomOf(d) ?? nameOf(d.id), href: '#/salon' });
  const lit = lights().filter((d) => isOn(d.id));
  if (lit.length) list.push({ key: 'lights', icon: 'light', title: t('maison.accueil.lumieres_allumees', { count: lit.length }), detail: [...new Set(lit.map((d) => roomOf(d)).filter(Boolean))].join(', '), panel: 'lights' });
  for (const d of clims()) if (value(d.id, 'on') === true) list.push({ key: d.id, icon: 'air-conditioner', title: nameOf(d.id), detail: t('maison.accueil.clim_consigne', { v: num(value(d.id, 'target_temperature'), 1) }), panel: 'climate' });
  for (const d of chargers()) if (chargerPhase(d.id) === 'charging') list.push({ key: d.id, icon: 'car', title: t('maison.accueil.voiture_charge'), detail: `${num(Number(value(d.id, 'cur_power') ?? 0) / 1000, 1)} kW`, panel: 'car' });
  return list;
}

// ---- room by room ------------------------------------------------------------------

/** The house room by room: one icon an equipment, lit when it is active. A
 *  room with nothing to show (only helpers, or nothing) does not come.
 *  `everything`: what the home page leaves out too (the Rooms page). */
export function rooms(everything = false) {
  const defs = (home.config?.rooms ?? []).map((r) => r.name);
  const rank = (n) => (defs.includes(n) ? defs.indexOf(n) : defs.length);
  const c = collator();
  return roomList()
    .filter((r) => !layout.hiddenRooms.includes(r.name))
    .map((r) => {
      const parts = inGarage();
      const inRoom = (everything ? Object.values(hub.devices).filter((d) => !hidden(d.id) && !parts.has(d.id)) : all()).filter((d) => roomOf(d) === r.name);
      const gear = [];
      const add = (d, icon, on, bad = false) => gear.push({ id: d.id, icon, on, bad, label: shortName(d, r.name) });
      for (const d of inRoom.filter((x) => deviceKind(x) === 'light')) add(d, 'light', isOn(d.id));
      for (const d of inRoom.filter((x) => deviceKind(x) === 'cover' && !isCoverGroup(x))) add(d, 'shutter', coverOf(d).open);
      for (const d of inRoom.filter((x) => deviceKind(x) === 'climate')) add(d, 'air-conditioner', value(d.id, 'on') === true);
      for (const d of inRoom.filter((x) => deviceKind(x) === 'speaker')) add(d, 'speaker', value(d.id, 'playing') === true);
      for (const d of inRoom.filter((x) => deviceKind(x) === 'tv')) add(d, 'tv', value(d.id, 'power') === true);
      for (const d of inRoom.filter((x) => deviceKind(x) === 'plug' && !isCharger(x))) add(d, 'power', isOn(d.id));
      for (const d of inRoom.filter((x) => deviceKind(x) === 'garage')) add(d, 'gate', garageOpen(d), garageOpen(d));
      for (const d of inRoom.filter(isCharger)) add(d, 'car', chargerPhase(d.id) === 'charging');
      for (const d of inRoom.filter((x) => deviceKind(x) === 'camera')) add(d, 'cctv', false);
      for (const d of inRoom.filter(isAlarm)) add(d, 'shield', alarmArmed(d));
      for (const d of inRoom.filter(isSmoke)) add(d, 'smoke-detector', false, value(d.id, 'smoke') === true);
      const thermo = inRoom.find((d) => isThermo(d) && !Object.values(home.config?.outdoor ?? {}).includes(d.id));
      const clim = inRoom.find((x) => deviceKind(x) === 'climate');
      const temp = Number(value((thermo ?? clim)?.id, 'temperature') ?? NaN);
      return { name: r.name, gear, devices: inRoom, room: r, temp: Number.isFinite(temp) ? temp : null, humidity: thermo ? value(thermo.id, 'humidity') : null, lit: inRoom.some((d) => deviceKind(d) === 'light' && isOn(d.id)) };
    })
    .filter((r) => r.gear.length || (everything && r.devices.length))
    .sort((a, b) => rank(a.name) - rank(b.name) || c.compare(a.name, b.name));
}

// ---- how the home page is shown, on this device ------------------------------------

const VIEW_KEY = 'maison-accueil-vue';

export const accueil = $state({
  view: (() => {
    try {
      return localStorage.getItem(VIEW_KEY) === 'pieces' ? 'pieces' : 'usages';
    } catch {
      return 'usages';
    }
  })(),
});

export function setView(view) {
  accueil.view = view;
  savePref(VIEW_KEY, view);
}
