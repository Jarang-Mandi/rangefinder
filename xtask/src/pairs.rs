//! Lihat sebuah token berpasangan dengan apa saja di V4.
//!
//! Dipakai saat pencarian pool biasa tidak menemukan apa apa, karena pencarian
//! itu mengandaikan pasangannya WETH. Perintah ini tidak mengandaikan apa pun.
//!
//! Cara pakai: `cargo xtask pairs <alamat_token>`

use std::path::Path;

use alloy::primitives::Address;
use anyhow::{Context, Result};

use rangefinder_engine::chain::registry::Registry;
use rangefinder_engine::chain::ChainClient;
use rangefinder_engine::config::Config;
use rangefinder_engine::pools;

pub async fn run(repo_root: &Path, token_arg: &str) -> Result<bool> {
    println!("  Pasangan token di V4");
    println!("  --------------------");

    let registry = Registry::load_from(&Registry::default_path(repo_root))?;
    let cfg = Config::load()?;
    let token: Address = token_arg
        .parse()
        .with_context(|| format!("`{token_arg}` bukan alamat EVM"))?;

    // Lewat ChainClient supaya ikut mendapat lapisan percobaan ulang. Tanpa
    // itu, satu balasan 429 langsung menggagalkan seluruh pemindaian.
    let client = ChainClient::new(cfg.rpc.clone());
    let provider = client.provider().await?;

    // Cache yang sama dengan yang dipakai engine, supaya angka waktu di sini
    // mewakili apa yang benar benar dirasakan user di panel.
    let store = rangefinder_engine::store::Store::open(&rangefinder_engine::config::data_dir()?)?;

    let t0 = std::time::Instant::now();
    let meta = pools::token_meta_via(&provider, &registry, token).await?;
    let semua = pools::discover_v4_cached(
        &provider,
        &registry,
        Some(&store),
        token,
        pools::ModePencarian::Penuh,
    )
    .await?;
    println!(
        "  token   {} ({}), {} desimal",
        meta.symbol, token, meta.decimals
    );
    println!();
    println!(
        "  {} pool V4 ditemukan   [{} ms]",
        semua.len(),
        t0.elapsed().as_millis()
    );

    let lawan = pools::counterparties(&semua, &token.to_string());
    let alamat_lawan: Vec<alloy::primitives::Address> = lawan
        .iter()
        .filter_map(|(a, _)| a.parse().ok())
        .collect();
    let t1 = std::time::Instant::now();
    let meta_lawan = pools::token_meta_batch(&provider, &registry, &alamat_lawan).await?;
    println!();
    println!("  Berpasangan dengan:   [{} ms]", t1.elapsed().as_millis());
    for (i, (alamat, jumlah)) in lawan.iter().enumerate() {
        let simbol = meta_lawan
            .get(i)
            .and_then(|m| m.as_ref())
            .map(|m| m.symbol.clone())
            .unwrap_or_else(|| {
                // Alamat nol berarti ETH asli di V4, bukan token yang gagal
                // dibaca. Ia memang tidak punya `symbol()`.
                if alamat.eq_ignore_ascii_case(pools::ZERO_ADDRESS) {
                    "ETH".into()
                } else {
                    "?".into()
                }
            });
        println!("    {alamat}  {simbol:<10} {jumlah} pool");
    }

    // Rincian pembuangan ditampilkan, bukan hanya jumlah yang lolos. Pool fee
    // ekstrem adalah jebakan nyata di rantai ini, dan diam diam menyaringnya
    // membuat orang mengira pool itu tidak pernah ada.
    let kosong = semua
        .iter()
        .filter(|p| p.liquidity.parse::<u128>().unwrap_or(0) == 0)
        .count();
    let fee_gila: Vec<_> = semua
        .iter()
        .filter(|p| p.liquidity.parse::<u128>().unwrap_or(0) > 0 && pools::implausible_fee(p))
        .collect();

    let layak = pools::usable_pools(semua.clone());
    println!();
    println!(
        "  {} dari {} layak, {} tanpa likuiditas, {} berfee di luar akal sehat",
        layak.len(),
        semua.len(),
        kosong,
        fee_gila.len()
    );
    for p in &fee_gila {
        let fee = format!("{:.2}%", p.fee as f64 / 10_000.0);
        println!(
            "    dibuang  fee {fee:<8} spacing {:<5} likuiditas {}",
            p.tick_spacing, p.liquidity
        );
    }
    for p in layak.iter().take(10) {
        let fee = if p.dynamic_fee {
            "dinamis".to_string()
        } else {
            format!("{:.2}%", p.fee as f64 / 10_000.0)
        };
        println!(
            "    fee {fee:<8} spacing {:<5} tick {:<9} likuiditas {}",
            p.tick_spacing, p.tick, p.liquidity
        );
    }

    Ok(!layak.is_empty())
}
