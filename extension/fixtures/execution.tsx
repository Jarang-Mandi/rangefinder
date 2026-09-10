import { render } from "preact";
import { useState } from "preact/hooks";
import { SetoranCard } from "../src/sidepanel/App";
import type { Deposit, RangeView, TokenState } from "../src/shared/protocol";
import "../src/shared/tokens.css";
import "../src/sidepanel/panel.css";

const alamatMeme = "0x1111111111111111111111111111111111111111";
const alamatQuote = "0x2222222222222222222222222222222222222222";
const poolId = "0x3333333333333333333333333333333333333333";

const token: TokenState = {
  address: alamatMeme,
  meta: { address: alamatMeme, symbol: "PONS", decimals: 18, total_supply: 1_000_000_000 },
  quotes: [{
    quote: alamatQuote,
    quote_symbol: "USDG",
    quote_decimals: 18,
    canonical: true,
    pools: [],
  }],
  pools: [{
    version: "v4",
    id: poolId,
    token0: alamatMeme,
    token1: alamatQuote,
    fee: 3000,
    dynamic_fee: false,
    tick_spacing: 60,
    hooks: null,
    sqrt_price_x96: "79228162514264337593543950336",
    tick: 0,
    liquidity: "1000000000000000000000",
    raw_price: 0.0000042,
  }],
  selected_quote: alamatQuote,
  selected_pool_id: poolId,
  error: null,
  saldo: { memecoin: 0, quote: 200_000, gas: 0.09 },
};

const view: RangeView = {
  pool_id: poolId,
  tick_spacing: 60,
  eth_usd: 1,
  current: { tick: 0, price_weth: 0.0000042, price_usd: 0.0000042, mcap_usd: 4200 },
  lower: { tick: -600, price_weth: 0.0000039, price_usd: 0.0000039, mcap_usd: 3900, drift_bps: 0 },
  upper: { tick: 600, price_weth: 0.0000045, price_usd: 0.0000045, mcap_usd: 4500, drift_bps: 0 },
  width_pct: 14.3,
  in_range: true,
  warnings: [],
};

function Preview() {
  const [deposit, setDeposit] = useState<Deposit | null>(null);
  return (
    <main class="content" style="width:100%;max-width:480px;margin:0 auto">
      <section class="card execution-card">
        <SetoranCard
          plan={null}
          view={view}
          token={token}
          busy={false}
          canExecute
          onQuote={setDeposit}
        />
      </section>
      <output aria-label="Setoran terakhir" style="display:none">
        {JSON.stringify(deposit)}
      </output>
    </main>
  );
}

render(<Preview />, document.getElementById("root")!);
