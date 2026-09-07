//! Memuat posisi milik wallet dari rantai.
//!
//! INVARIAN
//! - Kepemilikan SELALU dibaca ulang, tidak pernah dari cache. Yang di-cache
//!   hanya daftar calon token id, karena NFT bisa berpindah kapan saja dan
//!   menampilkan posisi milik orang lain adalah kesalahan yang paling
//!   membingungkan.
//! - Fee yang belum diklaim dibaca dengan cara yang berbeda per protokol, dan
//!   keduanya bukan `tokensOwed`. Lihat catatan di masing masing fungsi.
//! - Jumlah permintaan RPC dijaga tetap sedikit lewat Multicall3, karena tab ini
//!   dimuat ulang setiap kali user membukanya.

use std::collections::HashMap;

use alloy::primitives::{keccak256, Address, FixedBytes, U256};
use alloy::providers::Provider;
use alloy::rpc::types::TransactionRequest;
use alloy::sol_types::{SolCall, SolValue};
use anyhow::Result;

use crate::chain::multicall;
use crate::chain::registry::Registry;
use crate::exec::abi_write::INonfungiblePositionManager as Npm;
use crate::exec::v4::abi::{IPositionManagerV4 as PmV4, IStateViewV4};
use crate::pools::PoolVersion;

use super::{fees, Posisi};

/// Berapa posisi terbanyak yang dimuat sekaligus.
///
/// Wallet dengan ratusan posisi akan membuat satu batch multicall melewati batas
/// gas node. Batasnya dipasang di sini alih alih membiarkan permintaannya
/// ditolak seluruhnya.
pub const MAKS_POSISI: usize = 60;

/// Muat seluruh posisi V3 milik sebuah wallet.
///
/// NonfungiblePositionManager v3 mengimplementasikan ERC721Enumerable, jadi
/// jumlahnya bisa dibaca lalu setiap token id diambil lewat indeks. Semuanya
/// digabung dalam dua permintaan.
pub async fn muat_v3<P: Provider>(
    provider: &P,
    registry: &Registry,
    owner: Address,
) -> Result<Vec<Posisi>> {
    let npm = registry.contract("uniswap_v3", "position_manager")?;
    let mc = registry.contract("common", "multicall3")?;

    let jumlah = multicall::batch_call(
        provider,
        mc,
        vec![multicall::encode(npm, &Npm::balanceOfCall { owner })],
    )
    .await?;
    let jumlah = multicall::decode::<Npm::balanceOfCall>(jumlah.first().and_then(|x| x.as_ref()))
        .unwrap_or(U256::ZERO)
        .to::<u64>() as usize;

    if jumlah == 0 {
        return Ok(Vec::new());
    }
    let jumlah = jumlah.min(MAKS_POSISI);

    let calls: Vec<_> = (0..jumlah)
        .map(|i| {
            multicall::encode(
                npm,
                &Npm::tokenOfOwnerByIndexCall {
                    owner,
                    index: U256::from(i),
                },
            )
        })
        .collect();
    let hasil = multicall::batch_call(provider, mc, calls).await?;

    let ids: Vec<U256> = hasil
        .iter()
        .filter_map(|x| multicall::decode::<Npm::tokenOfOwnerByIndexCall>(x.as_ref()))
        .collect();

    baca_v3(provider, registry, owner, &ids).await
}

