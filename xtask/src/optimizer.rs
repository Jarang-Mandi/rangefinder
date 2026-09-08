//! Peringkatkan pool sebuah token menurut proyeksi fee per satuan modal.
//!
//! Membuktikan optimizer benar benar bekerja terhadap volume sungguhan, bukan
//! hanya terhadap angka buatan di uji satuan.
//!
//! Cara pakai: `cargo xtask optimizer <alamat_token>`

use std::path::Path;

use alloy::primitives::Address;
use anyhow::{Context, Result};

use rangefinder_engine::chain::registry::Registry;
use rangefinder_engine::chain::ChainClient;
use rangefinder_engine::config::Config;
use rangefinder_engine::pools::{self, volume};
use rangefinder_engine::signals::optimizer;

pub async fn run(repo_root: &Path, token_arg: &str) -> Result<bool> {
    println!("  Optimizer fee tier");
    println!("  ------------------");

    let registry = Registry::load_from(&Registry::default_path(repo_root))?;
    let cfg = Config::load()?;
    let client = ChainClient::new(cfg.rpc.clone());
    let provider = client.provider().await?;

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

    println!(
        "  pasangan  {} ({} pool layak)",
        kelompok.quote_symbol,
        kelompok.pools.len()
    );

    let ids: Vec<String> = kelompok.pools.iter().map(|p| p.id.clone()).collect();
    let terukur = volume::ukur_volume(&provider, &registry, &ids).await?;

    println!(
        "  volume    {} swap pada blok {} sampai {}, kira kira satu jam",
        terukur.jumlah_swap, terukur.dari_blok, terukur.sampai_blok
    );
    println!();

    let vol: Vec<optimizer::VolumePool> = terukur
        .per_pool
        .iter()
        .map(|(id, v)| optimizer::VolumePool {
            pool_id: id,
            volume: *v,
        })
        .collect();

    let hasil = optimizer::peringkat(&kelompok.pools, &vol);

    println!("  Peringkat menurut fee per satuan modal");
    for (i, p) in hasil.iter().enumerate() {
        println!(
            "  {}. {} fee {:.2}%  proyeksi {:.3e}",
            i + 1,
            &p.pool_id[..12.min(p.pool_id.len())],
            p.fee as f64 / 10_000.0,
            p.proyeksi
        );
        println!("     {}", p.catatan);
    }

    if let Some((id, kali)) = optimizer::keunggulan_terhadap_paling_likuid(&hasil) {
        println!();
        println!(
            "  Pool teratas menghasilkan {kali:.1} kali lipat dibanding pool paling likuid {},",
            &id[..12.min(id.len())]
        );
        println!("  dan itulah alasan memilih berdasarkan likuiditas saja keliru.");
    }

    Ok(!hasil.is_empty())
}
