//! Penemuan pool dan pembacaan state on chain.
//!
//! INVARIAN
//! - V3 dan V4 ditemukan dengan cara yang sama sekali berbeda, dan itu bukan
//!   pilihan. V3 punya factory yang bisa ditanya `getPool` per fee tier. V4
//!   tidak punya factory sama sekali, pool-nya hanya ada sebagai entri di
//!   singleton PoolManager, jadi satu satunya cara menemukannya adalah memindai
//!   event `Initialize`.
//! - Pemindaian log V4 WAJIB memakai batas bawah `chain.deploy_block` dan
//!   disaring lewat topic `currency0` dan `currency1`. Tanpa itu RPC publik
//!   akan menolak permintaannya.
//! - Modul ini hanya membaca. Tidak ada satu pun fungsi yang mengubah state.
//!   Jalur tulis baru muncul di fase 3 setelah gate validasi lengkap ada.
//! - Alamat kontrak tidak pernah ditulis di sini, semuanya lewat `registry`.

pub mod abi;

use alloy::primitives::{Address, FixedBytes, U256};
use alloy::providers::Provider;
use alloy::rpc::types::Filter;
use alloy::sol_types::SolEvent;
use anyhow::{Context, Result};
use serde::Serialize;

use crate::chain::registry::Registry;

/// Fee tier V3 yang di-deploy secara baku oleh Uniswap.
///
/// Bukan daftar lengkap secara teori, tapi factory hanya mengizinkan tick
/// spacing yang sudah didaftarkan, dan keempat ini yang ada di semua chain.
pub const V3_FEE_TIERS: [u32; 4] = [100, 500, 3_000, 10_000];

/// Penanda dynamic fee pada V4.
///
/// Kalau bit ini menyala, angka fee di PoolKey BUKAN besaran fee. Fee
/// sebenarnya ditentukan hook saat swap terjadi, jadi tidak bisa diketahui dari
/// keadaan pool saja.
///
/// Ditemukan dari data sungguhan: pool V4 di Robinhood Chain melaporkan fee
/// 8388608, yaitu 0x800000 persis. Tanpa penanganan ini, pemeringkatan berbasis
/// fee akan menganggapnya 838 persen dan menempatkannya di puncak daftar.
pub const DYNAMIC_FEE_FLAG: u32 = 0x80_0000;

/// Fee yang diasumsikan untuk pool dynamic fee saat memeringkat.
///
/// Nilai konservatif, setara 0,3 persen. Ini hanya untuk urutan tampilan.
/// Angka sebenarnya baru diketahui saat kutipan diminta ke quoter, dan itu
/// yang dipakai untuk keputusan.
pub const ASSUMED_DYNAMIC_FEE: u32 = 3_000;

pub const ZERO_ADDRESS: &str = "0x0000000000000000000000000000000000000000";

/// Fee tertinggi yang masih masuk akal untuk pool yang benar benar dipakai.
///
/// KENAPA ADA BATAS INI
/// Pemindaian FIG di Robinhood Chain menemukan pool dengan fee 80,27 persen,
/// 81,00 persen, dan 20,00 persen, dan ketiganya PUNYA likuiditas. Tidak ada
/// swap waras yang mau membayar 80 persen, jadi pool seperti itu tidak akan
/// pernah menghasilkan fee bagi LP. Yang lebih berbahaya, pemeringkatan
/// `likuiditas x fee` justru menaruhnya di puncak daftar, tepat di tempat user
/// paling mungkin memilihnya.
///
/// Ambangnya 10 persen, bukan angka Uniswap baku, karena di chain ini pool 8
/// persen memang benar benar diperdagangkan dengan volume ratusan ribu dolar.
/// Membatasi ke 1 persen akan membuang pool yang sah.
pub const MAX_PLAUSIBLE_FEE: u32 = 100_000;

/// Benar kalau fee-nya di luar batas kewajaran untuk LP.
///
/// Dua arah, dan keduanya sama sama tidak berguna:
///
/// Fee terlalu tinggi berarti tidak akan ada yang mau swap lewat sana, jadi
/// tidak ada fee yang mengalir ke kita. Fee nol berarti swap memang lewat, tapi
/// kita tidak dibayar sepeser pun sambil tetap menanggung impermanent loss.
/// Pool fee nol yang ditemukan pada FIG bahkan punya likuiditas sangat besar,
/// yang membuatnya menang telak di pemeringkatan mana pun yang tidak menyaring.
pub fn implausible_fee(p: &PoolInfo) -> bool {
    if p.dynamic_fee {
        return false;
    }
    p.fee == 0 || p.fee > MAX_PLAUSIBLE_FEE
}

