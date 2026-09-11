/**
 * Klien engine: HTTP untuk aksi, WebSocket untuk status yang didorong.
 *
 * INVARIAN
 * - Panel adalah penerima pasif. Status datang dari dorongan engine, bukan dari
 *   polling. Satu satunya yang dikirim berkala adalah denyut, dan itu untuk
 *   menjaga koneksi sekaligus memundurkan kunci otomatis wallet.
 * - Token tidak pernah masuk URL. Lihat protocol.ts.
 * - Penyambungan ulang memakai backoff. Engine yang mati adalah kondisi normal,
 *   bukan kejadian luar biasa, jadi jangan membanjiri layar dengan error.
 */

import {
  DEFAULT_ENGINE_ORIGIN,
  WS_SUBPROTOCOL,
  PROTOCOL_VERSION,
  asEngineStatus,
  asActionPlan,
  asNotify,
  asSignals,
  asOpenPlan,
  asPositions,
  asTransactions,
  asDepth,
  asRangeView,
  asScan,
  asSendResult,
  asTokenState,
  type ClientMessage,
  type Deposit,
  type EnginePing,
  type EngineStatus,
  type ActionPlanView,
  type ActionInput,
  type AlertLevel,
  type Bar,
  type JenisAksi,
  type Kejadian,
  type OpenPlanView,
  type PositionsView,
  type TransactionsView,
  type RangeView,
  type Distribusi,
  type ScanView,
  type SignalsView,
  type SendResultView,
  type ServerMessage,
  type TokenState,
} from "./protocol";

export type ConnectionState =
  | "idle"
  | "connecting"
  | "connected"
  | "unauthorized"
  | "offline";

interface Handlers {
  onState: (state: ConnectionState, detail?: string) => void;
  onStatus: (status: EngineStatus) => void;
  onAlert: (level: AlertLevel, code: string, message: string) => void;
  onTokenState?: (token: TokenState) => void;
  onRangeView?: (view: RangeView) => void;
  onOpenPlan?: (plan: OpenPlanView) => void;
  onSendResult?: (result: SendResultView) => void;
  onPositions?: (view: PositionsView) => void;
  onTransactions?: (view: TransactionsView) => void;
  onActionPlan?: (plan: ActionPlanView) => void;
  onSignals?: (view: SignalsView) => void;
  onScan?: (view: ScanView) => void;
  onDepth?: (view: Distribusi) => void;
  onNotify?: (k: Kejadian) => void;
  onNotifyHistory?: (riwayat: Kejadian[]) => void;
}

const HEARTBEAT_MS = 20_000;
const BACKOFF_START_MS = 1_000;
const BACKOFF_MAX_MS = 15_000;

export class EngineClient {
  private socket: WebSocket | null = null;
  private heartbeat: number | null = null;
  private helloTimer: number | null = null;
  private reconnectTimer: number | null = null;
  private backoff = BACKOFF_START_MS;
  private stopped = false;
  private protocolReady = false;
  private hasConnected = false;
  private quoteRequest: string | null = null;
  private actionRequest: string | null = null;

  constructor(
    private readonly origin: string = DEFAULT_ENGINE_ORIGIN,
    private token: string = "",
    private readonly handlers: Handlers,
  ) {}

  setToken(token: string) {
    this.token = token;
  }

  /** Cek keberadaan engine tanpa token. Dipakai layar pairing. */
  async probe(): Promise<EnginePing> {
    const res = await fetch(`${this.origin}/v1/ping`);
    if (!res.ok) throw new Error(`engine menjawab ${res.status}`);
    return (await res.json()) as EnginePing;
  }

