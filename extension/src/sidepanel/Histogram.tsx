import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import type { Distribusi } from "../shared/protocol";
import { mergeBands, priceAt, priceY, zoomDomain } from "../shared/histogram";

const TOP = 18, HEIGHT = 252;
type Bounds = [number, number];

/** Penyunting range pada sumbu harga. Snap tick tetap dikerjakan engine. */
export function Histogram({ depth, kelipatanBawah, kelipatanAtas, onCommit }: {
  depth: Distribusi; kelipatanBawah: number; kelipatanAtas: number;
  onCommit: (lowerRatio: number, upperRatio: number) => void;
}) {
  const [zoom, setZoom] = useState(1);
  const [draft, setDraft] = useState<Bounds | null>(null);
  const svg = useRef<SVGSVGElement>(null);
  const drag = useRef<{ side: 0 | 1; bounds: Bounds } | null>(null);
  const bands = useMemo(() => mergeBands(depth.pita), [depth.pita]);
  const all = { low: Math.min(...depth.pita.map(p => p.harga_bawah)), high: Math.max(...depth.pita.map(p => p.harga_atas)) };
  const domain = zoomDomain(all, depth.harga_sekarang, zoom);
  const bounds: Bounds = draft ?? [kelipatanBawah * depth.harga_sekarang, kelipatanAtas * depth.harga_sekarang];
  const sorted = [...bounds].sort((a, b) => a - b);
  const peak = Math.max(...bands.map(p => p.nilai_quote), 1e-30);
  const y = (price: number) => TOP + priceY(price, domain) * HEIGHT;
  const label = (n: number) => n >= 1 ? n.toLocaleString("id-ID", { maximumFractionDigits: 3 }) : n.toPrecision(3);
  const commit = (b: Bounds) => {
    const [lo, hi] = [...b].sort((a, b) => a - b);
    if (lo > 0 && hi > lo) onCommit(lo / depth.harga_sekarang, hi / depth.harga_sekarang);
    // Pertahankan posisi optimistis sampai jawaban engine tiba. Mengosongkan
    // draft di sini membuat handle kembali sesaat ke batas lama, lalu meloncat
    // lagi ke tick hasil pembulatan setelah perjalanan WebSocket selesai.
    setDraft([lo, hi]);
  };
  useEffect(() => {
    // Perubahan props adalah pengakuan dari engine. Jangan membatalkan draft
    // ketika harga berkala datang di tengah drag; pointer masih memegang sumber
    // kebenaran visual sampai dilepas.
    if (!drag.current) setDraft(null);
  }, [kelipatanBawah, kelipatanAtas, depth.harga_sekarang]);
  const move = (event: PointerEvent) => {
    if (!drag.current || !svg.current) return;
    const rect = svg.current.getBoundingClientRect();
    const localY = (event.clientY - rect.top) / rect.height * 288;
    const b: Bounds = [...drag.current.bounds];
    b[drag.current.side] = priceAt((localY - TOP) / HEIGHT, domain);
    drag.current.bounds = b;
    setDraft(b);
  };
  return <div class="histogram">
    <div class="histogram-tools">
      <span>Harga / kutipan</span>
      <div class="histogram-zoom" aria-label="Zoom histogram">
        <button disabled={zoom <= 1} onClick={() => setZoom(z => z / 2)} aria-label="Perkecil histogram">−</button>
        <button onClick={() => setZoom(1)} aria-label="Reset zoom histogram">{zoom}×</button>
        <button disabled={zoom >= 8} onClick={() => setZoom(z => z * 2)} aria-label="Perbesar histogram">+</button>
      </div>
    </div>
    <svg ref={svg} viewBox="0 0 320 288" class="histogram-chart" aria-label="Distribusi likuiditas dan batas range">
      <rect x="68" y={y(sorted[1])} width="242" height={Math.max(0, y(sorted[0]) - y(sorted[1]))} class="histogram-selection" />
      {bands.filter(p => p.harga_atas > domain.low && p.harga_bawah < domain.high).map(p => {
        const top = y(p.harga_atas), bottom = y(p.harga_bawah);
        return <g key={`${p.tick_bawah}:${p.tick_atas}`}>
          <rect x="72" y={top} width={p.nilai_quote / peak * 234} height={Math.max(1, bottom - top - 1)} class={`histogram-bar${p.aktif ? " current" : ""}`}>
            <title>{label(p.harga_bawah)}–{label(p.harga_atas)}: {label(p.nilai_quote)} kutipan</title>
          </rect>
        </g>;
      })}
      {[0, .25, .5, .75, 1].map(t => <g key={t}>
        <line x1="68" x2="310" y1={TOP + t * HEIGHT} y2={TOP + t * HEIGHT} class="histogram-grid" />
        <text x="62" y={TOP + t * HEIGHT + 3} text-anchor="end" class="histogram-axis">{label(priceAt(t, domain))}</text>
      </g>)}
      <line x1="68" x2="310" y1={y(depth.harga_sekarang)} y2={y(depth.harga_sekarang)} class="histogram-spot" />
      {([0, 1] as const).map(side => <g key={side} role="slider" tabindex={0}
        aria-label={side === 0 ? "Batas bawah range" : "Batas atas range"}
        aria-orientation="vertical" aria-valuemin={Math.min(all.low, bounds[side])} aria-valuemax={Math.max(all.high, bounds[side])}
        aria-valuenow={bounds[side]} aria-valuetext={label(bounds[side])}
        class={`histogram-handle ${side === 0 ? "lower" : "upper"}`} transform={`translate(0 ${y(bounds[side])})`}
        onPointerDown={event => {
          event.preventDefault();
          event.currentTarget.setPointerCapture(event.pointerId);
          drag.current = { side, bounds: [...bounds] };
          setDraft([...bounds]);
        }}
        onPointerMove={move}
        onPointerUp={event => {
          if (!drag.current) return;
          move(event); const b = drag.current.bounds; drag.current = null;
          event.currentTarget.releasePointerCapture(event.pointerId); commit(b);
        }}
        onPointerCancel={() => { drag.current = null; setDraft(null); }}
        onKeyDown={event => {
          if (event.key === "Escape") { drag.current = null; setDraft(null); return; }
          if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
          event.preventDefault();
          const b: Bounds = [...bounds];
          b[side] *= 1.0001 ** (depth.tick_spacing * (event.shiftKey ? 10 : 1) * (event.key === "ArrowUp" ? 1 : -1));
          commit(b);
        }}>
        <rect x="68" y="-11" width="242" height="22" fill="transparent" />
        <line x1="68" x2="310" y1="0" y2="0" />
        <rect x="270" y="-10" width="40" height="20" rx="3" class="histogram-grip" />
        <text x="290" y="3" text-anchor="middle">{side === 0 ? "BAWAH" : "ATAS"}</text>
      </g>)}
    </svg>
    <div class="histogram-bounds num"><span>{label(sorted[0])}</span><span>{label(sorted[1])}</span></div>
  </div>;
}
