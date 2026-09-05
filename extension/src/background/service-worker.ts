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

const GMGN_TOKEN_URL = /^https:\/\/gmgn\.ai\/[a-z0-9]+\/token\//;

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
    chrome.runtime
      .sendMessage({ target: "sidepanel", tabId: sender.tab?.id, ...m })
      .catch(() => {});
    return false;
  }

  // Panel meminta navigasi ke halaman token tertentu. Lihat PRD 3.3.
  if (m.target === "navigate-gmgn" && typeof m.url === "string") {
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
  if (!tab.url || !GMGN_TOKEN_URL.test(tab.url)) return;

  chrome.runtime
    .sendMessage({ target: "sidepanel", kind: "token_page", url: tab.url, tabId })
    .catch(() => {});
});
