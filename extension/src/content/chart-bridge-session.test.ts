import assert from "node:assert/strict";
import { test } from "node:test";
import vm from "node:vm";
import { fileURLToPath } from "node:url";
import { build } from "vite";

// Bundle sumber produksi di memori, sehingga tes tidak bergantung dist lama.
const bundled = await build({ configFile: false, logLevel: "silent", build: {
  write: false, minify: false,
  rollupOptions: { input: fileURLToPath(new URL("./chart-bridge.ts", import.meta.url)),
    output: { format: "iife", inlineDynamicImports: true } },
} });
if (!("output" in bundled)) throw Error("bundle bridge tidak tersedia");
const chunk = bundled.output.find(out => out.type === "chunk");
if (!chunk || chunk.type !== "chunk") throw Error("chunk bridge tidak tersedia");
const source = chunk.code;
const A = "0x1111111111111111111111111111111111111111";
const B = "0x2222222222222222222222222222222222222222";
const url = (token: string) => `https://gmgn.ai/robinhood/token/${token}`;
const flush = () => new Promise<void>(resolve => setImmediate(resolve));

function fakeChart(name: string, slowReady = false, slowShape = false) {
  const removed: string[] = [];
  const created: string[] = [];
  let ready: (() => void) | null = null;
  let shape: ((id: string) => void) | null = null;
  const token = name === "A" ? A : B;
  const chart = {
    symbol: () => `robinhood/${token}/USD/MCAP`, resolution: () => "1", getSeries: () => null,
    dataReady: (callback: () => void) => { ready = callback; return !slowReady; },
    getVisibleRange: () => ({ from: 1, to: 2 }),
    getVisiblePriceRange: () => ({ from: 1, to: 2 }),
    getShapeById: () => ({ getPoints: () => [{ time: 1, price: 2 }], setPoints: () => {} }),
    getAllShapes: () => [],
    removeEntity: (id: string) => removed.push(id),
    createShape: () => {
      const id = `${name}-${created.length}`;
      created.push(id);
      return slowShape ? new Promise<string>(resolve => { shape = resolve; }) : Promise.resolve(id);
    },
  };
  const api = { activeChart: () => chart, subscribe: () => {}, unsubscribe: () => {} };
  return { api, created, removed, ready: () => ready?.(), finish: () => shape?.(`${name}-0`) };
}

function environment(initial: ReturnType<typeof fakeChart>) {
  const posted: Record<string, unknown>[] = [];
  const listeners = new Map<string, ((event: unknown) => void)[]>();
  const location = new URL(url(A));
  const frame = { contentWindow: { tradingViewApi: initial.api } };
  const timers = new Set<ReturnType<typeof setTimeout>>();
  const later = (callback: () => void, ms: number) => {
    const timer = setTimeout(() => { timers.delete(timer); callback(); }, ms);
    timers.add(timer); return timer;
  };
  const cancel = (timer: ReturnType<typeof setTimeout>) => { clearTimeout(timer); timers.delete(timer); };
  const window = {
    addEventListener: (name: string, listener: (event: unknown) => void) => {
      listeners.set(name, [...(listeners.get(name) ?? []), listener]);
    },
    postMessage: (data: Record<string, unknown>) => posted.push(data),
    setInterval: () => 1, clearInterval: () => {},
  };
  const history = { pushState: (_state: unknown, _title: string, next: string) => { location.href = next; }, replaceState: () => {} };
  const context = { window, location, history, console, URL, setTimeout: later, clearTimeout: cancel, queueMicrotask,
    document: { body: {}, querySelector: () => frame },
    MutationObserver: class { observe() {} },
  };
  vm.runInNewContext(source, context);
  return {
    posted,
    navigate: (token: string, next: ReturnType<typeof fakeChart>) => {
      frame.contentWindow.tradingViewApi = next.api;
      history.pushState(null, "", url(token));
    },
    navigateBeforeChart: (token: string) => history.pushState(null, "", url(token)),
    setChart: (next: ReturnType<typeof fakeChart>) => { frame.contentWindow.tradingViewApi = next.api; },
    draw: (token: string) => {
      const event = { source: window, origin: location.origin,
        data: { channel: "rangefinder", direction: "to-page", kind: "draw_lines", token, lowerPrice: 1, upperPrice: 2 } };
      for (const listener of listeners.get("message") ?? []) listener(event);
    },
    close: () => { for (const timer of timers) clearTimeout(timer); },
  };
}

test("bundle bridge: attach lama tidak mengganti API token tujuan", async () => {
  const old = fakeChart("A", true), next = fakeChart("B");
  const env = environment(old);
  try {
    env.navigate(B, next);
    await flush();
    old.ready();
    await flush();
    env.draw(A); // Pesan dari panel yang berangkat sebelum navigasi.
    await flush();
    assert.equal(next.created.length, 0);
    env.draw(B);
    await flush();
    assert.equal(old.created.length, 0);
    assert.deepEqual(next.created, ["B-0", "B-1"]);
    assert.deepEqual(env.posted.filter(p => p.kind === "token_page").map(p => p.token), [B]);
  } finally { env.close(); }
});

test("bundle bridge: URL baru menunggu simbol chart baru", async () => {
  const old = fakeChart("A"), next = fakeChart("B");
  const env = environment(old);
  try {
    await flush();
    env.posted.length = 0;
    env.navigateBeforeChart(B);
    await flush();

    assert.deepEqual(env.posted.filter(p => p.kind === "token_page"), []);
    assert.deepEqual(env.posted.filter(p => p.kind === "chart_price"), []);

    env.setChart(next);
    await new Promise(resolve => setTimeout(resolve, 70));
    await flush();

    assert.deepEqual(env.posted.filter(p => p.kind === "token_page").map(p => p.token), [B]);
    assert.equal(
      env.posted.some(p => p.kind === "chart_price" && p.symbol === `robinhood/${A}/USD/MCAP`),
      false,
    );
  } finally { env.close(); }
});

test("bundle bridge: shape lama tidak menahan antrean atau mencemari chart baru", async () => {
  const old = fakeChart("A", false, true), next = fakeChart("B");
  const env = environment(old);
  try {
    await flush();
    env.draw(A);
    await flush();
    assert.deepEqual(old.created, ["A-0"]);
    env.navigate(B, next);
    await flush();
    env.draw(B);
    await flush();
    assert.deepEqual(next.created, ["B-0", "B-1"]);
    old.finish();
    await flush();
    assert.deepEqual(old.removed, ["A-0"]);
    assert.deepEqual(next.removed, []);
    assert.deepEqual(env.posted.filter(p => p.kind === "lines_drawn").map(p => p.token), [B]);
  } finally { env.close(); }
});
