/**
 * Side panel Rangefinder.
 *
 * Kerangka navigasi empat tab sesuai PRD 3.2. Tab Range berisi alur token,
 * yaitu memuat pool dan menampilkan keadaan tick. Tab lain menyebutkan fase
 * berapa mereka akan hidup, bukan menampilkan tampilan palsu.
 *
 * INVARIAN
 * - Token pairing hanya disimpan di `chrome.storage.local`, tidak pernah masuk
 *   URL, dan tidak pernah ditampilkan kembali setelah tersimpan.
 * - Panel tidak pernah menyimpulkan sendiri boleh eksekusi atau tidak. Nilai
 *   `can_execute` dan `blocked_reasons` datang dari engine, karena engine yang
 *   memegang aturan sebenarnya. Lihat PRD 7.3.
 * - Panel juga tidak menyaring pool sendiri. Daftar yang datang dari engine
 *   sudah bersih dari pool tanpa likuiditas, dan panel menampilkannya apa
 *   adanya supaya tidak ada dua tempat yang memutuskan hal yang sama.
 * - Dua jalan masuk token wajib bekerja: menempel alamat di panel, dan user
 *   membuka sendiri halaman token di gmgn. Lihat PRD 3.3.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "preact/hooks";
import {
  EngineClient,
  type ConnectionState,
} from "../shared/engine-client";
import {
  DEFAULT_ENGINE_ORIGIN,
  gmgnTokenUrl,
  isEvmAddress,
  type EngineStatus,
  type ActionPlanView,
  type AlertLevel,
  type BarisScan,
  type Distribusi,
  type LabelScan,
  type HasilBasis,
  type JenisAksi,
  type Kriteria,
  type OpenPlanView,
  type Pita,
  type PoolInfo,
  type PositionsView,
  type Posisi,
  type PenilaianHook,
  type SaranPenempatan,
  type ScanView,
  type SignalsView,
  type SinyalSideways,
  type QuoteGroup,
  type SendResultView,
  type RangeBound,
  type RangeView,
  type TokenState,
} from "../shared/protocol";

const TOKEN_KEY = "rangefinder.pairingToken";

/**
 * Berapa lama kiriman `range_view` diabaikan setelah geseran terakhir.
 *
 * Harus lebih lama daripada satu perjalanan ke engine, karena kiriman yang
 * berangkat sesaat sebelum kursor dilepas baru sampai sesudahnya, dan kiriman
 * itulah yang dulu menggambar ulang garis di posisi hasil pembulatan.
 */
const JEDA_ABAIKAN_MS = 600;

type TabId = "range" | "signals" | "positions" | "pnl" | "settings";

const TABS: { id: TabId; label: string; phase: number }[] = [
  { id: "range", label: "Range", phase: 1 },
  { id: "signals", label: "Signals", phase: 0 },
  { id: "positions", label: "Positions", phase: 0 },
  { id: "pnl", label: "PnL", phase: 0 },
  { id: "settings", label: "Settings", phase: 0 },
];

