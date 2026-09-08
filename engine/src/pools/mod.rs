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
pub mod cache;
pub mod distribusi;
pub mod kandidat;
pub mod scan;
pub mod volume;

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
    implausible_fee_raw(p.fee)
}

/// Sama seperti `implausible_fee`, tapi bekerja langsung dari angka fee.
///
/// Dipakai sebelum PoolInfo terbentuk, yaitu saat memutuskan pool mana yang
/// perlu dibaca keadaannya. Fee adalah bagian dari PoolKey dan tidak pernah
/// berubah, jadi pool berfee mustahil tidak akan pernah menjadi layak, dan
/// membacanya hanya memperlambat tanpa mengubah hasil apa pun.
pub fn implausible_fee_raw(fee: u32) -> bool {
    if fee & DYNAMIC_FEE_FLAG != 0 {
        return false;
    }
    fee == 0 || fee > MAX_PLAUSIBLE_FEE
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
    /// Angka yang membantu memilih pool, kalau sudah terukur.
    ///
    /// `None` berarti belum diukur, bukan berarti nol. Pool yang volumenya
    /// benar benar nol tetap punya `Some` dengan volume nol, dan keduanya
    /// menuntun ke keputusan yang berbeda.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats: Option<PoolStats>,
}

/// Angka pembanding antar pool pada token yang sama.
///
/// Seluruhnya dinyatakan dalam token kutipan pool itu, dengan alasan yang sama
/// seperti di `kandidat`: satuan terkecil menghasilkan angka yang terbaca
/// seperti dolar padahal bukan, dan kurs dolar yang ditebak memasukkan
/// kesalahan ke angka yang seharusnya murni hasil penjumlahan.
#[derive(Debug, Clone, Serialize)]
pub struct PoolStats {
    /// Volume pada jendela yang diukur, dalam kutipan.
    ///
    /// `None` berarti belum bisa diukur, bukan nol. Pengukuran volume membaca
    /// event `Swap` PoolManager V4 dan menyaringnya menurut poolId, dan pool V3
    /// tidak punya keduanya. Nol berarti benar benar tidak ada perdagangan,
    /// dan itu kesimpulan yang sangat berbeda.
    pub volume: Option<f64>,
    /// Lamanya jendela itu, dalam menit. Volume tanpa jendela tidak berarti.
    pub jendela_menit: Option<f64>,
    /// Seluruh modal yang tertanam di pool, dalam kutipan.
    ///
    /// Dijumlahkan dari distribusi likuiditas, bukan ditaksir dari likuiditas
    /// di satu tick. Taksiran seperti itu pernah melaporkan 0,82 untuk pool
    /// yang isinya kira kira 360, karena likuiditas berubah di setiap tick yang
    /// terinisialisasi dan satu titik tidak mewakili seluruhnya.
    pub tvl: f64,
    /// Proyeksi fee setahun terhadap modal acuan, dalam persen.
    ///
    /// `None` untuk fee dinamis, yang besarannya baru diketahui saat swap
    /// terjadi, dan untuk pool yang kedalamannya tidak terbaca.
    pub apr: Option<f64>,
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

/// Baca metadata beberapa token sekaligus dalam satu permintaan.
///
/// Hasilnya sejajar dengan masukan. Token yang tidak menjawab `decimals()`
/// dikembalikan sebagai `None`, karena tanpa desimal seluruh perhitungan harga
/// menjadi salah dan menebaknya 18 lebih berbahaya daripada mengaku tidak tahu.
///
/// Simbol boleh gagal. Banyak memecoin memakai `bytes32` alih alih `string`,
/// dan itu bukan alasan untuk menolak tokennya.
/// Baca saldo dua token sekaligus untuk satu pemilik.
///
/// Satu batch, karena dua permintaan berurutan ke RPC publik dua kali lebih
/// lambat tanpa alasan. Saldo yang tidak terbaca dikembalikan sebagai `None`,
/// bukan nol: nol berarti user memang tidak punya token itu, dan itu kesimpulan
/// yang berbeda dari tidak bisa membaca.
pub async fn saldo_dua<P: Provider>(
    provider: &P,
    registry: &Registry,
    pemilik: Address,
    a: Address,
    b: Address,
) -> (Option<U256>, Option<U256>) {
    let Ok(multicall) = registry.contract("common", "multicall3") else {
        return (None, None);
    };

    let calls = vec![
        crate::chain::multicall::encode(a, &abi::IERC20::balanceOfCall { account: pemilik }),
        crate::chain::multicall::encode(b, &abi::IERC20::balanceOfCall { account: pemilik }),
    ];

    let Ok(hasil) = crate::chain::multicall::batch_call(provider, multicall, calls).await else {
        return (None, None);
    };

    let baca = |i: usize| {
        crate::chain::multicall::decode::<abi::IERC20::balanceOfCall>(
            hasil.get(i).and_then(|x| x.as_ref()),
        )
    };
    (baca(0), baca(1))
}

pub async fn token_meta_batch<P: Provider>(
    provider: &P,
    registry: &Registry,
    alamat: &[Address],
) -> Result<Vec<Option<TokenMeta>>> {
    if alamat.is_empty() {
        return Ok(Vec::new());
    }
    let multicall = registry.contract("common", "multicall3")?;

    let mut calls = Vec::with_capacity(alamat.len() * 3);
    for a in alamat {
        calls.push(crate::chain::multicall::encode(
            *a,
            &abi::IERC20::decimalsCall {},
        ));
        calls.push(crate::chain::multicall::encode(
            *a,
            &abi::IERC20::totalSupplyCall {},
        ));
        calls.push(crate::chain::multicall::encode(
            *a,
            &abi::IERC20::symbolCall {},
        ));
    }

    let hasil = crate::chain::multicall::batch_call(provider, multicall, calls).await?;

    let mut out = Vec::with_capacity(alamat.len());
    for (i, a) in alamat.iter().enumerate() {
        let decimals = crate::chain::multicall::decode::<abi::IERC20::decimalsCall>(
            hasil.get(i * 3).and_then(|x| x.as_ref()),
        );
        let Some(decimals) = decimals else {
            out.push(None);
            continue;
        };
        let supply = crate::chain::multicall::decode::<abi::IERC20::totalSupplyCall>(
            hasil.get(i * 3 + 1).and_then(|x| x.as_ref()),
        );
        let symbol = crate::chain::multicall::decode::<abi::IERC20::symbolCall>(
            hasil.get(i * 3 + 2).and_then(|x| x.as_ref()),
        )
        .unwrap_or_else(|| "?".to_string());

        let divisor = 10f64.powi(decimals as i32);
        let total_supply = supply
            .map(|s| s.to_string().parse::<f64>().unwrap_or(f64::NAN) / divisor)
            .unwrap_or(f64::NAN);

        out.push(Some(TokenMeta {
            address: a.to_string(),
            symbol,
            decimals,
            total_supply,
        }));
    }

    Ok(out)
}

/// Baca metadata satu token.
///
/// Lewat jalur batch supaya hanya ada satu tempat yang tahu cara membaca ERC20,
/// dan supaya biayanya satu round trip, bukan tiga.
pub async fn token_meta_via<P: Provider>(
    provider: &P,
    registry: &Registry,
    address: Address,
) -> Result<TokenMeta> {
    let mut hasil = token_meta_batch(provider, registry, &[address]).await?;
    hasil
        .pop()
        .flatten()
        .with_context(|| format!("{address} tidak menjawab decimals(), apakah ini ERC20?"))
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
    discover_v3_multi(provider, registry, token_a, &[token_b]).await
}

/// Cari pool V3 untuk satu token terhadap banyak token kutipan sekaligus.
///
/// KENAPA VERSI BANYAK SEKALIGUS
/// Versi lama memanggil `getPool` empat kali lalu membaca lima properti per
/// pool, semuanya berurutan. Untuk dua token kutipan itu sudah 8 sampai 30
/// permintaan hanya untuk V3, dan pada RPC publik dengan latensi 300 ms artinya
/// beberapa detik yang sepenuhnya bisa dihindari.
///
/// Sekarang seluruh pencarian muat dalam dua permintaan: satu untuk menanyakan
/// alamat semua pool, satu lagi untuk membaca keadaan pool yang ternyata ada.
pub async fn discover_v3_multi<P: Provider>(
    provider: &P,
    registry: &Registry,
    token: Address,
    quotes: &[Address],
) -> Result<Vec<PoolInfo>> {
    if quotes.is_empty() {
        return Ok(Vec::new());
    }

    let factory = registry.contract("uniswap_v3", "factory")?;
    let multicall = registry.contract("common", "multicall3")?;

    let mut tanya = Vec::with_capacity(quotes.len() * V3_FEE_TIERS.len());
    let mut label: Vec<u32> = Vec::with_capacity(tanya.capacity());
    for quote in quotes {
        for fee in V3_FEE_TIERS {
            tanya.push(crate::chain::multicall::encode(
                factory,
                &abi::IUniswapV3Factory::getPoolCall {
                    tokenA: token,
                    tokenB: *quote,
                    // `uint24` di ABI memetakan ke U24, bukan u32.
                    fee: alloy::primitives::aliases::U24::from(fee),
                },
            ));
            label.push(fee);
        }
    }

    let jawaban = crate::chain::multicall::batch_call(provider, multicall, tanya).await?;

    let mut ada: Vec<(Address, u32)> = Vec::new();
    for (i, fee) in label.into_iter().enumerate() {
        let Some(alamat) = crate::chain::multicall::decode::<abi::IUniswapV3Factory::getPoolCall>(
            jawaban.get(i).and_then(|x| x.as_ref()),
        ) else {
            continue;
        };
        // Alamat nol berarti fee tier itu memang belum pernah dibuat. Keadaan
        // normal, bukan kegagalan.
        if !alamat.is_zero() {
            ada.push((alamat, fee));
        }
    }

    hydrate_v3(provider, multicall, ada).await
}

/// Baca keadaan setiap pool V3 dalam satu permintaan.
///
/// Lima properti per pool, semuanya dalam batch yang sama. Pool yang salah satu
/// propertinya tidak terbaca dilewati seluruhnya, karena PoolInfo yang separuh
/// terisi lebih berbahaya daripada pool yang tidak ditampilkan: harga nol akan
/// lolos penyaring lalu muncul di panel sebagai angka yang terlihat sah.
async fn hydrate_v3<P: Provider>(
    provider: &P,
    multicall: Address,
    pools: Vec<(Address, u32)>,
) -> Result<Vec<PoolInfo>> {
    if pools.is_empty() {
        return Ok(Vec::new());
    }

    const PER_POOL: usize = 5;
    let mut calls = Vec::with_capacity(pools.len() * PER_POOL);
    for (alamat, _) in &pools {
        let a = *alamat;
        calls.push(crate::chain::multicall::encode(
            a,
            &abi::IUniswapV3Pool::slot0Call {},
        ));
        calls.push(crate::chain::multicall::encode(
            a,
            &abi::IUniswapV3Pool::liquidityCall {},
        ));
        calls.push(crate::chain::multicall::encode(
            a,
            &abi::IUniswapV3Pool::token0Call {},
        ));
        calls.push(crate::chain::multicall::encode(
            a,
            &abi::IUniswapV3Pool::token1Call {},
        ));
        calls.push(crate::chain::multicall::encode(
            a,
            &abi::IUniswapV3Pool::tickSpacingCall {},
        ));
    }

    let hasil = crate::chain::multicall::batch_call(provider, multicall, calls).await?;
    let ambil = |i: usize| hasil.get(i).and_then(|x| x.as_ref());

    let mut out = Vec::with_capacity(pools.len());
    for (n, (alamat, fee)) in pools.into_iter().enumerate() {
        let dasar = n * PER_POOL;
        let (Some(slot0), Some(liquidity), Some(token0), Some(token1), Some(tick_spacing)) = (
            crate::chain::multicall::decode::<abi::IUniswapV3Pool::slot0Call>(ambil(dasar)),
            crate::chain::multicall::decode::<abi::IUniswapV3Pool::liquidityCall>(ambil(dasar + 1)),
            crate::chain::multicall::decode::<abi::IUniswapV3Pool::token0Call>(ambil(dasar + 2)),
            crate::chain::multicall::decode::<abi::IUniswapV3Pool::token1Call>(ambil(dasar + 3)),
            crate::chain::multicall::decode::<abi::IUniswapV3Pool::tickSpacingCall>(
                ambil(dasar + 4),
            ),
        ) else {
            tracing::warn!(pool = %alamat, "pool V3 tidak terbaca lengkap, dilewati");
            continue;
        };

        let sqrt = U256::from(slot0.sqrtPriceX96);
        out.push(PoolInfo {
            version: PoolVersion::V3,
            id: alamat.to_string(),
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
            stats: None,
        });
    }

    Ok(out)
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
            stats: None,
        });
    }

    Ok(out)
}

