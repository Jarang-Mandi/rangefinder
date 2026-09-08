//! Engine lokal Rangefinder.
//!
//! Titik masuk. Urutan start sengaja dibuat begini:
//!
//!   1. konfigurasi   gagal di sini berarti berhenti, tidak ada tebak tebakan
//!   2. token pairing dibuat sekali, ditampilkan sekali
//!   3. registry      dibaca dari chain.toml
//!   4. gate G1       verifikasi bytecode. Mismatch menghentikan engine
//!   5. wallet        dibuka dalam keadaan terkunci
//!   6. server        mengikat loopback
//!
//! INVARIAN
//! - Langkah 4 boleh gagal karena RPC mati, dan itu tidak menghentikan engine,
//!   karena chain ini memang sering tidak stabil. Yang menghentikan engine
//!   hanyalah bytecode yang tidak cocok dengan yang dipin, karena itu berarti
//!   kontraknya bukan yang kita harapkan.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::sync::{broadcast, Mutex};

use rangefinder_engine::api::protocol::AlertLevel;
use rangefinder_engine::api::AppState;
use rangefinder_engine::chain::{registry::Registry, ChainClient};
use rangefinder_engine::config::Config;
use rangefinder_engine::{api, auth, chain, config, store, telemetry, wallet};

#[tokio::main]
async fn main() -> Result<()> {
    telemetry::init();

    let cfg = Config::load()?;
    let data_dir = config::data_dir()?;
    let bind = cfg.bind_addr()?;

    tracing::info!(
        version = api::ENGINE_VERSION,
        data_dir = %data_dir.display(),
        "rangefinder engine start"
    );

    let (token, baru) = auth::PairingToken::load_or_create(&data_dir)?;
    if std::env::args().any(|a| a == "--print-token") {
        println!("{}", token.as_hex());
        return Ok(());
    }
    print_pairing(&token, baru, &bind);

    let registry_path = Registry::default_path(&config::repo_root());
    let registry = Registry::load_from(&registry_path)?;
    tracing::info!(
        chain = %registry.chain.name,
        id = registry.chain.id,
        path = %registry_path.display(),
        "registry kontrak dimuat"
    );

    if registry.chain.id != cfg.rpc.chain_id {
        anyhow::bail!(
            "chain.toml menyebut chain id {} tapi config.toml meminta {}. \
             Keduanya harus sama",
            registry.chain.id,
            cfg.rpc.chain_id
        );
    }

    let chain_client = ChainClient::new(cfg.rpc.clone());

    // Pemeriksaan kesehatan pertama sebelum apa apa, supaya pesan gate G1
    // di bawah bisa membedakan RPC mati dari kontrak yang salah.
    let health = chain_client.check_health().await;
    if health.healthy {
        tracing::info!(
            block = health.block_number.unwrap_or_default(),
            lag = health.block_lag_secs.unwrap_or_default(),
            "RPC sehat"
        );
    } else {
        tracing::warn!(
            error = health.error.as_deref().unwrap_or("tidak diketahui"),
            "RPC tidak sehat saat start, engine tetap jalan tapi eksekusi diblokir"
        );
    }

    let registry_checks = run_registry_gate(&chain_client, &registry).await?;

    let wallet = wallet::Wallet::open(&data_dir, cfg.wallet.auto_lock_minutes)?;
    if wallet.exists() {
        tracing::info!(
            address = %wallet.address().map(|a| a.to_string()).unwrap_or_default(),
            "wallet ditemukan, keadaan terkunci"
        );
    } else {
        tracing::info!("belum ada wallet. Buat lewat side panel sebelum bisa membuka posisi");
    }

    let store = store::Store::open(&data_dir)?;
    let (events, _) = broadcast::channel(256);

    let state = AppState {
        cfg: Arc::new(cfg.clone()),
        auth: Arc::new(token),
        wallet: Arc::new(Mutex::new(wallet)),
        chain: chain_client.clone(),
        registry: Arc::new(registry),
        store: Arc::new(store),
        events,
        started_at: Instant::now(),
        registry_checks: Arc::new(registry_checks),
        active: Arc::new(tokio::sync::RwLock::new(None)),
        pending_plan: Arc::new(tokio::sync::RwLock::new(None)),
        nonce: Arc::new(rangefinder_engine::exec::nonce::NonceKeeper::new()),
        positions: Arc::new(tokio::sync::RwLock::new(Vec::new())),
        pending_action: Arc::new(tokio::sync::RwLock::new(None)),
        bars: Arc::new(tokio::sync::RwLock::new(None)),
        notifier: Arc::new(tokio::sync::RwLock::new(
            rangefinder_engine::notify::Notifier::default(),
        )),
    };

    spawn_health_loop(state.clone());
    spawn_auto_lock_loop(state.clone());

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("gagal mengikat {bind}. Apakah engine lain sudah jalan?"))?;

    tracing::info!(%bind, "server siap");

    axum::serve(listener, api::router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server berhenti dengan error")?;

    tracing::info!("engine berhenti");
    Ok(())
}

