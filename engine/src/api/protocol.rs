//! Kontrak pesan antara panel dan engine.
//!
//! INVARIAN
//! - Setiap tipe pesan di sini WAJIB punya pasangan di PRD bagian 7.3.
//!   `cargo xtask docs-check` menegakkan hal itu, dan build gagal kalau ada
//!   yang menyimpang. Lihat aturan sinkronisasi di PRD 13.1.
//! - Tidak ada pesan yang boleh membawa kunci privat atau passphrase keluar
//!   dari engine. Passphrase hanya boleh mengalir masuk.
//! - Tag serde memakai `snake_case` dan field `type`, sama persis dengan yang
//!   diharapkan sisi TypeScript di `extension/src/shared/protocol.ts`.

use serde::{Deserialize, Serialize};

use crate::chain::{ChainHealth, RegistryCheck};
use crate::wallet::WalletStatus;

/// Pesan dari panel ke engine lewat WebSocket.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Minta kiriman status terbaru sekarang juga.
    GetStatus,
    /// Denyut untuk menjaga koneksi dan memundurkan kunci otomatis.
    Ping,
    /// Minta engine mencari pool untuk sebuah token dan mengirim `token_state`.
    ///
    /// Pencariannya menyentuh jaringan dan bisa makan beberapa detik, jadi
    /// dijalankan di tugas terpisah dan jawabannya datang belakangan.
    SubscribeToken { address: String },

    /// Market cap yang sedang ditampilkan chart gmgn.
    ///
    /// Inilah yang mengkalibrasi kurs ETH ke USD. Datang tiap detik dari
    /// content script selama halaman token terbuka.
    ChartPrice { address: String, mcap_usd: f64 },

    /// User memilih pool lain dari daftar.
    SelectPool { pool_id: String },

    /// User menggeser garis. Batasnya dalam market cap USD, satuan chart.
    SetRange { lower_mcap: f64, upper_mcap: f64 },
}

/// Pesan dari engine ke panel.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Dikirim sekali saat koneksi terbentuk.
    Hello {
        engine_version: String,
        protocol_version: u32,
    },
    /// Status gabungan engine, dikirim berkala dan saat diminta.
    EngineStatus(Box<EngineStatus>),
    /// Balasan denyut.
    Pong,
    /// Hasil pencarian pool untuk satu token.
    TokenState(Box<TokenState>),
    /// Range dalam tiga satuan sekaligus, hasil kalibrasi dan snap tick.
    RangeView(Box<crate::range::RangeView>),
    /// Kejadian yang layak diberitahukan ke user. Lihat PRD 3.7.
    Alert {
        level: AlertLevel,
        code: String,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertLevel {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Serialize)]
pub struct EngineStatus {
    pub engine_version: String,
    pub uptime_secs: u64,
    pub chain: ChainHealth,
    pub wallet: WalletStatus,
    pub registry: RegistrySummary,
    /// Benar hanya kalau semua prasyarat eksekusi terpenuhi.
    pub can_execute: bool,
    /// Alasan `can_execute` bernilai salah, supaya panel tidak perlu menebak.
    pub blocked_reasons: Vec<String>,
}

/// Keadaan satu token beserta pool yang bisa dipakai.
///
/// `pools` sudah disaring dan diurutkan oleh `pools::usable_pools`, jadi panel
/// tidak perlu tahu bahwa pool kosong itu ada. Kalau pencariannya gagal,
/// `error` terisi dan `pools` kosong, bukan pesan gagal yang menggantikan
/// seluruh muatan.
#[derive(Debug, Clone, Serialize)]
pub struct TokenState {
    pub address: String,
    pub meta: Option<crate::pools::TokenMeta>,
    pub pools: Vec<crate::pools::PoolInfo>,
    /// Pool peringkat teratas, dipilih otomatis supaya panel punya titik awal.
    pub selected_pool_id: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegistrySummary {
    pub total: usize,
    pub verified: usize,
    pub unpinned: usize,
    pub no_code: usize,
    pub checks: Vec<RegistryCheck>,
}

pub const PROTOCOL_VERSION: u32 = 1;

/// Tentukan apakah engine boleh mengeksekusi transaksi, beserta alasannya.
///
/// Aturan ini terpusat di satu fungsi supaya panel, WebSocket, dan jalur HTTP
/// tidak mungkin memberi jawaban yang berbeda.
pub fn evaluate_execution_readiness(
    chain: &ChainHealth,
    wallet: &WalletStatus,
) -> (bool, Vec<String>) {
    let mut reasons = Vec::new();

    if !chain.healthy {
        reasons.push(
            chain
                .error
                .clone()
                .unwrap_or_else(|| "RPC tidak sehat".into()),
        );
    }
    if !wallet.exists {
        reasons.push("wallet belum dibuat".into());
    } else if wallet.locked {
        reasons.push("wallet masih terkunci".into());
    }

    (reasons.is_empty(), reasons)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain_sehat() -> ChainHealth {
        ChainHealth {
            healthy: true,
            endpoint: "https://contoh".into(),
            chain_id: Some(4663),
            block_number: Some(100),
            block_lag_secs: Some(1),
            max_block_lag_secs: 5,
            error: None,
            checked_at: "2026-09-05T00:00:00Z".into(),
        }
    }

    fn wallet_terbuka() -> WalletStatus {
        WalletStatus {
            exists: true,
            locked: false,
            address: Some("0xabc".into()),
            auto_lock_minutes: 30,
            seconds_until_auto_lock: Some(1800),
        }
    }

    #[test]
    fn siap_eksekusi_saat_semua_baik() {
        let (ok, reasons) = evaluate_execution_readiness(&chain_sehat(), &wallet_terbuka());
        assert!(ok);
        assert!(reasons.is_empty());
    }

    #[test]
    fn wallet_terkunci_memblokir_eksekusi() {
        let mut w = wallet_terbuka();
        w.locked = true;
        let (ok, reasons) = evaluate_execution_readiness(&chain_sehat(), &w);
        assert!(!ok);
        assert!(reasons.iter().any(|r| r.contains("terkunci")));
    }

    #[test]
    fn rpc_sakit_memblokir_eksekusi() {
        let mut c = chain_sehat();
        c.healthy = false;
        c.error = Some("lag blok 12 detik".into());
        let (ok, reasons) = evaluate_execution_readiness(&c, &wallet_terbuka());
        assert!(!ok);
        assert!(reasons.iter().any(|r| r.contains("lag blok")));
    }

    #[test]
    fn beberapa_masalah_dilaporkan_sekaligus() {
        let mut c = chain_sehat();
        c.healthy = false;
        c.error = None;
        let mut w = wallet_terbuka();
        w.exists = false;
        let (ok, reasons) = evaluate_execution_readiness(&c, &w);
        assert!(!ok);
        assert_eq!(reasons.len(), 2, "kedua masalah harus dilaporkan, bukan yang pertama saja");
    }

    #[test]
    fn pesan_klien_terbaca_dari_json() {
        let m: ClientMessage = serde_json::from_str(r#"{"type":"get_status"}"#).unwrap();
        assert!(matches!(m, ClientMessage::GetStatus));
        let m: ClientMessage = serde_json::from_str(r#"{"type":"ping"}"#).unwrap();
        assert!(matches!(m, ClientMessage::Ping));
    }

    #[test]
    fn pesan_server_memakai_tag_type() {
        let json = serde_json::to_string(&ServerMessage::Pong).unwrap();
        assert_eq!(json, r#"{"type":"pong"}"#);
    }
}
