/**
 * Jembatan chart, berjalan di MAIN world halaman gmgn.
 *
 * KENAPA MAIN WORLD
 * `tradingViewApi` adalah variabel milik halaman. Content script di isolated
 * world bisa melihat DOM iframe yang same origin, tapi TIDAK bisa melihat
 * variabel JavaScript halaman. Karena itu manifest memakai `world: "MAIN"`.
 *
 * KONSEKUENSINYA
 * MAIN world tidak punya akses ke `chrome.runtime`. Semua komunikasi keluar
 * lewat `window.postMessage`, dan `relay.ts` di isolated world yang
 * meneruskannya ke service worker.
 *
 * INVARIAN
 * - `createShape` mengembalikan Promise. Tanpa `await`, `getShapeById` akan
 *   melempar "There is no such shape". Ini sudah pernah menjebak, lihat
 *   memory/002-tradingview-gmgn.md.
 * - Hanya hapus entity id milik sendiri. `getAllShapes()` memuat gambar user,
 *   dan menghapusnya tidak bisa dibatalkan dari sisi kita.
 * - gmgn adalah Next.js satu halaman. Berpindah token tidak memuat ulang
 *   dokumen, jadi navigasi dideteksi lewat History API, bukan lewat `load`.
 */

import {
  IFRAME_SELECTOR,
  REQUIRED_CHART_METHODS,
  capabilityTier,
  methodsPresent,
  readAllBars,
  readLastBar,
  sideOfShape,
  tokenFromPath,
} from "./chart-logic";

const CHANNEL = "rangefinder";
const READY_TIMEOUT_MS = 15_000;
const POLL_MS = 50;

const COLOR_UPPER = "#2E9E63";
const COLOR_LOWER = "#D8484D";

type Side = "upper" | "lower";

interface ChartApi {
  symbol(): string;
  resolution(): string;
  getSeries(): unknown;
  dataReady(cb: () => void): boolean;
  getVisibleRange(): { from: number; to: number };
  getVisiblePriceRange(): { from: number; to: number };
  createShape(point: { time: number; price: number }, options: unknown): Promise<string>;
  getShapeById(id: string): {
    getPoints(): { time: number; price: number }[];
    setPoints(points: { time: number; price: number }[]): void;
  };
  getAllShapes(): { id: string; name: string }[];
  removeEntity(id: string): void;
}

interface TradingViewApi {
  activeChart(): ChartApi;
  subscribe(event: string, handler: (id: string, type: string) => void): void;
  unsubscribe(event: string, handler: (id: string, type: string) => void): void;
}

/** Entity id garis milik kita. Jangan pernah menyimpulkan dari getAllShapes. */
const lines: Partial<Record<Side, string>> = {};
let api: TradingViewApi | null = null;
let currentToken: string | null = null;

