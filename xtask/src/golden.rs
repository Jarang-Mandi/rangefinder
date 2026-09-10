//! Ambil golden vector dari transaksi `mint` sungguhan di Robinhood Chain.
//!
//! Dipisah dari ujinya dengan sengaja. Pengambilan menyentuh jaringan dan
//! dijalankan manual, sedangkan ujinya berjalan offline dan deterministik.
//! Uji yang menyentuh jaringan akan gagal secara acak lalu berhenti dipercaya.
//!
//! Cara pakai: `cargo xtask golden [jumlah]`

use std::path::Path;

use alloy::consensus::Transaction as _;
use alloy::primitives::{Bytes, TxHash};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::Filter;
use alloy::sol_types::SolEvent;
use anyhow::{Context, Result};

use rangefinder_engine::chain::registry::Registry;
use rangefinder_engine::config::Config;
use rangefinder_engine::exec::abi_write::IncreaseLiquidity;
use rangefinder_engine::exec::golden::{
    decode_mint, extract_mint_calldata, fixtures_dir, GoldenVector,
};

/// Berapa blok yang dipindai sekali jalan.
///
/// RPC publik biasanya menolak rentang yang terlalu lebar. Nilai ini diturunkan
/// otomatis saat permintaannya ditolak.
const CHUNK_AWAL: u64 = 50_000;
const CHUNK_MINIMUM: u64 = 2_000;

/// Sejauh mana mundur dari blok terakhir sebelum menyerah.
const MUNDUR_MAKSIMUM: u64 = 3_000_000;

pub async fn capture(repo_root: &Path, target: usize) -> Result<bool> {
    println!("  Golden vector");
    println!("  -------------");

    let registry = Registry::load_from(&Registry::default_path(repo_root))?;
    let cfg = Config::load().context("gagal memuat config untuk URL RPC")?;
    let npm = registry.contract("uniswap_v3", "position_manager")?;

    let url = cfg.rpc.http.first().cloned().context("rpc.http kosong")?;
    let provider = ProviderBuilder::new().connect_http(url.parse()?);

    let latest = provider
        .get_block_number()
        .await
        .context("gagal membaca blok terakhir")?;

    println!("  kontrak  {npm}");
    println!("  blok     {latest}");
    println!();

    let dir = fixtures_dir(repo_root);
    std::fs::create_dir_all(&dir)?;

    let mut terkumpul = 0usize;
    let mut chunk = CHUNK_AWAL;
    let mut hingga = latest;
    let batas_bawah = latest.saturating_sub(MUNDUR_MAKSIMUM);
    let mut sudah_dilihat: Vec<TxHash> = Vec::new();

    while terkumpul < target && hingga > batas_bawah {
        let dari = hingga.saturating_sub(chunk);

        let filter = Filter::new()
            .address(npm)
            .event_signature(IncreaseLiquidity::SIGNATURE_HASH)
            .from_block(dari)
            .to_block(hingga);

        let logs = match provider.get_logs(&filter).await {
            Ok(l) => l,
            Err(e) => {
                // Rentang terlalu lebar. Kecilkan dan coba lagi rentang yang
                // sama, bukan lanjut mundur, supaya tidak ada blok terlewat.
                if chunk > CHUNK_MINIMUM {
                    chunk /= 2;
                    println!("  rentang dikecilkan ke {chunk} blok");
                    continue;
                }
                anyhow::bail!("pemindaian log gagal bahkan pada {chunk} blok: {e}");
            }
        };

        if !logs.is_empty() {
            println!("  {} log pada blok {dari} sampai {hingga}", logs.len());
        }

        for log in logs {
            if terkumpul >= target {
                break;
            }
            let Some(tx_hash) = log.transaction_hash else {
                continue;
            };
            if sudah_dilihat.contains(&tx_hash) {
                continue;
            }
            sudah_dilihat.push(tx_hash);

            match ambil_satu(&provider, tx_hash, &dir).await {
                Ok(true) => {
                    terkumpul += 1;
                    println!("  tersimpan {terkumpul} dari {target}: {tx_hash}");
                }
                Ok(false) => {}
                Err(e) => println!("  lewati {tx_hash}: {e}"),
            }
        }

        hingga = dari.saturating_sub(1);
    }

    println!();
    if terkumpul == 0 {
        println!("  Tidak ada transaksi mint ditemukan dalam {MUNDUR_MAKSIMUM} blok terakhir.");
        println!("  Gate G3 belum bisa diaktifkan. Coba lagi nanti saat ada aktivitas LP.");
        return Ok(false);
    }

    println!("  {terkumpul} golden vector tersimpan di {}", dir.display());
    println!("  Jalankan `cargo test golden` untuk memeriksanya.");
    Ok(true)
}

async fn ambil_satu<P: Provider>(provider: &P, tx_hash: TxHash, dir: &Path) -> Result<bool> {
    let tx = provider
        .get_transaction_by_hash(tx_hash)
        .await?
        .context("transaksi tidak ditemukan")?;

    let input: Bytes = tx.input().clone();
    let Some(mint_calldata) = extract_mint_calldata(&input) else {
        // Bukan mint, kemungkinan increaseLiquidity biasa. Bukan kesalahan.
        return Ok(false);
    };

    let params = decode_mint(&mint_calldata).map_err(|e| anyhow::anyhow!(e))?;

    let vector = GoldenVector {
        source_tx: format!("{tx_hash:#x}"),
        block_number: tx.block_number.unwrap_or_default(),
        captured_at: chrono_now(),
        calldata: format!("0x{}", alloy::hex::encode(&mint_calldata)),
        params,
    };

    let nama = format!("mint-{}.json", &format!("{tx_hash:x}")[..16]);
    let path = dir.join(nama);
    std::fs::write(&path, serde_json::to_string_pretty(&vector)?)?;

    Ok(true)
}

/// Waktu sekarang dalam RFC 3339, tanpa menambah dependensi ke xtask.
fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let detik = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    format!("unix:{detik}")
}
