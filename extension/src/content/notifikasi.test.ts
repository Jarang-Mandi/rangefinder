import assert from "node:assert/strict";
import test from "node:test";
import { judulUntuk } from "./notifikasi-label.ts";
import type { Kejadian } from "../shared/protocol.ts";

function kejadian(code: string): Kejadian {
  return {
    level: "info",
    code,
    message: "uji",
    subject: "posisi-1",
    at: "2026-09-11T00:00:00Z",
    ditahan: false,
  };
}

test("setiap aksi utama mempunyai judul notifikasi yang jelas", () => {
  const expected: Record<string, string> = {
    open_started: "Membuka posisi",
    open_succeeded: "Posisi dibuka",
    open_pending: "Pembukaan sedang diproses",
    open_failed: "Gagal membuka posisi",
    open_unverified: "Posisi belum terverifikasi",
    increase_started: "Menambah likuiditas",
    increase_succeeded: "Likuiditas ditambahkan",
    increase_pending: "Tambah likuiditas diproses",
    increase_failed: "Tambah likuiditas gagal",
    rebalance_started: "Rebalance dimulai",
    rebalance_succeeded: "Rebalance selesai",
    rebalance_pending: "Rebalance diproses",
    rebalance_failed: "Rebalance gagal",
    claim_started: "Mengklaim fee",
    claim_succeeded: "Fee diklaim",
    claim_pending: "Klaim fee diproses",
    claim_failed: "Klaim fee gagal",
    close_started: "Menutup posisi",
    close_succeeded: "Posisi ditutup",
    close_pending: "Penutupan sedang diproses",
    close_failed: "Penutupan posisi gagal",
  };
  for (const [code, title] of Object.entries(expected)) {
    assert.equal(judulUntuk(kejadian(code)), title);
  }
});