function post(payload: Record<string, unknown>) {
  window.postMessage({ channel: CHANNEL, direction: "from-page", ...payload }, "*");
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Alamat token dari URL. `null` kalau halaman ini bukan halaman token. */
function tokenFromUrl(): string | null {
  return tokenFromPath(location.pathname);
}

async function waitForApi(): Promise<TradingViewApi | null> {
  const deadline = Date.now() + READY_TIMEOUT_MS;

  while (Date.now() < deadline) {
    const frame = document.querySelector<HTMLIFrameElement>(IFRAME_SELECTOR);
    const candidate = frame?.contentWindow as
      | (Window & { tradingViewApi?: TradingViewApi })
      | undefined;

    if (candidate?.tradingViewApi) {
      const found = candidate.tradingViewApi;
      try {
        // dataReady memanggil balik segera kalau data sudah ada.
        await new Promise<void>((resolve) => {
          const settled = found.activeChart().dataReady(() => resolve());
          if (settled) resolve();
        });
      } catch {
        // Chart ada tapi belum siap penuh. Tetap lanjut, probe akan melaporkan
        // method mana yang hilang.
      }
      return found;
    }
    await sleep(POLL_MS);
  }
  return null;
}

/**
 * Laporkan method apa saja yang benar benar ada.
 *
 * Ini yang menentukan tingkat degradasi di PRD 4.4. Kalau gmgn mengganti versi
 * Charting Library, panel harus menampilkan mode yang aktif, bukan gagal diam
 * diam.
 */
function probe(chart: ChartApi | null) {
  const present = methodsPresent(chart);
  const tier = capabilityTier(present, chart !== null);

  post({
    kind: "chart_capabilities",
    tier,
    methodsPresent: present,
    methodsMissing: REQUIRED_CHART_METHODS.filter((m) => !present.includes(m)),
    symbol: safe(() => chart?.symbol()),
    resolution: safe(() => chart?.resolution()),
    token: currentToken,
  });
}

function safe<T>(fn: () => T): T | null {
  try {
    return fn();
  } catch {
    return null;
  }
}

async function drawLines(upperPrice: number, lowerPrice: number) {
  if (!api) return;
  const chart = api.activeChart();
  clearLines();

  const time = chart.getVisibleRange().to;
  lines.upper = await chart.createShape(
    { time, price: upperPrice },
    {
      shape: "horizontal_line",
      disableSave: true,
      overrides: {
        linecolor: COLOR_UPPER,
        textcolor: COLOR_UPPER,
        linewidth: 2,
        showPrice: true,
      },
    },
  );
  lines.lower = await chart.createShape(
    { time, price: lowerPrice },
    {
      shape: "horizontal_line",
      disableSave: true,
      overrides: {
        linecolor: COLOR_LOWER,
        textcolor: COLOR_LOWER,
        linewidth: 2,
        showPrice: true,
      },
    },
  );

  post({ kind: "lines_drawn", upperPrice, lowerPrice });
}

function clearLines() {
  if (!api) return;
  const chart = api.activeChart();
  for (const side of ["upper", "lower"] as Side[]) {
    const id = lines[side];
    if (!id) continue;
    // Hanya id milik kita. Lihat invarian di kepala berkas.
    try {
      chart.removeEntity(id);
    } catch {
      // Shape mungkin sudah dihapus user. Itu bukan error.
    }
    delete lines[side];
  }
}

function sideOf(id: string): Side | null {
  return sideOfShape(id, lines);
}

function onDrawingEvent(id: string, type: string) {
  const side = sideOf(id);
  if (!side || !api) return;

  if (type === "remove") {
    delete lines[side];
    post({ kind: "line_removed", side });
    return;
  }

  const price = safe(() => api!.activeChart().getShapeById(id).getPoints()[0].price);
  if (price !== null && price !== undefined) {
    post({ kind: "chart_line_moved", side, price });
  }
}

async function attach() {
  const token = tokenFromUrl();
  currentToken = token;

  if (!token) {
    probe(null);
    return;
  }

  api = await waitForApi();
  if (!api) {
    probe(null);
    return;
  }

  probe(api.activeChart());

  try {
    api.subscribe("drawing_event", onDrawingEvent);
  } catch {
    post({ kind: "chart_capabilities", tier: 2, methodsPresent: [], note: "drawing_event tidak tersedia" });
  }

  post({ kind: "token_page", token, url: location.href });
  startPriceReporting();
}

/** Timer pelaporan harga. Disimpan supaya bisa dihentikan saat pindah token. */
let priceTimer: number | null = null;

/**
 * Laporkan lilin terakhir secara berkala.
 *
 * Satu detik cukup: chart 1 menit tidak berubah lebih cepat dari itu, dan
 * pelaporan yang lebih sering hanya membebani jalur pesan tanpa menambah
 * ketelitian.
 */
function startPriceReporting() {
  stopPriceReporting();
  const kirim = () => {
    if (!api) return;
    const bar = readLastBar(api.activeChart());
    if (bar) {
      post({ kind: "chart_price", token: currentToken, symbol: safe(() => api!.activeChart().symbol()), bar });
    } else {
      post({ kind: "chart_price_unavailable", token: currentToken });
    }
  };
  kirim();
  priceTimer = window.setInterval(kirim, 1_000);

  // Deret penuh dikirim jauh lebih jarang. Isinya ratusan bar, dan detektor
  // sinyal tidak berubah jawabannya dalam hitungan detik. Mengirimnya tiap
  // detik hanya membebani jalur pesan tanpa menambah apa pun.
  const kirimDeret = () => {
    if (!api) return;
    const bars = readAllBars(api.activeChart());
    if (bars.length > 0) {
      post({ kind: "chart_bars", token: currentToken, bars });
    }
  };
  kirimDeret();
  barsTimer = window.setInterval(kirimDeret, 30_000);
}

let barsTimer: number | null = null;

function stopPriceReporting() {
  if (barsTimer !== null) {
    window.clearInterval(barsTimer);
    barsTimer = null;
  }
  if (priceTimer !== null) {
    window.clearInterval(priceTimer);
    priceTimer = null;
  }
}

/**
 * gmgn tidak memuat ulang dokumen saat berpindah token, jadi History API yang
 * dipantau. Tanpa ini, garis hanya pernah muncul di token pertama.
 */
function watchNavigation() {
  let last = location.href;

  const check = () => {
    if (location.href === last) return;
    last = location.href;
    stopPriceReporting();
    clearLines();
    api = null;
    void attach();
  };

  for (const method of ["pushState", "replaceState"] as const) {
    const original = history[method];
    history[method] = function (this: History, ...args: Parameters<typeof original>) {
      const result = original.apply(this, args);
      queueMicrotask(check);
      return result;
    } as typeof original;
  }
  window.addEventListener("popstate", check);
  new MutationObserver(check).observe(document.body, { childList: true, subtree: false });
}

window.addEventListener("message", (event) => {
  if (event.source !== window) return;
  const data = event.data as Record<string, unknown> | null;
  if (!data || data.channel !== CHANNEL || data.direction !== "to-page") return;

  switch (data.kind) {
    case "draw_lines":
      void drawLines(Number(data.upperPrice), Number(data.lowerPrice));
      break;
    case "clear_lines":
      clearLines();
      break;
    case "probe":
      probe(api ? api.activeChart() : null);
      break;
  }
});

watchNavigation();
void attach();
