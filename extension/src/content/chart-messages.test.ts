import assert from "node:assert/strict";
import { test } from "node:test";
import { chartCommand, chartMessage, chartMatches, robinhoodToken } from "../shared/chart-messages.ts";
const token = "0x1111111111111111111111111111111111111111";
const url = `https://gmgn.ai/robinhood/token/${token}`;

test("perintah gambar tertunda tidak boleh masuk ke token tab yang sudah berubah", () => {
  const command = { kind: "draw_lines", token, lowerPrice: 1, upperPrice: 2, direction: "forged" };
  assert.deepEqual(chartCommand(command, url), { kind: "draw_lines", token, lowerPrice: 1, upperPrice: 2 });
  assert.equal(chartCommand(command, url.replaceAll("1", "2")), null);
  assert.equal(chartCommand({ ...command, upperPrice: NaN }, url), null);
  assert.equal(chartCommand({ ...command, lowerPrice: 3 }, url), null);
});
test("chain dan asal URL harus Robinhood di gmgn", () => {
  assert.equal(robinhoodToken(url), token);
  assert.equal(robinhoodToken(url.replace("robinhood", "base")), null);
  assert.equal(robinhoodToken(url.replace("gmgn.ai", "gmgn.ai.example.com")), null);
});
test("field perintah dan identitas tab dari halaman dibuang", () => {
  assert.deepEqual(chartMessage({ token, kind: "token_page", tabId: 999, target: "navigate-gmgn", source: "other", url: "https://example.com" }, url),
    { token, kind: "token_page", url, chain: "robinhood" });
  assert.equal(chartMessage({ token, kind: "open_position" }, url), null);
});
test("pesan token lama atau tab lain ditolak", () => {
  const data = { tabId: 1, token, url };
  assert.equal(chartMatches(data, 2, token), false);
  assert.equal(chartMatches(data, 1, token.replaceAll("1", "2")), false);
  assert.equal(chartMatches(data, 1, token), true);
  assert.equal(chartMessage({ token: token.replaceAll("1", "2"), kind: "token_page" }, url), null);
});
test("candle cacat dan deret tak terbatas tidak diteruskan", () => {
  const bar = { time: 1, open: 10, high: 12, low: 9, close: 11, volume: 1 };
  assert.ok(chartMessage({ token, kind: "chart_price", bar }, url));
  assert.equal(chartMessage({ token, kind: "chart_price", bar: { ...bar, close: 20 } }, url), null);
  assert.equal(chartMessage({ token, kind: "chart_bars", bars: Array(5001).fill(bar) }, url), null);
});
