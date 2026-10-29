import { test } from 'node:test';
import assert from 'node:assert/strict';
import { hueOf, rows, ticks, midnight } from './thermo.js';

test('cold is blue, comfort green, hot coral', () => {
  assert.equal(hueOf(-10), 262);
  assert.ok(hueOf(21) > 120 && hueOf(21) < 170);
  assert.equal(hueOf(40), 25);
});

test('days without a reading carry the one before', () => {
  const day0 = midnight(Date.UTC(2026, 9, 20, 12));
  const series = {
    before: [day0 - 1000, 18],
    buckets: [
      { ts: day0 + 3_600_000, avg: 20, min: 19, max: 21, count: 4 },
      { ts: midnight(day0 + 2.5 * 86_400_000) + 3_600_000, avg: 22, min: 21, max: 23, count: 2 },
    ],
  };
  const r = rows(series, day0, day0 + 2.5 * 86_400_000);
  assert.equal(r.length, 3);
  assert.deepEqual([r[0].min, r[0].max, r[0].carried], [18, 21, false]);
  assert.deepEqual([r[1].min, r[1].max, r[1].carried], [20, 20, true]);
  assert.equal(r[2].max, 23);
  const months = rows(series, day0, day0 + 2.5 * 86_400_000, 'month');
  assert.equal(months.length, 1);
  assert.equal(months[0].min, 18);
  // A year: every month, the empty ones without values.
  const year = rows({ buckets: series.buckets }, midnight(day0, 300), day0 + 2.5 * 86_400_000, 'month');
  assert.ok(year.length >= 10);
  assert.equal(year[0].avg, null);
  assert.equal(year.at(-1).max, 23);
});

test('ticks are round', () => {
  assert.deepEqual(ticks(12.3, 23.8), [10, 15, 20, 25]);
  assert.deepEqual(ticks(40, 60), [40, 45, 50, 55, 60]);
});