export function App() {
  const [token, setToken] = useState<string | null>(null);
  const [loadingToken, setLoadingToken] = useState(true);
  const [state, setState] = useState<ConnectionState>("idle");
  const [stateDetail, setStateDetail] = useState<string>();
  const [status, setStatus] = useState<EngineStatus | null>(null);
  const [tab, setTab] = useState<TabId>("range");
  const [tokenState, setTokenState] = useState<TokenState | null>(null);
  const [tokenLoading, setTokenLoading] = useState(false);
  const [rangeView, setRangeView] = useState<RangeView | null>(null);
  const [openPlan, setOpenPlan] = useState<OpenPlanView | null>(null);
  const [sendResult, setSendResult] = useState<SendResultView | null>(null);
  /** Menunggu jawaban engine. Mencegah tombol ditekan dua kali. */
  const [execBusy, setExecBusy] = useState(false);
  const [positions, setPositions] = useState<PositionsView | null>(null);
  const [posBusy, setPosBusy] = useState(false);
  const [actionPlan, setActionPlan] = useState<ActionPlanView | null>(null);
  const [actionBusy, setActionBusy] = useState(false);
  const [signals, setSignals] = useState<SignalsView | null>(null);
  const [scan, setScan] = useState<ScanView | null>(null);

  /**
   * Benar kalau perubahan range terakhir datang dari geseran di chart.
   *
   * KENAPA INI PERLU
   * Menggeser garis memicu peristiwa pada setiap gerakan tetikus. Tiap gerakan
   * dikirim ke engine, dibulatkan ke grid tick, lalu dikirim balik, dan panel
   * menggambar ulang garis di posisi hasil pembulatan itu. Garisnya tersentak
   * di bawah kursor yang masih bergerak, dan tersentak sekali lagi saat
   * kursornya dilepas.
   *
   * Chart sudah menaruh garis persis di tempat user melepasnya. Menggambarnya
   * ulang tidak menambah apa pun, dan hasil pembulatan tetap terbaca sebagai
   * angka di kartu Range. Jadi penggambaran ulang hanya berlaku untuk
   * perubahan yang BUKAN dari chart: preset, custom, dan ganti pool.
   */
  /**
   * Kabar terakhir dari engine, ditampilkan sebagai baris di atas isi tab.
   *
   * Sebelumnya kabar hanya masuk konsol panel, sehingga aksi seperti mengganti
   * endpoint RPC tidak memberi tanda apa pun di layar dan user tidak tahu
   * apakah permintaannya sampai.
   */
  const [kabar, setKabar] = useState<{
    level: AlertLevel;
    message: string;
  } | null>(null);

  /**
   * Kapan garis terakhir digeser user, dalam milidetik.
   *
   * KENAPA JAM, BUKAN PENANDA BENAR SALAH
   * Satu geseran memicu puluhan peristiwa, dan setiap peristiwa menghasilkan
   * satu kiriman range_view dari engine. Penanda benar salah hanya menahan
   * kiriman PERTAMA: kiriman kedua dan seterusnya sudah melihat penandanya
   * kembali salah, lalu menggambar ulang garis di posisi hasil pembulatan.
   * Itulah kenapa loncatannya kembali muncul setelah perbaikan sebelumnya.
   *
   * Jam menahan seluruh kiriman yang datang selama geseran dan sesaat
   * sesudahnya, berapa pun jumlahnya. Chart sudah menaruh garis persis di
   * tempat user melepasnya, dan hasil pembulatan tetap terbaca sebagai angka
   * di kartu Range.
   */
  const geserTerakhirRef = useRef(0);
  /** Alamat token yang keadaannya terakhir diterima. Lihat `onTokenState`. */
  const alamatTerakhirRef = useRef("");
  const [depth, setDepth] = useState<Distribusi | null>(null);
  const [scanBusy, setScanBusy] = useState(false);

  const clientRef = useRef<EngineClient | null>(null);
  /** Tab gmgn yang sedang menampilkan chart. Tujuan perintah gambar garis. */
  const gmgnTabRef = useRef<number | null>(null);
  /** Alamat token yang sedang aktif, dipakai saat meneruskan harga chart. */
  const addressRef = useRef<string | null>(null);
  /** Garis terakhir yang dikirim, supaya tidak menggambar ulang tanpa alasan. */
  const drawnRef = useRef<string>("");

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
      onTokenState: (t) => {
        // Satu token bisa mengirim beberapa kali: tahap cepat dari cache,
        // tahap penuh yang membawa statistik pool, lalu susulan saldo.
        // Mereset range pada setiap kiriman akan menghapus garis yang sudah
        // digambar dan rencana yang sudah disusun, padahal tokennya sama.
        // Ref, bukan state: handler ini dibuat sekali dan akan memegang nilai
        // state yang basi selamanya kalau membacanya langsung.
        const alamatBaru = (t.address ?? "").toLowerCase();
        const tokenBerganti = alamatBaru !== alamatTerakhirRef.current;
        alamatTerakhirRef.current = alamatBaru;

        setTokenState(t);
        setTokenLoading(false);

        if (tokenBerganti) {
          setRangeView(null);
          setDepth(null);
          // Rencana lama milik token lama. Membiarkannya berarti menawarkan
          // tombol buka posisi untuk pool yang sudah tidak dilihat lagi.
          setOpenPlan(null);
          setSendResult(null);
          drawnRef.current = "";
        }
      },
      onRangeView: (v) => {
        setRangeView((lama) => {
          // Distribusi diminta sekali per pool, bukan setiap geseran garis.
          // Bentuk pool tidak berubah secepat harganya, dan pembacaannya butuh
          // dua perjalanan ke rantai.
          if (!lama || lama.pool_id !== v.pool_id) {
            setDepth(null);
            clientRef.current?.getDepth();
          }
          return v;
        });
      },
      onSignals: setSignals,
      onScan: (v) => {
        setScan(v);
        setScanBusy(false);
      },
      onDepth: setDepth,
      onActionPlan: (p) => {
        setActionPlan(p);
        setActionBusy(false);
      },
      onPositions: (v) => {
        setPositions(v);
        setPosBusy(false);
      },
      onOpenPlan: (plan) => {
        setOpenPlan(plan);
        setExecBusy(false);
        setActionBusy(false);
      },
      onSendResult: (result) => {
        setSendResult(result);
        setExecBusy(false);
        // Rencana sudah dipakai engine dan dikeluarkan dari slotnya, jadi
        // menampilkannya lagi akan menawarkan tombol yang pasti ditolak.
        if (result.kind === "open") setOpenPlan(null);
        // Setelah zap, saldo berubah sehingga jumlah pada rencana lama tidak
        // lagi tepat. Rencananya dibuang supaya user menyusun ulang.
        if (result.kind === "zap" && result.status === "sukses") setOpenPlan(null);
        if (result.kind === "action") setActionPlan(null);
      },
      onAlert: (level, code, message) => {
        // Kegagalan eksekusi datang sebagai alert, dan tanpa ini tombolnya
        // akan tetap mati sesudahnya.
        setExecBusy(false);
        // Toast di halaman gmgn menangani kejadian yang layak mengganggu user,
        // sesuai PRD 3.7. Yang di sini adalah jawaban langsung atas aksi yang
        // baru saja user tekan, dan itu harus terlihat di tempat ia menekannya.
        setKabar({ level, message });
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

  /**
   * Minta engine memuat sebuah token, dan arahkan tab gmgn ke halamannya.
   *
   * Dipakai dua jalan masuk sekaligus: user menempel alamat di panel, dan user
   * membuka sendiri halaman token di gmgn. Lihat PRD 3.3.
   */
  const loadToken = useCallback((address: string, navigate: boolean) => {
    const clean = address.trim();
    if (!isEvmAddress(clean)) return;

    setTokenLoading(true);
    setTokenState(null);
    setRangeView(null);
    drawnRef.current = "";
    addressRef.current = clean;
    clientRef.current?.subscribeToken(clean);

    if (navigate) {
      void chrome.runtime.sendMessage({
        target: "navigate-gmgn",
        url: gmgnTokenUrl(clean),
      });
    }
  }, []);

  /**
   * Semua kabar dari halaman gmgn masuk lewat satu pintu.
   *
   * Tiga jenis yang ditangani: user membuka halaman token sendiri, chart
   * melaporkan harga terakhirnya, dan user menggeser garis.
   */
  useEffect(() => {
    const listener = (message: unknown) => {
      const m = message as Record<string, unknown> | null;
      if (!m || m.target !== "sidepanel") return;

      if (typeof m.tabId === "number") gmgnTabRef.current = m.tabId;

      switch (m.kind) {
        case "token_page": {
          const url = typeof m.url === "string" ? m.url : "";
          const found = url.match(/\/token\/(0x[a-fA-F0-9]{40})/);
          if (found) loadToken(found[1], false);
          break;
        }
        case "chart_bars": {
          // Deret penuh dipakai detektor sinyal. Dikirim jauh lebih jarang
          // daripada harga, karena isinya ratusan bar.
          const bars = m.bars;
          const address = addressRef.current;
          if (Array.isArray(bars) && address) {
            clientRef.current?.reportChartBars(address, bars as never);
          }
          break;
        }
        case "chart_price": {
          // Kalibrasi kurs bergantung pada angka ini. Diteruskan apa adanya,
          // engine yang memutuskan apakah masuk akal.
          const bar = m.bar as { close?: number } | undefined;
          const address = addressRef.current;
          if (bar && typeof bar.close === "number" && address) {
            clientRef.current?.reportChartPrice(address, bar.close);
          }
          break;
        }
        case "chart_line_moved": {
          // Satu garis bergerak, tapi engine butuh keduanya. Sisi yang tidak
          // bergerak diambil dari tampilan terakhir.
          const side = m.side;
          const price = m.price;
          if (typeof price !== "number" || !latestViewRef.current) break;
          const v = latestViewRef.current;
          const lower = side === "lower" ? price : v.lower.mcap_usd;
          const upper = side === "upper" ? price : v.upper.mcap_usd;
          clientRef.current?.setRange(lower, upper);

          // Garis yang digeser user TIDAK digambar ulang, bahkan setelah
          // geserannya berhenti. Chart sudah menaruhnya persis di tempat
          // kursor dilepas, dan menggesernya ke posisi hasil pembulatan tick
          // terlihat seperti loncatan tanpa sebab. Angka hasil pembulatan
          // tetap muncul di kartu Range, dan di sanalah tempat yang benar
          // untuk menunjukkannya.
          geserTerakhirRef.current = Date.now();
          break;
        }
      }
    };
    chrome.runtime.onMessage.addListener(listener);
    return () => chrome.runtime.onMessage.removeListener(listener);
  }, [loadToken]);

  // Ref supaya listener di atas selalu melihat tampilan terbaru tanpa harus
  // dipasang ulang setiap kali range berubah.
  const latestViewRef = useRef<RangeView | null>(null);
  latestViewRef.current = rangeView;

  /**
   * Gambar garis di chart setiap kali engine mengirim tampilan baru.
   *
   * Perbandingan dengan gambar terakhir mencegah penggambaran ulang saat
   * engine mengirim nilai yang sama, yang terjadi tiap detik karena laporan
   * harga chart datang berkala.
   */
  useEffect(() => {
    if (!rangeView) return;

    // Perubahan yang datang dari chart tidak digambar ulang. Garisnya sudah
    // ada di tempat yang benar menurut user, dan memindahkannya ke hasil
    // pembulatan terlihat seperti loncatan.
    // Jendelanya sengaja melebihi lama satu perjalanan ke engine, supaya
    // kiriman yang masih di jalan saat kursor dilepas ikut tertahan.
    if (Date.now() - geserTerakhirRef.current < JEDA_ABAIKAN_MS) {
      drawnRef.current = `${rangeView.lower.tick}:${rangeView.upper.tick}:${rangeView.pool_id}`;
      return;
    }

    const sidik = `${rangeView.lower.tick}:${rangeView.upper.tick}:${rangeView.pool_id}`;
    if (sidik === drawnRef.current) return;
    drawnRef.current = sidik;

    void gambar(rangeView.lower.mcap_usd, rangeView.upper.mcap_usd);
  }, [rangeView]);

  /**
   * Kirim perintah menggambar ke tab gmgn.
   *
   * KENAPA TAB DICARI ULANG KALAU BELUM DIKETAHUI
   * Nomor tab biasanya datang bersama pesan dari content script. Setelah
   * ekstensi dimuat ulang, content script yang lama kehilangan sambungannya
   * dan tidak mengirim apa apa sampai halamannya dibuka ulang. Selama itu
   * panel tidak tahu tab mana yang harus digambari, dan garis range tidak
   * pernah muncul tanpa satu pun pesan kesalahan.
   *
   * Mencarinya sendiri membuat panel pulih tanpa user perlu tahu apa apa.
   */
  const gambar = useCallback(async (lowerPrice: number, upperPrice: number) => {
    let tabId = gmgnTabRef.current;
    if (tabId === null) {
      try {
        const tabs = await chrome.tabs.query({ url: "https://gmgn.ai/*" });
        const ketemu = tabs.find((t) => typeof t.id === "number");
        if (ketemu?.id !== undefined) {
          tabId = ketemu.id;
          gmgnTabRef.current = tabId;
        }
      } catch {
        // Izin tab tidak ada atau tab tidak bisa dibaca. Bukan kondisi error.
      }
    }
    if (tabId === null) return;

    try {
      await chrome.tabs.sendMessage(tabId, {
        target: "gmgn-chart",
        kind: "draw_lines",
        upperPrice,
        lowerPrice,
      });
    } catch {
      // Content script belum terpasang di tab itu, biasanya karena halamannya
      // belum dibuka ulang setelah ekstensi dimuat ulang. Nomor tab dilupakan
      // supaya percobaan berikutnya mencarinya lagi.
      gmgnTabRef.current = null;
      drawnRef.current = "";
    }
  }, []);

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

      {kabar ? (
        <button
          type="button"
          class={`kabar ${kabar.level}`}
          onClick={() => setKabar(null)}
          title="Klik untuk menutup"
        >
          {kabar.message}
        </button>
      ) : null}

      <div class="content">
        {tab === "range" ? (
          <RangeTab
            status={status}
            state={state}
            detail={stateDetail}
            token={tokenState}
            loadingToken={tokenLoading}
            onLoadToken={loadToken}
            rangeView={rangeView}
            depth={depth}
            onSetRange={(lo, hi) => clientRef.current?.setRange(lo, hi)}
            onSetFullRange={() => clientRef.current?.setFullRange()}
            onMuatDepth={() => clientRef.current?.getDepth()}
            onSelectPool={(id) => clientRef.current?.selectPool(id)}
            onSelectQuote={(q) => clientRef.current?.selectQuote(q)}
            openPlan={openPlan}
            sendResult={sendResult}
            execBusy={execBusy}
            onQuoteRange={(usd) => {
              setExecBusy(true);
              clientRef.current?.quoteRange(usd);
            }}
            onApproveToken={(t) => {
              setExecBusy(true);
              clientRef.current?.approveToken(t);
            }}
            onSwapZap={() => {
              setExecBusy(true);
              clientRef.current?.swapZap();
            }}
            onOpenPosition={() => {
              setExecBusy(true);
              clientRef.current?.openPosition();
            }}
          />
        ) : tab === "signals" ? (
          <SignalsTab
            view={signals}
            scan={scan}
            scanBusy={scanBusy}
            onScan={() => {
              setScanBusy(true);
              clientRef.current?.scan();
            }}
            onSetujuiHook={(h: string) => clientRef.current?.approveHook(h)}
            onPilih={(b) => {
              // Klik baris membuka token itu di gmgn DAN memuat poolnya di tab
              // Range, sesuai PRD 3.4. Berpindah tab dilakukan di sini supaya
              // user langsung melihat hasilnya, bukan harus mencarinya.
              loadToken(b.token, true);
              setTab("range");
            }}
          />
        ) : tab === "pnl" ? (
          <PnlTab
            view={positions}
            busy={posBusy}
            onMuat={() => {
              setPosBusy(true);
              clientRef.current?.getPositions();
            }}
          />
        ) : tab === "settings" ? (
          <SettingsTab
            status={status}
            client={clientRef.current}
            onForget={forgetPairing}
          />
        ) : tab === "positions" ? (
          <PositionsTab
            view={positions}
            busy={posBusy}
            plan={actionPlan}
            actionBusy={actionBusy}
            onMuat={() => {
              setPosBusy(true);
              clientRef.current?.getPositions();
            }}
            onPlanAction={(id, a) => {
              setActionBusy(true);
              setActionPlan(null);
              clientRef.current?.planAction(id, a);
            }}
            onExecuteAction={() => {
              setActionBusy(true);
              clientRef.current?.executeAction();
            }}
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
  token,
  loadingToken,
  onLoadToken,
  rangeView,
  depth,
  onSetRange,
  onSetFullRange,
  onMuatDepth,
  onSelectPool,
  onSelectQuote,
  openPlan,
  sendResult,
  execBusy,
  onQuoteRange,
  onApproveToken,
  onSwapZap,
  onOpenPosition,
}: {
  status: EngineStatus | null;
  state: ConnectionState;
  detail?: string;
  token: TokenState | null;
  loadingToken: boolean;
  onLoadToken: (address: string, navigate: boolean) => void;
  rangeView: RangeView | null;
  depth: Distribusi | null;
  onSetRange: (lowerMcap: number, upperMcap: number) => void;
  onSetFullRange: () => void;
  onMuatDepth: () => void;
  onSelectPool: (poolId: string) => void;
  onSelectQuote: (quote: string) => void;
  openPlan: OpenPlanView | null;
  sendResult: SendResultView | null;
  execBusy: boolean;
  onQuoteRange: (sizeUsd: number) => void;
  onApproveToken: (token: string) => void;
  onSwapZap: () => void;
  onOpenPosition: () => void;
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
      <AddressInput onSubmit={(a) => onLoadToken(a, true)} busy={loadingToken} />

      {loadingToken ? <p class="phase-note">Mencari pool di chain.</p> : null}
      {token ? (
        <>
        <TokenCard
          token={token}
          rangeView={rangeView}
          depth={depth}
          onSetRange={onSetRange}
          onSetFullRange={onSetFullRange}
          onMuatDepth={onMuatDepth}
          onSelectPool={onSelectPool}
          onSelectQuote={onSelectQuote}
        />
        <ExecuteCard
          view={rangeView}
          token={token}
          plan={openPlan}
          result={sendResult}
          busy={execBusy}
          canExecute={status.can_execute}
          blockedReasons={status.blocked_reasons}
          onQuote={onQuoteRange}
          onApprove={onApproveToken}
          onZap={onSwapZap}
          onOpen={onOpenPosition}
        />
        </>
      ) : null}

    </div>
  );
}

/**
 * Tab Settings.
 *
 * Dipisahkan dari tab Range atas permintaan user. Isinya keadaan engine dan
 * hal hal yang diatur sekali lalu jarang disentuh, sedangkan tab Range dipakai
 * setiap kali menaruh posisi. Mencampurnya membuat yang sering dipakai harus
 * digulir melewati yang jarang.
 */
function SettingsTab({
  status,
  client,
  onForget,
}: {
  status: EngineStatus | null;
  client: EngineClient | null;
  onForget: () => void;
}) {
  if (!status) {
    return (
      <div class="stack">
        <section class="card">
          <h2>Settings</h2>
          <p class="muted" style="margin: var(--s2) 0 0">
            Menunggu engine.
          </p>
        </section>
      </div>
    );
  }

  return (
    <div class="stack">
      <ChainCard status={status} />
      <WalletCard status={status} client={client} />
      <RegistryCard status={status} />
      <EngineCard
        status={status}
        onSetRpc={(http) => client?.setRpc(http)}
        onForget={onForget}
      />
    </div>
  );
}

function AddressInput({
  onSubmit,
  busy,
}: {
  onSubmit: (address: string) => void;
  busy: boolean;
}) {
  const [value, setValue] = useState("");
  const valid = isEvmAddress(value);
  const kotor = value.trim().length > 0 && !valid;

  return (
    <section class="card">
      <h2>Token</h2>
      <div class="stack" style="margin-top: var(--s2)">
        <input
          type="text"
          value={value}
          placeholder="Tempel contract address"
          spellcheck={false}
          onInput={(e) => setValue((e.target as HTMLInputElement).value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && valid) onSubmit(value);
          }}
        />
        <button class="primary" disabled={!valid || busy} onClick={() => onSubmit(value)}>
          {busy ? "Mencari" : "Open"}
        </button>
      </div>
      {kotor ? <p class="error">Bukan alamat EVM. Harus 0x diikuti 40 karakter heksadesimal.</p> : null}
      <p class="hint">Membuka halaman token di gmgn juga memuatnya di sini otomatis.</p>
    </section>
  );
}

function TokenCard({
  token,
  rangeView,
  depth,
  onSetRange,
  onSetFullRange,
  onMuatDepth,
  onSelectPool,
  onSelectQuote,
}: {
  token: TokenState;
  rangeView: RangeView | null;
  depth: Distribusi | null;
  onSetRange: (lowerMcap: number, upperMcap: number) => void;
  onSetFullRange: () => void;
  onMuatDepth: () => void;
  onSelectPool: (poolId: string) => void;
  onSelectQuote: (quote: string) => void;
}) {
  const jendelaMenit =
    token.pools.find((p) => p.stats?.jendela_menit != null)?.stats?.jendela_menit ??
    null;

  // Pool aktif ditentukan engine, bukan panel, supaya tidak ada dua sumber
  // kebenaran. Selama tampilan range belum datang, pilihan awal dari
  // token_state yang dipakai.
  const activeId = rangeView?.pool_id ?? token.selected_pool_id;
  const active = token.pools.find((p) => p.id === activeId) ?? token.pools[0];

  if (token.error && token.pools.length === 0) {
    return (
      <section class="card">
        <h2>{token.meta?.symbol ?? shortAddress(token.address)}</h2>
        <p class="error">{token.error}</p>
      </section>
    );
  }

  return (
    <>
      <section class="card">
        <div class="section-title">
          <h2>{token.meta?.symbol ?? "Token"}</h2>
          <span class="chip">{shortAddress(token.address)}</span>
        </div>
        <div class="row">
          <span class="label">Suplai</span>
          <span class="value num">{formatCompact(token.meta?.total_supply ?? 0)}</span>
        </div>
        <div class="row">
          <span class="label">Desimal</span>
          <span class="value num">{token.meta?.decimals ?? "-"}</span>
        </div>
      </section>

      <section class="card">
        <div class="section-title">
          <h2>Pool</h2>
          <span class="chip">{token.pools.length} layak</span>
        </div>

        {/* Jendela disebut supaya angka nol terbaca sebagai tidak ada
            perdagangan, bukan sebagai data yang belum sampai. Keduanya
            terlihat sama persis di layar tanpa keterangan ini. */}
        {jendelaMenit !== null ? (
          <p class="hint" style="margin: var(--s2) 0 0">
            APR dan volume atas {jendelaMenit.toFixed(0)} menit terakhir.
          </p>
        ) : null}

        <QuotePicker
          quotes={token.quotes}
          selected={token.selected_quote}
          onSelect={onSelectQuote}
        />

        <div class="stack" style="margin-top: var(--s2)">
          {token.pools.map((p) => (
            <PoolRow
              key={p.id}
              pool={p}
              active={active?.id === p.id}
              onSelect={() => onSelectPool(p.id)}
            />
          ))}
        </div>

      </section>

      {rangeView ? (
        <>
          <RangeCard view={rangeView} />
          <DepthCard
            view={rangeView}
            depth={depth}
            onSetRange={onSetRange}
            onSetFullRange={onSetFullRange}
            onMuat={onMuatDepth}
          />
        </>
      ) : active ? (
        <section class="card">
          <h2>Range</h2>
          <p class="muted" style="margin: var(--s2) 0 0">
            Menunggu harga dari chart gmgn. Buka halaman token di gmgn supaya
            kurs bisa dikalibrasi.
          </p>
        </section>
      ) : null}
    </>
  );
}

/**
 * Angka besar dalam bentuk yang bisa dibaca sekilas.
 *
 * Panel ini sempit, dan deretan angka penuh berdampingan justru membuat
 * perbandingannya lebih sulit, bukan lebih mudah.
 */
function ringkas(v: number): string {
  const a = Math.abs(v);
  if (a >= 1e9) return `${(v / 1e9).toFixed(2)}B`;
  if (a >= 1e6) return `${(v / 1e6).toFixed(2)}M`;
  if (a >= 1e3) return `${(v / 1e3).toFixed(1)}K`;
  if (a >= 1) return v.toFixed(2);
  return v.toFixed(4);
}

const NAMA_LABEL: Record<LabelScan, string> = {
  lp_sideways: "LP Sideways",
  lp_beli_turun: "LP Beli Turun",
  lp_jual_naik: "LP Jual Naik",
  pantau: "Pantau",
  hindari: "Hindari",
};

/**
 * Daftar pool yang sedang ramai, PRD 3.4.
 *
 * KENAPA ANGKANYA DALAM KUTIPAN, BUKAN DOLAR
 * Setiap angka uang di sini adalah satuan token kutipan pool itu. Mengubahnya
 * ke dolar butuh kurs yang tidak selalu ada, dan kurs yang ditebak akan masuk
 * ke angka yang seharusnya murni hasil penjumlahan. Kolom pasangan menyebutkan
 * satuannya, jadi angkanya tidak pernah tanpa satuan.
 *
 * KENAPA BARIS YANG DITOLAK TETAP TAMPIL
 * Melihat alasan penolakan adalah cara user belajar membaca pasar, dan itu
 * lebih berharga daripada daftar yang hanya berisi kandidat bagus. Yang
 * ditolak tidak bisa diklik, dan alasannya tertulis.
 */
function ScanCard({
  scan,
  busy,
  onScan,
  onPilih,
  onSetujuiHook,
}: {
  scan: ScanView | null;
  busy: boolean;
  onScan: () => void;
  onPilih: (b: BarisScan) => void;
  onSetujuiHook: (hook: string) => void;
}) {
  return (
    <section class="card">
      <div class="section-title">
        <h2>Pool yang sedang ramai</h2>
        <button class="toggle" onClick={onScan} disabled={busy}>
          {busy ? "memindai" : "pindai"}
        </button>
      </div>

      {!scan ? (
        <p class="muted" style="margin: var(--s2) 0 0">
          Belum dipindai. Engine akan membaca setiap swap pada beberapa menit
          terakhir, lalu memeringkatnya menurut hasil fee per modal.
        </p>
      ) : scan.error ? (
        <p class="error" style="margin: var(--s2) 0 0">{scan.error}</p>
      ) : scan.rows.length === 0 ? (
        <p class="muted" style="margin: var(--s2) 0 0">
          Tidak ada pool yang ramai pada jendela ini.
        </p>
      ) : (
        <>
          <div class="scan-list">
            {scan.rows.map((b) => (
              <ScanRow
                key={b.pool_id}
                b={b}
                hook={
                  scan.hooks.find(
                    (h) => h.hook.toLowerCase() === b.hooks?.toLowerCase(),
                  ) ?? null
                }
                adopsiLengkap={scan.adopsi_lengkap}
                onPilih={onPilih}
                onSetujuiHook={onSetujuiHook}
              />
            ))}
          </div>
          <p class="hint">
            Hasil dan APR atas modal {ringkas(scan.modal_acuan)}, jendela{" "}
            {scan.jendela_volume_menit.toFixed(0)} menit.
            {scan.volume_jendela_pendek ? " Jendela pendek, angkanya kasar." : ""}
          </p>
        </>
      )}
    </section>
  );
}

function ScanRow({
  b,
  hook,
  adopsiLengkap,
  onPilih,
  onSetujuiHook,
}: {
  b: BarisScan;
  hook: PenilaianHook | null;
  adopsiLengkap: boolean;
  onPilih: (b: BarisScan) => void;
  onSetujuiHook: (hook: string) => void;
}) {
  const bisa = b.label !== "hindari";
  return (
    <div class={`scan-row${bisa ? "" : " ditolak"}`}>
      <button
        class="scan-head"
        disabled={!bisa}
        onClick={() => bisa && onPilih(b)}
        title={bisa ? "Buka token ini di gmgn dan isi tab Range" : b.alasan}
      >
        <span class="scan-sym">{b.symbol}</span>
        <span class="scan-pair">
          {b.quote_dikenal ? b.quote_symbol : `?${b.quote_symbol}`}
        </span>
        <span class={`scan-label ${b.label}`}>{NAMA_LABEL[b.label]}</span>
      </button>

      <div class="scan-num">
        <span>
          <i>mcap</i>
          {b.mcap === null ? "-" : ringkas(b.mcap)}
        </span>
        <span>
          <i>volume</i>
          {b.volume === null ? "-" : ringkas(b.volume)}
        </span>
        <span>
          <i>modal</i>
          {b.kedalaman === null ? "-" : ringkas(b.kedalaman)}
        </span>
        <span>
          <i>hasil</i>
          {b.hasil_jendela === null ? "-" : `${b.hasil_jendela.toFixed(2)}%`}
        </span>
        <span>
          <i>umur</i>
          {b.umur_jam >= 24
            ? `${(b.umur_jam / 24).toFixed(0)} hari`
            : `${b.umur_jam.toFixed(1)} jam`}
        </span>
        <span>
          <i>fee</i>
          {b.dynamic_fee ? "dinamis" : `${(b.fee / 10_000).toFixed(2)}%`}
        </span>
      </div>

      <p class="scan-alasan">{b.alasan}</p>

      {!b.quote_dikenal && b.label !== "hindari" ? (
        <p class="warn-line">
          Alamat kutipannya bukan yang ada di registry. Simbol bisa ditiru siapa
          saja, jadi periksa alamatnya sebelum menaruh dana.
        </p>
      ) : null}

      {hook ? (
        <HookPanel h={hook} lengkap={adopsiLengkap} onSetujui={onSetujuiHook} />
      ) : null}
    </div>
  );
}

const TANDA_KRITERIA: Record<Kriteria, string> = {
  lolos: "lolos",
  gagal: "tidak",
  belum_terukur: "belum diukur",
};

/**
 * Penilaian hook beserta jalan keluarnya, PRD 5.4.
 *
 * KENAPA SETIAP KRITERIA DITAMPILKAN, BUKAN HANYA KESIMPULANNYA
 * Dua dari lima syarat belum bisa diukur alat ini, sehingga tidak ada hook yang
 * lolos otomatis. Kalau yang tampil hanya kesimpulannya, setiap hook terlihat
 * sama buruknya, dan user tidak punya dasar untuk memutuskan. Yang membedakan
 * hook yang dipakai ribuan pool selama setahun dari hook yang dibuat kemarin
 * justru ada di rinciannya.
 *
 * `belum diukur` sengaja ditulis berbeda dari `tidak`. Yang pertama berarti kita
 * belum tahu, yang kedua berarti kita sudah tahu dan jawabannya tidak.
 */
function HookPanel({
  h,
  lengkap,
  onSetujui,
}: {
  h: PenilaianHook;
  lengkap: boolean;
  onSetujui: (hook: string) => void;
}) {
  const baris: [string, Kriteria][] = [
    ["jumlah pool", h.kriteria_pool],
    ["umur", h.kriteria_umur],
    ["tidak bisa diupgrade", h.kriteria_upgrade],
    ["TVL", h.kriteria_tvl],
    ["LP unik", h.kriteria_lp],
  ];

  return (
    <div class="hook-panel">
      <div class="hook-head">
        <span class="hook-alamat">hook {h.hook.slice(0, 10)}</span>
        {h.jumlah_pool === null ? null : (
          <span class="hook-fakta">
            {h.jumlah_pool} pool
            {h.umur_hari === null ? "" : `, ${h.umur_hari.toFixed(0)} hari`}
          </span>
        )}
      </div>

      <div class="hook-kriteria">
        {baris.map(([nama, k]) => (
          <span key={nama} class={`hook-k ${k}`}>
            {nama}: {TANDA_KRITERIA[k]}
          </span>
        ))}
      </div>

      {lengkap ? null : (
        <p class="scan-alasan">
          Sejarah rantai belum selesai dipindai, jadi jumlah pool dan umur di
          atas adalah batas bawah: angkanya hanya bisa naik, tidak pernah turun.
          Pindai lagi beberapa kali dan keduanya akan mengendap.
        </p>
      )}

      {h.terlarang ? (
        <p class="warn-line">
          Alamatnya proxy yang masih bisa diupgrade, atau pemeriksaannya gagal
          dibaca. Ini penolakan mutlak dan tidak bisa disetujui, karena kode yang
          diperiksa sekarang bukan kode yang akan berjalan nanti.
        </p>
      ) : (
        <>
          <p class="scan-alasan">
            TVL dan jumlah LP unik belum bisa diukur alat ini, jadi hook ini tidak
            bisa lolos otomatis. Anda tetap boleh memakainya sebagai pilihan sadar,
            dan persetujuannya dicatat beserta angka di atas.
          </p>
          <button class="toggle" onClick={() => onSetujui(h.hook)}>
            setujui hook ini
          </button>
        </>
      )}
    </div>
  );
}

/**
 * Tab Signals.
 *
 * Menjawab dua pertanyaan yang berbeda. Detektor menjawab "kapan layak masuk",
 * doktrin penempatan menjawab "di mana garisnya ditaruh". Yang kedua itu yang
 * user nyatakan sebagai kesulitannya sendiri, jadi ia diberi ruang paling besar
 * dan selalu disertai alasannya.
 *
 * Kondisi yang ditolak TIDAK disembunyikan. Melihat alasan penolakan adalah cara
 * user belajar membaca pasar, dan itu jauh lebih berharga daripada daftar yang
 * hanya berisi kandidat bagus.
 */
function SignalsTab({
  view,
  scan,
  scanBusy,
  onScan,
  onPilih,
  onSetujuiHook,
}: {
  view: SignalsView | null;
  scan: ScanView | null;
  scanBusy: boolean;
  onScan: () => void;
  onPilih: (b: BarisScan) => void;
  onSetujuiHook: (hook: string) => void;
}) {
  if (!view) {
    return (
      <div class="stack">
        <ScanCard
          scan={scan}
          busy={scanBusy}
          onScan={onScan}
          onPilih={onPilih}
          onSetujuiHook={onSetujuiHook}
        />
        <section class="card">
          <div class="section-title">
            <h2>Detektor</h2>
          </div>
          <p class="muted" style="margin: var(--s2) 0 0">
            Belum ada data candle. Buka halaman token di gmgn, dan deretnya akan
            terkirim sendiri dalam beberapa detik.
          </p>
          <p class="hint">Bar dibaca dari chart yang sedang Anda lihat.</p>
        </section>
      </div>
    );
  }

  return (
    <div class="stack">
      <ScanCard
        scan={scan}
        busy={scanBusy}
        onScan={onScan}
        onPilih={onPilih}
        onSetujuiHook={onSetujuiHook}
      />
      <section class="card">
        <div class="section-title">
          <h2>Token yang sedang dibuka</h2>
          <span class="chip">{view.bar_terpakai} bar</span>
        </div>
        <div class="row" style="margin-top: var(--s2)">
          <span class="label">Harga sekarang</span>
          <span class="value num">{view.harga.toPrecision(6)}</span>
        </div>
      </section>

      {view.saran ? (
        <SaranCard
          saran={view.saran}
          dibanding={view.konsentrasi_dibanding_lebar}
        />
      ) : (
        <section class="card">
          <div class="section-title">
            <h2>Penempatan</h2>
            <span class="chip">
              <span class="dot warn" />
              belum ada saran
            </span>
          </div>
          <p class="muted" style="margin: var(--s2) 0 0">
            Tidak ada detektor yang lolos, jadi tidak ada saran penempatan.
            Alasannya ada di kartu di bawah.
          </p>
        </section>
      )}

      <SidewaysCard sinyal={view.sideways} />
      <BasisCard hasil={view.basis} />
    </div>
  );
}

/**
 * Saran penempatan beserta alasannya.
 *
 * Alasan ditampilkan penuh, bukan disembunyikan di balik tooltip. Ini bagian
 * yang paling penting bagi user: bukan angkanya, melainkan kenapa angkanya
 * begitu.
 */
function SaranCard({
  saran,
  dibanding,
}: {
  saran: SaranPenempatan;
  dibanding: number | null;
}) {
  const nama =
    saran.bentuk === "dua_sisi"
      ? "dua sisi"
      : saran.bentuk === "beli_turun"
        ? "beli turun"
        : "jual naik";

  return (
    <section class="card">
      <div class="section-title">
        <h2>Penempatan</h2>
        <span class="chip">
          <span class="dot ok" />
          {nama}
        </span>
      </div>

      <div class="stack" style="margin-top: var(--s2)">
        <div class="row">
          <span class="label">Batas bawah</span>
          <span class="value num">{saran.bawah.toPrecision(6)}</span>
        </div>
        <div class="row">
          <span class="label">Batas atas</span>
          <span class="value num">{saran.atas.toPrecision(6)}</span>
        </div>
        <div class="row">
          <span class="label">Lebar</span>
          <span class="value num">{saran.lebar_persen.toFixed(1)}%</span>
        </div>
        {dibanding !== null ? (
          <div class="row">
            <span class="label">Konsentrasi</span>
            <span class="value num naik">
              {dibanding.toFixed(1)}x range lima kali lebih lebar
            </span>
          </div>
        ) : null}
        {saran.pembatalan !== null ? (
          <div class="row">
            <span class="label">Pembatalan</span>
            <span class="value num turun">{saran.pembatalan.toPrecision(6)}</span>
          </div>
        ) : null}
      </div>

      <div class="stack" style="margin-top: var(--s3)">
        {saran.alasan.map((a, i) => (
          <p key={i} class="hint" style="margin: 0">
            {a}
          </p>
        ))}
      </div>
    </section>
  );
}

function SidewaysCard({ sinyal }: { sinyal: SinyalSideways | null }) {
  if (!sinyal) {
    return (
      <section class="card">
        <div class="section-title">
          <h2>Sideways</h2>
          <span class="chip">data belum cukup</span>
        </div>
        <p class="hint">Butuh 240 bar, empat jam pada candle satu menit.</p>
      </section>
    );
  }

  const lolos = sinyal.ditolak.length === 0;
  return (
    <section class="card">
      <div class="section-title">
        <h2>Sideways</h2>
        <span class="chip">
          <span class={`dot ${lolos ? "ok" : "bad"}`} />
          skor {sinyal.skor.toFixed(0)}
        </span>
      </div>

      <div class="stack" style="margin-top: var(--s2)">
        <BarisMetrik nama="Efficiency ratio" nilai={sinyal.er.toFixed(2)} ket="makin kecil makin menyamping" />
        <BarisMetrik nama="Volume z score" nilai={sinyal.volume_z.toFixed(2)} ket="seberapa ramai dibanding biasanya" />
        <BarisMetrik nama="Containment" nilai={`${(sinyal.containment * 100).toFixed(0)}%`} ket="bar yang tutup di dalam kanal" />
        <BarisMetrik nama="Lebar kanal" nilai={`${(sinyal.bandwidth * 100).toFixed(1)}%`} ket="relatif terhadap harga tengah" />
        <BarisMetrik nama="Kontraksi" nilai={sinyal.kontraksi.toFixed(2)} ket="volatilitas pendek dibanding panjang" />
        <BarisMetrik nama="Keseimbangan tape" nilai={sinyal.tape.toFixed(2)} ket="makin besar makin seimbang" />
      </div>

      {!lolos ? (
        <div class="stack" style="margin-top: var(--s3)">
          {sinyal.ditolak.map((d, i) => (
            <p key={i} class="hint" style="margin: 0">
              {d}
            </p>
          ))}
        </div>
      ) : null}
    </section>
  );
}

function BasisCard({ hasil }: { hasil: HasilBasis | null }) {
  if (!hasil) {
    return (
      <section class="card">
        <div class="section-title">
          <h2>Dump lalu basis</h2>
          <span class="chip">data belum cukup</span>
        </div>

      </section>
    );
  }

  const lolos = hasil.gagal.length === 0;
  return (
    <section class="card">
      <div class="section-title">
        <h2>Dump lalu basis</h2>
        <span class="chip">
          <span class={`dot ${lolos ? "ok" : "bad"}`} />
          skor {hasil.skor.toFixed(0)}
        </span>
      </div>

      <div class="stack" style="margin-top: var(--s2)">
        <BarisMetrik nama="Kedalaman" nilai={`${(hasil.drawdown * 100).toFixed(0)}%`} ket="dari puncak ke dasar" />
        <BarisMetrik nama="Bar turun" nilai={String(hasil.bar_turun)} ket="makin sedikit makin tajam" />
        <BarisMetrik nama="Usia basis" nilai={`${hasil.usia_basis} bar`} ket="sejak dasar terbentuk" />
        <BarisMetrik nama="Volume basis" nilai={`${hasil.rasio_volume_hidup.toFixed(1)}x`} ket="dibanding sebelum pump" />
        <BarisMetrik nama="Volume mereda" nilai={`${hasil.rasio_volume_mereda.toFixed(2)}x`} ket="dibanding puncak dump" />
      </div>

      {lolos ? (
        <div class="stack" style="margin-top: var(--s3)">
          <div class="row">
            <span class="label">Dasar basis</span>
            <span class="value num">{hasil.jangkar.l_basis.toPrecision(6)}</span>
          </div>
          <div class="row">
            <span class="label">Harga paling ramai</span>
            <span class="value num">{hasil.jangkar.poc.toPrecision(6)}</span>
          </div>
          <div class="row">
            <span class="label">Pembatalan</span>
            <span class="value num turun">{hasil.jangkar.p_invalid.toPrecision(6)}</span>
          </div>
        </div>
      ) : (
        <div class="stack" style="margin-top: var(--s3)">
          {hasil.gagal.map((g, i) => (
            <p key={i} class="hint" style="margin: 0">
              {g}
            </p>
          ))}
        </div>
      )}

      {hasil.penguat.length > 0 ? (
        <div class="stack" style="margin-top: var(--s2)">
          {hasil.penguat.map((p, i) => (
            <p key={i} class="hint" style="margin: 0">
              Penguat: {p}
            </p>
          ))}
        </div>
      ) : null}
    </section>
  );
}

/**
 * Satu baris metrik beserta keterangannya.
 *
 * Keterangan ikut karena angka seperti efficiency ratio tidak berarti apa apa
 * tanpa tahu arah mana yang bagus.
 */
function BarisMetrik({ nama, nilai, ket }: { nama: string; nilai: string; ket: string }) {
  return (
    <div class="row" title={ket}>
      <span class="label">{nama}</span>
      <span class="value num">{nilai}</span>
    </div>
  );
}

/**
 * Tab PnL.
 *
 * KENAPA KOMPONENNYA SELALU DIPISAH
 * Fee kotor menipu. Posisi yang mengumpulkan fee besar bisa tetap rugi setelah
 * impermanent loss, gas, dan slippage. Kalau kartunya hanya menampilkan satu
 * angka, kesalahan itu akan terulang karena tidak pernah terlihat. PRD 11
 * menyebutnya eksplisit.
 *
 * Angka datang dari engine apa adanya. Panel tidak menjumlahkan apa pun sendiri,
 * karena dua tempat yang menjumlahkan hal yang sama pasti menyimpang.
 */
function PnlTab({
  view,
  busy,
  onMuat,
}: {
  view: PositionsView | null;
  busy: boolean;
  onMuat: () => void;
}) {
  if (!view) {
    return (
      <div class="stack">
        <section class="card">
          <div class="section-title">
            <h2>PnL</h2>
          </div>
          <p class="muted" style="margin: var(--s2) 0 0">
            Belum dimuat. Angka PnL dihitung dari posisi yang sedang berjalan.
          </p>
          <button
            type="button"
            class="primary"
            style="margin-top: var(--s3); width: 100%"
            disabled={busy}
            onClick={onMuat}
          >
            {busy ? "Membaca rantai" : "Muat posisi"}
          </button>
        </section>
      </div>
    );
  }

  const r = view.ringkasan;
  const berhasil = view.positions.filter((p) => p.pnl !== null);
  const tanpaSatuan = view.positions.length - berhasil.length;

  return (
    <div class="stack">
      <section class="card">
        <div class="section-title">
          <h2>Ringkasan</h2>
          <span class="chip">{r.jumlah} posisi</span>
        </div>

        <div class="stack" style="margin-top: var(--s2)">
          <div class="row">
            <span class="label">Net</span>
            <span class={`value num ${r.total_net >= 0 ? "naik" : "turun"}`}>
              {angka(r.total_net)}
            </span>
          </div>
          <div class="row">
            <span class="label">Fee diperoleh</span>
            <span class="value num">{angka(r.total_fee)}</span>
          </div>
          <div class="row">
            <span class="label">Total biaya</span>
            <span class="value num">{angka(r.total_biaya)}</span>
          </div>
          <div class="row">
            <span class="label">Win rate</span>
            <span class="value num">{(r.win_rate_bps / 100).toFixed(1)}%</span>
          </div>
        </div>

        {r.perkiraan > 0 ? (
          <p class="hint">
            {r.perkiraan} dari {r.jumlah} perkiraan: dibuka di luar Rangefinder,
            jadi impermanent loss-nya tidak tercatat.
          </p>
        ) : null}

        {tanpaSatuan > 0 ? (
          <p class="hint">
            {tanpaSatuan} posisi tanpa satuan: kedua sisinya memecoin.
          </p>
        ) : null}

        <button
          type="button"
          style="margin-top: var(--s3); width: 100%"
          disabled={busy}
          onClick={onMuat}
        >
          {busy ? "Membaca rantai" : "Muat ulang"}
        </button>
      </section>

      {berhasil.map((p) => (
        <PnlCard key={`${p.version}:${p.token_id}`} pos={p} />
      ))}
    </div>
  );
}

function PnlCard({ pos }: { pos: Posisi }) {
  const h = pos.pnl;
  if (!h) return null;

  return (
    <section class="card">
      <div class="section-title">
        <h2>
          {pos.version.toUpperCase()} #{pos.token_id}
        </h2>
        <span class="chip">
          <span class={`dot ${h.net >= 0 ? "ok" : "bad"}`} />
          {h.perkiraan ? "perkiraan" : "tercatat"}
        </span>
      </div>

      <div class="stack" style="margin-top: var(--s2)">
        <div class="row">
          <span class="label">Fee diperoleh</span>
          <span class="value num naik">{angka(h.fee)}</span>
        </div>
        <div class="row">
          <span class="label">Impermanent loss</span>
          <span class="value num turun">{angka(-h.impermanent_loss)}</span>
        </div>
        <div class="row">
          <span class="label">Gas</span>
          <span class="value num turun">{angka(-h.gas)}</span>
        </div>
        <div class="row">
          <span class="label">Slippage zap</span>
          <span class="value num turun">{angka(-h.slippage)}</span>
        </div>
        <div class="row pnl-net">
          <span class="label">Net</span>
          <span class={`value num ${h.net >= 0 ? "naik" : "turun"}`}>{angka(h.net)}</span>
        </div>
      </div>
    </section>
  );
}

/**
 * Angka hasil, dalam satuan token kutipan pool.
 *
 * Bukan dolar, dan tidak pernah ditulis sebagai dolar. Pada pool berkutipan
 * stablecoin keduanya memang berdekatan, tapi menyebutnya dolar berarti
 * mengklaim ketepatan yang tidak kita punya.
 */
function angka(v: number): string {
  if (!Number.isFinite(v)) return "?";
  const tanda = v > 0 ? "+" : "";
  return `${tanda}${v.toLocaleString("id-ID", {
    minimumFractionDigits: 2,
    maximumFractionDigits: 4,
  })}`;
}

/**
 * Tab Positions.
 *
 * Menampilkan semua posisi LP milik wallet, termasuk yang dibuka di luar
 * Rangefinder. Itu permintaan user yang eksplisit, dan konsekuensinya tidak ada
 * bagian di sini yang boleh bergantung pada catatan lokal untuk sekadar
 * menampilkan sebuah posisi.
 *
 * Yang keluar range naik ke puncak, karena itu yang butuh tindakan. Urutannya
 * ditentukan engine, bukan di sini, supaya tidak ada dua tempat yang mengurutkan
 * hal yang sama.
 */
function PositionsTab({
  view,
  busy,
  plan,
  actionBusy,
  onMuat,
  onPlanAction,
  onExecuteAction,
}: {
  view: PositionsView | null;
  busy: boolean;
  plan: ActionPlanView | null;
  actionBusy: boolean;
  onMuat: () => void;
  onPlanAction: (tokenId: string, action: JenisAksi) => void;
  onExecuteAction: () => void;
}) {
  if (!view) {
    return (
      <div class="stack">
        <section class="card">
          <div class="section-title">
            <h2>Positions</h2>
          </div>
          <p class="muted" style="margin: var(--s2) 0 0">
            Belum dimuat. Tekan tombol di bawah untuk membaca posisi dari rantai.
          </p>
          <button
            type="button"
            class="primary"
            style="margin-top: var(--s3); width: 100%"
            disabled={busy}
            onClick={onMuat}
          >
            {busy ? "Membaca rantai" : "Muat posisi"}
          </button>
        </section>
      </div>
    );
  }

  return (
    <div class="stack">
      <section class="card">
        <div class="section-title">
          <h2>Positions</h2>
          <span class="chip">
            {view.keluar_range > 0 ? (
              <>
                <span class="dot bad" />
                {view.keluar_range} keluar range
              </>
            ) : (
              <>
                <span class="dot ok" />
                {view.positions.length} posisi
              </>
            )}
          </span>
        </div>

        {view.error ? (
          <p class="muted" style="margin: var(--s2) 0 0">
            {view.error}
          </p>
        ) : null}

        <div class="row" style="margin-top: var(--s2)">
          <span class="label">Wallet</span>
          <span class="value num">{shortAddress(view.owner)}</span>
        </div>

        <button
          type="button"
          style="margin-top: var(--s3); width: 100%"
          disabled={busy}
          onClick={onMuat}
        >
          {busy ? "Membaca rantai" : "Muat ulang"}
        </button>
      </section>

      {view.positions.map((p) => (
        <PositionCard
          key={`${p.version}:${p.token_id}`}
          pos={p}
          plan={plan?.token_id === p.token_id ? plan : null}
          busy={actionBusy}
          onPlanAction={onPlanAction}
          onExecuteAction={onExecuteAction}
        />
      ))}

      {view.positions.length === 0 && !view.error ? (
        <section class="card">
          <p class="muted" style="margin: 0">
            Tidak ada posisi dalam jendela pemindaian. Posisi V4 ditemukan lewat
            log transfer, jadi yang lebih tua dari tiga juta blok baru muncul
            setelah sekali ketemu.
          </p>
        </section>
      ) : null}
    </div>
  );
}

/**
 * Satu kartu posisi.
 *
 * Bar posisi harga di dalam range memakai warna datar, bukan gradient, sesuai
 * aturan desain proyek.
 */
function PositionCard({
  pos,
  plan,
  busy,
  onPlanAction,
  onExecuteAction,
}: {
  pos: Posisi;
  plan: ActionPlanView | null;
  busy: boolean;
  onPlanAction: (tokenId: string, action: JenisAksi) => void;
  onExecuteAction: () => void;
}) {
  const porsi = porsiDalamRange(pos);
  const nada = pos.kosong ? "" : pos.in_range ? "ok" : "bad";
  const status = pos.kosong ? "kosong" : pos.in_range ? "di dalam" : "di luar";

  return (
    <section class="card">
      <div class="section-title">
        <h2>
          {pos.version.toUpperCase()} #{pos.token_id}
        </h2>
        <span class="chip">
          <span class={`dot ${nada}`} />
          {status}
        </span>
      </div>

      <div class="pos-bar" title={`Harga berada di ${(porsi * 100).toFixed(0)}% lebar range`}>
        <div class="pos-bar-isi" style={`left: ${(porsi * 100).toFixed(2)}%`} />
      </div>

      <div class="stack" style="margin-top: var(--s2)">
        <div class="row">
          <span class="label">Fee tier</span>
          <span class="value num">{(pos.fee / 10_000).toFixed(2)}%</span>
        </div>
        <div class="row">
          <span class="label">Tick</span>
          <span class="value num">
            {pos.tick_lower} sampai {pos.tick_upper}
          </span>
        </div>
        <div class="row">
          <span class="label">Likuiditas</span>
          <span class="value num">{pos.liquidity}</span>
        </div>
        <div class="row">
          <span class="label">Fee belum diklaim</span>
          <span class="value num">
            {pos.fee0} dan {pos.fee1}
          </span>
        </div>
      </div>

      {pos.hooks && pos.hooks !== "0x0000000000000000000000000000000000000000" ? (
        <p class="hint">Hook {shortAddress(pos.hooks)}, belum diperiksa.</p>
      ) : null}

      <div class="stack" style="margin-top: var(--s3)">
        <div class="row" style="gap: var(--s2)">
          <button
            type="button"
            style="flex: 1"
            disabled={busy || !punyaFee(pos)}
            onClick={() => onPlanAction(pos.token_id, "klaim_fee")}
          >
            Klaim fee
          </button>
          <button
            type="button"
            style="flex: 1"
            disabled={busy}
            onClick={() => onPlanAction(pos.token_id, "tutup_posisi")}
          >
            Tutup posisi
          </button>
        </div>

        {plan ? <ActionPlanCard plan={plan} busy={busy} onKirim={onExecuteAction} /> : null}
      </div>

    </section>
  );
}

/**
 * Hasil pemeriksaan sebuah aksi, ditampilkan sebelum tombol konfirmasi aktif.
 *
 * PRD 3.5 menuntut simulasi ditampilkan lebih dulu. `siap` datang dari engine,
 * bukan disimpulkan di sini, karena panel yang menyimpulkan sendiri akan
 * menyimpang begitu ada gate baru ditambahkan.
 */
function ActionPlanCard({
  plan,
  busy,
  onKirim,
}: {
  plan: ActionPlanView;
  busy: boolean;
  onKirim: () => void;
}) {
  return (
    <div class="stack" style="border-top: 1px solid var(--border); padding-top: var(--s2)">
      {plan.gates.map((g) => (
        <div key={g.id} class="row">
          <span class="label">
            <span class={`dot ${g.passed ? "ok" : "bad"}`} /> {g.id} {g.name}
          </span>
          <span class="value muted">{g.detail}</span>
        </div>
      ))}
      <div class="row">
        <span class="label">Gas perkiraan</span>
        <span class="value num">{plan.gas_estimate.toLocaleString("id-ID")}</span>
      </div>

      <button
        type="button"
        class="primary"
        style="width: 100%"
        disabled={!plan.siap || busy}
        onClick={onKirim}
      >
        {plan.dry_run
          ? "Dry run aktif, tidak akan dikirim"
          : plan.siap
            ? `Konfirmasi ${plan.nama}`
            : "Ada pemeriksaan yang gagal"}
      </button>

      {plan.menutup ? (
        <p class="hint">Posisi ditutup habis dan NFT-nya dibakar.</p>
      ) : null}
    </div>
  );
}

/**
 * Benar kalau posisinya punya fee yang belum diklaim.
 *
 * Tombol klaim dimatikan kalau tidak ada, karena mengirimnya tetap berhasil
 * tapi hanya membakar gas untuk memindahkan nol.
 */
function punyaFee(p: Posisi): boolean {
  return p.fee0 !== "0" || p.fee1 !== "0";
}
/**
 * Seberapa jauh harga sekarang di dalam range, sebagai porsi 0 sampai 1.
 *
 * Disalin dari engine dengan sengaja: ini murni untuk menggambar, dan tidak ada
 * satu pun keputusan yang bergantung padanya. Nilai yang dipakai untuk
 * keputusan selalu datang dari engine.
 */
function porsiDalamRange(p: Posisi): number {
  if (p.tick_sekarang === null || p.tick_upper <= p.tick_lower) return 0.5;
  const lebar = p.tick_upper - p.tick_lower;
  return Math.min(1, Math.max(0, (p.tick_sekarang - p.tick_lower) / lebar));
}

/**
 * Kartu eksekusi: susun rencana, beri izin, buka posisi.
 *
 * Tiga langkah sengaja terpisah dan berurutan. Menggabungkannya jadi satu
 * tombol berarti user menyetujui sesuatu yang belum pernah ia lihat, dan yang
 * disetujui di sini memindahkan uang sungguhan.
 *
 * Panel tidak menyimpulkan apa pun sendiri. Boleh tidaknya mengirim, izin mana
 * yang kurang, dan gate mana yang gagal, semuanya datang dari engine. Panel
 * hanya menampilkan dan meneruskan tekanan tombol.
 */
/**
 * Setoran dua sisi, mengikuti tata letak acuan MMT Finance.
 *
 * Dua kotak yang selalu terlihat, masing masing dengan nama token, nilainya
 * dalam USD, dan saldo wallet. Di bawahnya satu tombol besar yang labelnya
 * menyatakan keadaan, lalu baris estimasi APR.
 *
 * KENAPA MENGISI SATU KOTAK MENGISI YANG LAIN
 * Rasio antara kedua sisi ditentukan range, bukan oleh user. Range yang
 * seluruhnya di bawah harga hanya menerima kutipan, yang seluruhnya di atas
 * hanya menerima memecoin, dan yang mengapit harga menerima keduanya menurut
 * perbandingan yang sudah tertentu. Engine yang menghitungnya, dan angka yang
 * muncul setelah rencana tersusun adalah angka yang benar.
 *
 * KENAPA SALDO DITAMPILKAN
 * Rencana yang tidak mungkin dijalankan lebih baik ketahuan sebelum gate
 * berjalan. Saldo yang tidak terbaca ditulis sebagai garis, bukan nol, karena
 * wallet yang belum dibuka bukan wallet yang kosong.
 */
function SetoranCard({
  plan,
  view,
  token,
  setSize,
  busy,
  canExecute,
  onQuote,
}: {
  plan: OpenPlanView | null;
  view: RangeView | null;
  token: TokenState | null;
  setSize: (v: string) => void;
  busy: boolean;
  canExecute: boolean;
  onQuote: (sizeUsd: number) => void;
}) {
  const setor = plan?.setor ?? null;
  const saldo = token?.saldo ?? null;

  const simbolMeme = setor?.simbol_memecoin ?? token?.meta?.symbol ?? "token";
  const kelompok = token?.quotes?.find(
    (q) => q.quote.toLowerCase() === (token?.selected_quote ?? "").toLowerCase(),
  );
  const simbolQuote = setor?.simbol_quote ?? kelompok?.quote_symbol ?? "kutipan";

  // `eth_usd` adalah harga satu token kutipan, berlaku untuk kutipan apa pun.
  const hargaMeme = view?.current.price_usd ?? 0;
  const hargaQuote = view?.eth_usd ?? 0;

  const [isiMeme, setIsiMeme] = useState("");
  const [isiQuote, setIsiQuote] = useState("");
  const [zap, setZap] = useState(true);
  const [sisiAktif, setSisiAktif] = useState<"meme" | "quote">("quote");

  // Kotak diisi ulang dari rencana begitu ia datang, karena angka engine yang
  // benar, bukan yang diketik. Sisi yang sedang disunting tidak ditimpa.
  const jm = setor?.jumlah_memecoin;
  const jq = setor?.jumlah_quote;
  useEffect(() => {
    if (jm === undefined || jq === undefined) return;
    if (sisiAktif !== "meme") setIsiMeme(String(jm));
    if (sisiAktif !== "quote") setIsiQuote(String(jq));
  }, [jm, jq]);

  const susun = (sisi: "meme" | "quote") => {
    const teks = sisi === "meme" ? isiMeme : isiQuote;
    const harga = sisi === "meme" ? hargaMeme : hargaQuote;
    const n = Number(teks);
    if (!Number.isFinite(n) || n <= 0 || harga <= 0) return;
    const usd = n * harga;
    setSize(String(usd));
    onQuote(usd);
  };

  const cukup =
    saldo === null ||
    setor === null ||
    (saldo.memecoin >= setor.jumlah_memecoin && saldo.quote >= setor.jumlah_quote);

  const adaIsi = Number(isiMeme) > 0 || Number(isiQuote) > 0;
  const label = !canExecute
    ? "Engine belum siap"
    : busy
      ? "Menyusun"
      : !adaIsi
        ? "Isi jumlahnya"
        : !cukup
          ? "Saldo kurang"
          : "Susun rencana";

  return (
    <div class="setoran">
      <div class="section-title">
        <h3>Set deposit amounts</h3>
        <button
          type="button"
          class={`sakelar${zap ? " nyala" : ""}`}
          onClick={() => setZap(!zap)}
          aria-pressed={zap}
        >
          <span class="sakelar-tuas" />
          <span class="sakelar-teks">Zap in</span>
        </button>
      </div>

      {/* Zap menyala berarti satu aset saja: engine menukar sebagiannya
          sendiri supaya cocok rasio range. Kotak sisi yang lain disembunyikan,
          bukan dinonaktifkan, karena kotak mati yang tetap terlihat mengundang
          pertanyaan kenapa ia tidak bisa diisi. */}
      {zap && sisiAktif !== "meme" ? null : (
      <KotakSetor
        simbol={simbolMeme}
        nilai={isiMeme}
        usd={Number(isiMeme) * hargaMeme}
        saldo={saldo ? saldo.memecoin : null}
        onInput={(v) => {
          setSisiAktif("meme");
          setIsiMeme(v);
        }}
        onSelesai={() => susun("meme")}
      />
      )}

      {zap && sisiAktif !== "quote" ? null : (
      <KotakSetor
        simbol={simbolQuote}
        nilai={isiQuote}
        usd={Number(isiQuote) * hargaQuote}
        saldo={saldo ? saldo.quote : null}
        onInput={(v) => {
          setSisiAktif("quote");
          setIsiQuote(v);
        }}
        onSelesai={() => susun("quote")}
      />
      )}

      {zap ? (
        <div class="setor-sisi" style="margin-top: var(--s2)">
          <button
            class={`preset${sisiAktif === "meme" ? " aktif" : ""}`}
            onClick={() => setSisiAktif("meme")}
          >
            setor {simbolMeme}
          </button>
          <button
            class={`preset${sisiAktif === "quote" ? " aktif" : ""}`}
            onClick={() => setSisiAktif("quote")}
          >
            setor {simbolQuote}
          </button>
        </div>
      ) : null}

      {zap && setor ? (
        <p class="hint">
          Engine menukar sebagiannya supaya cocok rasio range. Hasilnya kira kira{" "}
          {formatCompact(setor.jumlah_memecoin)} {simbolMeme} dan{" "}
          {formatCompact(setor.jumlah_quote)} {simbolQuote}.
        </p>
      ) : null}

      <button
        class="lebar utama"
        disabled={!canExecute || busy || !adaIsi}
        onClick={() => susun(sisiAktif)}
      >
        {label}
      </button>

      <div class="row" style="margin-top: var(--s2)">
        <span class="label">Perkiraan gas</span>
        <span class="value num">
          {plan?.gas_biaya == null
            ? "-"
            : `${plan.gas_biaya.toFixed(6)} ETH`}
        </span>
      </div>

      <div class="row">
        <span class="label">Estimasi APR</span>
        <span class="value num">
          {plan?.apr_estimasi == null
            ? "-"
            : Math.round(plan.apr_estimasi).toLocaleString("id-ID") + "%"}
        </span>
      </div>

      {plan?.zap && !zap ? (
        <p class="warn-line">
          Tanpa zap, sebagian modal menganggur tanpa menghasilkan fee.
        </p>
      ) : null}

      {saldo === null ? <p class="hint">Buka wallet untuk melihat saldo.</p> : null}
    </div>
  );
}

/** Satu kotak jumlah: nama token, angka besar, nilai USD, dan saldo. */
function KotakSetor({
  simbol,
  nilai,
  usd,
  saldo,
  onInput,
  onSelesai,
}: {
  simbol: string;
  nilai: string;
  usd: number;
  saldo: number | null;
  onInput: (v: string) => void;
  onSelesai: () => void;
}) {
  return (
    <div class="kotak-setor">
      <div class="kotak-atas">
        <span class="kotak-nama">{simbol}</span>
        <span class="kotak-saldo num">
          {saldo === null ? "saldo -" : "saldo " + formatCompact(saldo)}
        </span>
      </div>
      <input
        class="kotak-input num"
        type="number"
        min="0"
        step="any"
        placeholder="0"
        value={nilai}
        onInput={(e) => onInput((e.target as HTMLInputElement).value)}
        onBlur={onSelesai}
        onKeyDown={(e) => {
          if (e.key === "Enter") onSelesai();
        }}
      />
      <span class="kotak-usd num">
        {Number.isFinite(usd) && usd > 0 ? formatUsd(usd) : "$0"}
      </span>
    </div>
  );
}
function ExecuteCard({
  view,
  token,
  plan,
  result,
  busy,
  canExecute,
  blockedReasons,
  onQuote,
  onApprove,
  onZap,
  onOpen,
}: {
  view: RangeView | null;
  token: TokenState | null;
  plan: OpenPlanView | null;
  result: SendResultView | null;
  busy: boolean;
  canExecute: boolean;
  blockedReasons: string[];
  onQuote: (sizeUsd: number) => void;
  onApprove: (token: string) => void;
  onZap: () => void;
  onOpen: () => void;
}) {
  // Nominal terakhir yang dikirim, disimpan supaya kartu setoran bisa
  // menyebutkannya kembali tanpa menghitung ulang dari kedua kotak.
  const [, setSize] = useState("");

  // Engine sudah menyaring: daftar ini hanya berisi langkah yang memang
  // masih kurang. Panel tidak menyaring ulang, karena dua tempat yang
  // memutuskan hal yang sama pasti menyimpang.
  const kurang = plan?.allowances ?? [];
  const gateGagal = plan?.gates.filter((g) => !g.passed) ?? [];
  // Zap yang belum dijalankan menahan pembukaan posisi. Membukanya sekarang
  // tetap berhasil, tapi sebagian modal akan menganggur tanpa menghasilkan
  // fee apa pun, dan itu kerugian yang tidak terlihat di layar mana pun.
  const siapKirim =
    plan !== null &&
    !plan.dry_run &&
    gateGagal.length === 0 &&
    kurang.length === 0 &&
    plan.zap === null;

  return (
    <section class="card">
      <div class="section-title">
        <h2>Eksekusi</h2>
        <span class="chip">
          {plan ? plan.version.toUpperCase() : "belum ada rencana"}
          {plan?.dry_run ? " · dry run" : ""}
        </span>
      </div>

      {!canExecute ? (
        <div class="stack" style="margin-top: var(--s2)">
          {blockedReasons.map((r) => (
            <p key={r} class="muted" style="margin: 0">
              {r}
            </p>
          ))}
        </div>
      ) : null}

      <SetoranCard
        plan={plan}
        view={view}
        token={token}
        setSize={setSize}
        busy={busy}
        canExecute={canExecute}
        onQuote={onQuote}
      />

      {plan ? (
        <>
          <div class="stack" style="margin-top: var(--s3)">
            <div class="row">
              <span class="label">Likuiditas</span>
              <span class="value num">{plan.liquidity}</span>
            </div>
            <div class="row">
              <span class="label">Tick</span>
              <span class="value num">
                {plan.tick_lower} sampai {plan.tick_upper}
              </span>
            </div>
            <div class="row">
              <span class="label">Gas perkiraan</span>
              <span class="value num">{plan.gas_estimate.toLocaleString("id-ID")}</span>
            </div>
          </div>

          <div class="stack" style="margin-top: var(--s3)">
            {plan.gates.map((g) => (
              <div key={g.id} class="row">
                <span class="label">
                  <span class={`dot ${g.passed ? "ok" : "bad"}`} /> {g.id} {g.name}
                </span>
                <span class="value muted">{g.detail}</span>
              </div>
            ))}
          </div>

          {plan.warnings.length > 0 ? (
            <div class="stack" style="margin-top: var(--s2)">
              {plan.warnings.map((w) => (
                <p key={w} class="hint">
                  {w}
                </p>
              ))}
            </div>
          ) : null}

          {kurang.length > 0 ? (
            <div class="stack" style="margin-top: var(--s3)">
              <p class="hint">
                {plan.version === "v4"
                  ? "V4 menarik token lewat Permit2, jadi izinnya dua lapis. Kerjakan berurutan, satu tombol satu transaksi."
                  : "Izin ERC20 belum cukup. Diberikan sebesar yang dibutuhkan saja, bukan tak terbatas."}
              </p>
              {kurang.map((a) => (
                <button
                  key={`${a.jenis}:${a.token}`}
                  type="button"
                  disabled={busy}
                  onClick={() => onApprove(a.token)}
                >
                  {a.label}
                </button>
              ))}
            </div>
          ) : null}

          {plan.zap ? (
            <div class="stack" style="margin-top: var(--s3)">
              <div class="row">
                <span class="label">Zap</span>
                <span class="value num">
                  {(plan.zap.porsi_bps / 100).toFixed(1)}% ditukar
                </span>
              </div>
              <div class="row">
                <span class="label">Perkiraan diterima</span>
                <span class="value num">{plan.zap.perkiraan_diterima}</span>
              </div>
              <div class="row">
                <span class="label">Minimum diterima</span>
                <span class="value num">{plan.zap.minimum_diterima}</span>
              </div>
              <p class="hint">
                Modal belum sesuai rasio range, jadi sebagian perlu ditukar
                lebih dulu. Tanpa ini {plan.zap.menganggur_tanpa_zap} satuan
                akan menganggur tanpa menghasilkan fee.
              </p>
              <button type="button" disabled={busy || kurang.length > 0} onClick={onZap}>
                Tukar {shortAddress(plan.zap.dari)} ke {shortAddress(plan.zap.ke)}
              </button>
              <p class="hint">
                Setelah swap selesai, susun rencana lagi supaya jumlahnya
                dihitung ulang dari saldo yang baru.
              </p>
            </div>
          ) : null}

          <button
            type="button"
            class="primary"
            style="margin-top: var(--s3); width: 100%"
            disabled={!siapKirim || busy}
            onClick={onOpen}
          >
            {plan.dry_run
              ? "Dry run aktif, tidak akan dikirim"
              : plan.zap
                ? "Jalankan zap dulu"
                : "Buka posisi"}
          </button>

          <p class="hint">Rencana berlaku sampai {waktuSingkat(plan.expires_at)}.</p>
        </>
      ) : null}

      {result ? <SendResultRow result={result} /> : null}
    </section>
  );
}

function SendResultRow({ result }: { result: SendResultView }) {
  const nada =
    result.status === "sukses" ? "ok" : result.status === "revert" ? "bad" : "warn";
  return (
    <div class="stack" style="margin-top: var(--s3)">
      <div class="row">
        <span class="label">
          <span class={`dot ${nada}`} /> {result.kind}
        </span>
        <span class="value num">{shortAddress(result.tx_hash)}</span>
      </div>
      <div class="row">
        <span class="label">Status</span>
        <span class="value">{result.status}</span>
      </div>
      {result.gas_used !== null ? (
        <div class="row">
          <span class="label">Gas terpakai</span>
          <span class="value num">{result.gas_used.toLocaleString("id-ID")}</span>
        </div>
      ) : null}
      {result.token_id ? (
        <div class="row">
          <span class="label">Token id</span>
          <span class="value num">{result.token_id}</span>
        </div>
      ) : null}
      {result.verification ? (
        <p class={result.verification.passed ? "hint" : "muted"}>
          {result.verification.id} {result.verification.detail}
        </p>
      ) : null}
      {result.status === "menggantung" ? (
        <p class="hint">
          Receipt belum datang. Transaksinya sudah ada di rantai, statusnya yang
          belum diketahui. Jangan mengirim ulang sebelum memeriksanya.
        </p>
      ) : null}
    </div>
  );
}

/** Jam menit dari waktu ISO, cukup untuk menilai apakah rencana masih segar. */
function waktuSingkat(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime())
    ? iso
    : d.toLocaleTimeString("id-ID", { hour: "2-digit", minute: "2-digit" });
}

