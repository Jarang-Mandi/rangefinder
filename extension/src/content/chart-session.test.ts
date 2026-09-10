import assert from "node:assert/strict";
import { test } from "node:test";
import { drawOwnedPair, waitDataReady } from "../shared/chart-session.ts";

test("chart yang tak pernah siap berhenti menunggu, callback terlambat diabaikan", async () => {
  let late = () => {};
  assert.equal(await waitDataReady(cb => { late = cb; return false; }, 5), false);
  late();
  assert.equal(await waitDataReady(() => true, 100), true);
  assert.equal(await waitDataReady(cb => { cb(); return false; }, 100), true);
  assert.equal(await waitDataReady(() => { throw Error("iframe hilang"); }, 100), false);
});

test("garis yang selesai setelah navigasi dibuang dari chart pemiliknya", async () => {
  let current = true;
  let finish!: (id: string) => void;
  const removed: string[] = [];
  const created: string[] = [];
  const drawing = drawOwnedPair(side => {
    created.push(side);
    return new Promise(resolve => { finish = resolve; });
  }, id => removed.push(id), () => current);
  current = false;
  finish("old-upper");
  assert.equal(await drawing, null);
  assert.deepEqual(created, ["upper"]);
  assert.deepEqual(removed, ["old-upper"]);
});

test("kegagalan garis kedua tidak meninggalkan garis pertama", async () => {
  const removed: string[] = [];
  assert.equal(await drawOwnedPair(async side => {
    if (side === "lower") throw Error("iframe dibuang");
    return "upper-owned";
  }, id => removed.push(id), () => true), null);
  assert.deepEqual(removed, ["upper-owned"]);
});

test("dua garis tetap dimiliki ketika sesi dan permintaan masih berlaku", async () => {
  const removed: string[] = [];
  assert.deepEqual(await drawOwnedPair(async side => side, id => removed.push(id), () => true), { upper: "upper", lower: "lower" });
  assert.deepEqual(removed, []);
});