/// Baca keadaan sekumpulan posisi V3.
async fn baca_v3<P: Provider>(
    provider: &P,
    registry: &Registry,
    owner: Address,
    ids: &[U256],
) -> Result<Vec<Posisi>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let npm = registry.contract("uniswap_v3", "position_manager")?;
    let mc = registry.contract("common", "multicall3")?;

    let calls: Vec<_> = ids
        .iter()
        .map(|id| multicall::encode(npm, &Npm::positionsCall { tokenId: *id }))
        .collect();
    let hasil = multicall::batch_call(provider, mc, calls).await?;

    let mut out = Vec::with_capacity(ids.len());
    let mut gagal = 0usize;

    for (i, id) in ids.iter().enumerate() {
        let Some(p) = multicall::decode::<Npm::positionsCall>(hasil.get(i).and_then(|x| x.as_ref()))
        else {
            gagal += 1;
            continue;
        };

        // Fee dibaca dengan menyimulasikan `collect` dari pemiliknya.
        //
        // KENAPA BUKAN `tokensOwed`
        // Field itu hanya diperbarui saat posisinya disentuh, jadi posisi yang
        // didiamkan melaporkan nol selamanya. Simulasi `collect` menjalankan
        // perhitungan yang sama dengan yang akan terjadi sungguhan, jadi
        // hasilnya persis.
        //
        // Tidak bisa lewat Multicall3, karena `collect` menuntut pemanggilnya
        // adalah pemilik NFT, dan lewat multicall pemanggilnya jadi multicall.
        let (fee0, fee1) = simulasi_collect(provider, npm, owner, *id)
            .await
            .unwrap_or((U256::ZERO, U256::ZERO));

        out.push(Posisi {
            token_id: id.to_string(),
            version: PoolVersion::V3,
            token0: p.token0.to_string(),
            token1: p.token1.to_string(),
            fee: p.fee.to::<u32>(),
            tick_lower: p.tickLower.as_i32(),
            tick_upper: p.tickUpper.as_i32(),
            liquidity: p.liquidity.to_string(),
            tick_sekarang: None,
            in_range: false,
            fee0: fee0.to_string(),
            fee1: fee1.to_string(),
            hooks: None,
            sqrt_price_x96: None,
            amount0: "0".into(),
            amount1: "0".into(),
            pnl: None,
            kosong: p.liquidity == 0,
        });
    }

    if gagal > 0 {
        tracing::warn!(gagal, "sebagian posisi V3 tidak terbaca, dilewati");
    }
    Ok(out)
}

/// Simulasikan `collect` untuk mengetahui fee yang belum diklaim.
///
/// Dijalankan lewat `eth_call`, jadi tidak ada yang benar benar diklaim.
async fn simulasi_collect<P: Provider>(
    provider: &P,
    npm: Address,
    owner: Address,
    token_id: U256,
) -> Result<(U256, U256), String> {
    let data = Npm::collectCall {
        params: Npm::CollectParams {
            tokenId: token_id,
            recipient: owner,
            amount0Max: u128::MAX,
            amount1Max: u128::MAX,
        },
    }
    .abi_encode();

    let tx = TransactionRequest::default()
        .from(owner)
        .to(npm)
        .input(alloy::primitives::Bytes::from(data).into());

    let keluaran = provider.call(tx).await.map_err(|e| e.to_string())?;
    let hasil = Npm::collectCall::abi_decode_returns(&keluaran).map_err(|e| e.to_string())?;
    Ok((hasil.amount0, hasil.amount1))
}