/**
 * Pemilih pasangan token lawan.
 *
 * Satu token bisa punya pasar terhadap beberapa token sekaligus, dan yang
 * dipilihkan otomatis belum tentu yang diinginkan. Pada salinan FIG di
 * 0x41F4..., ada 32 pasangan sekaligus, sebagian besar terhadap memecoin lain.
 *
 * Pasangan terhadap kutipan yang dikenal rantai ini ditandai, karena di situlah
 * pasar sungguhan berada. Sisanya tetap bisa dipilih, hanya tidak dipilihkan.
 *
 * Tidak ada yang dihitung di sini. Menekan sebuah pasangan hanya mengirim
 * pilihan itu ke engine, dan engine yang mengganti daftar pool beserta desimal
 * kutipannya.
 */
function QuotePicker({
  quotes,
  selected,
  onSelect,
}: {
  quotes: QuoteGroup[];
  selected: string | null;
  onSelect: (quote: string) => void;
}) {
  if (quotes.length <= 1) return null;

  return (
    <div class="quote-picker">
      {quotes.map((q) => {
        const aktif = q.quote === selected;
        return (
          <button
            key={q.quote}
            type="button"
            class={`quote-chip${aktif ? " active" : ""}${q.canonical ? " canonical" : ""}`}
            title={`${q.quote} · ${q.pools.length} pool · ${q.quote_decimals} desimal`}
            onClick={() => onSelect(q.quote)}
          >
            {q.quote_symbol}
            <span class="quote-count">{q.pools.length}</span>
          </button>
        );
      })}
    </div>
  );
}