/// Seluruh yang panel butuhkan tentang satu token.
#[derive(Debug, Clone)]
pub struct TokenSnapshot {
    pub meta: Option<TokenMeta>,
    pub quotes: Vec<QuoteGroup>,
}

/// Muat metadata token dan seluruh poolnya dengan permintaan sesedikit mungkin.
///
/// KENAPA SATU FUNGSI, BUKAN TIGA PANGGILAN TERPISAH
/// Membaca metadata, menanyakan pool V3, dan membaca keadaan pool V4 adalah
/// tiga hal yang tidak saling bergantung. Dijalankan terpisah, masing masing
/// membayar satu perjalanan bolak balik ke RPC publik, sekitar 350 ms setiap
/// kali, dan panel menunggu lebih dari satu detik untuk pekerjaan yang
/// sebenarnya bisa selesai dalam satu permintaan.
///
/// Multicall3 mengeksekusi semuanya di dalam satu `eth_call`, jadi urutannya
/// tidak lagi berarti biaya. Yang tersisa hanya satu permintaan lanjutan, dan
/// itu pun hanya kalau pool V3 benar benar ada.
///
/// Pada mode `CacheSaja` fungsi ini tidak menyentuh log sama sekali, sehingga
/// seluruh pemuatan ulang sebuah token yang pernah dibuka berbiaya satu
/// permintaan.
pub async fn muat_token<P: Provider>(
    provider: &P,
    registry: &Registry,
    store: Option<&crate::store::Store>,
    token: Address,
    mode: ModePencarian,
) -> Result<TokenSnapshot> {
    let state_view = registry.contract("uniswap_v4", "state_view")?;
    let multicall = registry.contract("common", "multicall3")?;
    let factory_v3 = registry.contract("uniswap_v3", "factory")?;
    let alamat_token = token.to_string();

    // Pemindaian log hanya terjadi pada mode penuh, dan hasilnya langsung
    // digabung ke cache. Setelah ini, kunci pool untuk kedua mode sama saja.
    let kunci = match mode {
        ModePencarian::CacheSaja => store
            .map(|s| cache::baca(s, &alamat_token).keys)
            .unwrap_or_default(),
        ModePencarian::Penuh => pindai_dan_gabung(provider, registry, store, token).await?,
    };

    let quotes: Vec<Address> = registry
        .tokens
        .values()
        .filter_map(|e| {
            let a = e.address.parse::<Address>().ok()?;
            (a != token).then_some(a)
        })
        .collect();

    // Satu batch untuk tiga urusan sekaligus. Urutannya dicatat lewat penanda
    // di bawah, bukan dihitung ulang di tempat lain, supaya menambah panggilan
    // baru tidak diam diam menggeser indeks yang sudah ada.
    let mut calls = Vec::new();

    let awal_meta = calls.len();
    calls.push(crate::chain::multicall::encode(
        token,
        &abi::IERC20::decimalsCall {},
    ));
    calls.push(crate::chain::multicall::encode(
        token,
        &abi::IERC20::totalSupplyCall {},
    ));
    calls.push(crate::chain::multicall::encode(
        token,
        &abi::IERC20::symbolCall {},
    ));

    let awal_v3 = calls.len();
    let mut label_v3: Vec<u32> = Vec::new();
    for quote in &quotes {
        for fee in V3_FEE_TIERS {
            calls.push(crate::chain::multicall::encode(
                factory_v3,
                &abi::IUniswapV3Factory::getPoolCall {
                    tokenA: token,
                    tokenB: *quote,
                    fee: alloy::primitives::aliases::U24::from(fee),
                },
            ));
            label_v3.push(fee);
        }
    }

    let awal_v4 = calls.len();
    let mut kunci_terpakai: Vec<cache::PoolKeyRecord> = Vec::with_capacity(kunci.len());
    let mut dilewati_fee = 0usize;
    for k in kunci {
        // Fee bagian dari PoolKey dan tidak pernah berubah, jadi pool berfee
        // mustahil tidak akan pernah menjadi layak. Membacanya hanya menambah
        // dua panggilan per pool tanpa mengubah hasil.
        if implausible_fee_raw(k.fee) {
            dilewati_fee += 1;
            continue;
        }
        let Ok(id) = k.id.parse::<FixedBytes<32>>() else {
            tracing::warn!(id = %k.id, "poolId di cache tidak berbentuk bytes32, dilewati");
            continue;
        };
        calls.push(crate::chain::multicall::encode(
            state_view,
            &abi::IStateView::getSlot0Call { poolId: id },
        ));
        calls.push(crate::chain::multicall::encode(
            state_view,
            &abi::IStateView::getLiquidityCall { poolId: id },
        ));
        kunci_terpakai.push(k);
    }

    if dilewati_fee > 0 {
        tracing::debug!(
            dilewati_fee,
            "pool berfee mustahil tidak dibaca keadaannya"
        );
    }

    let jam = std::time::Instant::now();
    let hasil = crate::chain::multicall::batch_call(provider, multicall, calls).await?;
    tracing::debug!(
        ms = jam.elapsed().as_millis(),
        panggilan = hasil.len(),
        "batch gabungan selesai"
    );

    let ambil = |i: usize| hasil.get(i).and_then(|x| x.as_ref());

    // Metadata. Tanpa desimal, seluruh perhitungan harga salah, jadi token
    // seperti itu diperlakukan sebagai bukan ERC20 alih alih ditebak 18.
    let meta = crate::chain::multicall::decode::<abi::IERC20::decimalsCall>(ambil(awal_meta)).map(
        |decimals| {
            let supply = crate::chain::multicall::decode::<abi::IERC20::totalSupplyCall>(
                ambil(awal_meta + 1),
            );
            let symbol =
                crate::chain::multicall::decode::<abi::IERC20::symbolCall>(ambil(awal_meta + 2))
                    .unwrap_or_else(|| "?".to_string());
            let divisor = 10f64.powi(decimals as i32);
            TokenMeta {
                address: alamat_token.clone(),
                symbol,
                decimals,
                total_supply: supply
                    .map(|s| s.to_string().parse::<f64>().unwrap_or(f64::NAN) / divisor)
                    .unwrap_or(f64::NAN),
            }
        },
    );

    // Pool V3 yang ternyata ada. Alamat nol berarti fee tier itu memang belum
    // pernah dibuat, keadaan normal.
    let mut v3_ada: Vec<(Address, u32)> = Vec::new();
    for (i, fee) in label_v3.into_iter().enumerate() {
        let Some(alamat) = crate::chain::multicall::decode::<abi::IUniswapV3Factory::getPoolCall>(
            ambil(awal_v3 + i),
        ) else {
            continue;
        };
        if !alamat.is_zero() {
            v3_ada.push((alamat, fee));
        }
    }

    let mut semua = baca_v4_dari_batch(&hasil, awal_v4, kunci_terpakai);

    // Permintaan kedua hanya kalau ada yang perlu dibaca. Pada rantai ini V3
    // umumnya kosong, jadi biaya ini biasanya nol.
    if !v3_ada.is_empty() {
        match hydrate_v3(provider, multicall, v3_ada).await {
            Ok(mut v) => semua.append(&mut v),
            Err(e) => tracing::warn!(error = %e, "pembacaan pool V3 gagal"),
        }
    }

    // Token lawan yang tidak ada di chain.toml dibaca on chain, bukan
    // ditebak. Salinan FIG di 0x41F4... berpasangan dengan tiga puluh token
    // berbeda, dan menganggap semuanya berdesimal 18 akan menampilkan harga
    // yang meleset sepuluh pangkat selisihnya untuk setiap pasangan yang
    // ternyata memakai 6.
    let dikenal = kenali_lawan_asing(provider, registry, token, &semua).await;

    let mut quotes = kelompokkan(semua, token, registry, &dikenal);

    // Angka pembanding diukur untuk kelompok kutipan teratas saja. Kelompok
    // lain jarang dibuka, dan mengukur semuanya berarti satu permintaan log
    // tambahan per kelompok pada jalur yang harus terasa seketika.
    //
    // Tahap cepat dilewati sama sekali. Ia ada supaya panel punya sesuatu
    // untuk ditampilkan dalam beberapa ratus milidetik, dan menambahkan
    // pemindaian log di sana menghapus alasan keberadaannya.
    if mode == ModePencarian::Penuh {
        if let (Some(m), Some(k)) = (meta.as_ref(), quotes.first_mut()) {
            ukur_stats(provider, registry, m, k).await;
        }
    }

    Ok(TokenSnapshot { meta, quotes })
}

