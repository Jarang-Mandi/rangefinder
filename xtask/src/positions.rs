//! Muat posisi LP sebuah wallet dari rantai.
//!
//! Membuktikan penemuan posisi, pembacaan keadaan, dan perhitungan fee benar
//! benar bekerja terhadap data sungguhan. Tanpa alat ini, satu satunya cara
//! mengujinya adalah lewat panel, dan kegagalan di sana sulit dibaca.
//!
//! Cara pakai: `cargo xtask positions <alamat_wallet>`

use std::path::Path;

use alloy::primitives::Address;
use anyhow::{Context, Result};

use rangefinder_engine::chain::registry::Registry;
use rangefinder_engine::chain::ChainClient;
use rangefinder_engine::config::Config;
use rangefinder_engine::positions::muat;

pub async fn run(repo_root: &Path, alamat_arg: &str) -> Result<bool> {
    println!("  Posisi LP");
    println!("  ---------");

    let registry = Registry::load_from(&Registry::default_path(repo_root))?;
    let cfg = Config::load()?;
    let client = ChainClient::new(cfg.rpc.clone());
    let provider = client.provider().await?;

    let owner: Address = alamat_arg
        .parse()
        .with_context(|| format!("`{alamat_arg}` bukan alamat EVM"))?;
    println!("  wallet    {owner}");
    println!();

    let jam = std::time::Instant::now();
    let posisi = muat::muat_semua(&provider, &registry, None, owner).await;
    println!(
        "  {} posisi, dimuat dalam {} ms",
        posisi.len(),
        jam.elapsed().as_millis()
    );

    if posisi.is_empty() {
        println!();
        println!("  Wallet ini tidak punya posisi LP dalam jendela pemindaian.");
        // Bukan kegagalan. Wallet tanpa posisi adalah keadaan yang sah.
        return Ok(true);
    }

    println!();
    for p in &posisi {
        let status = if p.kosong {
            "kosong"
        } else if p.in_range {
            "dalam "
        } else {
            "KELUAR"
        };
        println!(
            "  {status} {:?} #{:<10} fee {:.2}%  tick {} sampai {}",
            p.version,
            &p.token_id[..10.min(p.token_id.len())],
            p.fee as f64 / 10_000.0,
            p.tick_lower,
            p.tick_upper
        );
        println!(
            "         likuiditas {}  fee belum diklaim {} dan {}",
            p.liquidity,
            p.fee0.as_deref().unwrap_or("belum diketahui"),
            p.fee1.as_deref().unwrap_or("belum diketahui")
        );
        if let Some(t) = p.tick_sekarang {
            println!(
                "         tick sekarang {t}, posisi di {:.0}% lebar range",
                p.porsi_dalam_range() * 100.0
            );
        } else {
            println!("         tick pool tidak terbaca");
        }
        if let Some(h) = &p.pnl {
            let angka = |value: Option<f64>| {
                value
                    .map(|n| format!("{n:.4}"))
                    .unwrap_or_else(|| "belum diketahui".into())
            };
            println!(
                "         hasil ({}) fee {}  IL {:.4}  gas {}  slippage {}  net {}{}",
                h.currency.symbol,
                angka(h.fee),
                h.impermanent_loss,
                angka(h.gas),
                angka(h.slippage),
                angka(h.net),
                if h.perkiraan { "  (perkiraan)" } else { "" }
            );
        }
        if let Some(h) = &p.hooks {
            if h != "0x0000000000000000000000000000000000000000" {
                println!("         hook {h}, periksa allowlist sebelum menambah dana");
            }
        }
    }

    Ok(true)
}
