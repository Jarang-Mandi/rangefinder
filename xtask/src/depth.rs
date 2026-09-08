//! Gambar distribusi likuiditas sebuah pool sebagai histogram di terminal.
//!
//! Membuktikan `pools::distribusi` benar benar bekerja terhadap rantai, dan
//! sekaligus memberi cara memeriksa arah sumbunya. Histogram yang terbalik
//! tetap terlihat masuk akal di layar, jadi ia harus dilihat berdampingan
//! dengan harga sekarang yang sudah diketahui benar dari sumber lain.
//!
//! Cara pakai: `cargo xtask depth <alamat_token>`

use std::path::Path;

use alloy::primitives::Address;
use anyhow::{Context, Result};

use rangefinder_engine::chain::registry::Registry;
use rangefinder_engine::chain::ChainClient;
use rangefinder_engine::config::Config;
use rangefinder_engine::pools::{self, distribusi};

fn ringkas(v: f64) -> String {
    let a = v.abs();
    if a >= 1e9 {
        format!("{:.2}B", v / 1e9)
    } else if a >= 1e6 {
        format!("{:.2}M", v / 1e6)
    } else if a >= 1e3 {
        format!("{:.1}K", v / 1e3)
    } else {
        format!("{v:.2}")
    }
}

pub async fn run(repo_root: &Path, token_arg: &str) -> Result<bool> {
    println!("  Distribusi likuiditas");
    println!("  ---------------------");

    let registry = Registry::load_from(&Registry::default_path(repo_root))?;
    let cfg = Config::load()?;
    let client = ChainClient::new(cfg.rpc.clone());
    let provider = client.provider().await?;

    let token: Address = token_arg
        .parse()
        .with_context(|| format!("`{token_arg}` bukan alamat EVM"))?;

    let snapshot =
        pools::muat_token(&provider, &registry, None, token, pools::ModePencarian::Penuh).await?;

    let Some(kelompok) = snapshot.quotes.first() else {
        println!("  Tidak ada pool layak untuk token ini.");
        return Ok(false);
    };
    let Some(pool) = kelompok.pools.first() else {
        println!("  Kelompok kutipan kosong.");
        return Ok(false);
    };

    let meme = snapshot.meta.as_ref();
    let skala_meme = 10f64.powi(meme.map(|m| m.decimals).unwrap_or(18) as i32);
    let skala_quote = 10f64.powi(kelompok.quote_decimals as i32);

    // Memecoin ada di token0 berarti kutipannya token1.
    let quote_adalah_token1 = pool.token0.eq_ignore_ascii_case(&token.to_string());

    println!(
        "  pasangan  {} terhadap {}, pool {} fee {}",
        meme.map(|m| m.symbol.as_str()).unwrap_or("?"),
        kelompok.quote_symbol,
        &pool.id[..12.min(pool.id.len())],
        if pool.dynamic_fee {
            "dinamis".to_string()
        } else {
            format!("{:.2}%", pool.fee as f64 / 10_000.0)
        }
    );

    let state_view = registry.contract("uniswap_v4", "state_view")?;
    let multicall = registry.contract("common", "multicall3")?;

    let jam = std::time::Instant::now();
    let d = distribusi::susun(
        &provider,
        state_view,
        multicall,
        &pool.id,
        distribusi::KeadaanPool {
            tick_spacing: pool.tick_spacing,
            tick_sekarang: pool.tick,
            likuiditas: pool.liquidity.parse().unwrap_or(0),
            quote_adalah_token1,
            skala_meme,
            skala_quote,
        },
    )
    .await?;

    println!(
        "  harga     {:.8} {} per {}, tick {}, dibaca dalam {} ms",
        d.harga_sekarang,
        kelompok.quote_symbol,
        meme.map(|m| m.symbol.as_str()).unwrap_or("?"),
        d.tick_sekarang,
        jam.elapsed().as_millis()
    );

    if let Some(e) = &d.error {
        println!("  {e}");
        return Ok(false);
    }

    println!("  {} pita terbaca", d.pita.len());
    println!();

    let puncak = d
        .pita
        .iter()
        .map(|p| p.nilai_quote)
        .fold(0.0_f64, f64::max);
    if puncak <= 0.0 {
        println!("  Seluruh pita bernilai nol, tidak ada yang bisa digambar.");
        return Ok(false);
    }

    // Diurutkan menurut HARGA, bukan menurut tick, lalu digambar dari yang
    // tertinggi ke terendah supaya sumbunya searah dengan chart. Mengurutkan
    // menurut tick akan menggambar terbalik pada pool yang memecoinnya token1.
    let mut baris: Vec<_> = d.pita.iter().collect();
    baris.sort_by(|a, b| {
        b.harga_bawah
            .partial_cmp(&a.harga_bawah)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    for p in baris {
        let lebar = ((p.nilai_quote / puncak) * 38.0).round() as usize;
        println!(
            "  {:>11.8} sampai {:>11.8} {:<38} {:>9}{}",
            p.harga_bawah,
            p.harga_atas,
            "#".repeat(lebar),
            ringkas(p.nilai_quote),
            if p.aktif { "  <- harga sekarang" } else { "" }
        );
    }

    println!();
    println!(
        "  Tinggi batang adalah modal yang tertanam di pita itu, dalam {}.",
        kelompok.quote_symbol
    );
    println!("  Porsi fee kita adalah modal kita dibagi batang di pita tempat harga bergerak.");

    Ok(!d.pita.is_empty())
}
