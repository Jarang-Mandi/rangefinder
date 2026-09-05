/**
 * Uji logika jembatan chart terhadap snapshot struktur gmgn.
 *
 * Ini gerbang sebelum fase 3 menurut PRD 14: fase 2 wajib punya uji integrasi
 * terhadap snapshot gmgn supaya regresi ketahuan tanpa membuka browser.
 *
 * Jalankan dengan `npm test`.
 */

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { describe, it } from "node:test";

import {
  IFRAME_SELECTOR,
  REQUIRED_CHART_METHODS,
  capabilityTier,
  methodsPresent,
  parseBar,
  readLastBar,
  sideOfShape,
  tokenFromPath,
} from "./chart-logic.ts";

const here = dirname(fileURLToPath(import.meta.url));
const snapshot = readFileSync(
  resolve(here, "../../fixtures/gmgn-token-page.html"),
  "utf8",
);

describe("bentuk URL gmgn", () => {
  it("mengenali halaman token", () => {
    assert.equal(
      tokenFromPath("/robinhood/token/0x39dbed3a2bd333467115de45665cc57f813c4571"),
      "0x39dbed3a2bd333467115de45665cc57f813c4571",
    );
  });

  it("bekerja untuk chain lain juga", () => {
    // Slug sengaja tidak dibatasi, supaya tidak rusak saat gmgn menambah chain.
    assert.ok(tokenFromPath("/base/token/0x39dbed3a2bd333467115de45665cc57f813c4571"));
    assert.ok(tokenFromPath("/sol/token/So11111111111111111111111111111111111111112"));
  });

  it("menolak halaman yang bukan halaman token", () => {
    assert.equal(tokenFromPath("/trend"), null);
    assert.equal(tokenFromPath("/robinhood/token/"), null);
    assert.equal(tokenFromPath("/robinhood/token/bukan-alamat"), null);
    // Alamat EVM kurang satu karakter harus ditolak.
    //
    // Ini pernah lolos: alternatif Solana yang memakai kelas karakter longgar
    // menelan seluruh string termasuk awalan 0x, sehingga alamat cacat diterima
    // seolah alamat Solana. Abjad base58 yang benar tidak memuat angka nol,
    // jadi awalan 0x tidak mungkin cocok lagi.
    assert.equal(tokenFromPath("/robinhood/token/0x39dbed3a2bd333467115de45665cc57f813c457"), null);
    // Kelebihan satu karakter juga harus ditolak.
    assert.equal(
      tokenFromPath("/robinhood/token/0x39dbed3a2bd333467115de45665cc57f813c45711"),
      null,
    );
  });

  it("alamat Solana memakai abjad base58", () => {
    // Karakter yang tidak ada di base58 harus menggagalkan pencocokan.
    assert.equal(tokenFromPath("/sol/token/0OIl1111111111111111111111111111111111112"), null);
  });
});

describe("snapshot halaman gmgn", () => {
  it("memuat iframe chart dengan pola id yang kita andalkan", () => {
    // Selector ini yang dipakai jembatan chart untuk menemukan chart. Kalau
    // gmgn mengubah pola id-nya, uji ini yang gagal lebih dulu.
    const pola = /<iframe[^>]*id="(tradingview_[^"]+)"/;
    const cocok = snapshot.match(pola);
    assert.ok(cocok, `snapshot harus memuat iframe dengan id berpola tradingview_`);
    assert.ok(IFRAME_SELECTOR.includes("tradingview_"));
  });

  it("memuat tautan token yang bisa diurai", () => {
    const cocok = snapshot.match(/\/robinhood\/token\/(0x[a-fA-F0-9]{40})/);
    assert.ok(cocok, "snapshot harus memuat tautan token robinhood");
    assert.equal(tokenFromPath(`/robinhood/token/${cocok![1]}`), cocok![1]);
  });
});

