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

pub mod execute;
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

    /// Susun rencana membuka posisi, jalankan seluruh gate, TIDAK mengirim.
    ///
    /// Seluruh angka datang dari keadaan yang sedang aktif: pool terpilih,
    /// range yang sedang digambar, dan saldo wallet. Panel hanya menyebut
    /// nominalnya, karena itu satu satunya hal yang memang keputusan user.
    pub async fn on_quote_range(&self, size_usd: f64) {
        match self.susun_rencana(size_usd).await {
            Ok(view) => {
                let _ = self
                    .events
                    .send(protocol::ServerMessage::OpenPlan(Box::new(view)));
            }
            Err(pesan) => self.alert_kritis("plan_failed", pesan),
        }
    }

    async fn susun_rencana(&self, size_usd: f64) -> Result<execute::OpenPlanView, String> {
        if !(size_usd.is_finite() && size_usd > 0.0) {
            return Err("ukuran posisi harus angka positif".into());
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

        let (pool, meta, quote_decimals, requested, mcap) = {
            let guard = self.active.read().await;
            let active = guard.as_ref().ok_or("belum ada token yang dimuat")?;
            let pool = active
                .selected_pool()
                .ok_or("pool terpilih tidak ada di daftar")?
                .clone();
            let mcap = active
                .chart_mcap_usd
                .ok_or("harga dari chart gmgn belum masuk, kurs belum bisa dikalibrasi")?;
            (
                pool,
                active.meta.clone(),
                active.quote_decimals,
                active.requested,
                mcap,
            )
        };

        // Range yang dipakai persis yang sedang tergambar di chart, termasuk
        // pembulatan ke grid tick. Menghitungnya ulang di sini dengan cara lain
        // akan membuat yang dikirim berbeda dari yang dilihat user.
        let view = crate::range::build_view(&pool, &meta, quote_decimals, mcap, requested)
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

        // Nominal diubah menjadi jumlah token memecoin, lalu dibatasi saldo.
        // Sisi lawannya dibiarkan sebesar saldo, karena berapa yang benar benar
        // terpakai ditentukan matematika likuiditas, bukan oleh kita.
        let harga_token_usd = view.current.price_usd;
        let memecoin_adalah_token0 = pool.token0.eq_ignore_ascii_case(&meta.address);
        let desimal_memecoin = meta.decimals;

        let diminta = execute::usd_ke_satuan_terkecil(size_usd, harga_token_usd, desimal_memecoin)
            .ok_or("nominal posisi tidak masuk akal untuk harga token ini")?;

        let (mut available0, mut available1) = if memecoin_adalah_token0 {
            (diminta.min(saldo0), saldo1)
        } else {
            (saldo0, diminta.min(saldo1))
        };

        // ETH ASLI TIDAK BOLEH DIPAKAI SAMPAI HABIS
        //
        // Di V4, currency nol adalah ETH asli, dan ia juga yang membayar gas.
        // Menawarkan seluruh saldo sebagai modal berarti posisi yang berhasil
        // dibuka tapi tidak bisa ditutup, karena tidak ada gas tersisa.
        // Cadangannya diambil dari konfigurasi yang sama dengan gate G6,
        // supaya tidak ada dua angka yang bisa menyimpang.
        let cadangan_wei = alloy::primitives::U256::from(
            (self.cfg.limits.min_eth_reserve.max(0.0) * 1e18) as u128,
        );
        if token0.is_zero() {
            available0 = available0.saturating_sub(cadangan_wei);
        }
        if token1.is_zero() {
            available1 = available1.saturating_sub(cadangan_wei);
        }

        if available0.is_zero() && available1.is_zero() {
            return Err("saldo kedua token nol, tidak ada yang bisa disetor".into());
        }

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
            eth_balance_f64,
            // Biaya gas ditambah ETH yang benar benar masuk ke posisi. Untuk
            // pool yang kedua sisinya token, yang kedua nol. Untuk pool
            // berpasangan ETH asli, mengabaikannya membuat gate G6 mengira
            // cadangan masih utuh padahal modalnya diambil dari sana juga.
            PERKIRAAN_GAS_ETH + eth_komitmen,
            DEADLINE_DETIK,
        );

        let plan = crate::exec::plan::plan_open_auto(
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
        let zap = self
            .hitung_zap(
                &provider,
                &alamat,
                &pool,
                tick_lower,
                tick_upper,
                available0,
                available1,
                amount0,
                amount1,
            )
            .await;

        let allowances = crate::exec::plan::izin_yang_kurang(
            &provider,
            &alamat,
            pool.version,
            pemilik,
            token0,
            amount0,
            token1,
            amount1,
            chrono::Utc::now().timestamp().max(0) as u64,
        )
        .await?;
        let disusun_pada = chrono::Utc::now();
        let expires_at = (disusun_pada
            + chrono::Duration::seconds(execute::RENCANA_KEDALUWARSA_DETIK))
        .to_rfc3339();

        *self.pending_plan.write().await = Some(execute::RencanaTersimpan {
            plan: plan.clone(),
            zap_calldata: zap.as_ref().map(|(_, c, v)| (c.clone(), *v)),
            versi: pool.version,
            token0,
            token1,
            amount0,
            amount1,
            disusun_pada,
        });

        // Swap zap berjalan lewat UniversalRouter, yang menarik token lewat
        // Permit2 juga. Izin ke PositionManager tidak berlaku untuknya.
        let mut allowances = allowances;
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
                    Err(e) => tracing::warn!(error = %e, "izin router untuk zap tidak terbaca"),
                }
            }
        }

        Ok(execute::OpenPlanView {
            plan,
            allowances,
            zap: zap.as_ref().map(|(v, _, _)| v.clone()),
            version: match pool.version {
                crate::pools::PoolVersion::V3 => "v3",
                crate::pools::PoolVersion::V4 => "v4",
            },
            expires_at,
        })
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
    ) -> Option<(execute::ZapView, alloy::primitives::Bytes, alloy::primitives::U256)> {
        use alloy::primitives::U256;
        use crate::exec::zap::{self, SisiSumber};

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

        let minimum = crate::exec::limits::apply_slippage(diterima, self.cfg.limits.max_slippage_bps);
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
    pub async fn on_approve_token(&self, token: String) {
        match self.kirim_approve(token).await {
            Ok(view) => {
                let _ = self
                    .events
                    .send(protocol::ServerMessage::SendResult(Box::new(view)));
            }
            Err(pesan) => self.alert_kritis("approve_failed", pesan),
        }
    }

    async fn kirim_approve(&self, token: String) -> Result<execute::SendResultView, String> {
        let target: alloy::primitives::Address = token
            .parse()
            .map_err(|_| format!("`{token}` bukan alamat EVM yang valid"))?;

        let (jumlah, versi) = {
            let guard = self.pending_plan.read().await;
            let r = guard.as_ref().ok_or("belum ada rencana yang disusun")?;
            if r.kedaluwarsa() {
                return Err("rencana sudah kedaluwarsa, susun ulang".into());
            }
            if target == r.token0 {
                (r.amount0, r.versi)
            } else if target == r.token1 {
                (r.amount1, r.versi)
            } else {
                return Err(format!("{token} bukan salah satu token rencana ini"));
            }
        };

        if jumlah.is_zero() {
            return Err("jumlah yang dibutuhkan nol, izin tidak perlu diberikan".into());
        }

        let provider = self
            .chain
            .provider()
            .await
            .map_err(|e| format!("RPC tidak bisa dipakai: {e}"))?;
        let alamat = crate::exec::plan::AlamatEksekusi::dari_registry(&self.registry)
            .map_err(|e| e.to_string())?;

        let mut wallet = self.wallet.lock().await;
        let signer = wallet.signer().map_err(|e| e.to_string())?.clone();
        drop(wallet);

        let sekarang = chrono::Utc::now().timestamp().max(0) as u64;
        let outcome = match versi {
            crate::pools::PoolVersion::V3 => crate::exec::send::send_approve(
                &provider,
                &self.nonce,
                &signer,
                alamat.multicall,
                target,
                alamat.position_manager_v3,
                jumlah,
                self.registry.chain.id,
            )
            .await
            .map_err(|e| e.to_string())?,

            crate::pools::PoolVersion::V4 => {
                // V4 butuh dua lapis izin, dan urutannya penting: izin Permit2
                // tidak berguna sebelum token mengizinkan Permit2 lebih dulu.
                // Engine mengerjakan langkah PERTAMA yang masih kurang, jadi
                // panel cukup menekan tombolnya lagi untuk langkah berikutnya.
                let kurang = crate::exec::plan::izin_yang_kurang(
                    &provider,
                    &alamat,
                    crate::pools::PoolVersion::V4,
                    signer.address(),
                    target,
                    jumlah,
                    target,
                    alloy::primitives::U256::ZERO,
                    sekarang,
                )
                .await?;

                let langkah = kurang
                    .first()
                    .ok_or("izin untuk token ini sudah cukup")?;

                match langkah.jenis {
                    "erc20_ke_permit2" => crate::exec::send::send_approve(
                        &provider,
                        &self.nonce,
                        &signer,
                        alamat.multicall,
                        target,
                        alamat.permit2,
                        jumlah,
                        self.registry.chain.id,
                    )
                    .await
                    .map_err(|e| e.to_string())?,

                    _ => {
                        let calldata = crate::exec::v4::permit::permit2_approve_calldata(
                            target,
                            alamat.position_manager_v4,
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
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                        crate::exec::send::jadikan_hasil(terkirim)
                    }
                }
            }
        };

        Ok(execute::SendResultView {
            kind: "approve",
            outcome,
        })
    }
    /// Jalankan swap zap dari rencana terakhir.
    ///
    /// Rencananya TIDAK dikeluarkan dari slot, karena setelah swap user
    /// masih perlu menyusun ulang rencana dengan saldo yang sudah berubah.
    /// Yang dikosongkan hanya calldata zapnya, supaya swap yang sama tidak
    /// bisa dijalankan dua kali.
    pub async fn on_swap_zap(&self) {
        match self.kirim_zap().await {
            Ok(view) => {
                let _ = self
                    .events
                    .send(protocol::ServerMessage::SendResult(Box::new(view)));
            }
            Err(pesan) => self.alert_kritis("zap_failed", pesan),
        }
    }

    async fn kirim_zap(&self) -> Result<execute::SendResultView, String> {
        let (calldata, value) = {
            let mut guard = self.pending_plan.write().await;
            let r = guard.as_mut().ok_or("belum ada rencana yang disusun")?;
            if r.kedaluwarsa() {
                return Err("rencana sudah kedaluwarsa, susun ulang".into());
            }
            r.zap_calldata
                .take()
                .ok_or("rencana ini tidak memerlukan zap")?
        };

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

        let terkirim = crate::exec::send::send_calldata(
            &provider,
            &self.nonce,
            &signer,
            router,
            calldata,
            value,
            self.registry.chain.id,
        )
        .await
        .map_err(|e| e.to_string())?;

        Ok(execute::SendResultView {
            kind: "zap",
            outcome: crate::exec::send::jadikan_hasil(terkirim),
        })
    }

    /// Setujui rencana terakhir dan kirim transaksinya.
    ///
    /// Rencana dikeluarkan dari slot SEBELUM dikirim, apa pun hasilnya. Rencana
    /// yang tetap tersimpan setelah dikirim bisa disetujui dua kali, dan dua
    /// persetujuan berarti dua posisi beserta dua kali biayanya.
    pub async fn on_open_position(&self) {
        match self.kirim_open().await {
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

    async fn kirim_open(&self) -> Result<execute::SendResultView, String> {
        let tersimpan = self
            .pending_plan
            .write()
            .await
            .take()
            .ok_or("belum ada rencana yang disusun")?;

        if tersimpan.kedaluwarsa() {
            return Err("rencana sudah kedaluwarsa, susun ulang".into());
        }

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
        )
        .await
        .map_err(|e| e.to_string())?;

        Ok(execute::SendResultView {
            kind: "open",
            outcome,
        })
    }

    fn alert_kritis(&self, code: &str, message: String) {
        tracing::warn!(code, %message, "eksekusi tidak dilanjutkan");
        let _ = self.events.send(protocol::ServerMessage::Alert {
            level: protocol::AlertLevel::Critical,
            code: code.to_string(),
            message,
        });
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
