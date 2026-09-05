/**
 * Kontrak pesan antara panel dan engine.
 *
 * INVARIAN
 * - Berkas ini adalah cerminan `engine/src/api/protocol.rs`. Kalau salah satu
 *   berubah, yang lain wajib ikut berubah dalam commit yang sama, begitu juga
 *   PRD bagian 7.3. Lihat aturan sinkronisasi di PRD 13.1.
 * - Token pairing TIDAK PERNAH masuk URL. Untuk HTTP ia ada di header
 *   Authorization, untuk WebSocket ia dititipkan di subprotocol, karena browser
 *   tidak mengizinkan memasang header pada koneksi WebSocket.
 */

export const PROTOCOL_VERSION = 1;
export const WS_SUBPROTOCOL = "rangefinder";
export const DEFAULT_ENGINE_ORIGIN = "http://127.0.0.1:8787";

export type AlertLevel = "info" | "warning" | "critical";

export type RegistryStatus = "verified" | "unpinned" | "no_code" | "mismatch";

export interface RegistryCheck {
  section: string;
  name: string;
  address: string;
  has_code: boolean;
  code_hash: string;
  pinned: string | null;
  status: RegistryStatus;
}

export interface RegistrySummary {
  total: number;
  verified: number;
  unpinned: number;
  no_code: number;
  checks: RegistryCheck[];
}

export interface ChainHealth {
  healthy: boolean;
  endpoint: string;
  chain_id: number | null;
  block_number: number | null;
  block_lag_secs: number | null;
  max_block_lag_secs: number;
  error: string | null;
  checked_at: string;
}

export interface WalletStatus {
  exists: boolean;
  locked: boolean;
  address: string | null;
  auto_lock_minutes: number;
  seconds_until_auto_lock: number | null;
}

export interface EngineStatus {
  engine_version: string;
  uptime_secs: number;
  chain: ChainHealth;
  wallet: WalletStatus;
  registry: RegistrySummary;
  can_execute: boolean;
  blocked_reasons: string[];
}

export type PoolVersion = "v3" | "v4";

export interface PoolInfo {
  version: PoolVersion;
  /** Alamat pool untuk V3, atau poolId heksadesimal untuk V4. */
  id: string;
  token0: string;
  token1: string;
  fee: number;
  tick_spacing: number;
  hooks: string | null;
  sqrt_price_x96: string;
  tick: number;
  liquidity: string;
  /** Harga mentah token1 per token0 pada satuan terkecil. */
  raw_price: number;
}

export interface TokenMeta {
  address: string;
  symbol: string;
  decimals: number;
  total_supply: number;
}

/**
 * Jawaban atas `subscribe_token`.
 *
 * `pools` sudah disaring engine, pool tanpa likuiditas tidak pernah ikut,
 * karena harganya berasal dari saat pool dibuat dan bisa meleset puluhan kali
 * lipat. Panel tidak perlu tahu pool itu ada.
 */
export interface TokenState {
  address: string;
  meta: TokenMeta | null;
  pools: PoolInfo[];
  selected_pool_id: string | null;
  error: string | null;
}

export type ServerMessage =
  | { type: "hello"; engine_version: string; protocol_version: number }
  | ({ type: "engine_status" } & Partial<EngineStatus>)
  | { type: "pong" }
  | ({ type: "token_state" } & Partial<TokenState>)
  | { type: "alert"; level: AlertLevel; code: string; message: string };

export type ClientMessage =
  | { type: "get_status" }
  | { type: "ping" }
  | { type: "subscribe_token"; address: string };

export function asTokenState(msg: unknown): TokenState | null {
  if (!msg || typeof msg !== "object") return null;
  const m = msg as Record<string, unknown>;
  if (m.type !== "token_state") return null;
  const { type: _type, ...rest } = m;
  return rest as unknown as TokenState;
}

/** Alamat EVM yang bentuknya benar. Checksum tidak diperiksa di sini. */
export function isEvmAddress(value: string): boolean {
  return /^0x[a-fA-F0-9]{40}$/.test(value.trim());
}

export function gmgnTokenUrl(address: string): string {
  return `https://gmgn.ai/robinhood/token/${address.trim().toLowerCase()}`;
}

/**
 * Bentuk `engine_status` di kabel adalah EngineStatus yang diratakan bersama
 * field `type`, karena serde memakai tag internal. Pembantu ini memisahkan
 * keduanya supaya sisi UI tidak perlu tahu detail itu.
 */
export function asEngineStatus(msg: unknown): EngineStatus | null {
  if (!msg || typeof msg !== "object") return null;
  const m = msg as Record<string, unknown>;
  if (m.type !== "engine_status") return null;
  const { type: _type, ...rest } = m;
  return rest as unknown as EngineStatus;
}

export interface EnginePing {
  name: string;
  engine_version: string;
  protocol_version: number;
  requires_pairing: boolean;
}
