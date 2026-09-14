import assert from "node:assert/strict";
import test from "node:test";

import type { PoolInfo, TokenState } from "../shared/protocol.ts";
import { gabungkanTokenState } from "../sidepanel/token-state.ts";

function pool(id: string, lengkap: boolean): PoolInfo {
  return {
    id,
    version: "v4",
    token0: "0x0000000000000000000000000000000000000001",
    token1: "0x0000000000000000000000000000000000000002",
    fee: 3000,
    dynamic_fee: false,
    tick_spacing: 60,
    hooks: null,
    liquidity: "100",
    sqrt_price_x96: "1",
    tick: 0,
    raw_price: 1,
    stats: lengkap ? {
      tvl: 100,
      volume: 25,
      apr: 36.5,
      jendela_menit: 180,
    } : null,
  };
}

function state(pools: PoolInfo[], saldo: TokenState["saldo"]): TokenState {
  return {
    address: "0x0000000000000000000000000000000000000001",
    meta: null,
    quotes: [],
    pools,
    selected_quote: "0x0000000000000000000000000000000000000002",
    selected_pool_id: pools[0]?.id ?? null,
    error: null,
    saldo,
  };
}

test("saldo terbaru masuk tanpa menunggu seluruh statistik pool", () => {
  const lama = state([pool("lama", true)], { memecoin: 1, quote: 2, gas: 3 });
  const parsial = state(
    [pool("lama", true), pool("baru", false)],
    { memecoin: 11, quote: 22, gas: 33 },
  );

  const hasil = gabungkanTokenState(lama, parsial);
  assert.equal(hasil?.pools.length, 1, "daftar parsial tidak mengganti snapshot lengkap");
  assert.deepEqual(hasil?.saldo, parsial.saldo, "saldo cepat tetap diterapkan");
});

test("saldo pertama tampil saat daftar pool baru masih disiapkan", () => {
  const parsial = state(
    [pool("lama", true), pool("baru", false)],
    { memecoin: 11, quote: 22, gas: 33 },
  );

  const hasil = gabungkanTokenState(null, parsial);
  assert.deepEqual(hasil?.saldo, parsial.saldo);
  assert.deepEqual(hasil?.pools, [], "pool parsial belum boleh muncul");
  assert.equal(hasil?.selected_pool_id, null);
});

test("snapshot statistik lengkap mempertahankan saldo cepat yang sudah diterima", () => {
  const lama = state([pool("lama", true)], { memecoin: 11, quote: 22, gas: 33 });
  const lengkap = state([pool("lama", true), pool("baru", true)], null);

  const hasil = gabungkanTokenState(lama, lengkap);
  assert.equal(hasil?.pools.length, 2);
  assert.deepEqual(hasil?.saldo, lama.saldo);
});

test("saldo token atau pasangan lain tidak boleh mencemari tampilan aktif", () => {
  const lama = state([pool("lama", true)], { memecoin: 1, quote: 2, gas: 3 });
  const lain = {
    ...state([pool("baru", false)], { memecoin: 99, quote: 99, gas: 99 }),
    address: "0x0000000000000000000000000000000000000009",
  };

  assert.equal(gabungkanTokenState(lama, lain), lama);
});