describe("tingkat degradasi", () => {
  const chartLengkap = Object.fromEntries(
    REQUIRED_CHART_METHODS.map((m) => [m, () => {}]),
  );

  it("tingkat 1 saat semua method ada", () => {
    const present = methodsPresent(chartLengkap);
    assert.equal(present.length, REQUIRED_CHART_METHODS.length);
    assert.equal(capabilityTier(present, true), 1);
  });

  it("tingkat 2 saat sebagian method hilang", () => {
    const { createShape: _hapus, ...sebagian } = chartLengkap;
    const present = methodsPresent(sebagian);
    assert.ok(present.length < REQUIRED_CHART_METHODS.length);
    assert.equal(capabilityTier(present, true), 2);
  });

  it("tingkat 3 saat chart tidak ada sama sekali", () => {
    assert.equal(capabilityTier([], false), 3);
    assert.deepEqual(methodsPresent(null), []);
    assert.deepEqual(methodsPresent("bukan objek"), []);
  });
});

describe("membaca lilin terakhir", () => {
  // Nilai nyata dari PONS, diambil langsung dari chart gmgn pada
  // 5 September 2026. Close 937.234.340 cocok dengan judul halaman $938.86M.
  const nyata = [1788627540, 938203830, 946099140, 933206500, 937234340, 49072.95724354];

  it("mengurai nilai nyata dari gmgn", () => {
    const bar = parseBar(nyata);
    assert.ok(bar);
    assert.equal(bar!.close, 937234340);
    assert.equal(bar!.high, 946099140);
    assert.equal(bar!.low, 933206500);
    assert.equal(bar!.time, 1788627540);
  });

  it("menolak nilai yang tidak konsisten sebagai OHLC", () => {
    // Ini yang menangkap perubahan urutan indeks di versi Charting Library
    // berikutnya. High yang lebih kecil dari close mustahil pada OHLC sejati.
    assert.equal(parseBar([1788627540, 100, 90, 80, 95, 1]), null);
    // Low yang lebih besar dari open juga mustahil.
    assert.equal(parseBar([1788627540, 100, 120, 110, 105, 1]), null);
  });

  it("menolak bentuk yang salah", () => {
    assert.equal(parseBar(null), null);
    assert.equal(parseBar([]), null);
    assert.equal(parseBar([1, 2, 3]), null);
    assert.equal(parseBar([1788627540, 100, 120, 80, 0, 1]), null, "close nol tidak sah");
    assert.equal(parseBar([1788627540, "a", 120, 80, 95, 1]), null);
    assert.equal(parseBar([1788627540, 100, 120, 80, NaN, 1]), null);
  });

  it("volume yang hilang tidak menggagalkan pembacaan", () => {
    const bar = parseBar([1788627540, 100, 120, 80, 95]);
    assert.ok(bar);
    assert.equal(bar!.volume, 0);
  });

  it("menelusuri rantai objek chart yang sebenarnya", () => {
    const chart = {
      getSeries: () => ({ data: () => ({ m_bars: { last: () => ({ value: nyata }) } }) }),
    };
    const bar = readLastBar(chart);
    assert.ok(bar);
    assert.equal(bar!.close, 937234340);
  });

  it("rantai objek yang putus menghasilkan null, bukan lemparan", () => {
    assert.equal(readLastBar(null), null);
    assert.equal(readLastBar({}), null);
    assert.equal(readLastBar({ getSeries: () => ({}) }), null);
    assert.equal(readLastBar({ getSeries: () => ({ data: () => ({}) }) }), null);
  });
});

describe("mengenali garis milik sendiri", () => {
  const lines = { upper: "jnm56k", lower: "OOyO39" };

  it("mencocokkan id yang tercatat", () => {
    assert.equal(sideOfShape("jnm56k", lines), "upper");
    assert.equal(sideOfShape("OOyO39", lines), "lower");
  });

  it("mengabaikan gambar milik user", () => {
    // Ini yang mencegah penghapusan gambar user. Entity id acak, jadi id asing
    // harus dikembalikan sebagai null, bukan ditebak.
    assert.equal(sideOfShape("BFjeJJ", lines), null);
    assert.equal(sideOfShape("", lines), null);
    assert.equal(sideOfShape("jnm56k", {}), null);
  });
});