/// Isi `stats` untuk seluruh pool dalam satu kelompok kutipan.
///
/// Kegagalan tidak menggagalkan pemuatan token: `stats` tetap `None`, dan panel
/// menampilkan garis alih alih angka. Angka nol akan terbaca sebagai pool yang
/// sepi, kesimpulan yang sangat berbeda dari tidak bisa diukur.
pub async fn ukur_stats<P: Provider>(
    provider: &P,
    registry: &Registry,
    meta: &TokenMeta,
    kelompok: &mut QuoteGroup,
) {
    use crate::pools::kandidat;

    if kelompok.pools.is_empty() {
        return;
    }

    // Volume diukur per protokol, karena sumbernya berbeda: V4 memancarkan
    // Swap dari PoolManager dengan poolId sebagai topic, V3 dari kontrak pool
    // masing masing tanpa pengenal apa pun selain alamat log.
    let (v4, v3): (Vec<&PoolInfo>, Vec<&PoolInfo>) = kelompok
        .pools
        .iter()
        .partition(|p| p.version == PoolVersion::V4);

    let mut per_pool: std::collections::HashMap<String, u128> = Default::default();
    let mut per_pool1: std::collections::HashMap<String, u128> = Default::default();
    let mut jendela_detik = 0.0_f64;

    if !v4.is_empty() {
        let ids: Vec<String> = v4.iter().map(|p| p.id.clone()).collect();
        match volume::ukur_volume(provider, registry, &ids).await {
            Ok(v) => {
                jendela_detik = v.jendela_detik(registry.chain.block_time_ms);
                per_pool.extend(v.per_pool);
                per_pool1.extend(v.per_pool1);
            }
            Err(e) => tracing::warn!(error = %e, "volume V4 gagal diukur"),
        }
    }

    if !v3.is_empty() {
        let alamat: Vec<String> = v3.iter().map(|p| p.id.clone()).collect();
        match volume::ukur_volume_v3(provider, registry, &alamat).await {
            Ok(v) => {
                // Jendela keduanya sama panjang kecuali salah satunya
                // menyempit. Yang terpendek yang dipakai, karena volume dari
                // dua jendela berbeda tidak bisa dibandingkan.
                let j = v.jendela_detik(registry.chain.block_time_ms);
                jendela_detik = if jendela_detik == 0.0 {
                    j
                } else {
                    jendela_detik.min(j)
                };
                per_pool.extend(v.per_pool);
                per_pool1.extend(v.per_pool1);
            }
            Err(e) => tracing::warn!(error = %e, "volume V3 gagal diukur"),
        }
    }
    let skala_meme = 10f64.powi(meta.decimals as i32);
    let skala_quote = 10f64.powi(kelompok.quote_decimals as i32);
    // TVL sebenarnya, dijumlahkan dari distribusi seluruh pool sekaligus.
    // Taksiran memakai likuiditas di satu tick saja pernah melaporkan 0,82
    // untuk pool yang isinya kira kira 360, karena likuiditas berubah di setiap
    // tick yang terinisialisasi dan satu titik tidak mewakili seluruhnya.
    let state_view = registry.contract("uniswap_v4", "state_view").ok();

    // Sumber data per tick berbeda menurut protokol pool. Pool yang sumbernya
    // tidak bisa ditentukan dilewati, bukan diberi TVL nol, karena nol berarti
    // pool kosong dan itu kesimpulan yang berbeda.
    let daftar: Vec<(String, distribusi::SumberTick, distribusi::KeadaanPool)> = kelompok
        .pools
        .iter()
        .filter_map(|p| {
            let sumber = match p.version {
                PoolVersion::V4 => distribusi::SumberTick::V4 {
                    state_view: state_view?,
                },
                PoolVersion::V3 => distribusi::SumberTick::V3 {
                    pool: p.id.parse().ok()?,
                },
            };
            Some((
                p.id.clone(),
                sumber,
                distribusi::KeadaanPool {
                    tick_spacing: p.tick_spacing,
                    tick_sekarang: p.tick,
                    likuiditas: p.liquidity.parse().unwrap_or(0),
                    quote_adalah_token1: p.token0.eq_ignore_ascii_case(&meta.address),
                    skala_meme,
                    skala_quote,
                },
            ))
        })
        .collect();

    let tvl_pool = match registry.contract("common", "multicall3") {
        Ok(mc) => distribusi::tvl_beberapa(provider, mc, &daftar).await,
        Err(_) => Default::default(),
    };

    for pool in &mut kelompok.pools {
        let kunci = pool.id.to_lowercase();
        let quote_adalah_token1 = pool.token0.eq_ignore_ascii_case(&meta.address);

        // Jendela nol berarti pengukurannya tidak pernah berjalan, biasanya
        // karena poolnya V3. Melaporkan volume nol di situ akan terbaca sebagai
        // pool yang sepi, padahal ia justru bisa sangat ramai.
        // Jendela nol berarti pengukurannya tidak pernah berjalan. Melaporkan
        // volume nol di situ akan terbaca sebagai pool yang sepi, padahal ia
        // justru bisa sangat ramai.
        let terukur_volume = jendela_detik > 0.0 && per_pool.contains_key(&kunci);
        let volume = terukur_volume.then(|| {
            let mentah = if quote_adalah_token1 {
                per_pool1.get(&kunci).copied().unwrap_or(0)
            } else {
                per_pool.get(&kunci).copied().unwrap_or(0)
            };
            mentah as f64 / skala_quote
        });

        let tvl = tvl_pool.get(&kunci).copied();
    
        let apr = (!pool.dynamic_fee)
            .then_some(())
            .and(tvl)
            .zip(volume)
            .and_then(|(d, v)| kandidat::proyeksi_apr(v, pool.fee, d, jendela_detik));

        pool.stats = Some(PoolStats {
            volume,
            jendela_menit: terukur_volume.then(|| jendela_detik / 60.0),
            tvl: tvl.unwrap_or(0.0),
            apr,
        });
    }
}

