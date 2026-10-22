// Baca keadaan yang benar-benar diterima panel tanpa mencetak pairing token.
// Cara pakai: node scripts/probe-runtime.mjs

import { readFileSync } from "node:fs";
import path from "node:path";

const dir = path.join(process.env.LOCALAPPDATA, "Rangefinder");
const token = readFileSync(path.join(dir, "pairing.token"), "utf8").trim();
const ws = new WebSocket("ws://127.0.0.1:8787/ws", ["rangefinder", token]);
const positionsOnly = process.argv.includes("--positions-only");
const statusOnly = process.argv.includes("--status-only");
let positionsReceived = false;
let transactionsReceived = positionsOnly;
let protocolVersion = null;

function finishIfComplete() {
  if (!positionsReceived || !transactionsReceived) return;
  ws.close();
  setTimeout(() => process.exit(0), 50);
}

ws.addEventListener("message", (event) => {
  const message = JSON.parse(event.data);
  if (message.type === "hello") {
    protocolVersion = message.protocol_version;
    if (statusOnly) return;
    ws.send(JSON.stringify({ type: "get_positions" }));
    if (!positionsOnly) ws.send(JSON.stringify({ type: "get_transactions" }));
    return;
  }
  if (message.type === "engine_status" && statusOnly) {
    console.log(JSON.stringify({
      type: message.type,
      protocol: protocolVersion,
      healthy: message.chain?.healthy,
      uniswap: message.uniswap,
      rpc_endpoints: message.rpc_endpoints ?? [],
    }, null, 2));
    ws.close();
    setTimeout(() => process.exit(0), 50);
    return;
  }
  if (message.type === "positions") {
    positionsReceived = true;
    console.log(JSON.stringify({
      type: message.type,
      error: message.error,
      keluar_range: message.keluar_range,
      positions: (message.positions ?? []).map((position) => ({
        id: position.position_id,
        version: position.version,
        token0: position.token0,
        token1: position.token1,
        symbol0: position.symbol0,
        symbol1: position.symbol1,
        liquidity: position.liquidity,
        kosong: position.kosong,
        in_range: position.in_range,
      })),
      closed: (message.tertutup ?? []).map((position) => ({
        id: position.position_id,
        pair: `${position.symbol0 ?? "?"}/${position.symbol1 ?? "?"}`,
        detected_at: position.ditutup_pada,
        evidence: position.bukti_penerimaan,
        pnl: position.pnl ? {
          deposited: position.pnl.deposited,
          value: position.pnl.value,
          net: position.pnl.net,
          percent: position.pnl.pnl_percent,
          estimated: position.pnl.perkiraan,
        } : null,
      })),
    }, null, 2));
    finishIfComplete();
  }
  if (message.type === "transactions") {
    transactionsReceived = true;
    console.log(JSON.stringify({
      type: message.type,
      items: (message.items ?? []).slice(0, 30).map((item) => ({
        operation: item.operation,
        status: item.status,
        tx: item.tx_hash,
        position: item.position_id,
        plan: item.plan_id,
        flow: item.flow_id,
        error: item.error,
        created: item.created_at,
      })),
    }, null, 2));
    finishIfComplete();
  }
});

setTimeout(() => {
  ws.close();
  process.exit(0);
}, 15_000);