  /** Panggilan HTTP yang butuh token. */
  async call<T>(path: string, init?: RequestInit): Promise<T> {
    const res = await fetch(`${this.origin}${path}`, {
      ...init,
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${this.token}`,
        ...(init?.headers ?? {}),
      },
    });

    if (res.status === 401) {
      this.handlers.onState("unauthorized", "token pairing tidak cocok");
      throw new Error("token pairing tidak cocok");
    }

    const body = await res.json().catch(() => null);
    if (!res.ok) {
      const message =
        (body && typeof body === "object" && "message" in body
          ? String((body as Record<string, unknown>).message)
          : null) ?? `engine menjawab ${res.status}`;
      throw new Error(message);
    }
    return body as T;
  }

  connect() {
    this.stopped = false;
    this.openSocket();
  }

  disconnect() {
    this.stopped = true;
    this.clearTimers();
    this.socket?.close();
    this.socket = null;
    this.handlers.onState("idle");
  }

  private openSocket() {
    if (this.stopped || !this.token) return;

    this.handlers.onState("connecting");
    const wsUrl = this.origin.replace(/^http/, "ws") + "/ws";

    let socket: WebSocket;
    try {
      // Argumen kedua adalah daftar subprotocol. Di sinilah token dititipkan,
      // karena header tidak bisa dipasang pada WebSocket dari browser.
      socket = new WebSocket(wsUrl, [WS_SUBPROTOCOL, this.token]);
    } catch {
      this.scheduleReconnect();
      return;
    }
    this.socket = socket;
    this.protocolReady = false;

    socket.onopen = () => {
      this.helloTimer = self.setTimeout(() => {
        if (!this.protocolReady) socket.close(4000, "hello engine tidak diterima");
      }, 7_000);
    };

    socket.onmessage = (event) => {
      let msg: ServerMessage;
      try {
        msg = JSON.parse(event.data as string) as ServerMessage;
      } catch {
        return;
      }

      if (msg.type === "hello") {
        if (msg.protocol_version !== PROTOCOL_VERSION) {
          this.stopped = true;
          socket.close(1000, "versi protokol berbeda");
          this.handlers.onState("offline", "Versi engine dan ekstensi berbeda. Perbarui keduanya.");
          return;
        }
        this.clearHelloTimer();
        this.protocolReady = true;
        this.hasConnected = true;
        this.backoff = BACKOFF_START_MS;
        this.handlers.onState("connected");
        this.startHeartbeat();
        return;
      }
      if (!this.protocolReady) return;

      const status = asEngineStatus(msg);
      if (status) {
        this.handlers.onStatus(status);
        return;
      }
      const token = asTokenState(msg);
      if (token) {
        this.handlers.onTokenState?.(token);
        return;
      }
      const view = asRangeView(msg);
      if (view) {
        this.handlers.onRangeView?.(view);
        return;
      }
      const posisi = asPositions(msg);
      if (posisi) {
        this.handlers.onPositions?.(posisi);
        return;
      }
      const transaksi = asTransactions(msg);
      if (transaksi) {
        this.handlers.onTransactions?.(transaksi);
        return;
      }
      const aksi = asActionPlan(msg);
      if (aksi) {
        if (aksi.request_id !== this.actionRequest) return;
        this.handlers.onActionPlan?.(aksi);
        return;
      }
      const sinyal = asSignals(msg);
      if (sinyal) {
        this.handlers.onSignals?.(sinyal);
        return;
      }
      const pindaian = asScan(msg);
      if (pindaian) {
        this.handlers.onScan?.(pindaian);
        return;
      }
      const kedalaman = asDepth(msg);
      if (kedalaman) {
        this.handlers.onDepth?.(kedalaman);
        return;
      }
      const kabar = asNotify(msg);
      if (kabar) {
        this.handlers.onNotify?.(kabar);
        return;
      }
      if (msg.type === "notify_history") {
        const isi = (msg as unknown as { items?: Kejadian[] }).items;
        this.handlers.onNotifyHistory?.(Array.isArray(isi) ? isi : []);
        return;
      }
      const plan = asOpenPlan(msg);
      if (plan) {
        if (plan.request_id !== this.quoteRequest) return;
        this.handlers.onOpenPlan?.(plan);
        return;
      }
      const sent = asSendResult(msg);
      if (sent) {
        this.handlers.onSendResult?.(sent);
        return;
      }
      if (msg.type === "alert") {
        this.handlers.onAlert(msg.level, msg.code, msg.message);
      }
    };

    socket.onclose = (event) => {
      this.clearTimers();
      this.socket = null;
      this.protocolReady = false;
      if (this.stopped) return;
      // Sesudah pernah tersambung, putus singkat adalah keadaan pemulihan.
      // Panel mempertahankan data terakhir dan menunjukkan proses sambung
      // ulang, sementara launcher lokal menghidupkan engine kembali.
      if (this.hasConnected) {
        this.handlers.onState("connecting", "Menyambung ulang ke engine lokal…");
      } else if (event.code === 1006 && this.backoff === BACKOFF_START_MS) {
        this.handlers.onState("offline", "engine tidak menjawab, atau token salah");
      } else {
        this.handlers.onState("offline");
      }
      this.scheduleReconnect();
    };

    socket.onerror = () => {
      // onclose selalu menyusul, jadi penanganannya cukup di sana.
    };
  }

  private send(msg: ClientMessage) {
    if (this.protocolReady && this.socket?.readyState === WebSocket.OPEN) {
      this.socket.send(JSON.stringify(msg));
    }
  }

  requestStatus() {
    this.send({ type: "get_status" });
  }

  /**
   * Minta engine mencari pool untuk sebuah token.
   *
   * Jawabannya datang belakangan lewat `onTokenState`, bukan sebagai nilai
   * balik, karena pencariannya menyentuh rantai dan bisa makan beberapa detik.
   */
  subscribeToken(address: string) {
    this.send({ type: "subscribe_token", address });
  }

  /**
   * Laporkan market cap yang sedang ditampilkan chart.
   *
   * Inilah yang mengkalibrasi kurs ETH ke USD di engine. Tanpa laporan ini,
   * engine tidak bisa menyusun tampilan range sama sekali.
   */
  reportChartPrice(address: string, mcapUsd: number) {
    this.send({ type: "chart_price", address, mcap_usd: mcapUsd });
  }

  selectPool(poolId: string) {
    this.send({ type: "select_pool", pool_id: poolId });
  }

  /**
   * Pindah ke pasangan token lawan yang lain.
   *
   * Engine yang mengganti daftar pool dan desimal kutipan, bukan panel,
   * karena desimal itu menentukan skala seluruh harga dan dua tempat yang
   * menghitungnya akan menyimpang.
   */
  selectQuote(quote: string) {
    this.send({ type: "select_quote", quote });
  }

  /**
   * Minta engine menyusun rencana membuka posisi.
   *
   * Tidak mengirim apa pun ke rantai. Pool dan tick tidak ikut dikirim,
   * karena keduanya sudah ada di engine, dan menerimanya dari panel berarti
   * menerima parameter transaksi dari luar.
   */
  quoteRange(deposit: Deposit) {
    this.quoteRequest = crypto.randomUUID();
    this.send({ type: "quote_range", deposit, request_id: this.quoteRequest });
  }

  invalidatePlan() {
    this.quoteRequest = null;
    this.send({ type: "invalidate_plan" });
  }

  /** Beri izin ERC20 sebesar yang dibutuhkan rencana terakhir. */
  approveToken(planId: string, stepId: string) {
    this.send({ type: "approve_token", plan_id: planId, step_id: stepId });
  }

  /** Jalankan seluruh approval, zap, dan pembukaan di engine dari satu klik. */
  runOpenFlow(planId: string) {
    this.send({ type: "run_open_flow", plan_id: planId });
  }

  /** Muat ulang posisi LP wallet dari rantai. */
  getPositions() {
    this.send({ type: "get_positions" });
  }

  /** Baca jurnal transaksi persisten tanpa menyentuh wallet signer. */
  getTransactions() {
    this.send({ type: "get_transactions" });
  }

  /**
   * Kirim deret candle dari chart gmgn.
   *
   * Dikirim jauh lebih jarang daripada harga, karena isinya ratusan bar dan
   * jawaban detektor tidak berubah dalam hitungan detik.
   */
  reportChartBars(address: string, bars: Bar[]) {
    this.send({ type: "chart_bars", address, bars });
  }

  /** Susun rencana sebuah aksi pada posisi. Tidak mengirim apa pun. */
  planAction(positionId: string, action: JenisAksi, input: ActionInput = {}) {
    this.actionRequest = crypto.randomUUID();
    this.send({
      type: "plan_action",
      position_id: positionId,
      request_id: this.actionRequest,
      action,
      input,
    });
  }

  /** Setujui aksi terakhir. Ini yang benar benar memindahkan dana. */
  executeAction(planId: string) {
    this.send({ type: "execute_action", plan_id: planId });
  }

  /** Jalankan approval dan aksi posisi di engine dari satu klik. */
  runActionFlow(planId: string) {
    this.send({ type: "run_action_flow", plan_id: planId });
  }

  /** Minta riwayat notifikasi terakhir. */
  getNotifications() {
    this.send({ type: "get_notifications" });
  }

  /**
   * Minta engine memindai pool paling ramai.
   *
   * Mahal: engine membaca setiap swap pada jendela terakhir. Karena itu ia
   * hanya dijalankan saat user memintanya, tidak berkala sendiri.
   */
  scan() {
    this.send({ type: "scan" });
  }

  /**
   * Minta distribusi likuiditas pool yang sedang aktif.
   *
   * Diminta sekali per pool, bukan setiap geseran garis. Bentuk pool tidak
   * berubah secepat harganya, dan pembacaannya butuh dua perjalanan ke rantai.
   */
  getDepth() {
    this.send({ type: "get_depth" });
  }

  /**
   * Ganti daftar endpoint RPC.
   *
   * Engine hanya menulis config.toml. Endpoint yang sedang dipakai tidak
   * ditukar saat engine berjalan, jadi engine harus dijalankan ulang.
   */
  setRpc(http: string[]) {
    this.send({ type: "set_rpc", http });
  }

  /**
   * Setujui sebuah hook V4 secara sadar, PRD 5.4.
   *
   * Engine menilai ulang hooknya sendiri sebelum mencatat. Panel tidak pernah
   * mengirim kesimpulan penilaian, hanya alamat hooknya.
   */
  approveHook(hook: string) {
    this.send({ type: "approve_hook", hook });
  }

  forgetHook(hook: string) {
    this.send({ type: "forget_hook", hook });
  }

  /** Jalankan swap zap dari rencana terakhir. */
  swapZap(planId: string) {
    this.send({ type: "swap_zap", plan_id: planId });
  }

  /** Setujui rencana terakhir. Ini yang benar benar memindahkan dana. */
  openPosition(planId: string) {
    this.send({ type: "open_position", plan_id: planId });
  }

  /** Batas dalam market cap USD, karena itu satuan yang dipakai chart. */
  setRange(lowerMcap: number, upperMcap: number) {
    this.send({ type: "set_range", lower_mcap: lowerMcap, upper_mcap: upperMcap });
  }

  /**
   * Pasang range selebar yang diizinkan protokol.
   *
   * Batasnya dihitung engine, karena ia bergantung pada tick spacing pool.
   * Mengirim angka raksasa lewat `setRange` tidak bekerja: harganya tidak lagi
   * bisa diubah menjadi tick dan engine menolaknya tanpa ada yang berubah.
   */
  setFullRange() {
    this.send({ type: "set_full_range" });
  }
  private startHeartbeat() {
    this.clearHeartbeat();
    this.heartbeat = self.setInterval(() => this.send({ type: "ping" }), HEARTBEAT_MS);
  }

  private clearHeartbeat() {
    if (this.heartbeat !== null) {
      self.clearInterval(this.heartbeat);
      this.heartbeat = null;
    }
  }

  private clearHelloTimer() {
    if (this.helloTimer !== null) {
      self.clearTimeout(this.helloTimer);
      this.helloTimer = null;
    }
  }

  private clearTimers() {
    this.clearHeartbeat();
    this.clearHelloTimer();
    if (this.reconnectTimer !== null) {
      self.clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
  }

  private scheduleReconnect() {
    if (this.stopped) return;
    const delay = this.backoff;
    this.backoff = Math.min(this.backoff * 2, BACKOFF_MAX_MS);
    this.reconnectTimer = self.setTimeout(() => this.openSocket(), delay);
  }
}
