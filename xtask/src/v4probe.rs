//! Buktikan ABI V4 yang kita tulis benar benar ada di kontrak yang terpasang.
//!
//! KENAPA PERINTAH INI ADA
//! ABI yang disalin dari repo Uniswap adalah asumsi, bukan fakta. Rantai ini
//! sudah terbukti memakai fork pada UniversalRouter, yang membawa field
//! tambahan `minHopPriceX36` sehingga calldata SDK standar gagal. Kalau hal
//! serupa terjadi pada PositionManager, kesalahannya baru ketahuan saat
//! transaksi revert setelah gas terbakar.
//!
//! Dua cara pembuktian dipakai sekaligus:
//!
//! 1. Selector fungsi dicari langsung di dalam bytecode. Compiler Solidity
//!    menaruh setiap selector sebagai konstanta di tabel dispatch, jadi
//!    selector yang tidak ada di bytecode berarti fungsinya tidak ada.
//! 2. Fungsi baca dipanggil sungguhan lewat `eth_call`. Ini menangkap kasus
//!    selector ada tapi bentuk kembaliannya berbeda.
//!
//! Cara pakai: `cargo xtask v4probe`

use std::path::Path;

use alloy::primitives::U256;
use alloy::providers::Provider;
use alloy::sol_types::{SolCall, SolEvent};
use anyhow::{Context, Result};

use rangefinder_engine::chain::registry::Registry;
use rangefinder_engine::chain::ChainClient;
use rangefinder_engine::config::Config;
use rangefinder_engine::exec::v4::abi::{IPermit2, IPositionManagerV4, ModifyLiquidity};

