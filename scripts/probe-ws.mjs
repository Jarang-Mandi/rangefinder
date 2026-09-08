// Lihat `token_state` persis seperti panel menerimanya.
//
// KENAPA ALAT INI ADA
// Tiga kali berturut turut sebuah angka dilaporkan tidak muncul di panel, dan
// tiga kali jawabannya dari membaca kode ternyata salah. Kegagalannya selalu di
// jalur pengiriman, bukan di rumusnya: kiriman kedua ditahan karena dianggap
// sama dengan yang pertama, atau field barunya tidak ada karena enginenya masih
// versi lama.
//
// Menyambung persis seperti panel menyelesaikannya dalam satu percobaan.
//
// Cara pakai: node scripts/probe-ws.mjs <alamat_token>
// Engine harus sedang berjalan.

import { readFileSync } from "node:fs";
import path from "node:path";

const dir = path.join(process.env.LOCALAPPDATA, "Rangefinder");
const token = readFileSync(path.join(dir, "pairing.token"), "utf8").trim();
const alamat = process.argv[2] ?? "0xdc4dd8839f02f2242e9552fe4652635fb7f95cc6";

const ws = new WebSocket("ws://127.0.0.1:8787/ws", ["rangefinder", token]);

let n = 0;
ws.addEventListener("open", () => {
  console.log("tersambung, minta token", alamat);
  ws.send(JSON.stringify({ type: "subscribe_token", address: alamat }));
});

ws.addEventListener("message", (ev) => {
  const m = JSON.parse(ev.data);
  if (m.type !== "token_state") return;
  n += 1;
  const pools = m.pools ?? [];
  const dengan = pools.filter((p) => p.stats).length;
  console.log(
    `token_state #${n}: ${pools.length} pool, ${dengan} punya stats, saldo ${m.saldo ? "ada" : "kosong"}`,
  );
  if (dengan > 0) {
    const s = pools.find((p) => p.stats).stats;
    console.log("   contoh stats:", JSON.stringify(s));
  }
});

ws.addEventListener("error", (e) => console.log("error:", e.message ?? e));

setTimeout(() => {
  console.log(`selesai, total ${n} kiriman token_state`);
  ws.close();
  process.exit(0);
}, 25000);