/**
 * Satu pool beserta angka yang menentukan pilihannya.
 *
 * APR adalah proyeksi dari jendela yang diukur, bukan hasil. TVL hanya
 * mencakup pita di sekitar harga, karena likuiditas yang jauh dari harga tidak
 * menerima fee apa pun dan memasukkannya akan membuat pool tebal terlihat
 * lebih baik daripada yang sebenarnya.
 *
 * Tick spacing TIDAK ditampilkan. Ia menentukan grid tempat batas range
 * dibulatkan, tapi akibatnya sudah terlihat langsung sebagai selisih harga di
 * kartu Range, dan angkanya sendiri tidak mengubah keputusan memilih pool.
 */
function PoolRow({
  pool,
  active,
  onSelect,
}: {
  pool: PoolInfo;
  active: boolean;
  onSelect: () => void;
}) {
  const s = pool.stats ?? null;
  const berhook =
    pool.hooks && pool.hooks !== "0x0000000000000000000000000000000000000000";

  return (
    <button class={`pool-row${active ? " active" : ""}`} onClick={onSelect}>
      <span class="pool-kiri">
        <span class="chip">{pool.version.toUpperCase()}</span>
        <span class="num">
          {/* Pool dynamic fee tidak punya besaran fee yang bisa ditampilkan.
              Angkanya di PoolKey hanya penanda, dan menampilkannya sebagai
              persen akan tertulis 838,86 persen. */}
          {pool.dynamic_fee ? "fee dinamis" : `${(pool.fee / 10_000).toFixed(2)}%`}
        </span>
        {berhook ? <span class="chip warn">hook</span> : null}
      </span>

      <span class="pool-angka num">
        <span>
          <i>APR</i>
          {s?.apr == null ? "-" : `${Math.round(s.apr).toLocaleString("id-ID")}%`}
        </span>
        <span>
          <i>TVL</i>
          {s ? formatCompact(s.tvl) : "-"}
        </span>
        <span>
          <i>vol</i>
          {s?.volume == null ? "-" : formatCompact(s.volume)}
        </span>
      </span>
    </button>
  );
}

