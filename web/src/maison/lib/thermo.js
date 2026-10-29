// A thermometer's colours and its history by day: shared by the card and its
// long view.

// Cold blue → teal → comfort green → amber → coral, as an oklch hue.
const STOPS = [
  [-5, 262],
  [8, 238],
  [15, 205],
  [19, 170],
  [22, 135],
  [25, 75],
  [28, 45],
  [33, 25],
];

/** The hue of a temperature (°C). */
export function hueOf(deg) {
  if (typeof deg !== 'number') return 240;
  if (deg <= STOPS[0][0]) return STOPS[0][1];
  for (let i = 1; i < STOPS.length; i++) {
    const [t1, h1] = STOPS[i];
    const [t0, h0] = STOPS[i - 1];
    if (deg <= t1) return h0 + ((deg - t0) / (t1 - t0)) * (h1 - h0);
  }
  return STOPS.at(-1)[1];
}

/** The hue of a humidity (%): sand when dry, green when right, blue when damp. */
export function humidityHue(pct) {
  if (typeof pct !== 'number') return 230;
  if (pct < 40) return 70 + (pct / 40) * 60;
  if (pct <= 60) return 150;
  return 150 + Math.min(1, (pct - 60) / 30) * 105;
}

const DAY = 86_400_000;

/** Local midnight, `days` days before the one of `ts`. */
export function midnight(ts, days = 0) {
  const d = new Date(ts);
  d.setHours(0, 0, 0, 0);
  d.setDate(d.getDate() - days);
  return d.getTime();
}

const dayKey = (ts) => {
  const d = new Date(ts);
  return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
};
const monthKey = (ts) => {
  const d = new Date(ts);
  return `${d.getFullYear()}-${d.getMonth()}`;
};

/** Daily buckets (`/api/history` with `step`) → one row per day or month
 *  from `from` to `now`: `{ts, min, max, avg, carried}`. A day without a
 *  reading (the value held) carries the day before; a month without any
 *  reading stays empty (`avg` null). */
export function rows(series, from, now, by = 'day') {
  const byDay = new Map();
  for (const b of series?.buckets ?? []) {
    const k = dayKey(b.ts);
    const had = byDay.get(k);
    if (!had) byDay.set(k, { min: b.min, max: b.max, sum: b.avg * b.count, count: b.count });
    else {
      had.min = Math.min(had.min, b.min);
      had.max = Math.max(had.max, b.max);
      had.sum += b.avg * b.count;
      had.count += b.count;
    }
  }
  let held = typeof series?.before?.[1] === 'number' ? series.before[1] : null;
  const days = [];
  for (let ts = midnight(from); ts <= now; ts = midnight(ts + DAY + 3_600_000)) {
    const d = byDay.get(dayKey(ts));
    if (d) {
      const avg = d.sum / Math.max(1, d.count);
      // The value held from the day before counts in this day too.
      const min = held == null ? d.min : Math.min(d.min, held);
      const max = held == null ? d.max : Math.max(d.max, held);
      days.push({ ts, min, max, avg, carried: false });
      held = avg;
    } else if (held != null) {
      days.push({ ts, min: held, max: held, avg: held, carried: true });
    } else {
      days.push({ ts, min: null, max: null, avg: null, carried: true });
    }
  }
  if (by === 'day') return days;
  // Every month of the span, an empty one too (a gap says « no reading »).
  const months = new Map();
  for (const d of days) {
    const k = monthKey(d.ts);
    if (!months.has(k)) months.set(k, { ts: d.ts, min: null, max: null, sum: 0, n: 0, real: false });
    if (d.avg == null) continue;
    const m = months.get(k);
    if (!m.n) Object.assign(m, { min: d.min, max: d.max, sum: d.avg, n: 1, real: !d.carried });
    else {
      m.min = Math.min(m.min, d.min);
      m.max = Math.max(m.max, d.max);
      m.sum += d.avg;
      m.n += 1;
      m.real ||= !d.carried;
    }
  }
  return [...months.values()].map((m) => ({ ts: m.ts, min: m.min, max: m.max, avg: m.n ? m.sum / m.n : null, carried: !m.real }));
}

/** Round ticks covering [lo, hi]. */
export function ticks(lo, hi, count = 4) {
  const span = Math.max(hi - lo, 1);
  const raw = span / count;
  const mag = 10 ** Math.floor(Math.log10(raw));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((s) => s >= raw) ?? 10 * mag;
  const out = [];
  for (let v = Math.floor(lo / step) * step; v <= hi + step / 2; v += step) out.push(Math.round(v * 100) / 100);
  return out;
}
