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

    let meta = pools::token_meta(&provider, token).await?;
    println!("  token   {} ({}), {} desimal", meta.symbol, token, meta.decimals);
    println!();

    let semua = pools::discover_v4_any(&provider, &registry, token).await?;
    println!("  {} pool V4 ditemukan", semua.len());

    let lawan = pools::counterparties(&semua, &token.to_string());
    println!();
    println!("  Berpasangan dengan:");
    for (alamat, jumlah) in &lawan {
        let simbol = match alamat.parse::<Address>() {
            Ok(a) => pools::token_meta(&provider, a)
                .await
                .map(|m| m.symbol)
                .unwrap_or_else(|_| "?".into()),
            Err(_) => "?".into(),
        };
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
