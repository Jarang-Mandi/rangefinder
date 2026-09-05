//! Jalankan seluruh pipeline membuka posisi terhadap pool sungguhan, tanpa
//! mengirim apa pun.
//!
//! Ini yang membuktikan ketujuh gate benar benar terangkai, bukan sekadar ada
//! sebagai fungsi terpisah yang lolos uji satuan.
//!
//! Cara pakai: `cargo xtask dryrun <alamat_token> [alamat_pengirim]`
//!
//! Alamat pengirim menentukan saldo siapa yang dipakai simulasi. Kalau tidak
//! disebut, wallet engine yang dipakai. Kalau wallet belum ada, alamat penerima
//! dari golden vector pertama yang dipakai, karena alamat itu terbukti pernah
//! membuka posisi dan kemungkinan besar masih memegang token.

use std::path::Path;

use alloy::primitives::{Address, U256};
use alloy::providers::{Provider, ProviderBuilder};
use anyhow::{Context, Result};

use rangefinder_engine::chain::registry::Registry;
use rangefinder_engine::config::{self, Config};
use rangefinder_engine::exec::golden;
use rangefinder_engine::exec::pipeline::{self, OpenRequest};
use rangefinder_engine::pools;
use rangefinder_engine::wallet::Wallet;

/// Lebar range percobaan, dalam kelipatan tick spacing pool.
const LEBAR_SPACING: i32 = 10;

pub async fn run(repo_root: &Path, token_arg: &str, from_arg: Option<&str>) -> Result<bool> {
    println!("  Dry run pipeline");
    println!("  ----------------");

    let registry = Registry::load_from(&Registry::default_path(repo_root))?;
    let cfg = Config::load()?;
    let npm = registry.contract("uniswap_v3", "position_manager")?;

    let token: Address = token_arg
        .parse()
        .with_context(|| format!("`{token_arg}` bukan alamat EVM"))?;
    let weth: Address = registry
        .tokens
        .get("weth")
        .context("weth tidak ada di chain.toml")?
        .address
        .parse()?;

    let from = tentukan_pengirim(repo_root, from_arg, &cfg)?;

    let url = cfg.rpc.http.first().cloned().context("rpc.http kosong")?;
    let provider = ProviderBuilder::new().connect_http(url.parse()?);

    println!("  token     {token}");
    println!("  pengirim  {from}");
    println!("  dry run   {}", cfg.execution.dry_run);
    println!();

    // ─── Cari pool ────────────────────────────────────────────────
    let semua = pools::discover_all(&provider, &registry, token, weth).await;
    let layak = pools::usable_pools(semua);
    let Some(pool) = layak.first().cloned() else {
        println!("  Tidak ada pool dengan likuiditas untuk pasangan ini.");
        return Ok(false);
    };

    println!(
        "  pool      {} {:?} spacing {} tick {}",
        &pool.id[..12.min(pool.id.len())],
        pool.version,
        pool.tick_spacing,
        pool.tick
    );

    // ─── Saldo sungguhan pengirim ─────────────────────────────────
    let (saldo0, saldo1) = saldo_pasangan(&provider, &pool, from).await?;
    println!(
        "  saldo     token0 {} token1 {}",
        ringkas(saldo0),
        ringkas(saldo1)
    );

    // ─── Pilih sisi range sesuai token yang benar benar ada ───────
    //
    // Ini bukan sekadar kenyamanan alat. Posisi dua sisi butuh kedua token,
    // dan memaksakannya saat salah satu kosong akan menghasilkan likuiditas
    // nol. Yang benar adalah menempatkan range seluruhnya di satu sisi harga,
    // yaitu strategi LP satu sisi di PRD 11.6, yang berperan sebagai tangga
    // limit order yang dibayar fee.
    let spacing = pool.tick_spacing;
    let tengah = (pool.tick / spacing) * spacing;
    let lebar = spacing * LEBAR_SPACING;

    let (tick_lower, tick_upper, bentuk) = if saldo0.is_zero() && !saldo1.is_zero() {
        // Hanya token1. Range harus seluruhnya DI BAWAH harga pool sekarang,
        // karena di bawah range posisi hanya berisi token1.
        (tengah - lebar * 2, tengah - spacing, "satu sisi, hanya token1")
    } else if saldo1.is_zero() && !saldo0.is_zero() {
        // Hanya token0. Range seluruhnya di atas harga pool.
        (tengah + spacing, tengah + lebar * 2, "satu sisi, hanya token0")
    } else {
        (tengah - lebar, tengah + lebar, "dua sisi, mengelilingi harga")
    };

    println!("  range     {tick_lower} sampai {tick_upper} ({bentuk})");
    println!();

    // Kegagalan membaca saldo TIDAK boleh ditelan jadi nol. Nol yang palsu
    // membuat gate G6 menolak dengan alasan yang salah, dan user akan mengejar
    // masalah yang tidak ada.
    let eth_balance = provider
        .get_balance(from)
        .await
        .with_context(|| format!("gagal membaca saldo ETH {from}"))?;
    let eth_f: f64 = eth_balance
        .to_string()
        .parse::<f64>()
        .with_context(|| format!("saldo {eth_balance} tidak bisa diubah ke pecahan"))?
        / 1e18;
    println!("  eth       {eth_f:.6}");

    let deadline = U256::from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            + 1_200,
    );

    let req = OpenRequest {
        pool: pool.clone(),
        tick_lower,
        tick_upper,
        available0: saldo0,
        available1: saldo1,
        recipient: from,
        deadline,
        // Nilai USD tidak dihitung di sini karena dry run ini tidak menyentuh
        // kalibrasi kurs. Pembatas nominal bawaannya tidak terbatas, jadi nol
        // tidak mengubah hasil gate G6.
        position_usd: 0.0,
        daily_spent_usd: 0.0,
        open_positions: 0,
        is_new_pool: false,
        eth_balance: eth_f,
        eth_needed: 0.0,
    };

    let plan = pipeline::plan_open(&provider, npm, &req, &cfg.limits, &cfg.execution).await;

    // ─── Laporan ──────────────────────────────────────────────────
    println!("  Gate");
    for g in &plan.gates {
        println!(
            "  {} {:<4} {:<18} {}",
            if g.passed { "lolos" } else { "GAGAL" },
            g.id,
            g.name,
            g.detail
        );
    }
    println!();

    for w in &plan.warnings {
        println!("  peringatan: {w}");
    }

    println!("  likuiditas {}", plan.liquidity);
    println!("  token0     {}", plan.amount0);
    println!("  token1     {}", plan.amount1);
    println!("  bisa kirim {}", plan.is_sendable());

    if plan.dry_run {
        println!();
        println!("  Dry run menyala, tidak ada yang dikirim.");
    }

    let semua_lolos = plan.gates.iter().all(|g| g.passed);
    Ok(semua_lolos)
}