/// Baca simbol dan desimal token lawan yang belum dikenal registry.
///
/// Satu permintaan untuk semuanya sekaligus. Kalau pembacaannya gagal, token
/// itu tidak masuk hasil, dan `kelompokkan` akan membuang kelompoknya. Itu
/// disengaja: pasangan yang desimalnya tidak diketahui tidak bisa dihargai
/// dengan benar, dan menampilkan harga salah lebih buruk daripada diam.
async fn kenali_lawan_asing<P: Provider>(
    provider: &P,
    registry: &Registry,
    token: Address,
    pools: &[PoolInfo],
) -> std::collections::HashMap<String, TokenMeta> {
    let t = token.to_string().to_lowercase();
    let mut asing: Vec<Address> = Vec::new();
    let mut terlihat: std::collections::HashSet<String> = Default::default();

    for p in pools {
        let lawan = if p.token0.to_lowercase() == t {
            &p.token1
        } else if p.token1.to_lowercase() == t {
            &p.token0
        } else {
            continue;
        };
        let kecil = lawan.to_lowercase();
        if kecil == ZERO_ADDRESS {
            continue;
        }
        if registry
            .tokens
            .values()
            .any(|e| e.address.eq_ignore_ascii_case(lawan))
        {
            continue;
        }
        if terlihat.insert(kecil) {
            if let Ok(a) = lawan.parse::<Address>() {
                asing.push(a);
            }
        }
    }

    if asing.is_empty() {
        return Default::default();
    }

    match token_meta_batch(provider, registry, &asing).await {
        Ok(hasil) => asing
            .iter()
            .zip(hasil)
            .filter_map(|(a, m)| Some((a.to_string().to_lowercase(), m?)))
            .collect(),
        Err(e) => {
            tracing::warn!(error = %e, jumlah = asing.len(), "token lawan asing gagal dibaca");
            Default::default()
        }
    }
}