/** Preset lebar range, dalam persen ke tiap arah. */
const PRESET_LEBAR = [1, 5, 10, 20];

/**
 * Atur range di atas gambar likuiditas yang sudah ada di pool.
 *
 * Porsi fee kita adalah modal kita dibagi likuiditas lain yang aktif di pita
 * tempat harga bergerak. Histogram inilah yang menunjukkannya.
 *
 * Batangnya mendatar karena panel ini lebarnya 320 sampai 480 piksel: sumbu
 * harga yang tegak tidak menyisakan ruang untuk label harga tiap pita.
 *
 * Batas range dan batas pita dibandingkan sebagai kelipatan terhadap harga
 * sekarang. Gerakan satu persen pada harga adalah gerakan satu persen pada
 * market cap, jadi tidak ada konversi satuan yang bisa meleset.
 */
function DepthCard({
  view,
  depth,
  onSetRange,
  onSetFullRange,
  onMuat,
}: {
  view: RangeView;
  depth: Distribusi | null;
  onSetRange: (lowerMcap: number, upperMcap: number) => void;
  onSetFullRange: () => void;
  onMuat: () => void;
}) {
  const mcapSekarang = view.current.mcap_usd;
  const [custom, setCustom] = useState(false);
  const [bawah, setBawah] = useState("");
  const [atas, setAtas] = useState("");

  // Preset mana yang sedang berlaku ditentukan lebar range yang sebenarnya,
  // bukan tombol mana yang terakhir ditekan. Engine membulatkan batas ke grid
  // tick, jadi lebarnya bisa bergeser sedikit, dan menandai tombol menurut
  // ingatan panel akan berbohong setelah pembulatan.
  const lebarSekarang = view.width_pct / 2;
  const presetAktif =
    PRESET_LEBAR.find((x) => Math.abs(lebarSekarang - x) / x < 0.1) ?? null;
  const penuhAktif = view.width_pct > 1_000;

  const pilihLebar = (persen: number) => {
    setCustom(false);
    const f = persen / 100;
    onSetRange(mcapSekarang * (1 - f), mcapSekarang * (1 + f));
  };

  const pilihPenuh = () => {
    setCustom(false);
    onSetFullRange();
  };

  const bukaCustom = () => {
    // Diisi dengan batas yang sedang berlaku, supaya user menyunting angka
    // yang sedang dilihatnya, bukan kotak kosong.
    setBawah(String(view.lower.price_usd));
    setAtas(String(view.upper.price_usd));
    setCustom(true);
  };

  const terapkanCustom = () => {
    const b = Number(bawah);
    const a = Number(atas);
    if (!Number.isFinite(b) || !Number.isFinite(a) || b <= 0 || a <= 0) return;
    // Harga USD diubah menjadi market cap lewat perbandingan terhadap harga
    // sekarang, satuan yang sudah dipakai seluruh kartu ini.
    const skala = mcapSekarang / view.current.price_usd;
    onSetRange(b * skala, a * skala);
  };

  const kelipatanBawah = view.lower.mcap_usd / mcapSekarang;
  const kelipatanAtas = view.upper.mcap_usd / mcapSekarang;

  return (
    <section class="card">
      <div class="section-title">
        <h2>Atur range</h2>
        <span class="chip">{view.width_pct.toFixed(1)}% lebar</span>
        {depth?.error ? (
          <button class="toggle" onClick={onMuat}>
            coba lagi
          </button>
        ) : null}
      </div>

      <div class="preset-row">
        {PRESET_LEBAR.map((x) => (
          <button
            key={x}
            class={`preset${presetAktif === x ? " aktif" : ""}`}
            onClick={() => pilihLebar(x)}
          >
            ±{x}%
          </button>
        ))}
        <button class={`preset${penuhAktif ? " aktif" : ""}`} onClick={pilihPenuh}>
          penuh
        </button>
        <button
          class={`preset${custom ? " aktif" : ""}`}
          onClick={() => (custom ? setCustom(false) : bukaCustom())}
        >
          custom
        </button>
      </div>

      {custom ? (
        <div class="custom-range">
          <label>
            <span>Batas bawah, USD</span>
            <input
              type="text"
              value={bawah}
              spellcheck={false}
              onInput={(e) => setBawah((e.target as HTMLInputElement).value)}
            />
          </label>
          <label>
            <span>Batas atas, USD</span>
            <input
              type="text"
              value={atas}
              spellcheck={false}
              onInput={(e) => setAtas((e.target as HTMLInputElement).value)}
            />
          </label>
          <button class="lebar" onClick={terapkanCustom}>
            terapkan
          </button>
        </div>
      ) : null}

      {!depth ? (
        <p class="muted" style="margin: var(--s3) 0 0">Membaca bentuk pool.</p>
      ) : depth.error ? (
        <p class="error" style="margin: var(--s3) 0 0">{depth.error}</p>
      ) : depth.pita.length === 0 ? (
        <p class="muted" style="margin: var(--s3) 0 0">
          Belum ada likuiditas lain di sekitar harga ini.
        </p>
      ) : (
        <Histogram
          depth={depth}
          kelipatanBawah={kelipatanBawah}
          kelipatanAtas={kelipatanAtas}
        />
      )}

      <p class="hint">Batang biru: modal yang akan berbagi fee dengan Anda.</p>


    </section>
  );
}

