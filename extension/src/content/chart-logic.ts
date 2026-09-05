/**
 * Logika murni jembatan chart, dipisah supaya bisa diuji tanpa browser.
 *
 * KENAPA DIPISAH
 * `chart-bridge.ts` menjalankan efek samping saat dimuat, yaitu memasang
 * pengamat navigasi dan menyambung ke chart. Berkas ini tidak menyentuh apa
 * pun, sehingga bisa diuji langsung dengan `node --test`.
 *
 * Yang diuji di sini adalah hal hal yang paling mungkin rusak diam diam saat
 * gmgn memperbarui bundle-nya: bentuk URL, jalur akses lilin terakhir, dan
 * penentuan tingkat degradasi.
 */

/** Method yang wajib ada supaya garis bisa digambar dan dibaca. */
export const REQUIRED_CHART_METHODS = [
  "createShape",
  "getShapeById",
  "getAllShapes",
  "removeEntity",
  "getVisiblePriceRange",
  "getVisibleRange",
] as const;

export const IFRAME_SELECTOR = 'iframe[id^="tradingview_"]';

/** Satu lilin, dalam satuan yang sedang dipakai chart. */
export interface LastBar {
  time: number;
  open: number;
  high: number;
  low: number;
  close: number;
  volume: number;
}

/**
 * Alamat token dari sebuah path gmgn.
 *
 * Mengembalikan `null` untuk halaman yang bukan halaman token. Slug chain
 * sengaja tidak dibatasi ke `robinhood` saja, supaya pengenalannya tidak rusak
 * kalau gmgn menambah chain, dan penyaringan chain dikerjakan di tempat lain.
 */
export function tokenFromPath(pathname: string): string | null {
  // Alternatif kedua memakai abjad base58 Solana, yaitu tanpa 0, O, I, dan l.
  //
  // Ini bukan sekadar ketelitian. Kelas karakter yang longgar seperti
  // `[A-Za-z0-9]{32,44}` akan menelan alamat EVM yang cacat, misalnya yang
  // kurang satu karakter, dan meloloskannya seolah alamat Solana yang sah.
  // Uji `menolak halaman yang bukan halaman token` menangkap persis kasus itu.
  const m = pathname.match(
    /^\/([a-z0-9]+)\/token\/(0x[a-fA-F0-9]{40}|[1-9A-HJ-NP-Za-km-z]{32,44})$/,
  );
  return m ? m[2] : null;
}

/**
 * Tingkat degradasi berdasarkan method yang benar benar ada.
 *
 * Tingkat 1 normal, tingkat 2 sebagian API hilang, tingkat 3 tidak ada chart
 * sama sekali. Lihat PRD 4.4.
 */
export function capabilityTier(present: readonly string[], chartAda: boolean): 1 | 2 | 3 {
  if (!chartAda) return 3;
  return present.length === REQUIRED_CHART_METHODS.length ? 1 : 2;
}

export function methodsPresent(chart: unknown): string[] {
  if (!chart || typeof chart !== "object") return [];
  const c = chart as Record<string, unknown>;
  return REQUIRED_CHART_METHODS.filter((m) => typeof c[m] === "function");
}

/**
 * Ambil lilin terakhir dari penyimpanan internal Charting Library.
 *
 * Jalurnya `getSeries().data().m_bars.last().value`, dan bentuk nilainya
 * `[waktu, open, high, low, close, volume, ...]`. Ini struktur internal, bukan
 * API publik, jadi bisa berubah kapan saja.
 *
 * Karena nilainya array, arti tiap posisi ditentukan urutannya. Kalau urutan
 * itu berubah, angkanya tidak akan lagi konsisten sebagai OHLC, dan lebih baik
 * melaporkan tidak tahu daripada menempatkan garis di tempat yang salah.
 */
export function readLastBar(chart: unknown): LastBar | null {
  try {
    const c = chart as { getSeries(): { data(): { m_bars: { last(): { value: number[] } } } } };
    const value = c.getSeries().data().m_bars.last().value;
    return parseBar(value);
  } catch {
    return null;
  }
}

/** Bagian yang bisa diuji tanpa membangun rantai objek tiruan. */
export function parseBar(value: unknown): LastBar | null {
  if (!Array.isArray(value) || value.length < 5) return null;

  const [time, open, high, low, close, volume] = value as number[];
  if (![time, open, high, low, close].every((n) => typeof n === "number" && Number.isFinite(n))) {
    return null;
  }

  const konsisten =
    high >= Math.max(open, close) && low <= Math.min(open, close) && close > 0;
  if (!konsisten) return null;

  return {
    time,
    open,
    high,
    low,
    close,
    volume: typeof volume === "number" && Number.isFinite(volume) ? volume : 0,
  };
}

/**
 * Tentukan sisi mana yang bergerak dari entity id yang dilaporkan chart.
 *
 * Entity id acak, jadi satu satunya cara mengenalinya adalah mencocokkan
 * dengan id yang kita simpan sendiri. Jangan pernah menyimpulkan dari urutan
 * `getAllShapes()`, karena daftar itu juga memuat gambar milik user.
 */
export function sideOfShape(
  id: string,
  lines: { upper?: string; lower?: string },
): "upper" | "lower" | null {
  if (lines.upper === id) return "upper";
  if (lines.lower === id) return "lower";
  return null;
}
