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

pub mod deposit;
pub mod execute;
pub mod intent;
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

use alloy::providers::Provider;

use crate::auth::PairingToken;
use crate::chain::{registry::Registry, ChainClient};
use crate::config::Config;
use crate::error::EngineError;
use crate::store::Store;
use crate::wallet::Wallet;
use protocol::{EngineStatus, RegistrySummary, ServerMessage};

pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Perkiraan biaya gas satu transaksi, dalam ETH.
///
/// Dipakai gate G6 untuk memastikan cadangan ETH tidak habis. Angkanya
/// sengaja dilebihkan: rantai ini murah, dan menolak transaksi karena terlalu
/// hati hati jauh lebih baik daripada wallet yang kehabisan gas di tengah
/// rangkaian aksi.
const PERKIRAAN_GAS_ETH: f64 = 0.0005;

/// Umur deadline yang dibawa MintParams, dalam detik.
///
/// Deadline terlalu panjang membuat transaksi yang tertunda tetap sah saat
/// harganya sudah jauh berubah. Lima menit cukup untuk antrean sequencer
/// paling buruk sekalipun pada rantai berblok 100 ms.
const DEADLINE_DETIK: u64 = 300;

/// Batas keras jumlah transaksi dalam satu alur tombol utama. Jalur normal
/// membutuhkan paling banyak beberapa approval, satu zap, lalu aksi akhir.
/// Batas ini mencegah replanning yang rusak mengulang transaksi tanpa akhir.
const MAX_FLOW_STEPS: usize = 12;

/// Deret candle beserta alamat token yang memilikinya.
///
/// Alamat ikut disimpan supaya deret milik token lama tidak terpakai untuk
/// token baru saat user berpindah, dan itu kesalahan yang hasilnya terlihat
/// masuk akal sehingga sulit disadari.
pub type DeretBar = (String, Vec<crate::signals::bar::Bar>);

#[derive(Clone)]
pub struct AppState {
    pub intent: Arc<intent::Intent>,
    pub execution_lock: Arc<Mutex<()>>,
    /// Menahan satu alur tombol utama sampai selesai. Tanpa kunci ini, dua
    /// pesan yang datang hampir bersamaan bisa mengirim approval pertama dua
    /// kali sebelum rencana sempat disusun ulang.
    pub flow_lock: Arc<Mutex<()>>,
    pub positions_lock: Arc<Mutex<()>>,
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
    pub registry_checks: Arc<tokio::sync::RwLock<Vec<crate::chain::RegistryCheck>>>,
    /// Token yang sedang dilihat user beserta pool terpilihnya.
    ///
    /// Satu slot saja, karena side panel hanya menampilkan satu token pada satu
    /// waktu. Kalau nanti panel bisa memantau beberapa token sekaligus, ini
    /// yang berubah jadi map.
    pub active: Arc<tokio::sync::RwLock<Option<ActiveToken>>>,
    /// Rencana terakhir yang sudah lolos gate, menunggu persetujuan user.
    ///
    /// Disimpan di engine, bukan dikirim bolak balik lewat panel. Rencana
    /// yang kembali dari panel bisa saja sudah diubah, dan bagian yang
    /// paling menarik untuk diubah justru calldata-nya.
    pub pending_plan: Arc<tokio::sync::RwLock<Option<crate::api::execute::RencanaTersimpan>>>,
    /// Penjaga nonce, satu untuk seluruh engine.
    ///
    /// Harus satu, karena dua penjaga akan memberikan nonce yang sama pada
    /// dua transaksi yang berbeda.
    pub nonce: Arc<crate::exec::nonce::NonceKeeper>,
    /// Posisi yang terakhir dimuat.
    ///
    /// Disimpan bukan untuk ditampilkan, melainkan untuk dibandingkan. Yang
    /// layak diberitahukan ke user adalah PERUBAHAN keadaan range, bukan
    /// keadaannya, dan perbandingan butuh keadaan sebelumnya.
    pub positions: Arc<tokio::sync::RwLock<Vec<crate::positions::Posisi>>>,
    /// Penjaga pendinginan notifikasi beserta riwayatnya.
    ///
    /// Satu untuk seluruh engine, karena pendinginan yang terpisah per
    /// koneksi tidak menahan apa pun saat panel dibuka tutup.
    pub notifier: Arc<tokio::sync::RwLock<crate::notify::Notifier>>,
    /// Aksi pada posisi yang sudah disusun, menunggu persetujuan.
    ///
    /// Disimpan di engine dengan alasan yang sama dengan rencana membuka
    /// posisi: calldata yang bolak balik lewat panel bisa kembali dalam
    /// keadaan sudah diubah.
    pub pending_action: Arc<tokio::sync::RwLock<Option<crate::api::execute::AksiTersimpan>>>,
    /// Deret candle terakhir dari chart, beserta alamat tokennya.
    ///
    /// Disimpan supaya detektor bisa dijalankan ulang dengan ambang yang
    /// berbeda tanpa menunggu kiriman berikutnya dari halaman gmgn.
    pub bars: Arc<tokio::sync::RwLock<Option<DeretBar>>>,
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

fn pool_dari_posisi(posisi: &crate::positions::Posisi) -> Result<crate::pools::PoolInfo, String> {
    Ok(crate::pools::PoolInfo {
        version: posisi.version,
        id: posisi.position_id.clone(),
        token0: posisi.token0.clone(),
        token1: posisi.token1.clone(),
        fee: posisi.fee,
        dynamic_fee: posisi.fee & crate::pools::DYNAMIC_FEE_FLAG != 0,
        tick_spacing: posisi.tick_spacing,
        hooks: posisi.hooks.clone(),
        sqrt_price_x96: posisi
            .sqrt_price_x96
            .clone()
            .ok_or("harga pool posisi belum terbaca")?,
        tick: posisi
            .tick_sekarang
            .ok_or("tick pool posisi belum terbaca")?,
        liquidity: posisi.liquidity.clone(),
        raw_price: f64::NAN,
        stats: None,
    })
}

fn jumlah_aksi(input: Option<&str>, decimals: u8) -> Result<alloy::primitives::U256, String> {
    let Some(value) = input.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(alloy::primitives::U256::ZERO);
    };
    if value.bytes().all(|byte| byte == b'0' || byte == b'.') {
        return Ok(alloy::primitives::U256::ZERO);
    }
    deposit::units(value, decimals)
}

fn batas_atas_aksi(
    dibutuhkan: alloy::primitives::U256,
    slippage_bps: u32,
    tersedia: alloy::primitives::U256,
) -> alloy::primitives::U256 {
    let bps = alloy::primitives::U256::from(10_000u32);
    let tambahan = alloy::primitives::U256::from(slippage_bps.min(10_000));
    (dibutuhkan * (bps + tambahan) / bps)
        .min(tersedia)
        .max(dibutuhkan.min(tersedia))
}

fn ke_u128_aksi(value: alloy::primitives::U256, nama: &str) -> Result<u128, String> {
    value
        .try_into()
        .map_err(|_| format!("{nama} melampaui batas uint128 kontrak"))
}

fn token_id_u256(value: &str) -> Result<alloy::primitives::U256, String> {
    value
        .parse()
        .map_err(|_| format!("token id `{value}` bukan angka"))
}

fn nilai_usd_aksi(
    posisi: &crate::positions::Posisi,
    meta0: &crate::pools::TokenMeta,
    meta1: &crate::pools::TokenMeta,
    amount0: alloy::primitives::U256,
    amount1: alloy::primitives::U256,
    active: &ActiveToken,
) -> Option<f64> {
    let mcap = active.chart_mcap_usd?;
    if !(mcap.is_finite() && mcap > 0.0 && active.meta.total_supply.is_finite())
        || active.meta.total_supply <= 0.0
    {
        return None;
    }
    let meme0 = posisi.token0.eq_ignore_ascii_case(&active.meta.address);
    let meme1 = posisi.token1.eq_ignore_ascii_case(&active.meta.address);
    if !meme0 && !meme1 {
        return None;
    }
    let sqrt: alloy::primitives::U256 = posisi.sqrt_price_x96.as_ref()?.parse().ok()?;
    let raw = crate::pools::raw_price_from_sqrt_x96(sqrt);
    if !(raw.is_finite() && raw > 0.0) {
        return None;
    }
    let human_ratio = raw * 10f64.powi(i32::from(meta0.decimals) - i32::from(meta1.decimals));
    if !(human_ratio.is_finite() && human_ratio > 0.0) {
        return None;
    }
    let meme_usd = mcap / active.meta.total_supply;
    let (price0, price1) = if meme0 {
        (meme_usd, meme_usd / human_ratio)
    } else {
        (meme_usd * human_ratio, meme_usd)
    };
    let value = f64::from(amount0) / 10f64.powi(i32::from(meta0.decimals)) * price0
        + f64::from(amount1) / 10f64.powi(i32::from(meta1.decimals)) * price1;
    (value.is_finite() && value >= 0.0).then_some(value)
}

impl AppState {
    pub async fn status(&self) -> EngineStatus {
        let chain = self.chain.health().await;
        let wallet = self.wallet.lock().await.status();
        let (_, mut blocked_reasons) = protocol::evaluate_execution_readiness(&chain, &wallet);
        let registry = self.registry_summary().await;
        if registry.total == 0 || registry.verified != registry.total {
            blocked_reasons.push("G1 belum memverifikasi seluruh kontrak yang dipin".into());
        }
        if let Err(e) = crate::exec::golden::verify_embedded() {
            blocked_reasons.push(format!("G3: {e}"));
        }
        let can_execute = blocked_reasons.is_empty();

        EngineStatus {
            engine_version: ENGINE_VERSION.to_string(),
            uptime_secs: self.started_at.elapsed().as_secs(),
            chain,
            wallet,
            registry,
            can_execute,
            blocked_reasons,
            // Disunting saat dibuat, bukan saat ditampilkan. Uji struktural di
            // chain/mod.rs menjaga kunci Alchemy tidak pernah sampai ke sini.
            rpc_http: self
                .cfg
                .rpc
                .http
                .iter()
                .map(|u| crate::chain::endpoint::aman(u))
                .collect(),
        }
    }

    pub async fn reverify_registry(&self) -> Result<(), String> {
        match self.chain.verify_registry(&self.registry).await {
            Ok(checks) => {
                let total = self
                    .registry
                    .all_contracts()
                    .map_err(|e| e.to_string())?
                    .len();
                let valid = total > 0
                    && checks.len() == total
                    && checks
                        .iter()
                        .all(|c| c.status == crate::chain::RegistryStatus::Verified);
                *self.registry_checks.write().await = checks;
                if valid {
                    Ok(())
                } else {
                    Err("G1: bytecode kontrak belum seluruhnya cocok dengan pin".into())
                }
            }
            Err(e) => {
                self.registry_checks.write().await.clear();
                Err(format!("G1 tidak bisa diverifikasi: {e}"))
            }
        }
    }

    async fn periksa_hook_rencana(&self, plan: &execute::RencanaTersimpan) -> Result<(), String> {
        let provider = self.chain.provider().await.map_err(|e| e.to_string())?;
        crate::risk::policy::periksa(&provider, &self.store, plan.hook)
            .await
            .map_err(|e| format!("kebijakan hook: {e}"))
    }

    async fn periksa_hook_aksi(&self, plan: &execute::AksiTersimpan) -> Result<(), String> {
        let provider = self.chain.provider().await.map_err(|e| e.to_string())?;
        crate::risk::policy::periksa(&provider, &self.store, plan.hook)
            .await
            .map_err(|e| format!("kebijakan hook: {e}"))
    }