/**
 * Berapa baris paling banyak digambar histogram.
 *
 * Pool V3 yang ramai bisa punya ratusan tick terinisialisasi, dan menggambar
 * satu baris per tick menghasilkan dinding batang yang tidak bisa dibaca.
 * Dua puluh empat baris cukup untuk melihat bentuk poolnya, dan cukup sedikit
 * untuk setiap barisnya tetap punya label harga yang terbaca.
 */
const MAKS_BARIS_HISTOGRAM = 24;

/**
 * Gabungkan pita yang berdekatan sampai jumlah barisnya masuk akal.
 *
 * Nilainya dijumlahkan dan rentang harganya disatukan, sehingga total modal
 * yang tergambar tetap sama persis. Pita yang memuat harga sekarang tidak
 * pernah digabung dengan tetangganya, karena letaknya itulah yang paling
 * ingin dilihat user.
 */
function ringkasPita(pita: Pita[], maks: number): Pita[] {
  if (pita.length <= maks) return pita;

  const per = Math.ceil(pita.length / maks);
  const keluar: Pita[] = [];
  let kelompok: Pita[] = [];

  const dorong = () => {
    if (kelompok.length === 0) return;
    const pertama = kelompok[0];
    const terakhir = kelompok[kelompok.length - 1];
    keluar.push({
      tick_bawah: Math.min(...kelompok.map((p) => p.tick_bawah)),
      tick_atas: Math.max(...kelompok.map((p) => p.tick_atas)),
      harga_bawah: Math.min(pertama.harga_bawah, terakhir.harga_bawah),
      harga_atas: Math.max(pertama.harga_atas, terakhir.harga_atas),
      liquidity: pertama.liquidity,
      nilai_quote: kelompok.reduce((t, p) => t + p.nilai_quote, 0),
      aktif: kelompok.some((p) => p.aktif),
    });
    kelompok = [];
  };

  for (const p of pita) {
    if (p.aktif) {
      dorong();
      keluar.push(p);
      continue;
    }
    kelompok.push(p);
    if (kelompok.length >= per) dorong();
  }
  dorong();

  return keluar;
}