/// Muat seluruh posisi V4 milik sebuah wallet.
///
/// Daftar calonnya datang dari pemindaian log, lalu kepemilikannya dibaca ulang
/// karena NFT bisa sudah berpindah sejak terakhir dipindai.
pub async fn muat_v4<P: Provider>(
    provider: &P,
    registry: &Registry,
    store: Option<&crate::store::Store>,
    owner: Address,
) -> Result<Vec<Posisi>> {
    let kandidat = super::kandidat_v4(provider, registry, store, owner).await?;
    if kandidat.is_empty() {
        return Ok(Vec::new());
    }

    let pm = registry.contract("uniswap_v4", "position_manager")?;
    let mc = registry.contract("common", "multicall3")?;

    // Kepemilikan sekarang, bukan yang tercatat saat pemindaian.
    let calls: Vec<_> = kandidat
        .iter()
        .map(|id| multicall::encode(pm, &PmV4::ownerOfCall { tokenId: *id }))
        .collect();
    let hasil = multicall::batch_call(provider, mc, calls).await?;

    let milik: Vec<U256> = kandidat
        .iter()
        .enumerate()
        .filter(|(i, _)| {
            multicall::decode::<PmV4::ownerOfCall>(hasil.get(*i).and_then(|x| x.as_ref()))
                .is_some_and(|a| a == owner)
        })
        .map(|(_, id)| *id)
        .take(MAKS_POSISI)
        .collect();

    if milik.is_empty() {
        return Ok(Vec::new());
    }

    // Kunci pool dan likuiditas, dua panggilan per posisi.
    let mut calls = Vec::with_capacity(milik.len() * 2);
    for id in &milik {
        calls.push(multicall::encode(
            pm,
            &PmV4::getPoolAndPositionInfoCall { tokenId: *id },
        ));
        calls.push(multicall::encode(
            pm,
            &PmV4::getPositionLiquidityCall { tokenId: *id },
        ));
    }
    let hasil = multicall::batch_call(provider, mc, calls).await?;

    let mut dasar = Vec::with_capacity(milik.len());
    for (n, id) in milik.iter().enumerate() {
        let info = multicall::decode::<PmV4::getPoolAndPositionInfoCall>(
            hasil.get(n * 2).and_then(|x| x.as_ref()),
        );
        let liq = multicall::decode::<PmV4::getPositionLiquidityCall>(
            hasil.get(n * 2 + 1).and_then(|x| x.as_ref()),
        );
        let (Some(info), Some(liq)) = (info, liq) else {
            continue;
        };
        let (tick_lower, tick_upper) = crate::exec::v4::verify::bongkar_tick(info._1);
        dasar.push((*id, info._0, tick_lower, tick_upper, liq));
    }

    baca_fee_v4(provider, registry, &dasar).await
}

/// Lengkapi posisi V4 dengan fee yang belum diklaim.
///
/// Fee dihitung dari pertumbuhan fee di dalam range, karena V4 tidak punya
/// fungsi `collect` yang bisa disimulasikan seperti V3.
///
/// Pemilik posisi di dalam PoolManager adalah PositionManager, bukan wallet
/// user, dan `salt` adalah token id. Itu yang membedakan posisi dua wallet pada
/// range yang sama persis, dan salah di sini akan menampilkan fee milik orang
/// lain.
type DasarV4 = (U256, PmV4::PoolKey, i32, i32, u128);

