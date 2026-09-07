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
  asEngineStatus,
  asOpenPlan,
  asRangeView,
  asSendResult,
  asTokenState,
  type ClientMessage,
  type EnginePing,
  type EngineStatus,
  type OpenPlanView,
  type RangeView,
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
  onAlert: (level: string, code: string, message: string) => void;
  onTokenState?: (token: TokenState) => void;
  onRangeView?: (view: RangeView) => void;
  onOpenPlan?: (plan: OpenPlanView) => void;
  onSendResult?: (result: SendResultView) => void;
}

const HEARTBEAT_MS = 20_000;
const BACKOFF_START_MS = 1_000;
const BACKOFF_MAX_MS = 15_000;

export class EngineClient {
  private socket: WebSocket | null = null;
  private heartbeat: number | null = null;
  private reconnectTimer: number | null = null;
  private backoff = BACKOFF_START_MS;
  private stopped = false;

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

    socket.onopen = () => {
      this.backoff = BACKOFF_START_MS;
      this.handlers.onState("connected");
      this.startHeartbeat();
    };

    socket.onmessage = (event) => {
      let msg: ServerMessage;
      try {
        msg = JSON.parse(event.data as string) as ServerMessage;
      } catch {
        return;
      }

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
      const plan = asOpenPlan(msg);
      if (plan) {
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
      // 1006 tanpa pernah terbuka biasanya berarti handshake ditolak, dan
      // penyebab paling umum adalah token yang salah.
      if (event.code === 1006 && this.backoff === BACKOFF_START_MS) {
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
    if (this.socket?.readyState === WebSocket.OPEN) {
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
  quoteRange(sizeUsd: number) {
    this.send({ type: "quote_range", size_usd: sizeUsd });
  }

  /** Beri izin ERC20 sebesar yang dibutuhkan rencana terakhir. */
  approveToken(token: string) {
    this.send({ type: "approve_token", token });
  }

  /** Jalankan swap zap dari rencana terakhir. */
  swapZap() {
    this.send({ type: "swap_zap" });
  }

  /** Setujui rencana terakhir. Ini yang benar benar memindahkan dana. */
  openPosition() {
    this.send({ type: "open_position" });
  }

  /** Batas dalam market cap USD, karena itu satuan yang dipakai chart. */
  setRange(lowerMcap: number, upperMcap: number) {
    this.send({ type: "set_range", lower_mcap: lowerMcap, upper_mcap: upperMcap });
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

  private clearTimers() {
    this.clearHeartbeat();
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
