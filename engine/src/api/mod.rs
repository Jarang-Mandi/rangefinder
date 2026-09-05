//! Server HTTP dan WebSocket lokal.
//!
//! INVARIAN
//! - Server hanya mengikat loopback. Itu ditegakkan di `config::Config::validate`,
//!   bukan di sini, supaya tidak bisa dilewati lewat jalur lain.
//! - Semua endpoint kecuali `GET /v1/ping` membutuhkan token pairing yang valid.
//!   `ping` sengaja terbuka supaya panel bisa mendeteksi keberadaan engine
//!   sebelum user memasukkan token, dan ia tidak membocorkan apa pun selain
//!   nama dan versi.
//! - Token TIDAK PERNAH dikirim lewat query string. Untuk HTTP ia ada di header
//!   Authorization, untuk WebSocket ia dititipkan di subprotocol, karena
//!   browser tidak bisa memasang header pada koneksi WebSocket.

pub mod protocol;
pub mod routes;
pub mod ws;

use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use tokio::sync::{broadcast, Mutex};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::trace::TraceLayer;

use crate::auth::PairingToken;
use crate::chain::{registry::Registry, ChainClient};
use crate::config::Config;
use crate::error::EngineError;
use crate::store::Store;
use crate::wallet::Wallet;
use protocol::{EngineStatus, RegistrySummary, ServerMessage};

pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub auth: Arc<PairingToken>,
    pub wallet: Arc<Mutex<Wallet>>,
    pub chain: Arc<ChainClient>,
    pub registry: Arc<Registry>,
    #[allow(dead_code)] // Dipakai mulai fase 3 saat posisi mulai disimpan.
    pub store: Arc<Store>,
    pub events: broadcast::Sender<ServerMessage>,
    pub started_at: Instant,
    /// Hasil gate G1 dari saat start, disimpan supaya panel bisa menampilkannya
    /// tanpa memanggil RPC ulang setiap kali.
    pub registry_checks: Arc<Vec<crate::chain::RegistryCheck>>,
}

impl AppState {
    pub async fn status(&self) -> EngineStatus {
        let chain = self.chain.health().await;
        let wallet = self.wallet.lock().await.status();
        let (can_execute, blocked_reasons) =
            protocol::evaluate_execution_readiness(&chain, &wallet);

        EngineStatus {
            engine_version: ENGINE_VERSION.to_string(),
            uptime_secs: self.started_at.elapsed().as_secs(),
            chain,
            wallet,
            registry: self.registry_summary(),
            can_execute,
            blocked_reasons,
        }
    }

    /// Cari pool untuk sebuah token di tugas terpisah, lalu siarkan hasilnya.
    ///
    /// Dijalankan terpisah karena pencariannya menyentuh jaringan dan bisa
    /// makan beberapa detik. Kalau dikerjakan di dalam loop WebSocket, panel
    /// akan berhenti menerima status selama itu.
    pub fn spawn_token_lookup(&self, address: String) {
        let state = self.clone();
        tokio::spawn(async move {
            let result = state.build_token_state(address).await;
            let _ = state
                .events
                .send(protocol::ServerMessage::TokenState(Box::new(result)));
        });
    }

