/**
 * Uji antrean toast dan pemetaan tingkat ke perilaku.
 *
 * Tidak ada browser di sini, jadi DOM-nya diganti tiruan seadanya. Yang diuji
 * bukan tampilannya melainkan aturannya: berapa yang boleh tampil bersamaan,
 * apa yang terjadi pada sisanya, dan bahwa slot tidak bocor saat sebuah toast
 * ditutup dua kali.
 *
 * Jalankan dengan `npm test`.
 */

import assert from "node:assert/strict";
import { describe, it, beforeEach } from "node:test";

// ── DOM tiruan, dipasang sebelum modul toast dimuat ───────────────
type ElemenTiruan = {
  id: string;
  anak: ElemenTiruan[];
  atribut: Record<string, string>;
  teks: string;
  setAttribute(n: string, v: string): void;
  appendChild(e: ElemenTiruan): ElemenTiruan;
  addEventListener(n: string, f: () => void): void;
  remove(): void;
  klik(): void;
};

const daftar = new Map<string, ElemenTiruan>();
let akar: ElemenTiruan;

function buatElemen(): ElemenTiruan {
  const pendengar: (() => void)[] = [];
  const el: ElemenTiruan = {
    id: "",
    anak: [],
    atribut: {},
    teks: "",
    setAttribute(n, v) {
      el.atribut[n] = v;
    },
    appendChild(anak) {
      el.anak.push(anak);
      if (anak.id) daftar.set(anak.id, anak);
      return anak;
    },
    addEventListener(n, f) {
      if (n === "click") pendengar.push(f);
    },
    remove() {
      if (el.id) daftar.delete(el.id);
      for (const induk of [akar, ...akar.anak]) {
        const i = induk.anak.indexOf(el);
        if (i >= 0) induk.anak.splice(i, 1);
      }
    },
    klik() {
      for (const f of pendengar) f();
    },
  };
  return el;
}

akar = buatElemen();

const g = globalThis as unknown as Record<string, unknown>;
g.document = {
  documentElement: akar,
  createElement: () => buatElemen(),
  createTextNode: (t: string) => ({ teks: t, anak: [], atribut: {} }),
  getElementById: (id: string) => daftar.get(id) ?? null,
};

// Waktu dikendalikan supaya toast yang hilang sendiri bisa diuji tanpa menunggu.
const tertunda: (() => void)[] = [];
g.self = {
  setTimeout: (f: () => void) => {
    tertunda.push(f);
    return tertunda.length;
  },
};

const { tampilkanToast, jumlahTampil, jumlahAntre, bersihkan, MAKS_TAMPIL } = await import(
  "./toast.ts"
);

function isi(judul: string, durasiMs = 6000) {
  return { level: "info" as const, judul, keterangan: "x", durasiMs };
}

describe("antrean toast", () => {
  beforeEach(() => {
    bersihkan();
    tertunda.length = 0;
  });

  it("menampilkan sampai batas lalu mengantrekan sisanya", () => {
    for (let i = 0; i < MAKS_TAMPIL + 3; i += 1) tampilkanToast(isi(`t${i}`));
    assert.equal(jumlahTampil(), MAKS_TAMPIL, "tumpukan lebih tinggi mulai menutupi halaman");
    assert.equal(jumlahAntre(), 3);
  });

  it("mengambil dari antrean saat satu ditutup", () => {
    for (let i = 0; i < MAKS_TAMPIL + 1; i += 1) tampilkanToast(isi(`t${i}`));
    assert.equal(jumlahAntre(), 1);

    // Toast pertama habis waktunya.
    tertunda[0]();
    assert.equal(jumlahAntre(), 0, "yang mengantre harus naik");
    assert.equal(jumlahTampil(), MAKS_TAMPIL);
  });

  it("menutup dua kali tidak membocorkan slot", () => {
    // Toast bisa ditutup oleh klik dan oleh waktu sekaligus. Tanpa penjaga,
    // penghitungnya turun dua kali dan slotnya bocor sampai tidak ada toast
    // yang bisa muncul lagi.
    tampilkanToast(isi("a"));
    assert.equal(jumlahTampil(), 1);
    tertunda[0]();
    tertunda[0]();
    assert.equal(jumlahTampil(), 0, "penghitung tidak boleh turun di bawah nol");

    for (let i = 0; i < MAKS_TAMPIL; i += 1) tampilkanToast(isi(`b${i}`));
    assert.equal(jumlahTampil(), MAKS_TAMPIL, "slot masih penuh tersedia");
  });

  it("toast kritis tidak dijadwalkan hilang sendiri", () => {
    tampilkanToast({ level: "critical", judul: "k", keterangan: "x", durasiMs: 0 });
    assert.equal(tertunda.length, 0, "durasi nol berarti menetap sampai diklik");
  });

  it("bersihkan mengosongkan tampilan dan antrean", () => {
    for (let i = 0; i < MAKS_TAMPIL + 2; i += 1) tampilkanToast(isi(`t${i}`));
    bersihkan();
    assert.equal(jumlahTampil(), 0);
    assert.equal(jumlahAntre(), 0);
  });
});
