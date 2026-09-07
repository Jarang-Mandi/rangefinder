//! Jalankan penyelesai zap terhadap pool sungguhan.
//!
//! Ini yang membuktikan tiga hal sekaligus benar benar tersambung: kutipan dari
//! V4Quoter, matematika rasio, dan pencarian titik tukar. Tanpa alat ini,
//! satu satunya cara mengujinya adalah lewat panel, dan kegagalan di sana sulit
//! dibaca.
//!
//! Cara pakai: `cargo xtask zap <alamat_token> [nominal_satuan_terkecil]`

use std::path::Path;

use alloy::primitives::{Address, U256};
use anyhow::{Context, Result};

use rangefinder_engine::chain::registry::Registry;
use rangefinder_engine::chain::ChainClient;
use rangefinder_engine::config::Config;
use rangefinder_engine::exec::v4::pipeline::pool_key_dari;
use rangefinder_engine::exec::v4::quote;
use rangefinder_engine::exec::zap::{self, SisiSumber};
use rangefinder_engine::pools;

/// Lebar range percobaan, dalam kelipatan tick spacing pool.
const LEBAR_SPACING: i32 = 10;

pub async fn run(repo_root: &Path, token_arg: &str, nominal_arg: Option<&str>) -> Result<bool> {
    println!("  Penyelesai zap");
    println!("  --------------");

    let registry = Registry::load_from(&Registry::default_path(repo_root))?;
    let cfg = Config::load()?;
    let client = ChainClient::new(cfg.rpc.clone());
    let provider = client.provider().await?;
    let quoter = registry.contract("uniswap_v4", "quoter")?;

    let token: Address = token_arg
        .parse()
        .with_context(|| format!("`{token_arg}` bukan alamat EVM"))?;

    let snapshot = pools::muat_token(
        &provider,
        &registry,
        None,
        token,
        pools::ModePencarian::Penuh,
    )
    .await?;

    let Some(kelompok) = snapshot.quotes.first() else {
        println!("  Tidak ada pool layak untuk token ini.");
        return Ok(false);
    };
    let Some(pool) = kelompok
        .pools
        .iter()
        .find(|p| p.version == pools::PoolVersion::V4)
        .cloned()
    else {
        println!("  Kelompok teratas tidak punya pool V4. Zap hanya untuk V4 saat ini.");
        return Ok(false);
    };

    println!(
        "  pasangan  {} ({} desimal)",
        kelompok.quote_symbol, kelompok.quote_decimals
    );
    println!(
        "  pool      {} fee {:.2}% spacing {} tick {}",
        &pool.id[..12.min(pool.id.len())],
        pool.fee as f64 / 10_000.0,
        pool.tick_spacing,
        pool.tick
    );

    let key = pool_key_dari(&pool).map_err(anyhow::Error::msg)?;
    let sqrt_p: U256 = pool
        .sqrt_price_x96
        .parse()
        .context("sqrtPriceX96 pool tidak bisa diurai")?;

    // Range mengelilingi harga sekarang, karena zap memang hanya berguna untuk
    // posisi dua sisi.
    let spacing = pool.tick_spacing;
    let tengah = (pool.tick / spacing) * spacing;
    let lebar = spacing * LEBAR_SPACING;
    let (tick_lower, tick_upper) = (tengah - lebar, tengah + lebar);

    let porsi = zap::porsi_token0_bps(sqrt_p, tick_lower, tick_upper).map_err(anyhow::Error::msg)?;
    println!("  range     {tick_lower} sampai {tick_upper}");
    println!(
        "  rasio     token0 {:.1}%, token1 {:.1}%",
        porsi as f64 / 100.0,
        (10_000 - porsi) as f64 / 100.0
    );
    println!();

    // ─── Kutipan mentah, supaya terlihat quoternya menjawab apa ───
    println!("  Kutipan mentah dari V4Quoter");
    let mut ada_jawaban = false;
    for uji in [1_000u64, 100_000, 1_000_000, 100_000_000] {
        let masuk = U256::from(uji);
        match quote::kutip_exact_in(&provider, quoter, &key, true, masuk).await {
            Ok(keluar) => {
                if keluar > U256::ZERO {
                    ada_jawaban = true;
                }
                println!("  {masuk:>12} currency0  ->  {keluar} currency1");
            }
            Err(e) => println!("  {masuk:>12} currency0  ->  gagal: {e}"),
        }
    }

    if !ada_jawaban {
        println!();
        println!("  Quoter menjawab nol untuk seluruh jumlah uji.");
        println!("  Itu bukan jawaban yang bisa dipakai: penyelesai zap akan");
        println!("  menganggapnya sah dan menghasilkan pembagian modal yang salah.");
        return Ok(false);
    }

    // ─── Sapuan, supaya bentuk fungsinya terlihat ─────────────────
    //
    // Likuiditas terhadap jumlah yang ditukar seharusnya naik lalu turun,
    // dengan satu puncak. Kalau bentuknya bukan itu, pencarian bagi dua tidak
    // bisa dipercaya, dan itu harus terlihat di sini bukan di panel.
    println!();
    println!("  Sapuan likuiditas terhadap porsi yang ditukar");
    let modal_sapu = U256::from(1_000_000u64);
    for persen in [10u64, 20, 30, 40, 50, 60, 70, 80, 90] {
        let tukar = modal_sapu * U256::from(persen) / U256::from(100u64);
        let diterima = match quote::kutip_exact_in(&provider, quoter, &key, true, tukar).await {
            Ok(v) => v,
            Err(e) => {
                println!("  {persen:>3}%  gagal: {e}");
                continue;
            }
        };
        let punya0 = modal_sapu - tukar;
        let sqrt_a = rangefinder_engine::math::tick_math::get_sqrt_ratio_at_tick(tick_lower)
            .map_err(anyhow::Error::msg)?;
        let sqrt_b = rangefinder_engine::math::tick_math::get_sqrt_ratio_at_tick(tick_upper)
            .map_err(anyhow::Error::msg)?;
        let l = rangefinder_engine::math::liquidity_math::liquidity_for_amounts(
            sqrt_p, sqrt_a, sqrt_b, punya0, diterima,
        )
        .unwrap_or(0);
        let (pakai0, pakai1) = rangefinder_engine::math::liquidity_math::amounts_for_liquidity(
            sqrt_p, sqrt_a, sqrt_b, l,
        )
        .unwrap_or((U256::ZERO, U256::ZERO));
        println!(
            "  {persen:>3}%  L {l:>22}  sisa0 {:>10}  sisa1 {}",
            punya0.saturating_sub(pakai0),
            diterima.saturating_sub(pakai1)
        );
    }

    // ─── Penyelesaian penuh ───────────────────────────────────────
    let modal = match nominal_arg {
        Some(v) => v
            .parse::<U256>()
            .with_context(|| format!("`{v}` bukan angka"))?,
        None => U256::from(1_000_000u64),
    };

    println!();
    println!("  Penyelesaian dengan modal {modal} pada currency0");

    // Kutipan dikumpulkan lewat penutup yang menyentuh jaringan. Penyelesainya
    // sendiri murni, jadi ia bisa diuji tanpa jaringan sama sekali.
    let hasil = {
        let rt = tokio::runtime::Handle::current();
        zap::selesaikan(sqrt_p, tick_lower, tick_upper, modal, SisiSumber::Token0, |x| {
            let p = &provider;
            let k = key.clone();
            tokio::task::block_in_place(|| {
                rt.block_on(quote::kutip_exact_in(p, quoter, &k, true, x))
            })
        })
    };

    match hasil {
        Ok(r) => {
            println!("  tukar      {} dari {modal}", r.tukar);
            println!("  setor0     {}", r.setor0);
            println!("  setor1     {}", r.setor1);
            println!("  menganggur {} dan {}", r.menganggur0, r.menganggur1);
            println!("  likuiditas {}", r.liquidity);
            println!("  putaran    {}", r.putaran);
            Ok(r.liquidity > 0)
        }
        Err(e) => {
            println!("  gagal: {e}");
            Ok(false)
        }
    }
}