#[derive(Debug, Clone, Serialize)]
pub struct TokenMeta {
    pub address: String,
    pub symbol: String,
    pub decimals: u8,
    /// Suplai dalam satuan manusiawi, sudah dibagi 10^decimals.
    pub total_supply: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PoolVersion {
    V3,
    V4,
}

#[derive(Debug, Clone, Serialize)]
pub struct PoolInfo {
    pub version: PoolVersion,
    /// Alamat pool untuk V3, atau poolId heksadesimal untuk V4.
    pub id: String,
    pub token0: String,
    pub token1: String,
    /// Angka fee apa adanya dari pool. Untuk V4 dengan dynamic fee, ini penanda
    /// `0x800000`, bukan besaran fee. Periksa `dynamic_fee` sebelum memakainya.
    pub fee: u32,
    /// Benar kalau fee ditentukan hook saat swap, bukan tetap.
    pub dynamic_fee: bool,
    pub tick_spacing: i32,
    /// Alamat hook. Nol berarti tanpa hook. Hanya relevan untuk V4.
    pub hooks: Option<String>,
    pub sqrt_price_x96: String,
    pub tick: i32,
    pub liquidity: String,
    /// Harga mentah token1 per token0 pada satuan terkecil.
    pub raw_price: f64,
}

/// Ubah `sqrtPriceX96` menjadi harga mentah token1 per token0.
///
/// Lewat `f64` karena nilainya bisa melampaui `u128`. Presisinya cukup untuk
/// tampilan dan untuk menurunkan tick. Nilai yang dikirim ke rantai TIDAK
/// boleh dihitung dari sini, lihat catatan presisi di modul `math`.
pub fn raw_price_from_sqrt_x96(sqrt_price_x96: U256) -> f64 {
    let s: f64 = sqrt_price_x96.to_string().parse().unwrap_or(f64::NAN);
    let ratio = s / 2f64.powi(96);
    ratio * ratio
}

pub async fn token_meta<P: Provider>(provider: &P, address: Address) -> Result<TokenMeta> {
    let erc20 = abi::IERC20::new(address, provider);

    let decimals = erc20
        .decimals()
        .call()
        .await
        .with_context(|| format!("{address} tidak menjawab decimals(), apakah ini ERC20?"))?;

    let raw_supply = erc20
        .totalSupply()
        .call()
        .await
        .with_context(|| format!("{address} tidak menjawab totalSupply()"))?;

    // Simbol sering tidak standar di memecoin, jadi kegagalannya tidak fatal.
    let symbol = erc20
        .symbol()
        .call()
        .await
        .unwrap_or_else(|_| "?".to_string());

    let divisor = 10f64.powi(decimals as i32);
    let supply: f64 = raw_supply.to_string().parse().unwrap_or(f64::NAN);

    Ok(TokenMeta {
        address: address.to_string(),
        symbol,
        decimals,
        total_supply: supply / divisor,
    })
}

/// Cari pool V3 untuk sepasang token pada seluruh fee tier baku.
///
/// Fee tier yang tidak punya pool dilewati diam diam, karena itu keadaan
/// normal, bukan kesalahan.
pub async fn discover_v3<P: Provider>(
    provider: &P,
    registry: &Registry,
    token_a: Address,
    token_b: Address,
) -> Result<Vec<PoolInfo>> {
    let factory_address = registry.contract("uniswap_v3", "factory")?;
    let factory = abi::IUniswapV3Factory::new(factory_address, provider);

    let mut out = Vec::new();

    for fee in V3_FEE_TIERS {
        // `uint24` di ABI memetakan ke U24, bukan u32, jadi konversinya eksplisit.
        let fee_u24 = alloy::primitives::aliases::U24::from(fee);
        let pool_address = match factory.getPool(token_a, token_b, fee_u24).call().await {
            Ok(a) => a,
            Err(e) => {
                tracing::debug!(fee, error = %e, "getPool gagal, fee tier dilewati");
                continue;
            }
        };
        if pool_address.is_zero() {
            continue;
        }

        match read_v3_pool(provider, pool_address, fee).await {
            Ok(info) => out.push(info),
            Err(e) => tracing::warn!(pool = %pool_address, error = %e, "pool V3 gagal dibaca"),
        }
    }

    Ok(out)
}

async fn read_v3_pool<P: Provider>(
    provider: &P,
    address: Address,
    fee: u32,
) -> Result<PoolInfo> {
    let pool = abi::IUniswapV3Pool::new(address, provider);

    let slot0 = pool.slot0().call().await.context("slot0() gagal")?;
    let liquidity = pool.liquidity().call().await.context("liquidity() gagal")?;
    let token0 = pool.token0().call().await.context("token0() gagal")?;
    let token1 = pool.token1().call().await.context("token1() gagal")?;
    let tick_spacing = pool
        .tickSpacing()
        .call()
        .await
        .context("tickSpacing() gagal")?;

    let sqrt = U256::from(slot0.sqrtPriceX96);

    Ok(PoolInfo {
        version: PoolVersion::V3,
        id: address.to_string(),
        token0: token0.to_string(),
        token1: token1.to_string(),
        fee,
        dynamic_fee: false,
        tick_spacing: tick_spacing.as_i32(),
        hooks: None,
        sqrt_price_x96: sqrt.to_string(),
        tick: slot0.tick.as_i32(),
        liquidity: liquidity.to_string(),
        raw_price: raw_price_from_sqrt_x96(sqrt),
    })
}

/// Cari pool V4 dengan memindai event `Initialize` milik PoolManager.
///
/// Penyaringan dilakukan lewat topic `currency0` dan `currency1`, sehingga
/// RPC yang mengerjakan penyaringan, bukan kita. Urutan currency mengikuti
/// aturan Uniswap: alamat yang lebih kecil menempati currency0.
pub async fn discover_v4<P: Provider>(
    provider: &P,
    registry: &Registry,
    token_a: Address,
    token_b: Address,
) -> Result<Vec<PoolInfo>> {
    let pool_manager = registry.contract("uniswap_v4", "pool_manager")?;
    let state_view = registry.contract("uniswap_v4", "state_view")?;

    let (currency0, currency1) = if token_a < token_b {
        (token_a, token_b)
    } else {
        (token_b, token_a)
    };

    let filter = Filter::new()
        .address(pool_manager)
        .event_signature(abi::Initialize::SIGNATURE_HASH)
        .topic2(currency0.into_word())
        .topic3(currency1.into_word())
        .from_block(registry.chain.deploy_block);

    let logs = provider
        .get_logs(&filter)
        .await
        .context("pemindaian log Initialize V4 gagal. Endpoint publik bisa membatasi rentang blok")?;

    let view = abi::IStateView::new(state_view, provider);
    let mut out = Vec::new();

    for log in logs {
        let decoded = match abi::Initialize::decode_log(&log.inner) {
            Ok(d) => d,
            Err(e) => {
                tracing::debug!(error = %e, "log Initialize gagal didekode, dilewati");
                continue;
            }
        };

        let pool_id: FixedBytes<32> = decoded.id;

        let slot0 = match view.getSlot0(pool_id).call().await {
            Ok(s) => s,
            Err(e) => {
                // Pool pernah dibuat tapi sekarang tidak terbaca. Itu bisa
                // terjadi kalau hook-nya bermasalah, dan bukan alasan
                // menggagalkan seluruh pencarian.
                tracing::warn!(pool = %pool_id, error = %e, "getSlot0 V4 gagal, pool dilewati");
                continue;
            }
        };
        let liquidity = view.getLiquidity(pool_id).call().await.unwrap_or(0);

        let sqrt = U256::from(slot0.sqrtPriceX96);
        let fee = decoded.fee.to::<u32>();
        let dynamic_fee = fee & DYNAMIC_FEE_FLAG != 0;

        out.push(PoolInfo {
            version: PoolVersion::V4,
            id: format!("0x{}", hex::encode(pool_id)),
            token0: decoded.currency0.to_string(),
            token1: decoded.currency1.to_string(),
            fee,
            dynamic_fee,
            tick_spacing: decoded.tickSpacing.as_i32(),
            hooks: Some(decoded.hooks.to_string()),
            sqrt_price_x96: sqrt.to_string(),
            tick: slot0.tick.as_i32(),
            liquidity: liquidity.to_string(),
            raw_price: raw_price_from_sqrt_x96(sqrt),
        });
    }

    Ok(out)
}

/// Cari pool V4 untuk satu token, TANPA mengandaikan pasangannya.
///
/// KENAPA INI ADA
/// `discover_v4` menyaring dua currency sekaligus, jadi ia hanya menemukan
/// pool kalau kita sudah menebak benar token lawannya. Tebakan itu ternyata
/// sering salah: banyak memecoin di chain ini berpasangan dengan stablecoin,
/// bukan WETH, dan hasilnya kita melaporkan tidak ada pool padahal ada puluhan.
///
/// Fungsi ini memindai dua kali, sekali dengan token pada posisi currency0 dan
/// sekali pada currency1, lalu menggabungkan hasilnya. Token lawannya ikut
/// terbaca dari log, jadi tidak perlu diketahui lebih dulu.
pub async fn discover_v4_any<P: Provider>(
    provider: &P,
    registry: &Registry,
    token: Address,
) -> Result<Vec<PoolInfo>> {
    let pool_manager = registry.contract("uniswap_v4", "pool_manager")?;
    let state_view = registry.contract("uniswap_v4", "state_view")?;
    let view = abi::IStateView::new(state_view, provider);

    let latest = provider
        .get_block_number()
        .await
        .context("gagal membaca blok terakhir")?;

    let mut out = Vec::new();
    let mut terlihat: Vec<String> = Vec::new();

    // Urutan currency ditentukan alamat, dan kita tidak tahu token ini yang
    // lebih kecil atau lebih besar, jadi kedua posisi harus dicoba.
    for sebagai_currency0 in [true, false] {
        // Batas bawah adalah jendela mundur, bukan deploy_block, karena pool
        // yang dicari selalu yang baru. Tidak pernah turun di bawah blok
        // pemasangan protokol.
        let batas_bawah = latest
            .saturating_sub(SCAN_LOOKBACK_DEFAULT)
            .max(registry.chain.deploy_block);

        let logs = scan_initialize_logs(
            provider,
            pool_manager,
            token,
            sebagai_currency0,
            batas_bawah,
            latest,
        )
        .await?;

        for log in logs {
            let Ok(decoded) = abi::Initialize::decode_log(&log.inner) else {
                continue;
            };
            let pool_id: FixedBytes<32> = decoded.id;
            let id = format!("0x{}", hex::encode(pool_id));
            if terlihat.contains(&id) {
                continue;
            }
            terlihat.push(id.clone());

            let Ok(slot0) = view.getSlot0(pool_id).call().await else {
                continue;
            };
            let liquidity = view.getLiquidity(pool_id).call().await.unwrap_or(0);
            let sqrt = U256::from(slot0.sqrtPriceX96);
            let fee = decoded.fee.to::<u32>();

            out.push(PoolInfo {
                version: PoolVersion::V4,
                id,
                token0: decoded.currency0.to_string(),
                token1: decoded.currency1.to_string(),
                fee,
                dynamic_fee: fee & DYNAMIC_FEE_FLAG != 0,
                tick_spacing: decoded.tickSpacing.as_i32(),
                hooks: Some(decoded.hooks.to_string()),
                sqrt_price_x96: sqrt.to_string(),
                tick: slot0.tick.as_i32(),
                liquidity: liquidity.to_string(),
                raw_price: raw_price_from_sqrt_x96(sqrt),
            });
        }
    }

    Ok(out)
}

/// Rentang blok terlebar yang dicoba sekali jalan.
///
/// RPC publik menolak rentang yang terlalu lebar, dan chain ini sudah melewati
/// 55 juta blok karena waktu bloknya sekitar 100 ms. Memindai sekaligus akan
/// selalu ditolak.
const SCAN_CHUNK_AWAL: u64 = 500_000;
const SCAN_CHUNK_MINIMUM: u64 = 5_000;

/// Sejauh mana mundur dari blok terakhir sebelum menyerah.
///
/// Tiga juta blok kira kira tiga setengah hari pada waktu blok 100 ms. Itu
/// cukup untuk seluruh memecoin yang masih relevan untuk LP, dan jauh lebih
/// murah daripada menyapu 55 juta blok sejarah.
pub const SCAN_LOOKBACK_DEFAULT: u64 = 3_000_000;

/// Pindai log `Initialize` mundur dari blok terakhir.
///
/// KENAPA MUNDUR, BUKAN MAJU
/// Percobaan pertama memindai maju dari `deploy_block`, dan itu keliru dua
/// kali. Pertama, chain ini sudah melewati 55 juta blok karena waktu bloknya
/// 100 ms, jadi menyapu seluruhnya butuh ribuan permintaan dan pasti kena rate
/// limit. Kedua, pool yang kita cari selalu yang baru, jadi sejarah lama itu
/// justru bagian yang paling tidak berguna.
///
/// Mundur dari blok terakhir menemukan yang relevan pada permintaan pertama.
///
/// KESALAHAN TIDAK DITELAN
/// Versi pertama memakai `continue` saat pemindaian gagal, sehingga hasilnya
/// nol pool tanpa satu pun pesan, padahal pool-nya ada puluhan. Kegagalan yang
/// tidak terlihat lebih buruk daripada kegagalan yang berisik.
async fn scan_initialize_logs<P: Provider>(
    provider: &P,
    pool_manager: Address,
    token: Address,
    sebagai_currency0: bool,
    batas_bawah: u64,
    sampai_blok: u64,
) -> Result<Vec<alloy::rpc::types::Log>> {
    let mut out = Vec::new();
    let mut chunk = SCAN_CHUNK_AWAL;
    let mut hingga = sampai_blok;

    while hingga > batas_bawah {
        let mulai = hingga.saturating_sub(chunk).max(batas_bawah);

        let dasar = Filter::new()
            .address(pool_manager)
            .event_signature(abi::Initialize::SIGNATURE_HASH)
            .from_block(mulai)
            .to_block(hingga);

        let filter = if sebagai_currency0 {
            dasar.topic2(token.into_word())
        } else {
            dasar.topic3(token.into_word())
        };

        match provider.get_logs(&filter).await {
            Ok(mut l) => {
                out.append(&mut l);
                if mulai == batas_bawah {
                    break;
                }
                hingga = mulai - 1;
            }
            Err(e) => {
                if chunk > SCAN_CHUNK_MINIMUM {
                    // Rentang terlalu lebar. Kecilkan lalu ulangi rentang yang
                    // sama, bukan lanjut mundur, supaya tidak ada blok terlewat.
                    chunk /= 2;
                    tracing::debug!(chunk, "rentang pemindaian dikecilkan");
                    continue;
                }
                return Err(anyhow::anyhow!(
                    "pemindaian log gagal pada blok {mulai} sampai {hingga} \
                     bahkan dengan rentang {chunk} blok: {e}"
                ));
            }
        }
    }

    Ok(out)
}

/// Token lawan yang muncul pada sekumpulan pool, beserta jumlah pool-nya.
///
/// Dipakai untuk menjawab pertanyaan "token ini sebenarnya berpasangan dengan
/// apa", tanpa perlu menebak lebih dulu.
pub fn counterparties(pools: &[PoolInfo], token: &str) -> Vec<(String, usize)> {
    let t = token.to_lowercase();
    let mut hitung: std::collections::BTreeMap<String, usize> = Default::default();

    for p in pools {
        let lawan = if p.token0.to_lowercase() == t {
            &p.token1
        } else if p.token1.to_lowercase() == t {
            &p.token0
        } else {
            continue;
        };
        *hitung.entry(lawan.clone()).or_insert(0) += 1;
    }

    let mut v: Vec<_> = hitung.into_iter().collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.1));
    v
}