async fn baca_fee_v4<P: Provider>(
    provider: &P,
    registry: &Registry,
    dasar: &[DasarV4],
) -> Result<Vec<Posisi>> {
    let sv = registry.contract("uniswap_v4", "state_view")?;
    let pm = registry.contract("uniswap_v4", "position_manager")?;
    let mc = registry.contract("common", "multicall3")?;

    let mut calls = Vec::with_capacity(dasar.len() * 3);
    for (id, key, lower, upper, _) in dasar {
        let pool_id = pool_id_dari(key);
        let l = alloy::primitives::aliases::I24::try_from(*lower).unwrap_or_default();
        let u = alloy::primitives::aliases::I24::try_from(*upper).unwrap_or_default();

        calls.push(multicall::encode(
            sv,
            &IStateViewV4::getFeeGrowthInsideCall {
                poolId: pool_id,
                tickLower: l,
                tickUpper: u,
            },
        ));
        calls.push(multicall::encode(
            sv,
            &IStateViewV4::getPositionInfoCall {
                poolId: pool_id,
                owner: pm,
                tickLower: l,
                tickUpper: u,
                salt: FixedBytes::<32>::from(id.to_be_bytes::<32>()),
            },
        ));
        calls.push(multicall::encode(
            sv,
            &crate::pools::abi::IStateView::getSlot0Call { poolId: pool_id },
        ));
    }

    let hasil = multicall::batch_call(provider, mc, calls).await?;

    let mut out = Vec::with_capacity(dasar.len());
    for (n, (id, key, lower, upper, liq)) in dasar.iter().enumerate() {
        let tumbuh = multicall::decode::<IStateViewV4::getFeeGrowthInsideCall>(
            hasil.get(n * 3).and_then(|x| x.as_ref()),
        );
        let posisi_pool = multicall::decode::<IStateViewV4::getPositionInfoCall>(
            hasil.get(n * 3 + 1).and_then(|x| x.as_ref()),
        );
        let slot0 = multicall::decode::<crate::pools::abi::IStateView::getSlot0Call>(
            hasil.get(n * 3 + 2).and_then(|x| x.as_ref()),
        );

        let (fee0, fee1) = match (&tumbuh, &posisi_pool) {
            (Some(t), Some(p)) => (
                fees::fee_belum_diklaim(t.feeGrowthInside0X128, p.feeGrowthInside0LastX128, *liq),
                fees::fee_belum_diklaim(t.feeGrowthInside1X128, p.feeGrowthInside1LastX128, *liq),
            ),
            // Kegagalan membaca TIDAK dipalsukan jadi nol tanpa jejak.
            _ => {
                tracing::warn!(token_id = %id, "pertumbuhan fee posisi V4 tidak terbaca");
                (U256::ZERO, U256::ZERO)
            }
        };

        let tick = slot0.as_ref().map(|s| s.tick.as_i32());
        let sqrt = slot0.as_ref().map(|s| s.sqrtPriceX96.to_string());

        out.push(Posisi {
            token_id: id.to_string(),
            version: PoolVersion::V4,
            token0: key.currency0.to_string(),
            token1: key.currency1.to_string(),
            fee: key.fee.to::<u32>(),
            tick_lower: *lower,
            tick_upper: *upper,
            liquidity: liq.to_string(),
            tick_sekarang: tick,
            // Range Uniswap setengah terbuka: batas atas sudah di luar.
            in_range: tick.is_some_and(|t| t >= *lower && t < *upper),
            fee0: fee0.to_string(),
            fee1: fee1.to_string(),
            hooks: Some(key.hooks.to_string()),
            sqrt_price_x96: sqrt,
            amount0: "0".into(),
            amount1: "0".into(),
            pnl: None,
            kosong: *liq == 0,
        });
    }

    Ok(out)
}

/// poolId adalah keccak256 dari PoolKey yang dikodekan ABI.
///
/// Kelima fieldnya ikut, termasuk hook. Menghilangkan salah satunya menghasilkan
/// id pool yang sama sekali lain.
pub fn pool_id_dari(key: &PmV4::PoolKey) -> FixedBytes<32> {
    keccak256(key.abi_encode())
}

/// Lengkapi tick pool sekarang untuk posisi V3.
///
/// Dipisah dari pembacaan posisi karena butuh alamat poolnya, yang harus dicari
/// lewat factory lebih dulu.
pub async fn lengkapi_tick_v3<P: Provider>(
    provider: &P,
    registry: &Registry,
    posisi: &mut [Posisi],
) -> Result<()> {
    use crate::pools::abi::{IUniswapV3Factory, IUniswapV3Pool};

    let factory = registry.contract("uniswap_v3", "factory")?;
    let mc = registry.contract("common", "multicall3")?;

    let perlu: Vec<usize> = posisi
        .iter()
        .enumerate()
        .filter(|(_, p)| p.version == PoolVersion::V3)
        .map(|(i, _)| i)
        .collect();
    if perlu.is_empty() {
        return Ok(());
    }

    let mut calls = Vec::with_capacity(perlu.len());
    for i in &perlu {
        let p = &posisi[*i];
        let (Ok(t0), Ok(t1)) = (p.token0.parse::<Address>(), p.token1.parse::<Address>()) else {
            continue;
        };
        calls.push(multicall::encode(
            factory,
            &IUniswapV3Factory::getPoolCall {
                tokenA: t0,
                tokenB: t1,
                fee: alloy::primitives::aliases::U24::from(p.fee),
            },
        ));
    }
    let hasil = multicall::batch_call(provider, mc, calls).await?;

    let mut alamat: HashMap<usize, Address> = HashMap::new();
    for (n, i) in perlu.iter().enumerate() {
        if let Some(a) =
            multicall::decode::<IUniswapV3Factory::getPoolCall>(hasil.get(n).and_then(|x| x.as_ref()))
        {
            if !a.is_zero() {
                alamat.insert(*i, a);
            }
        }
    }
    if alamat.is_empty() {
        return Ok(());
    }

    let urut: Vec<(usize, Address)> = alamat.into_iter().collect();
    let calls: Vec<_> = urut
        .iter()
        .map(|(_, a)| multicall::encode(*a, &IUniswapV3Pool::slot0Call {}))
        .collect();
    let hasil = multicall::batch_call(provider, mc, calls).await?;

    for (n, (i, _)) in urut.iter().enumerate() {
        if let Some(s) =
            multicall::decode::<IUniswapV3Pool::slot0Call>(hasil.get(n).and_then(|x| x.as_ref()))
        {
            let tick = s.tick.as_i32();
            posisi[*i].sqrt_price_x96 = Some(s.sqrtPriceX96.to_string());
            posisi[*i].tick_sekarang = Some(tick);
            posisi[*i].in_range = tick >= posisi[*i].tick_lower && tick < posisi[*i].tick_upper;
        }
    }
    Ok(())
}