function Histogram({
  depth,
  kelipatanBawah,
  kelipatanAtas,
}: {
  depth: Distribusi;
  kelipatanBawah: number;
  kelipatanAtas: number;
}) {
  const puncak = depth.pita.reduce((m, p) => Math.max(m, p.nilai_quote), 0);

  // Diurutkan menurut HARGA, bukan menurut tick, lalu digambar dari yang
  // tertinggi ke terendah supaya searah dengan chart. Mengurutkan menurut tick
  // akan menggambar terbalik pada pool yang memecoinnya token1.
  const urut = [...depth.pita].sort((a, b) => b.harga_bawah - a.harga_bawah);
  const baris = ringkasPita(urut, MAKS_BARIS_HISTOGRAM);

  const dalamRange = (p: { harga_bawah: number; harga_atas: number }) => {
    const bawah = p.harga_bawah / depth.harga_sekarang;
    const atas = p.harga_atas / depth.harga_sekarang;
    return atas > kelipatanBawah && bawah < kelipatanAtas;
  };

  return (
    <div class="depth">
      {baris.map((p) => (
        <div
          key={`${p.tick_bawah}:${p.tick_atas}`}
          class={`depth-row${dalamRange(p) ? " terpilih" : ""}${p.aktif ? " aktif" : ""}`}
        >
          <span class="depth-harga num">{formatHargaKecil(p.harga_atas)}</span>
          <span class="depth-bar-wrap">
            <span
              class="depth-bar"
              style={`width: ${puncak > 0 ? (p.nilai_quote / puncak) * 100 : 0}%`}
            />
          </span>
          <span class="depth-nilai num">{formatCompact(p.nilai_quote)}</span>
        </div>
      ))}
    </div>
  );
}