/// Pindai log yang belum pernah dilihat lalu gabungkan ke cache.
///
/// Mengembalikan seluruh kunci yang diketahui, lama maupun baru.
async fn pindai_dan_gabung<P: Provider>(
    provider: &P,
    registry: &Registry,
    store: Option<&crate::store::Store>,
    token: Address,
) -> Result<Vec<cache::PoolKeyRecord>> {
    let pool_manager = registry.contract("uniswap_v4", "pool_manager")?;
    let alamat_token = token.to_string();

    let latest = provider
        .get_block_number()
        .await
        .context("gagal membaca blok terakhir")?;

    let tersimpan = store
        .map(|s| cache::baca(s, &alamat_token))
        .unwrap_or_default();

    // Jendela mundur, bukan deploy_block, karena pool yang dicari selalu yang
    // baru. Tidak pernah turun di bawah blok pemasangan protokol.
    let jendela = latest
        .saturating_sub(SCAN_LOOKBACK_DEFAULT)
        .max(registry.chain.deploy_block);

    // Kalau cache masih menyambung dengan jendela sekarang, cukup pindai blok
    // yang belum pernah dilihat. Kalau ia sudah tertinggal jauh, jendela penuh
    // dipindai lagi, karena melompati blok berarti kehilangan pool selamanya.
    let bersambung = tersimpan.scanned_to > 0 && tersimpan.scanned_to + 1 >= jendela;
    let batas_bawah = if bersambung {
        (tersimpan.scanned_to + 1).min(latest)
    } else {
        jendela
    };

    let mut baru: Vec<cache::PoolKeyRecord> = Vec::new();

    if batas_bawah <= latest {
        // KENAPA BERURUTAN, BUKAN BERBARENGAN
        // Dua pemindaian ini pernah dijalankan berbarengan, dan hasilnya justru
        // lebih lambat. RPC publik membalas 429 untuk yang kedua, lalu lapisan
        // percobaan ulang menunggu sebelum mengulang. Diukur langsung pada
        // jendela tiga juta blok:
        //
        //   berurutan   1102 ms, keduanya berhasil
        //   berbarengan  658 ms untuk yang lolos, yang satunya kena 429 lalu
        //                diulang, sehingga totalnya sekitar 5 detik
        //
        // Rantai ini memberi satu permintaan berat pada satu waktu. Menghormati
        // itu lebih cepat daripada melawannya.
        let jam = std::time::Instant::now();
        let sebagai0 =
            scan_initialize_logs(provider, pool_manager, token, true, batas_bawah, latest).await?;
        let sebagai1 =
            scan_initialize_logs(provider, pool_manager, token, false, batas_bawah, latest).await?;
        tracing::debug!(
            ms = jam.elapsed().as_millis(),
            blok = latest - batas_bawah + 1,
            inkremental = bersambung,
            "pemindaian log selesai"
        );

        for log in sebagai0.into_iter().chain(sebagai1) {
            let Ok(decoded) = abi::Initialize::decode_log(&log.inner) else {
                continue;
            };
            baru.push(cache::PoolKeyRecord {
                id: format!("0x{}", hex::encode(decoded.id)),
                currency0: decoded.currency0.to_string(),
                currency1: decoded.currency1.to_string(),
                fee: decoded.fee.to::<u32>(),
                tick_spacing: decoded.tickSpacing.as_i32(),
                hooks: decoded.hooks.to_string(),
            });
        }
    }

    // `scanned_to` hanya maju kalau pemindaiannya benar benar selesai. Kalau
    // ada yang gagal, fungsi ini sudah kembali dengan error di atas.
    Ok(match store {
        Some(s) => cache::gabung(s, &alamat_token, tersimpan, baru, latest).keys,
        None => {
            let mut k = tersimpan.keys;
            k.extend(baru);
            k
        }
    })
}

