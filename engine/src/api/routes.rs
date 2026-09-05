//! Handler HTTP.
//!
//! INVARIAN
//! - Passphrase hanya boleh mengalir MASUK. Tidak ada respons yang memuatnya,
//!   dan tidak ada log yang mencetaknya.
//! - Membuat wallet tidak otomatis membukanya. User tetap harus membuka dengan
//!   passphrase, supaya jalur buka kunci teruji sejak menit pertama.

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::error::{EngineError, EngineResult};

use super::protocol::EngineStatus;
use super::{AppState, ENGINE_VERSION};

#[derive(Serialize)]
pub struct PingResponse {
    pub name: &'static str,
    pub engine_version: &'static str,
    pub protocol_version: u32,
    pub requires_pairing: bool,
}

/// Endpoint terbuka untuk penemuan. Sengaja tidak membocorkan apa pun.
pub async fn ping() -> Json<PingResponse> {
    Json(PingResponse {
        name: "rangefinder",
        engine_version: ENGINE_VERSION,
        protocol_version: super::protocol::PROTOCOL_VERSION,
        requires_pairing: true,
    })
}

pub async fn status(State(state): State<AppState>) -> Json<EngineStatus> {
    Json(state.status().await)
}

pub async fn registry(State(state): State<AppState>) -> Json<super::protocol::RegistrySummary> {
    Json(state.registry_summary())
}

#[derive(Deserialize)]
pub struct CreateWalletRequest {
    pub passphrase: String,
    /// Kunci privat hex opsional. Kosong berarti engine membuat kunci acak.
    #[serde(default)]
    pub private_key: Option<String>,
}

#[derive(Serialize)]
pub struct AddressResponse {
    pub address: String,
}

pub async fn wallet_create(
    State(state): State<AppState>,
    Json(req): Json<CreateWalletRequest>,
) -> EngineResult<Json<AddressResponse>> {
    if req.passphrase.chars().count() < 8 {
        return Err(EngineError::BadRequest(
            "passphrase minimal 8 karakter. Ini satu satunya yang melindungi kunci di disk".into(),
        ));
    }

    let key = match req.private_key.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(hex_key) => {
            let cleaned = hex_key.strip_prefix("0x").unwrap_or(hex_key);
            let bytes = hex::decode(cleaned).map_err(|_| {
                EngineError::BadRequest("private_key bukan hex yang valid".into())
            })?;
            if bytes.len() != 32 {
                return Err(EngineError::BadRequest(format!(
                    "private_key harus 32 byte, yang dikirim {} byte",
                    bytes.len()
                )));
            }
            Some(alloy::primitives::B256::from_slice(&bytes))
        }
    };

    let mut wallet = state.wallet.lock().await;
    let address = wallet.create(&req.passphrase, key)?;

    Ok(Json(AddressResponse {
        address: address.to_string(),
    }))
}

#[derive(Deserialize)]
pub struct UnlockRequest {
    pub passphrase: String,
}

pub async fn wallet_unlock(
    State(state): State<AppState>,
    Json(req): Json<UnlockRequest>,
) -> EngineResult<Json<AddressResponse>> {
    let mut wallet = state.wallet.lock().await;
    let address = wallet.unlock(&req.passphrase)?;
    drop(wallet);

    state.broadcast_status().await;

    Ok(Json(AddressResponse {
        address: address.to_string(),
    }))
}

#[derive(Serialize)]
pub struct OkResponse {
    pub ok: bool,
}

pub async fn wallet_lock(State(state): State<AppState>) -> EngineResult<Json<OkResponse>> {
    state.wallet.lock().await.lock();
    state.broadcast_status().await;
    Ok(Json(OkResponse { ok: true }))
}

impl AppState {
    /// Kirim status terbaru ke semua panel yang tersambung.
    ///
    /// Kegagalan diabaikan dengan sengaja: tidak ada penerima bukan kondisi
    /// error, itu hanya berarti panel sedang tertutup.
    pub async fn broadcast_status(&self) {
        let status = self.status().await;
        let _ = self
            .events
            .send(super::protocol::ServerMessage::EngineStatus(Box::new(
                status,
            )));
    }
}