/**
 * Harga memecoin biasanya jauh di bawah satu, dan pembulatan biasa akan
 * menampilkan seluruh baris sebagai 0.00.
 */
function formatHargaKecil(v: number): string {
  if (!Number.isFinite(v)) return "-";
  if (v === 0) return "0";
  if (Math.abs(v) >= 1) return v.toFixed(4);
  return v.toPrecision(4);
}

/**
 * Range dalam tiga satuan sekaligus, sesuai acceptance criteria di PRD 3.3.
 *
 * Semua angka datang dari engine. Panel tidak menghitung apa pun di sini,
 * termasuk konversi satuan, supaya tidak ada dua tempat yang bisa menyimpang.
 */
function RangeCard({ view }: { view: RangeView }) {
  return (
    <section class="card">
      <div class="section-title">
        <h2>Range</h2>
        <span class="chip">
          <span class={`dot ${view.in_range ? "ok" : "warn"}`} />
          {view.in_range ? "di dalam" : "di luar"}
        </span>
      </div>

      <table class="range-table">
        <thead>
          <tr>
            <th></th>
            <th>Market cap</th>
            <th>Harga USD</th>
            <th>Tick</th>
          </tr>
        </thead>
        <tbody>
          <BoundRow label="Atas" bound={view.upper} tone="upper" />
          <tr class="current">
            <td class="label">Sekarang</td>
            <td class="num">{formatCompact(view.current.mcap_usd)}</td>
            <td class="num">{formatUsd(view.current.price_usd)}</td>
            <td class="num">{view.current.tick.toLocaleString("id-ID")}</td>
          </tr>
          <BoundRow label="Bawah" bound={view.lower} tone="lower" />
        </tbody>
      </table>

      <div class="row" style="margin-top: var(--s3)">
        <span class="label">Lebar</span>
        <span class="value num">{view.width_pct.toFixed(1)}%</span>
      </div>
      <div class="row">
        <span class="label">Tick spacing</span>
        <span class="value num">{view.tick_spacing}</span>
      </div>

      {view.warnings.map((w) => (
        <p key={w} class="warn-line">
          {w}
        </p>
      ))}

      <p class="hint">Angka sudah dibulatkan ke grid tick.</p>
    </section>
  );
}

function BoundRow({
  label,
  bound,
  tone,
}: {
  label: string;
  bound: RangeBound;
  tone: "upper" | "lower";
}) {
  const drift = Math.abs(bound.drift_bps) >= 1;
  return (
    <tr>
      <td class="label">
        <span class={`bound-dot ${tone}`} />
        {label}
      </td>
      <td class="num">
        {formatCompact(bound.mcap_usd)}
        {drift ? (
          <span class="muted" style="font-size: 10px">
            {" "}
            {bound.drift_bps > 0 ? "+" : ""}
            {bound.drift_bps.toFixed(0)} bp
          </span>
        ) : null}
      </td>
      <td class="num">{formatUsd(bound.price_usd)}</td>
      <td class="num">{bound.tick.toLocaleString("id-ID")}</td>
    </tr>
  );
}

function formatUsd(n: number): string {
  if (!Number.isFinite(n)) return "-";
  if (n >= 1) return `$${n.toLocaleString("id-ID", { maximumFractionDigits: 2 })}`;
  // Harga memecoin sering punya banyak nol di depan, jadi angka pentingnya
  // yang dipertahankan, bukan jumlah desimalnya.
  return `$${n.toPrecision(4)}`;
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

  // Satu isian yang menerima keduanya. Bentuknya yang menentukan artinya,
  // bukan pilihan terpisah dari user.
  const [isian, setIsian] = useState("");
  const isianKunci = isPrivateKey(isian);
  const siapBuat = isianKunci
    ? passphrase.length >= 8
    : isian.length >= 8;
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
      setIsian("");
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

      {/* SATU ISIAN SAJA
          User menempelkan passphrase atau kunci privat ke kotak yang sama, dan
          panel mengenalinya dari bentuknya: 0x diikuti 64 karakter heksadesimal
          adalah kunci privat, selain itu passphrase.

          Kotak kedua hanya muncul kalau yang ditempel ternyata kunci privat,
          karena keystore tetap harus dienkripsi dan kunci privat bukan
          passphrase. Pada pemakaian biasa yang terlihat cuma satu kotak. */}
      {!w.exists ? (
        <div class="stack" style="margin-top: var(--s3)">
          <input
            type="password"
            value={isian}
            placeholder="Passphrase baru, atau tempel kunci privat"
            spellcheck={false}
            onInput={(e) => setIsian((e.target as HTMLInputElement).value)}
          />

          {isianKunci ? (
            <input
              type="password"
              value={passphrase}
              placeholder="Passphrase untuk mengunci kunci itu"
              onInput={(e) => setPassphrase((e.target as HTMLInputElement).value)}
            />
          ) : null}

          <button
            class="primary"
            disabled={busy || !siapBuat}
            onClick={() =>
              void act("/v1/wallet/create", {
                passphrase: isianKunci ? passphrase : isian,
                ...(isianKunci ? { private_key: isian.trim() } : {}),
              })
            }
          >
            {isianKunci ? "Impor wallet" : "Buat wallet"}
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

/**
 * Sunting daftar endpoint RPC.
 *
 * Yang pertama dipakai, sisanya cadangan. Kunci API di dalam URL aman ditulis
 * di sini: engine menyuntingnya sebelum URL pernah masuk ke log atau ke status,
 * dan yang ditampilkan di bawah adalah versi yang sudah tersunting.
 *
 * Engine tidak menukar endpoint saat sedang jalan, jadi perubahannya baru
 * berlaku setelah engine dijalankan ulang. Itu dikatakan apa adanya, karena
 * user yang mengira RPC barunya sudah bekerja akan salah menilai kecepatannya.
 */
function RpcEditor({
  endpoints,
  onSimpan,
}: {
  endpoints: string[] | undefined;
  onSimpan: (http: string[]) => void;
}) {
  // Engine versi lama tidak mengirim daftar ini. Tanpa penjagaan, seluruh
  // kartu Settings gagal dirender dan yang terlihat user adalah bagian yang
  // kosong, bukan pesan yang menjelaskan apa pun.
  const daftar = Array.isArray(endpoints) ? endpoints : [];
  const [buka, setBuka] = useState(false);
  const [teks, setTeks] = useState("");

  const mulai = () => {
    // Kotak dibiarkan kosong, bukan diisi versi tersunting. Menyimpan kembali
    // teks bertanda bintang akan mengganti endpoint yang bekerja dengan URL
    // yang tidak sah, dan itu baru ketahuan setelah engine dijalankan ulang.
    setTeks("");
    setBuka(true);
  };

  return (
    <div class="rpc">
      <div class="row">
        <span class="label">RPC</span>
        <button class="chip" style="cursor: pointer" onClick={() => (buka ? setBuka(false) : mulai())}>
          {buka ? "batal" : "ganti"}
        </button>
      </div>

      {daftar.length === 0 ? (
        <p class="rpc-baris">
          Engine belum melaporkan endpointnya. Jalankan ulang engine versi
          terbaru.
        </p>
      ) : null}

      {daftar.map((e, i) => (
        <p key={e} class="rpc-baris">
          {i === 0 ? "dipakai" : "cadangan"} · {e}
        </p>
      ))}

      {buka ? (
        <>
          <textarea
            class="rpc-input"
            rows={3}
            spellcheck={false}
            placeholder={"Satu endpoint per baris. Yang pertama dipakai."}
            value={teks}
            onInput={(e) => setTeks((e.target as HTMLTextAreaElement).value)}
          />
          <button
            class="lebar"
            onClick={() => {
              const baris = teks
                .split("\n")
                .map((x) => x.trim())
                .filter((x) => x.length > 0);
              if (baris.length === 0) return;
              onSimpan(baris);
              setBuka(false);
            }}
          >
            simpan, lalu jalankan ulang engine
          </button>
        </>
      ) : null}
    </div>
  );
}

function EngineCard({
  status,
  onSetRpc,
  onForget,
}: {
  status: EngineStatus;
  onSetRpc: (http: string[]) => void;
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

      <RpcEditor endpoints={status.rpc_http} onSimpan={onSetRpc} />

      

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

/**
 * Kunci privat secp256k1 yang bentuknya benar.
 *
 * Hanya memeriksa bentuk, bukan keabsahan kurva. Engine yang memutuskan itu,
 * dan pesannya akan spesifik kalau kuncinya tidak sah.
 */
function isPrivateKey(v: string): boolean {
  return /^0x[a-fA-F0-9]{64}$/.test(v.trim());
}

function shortAddress(a: string): string {
  return a.length > 12 ? `${a.slice(0, 6)}...${a.slice(-4)}` : a;
}

/** Angka besar dalam bentuk ringkas, karena side panel sempit. */
function formatCompact(n: number): string {
  if (!Number.isFinite(n)) return "-";
  const abs = Math.abs(n);
  if (abs >= 1e12) return `${(n / 1e12).toFixed(2)} T`;
  if (abs >= 1e9) return `${(n / 1e9).toFixed(2)} M`;
  if (abs >= 1e6) return `${(n / 1e6).toFixed(2)} jt`;
  if (abs >= 1e3) return `${(n / 1e3).toFixed(2)} rb`;
  return n.toLocaleString("id-ID", { maximumFractionDigits: 4 });
}

function formatDuration(secs: number): string {
  if (secs < 60) return `${secs} d`;
  const m = Math.floor(secs / 60);
  if (m < 60) return `${m} m`;
  const h = Math.floor(m / 60);
  return `${h} j ${m % 60} m`;
}