/// Rakit `PoolInfo` V4 dari hasil batch gabungan.
///
/// Kegagalan per pool tidak dipalsukan menjadi nol. Likuiditas nol palsu akan
/// membuat pool sehat terbuang oleh penyaring seolah olah ia memang kosong.
/// Lihat memory/003.
fn baca_v4_dari_batch(
    hasil: &[Option<alloy::primitives::Bytes>],
    awal: usize,
    kunci: Vec<cache::PoolKeyRecord>,
) -> Vec<PoolInfo> {
    let ambil = |i: usize| hasil.get(i).and_then(|x| x.as_ref());
    let mut out = Vec::with_capacity(kunci.len());
    let mut gagal = 0usize;

    for (n, k) in kunci.into_iter().enumerate() {
        let dasar = awal + n * 2;
        let (Some(slot0), Some(liquidity)) = (
            crate::chain::multicall::decode::<abi::IStateView::getSlot0Call>(ambil(dasar)),
            crate::chain::multicall::decode::<abi::IStateView::getLiquidityCall>(ambil(dasar + 1)),
        ) else {
            gagal += 1;
            continue;
        };

        let sqrt = U256::from(slot0.sqrtPriceX96);
        out.push(PoolInfo {
            version: PoolVersion::V4,
            id: k.id,
            token0: k.currency0,
            token1: k.currency1,
            fee: k.fee,
            dynamic_fee: k.fee & DYNAMIC_FEE_FLAG != 0,
            tick_spacing: k.tick_spacing,
            hooks: Some(k.hooks),
            sqrt_price_x96: sqrt.to_string(),
            tick: slot0.tick.as_i32(),
            liquidity: liquidity.to_string(),
            raw_price: raw_price_from_sqrt_x96(sqrt),
            stats: None,
        });
    }

    if gagal > 0 {
        tracing::warn!(
            gagal,
            terbaca = out.len(),
            "sebagian pool tidak terbaca lewat multicall, dilewati bukan dianggap kosong"
        );
    }

    out
}

/// Seberapa jauh pencarian pool boleh menyentuh jaringan.
///
/// KENAPA ADA DUA MODE
/// Menemukan pool V4 berarti memindai log, dan satu permintaan `eth_getLogs` ke
/// RPC publik ini memakan sekitar 700 ms berapa pun lebar rentangnya. Dua arah
/// berarti 1,4 detik yang harus dibayar sebelum satu pun pool bisa ditampilkan.
///
/// Padahal PoolKey tidak pernah berubah. Kalau token ini sudah pernah dibuka,
/// kunci poolnya sudah kita simpan, dan yang benar benar perlu dibaca ulang
/// hanya harga serta likuiditas, dan itu muat dalam satu permintaan.
///
/// Jadi panel memakai dua tahap: `CacheSaja` untuk menampilkan sesuatu dalam
/// waktu di bawah setengah detik, lalu `Penuh` di latar untuk menemukan pool
/// yang baru dibuat sejak terakhir kali.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModePencarian {
    /// Jangan memindai log sama sekali. Hanya baca keadaan pool yang kuncinya
    /// sudah tersimpan. Kalau cache kosong, hasilnya juga kosong.
    CacheSaja,
    /// Pindai blok yang belum pernah dilihat, lalu baca keadaan semuanya.
    Penuh,
}

/// Cari pool V4 untuk satu token, TANPA mengandaikan pasangannya.
///
/// KENAPA INI ADA
/// Pencarian yang menyaring dua currency sekaligus hanya menemukan pool kalau
/// kita sudah menebak benar token lawannya. Tebakan itu ternyata sering salah:
/// banyak memecoin di chain ini berpasangan dengan stablecoin, bukan WETH, dan
/// hasilnya kita melaporkan tidak ada pool padahal ada puluhan.
///
/// Versi tanpa cache. Dipakai alat baris perintah dan pengujian. Jalur panel
/// memakai `discover_v4_cached`.
pub async fn discover_v4_any<P: Provider>(
    provider: &P,
    registry: &Registry,
    token: Address,
) -> Result<Vec<PoolInfo>> {
    discover_v4_cached(provider, registry, None, token, ModePencarian::Penuh).await
}

/// Sama seperti `discover_v4_any`, tapi mengingat kunci pool yang sudah pernah
/// ditemukan.
///
/// KENAPA CACHE ADA DI SINI
/// Pemindaian log adalah bagian termahal dari seluruh alur, sekitar satu detik
/// per arah untuk jendela tiga juta blok. Tanpa cache, biaya itu dibayar ulang
/// setiap kali user membuka token yang sama, dan itulah kenapa panel terasa
/// menggantung sementara Zenith LP tampak seketika.
///
/// PoolKey tidak pernah berubah setelah pool dibuat, jadi menyimpannya aman.
/// Yang tidak disimpan adalah harga dan likuiditas: keduanya selalu dibaca
/// ulang, karena angka basi pada alat yang mengirim transaksi jauh lebih
/// berbahaya daripada menunggu sebentar.
pub async fn discover_v4_cached<P: Provider>(
    provider: &P,
    registry: &Registry,
    store: Option<&crate::store::Store>,
    token: Address,
    mode: ModePencarian,
) -> Result<Vec<PoolInfo>> {
    let pool_manager = registry.contract("uniswap_v4", "pool_manager")?;
    let state_view = registry.contract("uniswap_v4", "state_view")?;
    let multicall = registry.contract("common", "multicall3")?;
    let alamat_token = token.to_string();

    let tersimpan = store
        .map(|s| cache::baca(s, &alamat_token))
        .unwrap_or_default();

    // Mode cache tidak menyentuh log sama sekali, jadi ia juga tidak perlu tahu
    // blok terakhir. Menghilangkan satu round trip di sini terasa langsung,
    // karena jalur ini yang dipakai saat panel ingin menampilkan sesuatu cepat.
    if mode == ModePencarian::CacheSaja {
        if tersimpan.keys.is_empty() {
            return Ok(Vec::new());
        }
        return hydrate_v4(provider, state_view, multicall, tersimpan.keys).await;
    }

    let latest = provider
        .get_block_number()
        .await
        .context("gagal membaca blok terakhir")?;

    // Jendela mundur, bukan deploy_block, karena pool yang dicari selalu yang
    // baru. Tidak pernah turun di bawah blok pemasangan protokol.
    let jendela = latest
        .saturating_sub(SCAN_LOOKBACK_DEFAULT)
        .max(registry.chain.deploy_block);

    // Kalau cache masih menyambung dengan jendela sekarang, cukup pindai blok
    // yang belum pernah dilihat. Kalau ia sudah tertinggal jauh, jendela penuh
    // dipindai lagi, karena melompati blok berarti kehilangan pool selamanya.
    let bersambung = tersimpan.scanned_to > 0 && tersimpan.scanned_to + 1 >= jendela;
    let batas_bawah = if bersambung {
        (tersimpan.scanned_to + 1).min(latest)
    } else {
        jendela
    };

    let mut baru: Vec<cache::PoolKeyRecord> = Vec::new();

    if batas_bawah <= latest {
        // KENAPA BERURUTAN, BUKAN BERBARENGAN
        // Dua pemindaian ini pernah dijalankan berbarengan, dan hasilnya justru
        // lebih lambat. RPC publik membalas 429 untuk yang kedua, lalu lapisan
        // percobaan ulang menunggu sebelum mengulang. Diukur langsung pada
        // jendela tiga juta blok:
        //
        //   berurutan   1102 ms, keduanya berhasil
        //   berbarengan  658 ms untuk yang lolos, yang satunya kena 429 lalu
        //                diulang, sehingga totalnya sekitar 5 detik
        //
        // Rantai ini memberi satu permintaan berat pada satu waktu. Menghormati
        // itu lebih cepat daripada melawannya.
        let jam = std::time::Instant::now();
        let sebagai0 =
            scan_initialize_logs(provider, pool_manager, token, true, batas_bawah, latest).await?;
        let sebagai1 =
            scan_initialize_logs(provider, pool_manager, token, false, batas_bawah, latest).await?;
        tracing::debug!(
            ms = jam.elapsed().as_millis(),
            blok = latest - batas_bawah + 1,
            inkremental = bersambung,
            "pemindaian log selesai"
        );

        for log in sebagai0.into_iter().chain(sebagai1) {
            let Ok(decoded) = abi::Initialize::decode_log(&log.inner) else {
                continue;
            };
            baru.push(cache::PoolKeyRecord {
                id: format!("0x{}", hex::encode(decoded.id)),
                currency0: decoded.currency0.to_string(),
                currency1: decoded.currency1.to_string(),
                fee: decoded.fee.to::<u32>(),
                tick_spacing: decoded.tickSpacing.as_i32(),
                hooks: decoded.hooks.to_string(),
            });
        }
    }

    // `scanned_to` hanya maju kalau pemindaiannya benar benar selesai. Kalau
    // ada yang gagal, fungsi ini sudah kembali dengan error di atas.
    let semua = match store {
        Some(s) => cache::gabung(s, &alamat_token, tersimpan, baru, latest),
        None => {
            let mut c = tersimpan;
            c.keys.extend(baru);
            c.scanned_to = latest;
            c
        }
    };

    let jam = std::time::Instant::now();
    let hasil = hydrate_v4(provider, state_view, multicall, semua.keys).await;
    tracing::debug!(
        ms = jam.elapsed().as_millis(),
        "pembacaan keadaan pool selesai"
    );
    hasil
}