/// Muat seluruh posisi milik wallet, V3 dan V4 sekaligus.
///
/// Dijalankan berurutan, bukan berbarengan, dengan alasan yang sama seperti
/// pemindaian pool: RPC ini memberi satu permintaan berat pada satu waktu.
pub async fn muat_semua<P: Provider>(
    provider: &P,
    registry: &Registry,
    store: Option<&crate::store::Store>,
    owner: Address,
) -> Vec<Posisi> {
    let mut out = Vec::new();

    match muat_v3(provider, registry, owner).await {
        Ok(mut v) => {
            if let Err(e) = lengkapi_tick_v3(provider, registry, &mut v).await {
                tracing::warn!(error = %e, "tick pool V3 tidak terbaca");
            }
            out.append(&mut v);
        }
        Err(e) => tracing::warn!(error = %e, "pemuatan posisi V3 gagal"),
    }

    match muat_v4(provider, registry, store, owner).await {
        Ok(mut v) => out.append(&mut v),
        Err(e) => tracing::warn!(error = %e, "pemuatan posisi V4 gagal"),
    }

    lengkapi_pnl(&mut out, registry, store);
    urutkan(&mut out);
    out
}

/// Kenali token kutipan sebuah pool beserta desimalnya.
///
/// KENAPA INI PENTING UNTUK PnL
/// Nilai harus dinyatakan dalam satuan yang berarti bagi user. Menghitungnya
/// dalam satuan terkecil token1 memang konsisten secara aritmetika, tapi kalau
/// token1 ternyata memecoin, hasilnya adalah angka raksasa dalam satuan
/// memecoin yang terbaca seperti dolar. Itu lebih berbahaya daripada tidak
/// menampilkan angka sama sekali.
///
/// Mengembalikan `None` kalau tidak ada sisi yang merupakan kutipan yang
/// dikenal. Dalam keadaan itu PnL memang tidak bisa dinyatakan, dan kartunya
/// mengatakan begitu.
fn kutipan_pool(
    registry: &Registry,
    token0: &str,
    token1: &str,
) -> Option<(bool, u8)> {
    // ETH asli selalu berdesimal 18 dan selalu currency0 karena alamatnya nol.
    if token0.eq_ignore_ascii_case(crate::pools::ZERO_ADDRESS) {
        return Some((true, 18));
    }
    for entry in registry.tokens.values() {
        if entry.address.eq_ignore_ascii_case(token0) {
            return Some((true, entry.decimals));
        }
        if entry.address.eq_ignore_ascii_case(token1) {
            return Some((false, entry.decimals));
        }
    }
    None
}