fn tentukan_pengirim(repo_root: &Path, arg: Option<&str>, _cfg: &Config) -> Result<Address> {
    if let Some(s) = arg {
        return s.parse().with_context(|| format!("`{s}` bukan alamat EVM"));
    }

    // Wallet engine, kalau sudah dibuat.
    if let Ok(dir) = config::data_dir() {
        if let Ok(w) = Wallet::open(&dir, 30) {
            if let Some(a) = w.address() {
                return Ok(a);
            }
        }
    }

    // Cadangan terakhir: alamat penerima dari golden vector. Alamat itu
    // terbukti pernah membuka posisi di chain ini, jadi simulasinya lebih
    // berguna daripada memakai alamat kosong.
    let vectors = golden::load_all(repo_root).map_err(|e| anyhow::anyhow!(e))?;
    let v = vectors
        .first()
        .context("belum ada wallet dan belum ada golden vector, sebutkan alamat pengirim")?;
    v.params
        .recipient
        .parse()
        .context("alamat penerima di golden vector tidak valid")
}

async fn saldo_pasangan<P: Provider>(
    provider: &P,
    pool: &pools::PoolInfo,
    pemilik: Address,
) -> Result<(U256, U256)> {
    use rangefinder_engine::exec::abi_write::IERC20Write;

    let t0: Address = pool.token0.parse()?;
    let t1: Address = pool.token1.parse()?;

    let a = IERC20Write::new(t0, provider)
        .balanceOf(pemilik)
        .call()
        .await
        .with_context(|| format!("gagal membaca saldo token0 {t0}"))?;
    let b = IERC20Write::new(t1, provider)
        .balanceOf(pemilik)
        .call()
        .await
        .with_context(|| format!("gagal membaca saldo token1 {t1}"))?;

    Ok((a, b))
}

fn ringkas(v: U256) -> String {
    let f: f64 = v.to_string().parse().unwrap_or(0.0);
    if f == 0.0 {
        "0".into()
    } else {
        format!("{:.4e}", f / 1e18)
    }
}
