//! Tipe error engine dan pemetaannya ke respons HTTP.
//!
//! INVARIAN
//! - Pesan error yang dikirim ke panel harus spesifik dan bisa ditindaklanjuti.
//!   Tidak boleh ada "terjadi kesalahan" tanpa keterangan. Ini bagian dari
//!   acceptance criteria di PRD 3.3.
//! - Error yang menyentuh rahasia tidak boleh membocorkan isinya.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("token pairing tidak cocok")]
    Unauthorized,

    #[error("wallet masih terkunci, buka dulu sebelum aksi ini")]
    WalletLocked,

    #[error("wallet belum dibuat, jalankan perintah setup dulu")]
    WalletMissing,

    /// Menimpa keystore sama dengan menghapus akses ke dana, jadi ini ditolak
    /// dan harus jadi tindakan manual yang sadar.
    #[error(
        "wallet sudah ada di {0}. Hapus berkasnya secara manual kalau memang mau menggantinya, \
         dan pastikan kunci lamanya sudah dicadangkan"
    )]
    WalletExists(String),

    #[error("passphrase salah")]
    BadPassphrase,

    #[error("keystore rusak atau formatnya tidak dikenal: {0}")]
    KeystoreCorrupt(String),

    #[error("RPC tidak sehat: {0}")]
    RpcUnhealthy(String),

    #[error("registry kontrak gagal diverifikasi: {0}")]
    RegistryMismatch(String),

    #[error("permintaan tidak valid: {0}")]
    BadRequest(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl EngineError {
    fn status(&self) -> StatusCode {
        match self {
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::WalletLocked | Self::WalletMissing | Self::BadPassphrase => StatusCode::FORBIDDEN,
            // Wallet yang sudah ada adalah bentrokan keadaan, bukan kesalahan
            // internal. Panel membedakannya untuk menawarkan buka kunci.
            Self::WalletExists(_) => StatusCode::CONFLICT,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::RpcUnhealthy(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::KeystoreCorrupt(_) | Self::RegistryMismatch(_) | Self::Other(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }

    /// Kode mesin yang stabil, dipakai panel untuk membedakan penanganan.
    fn code(&self) -> &'static str {
        match self {
            Self::Unauthorized => "unauthorized",
            Self::WalletLocked => "wallet_locked",
            Self::WalletMissing => "wallet_missing",
            Self::WalletExists(_) => "wallet_exists",
            Self::BadPassphrase => "bad_passphrase",
            Self::KeystoreCorrupt(_) => "keystore_corrupt",
            Self::RpcUnhealthy(_) => "rpc_unhealthy",
            Self::RegistryMismatch(_) => "registry_mismatch",
            Self::BadRequest(_) => "bad_request",
            Self::Other(_) => "internal",
        }
    }
}

#[derive(Serialize)]
struct ErrorBody {
    code: &'static str,
    message: String,
}

impl IntoResponse for EngineError {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = ErrorBody {
            code: self.code(),
            message: self.to_string(),
        };
        if status.is_server_error() {
            tracing::error!(code = body.code, message = %body.message, "permintaan gagal");
        }
        (status, Json(body)).into_response()
    }
}

pub type EngineResult<T> = Result<T, EngineError>;
