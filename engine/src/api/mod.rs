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
    /// Token yang sedang dilihat user beserta pool terpilihnya.
    ///
    /// Satu slot saja, karena side panel hanya menampilkan satu token pada satu
    /// waktu. Kalau nanti panel bisa memantau beberapa token sekaligus, ini
    /// yang berubah jadi map.
    pub active: Arc<tokio::sync::RwLock<Option<ActiveToken>>>,
}

/// Keadaan token yang sedang aktif di panel.
#[derive(Debug, Clone)]
pub struct ActiveToken {
    pub meta: crate::pools::TokenMeta,
    /// Semua kelompok pasangan yang ditemukan, bukan hanya yang terpilih.
    ///
    /// Disimpan supaya user bisa berpindah pasangan tanpa memindai ulang.
    /// Pemindaian itu bagian termahal dari seluruh alur, dan memaksanya
    /// hanya untuk berganti tab akan terasa seperti alat yang macet.
    pub quotes: Vec<crate::pools::QuoteGroup>,
    pub selected_quote: String,
    pub pools: Vec<crate::pools::PoolInfo>,
    pub selected_pool_id: String,
    pub quote_decimals: u8,
    /// Batas yang diminta user dalam market cap USD. `None` berarti pakai
    /// range bawaan.
    pub requested: Option<(f64, f64)>,
    /// Market cap terakhir yang dilaporkan chart.
    pub chart_mcap_usd: Option<f64>,
}

impl ActiveToken {
    pub fn selected_pool(&self) -> Option<&crate::pools::PoolInfo> {
        self.pools.iter().find(|p| p.id == self.selected_pool_id)
    }
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
    /// DUA TAHAP, KARENA MENUNGGU ITU MAHAL
    /// Tahap pertama hanya membaca keadaan pool yang kuncinya sudah tersimpan.
    /// Ia tidak memindai log sama sekali, jadi biayanya satu permintaan dan
    /// panel sudah punya sesuatu untuk ditampilkan dalam waktu di bawah
    /// setengah detik. Tahap kedua memindai blok yang belum pernah dilihat lalu
    /// menyiarkan ulang kalau ada yang berubah.
    ///
    /// Kalau token ini belum pernah dibuka, tahap pertama tidak menghasilkan
    /// apa apa dan panel langsung menunggu tahap kedua. Itu keadaan yang memang
    /// tidak bisa dihindari: pool V4 hanya bisa ditemukan lewat log, dan log
    /// harus dipindai sekali.
    ///
    /// Harga dan likuiditas TIDAK pernah datang dari cache. Yang disimpan hanya
    /// kunci pool, yang memang tidak pernah berubah. Lihat `pools::cache`.
    pub fn spawn_token_lookup(&self, address: String) {
        let state = self.clone();
        tokio::spawn(async move {
            let cepat = state
                .build_token_state(address.clone(), crate::pools::ModePencarian::CacheSaja)
                .await;

            // Cache kosong berarti tidak ada yang bisa ditampilkan lebih awal.
            // Menyiarkan keadaan kosong justru akan menampilkan pesan "tidak ada
            // pool" sesaat sebelum hasil sebenarnya datang, dan itu keliru.
            let punya_isi = !cepat.pools.is_empty();
            let sidik_awal = sidik_pool(&cepat);
            if punya_isi {
                let _ = state
                    .events
                    .send(protocol::ServerMessage::TokenState(Box::new(cepat)));
            }

            let penuh = state
                .build_token_state(address, crate::pools::ModePencarian::Penuh)
                .await;

            // Siarkan ulang hanya kalau memang ada bedanya. Panel yang menerima
            // dua kiriman identik akan menggambar ulang garis chart tanpa
            // alasan, dan itu terlihat seperti kedipan.
            if !punya_isi || sidik_pool(&penuh) != sidik_awal {
                let _ = state
                    .events
                    .send(protocol::ServerMessage::TokenState(Box::new(penuh)));
            }
        });
    }

