import assert from "node:assert/strict";
import { test } from "node:test";
import { mergeBands, priceAt, priceY, zoomDomain } from "../shared/histogram.ts";
test("sumbu harga dan gerakan kursor saling membalik", () => {
  const domain = { low: 0.000001, high: 0.000004 };
  for (const y of [0, .1, .5, .9, 1]) assert.ok(Math.abs(priceY(priceAt(y, domain), domain) - y) < 1e-10);
  assert.equal(priceY(100, domain), 0);
  assert.equal(priceY(1e-10, domain), 1);
});
test("zoom tidak membuat jendela melewati data yang tersedia", () => {
  for (const current of [1, 1.5, 2]) {
    const d = zoomDomain({ low: 1, high: 2 }, current, 8);
    assert.ok(d.low >= 1 && d.high <= 2 + 1e-12);
  }
});
test("penggabungan 200 pita mempertahankan modal dan maksimal 24 batang", () => {
  const pita = Array.from({ length: 200 }, (_, i) => ({ tick_bawah: i, tick_atas: i + 1, harga_bawah: 1 + i / 1000,
    harga_atas: 1 + (i + 1) / 1000, liquidity: "1", nilai_quote: i + 1, aktif: i === 99 }));
  const merged = mergeBands(pita);
  assert.ok(merged.length <= 24);
  assert.equal(merged.reduce((a, b) => a + b.nilai_quote, 0), 20100);
  assert.equal(merged.filter(p => p.aktif).length, 1);
});