/// Sekelompok pool yang berbagi token lawan yang sama.
#[derive(Debug, Clone, Serialize)]
pub struct QuoteGroup {
    /// Alamat token lawan. Nol berarti ETH asli, yang hanya ada di V4.
    pub quote: String,
    pub quote_symbol: String,
    pub quote_decimals: u8,
    pub pools: Vec<PoolInfo>,
}

/// Cari seluruh pool untuk sebuah token, dikelompokkan menurut token lawannya.
///
/// KENAPA DIKELOMPOKKAN
/// Membandingkan pool FIG/USDG dengan FIG/ETH berdasarkan likuiditas tidak
/// bermakna, karena harganya berada di sumbu yang berbeda. User memilih
/// pasangannya dulu, baru fee tier di dalam pasangan itu.
///
/// KENAPA TIDAK MENEBAK PASANGANNYA
/// Versi pertama hanya mencari pasangan terhadap WETH, dan itu keliru. Pada FIG
/// di Robinhood Chain, 33 dari 39 pool berpasangan dengan USDG dan tidak satu
/// pun dengan WETH. Hasilnya kita melaporkan tidak ada pool sama sekali,
/// padahal alat lain menemukan puluhan.
pub async fn discover_for_token<P: Provider>(
    provider: &P,
    registry: &Registry,
    token: Address,
) -> Vec<QuoteGroup> {
    let mut semua: Vec<PoolInfo> = Vec::new();

    // V4 tidak perlu tahu pasangannya lebih dulu.
    match discover_v4_any(provider, registry, token).await {
        Ok(mut v) => semua.append(&mut v),
        Err(e) => tracing::warn!(error = %e, "penemuan pool V4 gagal"),
    }

    // V3 butuh kedua alamat, jadi setiap token kutipan yang dikenal dicoba.
    for (nama, entry) in &registry.tokens {
        let Ok(quote) = entry.address.parse::<Address>() else {
            continue;
        };
        if quote == token {
            continue;
        }
        match discover_v3(provider, registry, token, quote).await {
            Ok(mut v) => semua.append(&mut v),
            Err(e) => tracing::warn!(quote = %nama, error = %e, "penemuan pool V3 gagal"),
        }
    }

    kelompokkan(semua, token, registry)
}