    async fn build_token_state(&self, address: String) -> protocol::TokenState {
        let mut out = protocol::TokenState {
            address: address.clone(),
            meta: None,
            pools: Vec::new(),
            selected_pool_id: None,
            error: None,
        };

        let token: alloy::primitives::Address = match address.parse() {
            Ok(a) => a,
            Err(_) => {
                out.error = Some(format!("`{address}` bukan alamat EVM yang valid"));
                return out;
            }
        };

        let weth = match self
            .registry
            .tokens
            .get("weth")
            .and_then(|t| t.address.parse::<alloy::primitives::Address>().ok())
        {
            Some(a) => a,
            None => {
                out.error = Some("alamat WETH tidak ada atau tidak valid di chain.toml".into());
                return out;
            }
        };

        let provider = match self.chain.provider().await {
            Ok(p) => p,
            Err(e) => {
                out.error = Some(format!("RPC tidak bisa dipakai: {e}"));
                return out;
            }
        };

        match crate::pools::token_meta(&provider, token).await {
            Ok(m) => out.meta = Some(m),
            Err(e) => {
                // Metadata gagal biasanya berarti alamatnya bukan ERC20. Tidak
                // ada gunanya melanjutkan ke pencarian pool.
                out.error = Some(format!("{e}"));
                return out;
            }
        }

        let found = crate::pools::discover_all(&provider, &self.registry, token, weth).await;
        let usable = crate::pools::usable_pools(found);

        if usable.is_empty() {
            out.error = Some(
                "tidak ada pool dengan likuiditas untuk pasangan ini. \
                 Pool kosong sengaja tidak ditampilkan karena harganya tidak bisa dipercaya"
                    .into(),
            );
        }

        out.selected_pool_id = usable.first().map(|p| p.id.clone());
        out.pools = usable;
        out
    }

    pub fn registry_summary(&self) -> RegistrySummary {
        use crate::chain::RegistryStatus::*;
        let checks = self.registry_checks.as_ref();
        RegistrySummary {
            total: checks.len(),
            verified: checks.iter().filter(|c| c.status == Verified).count(),
            unpinned: checks.iter().filter(|c| c.status == Unpinned).count(),
            no_code: checks.iter().filter(|c| c.status == NoCode).count(),
            checks: checks.clone(),
        }
    }
}

pub fn router(state: AppState) -> Router {
    // Endpoint terbuka. Hanya ini yang tidak butuh token.
    let public = Router::new().route("/v1/ping", get(routes::ping));

    let protected = Router::new()
        .route("/v1/status", get(routes::status))
        .route("/v1/registry", get(routes::registry))
        .route("/v1/wallet/create", post(routes::wallet_create))
        .route("/v1/wallet/unlock", post(routes::wallet_unlock))
        .route("/v1/wallet/lock", post(routes::wallet_lock))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_token,
        ));

    // WebSocket memeriksa tokennya sendiri lewat subprotocol, jadi ia tidak
    // lewat middleware di atas.
    let socket = Router::new().route("/ws", get(ws::handler));

    Router::new()
        .merge(public)
        .merge(protected)
        .merge(socket)
        .layer(cors_layer())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Izinkan asal dari ekstensi Chrome mana pun.
///
/// Id ekstensi baru diketahui setelah dimuat, dan berbeda antara mode unpacked
/// dan terpaket, jadi mencantumkannya di config akan menyusahkan tanpa menambah
/// keamanan berarti. Penjaga yang sebenarnya adalah token pairing, dan server
/// ini hanya bisa dihubungi dari mesin yang sama.
fn cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin: &HeaderValue, _| {
            origin
                .to_str()
                .map(|o| o.starts_with("chrome-extension://"))
                .unwrap_or(false)
        }))
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any)
}

async fn require_token(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, EngineError> {
    let presented = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();

    if !state.auth.verify(presented) {
        return Err(EngineError::Unauthorized);
    }

    Ok(next.run(req).await)
}

/// Dipakai handler WebSocket, yang tidak bisa memakai middleware header.
pub fn token_from_subprotocols(header: Option<&HeaderValue>, auth: &PairingToken) -> bool {
    let Some(value) = header.and_then(|v| v.to_str().ok()) else {
        return false;
    };
    // Format: "rangefinder, <token>"
    value
        .split(',')
        .map(str::trim)
        .any(|part| auth.verify(part))
}

pub const WS_SUBPROTOCOL: &str = "rangefinder";

/// Status HTTP untuk penolakan WebSocket, dipisah supaya bisa diuji.
pub const WS_UNAUTHORIZED: StatusCode = StatusCode::UNAUTHORIZED;
