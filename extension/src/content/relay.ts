/**
 * Penerus pesan, berjalan di isolated world halaman gmgn.
 *
 * KENAPA ADA
 * `chart-bridge.ts` berjalan di MAIN world supaya bisa melihat `tradingViewApi`,
 * dan MAIN world tidak punya akses ke `chrome.runtime`. Berkas ini yang
 * menyambungkan keduanya.
 *
 * INVARIAN
 * - Hanya pesan dengan `channel: "rangefinder"` yang diteruskan. Halaman gmgn
 *   memakai postMessage untuk keperluannya sendiri, dan pesan dari halaman
 *   adalah data, bukan perintah.
 * - Arah pesan dibedakan eksplisit lewat field `direction`, supaya pesan yang
 *   kita kirim ke halaman tidak terpantul balik dan menjadi lingkaran.
 */

import { mulaiNotifikasi } from "./notifikasi";

const CHANNEL = "rangefinder";

window.addEventListener("message", (event) => {
  if (event.source !== window) return;
  const data = event.data as Record<string, unknown> | null;
  if (!data || data.channel !== CHANNEL || data.direction !== "from-page") return;

  const { channel: _c, direction: _d, ...payload } = data;
  chrome.runtime.sendMessage({ source: "gmgn-chart", ...payload }).catch(() => {
    // Service worker sedang tidur atau panel tertutup. Bukan kondisi error.
  });
});

chrome.runtime.onMessage.addListener((message: unknown) => {
  const m = message as Record<string, unknown> | null;
  if (!m || m.target !== "gmgn-chart") return;

  const { target: _t, ...payload } = m;
  window.postMessage({ channel: CHANNEL, direction: "to-page", ...payload }, "*");
});

// Notifikasi menyambung sendiri ke engine, tidak lewat panel. Alasannya ada di
// notifikasi.ts: panel yang tertutup adalah keadaan paling umum, dan justru saat
// itulah kabar penting harus tetap sampai.
mulaiNotifikasi();

// Menjadikan berkas ini modul, bukan skrip global, supaya konstanta di dalamnya
// tidak bertabrakan dengan berkas content script lain saat pemeriksaan tipe.
export {};
