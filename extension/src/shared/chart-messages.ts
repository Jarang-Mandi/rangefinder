/** Identitas chart diturunkan dari URL tab, bukan field kiriman halaman. */
export function robinhoodToken(url: string): string | null {
  try {
    const u = new URL(url);
    if (u.origin !== "https://gmgn.ai") return null;
    return u.pathname.match(/^\/robinhood\/token\/(0x[a-fA-F0-9]{40})\/?$/)?.[1].toLowerCase() ?? null;
  } catch { return null; }
}

type RecordData = Record<string, unknown>;
function record(v: unknown): v is RecordData {
  return v !== null && typeof v === "object" && !Array.isArray(v);
}
function bar(v: unknown): RecordData | null {
  if (!record(v)) return null;
  const keys = ["time", "open", "high", "low", "close", "volume"] as const;
  if (keys.some(k => typeof v[k] !== "number" || !Number.isFinite(v[k]))) return null;
  const b = v as Record<typeof keys[number], number>;
  if (b.time <= 0 || b.low <= 0 || b.volume < 0 || b.high < Math.max(b.open, b.close, b.low)
      || b.low > Math.min(b.open, b.close)) return null;
  return Object.fromEntries(keys.map(k => [k, b[k]]));
}

/** Perintah gambar juga terikat ke token tab tujuan setelah navigasi. */
export function chartCommand(raw: unknown, url: string): RecordData | null {
  const token = robinhoodToken(url);
  if (!token || !record(raw) || typeof raw.token !== "string" || raw.token.toLowerCase() !== token) return null;
  if (raw.kind === "clear_lines" || raw.kind === "probe") return { kind: raw.kind, token };
  if (raw.kind === "draw_lines" && typeof raw.lowerPrice === "number" && typeof raw.upperPrice === "number"
      && Number.isFinite(raw.lowerPrice) && Number.isFinite(raw.upperPrice) && raw.lowerPrice > 0 && raw.upperPrice > raw.lowerPrice) {
    return { kind: raw.kind, token, lowerPrice: raw.lowerPrice, upperPrice: raw.upperPrice };
  }
  return null;
}

/** Daftar field eksplisit mencegah halaman menyisipkan target, source, tabId,
 * maupun perintah runtime di jalur data chart. */
export function chartMessage(raw: unknown, url: string): RecordData | null {
  const token = robinhoodToken(url);
  if (!token || !record(raw) || typeof raw.token !== "string" || raw.token.toLowerCase() !== token) return null;
  const base = { token, url, chain: "robinhood" };
  switch (raw.kind) {
    case "token_page": return { ...base, kind: raw.kind };
    case "chart_price": {
      const b = bar(raw.bar);
      return b ? { ...base, kind: raw.kind, bar: b } : null;
    }
    case "chart_bars": {
      if (!Array.isArray(raw.bars) || raw.bars.length > 5000) return null;
      const bars = raw.bars.map(bar);
      return bars.every(b => b !== null) ? { ...base, kind: raw.kind, bars } : null;
    }
    case "chart_line_moved":
      return (raw.side === "upper" || raw.side === "lower") && typeof raw.price === "number"
        && Number.isFinite(raw.price) && raw.price > 0
        ? { ...base, kind: raw.kind, side: raw.side, price: raw.price } : null;
    default: return null;
  }
}

export function chartMatches(raw: RecordData, tabId: number | null, address: string | null): boolean {
  return tabId !== null && raw.tabId === tabId && !!address
    && typeof raw.token === "string" && raw.token.toLowerCase() === address.toLowerCase()
    && typeof raw.url === "string" && robinhoodToken(raw.url) === address.toLowerCase();
}
