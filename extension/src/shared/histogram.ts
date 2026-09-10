import type { Pita } from "./protocol.ts";

export interface Domain { low: number; high: number }
export function priceAt(y: number, domain: Domain): number {
  const t = Math.max(0, Math.min(1, y));
  return Math.exp(Math.log(domain.high) * (1 - t) + Math.log(domain.low) * t);
}
export function priceY(price: number, domain: Domain): number {
  return Math.max(0, Math.min(1, Math.log(domain.high / price) / Math.log(domain.high / domain.low)));
}
export function zoomDomain(domain: Domain, current: number, zoom: number): Domain {
  const low = Math.log(domain.low), high = Math.log(domain.high);
  const width = (high - low) / Math.max(1, zoom);
  const start = Math.max(low, Math.min(high - width, Math.log(current) - width / 2));
  return { low: Math.exp(start), high: Math.exp(start + width) };
}

/** Penggabungan mempertahankan total modal, orientasi harga, serta pita aktif.
 * Batas 24 tetap berlaku walau pita aktif ada di tengah sebuah kelompok. */
export function mergeBands(input: Pita[], max = 24): Pita[] {
  const out = [...input].sort((a, b) => b.harga_bawah - a.harga_bawah);
  while (out.length > Math.max(3, max)) {
    let index = -1, width = Infinity;
    for (let i = 0; i < out.length - 1; i++) {
      if (out[i].aktif || out[i + 1].aktif) continue;
      const w = Math.log(out[i].harga_atas / out[i + 1].harga_bawah);
      if (w < width) { width = w; index = i; }
    }
    if (index < 0) break;
    const a = out[index], b = out[index + 1];
    out.splice(index, 2, { ...a,
      tick_bawah: Math.min(a.tick_bawah, b.tick_bawah), tick_atas: Math.max(a.tick_atas, b.tick_atas),
      harga_bawah: Math.min(a.harga_bawah, b.harga_bawah), harga_atas: Math.max(a.harga_atas, b.harga_atas),
      nilai_quote: a.nilai_quote + b.nilai_quote, aktif: false,
    });
  }
  return out;
}