/// Baca keadaan setiap pool V4 dalam satu permintaan.
///
/// Kegagalan per pool tidak menjatuhkan yang lain, tapi juga tidak dipalsukan
/// menjadi nol. Pool yang tidak terbaca dilewati dan dicatat, karena likuiditas
/// nol palsu akan membuatnya terbuang oleh penyaring seolah olah pool itu
/// memang kosong. Lihat memory/003.
async fn hydrate_v4<P: Provider>(
    provider: &P,
    state_view: Address,
    multicall: Address,
    kunci: Vec<cache::PoolKeyRecord>,
) -> Result<Vec<PoolInfo>> {
    if kunci.is_empty() {
        return Ok(Vec::new());
    }

    // Dua panggilan per pool, berselang seling, supaya indeksnya bisa dihitung
    // langsung dari nomor pool tanpa tabel bantu.
    let mut calls = Vec::with_capacity(kunci.len() * 2);
    let mut terpakai: Vec<cache::PoolKeyRecord> = Vec::with_capacity(kunci.len());
    for k in kunci {
        let Ok(id) = k.id.parse::<FixedBytes<32>>() else {
            tracing::warn!(id = %k.id, "poolId di cache tidak berbentuk bytes32, dilewati");
            continue;
        };
        calls.push(crate::chain::multicall::encode(
            state_view,
            &abi::IStateView::getSlot0Call { poolId: id },
        ));
        calls.push(crate::chain::multicall::encode(
            state_view,
            &abi::IStateView::getLiquidityCall { poolId: id },
        ));
        terpakai.push(k);
    }

    let hasil = crate::chain::multicall::batch_call(provider, multicall, calls).await?;

    let mut out = Vec::with_capacity(terpakai.len());
    let mut gagal = 0usize;

    for (n, k) in terpakai.into_iter().enumerate() {
        let dasar = n * 2;

        let slot0 = crate::chain::multicall::decode::<abi::IStateView::getSlot0Call>(
            hasil.get(dasar).and_then(|x| x.as_ref()),
        );
        let Some(slot0) = slot0 else {
            gagal += 1;
            continue;
        };
        let liquidity = crate::chain::multicall::decode::<abi::IStateView::getLiquidityCall>(
            hasil.get(dasar + 1).and_then(|x| x.as_ref()),
        );
        let Some(liquidity) = liquidity else {
            gagal += 1;
            continue;
        };

        let sqrt = U256::from(slot0.sqrtPriceX96);

        out.push(PoolInfo {
            version: PoolVersion::V4,
            id: k.id,
            token0: k.currency0,
            token1: k.currency1,
            fee: k.fee,
            dynamic_fee: k.fee & DYNAMIC_FEE_FLAG != 0,
            tick_spacing: k.tick_spacing,
            hooks: Some(k.hooks),
            sqrt_price_x96: sqrt.to_string(),
            tick: slot0.tick.as_i32(),
            liquidity: liquidity.to_string(),
            raw_price: raw_price_from_sqrt_x96(sqrt),
            stats: None,
        });
    }

    if gagal > 0 {
        tracing::warn!(
            gagal,
            terbaca = out.len(),
            "sebagian pool tidak terbaca lewat multicall, dilewati bukan dianggap kosong"
        );
    }

    Ok(out)
}

/// Rentang blok terlebar yang dicoba sekali jalan.
///
/// DIUKUR, BUKAN DITEBAK
/// Nilai pertama 500_000 dipilih karena takut ditolak, dan itu membuat satu
/// pencarian butuh enam permintaan berurutan per arah. Pengukuran langsung ke
/// RPC menunjukkan angka sebenarnya jauh lebih longgar:
///
/// | Rentang | Hasil |
/// | --- | --- |
/// | 3 juta blok | berhasil, 1016 ms |
/// | 10 juta blok | `log query timed out` |
/// | 48 juta blok | `log query timed out` |
///
/// Jadi seluruh jendela mundur muat dalam satu permintaan, dan memecahnya
/// hanya menambah latensi sekaligus memancing rate limit. Kalau jendelanya
/// nanti diperlebar melewati batas ini, penurunan otomatis di bawah yang akan
/// menanganinya.
const SCAN_CHUNK_AWAL: u64 = SCAN_LOOKBACK_DEFAULT;
const SCAN_CHUNK_MINIMUM: u64 = 5_000;

/// Sejauh mana mundur dari blok terakhir sebelum menyerah.
///
/// Tiga juta blok kira kira tiga setengah hari pada waktu blok 100 ms. Itu
/// cukup untuk seluruh memecoin yang masih relevan untuk LP, dan jauh lebih
/// murah daripada menyapu 55 juta blok sejarah.
pub const SCAN_LOOKBACK_DEFAULT: u64 = 3_000_000;