/// Jalankan gate G1 dan putuskan apakah engine boleh lanjut.
async fn run_registry_gate(
    chain: &ChainClient,
    registry: &Registry,
) -> Result<Vec<chain::RegistryCheck>> {
    match chain.verify_registry(registry).await {
        Ok(checks) => {
            // Mismatch di sini adalah penghentian, bukan peringatan.
            let report = chain::registry_gate(&checks)?;
            tracing::info!(
                total = report.total,
                terverifikasi = report.verified,
                belum_dipin = report.unpinned,
                tanpa_kode = report.no_code,
                "gate G1 selesai"
            );
            if report.unpinned > 0 {
                tracing::warn!(
                    jumlah = report.unpinned,
                    "ada kontrak yang bytecode-nya belum dipin. \
                     Jalankan `cargo xtask verify --pin` setelah memastikan alamatnya benar"
                );
            }
            if report.no_code > 0 {
                tracing::warn!(
                    jumlah = report.no_code,
                    "ada alamat tanpa bytecode. Periksa chain.toml terhadap explorer"
                );
            }
            Ok(checks)
        }
        Err(e) => {
            // RPC mati bukan alasan menolak start. Chain ini memang sering
            // bermasalah, dan panel tetap berguna untuk melihat status.
            tracing::warn!(
                error = %e,
                "gate G1 tidak bisa dijalankan karena RPC. Engine jalan tanpa verifikasi, \
                 dan eksekusi tetap diblokir sampai RPC sehat"
            );
            Ok(Vec::new())
        }
    }
}

fn spawn_health_loop(state: AppState) {
    let interval = Duration::from_secs(state.cfg.rpc.health_interval_secs.max(1));
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        let mut sehat_sebelumnya: Option<bool> = None;

        loop {
            ticker.tick().await;
            let health = state.chain.check_health().await;

            // Hanya beri tahu saat status berubah, bukan setiap detak, supaya
            // tidak membanjiri panel. Lihat aturan pendinginan di PRD 3.7.
            if sehat_sebelumnya != Some(health.healthy) {
                // Lewat jalur notifikasi, bukan alert langsung, supaya ia
                // ikut pendinginan dan masuk riwayat. RPC yang berkedip
                // sehat dan sakit bergantian akan membanjiri layar tanpa ini.
                if health.healthy {
                    state.beri_tahu(
                        AlertLevel::Info,
                        "rpc_healthy",
                        "RPC kembali sehat".into(),
                    );
                } else {
                    state.beri_tahu(
                        AlertLevel::Critical,
                        "rpc_unhealthy",
                        health
                            .error
                            .clone()
                            .unwrap_or_else(|| "RPC tidak sehat".into()),
                    );
                }
                sehat_sebelumnya = Some(health.healthy);
            }

            state.broadcast_status().await;
        }
    });
}

fn spawn_auto_lock_loop(state: AppState) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(10));
        loop {
            ticker.tick().await;
            let terkunci = state.wallet.lock().await.tick_auto_lock();
            if terkunci {
                state.beri_tahu(
                    AlertLevel::Warning,
                    "wallet_auto_locked",
                    "Wallet terkunci otomatis karena tidak ada aktivitas".into(),
                );
                state.broadcast_status().await;
            }
        }
    });
}

fn print_pairing(token: &auth::PairingToken, baru: bool, bind: &std::net::SocketAddr) {
    let judul = if baru {
        "Token pairing dibuat"
    } else {
        "Token pairing"
    };
    println!();
    println!("  {judul}");
    println!("  {}", token.as_hex());
    println!();
    println!("  Buka side panel Rangefinder, tempel token di atas satu kali.");
    println!("  Engine mendengarkan di http://{bind}");
    if baru {
        println!("  Token disimpan di direktori data dan tidak akan ditampilkan lagi");
        println!("  kecuali dijalankan dengan --print-token.");
    }
    println!();
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("sinyal berhenti diterima");
}