fn kelompokkan(pools: Vec<PoolInfo>, token: Address, registry: &Registry) -> Vec<QuoteGroup> {
    let t = token.to_string().to_lowercase();
    let mut per_quote: std::collections::BTreeMap<String, Vec<PoolInfo>> = Default::default();

    for p in pools {
        let lawan = if p.token0.to_lowercase() == t {
            p.token1.clone()
        } else if p.token1.to_lowercase() == t {
            p.token0.clone()
        } else {
            continue;
        };
        per_quote.entry(lawan).or_default().push(p);
    }

    let mut out: Vec<QuoteGroup> = per_quote
        .into_iter()
        .map(|(quote, pools)| {
            let (symbol, decimals) = kenali_quote(&quote, registry);
            QuoteGroup {
                quote,
                quote_symbol: symbol,
                quote_decimals: decimals,
                pools: usable_pools(pools),
            }
        })
        .filter(|g| !g.pools.is_empty())
        .collect();

    // Kelompok dengan likuiditas terbesar lebih dulu, karena itu yang paling
    // mungkin jadi pasar sebenarnya.
    out.sort_by(|a, b| {
        let total = |g: &QuoteGroup| -> f64 {
            g.pools
                .iter()
                .map(|p| p.liquidity.parse::<f64>().unwrap_or(0.0))
                .sum()
        };
        total(b)
            .partial_cmp(&total(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    out
}

/// Kenali token lawan dari registry, atau tandai sebagai tidak dikenal.
///
/// Token yang tidak dikenal bukan berarti buruk, tapi wajib terlihat. Pasangan
/// dengan token asing bisa berarti pasar yang tipis, atau token tiruan.
fn kenali_quote(alamat: &str, registry: &Registry) -> (String, u8) {
    if alamat.eq_ignore_ascii_case(ZERO_ADDRESS) {
        // V4 mendukung ETH asli sebagai currency, bukan hanya WETH.
        return ("ETH".into(), 18);
    }
    for (nama, entry) in &registry.tokens {
        if entry.address.eq_ignore_ascii_case(alamat) {
            return (nama.to_uppercase(), entry.decimals);
        }
    }
    ("?".into(), 18)
}

/// Cari seluruh pool untuk sepasang token, V3 dan V4 sekaligus.
///
/// Kegagalan salah satu versi tidak menggagalkan yang lain, karena V4 memakai
/// pemindaian log yang lebih rentan ditolak RPC publik. Yang gagal dilaporkan
/// lewat log, bukan lewat error, supaya user tetap mendapat pool yang ada.
pub async fn discover_all<P: Provider>(
    provider: &P,
    registry: &Registry,
    token_a: Address,
    token_b: Address,
) -> Vec<PoolInfo> {
    let mut out = Vec::new();

    match discover_v3(provider, registry, token_a, token_b).await {
        Ok(mut v) => out.append(&mut v),
        Err(e) => tracing::warn!(error = %e, "penemuan pool V3 gagal"),
    }
    match discover_v4(provider, registry, token_a, token_b).await {
        Ok(mut v) => out.append(&mut v),
        Err(e) => tracing::warn!(error = %e, "penemuan pool V4 gagal"),
    }

    out
}

/// Saring pool yang tidak layak dipakai, lalu urutkan dari yang paling
/// menjanjikan.
///
/// KENAPA PENYARINGAN INI WAJIB
/// Pengujian terhadap chain sungguhan menunjukkan pasangan token biasanya punya
/// pool di semua fee tier, tapi sebagian besar kosong. Pool kosong tetap
/// menjawab `slot0` dengan harga dari saat ia dibuat, dan harga itu bisa
/// meleset berkali kali lipat dari harga sebenarnya. Menampilkannya di pemilih
/// pool akan membuat user memilih pool yang tidak pernah diperdagangkan.
///
/// Contoh nyata pada PONS/WETH: fee 100 melaporkan harga 9,77e4 sementara dua
/// pool yang benar benar likuid melaporkan 2,63e3. Selisihnya 37 kali lipat.
///
/// PEMERINGKATAN
/// Untuk sekarang urutannya memakai `fee x likuiditas` sebagai pendekatan
/// kasar. Peringkat yang sebenarnya, yaitu proyeksi fee per dolar modal seperti
/// di PRD 11.5, butuh data volume dari gmgn dan baru dipasang di fase 6.
pub fn usable_pools(mut pools: Vec<PoolInfo>) -> Vec<PoolInfo> {
    pools.retain(|p| {
        let likuid = p.liquidity.parse::<u128>().unwrap_or(0) > 0;
        if !likuid {
            tracing::debug!(
                pool = %p.id,
                fee = p.fee,
                "pool tanpa likuiditas dilewati, harganya tidak bisa dipercaya"
            );
            return false;
        }

        // Pool berfee mustahil dibuang, BUKAN sekadar diturunkan peringkatnya.
        // Tanpa ini, pool fee 80 persen justru menang karena skornya
        // likuiditas dikali fee. Lihat MAX_PLAUSIBLE_FEE.
        if implausible_fee(p) {
            tracing::warn!(
                pool = %p.id,
                fee_persen = p.fee as f64 / 10_000.0,
                "pool dengan fee di luar akal sehat dibuang"
            );
            return false;
        }

        p.raw_price.is_finite() && p.raw_price > 0.0
    });

    pools.sort_by(|a, b| {
        let skor = |p: &PoolInfo| {
            let l = p.liquidity.parse::<f64>().unwrap_or(0.0);
            l * ranking_fee(p) as f64
        };
        skor(b)
            .partial_cmp(&skor(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    pools
}

/// Fee yang dipakai untuk memeringkat.
///
/// Untuk pool dynamic fee, angka fee di PoolKey adalah penanda, bukan besaran.
/// Memakainya apa adanya akan membacanya sebagai 838 persen dan menaruh pool
/// itu di puncak daftar tanpa alasan.
pub fn ranking_fee(p: &PoolInfo) -> u32 {
    if p.dynamic_fee {
        ASSUMED_DYNAMIC_FEE
    } else {
        p.fee
    }
}

/// Benar kalau pool memakai hook selain alamat nol.
///
/// Hook bisa mengubah fee, membatasi siapa yang boleh menambah likuiditas, atau
/// memungut potongan. Keputusan boleh tidaknya dipakai ada di gate risiko,
/// bukan di sini. Lihat PRD 5.4.
pub fn has_hook(p: &PoolInfo) -> bool {
    p.hooks
        .as_deref()
        .is_some_and(|h| !h.eq_ignore_ascii_case(ZERO_ADDRESS))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(id: &str, fee: u32, liquidity: &str, raw_price: f64) -> PoolInfo {
        PoolInfo {
            version: PoolVersion::V3,
            id: id.into(),
            token0: "0x0".into(),
            token1: "0x1".into(),
            fee,
            dynamic_fee: false,
            tick_spacing: 60,
            hooks: None,
            sqrt_price_x96: "0".into(),
            tick: 0,
            liquidity: liquidity.into(),
            raw_price,
        }
    }

    #[test]
    fn pool_kosong_dibuang() {
        // Angka diambil dari hasil nyata PONS/WETH.
        let pools = vec![
            pool("a", 100, "0", 9.772e4),
            pool("b", 500, "0", 6.592e3),
            pool("c", 3_000, "252442060701674951515456", 2.633e3),
            pool("d", 10_000, "206048103769302921694521", 2.637e3),
        ];
        let hasil = usable_pools(pools);
        assert_eq!(hasil.len(), 2, "hanya dua pool yang punya likuiditas");
        assert!(hasil.iter().all(|p| p.id == "c" || p.id == "d"));
    }

    #[test]
    fn pool_diurutkan_dari_yang_paling_menjanjikan() {
        let pools = vec![
            pool("kecil", 3_000, "1000", 1.0),
            pool("besar", 3_000, "1000000", 1.0),
        ];
        let hasil = usable_pools(pools);
        assert_eq!(hasil[0].id, "besar");
    }

    #[test]
    fn fee_tier_lebih_tinggi_menang_pada_likuiditas_sama() {
        // Pada likuiditas sama, pool dengan fee lebih tinggi membayar lebih
        // banyak per unit volume.
        let pools = vec![
            pool("murah", 500, "1000", 1.0),
            pool("mahal", 10_000, "1000", 1.0),
        ];
        let hasil = usable_pools(pools);
        assert_eq!(hasil[0].id, "mahal");
    }

    #[test]
    fn penanda_dynamic_fee_tidak_dibaca_sebagai_besaran() {
        // Angka nyata dari pool V4 di Robinhood Chain. Tanpa penanganan khusus,
        // 8388608 akan dibaca sebagai 838 persen dan memenangi seluruh daftar.
        let mut dinamis = pool("dinamis", DYNAMIC_FEE_FLAG, "1000", 1.0);
        dinamis.dynamic_fee = true;
        assert_eq!(ranking_fee(&dinamis), ASSUMED_DYNAMIC_FEE);

        let tetap = pool("tetap", 10_000, "1000", 1.0);
        assert_eq!(ranking_fee(&tetap), 10_000);

        // Pool fee tetap 1 persen harus mengalahkan pool dynamic fee pada
        // likuiditas yang sama, karena asumsinya hanya 0,3 persen.
        let hasil = usable_pools(vec![dinamis, tetap]);
        assert_eq!(hasil[0].id, "tetap");
    }

    #[test]
    fn pool_berfee_mustahil_dibuang_bukan_dimenangkan() {
        // Angka nyata dari pemindaian FIG di Robinhood Chain. Tanpa penyaringan
        // ini, pool 80 persen menang telak karena skornya likuiditas dikali fee,
        // dan ia akan muncul di puncak daftar tepat di tempat user memilih.
        let jebakan = pool("jebakan", 802_700, "12581485599219836851", 1.0);
        let wajar = pool("wajar", 80_000, "223191483590178319", 1.0);

        assert!(implausible_fee(&jebakan));
        assert!(!implausible_fee(&wajar), "8 persen masih wajar di chain ini");

        let hasil = usable_pools(vec![jebakan, wajar]);
        assert_eq!(hasil.len(), 1);
        assert_eq!(hasil[0].id, "wajar");
    }

    #[test]
    fn pool_dynamic_fee_tidak_ikut_disaring_batas_fee() {
        // Penanda dynamic fee nilainya 8388608, jauh di atas ambang, tapi itu
        // bukan besaran fee. Menyaringnya akan membuang seluruh pool dinamis.
        let mut dinamis = pool("dinamis", DYNAMIC_FEE_FLAG, "1000", 1.0);
        dinamis.dynamic_fee = true;
        assert!(!implausible_fee(&dinamis));
        assert_eq!(usable_pools(vec![dinamis]).len(), 1);
    }

    #[test]
    fn batas_fee_tepat_di_ambang() {
        assert!(!implausible_fee(&pool("a", MAX_PLAUSIBLE_FEE, "1", 1.0)));
        assert!(implausible_fee(&pool("b", MAX_PLAUSIBLE_FEE + 1, "1", 1.0)));
    }

    #[test]
    fn pool_fee_nol_dibuang_meski_likuiditasnya_raksasa() {
        // Angka nyata dari FIG: fee nol dengan likuiditas 2,8e23, jauh lebih
        // besar daripada pool mana pun yang sah. Tanpa penyaringan, ia menang
        // telak padahal tidak membayar apa apa ke LP.
        let nol = pool("nol", 0, "282361371356284006478705", 1.0);
        let wajar = pool("wajar", 80_000, "223191483590178319", 1.0);

        assert!(implausible_fee(&nol));
        let hasil = usable_pools(vec![nol, wajar]);
        assert_eq!(hasil.len(), 1);
        assert_eq!(hasil[0].id, "wajar");
    }

    #[test]
    fn hook_dikenali_dari_alamat() {
        let mut tanpa = pool("tanpa", 3_000, "1", 1.0);
        assert!(!has_hook(&tanpa), "hooks None berarti tanpa hook");

        tanpa.hooks = Some(ZERO_ADDRESS.to_string());
        assert!(!has_hook(&tanpa), "alamat nol berarti tanpa hook");

        tanpa.hooks = Some("0x2380aBf72C17aABAb76480244759AC7E2932EEcC".into());
        assert!(has_hook(&tanpa));

        // Huruf besar kecil tidak boleh berpengaruh pada perbandingan alamat.
        tanpa.hooks = Some(ZERO_ADDRESS.to_uppercase().replace("0X", "0x"));
        assert!(!has_hook(&tanpa));
    }

    #[test]
    fn harga_tidak_masuk_akal_dibuang() {
        let pools = vec![
            pool("nan", 3_000, "1000", f64::NAN),
            pool("nol", 3_000, "1000", 0.0),
            pool("baik", 3_000, "1000", 1.5),
        ];
        let hasil = usable_pools(pools);
        assert_eq!(hasil.len(), 1);
        assert_eq!(hasil[0].id, "baik");
    }

    #[test]
    fn harga_dari_sqrt_price_x96() {
        // Harga 1:1 berarti sqrtPriceX96 = 2^96.
        let satu = U256::from(1u8) << 96;
        let p = raw_price_from_sqrt_x96(satu);
        assert!((p - 1.0).abs() < 1e-9, "harga 1:1 seharusnya 1.0, dapat {p}");

        // Harga 4:1 berarti sqrt-nya 2, jadi 2 * 2^96.
        let empat = U256::from(2u8) << 96;
        let p = raw_price_from_sqrt_x96(empat);
        assert!((p - 4.0).abs() < 1e-9, "harga seharusnya 4.0, dapat {p}");
    }

    #[test]
    fn sqrt_price_besar_tidak_meluap() {
        // Nilai mendekati batas atas Uniswap, jauh melampaui u128. Lewat f64
        // hasilnya tetap terhingga, bukan panic atau NaN.
        let besar = U256::from(1u8) << 158;
        let p = raw_price_from_sqrt_x96(besar);
        assert!(p.is_finite(), "harga dari sqrt besar harus terhingga, dapat {p}");
        assert!(p > 0.0);
    }

    #[test]
    fn fee_tier_baku_punya_tick_spacing_yang_dikenal() {
        use crate::math::v3_tick_spacing;
        for fee in V3_FEE_TIERS {
            assert!(
                v3_tick_spacing(fee).is_some(),
                "fee tier {fee} harus punya tick spacing yang dikenal"
            );
        }
    }

    fn pool_pasangan(token0: &str, token1: &str) -> PoolInfo {
        let mut p = pool("x", 3000, "1", 1.0);
        p.token0 = token0.into();
        p.token1 = token1.into();
        p
    }

    #[test]
    fn lawan_dihitung_dan_diurutkan_dari_yang_terbanyak() {
        let target = "0xdc4dd8839f02f2242e9552fe4652635fb7f95cc6";
        let usdg = "0x5fc5360d0400a0fd4f2af552add042d716f1d168";
        let eth = "0x0000000000000000000000000000000000000000";

        let pools = vec![
            pool_pasangan(usdg, target),
            pool_pasangan(target, eth),
            pool_pasangan(usdg, target),
            pool_pasangan(usdg, target),
        ];

        let lawan = counterparties(&pools, target);
        assert_eq!(lawan.len(), 2);
        assert_eq!(
            lawan[0],
            (usdg.to_string(), 3),
            "yang terbanyak harus di depan"
        );
        assert_eq!(lawan[1], (eth.to_string(), 1));
    }

    #[test]
    fn lawan_tidak_peduli_besar_kecil_huruf() {
        // Alamat datang dari dua sumber dengan gaya penulisan berbeda: log
        // memberi huruf kecil, sedangkan tipe Address mencetak checksum
        // campuran. Kalau perbandingannya sensitif huruf, token yang dicari
        // tidak akan pernah cocok dan hasilnya kosong tanpa pesan salah apa
        // pun, yang persis bentuk bug pasangan WETH.
        let target = "0xdC4dd8839f02F2242e9552FE4652635fb7f95Cc6";
        let usdg = "0x5fc5360d0400a0fd4f2af552add042d716f1d168";
        let pools = vec![pool_pasangan(usdg, &target.to_lowercase())];

        let lawan = counterparties(&pools, target);
        assert_eq!(
            lawan.len(),
            1,
            "alamat yang sama dengan huruf berbeda harus tetap cocok"
        );
        assert_eq!(lawan[0].0, usdg);
    }

    #[test]
    fn pool_yang_tidak_menyentuh_token_diabaikan() {
        let target = "0xdc4dd8839f02f2242e9552fe4652635fb7f95cc6";
        let pools = vec![pool_pasangan("0xaaa", "0xbbb")];
        assert!(counterparties(&pools, target).is_empty());
    }

    /// Uji terhadap chain sungguhan.
    ///
    /// Ditandai `ignore` supaya tidak ikut `cargo test` biasa, karena hasilnya
    /// bergantung pada RPC publik yang bisa lambat atau membatasi laju.
    /// Jalankan manual dengan `cargo test -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "menyentuh jaringan, jalankan dengan --ignored"]
    async fn menemukan_pool_di_chain_asli() {
        use crate::chain::ChainClient;
        use crate::config::{repo_root, Config};

        let registry = Registry::load_from(&Registry::default_path(&repo_root())).unwrap();
        let client = ChainClient::new(Config::default().rpc);
        let provider = client.provider().await.expect("provider");

        // PONS, token nyata di Robinhood Chain yang punya pool Uniswap V3.
        let token: Address = "0x39dbed3a2bd333467115de45665cc57f813c4571"
            .parse()
            .unwrap();
        let weth: Address = registry.tokens["weth"].address.parse().unwrap();

        let meta = token_meta(&provider, token).await.expect("metadata token");
        println!("token  {} {} desimal, suplai {:.0}", meta.symbol, meta.decimals, meta.total_supply);
        assert!(meta.decimals > 0, "desimal harus masuk akal");
        assert!(meta.total_supply > 0.0, "suplai harus positif");

        let pools = discover_all(&provider, &registry, token, weth).await;
        for p in &pools {
            println!(
                "pool   {:?} fee {} spacing {} tick {} likuiditas {} harga {:.3e} hook {}",
                p.version,
                p.fee,
                p.tick_spacing,
                p.tick,
                p.liquidity,
                p.raw_price,
                p.hooks.as_deref().unwrap_or("-")
            );
        }

        assert!(!pools.is_empty(), "pasangan ini harus punya minimal satu pool");
        for p in &pools {
            assert!(p.raw_price > 0.0, "harga mentah harus positif");
            assert!(p.tick_spacing > 0, "tick spacing harus positif");
        }

        // Verifikasi tick math kita terhadap keadaan pool yang sebenarnya.
        //
        // Kontrak Uniswap menjamin tick pada slot0 adalah tick terbesar yang
        // sqrtPriceX96-nya tidak melampaui harga sekarang. Kalau tabel
        // konstanta TickMath kita salah di satu bit saja, ketaksamaan ini akan
        // gagal pada pool yang ticknya menyentuh bit itu. Ini pemeriksaan
        // independen terhadap data produksi, bukan terhadap asumsi kita
        // sendiri.
        use crate::math::tick_math::{get_sqrt_ratio_at_tick, get_tick_at_sqrt_ratio};
        use alloy::primitives::U256;

        for p in &pools {
            let sqrt: U256 = p.sqrt_price_x96.parse().expect("sqrtPriceX96 harus angka");
            let bawah = get_sqrt_ratio_at_tick(p.tick).expect("tick pool harus sah");
            let atas = get_sqrt_ratio_at_tick(p.tick + 1).expect("tick berikutnya harus sah");

            assert!(
                bawah <= sqrt && sqrt < atas,
                "pool {} tick {}: sqrtPriceX96 {} tidak berada di antara {} dan {}",
                p.id,
                p.tick,
                sqrt,
                bawah,
                atas
            );

            // Dan sebaliknya, pembacaan balik harus menghasilkan tick yang sama.
            assert_eq!(
                get_tick_at_sqrt_ratio(sqrt).expect("harga pool harus dalam rentang"),
                p.tick,
                "pool {} membaca balik tick yang berbeda",
                p.id
            );
        }
        println!("tick math cocok dengan {} pool sungguhan di chain", pools.len());
    }

    #[test]
    fn urutan_currency_mengikuti_alamat() {
        use alloy::primitives::address;
        let kecil = address!("0000000000000000000000000000000000000001");
        let besar = address!("ffffffffffffffffffffffffffffffffffffffff");
        // Logika yang sama dipakai discover_v4 saat menyusun topic.
        let (c0, c1) = if kecil < besar { (kecil, besar) } else { (besar, kecil) };
        assert_eq!(c0, kecil);
        assert_eq!(c1, besar);
    }
}
