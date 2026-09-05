/**
 * Side panel Rangefinder.
 *
 * Fase 1 mengisi kerangka navigasi empat tab sesuai PRD 3.2, tapi hanya tab
 * Range yang punya isi, yaitu status engine. Tab lain menyebutkan fase berapa
 * mereka akan hidup, bukan menampilkan tampilan palsu.
 *
 * INVARIAN
 * - Token pairing hanya disimpan di `chrome.storage.local`, tidak pernah masuk
 *   URL, dan tidak pernah ditampilkan kembali setelah tersimpan.
 * - Panel tidak pernah menyimpulkan sendiri boleh eksekusi atau tidak. Nilai
 *   `can_execute` dan `blocked_reasons` datang dari engine, karena engine yang
 *   memegang aturan sebenarnya. Lihat PRD 7.3.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "preact/hooks";
import {
  EngineClient,
  type ConnectionState,
} from "../shared/engine-client";
import { DEFAULT_ENGINE_ORIGIN, type EngineStatus } from "../shared/protocol";

const TOKEN_KEY = "rangefinder.pairingToken";

type TabId = "range" | "signals" | "positions" | "pnl";

const TABS: { id: TabId; label: string; phase: number }[] = [
  { id: "range", label: "Range", phase: 1 },
  { id: "signals", label: "Signals", phase: 6 },
  { id: "positions", label: "Positions", phase: 5 },
  { id: "pnl", label: "PnL", phase: 5 },
];

export function App() {
  const [token, setToken] = useState<string | null>(null);
  const [loadingToken, setLoadingToken] = useState(true);
  const [state, setState] = useState<ConnectionState>("idle");
  const [stateDetail, setStateDetail] = useState<string>();
  const [status, setStatus] = useState<EngineStatus | null>(null);
  const [tab, setTab] = useState<TabId>("range");

  const clientRef = useRef<EngineClient | null>(null);

  useEffect(() => {
    chrome.storage.local.get(TOKEN_KEY).then((stored) => {
      const saved = stored[TOKEN_KEY];
      if (typeof saved === "string" && saved.length > 0) setToken(saved);
      setLoadingToken(false);
    });
  }, []);

  useEffect(() => {
    if (!token) return;

    const client = new EngineClient(DEFAULT_ENGINE_ORIGIN, token, {
      onState: (s, detail) => {
        setState(s);
        setStateDetail(detail);
      },
      onStatus: setStatus,
      onAlert: (level, code, message) => {
        // Fase 5 memindahkan ini ke toast di halaman gmgn beserta suaranya,
        // sesuai PRD 3.7. Untuk sekarang cukup tercatat di konsol panel.
        console.info(`[rangefinder] ${level} ${code}: ${message}`);
      },
    });

    clientRef.current = client;
    client.connect();
    return () => {
      client.disconnect();
      clientRef.current = null;
    };
  }, [token]);

  const savePairing = useCallback(async (value: string) => {
    await chrome.storage.local.set({ [TOKEN_KEY]: value });
    setToken(value);
  }, []);

  const forgetPairing = useCallback(async () => {
    await chrome.storage.local.remove(TOKEN_KEY);
    setToken(null);
    setStatus(null);
    setState("idle");
  }, []);

  if (loadingToken) return null;

  if (!token) {
    return <Pairing onPaired={savePairing} />;
  }

  return (
    <div class="app">
      <nav class="tabs">
        {TABS.map((t) => (
          <button
            key={t.id}
            class={`tab${tab === t.id ? " active" : ""}`}
            onClick={() => setTab(t.id)}
          >
            {t.label}
          </button>
        ))}
      </nav>

      <div class="content">
        {tab === "range" ? (
          <RangeTab
            status={status}
            state={state}
            detail={stateDetail}
            client={clientRef.current}
            onForget={forgetPairing}
          />
        ) : (
          <p class="phase-note">
            Tab ini hidup di fase {TABS.find((t) => t.id === tab)?.phase}.
          </p>
        )}
      </div>

      <StatusBar state={state} status={status} />
    </div>
  );
}

function Pairing({ onPaired }: { onPaired: (token: string) => void }) {
  const [value, setValue] = useState("");
  const [error, setError] = useState<string>();
  const [checking, setChecking] = useState(false);

  const submit = async () => {
    const trimmed = value.trim();
    if (!trimmed) {
      setError("Tempel token pairing dari jendela engine.");
      return;
    }

    setChecking(true);
    setError(undefined);
    try {
      const probe = new EngineClient(DEFAULT_ENGINE_ORIGIN, trimmed, {
        onState: () => {},
        onStatus: () => {},
        onAlert: () => {},
      });
      await probe.probe();
      // Token diuji lewat endpoint yang butuh token, supaya token yang salah
      // ketahuan sekarang, bukan nanti saat WebSocket gagal diam diam.
      await probe.call("/v1/status");
      onPaired(trimmed);
    } catch (e) {
      setError(
        e instanceof Error && e.message.includes("token")
          ? "Token tidak cocok. Ambil ulang dengan menjalankan engine, atau `--print-token`."
          : "Engine tidak menjawab di 127.0.0.1:8787. Pastikan engine sedang jalan.",
      );
    } finally {
      setChecking(false);
    }
  };

  return (
    <div class="pairing">
      <h1>Sambungkan engine</h1>
      <p>Tempel token pairing yang ditampilkan engine saat pertama kali jalan.</p>

      <input
        type="text"
        value={value}
        placeholder="32 karakter hex"
        spellcheck={false}
        onInput={(e) => setValue((e.target as HTMLInputElement).value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") void submit();
        }}
      />

      <div class="actions">
        <button class="primary" disabled={checking} onClick={() => void submit()}>
          {checking ? "Memeriksa" : "Sambungkan"}
        </button>
      </div>

      {error ? <p class="error">{error}</p> : null}
      <p class="hint">Token disimpan lokal dan tidak diminta lagi.</p>
    </div>
  );
}

function RangeTab({
  status,
  state,
  detail,
  client,
  onForget,
}: {
  status: EngineStatus | null;
  state: ConnectionState;
  detail?: string;
  client: EngineClient | null;
  onForget: () => void;
}) {
  if (!status) {
    return (
      <p class="phase-note">
        {state === "connecting"
          ? "Menyambung ke engine."
          : (detail ?? "Engine belum menjawab.")}
      </p>
    );
  }

  return (
    <div class="stack">
      <ChainCard status={status} />
      <WalletCard status={status} client={client} />
      <RegistryCard status={status} />
      <EngineCard status={status} onForget={onForget} />
    </div>
  );
}

function ChainCard({ status }: { status: EngineStatus }) {
  const c = status.chain;
  return (
    <section class="card">
      <div class="section-title">
        <h2>Chain</h2>
        <span class="chip">
          <span class={`dot ${c.healthy ? "ok" : "bad"}`} />
          {c.healthy ? "sehat" : "bermasalah"}
        </span>
      </div>

      <div class="row">
        <span class="label">Chain id</span>
        <span class="value num">{c.chain_id ?? "-"}</span>
      </div>
      <div class="row">
        <span class="label">Blok</span>
        <span class="value num">{c.block_number?.toLocaleString("id-ID") ?? "-"}</span>
      </div>
      <div class="row">
        <span class="label">Lag</span>
        <span class="value num">
          {c.block_lag_secs === null ? "-" : `${c.block_lag_secs} d`}
          <span class="muted"> / {c.max_block_lag_secs} d</span>
        </span>
      </div>
      {c.error ? <p class="error">{c.error}</p> : null}
    </section>
  );
}

function WalletCard({
  status,
  client,
}: {
  status: EngineStatus;
  client: EngineClient | null;
}) {
  const w = status.wallet;
  const [passphrase, setPassphrase] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();

  const act = async (path: string, body?: unknown) => {
    if (!client) return;
    setBusy(true);
    setError(undefined);
    try {
      await client.call(path, {
        method: "POST",
        body: JSON.stringify(body ?? {}),
      });
      setPassphrase("");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section class="card">
      <div class="section-title">
        <h2>Wallet</h2>
        <span class="chip">
          <span class={`dot ${!w.exists ? "" : w.locked ? "warn" : "ok"}`} />
          {!w.exists ? "belum ada" : w.locked ? "terkunci" : "terbuka"}
        </span>
      </div>

      {w.address ? (
        <div class="row">
          <span class="label">Alamat</span>
          <span class="value num">{shortAddress(w.address)}</span>
        </div>
      ) : null}

      {w.exists && !w.locked && w.seconds_until_auto_lock !== null ? (
        <div class="row">
          <span class="label">Kunci otomatis</span>
          <span class="value num">{formatDuration(w.seconds_until_auto_lock)}</span>
        </div>
      ) : null}

      {w.exists && w.locked ? (
        <div class="stack" style="margin-top: var(--s3)">
          <input
            type="password"
            value={passphrase}
            placeholder="Passphrase"
            onInput={(e) => setPassphrase((e.target as HTMLInputElement).value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void act("/v1/wallet/unlock", { passphrase });
            }}
          />
          <button
            class="primary"
            disabled={busy || !passphrase}
            onClick={() => void act("/v1/wallet/unlock", { passphrase })}
          >
            Buka wallet
          </button>
        </div>
      ) : null}

      {w.exists && !w.locked ? (
        <button
          style="margin-top: var(--s3); width: 100%"
          disabled={busy}
          onClick={() => void act("/v1/wallet/lock")}
        >
          Kunci sekarang
        </button>
      ) : null}

      {!w.exists ? (
        <div class="stack" style="margin-top: var(--s3)">
          <p class="muted" style="margin: 0">
            Pakai wallet terpisah khusus operasi ini. Pembatas nominal bawaannya
            tidak terbatas, jadi pengamannya adalah isi wallet itu sendiri.
          </p>
          <input
            type="password"
            value={passphrase}
            placeholder="Passphrase baru, minimal 8 karakter"
            onInput={(e) => setPassphrase((e.target as HTMLInputElement).value)}
          />
          <button
            class="primary"
            disabled={busy || passphrase.length < 8}
            onClick={() => void act("/v1/wallet/create", { passphrase })}
          >
            Buat wallet
          </button>
        </div>
      ) : null}

      {error ? <p class="error">{error}</p> : null}
    </section>
  );
}

function RegistryCard({ status }: { status: EngineStatus }) {
  const r = status.registry;
  const [open, setOpen] = useState(false);

  if (r.total === 0) {
    return (
      <section class="card">
        <h2>Registry kontrak</h2>
        <p class="muted" style="margin: var(--s2) 0 0">
          Belum diverifikasi. Gate G1 butuh RPC yang sehat.
        </p>
      </section>
    );
  }

  return (
    <section class="card">
      <div class="section-title">
        <h2>Registry kontrak</h2>
        <button
          class="chip"
          style="cursor: pointer"
          onClick={() => setOpen((v) => !v)}
        >
          {open ? "tutup" : "rinci"}
        </button>
      </div>

      <div class="row">
        <span class="label">Cocok</span>
        <span class="value num">
          {r.verified} <span class="muted">/ {r.total}</span>
        </span>
      </div>
      {r.unpinned > 0 ? (
        <div class="row">
          <span class="label">Belum dipin</span>
          <span class="value num">{r.unpinned}</span>
        </div>
      ) : null}
      {r.no_code > 0 ? (
        <div class="row">
          <span class="label">Tanpa bytecode</span>
          <span class="value num" style="color: var(--lower)">
            {r.no_code}
          </span>
        </div>
      ) : null}

      {open ? (
        <div class="scroll-x" style="margin-top: var(--s3)">
          <table class="registry-table">
            <thead>
              <tr>
                <th>Kontrak</th>
                <th style="text-align: right">Status</th>
              </tr>
            </thead>
            <tbody>
              {r.checks.map((c) => (
                <tr key={`${c.section}.${c.name}`}>
                  <td class="muted">
                    {c.section}.{c.name}
                  </td>
                  <td>{registryLabel(c.status)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : null}
    </section>
  );
}

function EngineCard({
  status,
  onForget,
}: {
  status: EngineStatus;
  onForget: () => void;
}) {
  return (
    <section class="card">
      <h2>Engine</h2>
      <div class="row" style="margin-top: var(--s2)">
        <span class="label">Versi</span>
        <span class="value num">{status.engine_version}</span>
      </div>
      <div class="row">
        <span class="label">Aktif</span>
        <span class="value num">{formatDuration(status.uptime_secs)}</span>
      </div>

      {!status.can_execute ? (
        <ul class="blocked">
          {status.blocked_reasons.map((r) => (
            <li key={r}>{r}</li>
          ))}
        </ul>
      ) : null}

      <button style="margin-top: var(--s3); width: 100%" onClick={onForget}>
        Lupakan pairing
      </button>
    </section>
  );
}

function StatusBar({
  state,
  status,
}: {
  state: ConnectionState;
  status: EngineStatus | null;
}) {
  const label = useMemo(() => {
    switch (state) {
      case "connected":
        return "tersambung";
      case "connecting":
        return "menyambung";
      case "unauthorized":
        return "token salah";
      case "offline":
        return "engine mati";
      default:
        return "diam";
    }
  }, [state]);

  const dot =
    state === "connected" ? "ok" : state === "connecting" ? "warn" : "bad";

  return (
    <footer class="statusbar">
      <span class="chip">
        <span class={`dot ${dot}`} />
        {label}
      </span>
      {status ? (
        <span class="chip">
          <span class={`dot ${status.can_execute ? "ok" : "warn"}`} />
          {status.can_execute ? "siap eksekusi" : "eksekusi diblokir"}
        </span>
      ) : null}
    </footer>
  );
}

function registryLabel(s: string): string {
  switch (s) {
    case "verified":
      return "cocok";
    case "unpinned":
      return "belum dipin";
    case "no_code":
      return "tanpa kode";
    case "mismatch":
      return "BEDA";
    default:
      return s;
  }
}

function shortAddress(a: string): string {
  return a.length > 12 ? `${a.slice(0, 6)}...${a.slice(-4)}` : a;
}

function formatDuration(secs: number): string {
  if (secs < 60) return `${secs} d`;
  const m = Math.floor(secs / 60);
  if (m < 60) return `${m} m`;
  const h = Math.floor(m / 60);
  return `${h} j ${m % 60} m`;
}
