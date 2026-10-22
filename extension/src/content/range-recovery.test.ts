import assert from "node:assert/strict";
import test from "node:test";
import { perluGambarUlang } from "../shared/range-recovery.ts";

test("reload token yang sama menggambar ulang range yang tersimpan", () => {
  const view = { pool_id: "pool" } as never;
  assert.equal(perluGambarUlang("0xAbC", "0xabc", view), true);
});

test("navigasi token baru tidak memakai range token lama", () => {
  const view = { pool_id: "pool" } as never;
  assert.equal(perluGambarUlang("0xabc", "0xdef", view), false);
  assert.equal(perluGambarUlang("0xabc", "0xabc", null), false);
});