/// Lengkapi posisi dengan jumlah token di dalamnya dan hasilnya.
///
/// KENAPA TERPISAH DARI PEMBACAAN
/// Perhitungannya murni, tidak menyentuh jaringan sama sekali. Memisahkannya
/// membuat bagian yang paling mudah salah bisa diuji tanpa rantai.
///
/// Posisi tanpa catatan biaya tetap dihitung, dengan menganggap jumlah yang
/// disetor sama dengan yang ada sekarang. Akibatnya impermanent loss-nya nol
/// dan hasilnya ditandai perkiraan. Itu jujur: kita memang tidak tahu berapa
/// yang dulu disetor, dan menyembunyikan posisinya justru melanggar permintaan
/// user bahwa posisi dari luar dikelola penuh.
pub fn lengkapi_pnl(
    posisi: &mut [Posisi],
    registry: &Registry,
    store: Option<&crate::store::Store>,
) {
    use crate::math::{liquidity_math, tick_math};

    for p in posisi.iter_mut() {
        let Some(sqrt_teks) = p.sqrt_price_x96.as_ref() else {
            continue;
        };
        let Ok(sqrt_p) = sqrt_teks.parse::<U256>() else {
            continue;
        };
        let (Ok(sqrt_a), Ok(sqrt_b)) = (
            tick_math::get_sqrt_ratio_at_tick(p.tick_lower),
            tick_math::get_sqrt_ratio_at_tick(p.tick_upper),
        ) else {
            continue;
        };
        let Ok(liq) = p.liquidity.parse::<u128>() else {
            continue;
        };

        let (a0, a1) = liquidity_math::amounts_for_liquidity(sqrt_p, sqrt_a, sqrt_b, liq)
            .unwrap_or((U256::ZERO, U256::ZERO));
        p.amount0 = a0.to_string();
        p.amount1 = a1.to_string();

        // Harga token1 per token0 pada satuan terkecil.
        let harga = crate::pools::raw_price_from_sqrt_x96(sqrt_p);
        if !(harga.is_finite() && harga > 0.0) {
            continue;
        }

        let Some((kutipan_token0, desimal)) = kutipan_pool(registry, &p.token0, &p.token1) else {
            // Pasangan memecoin melawan memecoin. Tidak ada satuan yang berarti,
            // dan angka dalam satuan salah satunya akan menyesatkan.
            continue;
        };

        let f = |s: &str| s.parse::<f64>().unwrap_or(0.0);
        let skala = 10f64.powi(desimal as i32);

        // Kalau kutipannya token0, nilai dinyatakan dalam token0, sehingga
        // token1 yang dibagi harga. Membalik ini menghasilkan angka yang
        // meleset sebesar kuadrat harganya.
        let ke_kutipan = |x0: f64, x1: f64| -> f64 {
            if kutipan_token0 {
                (x0 + x1 / harga) / skala
            } else {
                (x0 * harga + x1) / skala
            }
        };

        let posisi0 = f(&p.amount0);
        let posisi1 = f(&p.amount1);
        let fee_kutipan = ke_kutipan(f(&p.fee0), f(&p.fee1));

        let catatan = store.and_then(|s| crate::pnl::ambil(s, &p.token_id));
        let (setor0, setor1, gas, slippage, perkiraan) = match &catatan {
            Some(c) => (
                f(&c.setor0),
                f(&c.setor1),
                c.gas_kutipan,
                c.slippage_kutipan,
                false,
            ),
            // Tanpa catatan, yang disetor dianggap sama dengan yang ada
            // sekarang. Impermanent loss-nya jadi nol, dan itu ditandai.
            None => (posisi0, posisi1, 0.0, 0.0, true),
        };

        let il = ke_kutipan(setor0, setor1) - ke_kutipan(posisi0, posisi1);

        p.pnl = Some(crate::pnl::Hasil {
            fee: fee_kutipan,
            impermanent_loss: il,
            gas,
            slippage,
            net: fee_kutipan - il - gas - slippage,
            perkiraan,
        });
    }
}
/// Urutkan posisi: yang keluar range lebih dulu, lalu yang likuiditasnya besar.
///
/// PRD 3.5 meminta posisi yang keluar range naik ke puncak, karena itu yang
/// butuh tindakan. Posisi kosong turun ke bawah, karena ia hanya sisa yang
/// menunggu ditutup.
pub fn urutkan(posisi: &mut [Posisi]) {
    posisi.sort_by(|a, b| {
        let prioritas = |p: &Posisi| -> u8 {
            if p.kosong {
                2
            } else if !p.in_range {
                0
            } else {
                1
            }
        };
        prioritas(a).cmp(&prioritas(b)).then_with(|| {
            let l = |p: &Posisi| p.liquidity.parse::<u128>().unwrap_or(0);
            l(b).cmp(&l(a))
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(nama: &str, in_range: bool, kosong: bool, liq: &str) -> Posisi {
        Posisi {
            token_id: nama.into(),
            version: PoolVersion::V4,
            token0: "0x0".into(),
            token1: "0x1".into(),
            fee: 3000,
            tick_lower: 0,
            tick_upper: 100,
            liquidity: liq.into(),
            tick_sekarang: Some(50),
            in_range,
            fee0: "0".into(),
            fee1: "0".into(),
            hooks: None,
            sqrt_price_x96: None,
            amount0: "0".into(),
            amount1: "0".into(),
            pnl: None,
            kosong,
        }
    }

    #[test]
    fn keluar_range_naik_ke_puncak() {
        // Posisi yang keluar range butuh tindakan, jadi ia yang harus terlihat
        // lebih dulu. Ini permintaan eksplisit di PRD 3.5.
        let mut v = vec![
            p("dalam", true, false, "100"),
            p("keluar", false, false, "1"),
            p("kosong", false, true, "0"),
        ];
        urutkan(&mut v);
        assert_eq!(v[0].token_id, "keluar");
        assert_eq!(v[1].token_id, "dalam");
        assert_eq!(v[2].token_id, "kosong", "posisi kosong turun ke bawah");
    }

    #[test]
    fn likuiditas_besar_lebih_dulu_pada_prioritas_sama() {
        let mut v = vec![p("kecil", true, false, "5"), p("besar", true, false, "500")];
        urutkan(&mut v);
        assert_eq!(v[0].token_id, "besar");
    }

    #[test]
    fn posisi_kosong_tidak_naik_meski_keluar_range() {
        // Posisi tanpa likuiditas selalu di luar range menurut hitungan, tapi ia
        // tidak butuh tindakan apa pun selain ditutup.
        let mut v = vec![p("kosong", false, true, "0"), p("dalam", true, false, "1")];
        urutkan(&mut v);
        assert_eq!(v[0].token_id, "dalam");
    }

    #[test]
    fn pool_id_ikut_menghitung_hook() {
        // poolId adalah hash kelima field PoolKey. Hook yang berbeda berarti
        // pool yang berbeda, dan fee yang dibaca akan datang dari pool lain.
        use alloy::primitives::aliases::{I24, U24};
        let dasar = PmV4::PoolKey {
            currency0: Address::ZERO,
            currency1: Address::from([1u8; 20]),
            fee: U24::from(3000u32),
            tickSpacing: I24::try_from(60i32).unwrap(),
            hooks: Address::ZERO,
        };
        let mut lain = dasar.clone();
        lain.hooks = Address::from([9u8; 20]);
        assert_ne!(pool_id_dari(&dasar), pool_id_dari(&lain));
    }

    #[test]
    fn pool_id_sama_untuk_kunci_yang_sama() {
        use alloy::primitives::aliases::{I24, U24};
        let k = PmV4::PoolKey {
            currency0: Address::ZERO,
            currency1: Address::from([1u8; 20]),
            fee: U24::from(3000u32),
            tickSpacing: I24::try_from(60i32).unwrap(),
            hooks: Address::ZERO,
        };
        assert_eq!(pool_id_dari(&k), pool_id_dari(&k.clone()));
    }
}