    async fn ensure_execution(&self) -> Result<(), String> {
        if self.cfg.execution.dry_run {
            return Err("engine sedang dry run".into());
        }
        let health = self.chain.check_health().await;
        if !health.healthy {
            return Err(health.error.unwrap_or_else(|| "RPC belum sehat".into()));
        }
        self.reverify_registry().await?;
        crate::exec::golden::verify_embedded()?;
        Ok(())
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
        let ticket = self.intent.begin_lookup();
        let state = self.clone();
        tokio::spawn(async move {
            {
                let mut active = state.active.write().await;
                if !state.intent.lookup_current(ticket) {
                    return;
                }
                *active = None;
            }
            let cepat = state
                .build_token_state(
                    address.clone(),
                    crate::pools::ModePencarian::CacheSaja,
                    ticket,
                )
                .await;
            if !state.intent.lookup_current(ticket) {
                return;
            }

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
                .build_token_state(address, crate::pools::ModePencarian::Penuh, ticket)
                .await;
            if !state.intent.lookup_current(ticket) {
                return;
            }

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
        ticket: u64,
    ) -> protocol::TokenState {
        let mut out = protocol::TokenState {
            address: address.clone(),
            meta: None,
            quotes: Vec::new(),
            pools: Vec::new(),
            selected_quote: None,
            selected_pool_id: None,
            saldo: None,
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

        out.saldo = self.baca_saldo(&provider, &out.meta, &terpilih).await;

        // Simpan sebagai token aktif supaya laporan harga dari chart yang
        // datang berikutnya punya konteks untuk dikalibrasi.
        if let (Some(meta), Some(selected)) = (out.meta.clone(), out.selected_pool_id.clone()) {
            let mut guard = self.active.write().await;
            if !self.intent.lookup_current(ticket) {
                return out;
            }
            let sebelumnya = guard
                .as_ref()
                .filter(|a| a.meta.address.eq_ignore_ascii_case(&address));
            let kelompok = sebelumnya
                .and_then(|a| groups.iter().find(|g| g.quote == a.selected_quote))
                .unwrap_or(&terpilih);
            let selected = sebelumnya
                .and_then(|a| kelompok.pools.iter().find(|p| p.id == a.selected_pool_id))
                .or_else(|| kelompok.pools.first())
                .map(|p| p.id.clone())
                .unwrap_or(selected);
            let requested = sebelumnya.and_then(|a| a.requested);
            let chart_mcap_usd = sebelumnya.and_then(|a| a.chart_mcap_usd);
            out.selected_quote = Some(kelompok.quote.clone());
            out.selected_pool_id = Some(selected.clone());
            out.pools = kelompok.pools.clone();
            if kelompok.quote != terpilih.quote {
                out.saldo = None;
            }
            *guard = Some(ActiveToken {
                meta,
                quotes: groups.clone(),
                selected_quote: kelompok.quote.clone(),
                pools: kelompok.pools.clone(),
                selected_pool_id: selected,
                // Desimal kutipan ikut pasangan terpilih, bukan diandaikan 18.
                // USDG memakai 6, dan salah desimal menggeser harga sejuta kali.
                quote_decimals: kelompok.quote_decimals,
                requested,
                chart_mcap_usd,
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
            self.intent.change();
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
            self.intent.change();

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
                // Saldo menyusul lewat kiriman kedua. Membacanya di sini
                // berarti berpindah pasangan menyentuh jaringan, dan itu
                // membuat perpindahan yang seharusnya seketika terasa berat.
                saldo: None,
                error: None,
            }
        };

        let _ = self
            .events
            .send(protocol::ServerMessage::TokenState(Box::new(keadaan)));
        self.broadcast_range_view().await;
        self.kirim_saldo_menyusul().await;
    }

    /// Ukur statistik dan saldo pasangan yang sedang aktif, lalu kirim ulang.
    ///
    /// KENAPA MENYUSUL, BUKAN LANGSUNG
    /// Berpindah pasangan tidak boleh menyentuh jaringan, karena seluruh
    /// kelompok sudah ada di memori dan perpindahannya harus terasa seketika.
    /// Tapi APR, TVL, dan volume hanya diukur untuk kelompok teratas saat token
    /// dimuat, sehingga kelompok lain tidak punya angka apa pun sampai diukur.
    ///
    /// Kegagalannya tidak mengubah apa pun: panel tetap menampilkan keadaan
    /// yang sudah diterima, hanya tanpa angka.
    async fn kirim_saldo_menyusul(&self) {
        let s = self.clone();
        tokio::spawn(async move {
            let Ok(provider) = s.chain.provider().await else {
                return;
            };

            let (meta, kelompok) = {
                let guard = s.active.read().await;
                let Some(active) = guard.as_ref() else { return };
                let Some(k) = active
                    .quotes
                    .iter()
                    .find(|g| g.quote.eq_ignore_ascii_case(&active.selected_quote))
                    .cloned()
                else {
                    return;
                };
                (Some(active.meta.clone()), k)
            };

            let saldo = s.baca_saldo(&provider, &meta, &kelompok).await;

            // Saldo adalah angka yang langsung dibutuhkan kotak amount. Kirim
            // begitu RPC menjawab; pengukuran APR/TVL di bawah tidak boleh
            // menahannya beberapa detik.
            {
                let guard = s.active.read().await;
                let Some(active) = guard.as_ref() else { return };
                if !meta
                    .as_ref()
                    .is_some_and(|m| m.address.eq_ignore_ascii_case(&active.meta.address))
                    || !active.selected_quote.eq_ignore_ascii_case(&kelompok.quote)
                {
                    return;
                }
                let _ = s.events.send(protocol::ServerMessage::TokenState(Box::new(
                    protocol::TokenState {
                        address: active.meta.address.clone(),
                        meta: Some(active.meta.clone()),
                        quotes: active.quotes.clone(),
                        pools: active.pools.clone(),
                        selected_quote: Some(active.selected_quote.clone()),
                        selected_pool_id: Some(active.selected_pool_id.clone()),
                        saldo: saldo.clone(),
                        error: None,
                    },
                )));
            }

            // Statistik diukur untuk kelompok yang sedang dipilih, bukan hanya
            // yang teratas. Tanpa ini, setiap kali user berpindah pasangan,
            // kolom APR, TVL, dan volume kosong tanpa penjelasan.
            let mut kelompok = kelompok;
            let perlu_stats = kelompok.pools.iter().all(|p| p.stats.is_none());
            if !perlu_stats {
                return;
            }
            if let Some(m) = meta.as_ref() {
                crate::pools::ukur_stats(&provider, &s.registry, m, &mut kelompok).await;
            }

            {
                // Hasil pengukuran disimpan ke keadaan aktif supaya tidak
                // diukur ulang setiap kali user bolak balik antar pasangan.
                let mut guard = s.active.write().await;
                if let Some(active) = guard.as_mut() {
                    if !meta
                        .as_ref()
                        .is_some_and(|m| m.address.eq_ignore_ascii_case(&active.meta.address))
                    {
                        return;
                    }
                    if !active.selected_quote.eq_ignore_ascii_case(&kelompok.quote) {
                        return;
                    }
                    if active.selected_quote.eq_ignore_ascii_case(&kelompok.quote) {
                        active.pools = kelompok.pools.clone();
                    }
                    if let Some(g) = active
                        .quotes
                        .iter_mut()
                        .find(|g| g.quote.eq_ignore_ascii_case(&kelompok.quote))
                    {
                        *g = kelompok.clone();
                    }
                }
            }

            let guard = s.active.read().await;
            let Some(active) = guard.as_ref() else { return };
            if !meta
                .as_ref()
                .is_some_and(|m| m.address.eq_ignore_ascii_case(&active.meta.address))
                || !active.selected_quote.eq_ignore_ascii_case(&kelompok.quote)
            {
                return;
            }
            let _ = s.events.send(protocol::ServerMessage::TokenState(Box::new(
                protocol::TokenState {
                    address: active.meta.address.clone(),
                    meta: Some(active.meta.clone()),
                    quotes: active.quotes.clone(),
                    pools: active.pools.clone(),
                    selected_quote: Some(active.selected_quote.clone()),
                    selected_pool_id: Some(active.selected_pool_id.clone()),
                    saldo,
                    error: None,
                },
            )));
        });
    }

    /// Susun rencana membuka posisi, jalankan seluruh gate, TIDAK mengirim.
    ///
    /// Seluruh angka datang dari keadaan yang sedang aktif: pool terpilih,
    /// range yang sedang digambar, dan saldo wallet. Panel hanya menyebut
    /// nominalnya, karena itu satu satunya hal yang memang keputusan user.
    pub async fn on_quote_range(&self, task: execute::QuoteTask) {
        let revision = task.revision;
        match self.susun_rencana(task).await {
            Ok(view) => {
                if !self.intent.current(revision) {
                    return;
                }
                let _ = self
                    .events
                    .send(protocol::ServerMessage::OpenPlan(Box::new(view)));
            }
            Err(pesan) => self.alert_kritis("plan_failed", pesan),
        }
    }

    /// Beri preview nominal dari snapshot pool yang sudah berada di memori.
    ///
    /// Jalur ini sengaja tidak menyentuh RPC, wallet, allowance, simulasi,
    /// atau estimasi gas. Rencana penuh tetap menyusul lewat `quote_range`.
    pub async fn on_preview_deposit(&self, deposit: deposit::Deposit, request_id: String) {
        let hasil = {
            let guard = self.active.read().await;
            let active = guard
                .as_ref()
                .ok_or_else(|| "belum ada token yang dimuat".to_string());
            active.and_then(|active| {
                let pool = active
                    .selected_pool()
                    .ok_or_else(|| "pool terpilih tidak tersedia".to_string())?;
                let kelompok = active
                    .quotes
                    .iter()
                    .find(|q| q.quote.eq_ignore_ascii_case(&active.selected_quote))
                    .ok_or_else(|| "pasangan kutipan tidak tersedia".to_string())?;
                let mcap = active
                    .chart_mcap_usd
                    .ok_or_else(|| "harga chart belum tersedia".to_string())?;
                let view = crate::range::build_view(
                    pool,
                    &active.meta,
                    active.quote_decimals,
                    mcap,
                    active.requested,
                    self.quote_adalah_eth(&active.selected_quote),
                )
                .map_err(|_| "range belum tersedia".to_string())?;
                let (lower, upper) = execute::tick_menaik(view.lower.tick, view.upper.tick)
                    .ok_or_else(|| "range terlalu sempit".to_string())?;
                let meme0 = pool.token0.eq_ignore_ascii_case(&active.meta.address);
                let source0 = (deposit.side == deposit::Side::Meme) == meme0;
                let decimals = if deposit.side == deposit::Side::Meme {
                    active.meta.decimals
                } else {
                    active.quote_decimals
                };
                let amount = deposit::units(&deposit.amount, decimals)?;
                let (amount0, amount1) =
                    deposit::budget(pool, lower, upper, amount, source0, deposit.zap)?;
                Ok(Self::ringkas_jumlah_setoran(
                    pool,
                    &active.meta,
                    &kelompok.quote_symbol,
                    active.quote_decimals,
                    amount0,
                    amount1,
                ))
            })
        };

        let (setor, error) = match hasil {
            Ok(setor) => (Some(setor), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let _ = self.events.send(protocol::ServerMessage::DepositPreview {
            request_id,
            deposit,
            setor,
            error,
        });
    }

    async fn susun_rencana(
        &self,
        task: execute::QuoteTask,
    ) -> Result<execute::OpenPlanView, String> {
        let revision = task.revision;
        if !self.intent.current(revision) {
            return Err("permintaan sudah berubah".into());
        }

        // Wallet harus terbuka lebih dulu, karena penerima posisi adalah
        // alamatnya. Menyusun rencana untuk alamat yang belum diketahui berarti
        // menyusun rencana yang tidak bisa dikirim.
        let pemilik = {
            let w = self.wallet.lock().await;
            match (w.exists(), w.is_locked(), w.address()) {
                (false, _, _) => return Err("wallet belum dibuat".into()),
                (_, true, _) => return Err("wallet masih terkunci".into()),
                (_, _, Some(a)) => a,
                _ => return Err("alamat wallet tidak diketahui".into()),
            }
        };

        let (pool, token_alamat, quote_alamat) = {
            let guard = self.active.read().await;
            let active = guard.as_ref().ok_or("belum ada token yang dimuat")?;
            let pool = active
                .selected_pool()
                .ok_or("pool terpilih tidak ada di daftar")?
                .clone();
            (
                pool,
                active.meta.address.clone(),
                active.selected_quote.clone(),
            )
        };

        // Range yang dipakai persis yang sedang tergambar di chart, termasuk
        // pembulatan ke grid tick. Menghitungnya ulang di sini dengan cara lain
        // akan membuat yang dikirim berbeda dari yang dilihat user.
        let provider = self.chain.provider().await.map_err(|e| e.to_string())?;
        let pool = crate::pools::refresh(&provider, &self.registry, &pool)
            .await
            .map_err(|e| e.to_string())?;

        // Pasangkan snapshot pool yang baru selesai dibaca dengan kalibrasi
        // chart paling baru. Market cap sengaja diambil setelah RPC, bukan dari
        // salinan sebelum RPC. Pilihan token, pasangan, atau pool yang berubah
        // selama penantian membuat permintaan ini kedaluwarsa.
        if !self.intent.current(revision) {
            return Err("permintaan sudah berubah".into());
        }
        let (meta, quote_decimals, requested, mcap) = {
            let guard = self.active.read().await;
            let active = guard.as_ref().ok_or("token aktif sudah berubah")?;
            if !active.meta.address.eq_ignore_ascii_case(&token_alamat)
                || !active.selected_quote.eq_ignore_ascii_case(&quote_alamat)
                || active.selected_pool_id != pool.id
            {
                return Err("token, pasangan, atau pool berubah saat snapshot dibaca".into());
            }
            (
                active.meta.clone(),
                active.quote_decimals,
                active.requested,
                active
                    .chart_mcap_usd
                    .ok_or("harga dari chart gmgn belum masuk, kurs belum bisa dikalibrasi")?,
            )
        };
        let hook = if pool.version == crate::pools::PoolVersion::V4 {
            Some(
                pool.hooks
                    .as_ref()
                    .ok_or("alamat hook V4 belum tersedia")?
                    .parse::<alloy::primitives::Address>()
                    .map_err(|_| "alamat hook V4 tidak valid")?,
            )
        } else {
            None
        };
        crate::risk::policy::periksa(&provider, &self.store, hook)
            .await
            .map_err(|e| format!("kebijakan hook: {e}"))?;
        let view = crate::range::build_view(
            &pool,
            &meta,
            quote_decimals,
            mcap,
            requested,
            self.quote_adalah_eth(&quote_alamat),
        )
        .map_err(|e| format!("range belum bisa disusun: {e}"))?;

        // Nama `lower` dan `upper` di RangeView mengikuti harga, bukan tick.
        // Keduanya bertukar saat memecoin menempati token1. Lihat
        // execute::tick_menaik.
        let (tick_lower, tick_upper) = execute::tick_menaik(view.lower.tick, view.upper.tick)
            .ok_or("range terlalu sempit, kedua batasnya jatuh di tick yang sama")?;

        let token0: alloy::primitives::Address = pool
            .token0
            .parse()
            .map_err(|_| format!("alamat token0 pool {} tidak valid", pool.id))?;
        let token1: alloy::primitives::Address = pool
            .token1
            .parse()
            .map_err(|_| format!("alamat token1 pool {} tidak valid", pool.id))?;

        let provider = self
            .chain
            .provider()
            .await
            .map_err(|e| format!("RPC tidak bisa dipakai: {e}"))?;
        // Alamat kontrak dipilih menurut versi pool, di satu tempat. Mengirim
        // mint V4 ke NonfungiblePositionManager v3 akan revert, dan sebaliknya.
        let alamat = crate::exec::plan::AlamatEksekusi::dari_registry(&self.registry)
            .map_err(|e| e.to_string())?;
        let multicall = alamat.multicall;

        let (saldo0, saldo1) =
            execute::saldo_dua_token(&provider, multicall, pemilik, token0, token1).await?;

        let meme0 = pool.token0.eq_ignore_ascii_case(&meta.address);
        let source0 = (task.deposit.side == deposit::Side::Meme) == meme0;
        let decimals = if task.deposit.side == deposit::Side::Meme {
            meta.decimals
        } else {
            quote_decimals
        };
        let amount = deposit::units(&task.deposit.amount, decimals)?;
        let (available0, available1) = match task.budget_override {
            Some(budget) => budget,
            None => deposit::budget(
                &pool,
                tick_lower,
                tick_upper,
                amount,
                source0,
                task.deposit.zap,
            )?,
        };
        if available0 > saldo0 || available1 > saldo1 {
            return Err("saldo tidak cukup untuk setoran yang diminta dan rasio range".into());
        }
        if available0.is_zero() && available1.is_zero() {
            return Err("jumlah setoran nol".into());
        }
        let (meme, quote) = if meme0 {
            (available0, available1)
        } else {
            (available1, available0)
        };
        let size_usd = f64::from(meme) / 10f64.powi(meta.decimals as i32) * view.current.price_usd
            + f64::from(quote) / 10f64.powi(quote_decimals as i32) * view.eth_usd;
        if !size_usd.is_finite() || size_usd <= 0.0 {
            return Err("nilai setoran tidak dapat dihitung".into());
        }

        // Pembatas opsional harus memakai keadaan persisten dan pembacaan
        // posisi aktual. Kalau pembacaan yang dibutuhkan gagal, rencana
        // ditolak alih alih menyisipkan nol yang tampak sah.
        let perlu_usage = self.cfg.limits.max_daily_deploy_usd.is_some()
            || self.cfg.limits.new_pool_first_size_usd.is_some();
        let usage = if perlu_usage {
            crate::exec::journal::limit_usage(
                &self.store,
                self.registry.chain.id,
                pemilik,
                chrono::Utc::now(),
            )
            .map_err(|e| format!("penggunaan limit tidak terbaca: {e}"))?
        } else {
            crate::exec::journal::LimitUsage::default()
        };
        let open_positions = if self.cfg.limits.max_open_positions.is_some() {
            let _pemuatan = self.positions_lock.lock().await;
            let snapshot = crate::positions::muat::muat_snapshot_operasional(
                &provider,
                &self.registry,
                Some(&self.store),
                pemilik,
            )
            .await;
            if !snapshot.errors.is_empty() {
                return Err(format!(
                    "jumlah posisi terbuka tidak dapat dibuktikan: {}",
                    snapshot.errors.join("; ")
                ));
            }
            u32::try_from(snapshot.positions.iter().filter(|p| !p.kosong).count())
                .map_err(|_| "jumlah posisi terbuka melampaui u32".to_string())?
        } else {
            0
        };
        let is_new_pool = self.cfg.limits.new_pool_first_size_usd.is_some()
            && !usage.used_pools.contains(&pool.id);

        let eth_balance = provider
            .get_balance(pemilik)
            .await
            .map_err(|e| format!("gagal membaca saldo ETH: {e}"))?;
        let eth_balance_f64 = eth_balance.to_string().parse::<f64>().unwrap_or(0.0) / 1e18;

        let eth_komitmen = if token0.is_zero() {
            available0.to_string().parse::<f64>().unwrap_or(0.0) / 1e18
        } else if token1.is_zero() {
            available1.to_string().parse::<f64>().unwrap_or(0.0) / 1e18
        } else {
            0.0
        };

        let req = execute::susun_permintaan(
            &pool,
            tick_lower,
            tick_upper,
            available0,
            available1,
            pemilik,
            size_usd,
            usage.daily_committed_usd,
            open_positions,
            is_new_pool,
            eth_balance_f64,
            // Biaya gas ditambah ETH yang benar benar masuk ke posisi. Untuk
            // pool yang kedua sisinya token, yang kedua nol. Untuk pool
            // berpasangan ETH asli, mengabaikannya membuat gate G6 mengira
            // cadangan masih utuh padahal modalnya diambil dari sana juga.
            PERKIRAAN_GAS_ETH + eth_komitmen,
            DEADLINE_DETIK,
        );

        let mut plan = crate::exec::plan::plan_open_auto(
            &provider,
            &alamat,
            &req,
            &self.cfg.limits,
            &self.cfg.execution,
        )
        .await;

        let amount0: alloy::primitives::U256 = plan.amount0.parse().unwrap_or_default();
        let amount1: alloy::primitives::U256 = plan.amount1.parse().unwrap_or_default();

        // Zap dihitung setelah rencana, karena ia butuh tahu berapa yang benar
        // benar terpakai. Kalau modal sudah cocok rasio, hasilnya None.
        let zap = if task.deposit.zap && task.budget_override.is_none() {
            self.hitung_zap(
                &provider, &alamat, &pool, tick_lower, tick_upper, available0, available1, amount0,
                amount1,
            )
            .await
        } else {
            None
        };

        // Untuk posisi baru, izin mengikuti ANGGARAN yang sudah dibatasi input
        // user, bukan jumlah mint pada satu snapshot harga. Jumlah mint dapat
        // bergeser sedikit ketika approval masuk blok; memakai angka snapshot
        // membuat Permit2 kurang lagi setelah setiap quote ulang dan memicu
        // rangkaian approval tanpa akhir. Anggaran tetap menjadi batas keras:
        // engine tidak pernah mengizinkan lebih dari dana flow ini.
        let allowances = crate::exec::plan::izin_yang_kurang(
            &provider,
            &alamat,
            pool.version,
            pemilik,
            token0,
            available0,
            token1,
            available1,
            chrono::Utc::now().timestamp().max(0) as u64,
        )
        .await?;
        let disusun_pada = chrono::Utc::now();
        let expires_at = (disusun_pada
            + chrono::Duration::seconds(execute::RENCANA_KEDALUWARSA_DETIK))
        .to_rfc3339();

        // Swap zap berjalan lewat UniversalRouter, yang menarik token lewat
        // Permit2 juga. Izin ke PositionManager tidak berlaku untuknya.
        let mut allowances = if zap.is_some() {
            Vec::new()
        } else {
            allowances
        };
        if let Some((v, _, _)) = zap.as_ref() {
            if let (Ok(router), Ok(dari)) = (
                self.registry.contract("uniswap_v4", "universal_router"),
                v.dari.parse::<alloy::primitives::Address>(),
            ) {
                let jumlah: alloy::primitives::U256 = v.jumlah.parse().unwrap_or_default();
                match crate::exec::plan::izin_permit2_untuk(
                    &provider,
                    &alamat,
                    pemilik,
                    router,
                    "UniversalRouter",
                    dari,
                    jumlah,
                    chrono::Utc::now().timestamp().max(0) as u64,
                )
                .await
                {
                    Ok(mut l) => allowances.append(&mut l),
                    Err(e) => return Err(format!("izin router untuk zap tidak terbaca: {e}")),
                }
            }
        }

        // R1 memakai simulasi mint yang persis akan dikirim. Core hanya
        // menerima penyelesaian jika saldo yang masuk penuh, sehingga tax pada
        // jalur wallet, manager, lalu pool membuat simulasi ini gagal.
        let jalur_lp_lolos = plan.gates.iter().any(|g| g.id == "G4" && g.passed)
            && plan.gates.iter().any(|g| g.id == "G5" && g.passed);
        plan.gates.push(if jalur_lp_lolos {
            crate::exec::pipeline::GateResult::lolos_publik(
                "R1",
                "transfer tax jalur LP",
                "mint tersimulasi dengan penyelesaian jumlah penuh",
            )
        } else {
            crate::exec::pipeline::GateResult::gagal_publik(
                "R1",
                "transfer tax jalur LP",
                "jalur LP belum berhasil disimulasikan; selesaikan izin lalu susun ulang",
            )
        });

        // R2 menjual kira kira sepuluh sen lewat router nyata dengan allowance
        // nyata. eth_call menjalankan transfer lalu membuang seluruh perubahan.
        let memecoin = if meme0 { token0 } else { token1 };
        let saldo_meme = if meme0 { saldo0 } else { saldo1 };
        let jumlah_uji =
            execute::usd_ke_satuan_terkecil(0.10, view.current.price_usd, meta.decimals)
                .map(|jumlah| {
                    jumlah
                        .max(alloy::primitives::U256::from(1u8))
                        .min(saldo_meme)
                })
                .filter(|jumlah| !jumlah.is_zero());

        let mut izin_risiko_kurang = false;
        if let Some(jumlah) = jumlah_uji {
            let router = match pool.version {
                crate::pools::PoolVersion::V3 => self
                    .registry
                    .contract("uniswap_v3", "swap_router_02")
                    .map_err(|e| e.to_string())?,
                crate::pools::PoolVersion::V4 => self
                    .registry
                    .contract("uniswap_v4", "universal_router")
                    .map_err(|e| e.to_string())?,
            };
            let izin_risiko = match pool.version {
                crate::pools::PoolVersion::V3 => {
                    crate::exec::plan::izin_erc20_untuk(
                        &provider,
                        &alamat,
                        pemilik,
                        router,
                        "SwapRouter02",
                        memecoin,
                        jumlah,
                    )
                    .await?
                }
                crate::pools::PoolVersion::V4 => {
                    crate::exec::plan::izin_permit2_untuk(
                        &provider,
                        &alamat,
                        pemilik,
                        router,
                        "UniversalRouter",
                        memecoin,
                        jumlah,
                        chrono::Utc::now().timestamp().max(0) as u64,
                    )
                    .await?
                }
            };
            izin_risiko_kurang = !izin_risiko.is_empty();
            for langkah in izin_risiko {
                if !allowances.iter().any(|ada| {
                    ada.jenis == langkah.jenis
                        && ada.token.eq_ignore_ascii_case(&langkah.token)
                        && ada.spender.eq_ignore_ascii_case(&langkah.spender)
                }) {
                    allowances.push(langkah);
                }
            }
        }

        let jual_kecil_lolos = match jumlah_uji {
            None => {
                plan.gates.push(crate::exec::pipeline::GateResult::gagal_publik(
                    "R2",
                    "simulasi jual kecil",
                    "wallet belum mempunyai memecoin untuk uji jual. Jalankan zap pembelian lebih dulu atau sisakan sedikit token",
                ));
                false
            }
            Some(_) if izin_risiko_kurang => {
                plan.gates.push(crate::exec::pipeline::GateResult::gagal_publik(
                    "R2",
                    "simulasi jual kecil",
                    "izin router untuk uji jual belum lengkap; selesaikan langkah izin lalu susun ulang",
                ));
                false
            }
            Some(jumlah) => match crate::risk::token::uji_jual_kecil(
                &provider,
                &self.registry,
                &pool,
                pemilik,
                memecoin,
                jumlah,
                self.cfg.limits.max_slippage_bps,
            )
            .await
            {
                Ok(keluaran) => {
                    plan.gates.push(crate::exec::pipeline::GateResult::lolos_publik(
                        "R2",
                        "simulasi jual kecil",
                        format!(
                            "router menerima jual {jumlah} unit dan mengutip {keluaran} unit keluaran"
                        ),
                    ));
                    true
                }
                Err(error) => {
                    plan.gates
                        .push(crate::exec::pipeline::GateResult::gagal_publik(
                            "R2",
                            "simulasi jual kecil",
                            error.to_string(),
                        ));
                    false
                }
            },
        };
        if let Err(error) = crate::risk::token::simpan_bukti(
            &self.store,
            self.registry.chain.id,
            &pool.id,
            memecoin,
            jalur_lp_lolos,
            jual_kecil_lolos,
        ) {
            tracing::warn!(error = %error, "bukti risiko token gagal disimpan");
        }

        let (setor, apr_estimasi_mentah) = self.ringkas_setoran(&pool, &plan).await;

        // Harga gas dibaca sekarang, bukan diandaikan. Ia berubah dari blok ke
        // blok, dan biaya yang dihitung dari angka tetap akan meleset persis
        // saat rantai sedang sibuk, yaitu saat angkanya paling penting.
        let (gas_biaya, gas_gwei) = match provider.get_gas_price().await {
            Ok(wei) => {
                let harga = wei as f64;
                (
                    Some(plan.gas_estimate as f64 * harga / 1e18),
                    Some(harga / 1e9),
                )
            }
            Err(e) => {
                tracing::warn!(error = %e, "harga gas tidak terbaca");
                (None, None)
            }
        };

        let bars = self
            .bars
            .read()
            .await
            .as_ref()
            .filter(|(address, _)| address.eq_ignore_ascii_case(&meta.address))
            .map(|(_, bars)| bars.clone())
            .unwrap_or_default();
        let aturan_sideways = crate::learning::aturan_sideways(&self.store);
        let sideways = crate::signals::sideways::ukur(&bars, &aturan_sideways);
        let basis =
            crate::signals::dump_basis::cari(&bars, &crate::signals::dump_basis::Aturan::default());
        let protocol_version = match pool.version {
            crate::pools::PoolVersion::V3 => "v3",
            crate::pools::PoolVersion::V4 => "v4",
        };
        let gas_kutipan = setor
            .as_ref()
            .filter(|value| value.simbol_quote == "ETH" || value.simbol_quote == "WETH")
            .and(gas_biaya);
        let learning = crate::learning::create_draft(
            &self.store,
            crate::learning::DraftInput {
                token: &meta.address,
                pool_id: &pool.id,
                protocol_version,
                fee_tier: pool.fee,
                range_lower: view.lower.mcap_usd,
                range_upper: view.upper.mcap_usd,
                current_price: view.current.mcap_usd,
                planned_liquidity: plan.liquidity.parse().unwrap_or_default(),
                active_liquidity: pool.liquidity.parse().unwrap_or_default(),
                raw_fee_apr: apr_estimasi_mentah,
                position_value_quote: setor.as_ref().map(|value| value.nilai_quote),
                gas_cost_quote: gas_kutipan,
                bars: &bars,
                sideways: sideways.as_ref(),
                basis: basis.as_ref(),
            },
        );
        let apr_estimasi = learning.feature.predicted_fee_apr;

        for (i, step) in allowances.iter_mut().enumerate() {
            step.step_id = i.to_string();
        }
        let plan_id = intent::plan_id()?;
        {
            let mut pending = self.pending_plan.write().await;
            if !self.intent.current(revision) {
                return Err("permintaan berubah saat rencana disusun".into());
            }
            *pending = Some(execute::RencanaTersimpan {
                position_usd: size_usd,
                hook,
                quote: task.clone(),
                budget: (available0, available1),
                zap_view: zap.as_ref().map(|(v, _, _)| v.clone()),
                plan_id: plan_id.clone(),
                revision,
                allowances: allowances.clone(),
                plan: plan.clone(),
                learning,
                zap_calldata: zap.as_ref().map(|(_, c, v)| (c.clone(), *v)),
                versi: pool.version,
                token0,
                token1,
                amount0,
                amount1,
                harga_buka: pool.raw_price,
                disusun_pada,
            });
        }
        Ok(execute::OpenPlanView {
            plan_id,
            request_id: task.request_id,
            deposit: task.deposit,
            plan,
            allowances,
            zap: zap.as_ref().map(|(v, _, _)| v.clone()),
            setor,
            apr_estimasi,
            gas_biaya,
            gas_gwei,
            version: match pool.version {
                crate::pools::PoolVersion::V3 => "v3",
                crate::pools::PoolVersion::V4 => "v4",
            },
            expires_at,
        })
    }

    /// Ubah jumlah setoran menjadi satuan yang bisa dibaca, lalu proyeksikan
    /// hasilnya untuk ukuran posisi yang benar benar dipakai.
    ///
    /// KENAPA BUKAN MEMAKAI `stats.apr` APA ADANYA
    /// Angka itu memakai modal acuan tetap. Posisi yang jauh lebih besar akan
    /// mengencerkan poolnya sendiri, sehingga APR sebenarnya lebih kecil.
    /// Menampilkan angka modal acuan di sebelah nominal yang berbeda jauh akan
    /// menjanjikan hasil yang tidak akan pernah tercapai.
    async fn ringkas_setoran(
        &self,
        pool: &crate::pools::PoolInfo,
        plan: &crate::exec::pipeline::OpenPlan,
    ) -> (Option<execute::SetoranView>, Option<f64>) {
        let guard = self.active.read().await;
        let Some(active) = guard.as_ref() else {
            return (None, None);
        };
        let meta = &active.meta;
        let Some(kelompok) = active
            .quotes
            .iter()
            .find(|k| k.quote.eq_ignore_ascii_case(&active.selected_quote))
        else {
            return (None, None);
        };

        let setor = Self::ringkas_jumlah_setoran(
            pool,
            meta,
            &kelompok.quote_symbol,
            active.quote_decimals,
            plan.amount0.parse().unwrap_or_default(),
            plan.amount1.parse().unwrap_or_default(),
        );
        let nilai_quote = setor.nilai_quote;

        // APR memakai modal yang benar benar disetor, bukan modal acuan.
        // Penyebutnya memuat modal kita sendiri, jadi angkanya tidak pernah
        // melewati batas yang mungkin dicapai.
        // Volume yang belum terukur berarti APR belum bisa dihitung, bukan nol.
        let apr = pool.stats.as_ref().and_then(|st| {
            let volume = st.volume?;
            let jendela_menit = st.jendela_menit?;
            if pool.dynamic_fee || nilai_quote <= 0.0 || jendela_menit <= 0.0 {
                return None;
            }
            let fee = pool.fee as f64 / 1_000_000.0;
            let bagian = nilai_quote / (st.tvl? + nilai_quote);
            let per_jendela = volume * fee * bagian / nilai_quote;
            let periode = 365.0 * 24.0 * 60.0 / jendela_menit;
            Some(per_jendela * periode * 100.0)
        });

        (Some(setor), apr)
    }

    fn ringkas_jumlah_setoran(
        pool: &crate::pools::PoolInfo,
        meta: &crate::pools::TokenMeta,
        simbol_quote: &str,
        quote_decimals: u8,
        amount0: alloy::primitives::U256,
        amount1: alloy::primitives::U256,
    ) -> execute::SetoranView {
        let quote_adalah_token1 = pool.token0.eq_ignore_ascii_case(&meta.address);
        let skala_meme = 10f64.powi(meta.decimals as i32);
        let skala_quote = 10f64.powi(quote_decimals as i32);
        let a0 = f64::from(amount0);
        let a1 = f64::from(amount1);
        let (mentah_meme, mentah_quote) = if quote_adalah_token1 {
            (a0, a1)
        } else {
            (a1, a0)
        };
        let jumlah_memecoin = mentah_meme / skala_meme;
        let jumlah_quote = mentah_quote / skala_quote;
        let harga = if quote_adalah_token1 {
            pool.raw_price * skala_meme / skala_quote
        } else {
            (1.0 / pool.raw_price) * skala_meme / skala_quote
        };
        execute::SetoranView {
            simbol_memecoin: meta.symbol.clone(),
            jumlah_memecoin,
            simbol_quote: simbol_quote.to_string(),
            jumlah_quote,
            nilai_quote: jumlah_quote + jumlah_memecoin * harga,
        }
    }

    /// Hitung zap kalau modal tidak cocok dengan rasio range.
    ///
    /// Mengembalikan `None` kalau zap tidak diperlukan, yaitu saat posisinya
    /// memang satu sisi, atau saat modal yang menganggur sudah kecil.
    ///
    /// Zap hanya berlaku untuk V4. Jalur V3 memakai router yang berbeda, dan
    /// menebak bentuknya tanpa transaksi sungguhan sebagai acuan adalah persis
    /// kesalahan yang dihindari seluruh proyek ini.
    #[allow(clippy::too_many_arguments)]
    async fn hitung_zap<P: alloy::providers::Provider>(
        &self,
        provider: &P,
        alamat: &crate::exec::plan::AlamatEksekusi,
        pool: &crate::pools::PoolInfo,
        tick_lower: i32,
        tick_upper: i32,
        available0: alloy::primitives::U256,
        available1: alloy::primitives::U256,
        pakai0: alloy::primitives::U256,
        pakai1: alloy::primitives::U256,
    ) -> Option<(
        execute::ZapView,
        alloy::primitives::Bytes,
        alloy::primitives::U256,
    )> {
        use crate::exec::zap::{self, SisiSumber};
        use alloy::primitives::U256;

        if pool.version != crate::pools::PoolVersion::V4 {
            return None;
        }

        // Sisi mana yang menganggur. Kalau keduanya terpakai habis, tidak ada
        // yang perlu ditukar.
        let nganggur0 = available0.saturating_sub(pakai0);
        let nganggur1 = available1.saturating_sub(pakai1);

        // Ambang: modal menganggur di bawah satu persen tidak sepadan dengan
        // biaya gas dan slippage sebuah swap. Menukar demi perbaikan sekecil
        // itu justru merugikan.
        let sepadan = |nganggur: U256, punya: U256| -> bool {
            !punya.is_zero() && nganggur * U256::from(100u8) > punya
        };

        let (sisi, modal) = if sepadan(nganggur0, available0) {
            (SisiSumber::Token0, nganggur0)
        } else if sepadan(nganggur1, available1) {
            (SisiSumber::Token1, nganggur1)
        } else {
            return None;
        };

        let key = crate::exec::v4::pipeline::pool_key_dari(pool).ok()?;
        let sqrt_p: U256 = pool.sqrt_price_x96.parse().ok()?;
        let quoter = self.registry.contract("uniswap_v4", "quoter").ok()?;
        let zero_for_one = sisi == SisiSumber::Token0;

        // Kutipan dikumpulkan lebih dulu, sekali per titik yang dicoba. Ini
        // bagian yang menyentuh jaringan, dan jumlah panggilannya dibatasi oleh
        // penyelesainya sendiri.
        let mut cache: std::collections::BTreeMap<String, U256> = Default::default();
        let mut titik = Vec::new();
        {
            // Kumpulkan titik yang akan ditanyakan dengan menjalankan penyelesai
            // memakai kutipan tiruan lebih dulu. Cara ini menghindari memanggil
            // RPC dari dalam fungsi yang sengaja dibuat murni.
            let _ = zap::selesaikan(sqrt_p, tick_lower, tick_upper, modal, sisi, |x| {
                titik.push(x);
                // Perkiraan kasar hanya untuk memandu titik yang dikunjungi.
                Ok(x)
            });
        }
        titik.truncate(24);
        for x in titik {
            let kunci = x.to_string();
            if cache.contains_key(&kunci) {
                continue;
            }
            match crate::exec::v4::quote::kutip_exact_in(provider, quoter, &key, zero_for_one, x)
                .await
            {
                Ok(v) => {
                    cache.insert(kunci, v);
                }
                Err(e) => {
                    tracing::debug!(error = %e, "kutipan zap gagal, zap dilewati");
                    return None;
                }
            }
        }

        // Penyelesaian sungguhan memakai kutipan yang sudah dikumpulkan. Titik
        // yang belum sempat dikutip dijawab dengan interpolasi linear dari
        // tetangga terdekat, yang cukup untuk memandu pencarian karena hasil
        // akhirnya diverifikasi lagi lewat kutipan langsung di bawah.
        let rencana = zap::selesaikan(sqrt_p, tick_lower, tick_upper, modal, sisi, |x| {
            if let Some(v) = cache.get(&x.to_string()) {
                return Ok(*v);
            }
            let terdekat = cache
                .iter()
                .filter_map(|(k, v)| k.parse::<U256>().ok().map(|kk| (kk, *v)))
                .min_by_key(|(kk, _)| if *kk > x { *kk - x } else { x - *kk });
            match terdekat {
                Some((kk, v)) if !kk.is_zero() => Ok(v * x / kk),
                _ => Err("belum ada kutipan pembanding".to_string()),
            }
        })
        .ok()?;

        if rencana.tukar.is_zero() {
            return None;
        }

        // Kutipan final untuk jumlah yang benar benar akan ditukar. Angka inilah
        // yang dipakai sebagai dasar batas slippage, bukan hasil interpolasi.
        let diterima = crate::exec::v4::quote::kutip_exact_in(
            provider,
            quoter,
            &key,
            zero_for_one,
            rencana.tukar,
        )
        .await
        .ok()?;
        if diterima.is_zero() {
            return None;
        }

        let minimum =
            crate::exec::limits::apply_slippage(diterima, self.cfg.limits.max_slippage_bps);
        let (dari, ke) = if zero_for_one {
            (key.currency0, key.currency1)
        } else {
            (key.currency1, key.currency0)
        };

        let req = crate::exec::router_rbh::SwapExactIn {
            pool: crate::exec::router_rbh::KunciPool::dari_pool_key(&key),
            zero_for_one,
            amount_in: rencana.tukar.try_into().ok()?,
            amount_out_minimum: minimum.try_into().ok()?,
            deadline: alloy::primitives::U256::from(
                chrono::Utc::now().timestamp().max(0) as u64 + DEADLINE_DETIK,
            ),
        };
        let calldata = crate::exec::router_rbh::swap_exact_in(&req).ok()?;
        let value = req.value();
        let _ = alamat;

        Some((
            execute::ZapView {
                dari: dari.to_string(),
                ke: ke.to_string(),
                jumlah: rencana.tukar.to_string(),
                perkiraan_diterima: diterima.to_string(),
                minimum_diterima: minimum.to_string(),
                porsi_bps: if modal.is_zero() {
                    0
                } else {
                    (rencana.tukar * U256::from(10_000u32) / modal).to::<u32>()
                },
                menganggur_tanpa_zap: modal.to_string(),
            },
            calldata,
            value,
        ))
    }

    /// Kirim `approve` untuk satu token sebesar yang dibutuhkan rencana terakhir.
    ///
    /// Jumlahnya diambil dari rencana, bukan dari panel. Panel yang boleh
    /// menentukan jumlah izin adalah panel yang boleh menguras wallet.
    pub async fn on_approve_token(&self, plan_id: String, step_id: String) {
        let _flow = self.flow_lock.lock().await;
        match self.kirim_approve(plan_id.clone(), step_id).await {
            Ok(view) => {
                let ulang = view.outcome.status != crate::exec::send::TxStatus::Menggantung;
                let menggantung = !ulang;
                let _ = self
                    .events
                    .send(protocol::ServerMessage::SendResult(Box::new(view)));
                if ulang {
                    let lanjut_aksi = {
                        let pending = self.pending_action.read().await;
                        pending.as_ref().filter(|r| r.plan_id == plan_id).map(|r| {
                            (
                                r.position_id.clone(),
                                r.action,
                                r.input.clone(),
                                r.request_id.clone(),
                            )
                        })
                    };
                    if let Some((position_id, action, input, request_id)) = lanjut_aksi {
                        let revision = self.intent.begin_action();
                        self.on_plan_action(position_id, action, input, request_id, revision)
                            .await;
                        return;
                    }
                    let lanjut = {
                        let pending = self.pending_plan.read().await;
                        pending
                            .as_ref()
                            .filter(|r| r.plan_id == plan_id)
                            .and_then(|r| {
                                let revision = self.intent.advance(r.revision)?;
                                let mut task = r.quote.clone();
                                task.revision = revision;
                                Some(task)
                            })
                    };
                    if let Some(task) = lanjut {
                        self.on_quote_range(task).await;
                    }
                } else if menggantung {
                    {
                        let mut pending = self.pending_action.write().await;
                        if pending.as_ref().is_some_and(|r| r.plan_id == plan_id) {
                            pending.take();
                            self.intent.begin_action();
                        }
                    }
                    {
                        let mut pending = self.pending_plan.write().await;
                        if pending.as_ref().is_some_and(|r| r.plan_id == plan_id) {
                            pending.take();
                            self.intent.change();
                        }
                    }
                    self.alert_kritis(
                        "approve_unknown",
                        "status approval belum diketahui. Periksa jurnal transaksi sebelum menyusun atau mengirim ulang"
                            .into(),
                    );
                }
            }
            Err(pesan) => self.alert_kritis("approve_failed", pesan),
        }
    }

    /// Jalankan approval, zap, dan mint dari satu klik.
    pub async fn on_run_open_flow(&self, plan_id: String) {
        let subject = {
            let guard = self.pending_plan.read().await;
            guard
                .as_ref()
                .filter(|r| r.plan_id == plan_id)
                .map(|r| r.quote.request_id.clone())
                .or_else(|| Some(plan_id.clone()))
        };
        self.beri_tahu_subjek(
            protocol::AlertLevel::Info,
            "open_started",
            "Pembukaan posisi dimulai".into(),
            subject.clone(),
        )
        .await;
        match self.run_open_flow(plan_id).await {
            Ok((status, terverifikasi)) => {
                let (level, code, message) = match (status, terverifikasi) {
                    (crate::exec::send::TxStatus::Sukses, true) => (
                        protocol::AlertLevel::Info,
                        "open_succeeded",
                        "Posisi berhasil dibuka".to_string(),
                    ),
                    (crate::exec::send::TxStatus::Sukses, false) => (
                        protocol::AlertLevel::Warning,
                        "open_unverified",
                        "Transaksi masuk, tetapi posisi belum terverifikasi. Muat ulang Positions sebelum mencoba lagi".to_string(),
                    ),
                    (crate::exec::send::TxStatus::Menggantung, _) => (
                        protocol::AlertLevel::Warning,
                        "open_pending",
                        "Pembukaan masih diproses. Hasil akhirnya akan diberitahukan".to_string(),
                    ),
                    (crate::exec::send::TxStatus::Revert, _) => (
                        protocol::AlertLevel::Critical,
                        "open_failed",
                        "Posisi gagal dibuka karena transaksi ditolak rantai".to_string(),
                    ),
                };
                self.beri_tahu_subjek(level, code, message, subject).await;
            }
            Err(pesan) => {
                self.alert_kritis("open_flow_failed", pesan.clone());
                self.beri_tahu_subjek(
                    protocol::AlertLevel::Critical,
                    "open_failed",
                    format!("Posisi gagal dibuka: {pesan}"),
                    subject,
                )
                .await;
            }
        }
    }

    async fn run_open_flow(
        &self,
        mut plan_id: String,
    ) -> Result<(crate::exec::send::TxStatus, bool), String> {
        let _flow = self.flow_lock.lock().await;
        let mut izin_selesai = false;
        for _ in 0..MAX_FLOW_STEPS {
            let (step_id, perlu_zap, siap) = {
                let guard = self.pending_plan.read().await;
                let r = guard
                    .as_ref()
                    .ok_or("rencana pembukaan sudah tidak tersedia")?;
                r.periksa(&plan_id, &self.intent)?;
                (
                    r.allowances.first().map(|step| step.step_id.clone()),
                    r.zap_calldata.is_some(),
                    r.allowances.is_empty() && r.zap_calldata.is_none() && r.plan.is_sendable(),
                )
            };

            if let Some(step_id) = step_id {
                let view = self.kirim_approve(plan_id.clone(), step_id.clone()).await?;
                let status = view.outcome.status;
                let _ = self
                    .events
                    .send(protocol::ServerMessage::SendResult(Box::new(view)));
                if status != crate::exec::send::TxStatus::Sukses {
                    if status == crate::exec::send::TxStatus::Menggantung {
                        self.batalkan_flow_tidak_pasti(&plan_id).await;
                    }
                    return Ok((status, false));
                }
                self.tandai_izin_selesai(&plan_id, &step_id).await?;
                izin_selesai = true;
                continue;
            }

            // Approval mengubah state rantai, tetapi seluruh allowance dalam
            // rencana awal sudah terikat urutan dan diverifikasi receipt-nya.
            // Susun ulang sekali setelah semuanya selesai, bukan sekali per
            // approval. Gate akhir tetap memakai state rantai terbaru.
            if izin_selesai {
                let task = {
                    let guard = self.pending_plan.read().await;
                    let r = guard
                        .as_ref()
                        .filter(|r| r.plan_id == plan_id)
                        .ok_or("rencana pembukaan berubah setelah izin token")?;
                    let revision = self
                        .intent
                        .advance(r.revision)
                        .ok_or("rencana pembukaan berubah saat izin diproses")?;
                    let mut task = r.quote.clone();
                    task.revision = revision;
                    task
                };
                let view = self.susun_rencana(task).await?;
                plan_id = view.plan_id.clone();
                let _ = self
                    .events
                    .send(protocol::ServerMessage::OpenPlan(Box::new(view)));
                izin_selesai = false;
                continue;
            }

            if perlu_zap {
                let (view, lanjut) = self.kirim_zap(plan_id.clone()).await?;
                let status = view.outcome.status;
                let _ = self
                    .events
                    .send(protocol::ServerMessage::SendResult(Box::new(view)));
                if status != crate::exec::send::TxStatus::Sukses {
                    if status == crate::exec::send::TxStatus::Menggantung {
                        self.batalkan_flow_tidak_pasti(&plan_id).await;
                    }
                    return Ok((status, false));
                }
                let task = lanjut.ok_or("saldo hasil penyesuaian belum bisa dipakai")?;
                let view = self.susun_rencana(task).await?;
                plan_id = view.plan_id.clone();
                let _ = self
                    .events
                    .send(protocol::ServerMessage::OpenPlan(Box::new(view)));
                continue;
            }

            if !siap {
                return Err("Posisi belum siap dibuka. Periksa jumlah setoran dan saldo".into());
            }

            let view = self.kirim_open(plan_id).await?;
            let status = view.outcome.status;
            let terverifikasi = view.outcome.fully_verified();
            let _ = self
                .events
                .send(protocol::ServerMessage::SendResult(Box::new(view)));
            return Ok((status, terverifikasi));
        }
        Err("Alur pembukaan berhenti karena terlalu banyak langkah".into())
    }

    /// Hapus approval pertama sesudah receipt-nya sukses.
    async fn tandai_izin_selesai(&self, plan_id: &str, step_id: &str) -> Result<(), String> {
        {
            let mut guard = self.pending_plan.write().await;
            if let Some(rencana) = guard.as_mut().filter(|r| r.plan_id == plan_id) {
                let pertama = rencana
                    .allowances
                    .first()
                    .ok_or("izin pembukaan sudah selesai")?;
                if pertama.step_id != step_id {
                    return Err("urutan izin pembukaan berubah".into());
                }
                rencana.allowances.remove(0);
                return Ok(());
            }
        }
        let mut guard = self.pending_action.write().await;
        let rencana = guard
            .as_mut()
            .filter(|r| r.plan_id == plan_id)
            .ok_or("rencana untuk izin sudah tidak tersedia")?;
        let pertama = rencana
            .allowances
            .first()
            .ok_or("izin aksi sudah selesai")?;
        if pertama.step_id != step_id {
            return Err("urutan izin aksi berubah".into());
        }
        rencana.allowances.remove(0);
        Ok(())
    }

    async fn batalkan_flow_tidak_pasti(&self, plan_id: &str) {
        {
            let mut pending = self.pending_plan.write().await;
            if pending.as_ref().is_some_and(|r| r.plan_id == plan_id) {
                pending.take();
                self.intent.change();
            }
        }
        {
            let mut pending = self.pending_action.write().await;
            if pending.as_ref().is_some_and(|r| r.plan_id == plan_id) {
                pending.take();
                self.intent.begin_action();
            }
        }
    }

    async fn kirim_approve(
        &self,
        plan_id: String,
        step_id: String,
    ) -> Result<execute::SendResultView, String> {
        let _eksekusi = self.execution_lock.lock().await;
        self.ensure_execution().await?;
        let (langkah, context) = {
            let open_matches = self
                .pending_plan
                .read()
                .await
                .as_ref()
                .is_some_and(|r| r.plan_id == plan_id);
            if open_matches {
                let guard = self.pending_plan.read().await;
                let r = guard.as_ref().ok_or("rencana open hilang")?;
                r.periksa(&plan_id, &self.intent)?;
                self.periksa_hook_rencana(r).await?;
                r.periksa(&plan_id, &self.intent)?;
                let langkah = r.allowances.first().ok_or("izin rencana ini sudah cukup")?;
                if langkah.step_id != step_id {
                    return Err("kerjakan langkah izin yang pertama terlebih dulu".into());
                }
                (
                    r.allowances
                        .first()
                        .cloned()
                        .expect("langkah sudah diperiksa"),
                    r.journal_context(crate::exec::journal::Operation::Approve),
                )
            } else {
                let guard = self.pending_action.read().await;
                let r = guard
                    .as_ref()
                    .ok_or("belum ada rencana open atau aksi yang disusun")?;
                r.periksa(&plan_id, &self.intent)?;
                self.periksa_hook_aksi(r).await?;
                r.periksa(&plan_id, &self.intent)?;
                let langkah = r
                    .allowances
                    .first()
                    .ok_or("izin rencana aksi ini sudah cukup")?;
                if langkah.step_id != step_id {
                    return Err("kerjakan langkah izin yang pertama terlebih dulu".into());
                }
                (
                    r.allowances
                        .first()
                        .cloned()
                        .expect("langkah sudah diperiksa"),
                    r.journal_context.clone(),
                )
            }
        };
        let mut context = context;
        context.operation = crate::exec::journal::Operation::Approve;
        let target: alloy::primitives::Address = langkah
            .token
            .parse()
            .map_err(|_| "token izin tidak valid")?;
        let spender: alloy::primitives::Address = langkah
            .spender
            .parse()
            .map_err(|_| "spender izin tidak valid")?;
        let jumlah: alloy::primitives::U256 = langkah
            .needed
            .parse()
            .map_err(|_| "jumlah izin tidak valid")?;
        if jumlah.is_zero() {
            return Err("jumlah izin nol".into());
        }
        let provider = self.chain.provider().await.map_err(|e| e.to_string())?;
        let alamat = crate::exec::plan::AlamatEksekusi::dari_registry(&self.registry)
            .map_err(|e| e.to_string())?;
        let signer = self
            .wallet
            .lock()
            .await
            .signer()
            .map_err(|e| e.to_string())?
            .clone();
        let journal = crate::exec::journal::Journal {
            store: &self.store,
            context,
        };
        let outcome = match langkah.jenis {
            "erc20_ke_permit2" | "erc20_ke_position_manager" | "erc20_ke_router" => {
                crate::exec::send::send_approve(
                    &provider,
                    &self.nonce,
                    &signer,
                    alamat.multicall,
                    target,
                    spender,
                    jumlah,
                    self.registry.chain.id,
                    &self.cfg.execution,
                    &self.cfg.limits,
                    &journal,
                )
                .await
                .map_err(|e| e.to_string())?
            }
            "permit2_ke_position_manager" => {
                let sekarang = chrono::Utc::now().timestamp().max(0) as u64;
                let calldata = crate::exec::v4::permit::permit2_approve_calldata(
                    target,
                    spender,
                    jumlah,
                    sekarang + crate::exec::v4::permit::MASA_BERLAKU_DETIK,
                )?;
                let terkirim = crate::exec::send::send_calldata(
                    &provider,
                    &self.nonce,
                    &signer,
                    alamat.permit2,
                    calldata,
                    alloy::primitives::U256::ZERO,
                    self.registry.chain.id,
                    &self.cfg.execution,
                    &self.cfg.limits,
                    &journal,
                )
                .await
                .map_err(|e| e.to_string())?;
                crate::exec::send::jadikan_hasil(terkirim)
            }
            _ => return Err("jenis izin tidak dikenal".into()),
        };
        Ok(execute::SendResultView {
            kind: "approve",
            outcome,
        })
    }
    /// Catat sebuah kejadian, lalu siarkan kalau ia tidak tertahan pendinginan.
    ///
    /// Semua kejadian masuk riwayat, termasuk yang tertahan. Yang tertahan tidak
    /// disiarkan, karena toast yang berulang setiap detik membuat user
    /// mematikan notifikasi seluruhnya, dan itu hasil yang lebih buruk daripada
    /// tidak punya notifikasi sama sekali.
    pub async fn beri_tahu_subjek(
        &self,
        level: protocol::AlertLevel,
        code: &str,
        message: String,
        subject: Option<String>,
    ) {
        let tingkat = match level {
            protocol::AlertLevel::Info => crate::notify::Tingkat::Info,
            protocol::AlertLevel::Warning => crate::notify::Tingkat::Warning,
            protocol::AlertLevel::Critical => crate::notify::Tingkat::Critical,
        };

        let kejadian = {
            let mut n = self.notifier.write().await;
            n.catat(
                tingkat,
                code,
                message,
                subject,
                chrono::Utc::now().timestamp(),
            )
        };

        if kejadian.ditahan {
            tracing::debug!(code, "kejadian tertahan pendinginan, hanya masuk riwayat");
            return;
        }

        let _ = self
            .events
            .send(protocol::ServerMessage::Notify(Box::new(kejadian)));
    }

    /// Versi ringkas untuk kejadian yang tidak terikat posisi tertentu.
    pub fn beri_tahu(&self, level: protocol::AlertLevel, code: &str, message: String) {
        let state = self.clone();
        let code = code.to_string();
        tokio::spawn(async move {
            state.beri_tahu_subjek(level, &code, message, None).await;
        });
    }

    /// Kirim riwayat notifikasi ke panel.
    pub async fn on_get_notifications(&self) {
        let riwayat = self.notifier.read().await.riwayat();
        let _ = self
            .events
            .send(protocol::ServerMessage::NotifyHistory { items: riwayat });
    }

    /// Terima deret bar dari chart gmgn, lalu hitung sinyalnya.
    ///
    /// KENAPA BAR DATANG DARI CHART
    /// Angka yang dipakai untuk keputusan harus sama dengan yang dilihat user di
    /// layar. Sumber lain akan selalu sedikit berbeda, dan selisih itu muncul
    /// persis saat harga bergerak cepat, yaitu saat keputusannya paling penting.
    pub async fn on_chart_bars(&self, address: String, bars: Vec<crate::signals::bar::Bar>) {
        if !self
            .active
            .read()
            .await
            .as_ref()
            .is_some_and(|a| a.meta.address.eq_ignore_ascii_case(&address))
        {
            return;
        }
        let bersih = crate::signals::bar::bersihkan(bars);
        if bersih.is_empty() {
            return;
        }

        let jumlah = bersih.len();
        *self.bars.write().await = Some((address.clone(), bersih.clone()));

        let sideways =
            crate::signals::sideways::ukur(&bersih, &crate::learning::aturan_sideways(&self.store));
        let basis = crate::signals::dump_basis::cari(
            &bersih,
            &crate::signals::dump_basis::Aturan::default(),
        );

        let harga = bersih.last().map(|b| b.close).unwrap_or(0.0);

        // Saran penempatan diambil dari detektor yang paling bisa dipercaya.
        //
        // Basis menang atas kanal kalau ia lolos, karena jangkarnya terbentuk
        // dari perilaku sungguhan: penjual habis di satu harga, dan pembeli
        // mempertahankannya. Kanal hanya ringkasan statistik.
        let saran = match (&basis, &sideways) {
            (Some(b), _) if b.lolos() => crate::signals::penempatan::dari_basis(
                harga,
                b.jangkar.l_basis,
                b.jangkar.h_basis,
                b.jangkar.poc,
                b.jangkar.p_invalid,
            ),
            (_, Some(s)) if s.lolos() => {
                crate::signals::penempatan::dari_kanal(harga, s.kanal_bawah, s.kanal_atas)
            }
            _ => None,
        };

        let dibanding = saran
            .as_ref()
            .and_then(|s| crate::signals::penempatan::dibanding_lebar(harga, s, 5.0));

        let view = protocol::SignalsView {
            address,
            bar_terpakai: jumlah,
            harga,
            sideways,
            basis,
            saran,
            konsentrasi_dibanding_lebar: dibanding,
            dihitung_pada: chrono::Utc::now().to_rfc3339(),
        };

        let _ = self
            .events
            .send(protocol::ServerMessage::Signals(Box::new(view)));
    }

    /// Susun rencana sebuah aksi pada posisi, jalankan simulasinya, TIDAK kirim.
    ///
    /// PRD 3.5 menuntut simulasi hasil ditampilkan sebelum tombol konfirmasi
    /// aktif. Karena itu polanya sama dengan membuka posisi: susun, lihat, lalu
    /// setujui, dan calldata-nya tidak pernah bolak balik lewat panel.
    pub async fn on_plan_action(
        &self,
        position_id: String,
        aksi: crate::exec::aksi::JenisAksi,
        input: crate::exec::aksi::ActionInput,
        request_id: String,
        revision: u64,
    ) {
        match self
            .susun_rencana_aksi(position_id, aksi, input, request_id, revision)
            .await
        {
            Ok(view) => {
                if !self.intent.action_current(revision) {
                    return;
                }
                let _ = self
                    .events
                    .send(protocol::ServerMessage::ActionPlan(Box::new(view)));
            }
            Err(pesan) => self.alert_kritis("action_plan_failed", pesan),
        }
    }

    async fn susun_rencana_aksi(
        &self,
        position_id: String,
        aksi: crate::exec::aksi::JenisAksi,
        input: crate::exec::aksi::ActionInput,
        request_id: String,
        revision: u64,
    ) -> Result<execute::ActionPlanView, String> {
        let pemilik = {
            let w = self.wallet.lock().await;
            match (w.exists(), w.is_locked(), w.address()) {
                (false, _, _) => return Err("wallet belum dibuat".into()),
                (_, true, _) => return Err("wallet masih terkunci".into()),
                (_, _, Some(a)) => a,
                _ => return Err("alamat wallet tidak diketahui".into()),
            }
        };

        // Fee, likuiditas, dan kepemilikan harus segar sebelum quote aksi.
        let provider = self.chain.provider().await.map_err(|e| e.to_string())?;
        let posisi = {
            let daftar = crate::positions::muat::muat_semua(
                &provider,
                &self.registry,
                Some(&self.store),
                pemilik,
            )
            .await;
            daftar
                .iter()
                .find(|p| p.position_id == position_id)
                .cloned()
                .ok_or("posisi tidak ada di daftar. Muat ulang tab Positions dulu")?
        };
        let token_id = posisi.token_id.clone();
        let token_id_u256: alloy::primitives::U256 = token_id
            .parse()
            .map_err(|_| "token id posisi bukan angka")?;
        let token0: alloy::primitives::Address = posisi
            .token0
            .parse()
            .map_err(|_| "alamat token0 posisi tidak valid")?;
        let token1: alloy::primitives::Address = posisi
            .token1
            .parse()
            .map_err(|_| "alamat token1 posisi tidak valid")?;

        let deadline = alloy::primitives::U256::from(
            chrono::Utc::now().timestamp().max(0) as u64 + DEADLINE_DETIK,
        );
        let alamat = crate::exec::plan::AlamatEksekusi::dari_registry(&self.registry)
            .map_err(|e| e.to_string())?;
        let tujuan = alamat.position_manager(posisi.version);
        let memasukkan_modal = matches!(
            aksi,
            crate::exec::aksi::JenisAksi::TambahLikuiditas
                | crate::exec::aksi::JenisAksi::Rebalance
        );
        let hook = if memasukkan_modal && posisi.version == crate::pools::PoolVersion::V4 {
            posisi
                .hooks
                .as_deref()
                .filter(|value| *value != crate::pools::ZERO_ADDRESS)
                .map(|value| value.parse::<alloy::primitives::Address>())
                .transpose()
                .map_err(|_| "alamat hook posisi tidak valid")?
        } else {
            None
        };
        if memasukkan_modal {
            crate::risk::policy::periksa(&provider, &self.store, hook)
                .await
                .map_err(|e| format!("kebijakan hook: {e}"))?;
        }

        let metadata = if memasukkan_modal {
            let mut values =
                crate::pools::token_meta_batch(&provider, &self.registry, &[token0, token1])
                    .await
                    .map_err(|e| format!("metadata token posisi tidak terbaca: {e}"))?;
            if values.len() != 2 {
                return Err("metadata kedua token posisi tidak lengkap".into());
            }
            let meta1 = values
                .pop()
                .flatten()
                .ok_or("metadata token1 tidak terbaca")?;
            let meta0 = values
                .pop()
                .flatten()
                .ok_or("metadata token0 tidak terbaca")?;
            Some((meta0, meta1))
        } else {
            None
        };

        let mut target_ticks = None;
        let mut planned_amounts = None;
        let mut planned_raw = None;
        let mut planned_liquidity = None;
        let mut allowances = Vec::new();
        let (rencana, value, harapan) = match aksi {
            crate::exec::aksi::JenisAksi::KlaimFee | crate::exec::aksi::JenisAksi::TutupPosisi => {
                let rencana = crate::exec::aksi::susun(&posisi, aksi, pemilik, deadline)
                    .map_err(|e| e.to_string())?;
                (
                    rencana,
                    alloy::primitives::U256::ZERO,
                    execute::HarapanAksi::TidakAda,
                )
            }
            crate::exec::aksi::JenisAksi::TambahLikuiditas => {
                let (meta0, meta1) = metadata.as_ref().expect("metadata aksi modal");
                let entered0 = jumlah_aksi(input.amount0.as_deref(), meta0.decimals)?;
                let entered1 = jumlah_aksi(input.amount1.as_deref(), meta1.decimals)?;
                if entered0.is_zero() && entered1.is_zero() {
                    return Err("isi jumlah token0 atau token1 yang akan ditambahkan".into());
                }
                let pool = pool_dari_posisi(&posisi)?;
                // Kalau user mengisi satu sisi, turunkan sisi pasangannya dari
                // rasio range yang sama. Ini memakai matematika integer engine,
                // jadi angka yang muncul di panel sama dengan calon calldata.
                let (available0, available1) = match (entered0.is_zero(), entered1.is_zero()) {
                    (false, true) => deposit::budget(
                        &pool,
                        posisi.tick_lower,
                        posisi.tick_upper,
                        entered0,
                        true,
                        false,
                    )?,
                    (true, false) => deposit::budget(
                        &pool,
                        posisi.tick_lower,
                        posisi.tick_upper,
                        entered1,
                        false,
                        false,
                    )?,
                    _ => (entered0, entered1),
                };
                let (saldo0, saldo1) =
                    execute::saldo_dua_token(&provider, alamat.multicall, pemilik, token0, token1)
                        .await?;
                if available0 > saldo0 || available1 > saldo1 {
                    return Err("saldo tidak cukup untuk jumlah tambah likuiditas".into());
                }
                let (liquidity, butuh0, butuh1) = crate::exec::pipeline::hitung_jumlah(
                    &pool,
                    posisi.tick_lower,
                    posisi.tick_upper,
                    available0,
                    available1,
                )?;
                planned_liquidity = Some(liquidity);
                let (calldata, value, izin0, izin1) = match posisi.version {
                    crate::pools::PoolVersion::V3 => (
                        crate::exec::calldata::increase_liquidity(
                            token_id_u256,
                            butuh0,
                            butuh1,
                            crate::exec::limits::apply_slippage(
                                butuh0,
                                self.cfg.limits.max_slippage_bps,
                            ),
                            crate::exec::limits::apply_slippage(
                                butuh1,
                                self.cfg.limits.max_slippage_bps,
                            ),
                            deadline,
                        )
                        .map_err(|e| e.to_string())?,
                        alloy::primitives::U256::ZERO,
                        butuh0,
                        butuh1,
                    ),
                    crate::pools::PoolVersion::V4 => {
                        let max0 =
                            batas_atas_aksi(butuh0, self.cfg.limits.max_slippage_bps, available0);
                        let max1 =
                            batas_atas_aksi(butuh1, self.cfg.limits.max_slippage_bps, available1);
                        let data = crate::exec::v4::calldata::increase_liquidity(
                            token_id_u256,
                            liquidity,
                            ke_u128_aksi(max0, "batas token0")?,
                            ke_u128_aksi(max1, "batas token1")?,
                            token0,
                            token1,
                            deadline,
                        )
                        .map_err(|e| e.to_string())?;
                        let value = if token0.is_zero() {
                            max0
                        } else if token1.is_zero() {
                            max1
                        } else {
                            alloy::primitives::U256::ZERO
                        };
                        (data, value, max0, max1)
                    }
                };
                allowances = crate::exec::plan::izin_yang_kurang(
                    &provider,
                    &alamat,
                    posisi.version,
                    pemilik,
                    token0,
                    izin0,
                    token1,
                    izin1,
                    chrono::Utc::now().timestamp().max(0) as u64,
                )
                .await?;
                planned_raw = Some((izin0, izin1));
                planned_amounts = Some(execute::ActionAmountView {
                    symbol0: meta0.symbol.clone(),
                    amount0: deposit::display_units(izin0, meta0.decimals),
                    symbol1: meta1.symbol.clone(),
                    amount1: deposit::display_units(izin1, meta1.decimals),
                });
                (
                    crate::exec::aksi::Rencana {
                        calldata,
                        versi: posisi.version,
                        menutup: false,
                    },
                    value,
                    execute::HarapanAksi::Tambah {
                        sebelum: Box::new(posisi.clone()),
                        minimum_tambahan: liquidity.saturating_sub(liquidity / 100),
                    },
                )
            }
            crate::exec::aksi::JenisAksi::Rebalance => {
                let (meta0, meta1) = metadata.as_ref().expect("metadata aksi modal");
                let principal0: alloy::primitives::U256 = posisi
                    .amount0
                    .parse()
                    .map_err(|_| "jumlah token0 posisi tidak valid")?;
                let principal1: alloy::primitives::U256 = posisi
                    .amount1
                    .parse()
                    .map_err(|_| "jumlah token1 posisi tidak valid")?;
                let fee0: alloy::primitives::U256 = posisi
                    .fee0
                    .as_deref()
                    .ok_or("fee token0 belum terbaca, rebalance tidak boleh menebak")?
                    .parse()
                    .map_err(|_| "fee token0 posisi tidak valid")?;
                let fee1: alloy::primitives::U256 = posisi
                    .fee1
                    .as_deref()
                    .ok_or("fee token1 belum terbaca, rebalance tidak boleh menebak")?
                    .parse()
                    .map_err(|_| "fee token1 posisi tidak valid")?;
                let total0 = principal0.checked_add(fee0).ok_or("jumlah token0 meluap")?;
                let total1 = principal1.checked_add(fee1).ok_or("jumlah token1 meluap")?;
                let (tick_lower, tick_upper) =
                    crate::exec::aksi::target_rebalance(&posisi, total0, total1)
                        .map_err(|e| e.to_string())?;
                target_ticks = Some((tick_lower, tick_upper));
                let guaranteed0 = crate::exec::limits::apply_slippage(
                    principal0,
                    self.cfg.limits.max_slippage_bps,
                )
                .checked_add(fee0)
                .ok_or("jaminan token0 meluap")?;
                let guaranteed1 = crate::exec::limits::apply_slippage(
                    principal1,
                    self.cfg.limits.max_slippage_bps,
                )
                .checked_add(fee1)
                .ok_or("jaminan token1 meluap")?;
                let pool = pool_dari_posisi(&posisi)?;
                let (liquidity, butuh0, butuh1) = crate::exec::pipeline::hitung_jumlah(
                    &pool,
                    tick_lower,
                    tick_upper,
                    guaranteed0,
                    guaranteed1,
                )?;
                planned_liquidity = Some(liquidity);
                let old_liquidity: u128 = posisi
                    .liquidity
                    .parse()
                    .map_err(|_| "likuiditas posisi tidak valid")?;
                if old_liquidity == 0 {
                    return Err("posisi kosong tidak dapat direbalance".into());
                }
                let min_old0 = crate::exec::limits::apply_slippage(
                    principal0,
                    self.cfg.limits.max_slippage_bps,
                );
                let min_old1 = crate::exec::limits::apply_slippage(
                    principal1,
                    self.cfg.limits.max_slippage_bps,
                );
                let (calldata, value, harapan) = match posisi.version {
                    crate::pools::PoolVersion::V3 => {
                        let request = crate::exec::calldata::MintRequest {
                            token0,
                            token1,
                            fee: posisi.fee,
                            tick_lower,
                            tick_upper,
                            amount0_desired: butuh0,
                            amount1_desired: butuh1,
                            amount0_min: crate::exec::limits::apply_slippage(
                                butuh0,
                                self.cfg.limits.max_slippage_bps,
                            ),
                            amount1_min: crate::exec::limits::apply_slippage(
                                butuh1,
                                self.cfg.limits.max_slippage_bps,
                            ),
                            recipient: pemilik,
                            deadline,
                        };
                        let data = crate::exec::calldata::rebalance_position(
                            token_id_u256,
                            old_liquidity,
                            min_old0,
                            min_old1,
                            &request,
                            posisi.tick_spacing,
                        )
                        .map_err(|e| e.to_string())?;
                        (
                            data,
                            alloy::primitives::U256::ZERO,
                            execute::HarapanAksi::RebalanceV3 {
                                token_lama: token_id_u256,
                                permintaan: request,
                                expected_liquidity: liquidity,
                            },
                        )
                    }
                    crate::pools::PoolVersion::V4 => {
                        let max0 =
                            batas_atas_aksi(butuh0, self.cfg.limits.max_slippage_bps, guaranteed0);
                        let max1 =
                            batas_atas_aksi(butuh1, self.cfg.limits.max_slippage_bps, guaranteed1);
                        let request = crate::exec::v4::calldata::MintV4Request {
                            pool_key: crate::exec::v4::pipeline::pool_key_dari(&pool)?,
                            tick_lower,
                            tick_upper,
                            liquidity,
                            amount0_max: ke_u128_aksi(max0, "batas token0")?,
                            amount1_max: ke_u128_aksi(max1, "batas token1")?,
                            owner: pemilik,
                            deadline,
                        };
                        let data = crate::exec::v4::calldata::rebalance_position(
                            token_id_u256,
                            ke_u128_aksi(min_old0, "minimum penarikan token0")?,
                            ke_u128_aksi(min_old1, "minimum penarikan token1")?,
                            &request,
                        )
                        .map_err(|e| e.to_string())?;
                        let value = request.value();
                        (
                            data,
                            value,
                            execute::HarapanAksi::RebalanceV4 {
                                token_lama: token_id_u256,
                                permintaan: request,
                            },
                        )
                    }
                };
                allowances = crate::exec::plan::izin_yang_kurang(
                    &provider,
                    &alamat,
                    posisi.version,
                    pemilik,
                    token0,
                    butuh0,
                    token1,
                    butuh1,
                    chrono::Utc::now().timestamp().max(0) as u64,
                )
                .await?;
                planned_raw = Some((butuh0, butuh1));
                planned_amounts = Some(execute::ActionAmountView {
                    symbol0: meta0.symbol.clone(),
                    amount0: deposit::display_units(butuh0, meta0.decimals),
                    symbol1: meta1.symbol.clone(),
                    amount1: deposit::display_units(butuh1, meta1.decimals),
                });
                (
                    crate::exec::aksi::Rencana {
                        calldata,
                        versi: posisi.version,
                        menutup: false,
                    },
                    value,
                    harapan,
                )
            }
        };

        let mut izin_risiko_kurang = false;
        let uji_jual = if aksi == crate::exec::aksi::JenisAksi::TambahLikuiditas {
            let (quote_token0, _, _) = crate::positions::muat::kutipan_pool(
                &self.registry,
                &posisi.token0,
                &posisi.token1,
            )
            .ok_or("pasangan posisi tidak mempunyai token kutipan yang dikenal")?;
            let memecoin = if quote_token0 { token1 } else { token0 };
            let decimals = metadata
                .as_ref()
                .map(|(meta0, meta1)| {
                    if quote_token0 {
                        meta1.decimals
                    } else {
                        meta0.decimals
                    }
                })
                .ok_or("metadata memecoin untuk uji jual tidak tersedia")?;
            let (saldo0, saldo1) =
                execute::saldo_dua_token(&provider, alamat.multicall, pemilik, token0, token1)
                    .await?;
            let saldo = if quote_token0 { saldo1 } else { saldo0 };
            let satu_token = deposit::units("1", decimals)?;
            let jumlah = satu_token
                .max(saldo / alloy::primitives::U256::from(10_000u32))
                .min(saldo);
            if jumlah.is_zero() {
                None
            } else {
                let pool = pool_dari_posisi(&posisi)?;
                let router = match posisi.version {
                    crate::pools::PoolVersion::V3 => self
                        .registry
                        .contract("uniswap_v3", "swap_router_02")
                        .map_err(|e| e.to_string())?,
                    crate::pools::PoolVersion::V4 => self
                        .registry
                        .contract("uniswap_v4", "universal_router")
                        .map_err(|e| e.to_string())?,
                };
                let izin = match posisi.version {
                    crate::pools::PoolVersion::V3 => {
                        crate::exec::plan::izin_erc20_untuk(
                            &provider,
                            &alamat,
                            pemilik,
                            router,
                            "SwapRouter02",
                            memecoin,
                            jumlah,
                        )
                        .await?
                    }
                    crate::pools::PoolVersion::V4 => {
                        crate::exec::plan::izin_permit2_untuk(
                            &provider,
                            &alamat,
                            pemilik,
                            router,
                            "UniversalRouter",
                            memecoin,
                            jumlah,
                            chrono::Utc::now().timestamp().max(0) as u64,
                        )
                        .await?
                    }
                };
                izin_risiko_kurang = !izin.is_empty();
                for langkah in izin {
                    if !allowances.iter().any(|ada| {
                        ada.jenis == langkah.jenis
                            && ada.token.eq_ignore_ascii_case(&langkah.token)
                            && ada.spender.eq_ignore_ascii_case(&langkah.spender)
                    }) {
                        allowances.push(langkah);
                    }
                }
                Some((pool, memecoin, jumlah))
            }
        } else {
            None
        };

        for (index, step) in allowances.iter_mut().enumerate() {
            step.step_id = format!("aksi-{index}");
        }

        let nominal_dibatasi = self.cfg.limits.max_position_usd.is_some()
            || self.cfg.limits.max_daily_deploy_usd.is_some();
        let deployment_usd = if aksi == crate::exec::aksi::JenisAksi::TambahLikuiditas {
            if nominal_dibatasi {
                let (meta0, meta1) = metadata.as_ref().expect("metadata aksi modal");
                let active = self.active.read().await;
                Some(
                    active
                        .as_ref()
                        .and_then(|active| {
                            nilai_usd_aksi(
                                &posisi,
                                meta0,
                                meta1,
                                planned_raw.unwrap_or_default().0,
                                planned_raw.unwrap_or_default().1,
                                active,
                            )
                        })
                        .ok_or(
                            "batas nominal aktif tetapi nilai USD tambah likuiditas belum dapat dibuktikan dari chart aktif",
                        )?,
                )
            } else {
                Some(0.0)
            }
        } else {
            None
        };

        let mut gates = Vec::new();

        // Kepemilikan diperiksa terhadap rantai, bukan terhadap daftar kita.
        // Mengirim aksi untuk posisi milik orang lain akan revert setelah gas
        // terbakar, dan pesannya tidak menjelaskan apa apa.
        gates.push(crate::exec::pipeline::GateResult::lolos_publik(
            "G0",
            "versi protokol",
            format!("posisi {:?}, kontrak tujuan sesuai", rencana.versi),
        ));

        if memasukkan_modal {
            gates.push(crate::exec::pipeline::GateResult::lolos_publik(
                "G2",
                "builder calldata",
                format!(
                    "{} byte, jumlah dan tick tervalidasi",
                    rencana.calldata.len()
                ),
            ));
            match crate::exec::golden::verify_embedded() {
                Ok(()) => gates.push(crate::exec::pipeline::GateResult::lolos_publik(
                    "G3",
                    "golden vector",
                    "encoder dasar cocok dengan transaksi acuan",
                )),
                Err(error) => gates.push(crate::exec::pipeline::GateResult::gagal_publik(
                    "G3",
                    "golden vector",
                    error,
                )),
            }
        }

        let tx = alloy::rpc::types::TransactionRequest::default()
            .from(pemilik)
            .to(tujuan)
            .value(value)
            .input(rencana.calldata.clone().into());

        let gas = provider.estimate_gas(tx.clone()).await.unwrap_or(0);
        if memasukkan_modal {
            let usage = if self.cfg.limits.max_daily_deploy_usd.is_some() {
                crate::exec::journal::limit_usage(
                    &self.store,
                    self.registry.chain.id,
                    pemilik,
                    chrono::Utc::now(),
                )
                .map_err(|error| format!("penggunaan limit tidak terbaca: {error}"))?
            } else {
                crate::exec::journal::LimitUsage::default()
            };
            let balance = provider
                .get_balance(pemilik)
                .await
                .map_err(|error| format!("saldo ETH untuk G6 tidak terbaca: {error}"))?;
            let gas_price = provider
                .get_gas_price()
                .await
                .map_err(|error| format!("harga gas untuk G6 tidak terbaca: {error}"))?;
            let eth_needed = (f64::from(value) + gas as f64 * gas_price as f64) / 1e18;
            let mut config = self.cfg.limits.clone();
            // Kedua aksi ini tidak menambah jumlah posisi bersih dan tidak
            // memperkenalkan pool baru.
            config.max_open_positions = None;
            config.new_pool_first_size_usd = None;
            let context = crate::exec::limits::LimitContext {
                position_usd: deployment_usd.unwrap_or(0.0),
                daily_spent_usd: usage.daily_committed_usd,
                open_positions: 0,
                is_new_pool: false,
                eth_balance: f64::from(balance) / 1e18,
                eth_needed,
            };
            match crate::exec::limits::check(&config, &context) {
                Ok(()) => gates.push(crate::exec::pipeline::GateResult::lolos_publik(
                    "G6",
                    "pembatas nilai",
                    "jumlah dan cadangan gas berada dalam batas",
                )),
                Err(errors) => gates.push(crate::exec::pipeline::GateResult::gagal_publik(
                    "G6",
                    "pembatas nilai",
                    errors.join("; "),
                )),
            }
        }

        match provider.call(tx.clone()).await {
            Ok(output) => {
                gates.push(crate::exec::pipeline::GateResult::lolos_publik(
                    "G4",
                    "simulasi",
                    format!("kontrak menerima {}", aksi.nama()),
                ));
                if memasukkan_modal {
                    let (max0, max1) = planned_raw.unwrap_or_default();
                    let minimum_liquidity = planned_liquidity
                        .unwrap_or_default()
                        .saturating_sub(planned_liquidity.unwrap_or_default() / 100);
                    if rencana.versi == crate::pools::PoolVersion::V3 {
                        match crate::exec::simulate::decode_v3_liquidity_action(aksi, &output)
                            .and_then(|simulated| {
                                crate::exec::simulate::check_liquidity_action(
                                    &simulated,
                                    minimum_liquidity,
                                    max0,
                                    max1,
                                )
                                .map_err(|errors| errors.join("; "))?;
                                Ok(simulated)
                            }) {
                            Ok(simulated) => {
                                gates.push(crate::exec::pipeline::GateResult::lolos_publik(
                                    "G5",
                                    "asersi invarian",
                                    format!(
                                        "likuiditas {}, token0 {}, token1 {}",
                                        simulated.liquidity, simulated.amount0, simulated.amount1
                                    ),
                                ))
                            }
                            Err(error) => {
                                gates.push(crate::exec::pipeline::GateResult::gagal_publik(
                                    "G5",
                                    "asersi invarian",
                                    error,
                                ))
                            }
                        }
                    } else {
                        gates.push(crate::exec::pipeline::GateResult::lolos_publik(
                            "G5",
                            "asersi invarian",
                            format!(
                                "likuiditas {} dengan batas token0 {max0}, token1 {max1}",
                                planned_liquidity.unwrap_or_default()
                            ),
                        ));
                    }
                }
            }
            Err(e) => {
                let teks = e.to_string();
                let pesan = if teks.contains("revert") || teks.contains("execution reverted") {
                    crate::exec::simulate::jelaskan_revert(&teks)
                } else {
                    format!("RPC gagal: {teks}")
                };
                gates.push(crate::exec::pipeline::GateResult::gagal_publik(
                    "G4", "simulasi", pesan,
                ));
            }
        }

        let jalur_lp_lolos = if memasukkan_modal {
            let lolos = gates.iter().any(|g| g.id == "G4" && g.passed)
                && gates.iter().any(|g| g.id == "G5" && g.passed);
            gates.push(if lolos {
                crate::exec::pipeline::GateResult::lolos_publik(
                    "R1",
                    "transfer tax jalur LP",
                    "aksi likuiditas tersimulasi dengan penyelesaian jumlah penuh",
                )
            } else {
                crate::exec::pipeline::GateResult::gagal_publik(
                    "R1",
                    "transfer tax jalur LP",
                    "jalur LP belum berhasil disimulasikan; selesaikan izin lalu susun ulang",
                )
            });
            lolos
        } else {
            false
        };

        if aksi == crate::exec::aksi::JenisAksi::TambahLikuiditas {
            let jual_kecil_lolos = match uji_jual.as_ref() {
                None => {
                    gates.push(crate::exec::pipeline::GateResult::gagal_publik(
                        "R2",
                        "simulasi jual kecil",
                        "wallet belum mempunyai memecoin untuk uji jual kecil",
                    ));
                    false
                }
                Some(_) if izin_risiko_kurang => {
                    gates.push(crate::exec::pipeline::GateResult::gagal_publik(
                        "R2",
                        "simulasi jual kecil",
                        "izin router belum lengkap; selesaikan izin lalu susun ulang",
                    ));
                    false
                }
                Some((pool, memecoin, jumlah)) => {
                    match crate::risk::token::uji_jual_kecil(
                        &provider,
                        &self.registry,
                        pool,
                        pemilik,
                        *memecoin,
                        *jumlah,
                        self.cfg.limits.max_slippage_bps,
                    )
                    .await
                    {
                        Ok(keluaran) => {
                            gates.push(crate::exec::pipeline::GateResult::lolos_publik(
                                "R2",
                                "simulasi jual kecil",
                                format!(
                                    "router menerima jual {jumlah} unit dan mengutip {keluaran} unit keluaran"
                                ),
                            ));
                            true
                        }
                        Err(error) => {
                            gates.push(crate::exec::pipeline::GateResult::gagal_publik(
                                "R2",
                                "simulasi jual kecil",
                                error.to_string(),
                            ));
                            false
                        }
                    }
                }
            };
            if let Some((pool, memecoin, _)) = uji_jual.as_ref() {
                if let Err(error) = crate::risk::token::simpan_bukti(
                    &self.store,
                    self.registry.chain.id,
                    &pool.id,
                    *memecoin,
                    jalur_lp_lolos,
                    jual_kecil_lolos,
                ) {
                    tracing::warn!(error = %error, "bukti risiko token aksi gagal disimpan");
                }
            }
        }

        let semua_lolos = gates.iter().all(|g| g.passed);

        let disusun_pada = chrono::Utc::now();
        let plan_id = intent::plan_id()?;
        let siap = semua_lolos && gas > 0 && allowances.is_empty() && !self.cfg.execution.dry_run;
        let close_basis = if rencana.menutup {
            posisi.sqrt_price_x96.as_ref().and_then(|sqrt_price_x96| {
                crate::positions::muat::kutipan_pool(&self.registry, &posisi.token0, &posisi.token1)
                    .map(|(quote_token0, quote_decimals, currency)| {
                        crate::exec::journal::CloseBasis {
                            sqrt_price_x96: sqrt_price_x96.clone(),
                            fee0: posisi.fee0.clone(),
                            fee1: posisi.fee1.clone(),
                            quote_token0,
                            quote_decimals,
                            currency_id: currency.id,
                            currency_symbol: currency.symbol,
                            symbol0: posisi.symbol0.clone(),
                            symbol1: posisi.symbol1.clone(),
                        }
                    })
            })
        } else {
            None
        };
        let mut pending = self.pending_action.write().await;
        if !self.intent.action_current(revision) {
            return Err("permintaan aksi sudah berubah".into());
        }
        *pending = Some(execute::AksiTersimpan {
            journal_context: crate::exec::journal::Context {
                operation: match aksi {
                    crate::exec::aksi::JenisAksi::TambahLikuiditas => {
                        crate::exec::journal::Operation::Increase
                    }
                    crate::exec::aksi::JenisAksi::Rebalance => {
                        crate::exec::journal::Operation::Rebalance
                    }
                    crate::exec::aksi::JenisAksi::KlaimFee => {
                        crate::exec::journal::Operation::Claim
                    }
                    crate::exec::aksi::JenisAksi::TutupPosisi => {
                        crate::exec::journal::Operation::Close
                    }
                },
                plan_id: plan_id.clone(),
                flow_id: request_id.clone(),
                pool_id: None,
                position_id: Some(position_id.clone()),
                token0: posisi.token0.parse().ok(),
                token1: posisi.token1.parse().ok(),
                deployment_usd,
                close_basis,
            },
            plan_id: plan_id.clone(),
            position_id: position_id.clone(),
            revision,
            request_id: request_id.clone(),
            action: aksi,
            input,
            siap,
            token_id: token_id.clone(),
            versi: rencana.versi,
            calldata: rencana.calldata,
            tujuan,
            value,
            hook,
            allowances: allowances.clone(),
            amount0: planned_raw.map(|amounts| amounts.0).unwrap_or_default(),
            amount1: planned_raw.map(|amounts| amounts.1).unwrap_or_default(),
            harapan,
            menutup: rencana.menutup,
            nama: aksi.nama(),
            disusun_pada,
        });

        Ok(execute::ActionPlanView {
            plan_id,
            position_id,
            request_id,
            token_id,
            action: aksi,
            nama: aksi.nama(),
            gates,
            gas_estimate: gas,
            dry_run: self.cfg.execution.dry_run,
            menutup: rencana.menutup,
            siap,
            expires_at: (disusun_pada
                + chrono::Duration::seconds(execute::RENCANA_KEDALUWARSA_DETIK))
            .to_rfc3339(),
            fee0: posisi.fee0,
            fee1: posisi.fee1,
            allowances,
            amounts: planned_amounts,
            tick_lower_baru: target_ticks.map(|ticks| ticks.0),
            tick_upper_baru: target_ticks.map(|ticks| ticks.1),
        })
    }

    /// Setujui aksi terakhir dan kirim transaksinya.
    pub async fn on_execute_action(&self, plan_id: String) {
        let _flow = self.flow_lock.lock().await;
        match self.kirim_aksi(plan_id).await {
            Ok(view) => {
                let verifikasi_gagal = view
                    .outcome
                    .verification
                    .as_ref()
                    .is_some_and(|gate| !gate.passed);
                let _ = self
                    .events
                    .send(protocol::ServerMessage::SendResult(Box::new(view)));
                if verifikasi_gagal {
                    self.alert_kritis(
                        "action_unverified",
                        "transaksi sukses tetapi keadaan posisi tidak cocok dengan rencana. Periksa posisi sebelum melakukan aksi lain"
                            .into(),
                    );
                }
                // Keadaan posisi berubah setelah aksi, jadi daftarnya dimuat
                // ulang. Tanpa ini kartunya menampilkan fee yang sudah diambil.
                self.on_get_positions().await;
            }
            Err(pesan) => self.alert_kritis("action_failed", pesan),
        }
    }

    /// Jalankan seluruh izin lalu aksi posisi dari satu tombol.
    pub async fn on_run_action_flow(&self, plan_id: String) {
        let (action, subject) = {
            let guard = self.pending_action.read().await;
            guard
                .as_ref()
                .filter(|r| r.plan_id == plan_id)
                .map(|r| (r.action, Some(r.request_id.clone())))
                .unwrap_or((
                    crate::exec::aksi::JenisAksi::Rebalance,
                    Some(plan_id.clone()),
                ))
        };
        let (awal, sukses, tertunda, gagal, nama) = Self::label_notifikasi_aksi(action);
        self.beri_tahu_subjek(
            protocol::AlertLevel::Info,
            awal,
            format!("{nama} dimulai"),
            subject.clone(),
        )
        .await;
        match self.run_action_flow(plan_id).await {
            Ok((status, terverifikasi)) => {
                let (level, code, message) = match (status, terverifikasi) {
                    (crate::exec::send::TxStatus::Sukses, true) => (
                        protocol::AlertLevel::Info,
                        sukses,
                        format!("{nama} berhasil"),
                    ),
                    (crate::exec::send::TxStatus::Sukses, false) => (
                        protocol::AlertLevel::Warning,
                        gagal,
                        format!("{nama} masuk rantai, tetapi hasilnya belum terverifikasi"),
                    ),
                    (crate::exec::send::TxStatus::Menggantung, _) => (
                        protocol::AlertLevel::Warning,
                        tertunda,
                        format!("{nama} masih diproses. Hasil akhirnya akan diberitahukan"),
                    ),
                    (crate::exec::send::TxStatus::Revert, _) => (
                        protocol::AlertLevel::Critical,
                        gagal,
                        format!("{nama} gagal karena transaksi ditolak rantai"),
                    ),
                };
                self.beri_tahu_subjek(level, code, message, subject).await;
            }
            Err(pesan) => {
                self.alert_kritis("action_flow_failed", pesan.clone());
                self.beri_tahu_subjek(
                    protocol::AlertLevel::Critical,
                    gagal,
                    format!("{nama} gagal: {pesan}"),
                    subject,
                )
                .await;
            }
        }
    }

    fn label_notifikasi_aksi(
        action: crate::exec::aksi::JenisAksi,
    ) -> (
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
    ) {
        match action {
            crate::exec::aksi::JenisAksi::TambahLikuiditas => (
                "increase_started",
                "increase_succeeded",
                "increase_pending",
                "increase_failed",
                "Tambah likuiditas",
            ),
            crate::exec::aksi::JenisAksi::Rebalance => (
                "rebalance_started",
                "rebalance_succeeded",
                "rebalance_pending",
                "rebalance_failed",
                "Rebalance",
            ),
            crate::exec::aksi::JenisAksi::KlaimFee => (
                "claim_started",
                "claim_succeeded",
                "claim_pending",
                "claim_failed",
                "Klaim fee",
            ),
            crate::exec::aksi::JenisAksi::TutupPosisi => (
                "close_started",
                "close_succeeded",
                "close_pending",
                "close_failed",
                "Penutupan posisi",
            ),
        }
    }

    async fn run_action_flow(
        &self,
        mut plan_id: String,
    ) -> Result<(crate::exec::send::TxStatus, bool), String> {
        let _flow = self.flow_lock.lock().await;
        let mut izin_selesai = false;
        for _ in 0..MAX_FLOW_STEPS {
            let (step_id, siap) = {
                let guard = self.pending_action.read().await;
                let r = guard.as_ref().ok_or("rencana aksi sudah tidak tersedia")?;
                r.periksa(&plan_id, &self.intent)?;
                (
                    r.allowances.first().map(|step| step.step_id.clone()),
                    r.allowances.is_empty() && r.siap,
                )
            };

            if let Some(step_id) = step_id {
                let view = self.kirim_approve(plan_id.clone(), step_id.clone()).await?;
                let status = view.outcome.status;
                let _ = self
                    .events
                    .send(protocol::ServerMessage::SendResult(Box::new(view)));
                if status != crate::exec::send::TxStatus::Sukses {
                    if status == crate::exec::send::TxStatus::Menggantung {
                        self.batalkan_flow_tidak_pasti(&plan_id).await;
                    }
                    return Ok((status, false));
                }

                self.tandai_izin_selesai(&plan_id, &step_id).await?;
                izin_selesai = true;
                continue;
            }

            if izin_selesai {
                let (position_id, action, input, request_id) = {
                    let guard = self.pending_action.read().await;
                    let r = guard
                        .as_ref()
                        .filter(|r| r.plan_id == plan_id)
                        .ok_or("rencana aksi berubah setelah izin token")?;
                    (
                        r.position_id.clone(),
                        r.action,
                        r.input.clone(),
                        r.request_id.clone(),
                    )
                };
                let revision = self.intent.begin_action();
                let view = self
                    .susun_rencana_aksi(position_id, action, input, request_id, revision)
                    .await?;
                plan_id = view.plan_id.clone();
                let _ = self
                    .events
                    .send(protocol::ServerMessage::ActionPlan(Box::new(view)));
                izin_selesai = false;
                continue;
            }

            if !siap {
                return Err("Aksi belum siap. Muat ulang posisi lalu coba lagi".into());
            }

            let view = self.kirim_aksi(plan_id).await?;
            let verifikasi_gagal = view
                .outcome
                .verification
                .as_ref()
                .is_some_and(|gate| !gate.passed);
            let status = view.outcome.status;
            let _ = self
                .events
                .send(protocol::ServerMessage::SendResult(Box::new(view)));
            self.on_get_positions().await;
            return Ok((status, !verifikasi_gagal));
        }
        Err("Alur aksi berhenti karena terlalu banyak langkah".into())
    }

    async fn kirim_aksi(&self, plan_id: String) -> Result<execute::SendResultView, String> {
        let _eksekusi = self.execution_lock.lock().await;
        self.ensure_execution().await?;
        if self.cfg.execution.dry_run {
            return Err("engine sedang dry run, tidak ada yang dikirim".into());
        }

        // Dikeluarkan dari slot sebelum dikirim, dengan alasan yang sama seperti
        // rencana membuka posisi: yang masih tersimpan bisa disetujui dua kali.
        let tersimpan = {
            let mut pending = self.pending_action.write().await;
            let r = pending.as_ref().ok_or("belum ada aksi yang disusun")?;
            r.periksa(&plan_id, &self.intent)?;
            self.periksa_hook_aksi(r).await?;
            r.periksa(&plan_id, &self.intent)?;
            if !r.allowances.is_empty() || !r.siap {
                return Err("rencana aksi berubah atau belum lolos pemeriksaan".into());
            }
            pending.take().ok_or("rencana aksi hilang")?
        };

        if tersimpan.kedaluwarsa() {
            return Err("rencana aksi sudah kedaluwarsa, susun ulang".into());
        }

        let provider = self
            .chain
            .provider()
            .await
            .map_err(|e| format!("RPC tidak bisa dipakai: {e}"))?;

        let mut wallet = self.wallet.lock().await;
        let signer = wallet.signer().map_err(|e| e.to_string())?.clone();
        drop(wallet);

        let tokens = tersimpan
            .journal_context
            .token0
            .zip(tersimpan.journal_context.token1);
        let multicall = self.registry.contract("common", "multicall3").ok();
        let catat_penerimaan = matches!(
            tersimpan.action,
            crate::exec::aksi::JenisAksi::KlaimFee | crate::exec::aksi::JenisAksi::TutupPosisi
        );
        let before = match (catat_penerimaan, tokens, multicall) {
            (true, Some((token0, token1)), Some(multicall)) => {
                execute::saldo_dua_token(&provider, multicall, signer.address(), token0, token1)
                    .await
                    .ok()
            }
            _ => None,
        };
        let journal_context = tersimpan.journal_context.clone();

        let terkirim = crate::exec::send::send_calldata(
            &provider,
            &self.nonce,
            &signer,
            tersimpan.tujuan,
            tersimpan.calldata,
            tersimpan.value,
            self.registry.chain.id,
            &self.cfg.execution,
            &self.cfg.limits,
            &crate::exec::journal::Journal {
                store: &self.store,
                context: journal_context,
            },
        )
        .await
        .map_err(|e| e.to_string())?;

        let mut minted_amounts = None;
        let mut hasil_token_id = None;
        let mut verification = None;
        if let Some(receipt) = terkirim.receipt.as_ref().filter(|receipt| receipt.status()) {
            let token_id = tersimpan.token_id.parse().ok();
            let from_event = (catat_penerimaan && tersimpan.versi == crate::pools::PoolVersion::V3)
                .then(|| {
                    crate::exec::send::collect_dari_log(
                        receipt.inner.logs(),
                        tersimpan.tujuan,
                        token_id?,
                        signer.address(),
                    )
                })
                .flatten()
                .map(|amounts| crate::exec::journal::ReceivedAmounts {
                    amount0: amounts.amount0,
                    amount1: amounts.amount1,
                    evidence: "v3_collect_event".into(),
                });
            let from_balances = match (before, tokens, multicall) {
                (Some(before), Some((token0, token1)), Some(multicall)) => {
                    let after = execute::saldo_dua_token(
                        &provider,
                        multicall,
                        signer.address(),
                        token0,
                        token1,
                    )
                    .await
                    .ok();
                    let gas_wei =
                        (receipt.gas_used > 0 && receipt.effective_gas_price > 0).then(|| {
                            alloy::primitives::U256::from(receipt.gas_used)
                                * alloy::primitives::U256::from(receipt.effective_gas_price)
                        });
                    after
                        .and_then(|after| {
                            execute::received_from_balances(before, after, token0, token1, gas_wei)
                        })
                        .map(|(amount0, amount1)| crate::exec::journal::ReceivedAmounts {
                            amount0: amount0.to_string(),
                            amount1: amount1.to_string(),
                            evidence: "wallet_balance_delta".into(),
                        })
                }
                _ => None,
            };
            if let Some(received) = from_event.or(from_balances) {
                if let Err(error) = crate::exec::journal::record_received(
                    &self.store,
                    self.registry.chain.id,
                    signer.address(),
                    terkirim.tx_hash,
                    received,
                ) {
                    tracing::warn!(error = %error, tx = %terkirim.tx_hash, "hasil aksi tidak dapat dicatat");
                }
            }

            let gate = match &tersimpan.harapan {
                execute::HarapanAksi::TidakAda => None,
                execute::HarapanAksi::Tambah {
                    sebelum,
                    minimum_tambahan,
                } => {
                    hasil_token_id = Some(tersimpan.token_id.clone());
                    if tersimpan.versi == crate::pools::PoolVersion::V3 {
                        if let Some((id, amounts)) =
                            crate::exec::send::mint_dari_log(receipt.inner.logs(), tersimpan.tujuan)
                                .filter(|(id, _)| id.to_string() == tersimpan.token_id)
                        {
                            hasil_token_id = Some(id.to_string());
                            minted_amounts = Some(amounts);
                        }
                    }
                    let actual = match tersimpan.versi {
                        crate::pools::PoolVersion::V3 => crate::exec::verify::read_position(
                            &provider,
                            tersimpan.tujuan,
                            token_id_u256(&tersimpan.token_id)?,
                        )
                        .await
                        .map(|position| {
                            (
                                position.token0,
                                position.token1,
                                position.fee,
                                position.tick_lower,
                                position.tick_upper,
                                position.liquidity,
                            )
                        }),
                        crate::pools::PoolVersion::V4 => crate::exec::v4::verify::baca_posisi(
                            &provider,
                            tersimpan.tujuan,
                            multicall.ok_or("alamat multicall tidak tersedia")?,
                            token_id_u256(&tersimpan.token_id)?,
                        )
                        .await
                        .map(|position| {
                            (
                                position.currency0,
                                position.currency1,
                                position.fee,
                                position.tick_lower,
                                position.tick_upper,
                                position.liquidity,
                            )
                        }),
                    };
                    Some(match actual {
                        Ok((a0, a1, fee, lower, upper, liquidity)) => {
                            let before_liquidity = sebelum.liquidity.parse::<u128>().unwrap_or(0);
                            let minimum = before_liquidity.saturating_add(*minimum_tambahan);
                            let cocok = a0.to_string().eq_ignore_ascii_case(&sebelum.token0)
                                && a1.to_string().eq_ignore_ascii_case(&sebelum.token1)
                                && fee == sebelum.fee
                                && lower == sebelum.tick_lower
                                && upper == sebelum.tick_upper
                                && liquidity >= minimum;
                            if cocok {
                                crate::exec::pipeline::GateResult::lolos_publik(
                                    "G7",
                                    "verifikasi pasca kirim",
                                    format!("likuiditas posisi naik menjadi {liquidity}"),
                                )
                            } else {
                                crate::exec::pipeline::GateResult::gagal_publik(
                                    "G7",
                                    "verifikasi pasca kirim",
                                    format!(
                                        "posisi setelah tambah tidak cocok: likuiditas {liquidity}, minimum {minimum}"
                                    ),
                                )
                            }
                        }
                        Err(error) => crate::exec::pipeline::GateResult::gagal_publik(
                            "G7",
                            "verifikasi pasca kirim",
                            error,
                        ),
                    })
                }
                execute::HarapanAksi::RebalanceV3 {
                    token_lama,
                    permintaan,
                    expected_liquidity,
                } => {
                    let event =
                        crate::exec::send::mint_dari_log(receipt.inner.logs(), tersimpan.tujuan);
                    if let Some((id, amounts)) = event.as_ref() {
                        hasil_token_id = Some(id.to_string());
                        minted_amounts = Some(amounts.clone());
                    }
                    Some(match event {
                        Some((id, _)) => {
                            let baru =
                                crate::exec::verify::read_position(&provider, tersimpan.tujuan, id)
                                    .await;
                            let lama_hilang = crate::exec::verify::read_position(
                                &provider,
                                tersimpan.tujuan,
                                *token_lama,
                            )
                            .await
                            .is_err();
                            match baru {
                                Ok(position) if lama_hilang => {
                                    match crate::exec::verify::check_matches_request(
                                        &position,
                                        permintaan,
                                        *expected_liquidity,
                                    ) {
                                        Ok(()) => crate::exec::pipeline::GateResult::lolos_publik(
                                            "G7",
                                            "verifikasi pasca kirim",
                                            format!("NFT lama dibakar dan posisi baru {id} cocok"),
                                        ),
                                        Err(errors) => {
                                            crate::exec::pipeline::GateResult::gagal_publik(
                                                "G7",
                                                "verifikasi pasca kirim",
                                                errors.join("; "),
                                            )
                                        }
                                    }
                                }
                                Ok(_) => crate::exec::pipeline::GateResult::gagal_publik(
                                    "G7",
                                    "verifikasi pasca kirim",
                                    "NFT lama masih terbaca setelah rebalance",
                                ),
                                Err(error) => crate::exec::pipeline::GateResult::gagal_publik(
                                    "G7",
                                    "verifikasi pasca kirim",
                                    error,
                                ),
                            }
                        }
                        None => crate::exec::pipeline::GateResult::gagal_publik(
                            "G7",
                            "verifikasi pasca kirim",
                            "event IncreaseLiquidity posisi baru tidak ditemukan atau ambigu",
                        ),
                    })
                }
                execute::HarapanAksi::RebalanceV4 {
                    token_lama,
                    permintaan,
                } => {
                    let id =
                        crate::exec::v4::pipeline::token_id_dari_receipt(receipt, tersimpan.tujuan);
                    hasil_token_id = id.map(|value| value.to_string());
                    Some(match id {
                        Some(id) => {
                            let baru = crate::exec::v4::verify::baca_posisi(
                                &provider,
                                tersimpan.tujuan,
                                multicall.ok_or("alamat multicall tidak tersedia")?,
                                id,
                            )
                            .await;
                            let lama_hilang = crate::exec::v4::verify::baca_posisi(
                                &provider,
                                tersimpan.tujuan,
                                multicall.ok_or("alamat multicall tidak tersedia")?,
                                *token_lama,
                            )
                            .await
                            .is_err();
                            match baru {
                                Ok(position) if lama_hilang => {
                                    match crate::exec::v4::verify::cocok_dengan_permintaan(
                                        &position, permintaan,
                                    ) {
                                        Ok(()) => crate::exec::pipeline::GateResult::lolos_publik(
                                            "G7",
                                            "verifikasi pasca kirim",
                                            format!(
                                                "NFT lama dibakar dan posisi V4 baru {id} cocok"
                                            ),
                                        ),
                                        Err(errors) => {
                                            crate::exec::pipeline::GateResult::gagal_publik(
                                                "G7",
                                                "verifikasi pasca kirim",
                                                errors.join("; "),
                                            )
                                        }
                                    }
                                }
                                Ok(_) => crate::exec::pipeline::GateResult::gagal_publik(
                                    "G7",
                                    "verifikasi pasca kirim",
                                    "NFT lama masih terbaca setelah rebalance",
                                ),
                                Err(error) => crate::exec::pipeline::GateResult::gagal_publik(
                                    "G7",
                                    "verifikasi pasca kirim",
                                    error,
                                ),
                            }
                        }
                        None => crate::exec::pipeline::GateResult::gagal_publik(
                            "G7",
                            "verifikasi pasca kirim",
                            "Transfer NFT posisi baru tidak ditemukan",
                        ),
                    })
                }
            };
            verification = gate;
        }
        let mut outcome = crate::exec::send::jadikan_hasil(terkirim);
        outcome.verification = verification;
        outcome.token_id = hasil_token_id;
        outcome.minted_amounts = minted_amounts;
        let verified = outcome.status == crate::exec::send::TxStatus::Sukses
            && outcome
                .verification
                .as_ref()
                .is_some_and(|gate| gate.passed);
        if verified {
            match tersimpan.action {
                crate::exec::aksi::JenisAksi::TambahLikuiditas => {
                    let actual = outcome.minted_amounts.as_ref();
                    let amount0 = actual
                        .and_then(|amounts| amounts.amount0.parse().ok())
                        .unwrap_or(tersimpan.amount0);
                    let amount1 = actual
                        .and_then(|amounts| amounts.amount1.parse().ok())
                        .unwrap_or(tersimpan.amount1);
                    if let Err(error) = crate::pnl::tambah_setoran(
                        &self.store,
                        &tersimpan.position_id,
                        amount0,
                        amount1,
                        actual.is_some(),
                    ) {
                        tracing::warn!(error = %error, "basis tambah likuiditas tidak dapat dicatat");
                    }
                }
                crate::exec::aksi::JenisAksi::Rebalance => {
                    if let Some(token_id) = outcome.token_id.as_ref() {
                        let position_id = crate::positions::position_id(
                            self.registry.chain.id,
                            tersimpan.tujuan,
                            token_id
                                .parse()
                                .map_err(|_| "token ID rebalance tidak valid")?,
                        );
                        let actual = outcome.minted_amounts.as_ref();
                        crate::pnl::simpan(
                            &self.store,
                            &crate::pnl::Catatan {
                                position_id,
                                token_id: token_id.clone(),
                                flow_id: Some(tersimpan.request_id.clone()),
                                setor0: actual
                                    .map(|amounts| amounts.amount0.clone())
                                    .unwrap_or_else(|| tersimpan.amount0.to_string()),
                                setor1: actual
                                    .map(|amounts| amounts.amount1.clone())
                                    .unwrap_or_else(|| tersimpan.amount1.to_string()),
                                harga_buka: None,
                                lengkap: false,
                                setoran_aktual: actual.is_some(),
                                gas_wei: None,
                                gas_kutipan: None,
                                slippage_kutipan: Some(0.0),
                                dibuka_pada: outcome.sent_at.clone(),
                                tx_buka: Some(outcome.tx_hash.clone()),
                                rekonstruksi_log: None,
                                rekonstruksi_v4: None,
                            },
                        );
                    }
                }
                crate::exec::aksi::JenisAksi::KlaimFee
                | crate::exec::aksi::JenisAksi::TutupPosisi => {}
            }
        }
        Ok(execute::SendResultView {
            kind: "action",
            outcome,
        })
    }

    /// Muat ulang posisi LP wallet dan siarkan hasilnya.
    ///
    /// Pemuatannya menyentuh jaringan dan bisa makan beberapa detik, jadi ia
    /// dijalankan di tugas terpisah. Kegagalan dikirim sebagai bagian dari
    /// muatan, bukan menggantikannya, supaya panel tetap bisa menampilkan
    /// posisi lama sambil menyebutkan apa yang salah.
    /// Pindai pool ramai dan kirim hasilnya ke panel.
    ///
    /// Kegagalan dikirim sebagai `error` di dalam hasil, bukan ditelan menjadi
    /// daftar kosong. Daftar kosong berarti rantainya sepi, dan user yang
    /// membaca daftar kosong akan menyimpulkan tidak ada peluang, bukan bahwa
    /// alatnya sedang tidak bisa membaca.
    /// Baca distribusi likuiditas pool yang sedang aktif dan kirim ke panel.
    ///
    /// Kegagalan dikirim sebagai `error` di dalam hasil, bukan sebagai
    /// distribusi kosong. Histogram kosong berarti tidak ada yang menaruh dana
    /// di sekitar harga ini, dan itu justru sinyal yang sangat kuat. Ia tidak
    /// boleh tertukar dengan gagal membaca.
    /// Tulis daftar endpoint RPC baru ke config.toml.
    ///
    /// Tidak menukar endpoint yang sedang dipakai. Panel diberi tahu bahwa
    /// engine perlu dijalankan ulang, karena mengatakan tersimpan saja akan
    /// membuat user mengira RPC barunya sudah bekerja padahal belum.
    pub async fn on_set_rpc(&self, http: Vec<String>) {
        match crate::config::tulis_rpc(&http) {
            Ok(path) => self.alert_info(
                "rpc_saved",
                format!(
                    "Endpoint tersimpan di {}. Jalankan ulang engine supaya dipakai.",
                    path.display()
                ),
            ),
            Err(e) => self.alert_kritis("rpc_failed", e.to_string()),
        }
    }

    pub async fn on_get_depth(&self) {
        let Some(view) = self.susun_depth().await else {
            return;
        };
        let _ = self
            .events
            .send(protocol::ServerMessage::Depth(Box::new(view)));
    }

    async fn susun_depth(&self) -> Option<crate::pools::distribusi::Distribusi> {
        use crate::pools::distribusi;

        let (pool, alamat_token, desimal_meme, desimal_quote) = {
            let guard = self.active.read().await;
            let active = guard.as_ref()?;
            let pool = active.selected_pool()?.clone();
            let meta = active.meta.clone();
            (
                pool,
                meta.address.clone(),
                meta.decimals,
                active.quote_decimals,
            )
        };

        let kosong = |error: String| distribusi::Distribusi {
            pool_id: pool.id.clone(),
            tick_sekarang: pool.tick,
            harga_sekarang: 0.0,
            tick_spacing: pool.tick_spacing,
            pita: Vec::new(),
            quote_adalah_token1: true,
            error: Some(error),
        };

        let provider = match self.chain.provider().await {
            Ok(p) => p,
            Err(e) => return Some(kosong(format!("RPC tidak bisa dipakai: {e}"))),
        };

        let multicall = match self.registry.contract("common", "multicall3") {
            Ok(a) => a,
            Err(e) => return Some(kosong(e.to_string())),
        };

        // Data per tick ada di tempat yang berbeda menurut protokolnya: V3
        // menyimpannya di kontrak poolnya sendiri, V4 di PoolManager yang
        // dibaca lewat StateView. Sebelum ini seluruh jalur mengandaikan V4,
        // dan setiap pool V3 selalu melaporkan distribusi gagal dibaca.
        let sumber = match pool.version {
            crate::pools::PoolVersion::V4 => {
                match self.registry.contract("uniswap_v4", "state_view") {
                    Ok(a) => distribusi::SumberTick::V4 { state_view: a },
                    Err(e) => return Some(kosong(e.to_string())),
                }
            }
            crate::pools::PoolVersion::V3 => match pool.id.parse() {
                Ok(a) => distribusi::SumberTick::V3 { pool: a },
                Err(_) => {
                    return Some(kosong(format!(
                        "alamat pool V3 `{}` tidak bisa dibaca",
                        pool.id
                    )))
                }
            },
        };

        // Memecoin di sisi token0 berarti kutipannya token1. Arah ini
        // menentukan arah seluruh sumbu harga, jadi ia diambil dari alamat
        // yang sedang dilihat, bukan ditebak dari urutan.
        let quote_adalah_token1 = pool.token0.eq_ignore_ascii_case(&alamat_token);

        match distribusi::susun(
            &provider,
            sumber,
            multicall,
            &pool.id,
            distribusi::KeadaanPool {
                tick_spacing: pool.tick_spacing,
                tick_sekarang: pool.tick,
                likuiditas: pool.liquidity.parse().unwrap_or(0),
                quote_adalah_token1,
                skala_meme: 10f64.powi(desimal_meme as i32),
                skala_quote: 10f64.powi(desimal_quote as i32),
            },
        )
        .await
        {
            Ok(d) => Some(d),
            Err(e) => Some(kosong(format!("distribusi gagal dibaca: {e}"))),
        }
    }

    pub async fn on_scan(&self) {
        let view = self.susun_scan().await;
        let _ = self
            .events
            .send(protocol::ServerMessage::ScanResults(Box::new(view)));
    }

    async fn susun_scan(&self) -> protocol::ScanView {
        use crate::pools::kandidat;

        let waktu = chrono::Utc::now().to_rfc3339();
        let kosong = |error: Option<String>| protocol::ScanView {
            rows: Vec::new(),
            hooks: Vec::new(),
            adopsi_lengkap: false,
            jendela_pindai_menit: 0.0,
            jendela_volume_menit: 0.0,
            volume_jendela_pendek: false,
            modal_acuan: kandidat::MODAL_ACUAN,
            total_swap: 0,
            dipindai_pada: waktu.clone(),
            error,
        };

        let provider = match self.chain.provider().await {
            Ok(p) => p,
            Err(e) => return kosong(Some(format!("RPC tidak bisa dipakai: {e}"))),
        };

        let hasil = match crate::pools::scan::pindai(&provider, &self.registry).await {
            Ok(h) => h,
            Err(e) => return kosong(Some(format!("pemindaian gagal: {e}"))),
        };

        let menit = |dari: u64, sampai: u64| {
            sampai.saturating_sub(dari) as f64 * self.registry.chain.block_time_ms as f64 / 60_000.0
        };
        let jendela_pindai_menit = menit(hasil.dari_blok, hasil.sampai_blok);

        // ALLOWLIST DATANG DARI PERSETUJUAN YANG TERCATAT
        //
        // Belum ada hook yang bisa lolos otomatis, karena dua dari lima syarat
        // adopsi PRD 5.4 belum bisa diukur alat ini. Yang mengisi allowlist
        // adalah persetujuan sadar user, yang tersimpan beserta penilaian saat
        // itu. Hook yang belum disetujui tetap ditandai `Hindari`, dan itu
        // sikap bawaan yang benar: hook bisa menghalangi penarikan, dan itu
        // kehilangan modal seluruhnya, bukan risiko harga yang bisa ditanggung.
        let disetujui: Vec<String> = hasil
            .kandidat
            .iter()
            .map(|k| k.hooks.to_lowercase())
            .filter(|h| h != crate::pools::ZERO_ADDRESS)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .filter(|h| crate::risk::persetujuan::disetujui(&self.store, h))
            .collect();

        let daftar = match kandidat::perkaya(&provider, &self.registry, &hasil, &disetujui).await {
            Ok(d) => d,
            Err(e) => {
                let mut v = kosong(Some(format!("penilaian kandidat gagal: {e}")));
                v.total_swap = hasil.total_swap;
                v.jendela_pindai_menit = jendela_pindai_menit;
                return v;
            }
        };

        // Penilaian hook disusun untuk hook yang benar benar muncul di hasil,
        // bukan untuk semuanya. Satu pemindaian `Initialize` melayani seluruh
        // hook sekaligus, jadi biayanya tidak bertambah per hook.
        let (hooks, adopsi_lengkap) = self.nilai_hook_pada(&daftar.baris, hasil.sampai_blok).await;

        protocol::ScanView {
            rows: daftar.baris,
            hooks,
            adopsi_lengkap,
            jendela_pindai_menit,
            jendela_volume_menit: daftar.jendela_volume_detik / 60.0,
            volume_jendela_pendek: daftar.volume_jendela_pendek,
            modal_acuan: kandidat::MODAL_ACUAN,
            total_swap: hasil.total_swap,
            dipindai_pada: hasil.dipindai_pada,
            error: None,
        }
    }

    /// Nilai setiap hook yang muncul di hasil pemindaian.
    ///
    /// Kegagalan menilai satu hook tidak menggagalkan yang lain, dan tidak
    /// menggagalkan pemindaiannya. Hook yang gagal dinilai tetap ditolak,
    /// karena yang belum terbukti aman bukan berarti aman.
    async fn nilai_hook_pada(
        &self,
        baris: &[crate::pools::kandidat::BarisScan],
        sampai_blok: u64,
    ) -> (Vec<crate::risk::hook::PenilaianHook>, bool) {
        let alamat: std::collections::BTreeSet<String> =
            baris.iter().filter_map(|b| b.hooks.clone()).collect();
        if alamat.is_empty() {
            // Tidak ada hook berarti tidak ada yang perlu dinilai, dan sejarah
            // adopsi tidak perlu disentuh sama sekali. Ini jalur tercepat, dan
            // ia memang yang paling sering terjadi pada pool tanpa hook.
            return (Vec::new(), true);
        }

        let Ok(provider) = self.chain.provider().await else {
            return (Vec::new(), false);
        };

        // Satu pemindaian adopsi melayani seluruh hook. Kegagalannya tidak
        // menggagalkan penilaian: kriteria jumlah pool dan umur akan terbaca
        // `belum diukur`, dan yang belum diukur tidak pernah lolos.
        let adopsi = crate::risk::hook::hitung_adopsi(
            &provider,
            &self.registry,
            Some(&self.store),
            sampai_blok,
        )
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "penghitungan adopsi hook gagal");
            crate::risk::hook::Adopsi::default()
        });

        let mut out = Vec::with_capacity(alamat.len());
        for h in alamat {
            match crate::risk::hook::nilai(&provider, &self.registry, &adopsi, &h, sampai_blok)
                .await
            {
                Ok(p) => {
                    if p.lolos_otomatis {
                        if let Err(e) = crate::risk::persetujuan::catat_otomatis(&self.store, &p) {
                            tracing::warn!(hook = %h, error = %e, "allowlist otomatis gagal disimpan");
                        }
                    }
                    out.push(p);
                }
                Err(e) => tracing::warn!(hook = %h, error = %e, "penilaian hook gagal"),
            }
        }
        (out, adopsi.lengkap && adopsi.posisi_lengkap)
    }

    /// Catat persetujuan sadar user atas sebuah hook, PRD 5.4.
    ///
    /// Penilaiannya dibaca ulang di sini, tidak diterima dari panel. Penilaian
    /// yang datang dari panel bisa saja sudah diubah, dan bagian yang paling
    /// menarik untuk diubah justru kesimpulan bisa diupgrade atau tidaknya.
    pub async fn on_approve_hook(&self, hook: String) {
        if hook.parse::<alloy::primitives::Address>().is_err() {
            return self.alert_kritis("hook_failed", "alamat hook tidak valid".into());
        }
        let ticket = self.intent.begin_hook(&hook);
        let provider = match self.chain.provider().await {
            Ok(p) => p,
            Err(e) => {
                return self.alert_kritis("hook_failed", format!("RPC tidak bisa dipakai: {e}"))
            }
        };
        let blok = match provider.get_block_number().await {
            Ok(block) => block,
            Err(e) => {
                return self
                    .alert_kritis("hook_failed", format!("blok penilaian tidak terbaca: {e}"))
            }
        };

        // Adopsi dibaca ulang di sini, bukan diambil dari hasil pemindaian
        // terakhir. Persetujuan menyimpan penilaian saat itu sebagai catatan
        // yang bisa ditinjau, jadi angkanya harus yang paling baru, bukan yang
        // kebetulan masih tersimpan dari pemindaian entah kapan.
        let adopsi =
            crate::risk::hook::hitung_adopsi(&provider, &self.registry, Some(&self.store), blok)
                .await
                .unwrap_or_default();

        let penilaian =
            match crate::risk::hook::nilai(&provider, &self.registry, &adopsi, &hook, blok).await {
                Ok(p) => p,
                Err(e) => {
                    return self
                        .alert_kritis("hook_failed", format!("hook tidak bisa dinilai: {e}"))
                }
            };

        let _eksekusi = self.execution_lock.lock().await;
        if !self.intent.hook_current(&hook, ticket) {
            return;
        }
        match crate::risk::persetujuan::setujui(&self.store, &penilaian) {
            Ok(_) => {
                self.intent.change();
                self.alert_info(
                    "hook_approved",
                    format!(
                        "Hook {hook} disetujui dan dicatat. Pindai ulang untuk melihat efeknya."
                    ),
                );
            }
            Err(e) => self.alert_kritis("hook_rejected", e.to_string()),
        }
    }

    /// Cabut persetujuan sebuah hook.
    pub async fn on_forget_hook(&self, hook: String) {
        if hook.parse::<alloy::primitives::Address>().is_err() {
            return self.alert_kritis("hook_failed", "alamat hook tidak valid".into());
        }
        let ticket = self.intent.begin_hook(&hook);
        let _eksekusi = self.execution_lock.lock().await;
        if !self.intent.hook_current(&hook, ticket) {
            return;
        }
        let ada = crate::risk::persetujuan::cabut(&self.store, &hook);
        if ada {
            self.intent.change();
        }
        self.alert_info(
            "hook_forgotten",
            if ada {
                format!("Persetujuan hook {hook} dicabut. Pindai ulang untuk melihat efeknya.")
            } else {
                format!("Hook {hook} memang belum pernah disetujui.")
            },
        );
    }

    pub async fn on_get_positions(&self) {
        let _pemuatan = self.positions_lock.lock().await;
        let view = self.susun_positions().await;

        // Perubahan keadaan range diperiksa SEBELUM disimpan, karena
        // perbandingannya butuh keadaan sebelumnya.
        self.periksa_perubahan_range(&view.positions).await;

        if view.error.is_none() {
            *self.positions.write().await = view.positions.clone();
        }
        let _ = self
            .events
            .send(protocol::ServerMessage::Positions(Box::new(view)));
    }

    pub async fn on_get_transactions(&self) {
        let owner = self.wallet.lock().await.address();
        let view = match owner {
            Some(owner) => {
                match crate::exec::journal::views(&self.store, self.registry.chain.id, owner) {
                    Ok(items) => protocol::TransactionsView {
                        owner: owner.to_string(),
                        items,
                        error: None,
                    },
                    Err(error) => protocol::TransactionsView {
                        owner: owner.to_string(),
                        items: Vec::new(),
                        error: Some(format!("jurnal transaksi tidak terbaca: {error}")),
                    },
                }
            }
            None => protocol::TransactionsView {
                owner: String::new(),
                items: Vec::new(),
                error: Some("wallet belum dibuat".into()),
            },
        };
        let _ = self
            .events
            .send(protocol::ServerMessage::Transactions(Box::new(view)));
    }

    async fn susun_positions(&self) -> protocol::PositionsView {
        let waktu = chrono::Utc::now().to_rfc3339();

        let pemilik = {
            let w = self.wallet.lock().await;
            w.address()
        };
        let Some(pemilik) = pemilik else {
            return protocol::PositionsView {
                positions: Vec::new(),
                tertutup: Vec::new(),
                owner: String::new(),
                keluar_range: 0,
                ringkasan: crate::pnl::ringkas(&[]),
                learning: crate::learning::view(&self.store, &[], &[]).unwrap_or_default(),
                error: Some("wallet belum dibuat".into()),
                dimuat_pada: waktu,
            };
        };

        let provider = match self.chain.provider().await {
            Ok(p) => p,
            Err(e) => {
                let (tertutup, histori_error) = match crate::pnl::histori_tertutup(
                    &self.store,
                    self.registry.chain.id,
                    pemilik,
                ) {
                    Ok(items) => (items, None),
                    Err(_) => (Vec::new(), Some("histori tertutup tidak terbaca")),
                };
                let hasil: Vec<_> = tertutup.iter().filter_map(|p| p.pnl.clone()).collect();
                let _ =
                    crate::learning::synchronize(&self.store, &[], &tertutup, chrono::Utc::now());
                let learning =
                    crate::learning::view(&self.store, &[], &tertutup).unwrap_or_default();
                return protocol::PositionsView {
                    positions: Vec::new(),
                    tertutup,
                    owner: pemilik.to_string(),
                    keluar_range: 0,
                    ringkasan: crate::pnl::ringkas(&hasil),
                    learning,
                    error: Some(match histori_error {
                        Some(histori) => format!("RPC tidak bisa dipakai: {e}; {histori}"),
                        None => format!("RPC tidak bisa dipakai: {e}"),
                    }),
                    dimuat_pada: waktu,
                };
            }
        };

        let snapshot = crate::positions::muat::muat_snapshot(
            &provider,
            &self.registry,
            Some(&self.store),
            pemilik,
        )
        .await;

        let posisi = snapshot.positions;
        let mut errors = snapshot.errors;
        let tertutup =
            match crate::pnl::histori_tertutup(&self.store, self.registry.chain.id, pemilik) {
                Ok(items) => items,
                Err(_) => {
                    errors.push("histori tertutup tidak terbaca".into());
                    Vec::new()
                }
            };

        if let Err(error) =
            crate::learning::synchronize(&self.store, &posisi, &tertutup, chrono::Utc::now())
        {
            errors.push(format!("self improvement tidak tersinkron: {error}"));
        }

        let keluar = posisi.iter().filter(|p| !p.in_range && !p.kosong).count();
        let hasil: Vec<crate::pnl::Hasil> = posisi
            .iter()
            .filter_map(|p| p.pnl.clone())
            .chain(tertutup.iter().filter_map(|p| p.pnl.clone()))
            .collect();
        let ringkasan = crate::pnl::ringkas(&hasil);
        let learning = match crate::learning::view(&self.store, &posisi, &tertutup) {
            Ok(view) => view,
            Err(error) => {
                errors.push(format!("versi kalibrasi tidak terbaca: {error}"));
                crate::learning::LearningView::default()
            }
        };

        protocol::PositionsView {
            positions: posisi,
            tertutup,
            owner: pemilik.to_string(),
            keluar_range: keluar,
            ringkasan,
            learning,
            error: (!errors.is_empty()).then(|| errors.join("; ")),
            dimuat_pada: waktu,
        }
    }

    /// Bandingkan keadaan range sekarang dengan sebelumnya, lalu beri tahu.
    ///
    /// KENAPA PERBANDINGAN, BUKAN KEADAAN
    /// Memberi tahu setiap kali posisi berada di luar range akan membanjiri
    /// layar setiap kali daftar dimuat ulang. Yang layak diberitahukan adalah
    /// PERUBAHANNYA: baru saja keluar, atau baru saja kembali masuk.
    async fn periksa_perubahan_range(&self, sekarang: &[crate::positions::Posisi]) {
        let sebelum = self.positions.read().await.clone();
        if sebelum.is_empty() {
            // Pemuatan pertama bukan perubahan. Memberi tahu di sini berarti
            // membanjiri layar setiap kali engine baru dijalankan.
            return;
        }

        let lama: std::collections::HashMap<&str, bool> = sebelum
            .iter()
            .filter(|p| p.tick_sekarang.is_some())
            .map(|p| (p.position_id.as_str(), p.in_range))
            .collect();

        for p in sekarang {
            if p.kosong || p.tick_sekarang.is_none() {
                continue;
            }
            let Some(dulu) = lama.get(p.position_id.as_str()) else {
                continue;
            };
            if *dulu == p.in_range {
                continue;
            }

            if p.in_range {
                self.beri_tahu_subjek(
                    protocol::AlertLevel::Info,
                    "position_back_in_range",
                    format!("Posisi #{} kembali masuk range", p.token_id),
                    Some(p.position_id.clone()),
                )
                .await;
            } else {
                self.beri_tahu_subjek(
                    protocol::AlertLevel::Critical,
                    "position_out_of_range",
                    format!(
                        "Posisi #{} keluar range dan berhenti menghasilkan fee",
                        p.token_id
                    ),
                    Some(p.position_id.clone()),
                )
                .await;
            }
        }
    }

    /// Jalankan swap zap dari rencana terakhir.
    ///
    /// Rencananya TIDAK dikeluarkan dari slot, karena setelah swap user
    /// masih perlu menyusun ulang rencana dengan saldo yang sudah berubah.
    /// Yang dikosongkan hanya calldata zapnya, supaya swap yang sama tidak
    /// bisa dijalankan dua kali.
    pub async fn on_swap_zap(&self, plan_id: String) {
        let _flow = self.flow_lock.lock().await;
        match self.kirim_zap(plan_id).await {
            Ok((view, lanjut)) => {
                let _ = self
                    .events
                    .send(protocol::ServerMessage::SendResult(Box::new(view)));
                if let Some(task) = lanjut {
                    self.on_quote_range(task).await;
                }
            }
            Err(pesan) => self.alert_kritis("zap_failed", pesan),
        }
    }

    async fn kirim_zap(
        &self,
        plan_id: String,
    ) -> Result<(execute::SendResultView, Option<execute::QuoteTask>), String> {
        let _eksekusi = self.execution_lock.lock().await;
        self.ensure_execution().await?;
        let mut rencana = {
            let mut guard = self.pending_plan.write().await;
            let r = guard.as_mut().ok_or("belum ada rencana yang disusun")?;
            r.periksa(&plan_id, &self.intent)?;
            self.periksa_hook_rencana(r).await?;
            r.periksa(&plan_id, &self.intent)?;
            if r.plan.gates.iter().any(|g| g.id == "G6" && !g.passed) {
                return Err("pembatas G6 belum lolos".into());
            }
            if r.zap_calldata.is_none() {
                return Err("rencana ini tidak memerlukan zap".into());
            }
            guard.take().ok_or("rencana zap hilang")?
        };
        let (calldata, value) = rencana
            .zap_calldata
            .take()
            .ok_or("calldata zap tidak tersedia")?;
        let zap = rencana
            .zap_view
            .as_ref()
            .ok_or("rincian zap tidak tersedia")?;

        let provider = self
            .chain
            .provider()
            .await
            .map_err(|e| format!("RPC tidak bisa dipakai: {e}"))?;
        let router = self
            .registry
            .contract("uniswap_v4", "universal_router")
            .map_err(|e| e.to_string())?;

        let mut wallet = self.wallet.lock().await;
        let signer = wallet.signer().map_err(|e| e.to_string())?.clone();
        drop(wallet);

        let mc = self
            .registry
            .contract("common", "multicall3")
            .map_err(|e| e.to_string())?;
        let before = execute::saldo_dua_token(
            &provider,
            mc,
            signer.address(),
            rencana.token0,
            rencana.token1,
        )
        .await?;

        // Jalankan calldata yang persis sama dari wallet yang sama sebelum
        // menyentuh signer. Ini membuktikan allowance, transfer token, hook,
        // batas keluaran, dan jalur router pada state rantai terkini. Estimasi
        // gas di pintu kirim tetap mengulang pemeriksaan ini karena state bisa
        // berubah di antara dua panggilan.
        provider
            .call(
                alloy::rpc::types::TransactionRequest::default()
                    .from(signer.address())
                    .to(router)
                    .value(value)
                    .input(calldata.clone().into()),
            )
            .await
            .map_err(|error| {
                format!(
                    "simulasi zap ditolak: {}",
                    crate::exec::simulate::jelaskan_revert(&error.to_string())
                )
            })?;

        let terkirim = crate::exec::send::send_calldata(
            &provider,
            &self.nonce,
            &signer,
            router,
            calldata,
            value,
            self.registry.chain.id,
            &self.cfg.execution,
            &self.cfg.limits,
            &crate::exec::journal::Journal {
                store: &self.store,
                context: rencana.journal_context(crate::exec::journal::Operation::Zap),
            },
        )
        .await
        .map_err(|e| e.to_string())?;

        let outcome = crate::exec::send::jadikan_hasil(terkirim);
        let mut lanjut = None;
        if outcome.status == crate::exec::send::TxStatus::Sukses
            && self.intent.current(rencana.revision)
        {
            match execute::saldo_dua_token(
                &provider,
                mc,
                signer.address(),
                rencana.token0,
                rencana.token1,
            )
            .await
            {
                Ok(after) => {
                    let input: alloy::primitives::U256 =
                        zap.jumlah.parse().map_err(|_| "jumlah zap tidak valid")?;
                    let cap: alloy::primitives::U256 = zap
                        .perkiraan_diterima
                        .parse()
                        .map_err(|_| "kutipan zap tidak valid")?;
                    let source0 = zap.dari.eq_ignore_ascii_case(&rencana.token0.to_string());
                    // Anggaran berikutnya hanya sisa input dan hasil swap yang
                    // diterima. Saldo tujuan yang sudah ada tidak ikut dipakai.
                    let budget = if source0 {
                        (
                            rencana.budget.0.saturating_sub(input),
                            after.1.saturating_sub(before.1).min(cap),
                        )
                    } else {
                        (
                            after.0.saturating_sub(before.0).min(cap),
                            rencana.budget.1.saturating_sub(input),
                        )
                    };
                    if let Some(revision) = self.intent.advance(rencana.revision) {
                        rencana.quote.revision = revision;
                        rencana.quote.budget_override = Some(budget);
                        rencana.quote.deposit.zap = false;
                        lanjut = Some(rencana.quote);
                    }
                }
                Err(e) => self.alert_kritis(
                    "zap_balance_unknown",
                    format!("swap berhasil, saldo hasil belum terbaca: {e}"),
                ),
            }
        }
        Ok((
            execute::SendResultView {
                kind: "zap",
                outcome,
            },
            lanjut,
        ))
    }

    /// Setujui rencana terakhir dan kirim transaksinya.
    ///
    /// Rencana dikeluarkan dari slot SEBELUM dikirim, apa pun hasilnya. Rencana
    /// yang tetap tersimpan setelah dikirim bisa disetujui dua kali, dan dua
    /// persetujuan berarti dua posisi beserta dua kali biayanya.
    pub async fn on_open_position(&self, plan_id: String) {
        let _flow = self.flow_lock.lock().await;
        match self.kirim_open(plan_id).await {
            Ok(view) => {
                let terverifikasi = view.outcome.fully_verified();
                let _ = self
                    .events
                    .send(protocol::ServerMessage::SendResult(Box::new(view)));
                if !terverifikasi {
                    self.alert_kritis(
                        "open_unverified",
                        "transaksi terkirim tapi posisinya belum terbukti sama dengan yang \
                         diminta. Periksa hasilnya sebelum membuka posisi lagi"
                            .into(),
                    );
                }
            }
            Err(pesan) => self.alert_kritis("open_failed", pesan),
        }
    }

    async fn kirim_open(&self, plan_id: String) -> Result<execute::SendResultView, String> {
        let _eksekusi = self.execution_lock.lock().await;
        self.ensure_execution().await?;
        let tersimpan = {
            let mut pending = self.pending_plan.write().await;
            let r = pending.as_ref().ok_or("belum ada rencana yang disusun")?;
            r.periksa(&plan_id, &self.intent)?;
            self.periksa_hook_rencana(r).await?;
            r.periksa(&plan_id, &self.intent)?;
            if r.zap_calldata.is_some() {
                return Err("jalankan zap lalu susun ulang rencana mint".into());
            }
            pending.take().ok_or("rencana hilang")?
        };

        let provider = self
            .chain
            .provider()
            .await
            .map_err(|e| format!("RPC tidak bisa dipakai: {e}"))?;
        let alamat = crate::exec::plan::AlamatEksekusi::dari_registry(&self.registry)
            .map_err(|e| e.to_string())?;
        let position_manager = alamat.position_manager(tersimpan.versi);

        let mut wallet = self.wallet.lock().await;
        let signer = wallet.signer().map_err(|e| e.to_string())?.clone();
        drop(wallet);

        // Nilai ETH hanya ikut kalau salah satu currency V4 adalah ETH asli.
        // Posisi V3 selalu memakai WETH, jadi nilainya nol. Mengirim ETH ke
        // pool yang tidak memakainya membuatnya tersangkut di kontrak.
        let value = tersimpan
            .plan
            .v4_request
            .as_ref()
            .map(|r| r.value())
            .unwrap_or(alloy::primitives::U256::ZERO);

        let outcome = crate::exec::send::send_open(
            &provider,
            &self.nonce,
            &signer,
            position_manager,
            alamat.multicall,
            &tersimpan.plan,
            self.registry.chain.id,
            value,
            &self.cfg.execution,
            &self.cfg.limits,
            &crate::exec::journal::Journal {
                store: &self.store,
                context: tersimpan.journal_context(crate::exec::journal::Operation::Open),
            },
        )
        .await
        .map_err(|e| e.to_string())?;

        // Receipt menjadi sumber jumlah aktual. Catatan tetap belum lengkap
        // sampai biaya approval/zap serta perubahan posisi ikut direkonsiliasi.
        if let Some(token_id) = outcome.token_id.as_ref() {
            let position_id = crate::positions::position_id(
                self.registry.chain.id,
                position_manager,
                token_id
                    .parse()
                    .map_err(|_| "token ID receipt tidak valid")?,
            );
            crate::exec::journal::link_position(
                &self.store,
                self.registry.chain.id,
                signer.address(),
                outcome
                    .tx_hash
                    .parse()
                    .map_err(|_| "hash transaksi open tidak valid")?,
                &position_id,
            )
            .map_err(|e| format!("posisi tidak dapat diikat ke jurnal: {e}"))?;
            let flow_costs = crate::exec::journal::flow_costs(
                &self.store,
                self.registry.chain.id,
                signer.address(),
                &tersimpan.quote.request_id,
            )
            .map_err(|e| format!("biaya flow tidak terbaca: {e}"))?;
            let currency = crate::positions::muat::kutipan_pool(
                &self.registry,
                &tersimpan.token0.to_string(),
                &tersimpan.token1.to_string(),
            )
            .map(|(_, _, currency)| currency);
            let gas_kutipan = currency
                .filter(|c| c.symbol == "ETH" || c.symbol == "WETH")
                .filter(|_| flow_costs.complete)
                .and_then(|_| flow_costs.gas_wei.to_string().parse::<f64>().ok())
                .map(|wei| wei / 1e18);
            crate::pnl::simpan(
                &self.store,
                &crate::pnl::Catatan {
                    position_id: position_id.clone(),
                    token_id: token_id.clone(),
                    flow_id: Some(tersimpan.quote.request_id.clone()),
                    setor0: outcome
                        .minted_amounts
                        .as_ref()
                        .map(|a| a.amount0.clone())
                        .unwrap_or_else(|| tersimpan.plan.amount0.clone()),
                    setor1: outcome
                        .minted_amounts
                        .as_ref()
                        .map(|a| a.amount1.clone())
                        .unwrap_or_else(|| tersimpan.plan.amount1.clone()),
                    harga_buka: Some(tersimpan.harga_buka),
                    lengkap: false,
                    setoran_aktual: outcome.minted_amounts.is_some(),
                    gas_wei: (flow_costs.gas_wei > alloy::primitives::U256::ZERO)
                        .then(|| flow_costs.gas_wei.to_string()),
                    gas_kutipan,
                    slippage_kutipan: tersimpan.quote.budget_override.is_none().then_some(0.0),
                    dibuka_pada: outcome.sent_at.clone(),
                    tx_buka: Some(outcome.tx_hash.clone()),
                    rekonstruksi_log: None,
                    rekonstruksi_v4: None,
                },
            );
            if outcome.fully_verified() {
                if let Err(error) = crate::learning::record_open(
                    &self.store,
                    &position_id,
                    &outcome.sent_at,
                    tersimpan.learning.clone(),
                ) {
                    // Posisi sudah aman dibuka dan tidak boleh dianggap gagal
                    // hanya karena catatan analitik lokal bermasalah.
                    tracing::warn!(%position_id, error = %error, "vektor fitur posisi gagal disimpan");
                }
            }
        }

        Ok(execute::SendResultView {
            kind: "open",
            outcome,
        })
    }

    /// Kabar biasa yang bukan kegagalan.
    ///
    /// Terpisah dari `alert_kritis` supaya tingkatnya benar. Persetujuan hook
    /// yang berhasil dicatat bukan kegagalan, dan menampilkannya dengan warna
    /// kegagalan akan mengajari user mengabaikan warna itu.
    fn alert_info(&self, code: &str, message: String) {
        tracing::info!(code, %message, "kabar untuk panel");
        let _ = self.events.send(protocol::ServerMessage::Alert {
            level: protocol::AlertLevel::Info,
            code: code.to_string(),
            message,
        });
    }

    fn alert_kritis(&self, code: &str, message: String) {
        tracing::warn!(code, %message, "eksekusi tidak dilanjutkan");
        let _ = self.events.send(protocol::ServerMessage::Alert {
            level: protocol::AlertLevel::Critical,
            code: code.to_string(),
            message,
        });
    }

    /// Pasang range selebar yang diizinkan protokol.
    ///
    /// Batasnya dihitung dari tick terjauh yang masih kelipatan spacing, lalu
    /// diubah menjadi market cap supaya sisa alurnya sama persis dengan range
    /// biasa. Tidak ada jalur eksekusi terpisah untuk range penuh, dan itu
    /// disengaja: jalur yang jarang dipakai adalah jalur yang jarang teruji.
    /// Baca saldo kedua sisi pasangan untuk wallet yang sedang dibuka.
    ///
    /// Mengembalikan `None` kalau wallet belum dibuka. Itu berbeda dari saldo
    /// nol: yang pertama berarti panel harus menyuruh membuka wallet, yang
    /// kedua berarti user memang belum punya tokennya.
    async fn baca_saldo<P: alloy::providers::Provider>(
        &self,
        provider: &P,
        meta: &Option<crate::pools::TokenMeta>,
        kelompok: &crate::pools::QuoteGroup,
    ) -> Option<protocol::SaldoPasangan> {
        use alloy::primitives::Address;

        let pemilik = self.wallet.lock().await.address()?;
        let meta = meta.as_ref()?;

        let (Ok(a), Ok(b)) = (
            meta.address.parse::<Address>(),
            kelompok.quote.parse::<Address>(),
        ) else {
            return None;
        };

        let (sa, sb) = crate::pools::saldo_dua(provider, &self.registry, pemilik, a, b).await;

        // Saldo gas dibaca terpisah karena ia bukan ERC20. Kegagalannya tidak
        // menggagalkan yang lain: user yang punya token tapi tidak punya gas
        // tetap perlu melihat tokennya.
        let gas = provider
            .get_balance(pemilik)
            .await
            .ok()
            .map(|v| f64::from(v) / 1e18)
            .unwrap_or(0.0);

        Some(protocol::SaldoPasangan {
            memecoin: sa.map(|v| f64::from(v) / 10f64.powi(meta.decimals as i32))?,
            quote: sb.map(|v| f64::from(v) / 10f64.powi(kelompok.quote_decimals as i32))?,
            gas,
        })
    }

    pub async fn on_set_full_range(&self) {
        let batas = {
            let guard = self.active.read().await;
            let Some(active) = guard.as_ref() else { return };
            let Some(pool) = active.selected_pool() else {
                return;
            };
            let Some(mcap) = active.chart_mcap_usd else {
                return;
            };

            let Some(batas) = crate::range::full_bounds(pool, &active.meta.address, mcap) else {
                return;
            };
            batas
        };

        self.on_set_range(batas.0, batas.1).await;
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
            self.intent.change();
        }
        self.broadcast_range_view().await;
    }

    /// Benar kalau kutipan yang sedang aktif adalah ETH terbungkus.
    ///
    /// Dipakai memutuskan apakah pemeriksaan rentang kurs berlaku. Pada pool
    /// berpasangan stablecoin, harga satu kutipan memang kira kira satu dolar,
    /// dan memeriksanya terhadap rentang harga ETH akan memunculkan peringatan
    /// pada setiap pool yang sebenarnya sehat.
    fn quote_adalah_eth(&self, quote: &str) -> bool {
        if quote
            .parse::<alloy::primitives::Address>()
            .is_ok_and(|a| a.is_zero())
        {
            return true;
        }
        match self.registry.tokens.get("weth") {
            Some(w) => w.address.eq_ignore_ascii_case(quote),
            None => false,
        }
    }

    async fn broadcast_range_view(&self) {
        let guard = self.active.read().await;
        let Some(active) = guard.as_ref() else { return };
        let Some(pool) = active.selected_pool() else {
            return;
        };
        let Some(mcap) = active.chart_mcap_usd else {
            return;
        };

        match crate::range::build_view(
            pool,
            &active.meta,
            active.quote_decimals,
            mcap,
            active.requested,
            self.quote_adalah_eth(&active.selected_quote),
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

    pub async fn registry_summary(&self) -> RegistrySummary {
        use crate::chain::RegistryStatus::*;
        let checks = self.registry_checks.read().await;
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

    // KENAPA KETERISIAN STATISTIK IKUT DIHITUNG
    // Tahap cepat memakai kunci pool dari cache dan tidak mengukur apa apa,
    // sehingga daftar poolnya sering sama persis dengan tahap penuh. Tanpa
    // suku ini, keduanya dianggap identik, kiriman kedua ditahan, dan APR,
    // TVL, serta volume tidak pernah sampai ke panel meski sudah terhitung.
    let ada_stats = state.pools.iter().any(|p| p.stats.is_some());

    format!(
        "{}|{}|{}|{}",
        state.selected_quote.as_deref().unwrap_or(""),
        state.selected_pool_id.as_deref().unwrap_or(""),
        ada_stats,
        bagian.join(",")
    )
}