    async fn build_token_state(
        &self,
        address: String,
        mode: crate::pools::ModePencarian,
    ) -> protocol::TokenState {
        let mut out = protocol::TokenState {
            address: address.clone(),
            meta: None,
            quotes: Vec::new(),
            pools: Vec::new(),
            selected_quote: None,
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

        let provider = match self.chain.provider().await {
            Ok(p) => p,
            Err(e) => {
                out.error = Some(format!("RPC tidak bisa dipakai: {e}"));
                return out;
            }
        };

        // Metadata token, pool V3, dan keadaan pool V4 dibaca dalam satu
        // permintaan. Pasangannya TIDAK ditebak: token bisa berpasangan dengan
        // stablecoin, ETH asli, atau apa pun. Menebak WETH pernah membuat kita
        // melaporkan nol pool padahal ada puluhan. Lihat pools::muat_token.
        let snapshot = match crate::pools::muat_token(
            &provider,
            &self.registry,
            Some(&self.store),
            token,
            mode,
        )
        .await
        {
            Ok(s) => s,
            Err(e) => {
                out.error = Some(format!("pencarian pool gagal: {e}"));
                return out;
            }
        };

        // Tanpa desimal, seluruh perhitungan harga salah. Itu biasanya berarti
        // alamatnya bukan ERC20, dan tidak ada gunanya melanjutkan.
        let Some(meta) = snapshot.meta else {
            out.error = Some(format!(
                "{address} tidak menjawab decimals(), apakah ini ERC20?"
            ));
            return out;
        };
        out.meta = Some(meta);

        let groups = snapshot.quotes;

        if groups.is_empty() {
            out.error = Some(
                "tidak ada pool yang layak untuk token ini. Pool tanpa likuiditas, \
                 pool berfee mustahil, dan pool tanpa fee sengaja tidak ditampilkan \
                 karena tidak berguna untuk LP"
                    .into(),
            );
            return out;
        }

        // Kelompok teratas sudah diurutkan menurut total likuiditas, jadi ia
        // yang paling mungkin merupakan pasar sebenarnya.
        let terpilih = groups[0].clone();
        out.selected_quote = Some(terpilih.quote.clone());
        out.selected_pool_id = terpilih.pools.first().map(|p| p.id.clone());
        out.pools = terpilih.pools.clone();

        if groups.len() > 1 {
            let lain: Vec<String> = groups[1..]
                .iter()
                .map(|g| format!("{} ({} pool)", g.quote_symbol, g.pools.len()))
                .collect();
            tracing::info!(
                terpilih = %terpilih.quote_symbol,
                alternatif = %lain.join(", "),
                "token berpasangan dengan lebih dari satu kutipan"
            );
        }

        // Simpan sebagai token aktif supaya laporan harga dari chart yang
        // datang berikutnya punya konteks untuk dikalibrasi.
        if let (Some(meta), Some(selected)) = (out.meta.clone(), out.selected_pool_id.clone()) {
            *self.active.write().await = Some(ActiveToken {
                meta,
                quotes: groups.clone(),
                selected_quote: terpilih.quote.clone(),
                pools: terpilih.pools.clone(),
                selected_pool_id: selected,
                // Desimal kutipan ikut pasangan terpilih, bukan diandaikan 18.
                // USDG memakai 6, dan salah desimal menggeser harga sejuta kali.
                quote_decimals: terpilih.quote_decimals,
                requested: None,
                chart_mcap_usd: None,
            });
        }

        out.quotes = groups;
        out
    }

    /// Catat market cap dari chart, lalu kirim ulang tampilan range.
    ///
    /// Kalibrasi kurs bergantung sepenuhnya pada angka ini, jadi laporan yang
    /// tidak masuk akal diabaikan diam diam ketimbang merusak tampilan.
    pub async fn on_chart_price(&self, address: &str, mcap_usd: f64) {
        if !(mcap_usd.is_finite() && mcap_usd > 0.0) {
            return;
        }

        {
            let mut guard = self.active.write().await;
            let Some(active) = guard.as_mut() else { return };
            // Laporan untuk token lain berarti panel dan chart sedang tidak
            // menunjuk hal yang sama. Diabaikan sampai keduanya sepakat.
            if !active.meta.address.eq_ignore_ascii_case(address) {
                return;
            }
            active.chart_mcap_usd = Some(mcap_usd);
        }

        self.broadcast_range_view().await;
    }

    pub async fn on_select_pool(&self, pool_id: String) {
        {
            let mut guard = self.active.write().await;
            let Some(active) = guard.as_mut() else { return };
            if !active.pools.iter().any(|p| p.id == pool_id) {
                return;
            }
            active.selected_pool_id = pool_id;
            // Grid tick tiap pool berbeda, jadi batas yang diminta user pada
            // pool lama tidak lagi berlaku apa adanya. Dibiarkan supaya niat
            // user tetap terjaga, dan snap yang menyesuaikannya.
        }
        self.broadcast_range_view().await;
    }

    /// Pindah ke pasangan token lawan yang lain.
    ///
    /// Tidak menyentuh jaringan sama sekali. Seluruh kelompok sudah ada di
    /// memori sejak token dimuat, jadi berpindah pasangan hanya memilih
    /// ulang dari data yang sama. Desimal kutipan ikut berpindah, karena
    /// itu yang menentukan skala seluruh harga.
    pub async fn on_select_quote(&self, quote: String) {
        let keadaan = {
            let mut guard = self.active.write().await;
            let Some(active) = guard.as_mut() else { return };

            let Some(kelompok) = active
                .quotes
                .iter()
                .find(|g| g.quote.eq_ignore_ascii_case(&quote))
                .cloned()
            else {
                return;
            };
            if kelompok.pools.is_empty() {
                return;
            }

            active.selected_quote = kelompok.quote.clone();
            active.quote_decimals = kelompok.quote_decimals;
            active.selected_pool_id = kelompok.pools[0].id.clone();
            active.pools = kelompok.pools.clone();
            // Batas yang diminta user dinyatakan dalam market cap USD, jadi
            // ia tetap berlaku setelah berpindah pasangan. Yang berubah
            // hanya grid tick, dan snap yang menyesuaikannya.

            protocol::TokenState {
                address: active.meta.address.clone(),
                meta: Some(active.meta.clone()),
                quotes: active.quotes.clone(),
                pools: active.pools.clone(),
                selected_quote: Some(active.selected_quote.clone()),
                selected_pool_id: Some(active.selected_pool_id.clone()),
                error: None,
            }
        };

        let _ = self
            .events
            .send(protocol::ServerMessage::TokenState(Box::new(keadaan)));
        self.broadcast_range_view().await;
    }

    pub async fn on_set_range(&self, lower_mcap: f64, upper_mcap: f64) {
        if !(lower_mcap.is_finite() && upper_mcap.is_finite()) {
            return;
        }
        let (lo, hi) = if lower_mcap <= upper_mcap {
            (lower_mcap, upper_mcap)
        } else {
            // User menyeret garis bawah melewati garis atas. Keduanya bertukar
            // peran, sesuai acceptance criteria di PRD 3.3.
            (upper_mcap, lower_mcap)
        };

        {
            let mut guard = self.active.write().await;
            let Some(active) = guard.as_mut() else { return };
            active.requested = Some((lo, hi));
        }
        self.broadcast_range_view().await;
    }

    async fn broadcast_range_view(&self) {
        let guard = self.active.read().await;
        let Some(active) = guard.as_ref() else { return };
        let Some(pool) = active.selected_pool() else { return };
        let Some(mcap) = active.chart_mcap_usd else { return };

        match crate::range::build_view(
            pool,
            &active.meta,
            active.quote_decimals,
            mcap,
            active.requested,
        ) {
            Ok(view) => {
                let _ = self
                    .events
                    .send(protocol::ServerMessage::RangeView(Box::new(view)));
            }
            Err(e) => {
                tracing::debug!(error = %e, "tampilan range belum bisa disusun");
            }
        }
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

/// Sidik jari daftar pool, dipakai untuk memutuskan perlu siaran ulang atau tidak.
///
/// Harga sengaja TIDAK ikut, karena harga berubah setiap blok dan panel tidak
/// perlu digambar ulang untuk itu. Yang penting hanya apakah kumpulan pool dan
/// pilihannya berubah.
fn sidik_pool(state: &protocol::TokenState) -> String {
    let mut bagian: Vec<&str> = state.pools.iter().map(|p| p.id.as_str()).collect();
    bagian.sort_unstable();
    format!(
        "{}|{}|{}",
        state.selected_quote.as_deref().unwrap_or(""),
        state.selected_pool_id.as_deref().unwrap_or(""),
        bagian.join(",")
    )
}