pub async fn run(repo_root: &Path) -> Result<bool> {
    println!("  Pembuktian ABI V4");
    println!("  -----------------");

    let registry = Registry::load_from(&Registry::default_path(repo_root))?;
    let cfg = Config::load()?;
    let client = ChainClient::new(cfg.rpc.clone());
    let provider = client.provider().await?;

    let pm = registry.contract("uniswap_v4", "position_manager")?;
    let permit2 = registry.contract("common", "permit2")?;
    let pool_manager = registry.contract("uniswap_v4", "pool_manager")?;

    println!("  position manager  {pm}");
    println!("  permit2           {permit2}");
    println!();

    let kode_pm = provider
        .get_code_at(pm)
        .await
        .with_context(|| format!("gagal membaca bytecode {pm}"))?;
    let kode_permit2 = provider
        .get_code_at(permit2)
        .await
        .with_context(|| format!("gagal membaca bytecode {permit2}"))?;

    let mut semua_ada = true;

    println!("  Selector di bytecode PositionManager");
    for (nama, sel) in [
        (
            "modifyLiquidities(bytes,uint256)",
            IPositionManagerV4::modifyLiquiditiesCall::SELECTOR,
        ),
        ("nextTokenId()", IPositionManagerV4::nextTokenIdCall::SELECTOR),
        (
            "getPositionLiquidity(uint256)",
            IPositionManagerV4::getPositionLiquidityCall::SELECTOR,
        ),
        (
            "getPoolAndPositionInfo(uint256)",
            IPositionManagerV4::getPoolAndPositionInfoCall::SELECTOR,
        ),
        ("poolManager()", IPositionManagerV4::poolManagerCall::SELECTOR),
        ("permit2()", IPositionManagerV4::permit2Call::SELECTOR),
        ("ownerOf(uint256)", IPositionManagerV4::ownerOfCall::SELECTOR),
    ] {
        let ada = cari_selector(&kode_pm, &sel);
        if !ada {
            semua_ada = false;
        }
        println!(
            "  {} 0x{}  {nama}",
            if ada { "ada   " } else { "HILANG" },
            hex::encode(sel)
        );
    }

    println!();
    println!("  Selector di bytecode Permit2");
    for (nama, sel) in [
        (
            "approve(address,address,uint160,uint48)",
            IPermit2::approveCall::SELECTOR,
        ),
        (
            "allowance(address,address,address)",
            IPermit2::allowanceCall::SELECTOR,
        ),
    ] {
        let ada = cari_selector(&kode_permit2, &sel);
        if !ada {
            semua_ada = false;
        }
        println!(
            "  {} 0x{}  {nama}",
            if ada { "ada   " } else { "HILANG" },
            hex::encode(sel)
        );
    }

    // ─── Panggil sungguhan, supaya bentuk kembaliannya ikut terbukti ───
    println!();
    println!("  Panggilan baca sungguhan");

    let npm = IPositionManagerV4::new(pm, &provider);

    let next_id = match npm.nextTokenId().call().await {
        Ok(v) => {
            println!("  ada    nextTokenId() = {v}");
            Some(v)
        }
        Err(e) => {
            semua_ada = false;
            println!("  GAGAL  nextTokenId(): {e}");
            None
        }
    };

    match npm.poolManager().call().await {
        Ok(v) => {
            let cocok = v == pool_manager;
            if !cocok {
                semua_ada = false;
            }
            println!(
                "  {} poolManager() = {v}{}",
                if cocok { "ada   " } else { "BEDA  " },
                if cocok {
                    String::new()
                } else {
                    format!(", chain.toml menyebut {pool_manager}")
                }
            );
        }
        Err(e) => {
            semua_ada = false;
            println!("  GAGAL  poolManager(): {e}");
        }
    }

    match npm.permit2().call().await {
        Ok(v) => {
            let cocok = v == permit2;
            if !cocok {
                semua_ada = false;
            }
            println!(
                "  {} permit2() = {v}{}",
                if cocok { "ada   " } else { "BEDA  " },
                if cocok {
                    String::new()
                } else {
                    format!(", chain.toml menyebut {permit2}")
                }
            );
        }
        Err(e) => {
            semua_ada = false;
            println!("  GAGAL  permit2(): {e}");
        }
    }

    // Posisi terakhir yang pernah dibuat. Membacanya membuktikan bentuk
    // kembalian `getPoolAndPositionInfo` benar, dan sekaligus memberi contoh
    // PoolKey sungguhan.
    if let Some(next) = next_id {
        if next > U256::from(1u8) {
            let token_id = next - U256::from(1u8);
            println!();
            println!("  Posisi V4 sungguhan, tokenId {token_id}");

            match npm.getPoolAndPositionInfo(token_id).call().await {
                Ok(h) => {
                    println!("    currency0    {}", h._0.currency0);
                    println!("    currency1    {}", h._0.currency1);
                    println!("    fee          {}", h._0.fee);
                    println!("    tickSpacing  {}", h._0.tickSpacing);
                    println!("    hooks        {}", h._0.hooks);
                    println!("    info         0x{:x}", h._1);
                }
                Err(e) => {
                    semua_ada = false;
                    println!("    GAGAL getPoolAndPositionInfo: {e}");
                }
            }

            match npm.getPositionLiquidity(token_id).call().await {
                Ok(v) => println!("    likuiditas   {v}"),
                Err(e) => {
                    semua_ada = false;
                    println!("    GAGAL getPositionLiquidity: {e}");
                }
            }

            match npm.ownerOf(token_id).call().await {
                Ok(v) => println!("    pemilik      {v}"),
                Err(e) => {
                    semua_ada = false;
                    println!("    GAGAL ownerOf: {e}");
                }
            }
        }
    }

    println!();
    println!(
        "  topic ModifyLiquidity 0x{}",
        hex::encode(ModifyLiquidity::SIGNATURE_HASH)
    );

    Ok(semua_ada)
}
/// Cari empat byte selector di dalam bytecode.
///
/// Compiler Solidity menaruh setiap selector sebagai konstanta `PUSH4` di tabel
/// dispatch, jadi pencarian byte mentah sudah cukup. Kemungkinan cocok secara
/// kebetulan ada, tapi kecil, dan pembuktian kedua lewat `eth_call` menutupnya.
fn cari_selector(kode: &[u8], selector: &[u8; 4]) -> bool {
    kode.windows(4).any(|w| w == selector)
}
