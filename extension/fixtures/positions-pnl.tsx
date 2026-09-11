import { render } from "preact";
import { useState } from "preact/hooks";
import { PnlTab, PositionCard } from "../src/sidepanel/App";
import type { ActionInput, ActionPlanView, JenisAksi, Posisi, PosisiTertutup, PositionsView } from "../src/shared/protocol";
import "../src/shared/tokens.css";
import "../src/sidepanel/panel.css";

const eth = "0x0000000000000000000000000000000000000000";
const pons = "0x1111111111111111111111111111111111111111";
const position: Posisi = {
  position_id: "4663:0x2222222222222222222222222222222222222222:2339246",
  token_id: "2339246",
  version: "v4",
  token0: eth,
  token1: pons,
  symbol0: "ETH",
  symbol1: "PONS",
  decimals0: 18,
  decimals1: 18,
  base_symbol: "PONS",
  quote_symbol: "ETH",
  price_current: 0.00029931,
  price_lower: 0.00025744,
  price_upper: 0.00034851,
  fee: 40000,
  tick_spacing: 1000,
  tick_lower: 80_000,
  tick_upper: 83_000,
  liquidity: "7283155827203011",
  tick_sekarang: 81_420,
  in_range: true,
  fee0: "1548293102",
  fee1: "3200000000000000",
  hooks: null,
  sqrt_price_x96: "151389800000000000000000000000000000000",
  amount0: "283694831448035",
  amount1: "948195735364774896",
  pnl: {
    currency: { id: `4663:${eth}`, symbol: "ETH" },
    fee: 0.000001551,
    deposited: 0.0242,
    value: 0.0254,
    pnl_percent: -0.000050338272946 / 0.0242 * 100,
    opened_at: new Date(Date.now() - 3 * 60 * 60_000).toISOString(),
    impermanent_loss: 0.00000282,
    gas: 0.000048459272946,
    slippage: 0.00000061,
    net: -0.000050338272946,
    perkiraan: false,
  },
  kosong: false,
};

const closed: PosisiTertutup = {
  position_id: "4663:0x2222222222222222222222222222222222222222:2339000",
  token0: eth,
  token1: pons,
  symbol0: "ETH",
  symbol1: "PONS",
  tx_hash: "0x3333333333333333333333333333333333333333333333333333333333333333",
  ditutup_pada: new Date(Date.now() - 30 * 60_000).toISOString(),
  diterima0: "25400000000000000",
  diterima1: "0",
  bukti_penerimaan: "wallet_balance_delta",
  pnl: {
    currency: { id: `4663:${eth}`, symbol: "ETH" },
    fee: 0.0022,
    deposited: 0.0242,
    value: 0.0254,
    pnl_percent: 4.87,
    opened_at: new Date(Date.now() - 3.5 * 60 * 60_000).toISOString(),
    impermanent_loss: 0.0004,
    gas: 0.0002,
    slippage: 0,
    net: 0.00118,
    perkiraan: false,
  },
  error: null,
};

const view: PositionsView = {
  positions: [position],
  tertutup: [closed],
  owner: "0x420ac1fcca78c4cdfed3216201216ed80e7b2680",
  keluar_range: 0,
  ringkasan: {
    jumlah: 2,
    total_net: position.pnl!.net! + closed.pnl!.net!,
    total_fee: position.pnl!.fee! + closed.pnl!.fee!,
    total_biaya: 0.000051889272946,
    currencies: [{
      currency: position.pnl!.currency,
      jumlah: 2,
      total_net: position.pnl!.net! + closed.pnl!.net!,
      total_fee: position.pnl!.fee! + closed.pnl!.fee!,
      total_biaya: 0.000051889272946,
    }],
    win_rate_bps: 0,
    perkiraan: 0,
  },
  learning: {
    records: 2,
    closed_records: 1,
    position_versions: { [position.position_id.toLowerCase()]: "baseline-v1" },
    calibrations: [],
    summaries: [],
  },
  error: null,
  dimuat_pada: new Date().toISOString(),
};

function Preview() {
  const [plan, setPlan] = useState<ActionPlanView | null>(null);
  const [started, setStarted] = useState<JenisAksi | null>(null);
  const onPlan = (_id: string, action: JenisAksi, input?: ActionInput) => {
    if (action !== "tambah_likuiditas") return;
    const fromPons = Boolean(input?.amount1);
    const ethPerPons = 0.00029931;
    const amount0 = fromPons
      ? String(Number(input?.amount1 ?? "0") * ethPerPons)
      : input?.amount0 ?? "0";
    const amount1 = fromPons
      ? input?.amount1 ?? "0"
      : String(Number(input?.amount0 ?? "0") / ethPerPons);
    setPlan({
      plan_id: crypto.randomUUID(),
      position_id: position.position_id,
      request_id: crypto.randomUUID(),
      token_id: position.token_id,
      action,
      nama: "tambah likuiditas",
      gates: [],
      gas_estimate: 182_400,
      dry_run: false,
      menutup: false,
      siap: true,
      expires_at: new Date(Date.now() + 180_000).toISOString(),
      fee0: null,
      fee1: null,
      allowances: [],
      amounts: { symbol0: "ETH", amount0, symbol1: "PONS", amount1 },
      tick_lower_baru: null,
      tick_upper_baru: null,
    });
  };
  return (
    <main class="content fixture-grid" data-started={started ?? ""}>
      <div><PnlTab view={view} busy={false} onMuat={() => {}} /></div>
      <div>
        <PositionCard
          pos={position}
          plan={plan}
          busy={false}
          onPlanAction={onPlan}
          onStartAction={(_id, action) => setStarted(action)}
          onRunAction={() => {}}
        />
      </div>
    </main>
  );
}

render(<Preview />, document.getElementById("root")!);
