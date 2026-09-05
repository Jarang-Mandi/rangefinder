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

export type ServerMessage =
  | { type: "hello"; engine_version: string; protocol_version: number }
  | { type: "engine_status" } & Partial<EngineStatus>
  | { type: "pong" }
  | { type: "alert"; level: AlertLevel; code: string; message: string };

export type ClientMessage = { type: "get_status" } | { type: "ping" };

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
