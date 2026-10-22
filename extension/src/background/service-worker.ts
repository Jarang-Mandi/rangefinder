/**
 * Service worker MV3.
 *
 * Tugasnya tipis dengan sengaja: membuka side panel, dan meneruskan pesan
 * antara content script dan panel. Semua kecerdasan ada di engine Rust, karena
 * service worker bisa dimatikan Chrome kapan saja dan tidak boleh memegang
 * state yang penting.
 *
 * INVARIAN
 * - Jangan pernah menyimpan token pairing di sini. Tempatnya
 *   `chrome.storage.local`, dibaca panel.
 * - Jangan memanggil engine dari sini. Panel yang memegang koneksi, karena
 *   panel yang punya siklus hidup jelas.
 */

import { chartMessage, robinhoodToken } from "../shared/chart-messages";

chrome.runtime.onInstalled.addListener(() => {
  // Klik ikon ekstensi membuka side panel, bukan popup.
  chrome.sidePanel
    .setPanelBehavior({ openPanelOnActionClick: true })
    .catch((e) => console.warn("[rangefinder] gagal mengatur side panel", e));
});

/**
 * Teruskan kabar dari content script ke panel yang sedang terbuka.
 *
 * Panel mungkin tertutup, dan itu normal. Kegagalan pengiriman diabaikan.
 */
chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  const m = message as Record<string, unknown>;

  if (m.source === "gmgn-chart") {
    if (sender.id !== chrome.runtime.id || sender.frameId !== 0 || sender.tab?.id === undefined) return false;
    const payload = chartMessage(m, sender.url ?? "");
    if (!payload || (payload.kind === "token_page" && !sender.tab.active)) return false;
    chrome.runtime
      .sendMessage({ ...payload, source: "gmgn-chart", target: "sidepanel", tabId: sender.tab.id })
      .catch(() => {});
    return false;
  }

  // Panel meminta navigasi ke halaman token tertentu. Lihat PRD 3.3.
  if (m.target === "navigate-gmgn" && typeof m.url === "string") {
    if (sender.tab || sender.id !== chrome.runtime.id || !robinhoodToken(m.url)) return false;
    void navigateToToken(m.url).then((tabId) => sendResponse({ tabId }));
    return true;
  }

  return false;
});

/**
 * Arahkan tab gmgn yang sudah terbuka ke alamat token, atau buka tab baru
 * kalau belum ada satu pun.
 *
 * Memakai ulang tab yang ada penting supaya user tidak menumpuk puluhan tab
 * gmgn sepanjang hari.
 */
async function navigateToToken(url: string): Promise<number | undefined> {
  const tabs = await chrome.tabs.query({ url: "https://gmgn.ai/*" });

  const existing = tabs.find((t) => t.id !== undefined);
  if (existing?.id !== undefined) {
    await chrome.tabs.update(existing.id, { url, active: true });
    return existing.id;
  }

  const created = await chrome.tabs.create({ url });
  return created.id;
}

/**
 * Beri tahu panel saat user sendiri membuka halaman token, tanpa lewat panel.
 * Ini yang membuat garis muncul otomatis di setiap pair yang dibuka.
 */
chrome.tabs.onUpdated.addListener((tabId, changeInfo, tab) => {
  if (changeInfo.status !== "complete") return;
  if (!tab.active || !tab.url || !robinhoodToken(tab.url)) return;

  chrome.runtime
    .sendMessage({ target: "sidepanel", kind: "token_page", url: tab.url, tabId })
    .catch(() => {});
});

chrome.tabs.onActivated.addListener(({ tabId }) => {
  void chrome.tabs.get(tabId).then(tab => {
    if (!tab.url || !robinhoodToken(tab.url)) return;
    return chrome.runtime.sendMessage({ target: "sidepanel", kind: "token_page", url: tab.url, tabId });
  }).catch(() => {});
});