/// Berapa kali menunggu saat RPC membatasi laju sebelum menyerah.
const SABAR_MAKSIMUM: u32 = 4;

/// Jeda dasar saat dibatasi laju. Naik berlipat tiap percobaan.
const JEDA_DASAR_MS: u64 = 400;

/// Benar kalau kegagalannya karena laju dibatasi, bukan karena rentangnya
/// terlalu lebar.
///
/// Dikenali dari teks, karena lapisan transport membungkus kode HTTP-nya dan
/// tidak menyediakan jalan yang lebih rapi. Kalau pengenalan ini meleset,
/// akibatnya hanya kembali ke perilaku lama, yaitu mengecilkan rentang.
fn dibatasi_laju(e: &impl std::fmt::Display) -> bool {
    let teks = e.to_string();
    teks.contains("429") || teks.to_lowercase().contains("too many requests")
}

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
    let mut jeda_sisa = SABAR_MAKSIMUM;

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
                // DIBATASI LAJU BERBEDA DARI RENTANG TERLALU LEBAR
                //
                // Keduanya muncul sebagai kegagalan yang sama, tapi obatnya
                // berlawanan. Rentang yang kelebaran memang harus dipecah.
                // Tapi kalau sebabnya 429, memecah rentang justru menambah
                // jumlah permintaan dan memperburuk keadaan, sampai akhirnya
                // pemindaian menyerah pada rentang kecil yang sebenarnya
                // sanggup dilayani.
                //
                // Yang benar untuk 429 adalah menunggu, bukan mengecilkan.
                // Batas LEBAR rentang tidak bisa diatasi dengan mengecilkan.
                // Endpoint yang hanya mengizinkan sepuluh blok menuntut ratusan
                // ribu permintaan untuk sekali pencarian, dan penyempitan
                // otomatis hanya akan menyerah dengan pesan yang tidak
                // menjelaskan apa apa.
                if crate::chain::logs::lebar_dibatasi(&e) {
                    return Err(anyhow::anyhow!(
                        "endpoint RPC ini membatasi lebar rentang eth_getLogs, sedangkan pencarian pool menuntut pemindaian jutaan blok. Ganti ke endpoint yang mengizinkan rentang lebar, misalnya RPC publik Robinhood Chain. Pesan aslinya: {e}"
                    ));
                }
                if dibatasi_laju(&e) && jeda_sisa > 0 {
                    let jeda = JEDA_DASAR_MS * (SABAR_MAKSIMUM - jeda_sisa + 1) as u64;
                    jeda_sisa -= 1;
                    tracing::debug!(jeda_ms = jeda, "RPC membatasi laju, menunggu");
                    tokio::time::sleep(std::time::Duration::from_millis(jeda)).await;
                    continue;
                }
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
    /// Benar kalau token lawannya adalah kutipan yang dikenal rantai ini,
    /// yaitu ETH asli atau salah satu entri tokens di chain.toml. Kelompok
    /// kanonik didahulukan saat memilih pasar utama, karena di situlah pasar
    /// sungguhan berada.
    pub canonical: bool,
    pub pools: Vec<PoolInfo>,
}


/// Kelompokkan pool menurut token lawan, lalu urutkan kelompoknya.
///
/// `dikenal` berisi metadata token lawan yang tidak ada di `chain.toml`, hasil
/// pembacaan on chain. Kalau sebuah lawan tidak ada di sana, desimalnya TIDAK
/// ditebak 18, kelompoknya dibuang. Menebak desimal menggeser harga sebesar
/// sepuluh pangkat selisihnya, dan pada USDG selisih itu sepuluh triliun.
fn kelompokkan(
    pools: Vec<PoolInfo>,
    token: Address,
    registry: &Registry,
    dikenal: &std::collections::HashMap<String, TokenMeta>,
) -> Vec<QuoteGroup> {
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
        .filter_map(|(quote, pools)| {
            let (symbol, decimals, kanonik) = kenali_quote(&quote, registry, dikenal)?;
            let pools = usable_pools(pools);
            (!pools.is_empty()).then_some(QuoteGroup {
                quote,
                quote_symbol: symbol,
                quote_decimals: decimals,
                canonical: kanonik,
                pools,
            })
        })
        .collect();

    // KENAPA KANONIK DULU, BUKAN LIKUIDITAS DULU
    // Token yang ramai bisa berpasangan dengan puluhan token lain, dan sebagian
    // pasangan itu hanya pool sampah dengan likuiditas besar yang tidak pernah
    // ditukar. Pada salinan FIG di 0x41F4..., ada 32 pasangan sekaligus, dan
    // pengurutan murni likuiditas menaruh pasangan berisi satu pool sampah di
    // atas pasar USDG yang sebenarnya.
    //
    // Pasangan terhadap token kutipan yang dikenal rantai ini didahulukan,
    // karena di situlah pasar sungguhan berada. Sisanya tetap ditampilkan dan
    // bisa dipilih user, hanya tidak dipilihkan otomatis.
    out.sort_by(|a, b| {
        let total = |g: &QuoteGroup| -> f64 {
            g.pools
                .iter()
                .map(|p| p.liquidity.parse::<f64>().unwrap_or(0.0))
                .sum()
        };
        b.canonical
            .cmp(&a.canonical)
            .then_with(|| {
                total(b)
                    .partial_cmp(&total(a))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.quote.cmp(&b.quote))
    });

    out
}

/// Simbol, desimal, dan status kanonik sebuah token lawan.
///
/// Mengembalikan `None` kalau tokennya tidak dikenal sama sekali dan juga tidak
/// bisa dibaca on chain. Tanpa desimal yang benar, harga kelompok itu akan
/// salah, dan menampilkan harga salah lebih buruk daripada tidak menampilkan.
fn kenali_quote(
    alamat: &str,
    registry: &Registry,
    dikenal: &std::collections::HashMap<String, TokenMeta>,
) -> Option<(String, u8, bool)> {
    if alamat.eq_ignore_ascii_case(ZERO_ADDRESS) {
        // V4 mendukung ETH asli sebagai currency, bukan hanya WETH. Ia tidak
        // punya `decimals()` karena bukan kontrak, jadi 18 di sini adalah fakta
        // rantai, bukan tebakan.
        return Some(("ETH".into(), 18, true));
    }
    for (nama, entry) in &registry.tokens {
        if entry.address.eq_ignore_ascii_case(alamat) {
            return Some((nama.to_uppercase(), entry.decimals, true));
        }
    }
    dikenal
        .get(&alamat.to_lowercase())
        .map(|m| (m.symbol.clone(), m.decimals, false))
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
            stats: None,
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
