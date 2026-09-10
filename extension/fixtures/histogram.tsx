import { render } from "preact";
import { useState } from "preact/hooks";
import { Histogram } from "../src/sidepanel/Histogram";
import "../src/shared/tokens.css";
import "../src/sidepanel/panel.css";
import type { Distribusi } from "../src/shared/protocol";

// Fixture interaksi tanpa wallet, RPC, atau API Chrome. Tidak masuk build ekstensi.
const depth: Distribusi = {
  pool_id: "fixture", harga_sekarang: .00001, tick_sekarang: 0, tick_spacing: 60,
  quote_adalah_token1: true, error: null,
  pita: Array.from({ length: 200 }, (_, i) => ({
    harga_bawah: .00001 * 1.0001 ** ((i - 100) * 30),
    harga_atas: .00001 * 1.0001 ** ((i - 99) * 30),
    tick_bawah: (i - 100) * 30, tick_atas: (i - 99) * 30,
    liquidity: "10000", nilai_quote: 10 + 100 * Math.exp(-(((i - 120) / 30) ** 2)), aktif: i === 100,
  })),
};
function Preview() {
  const [bounds, setBounds] = useState<[number, number]>([.9, 1.1]);
  const [commits, setCommits] = useState(0);
  const [pending, setPending] = useState<[number, number] | null>(null);
  return <main class="content" style="max-width:480px;margin:0 auto">
    <section class="card"><h2>Distribusi likuiditas</h2>
      <Histogram depth={depth} kelipatanBawah={bounds[0]} kelipatanAtas={bounds[1]}
        onCommit={(lo, hi) => {
          setPending([lo, hi]);
          setCommits(c => c + 1);
          // Meniru perjalanan panel -> engine -> panel. Handle tidak boleh
          // kembali ke batas lama selama jawaban ini belum tiba.
          window.setTimeout(() => {
            setBounds([lo, hi]);
            setPending(null);
          }, 300);
        }} />
      <output aria-label="Hasil interaksi">{JSON.stringify({ commits, bounds, pending })}</output>
    </section>
  </main>;
}
render(<Preview />, document.getElementById("root")!);
