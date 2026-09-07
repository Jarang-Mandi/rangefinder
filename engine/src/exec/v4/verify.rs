//! Gate G7 untuk V4: verifikasi pasca kirim.
//!
//! KENAPA TIDAK CUKUP RECEIPT
//! `modifyLiquidities` tidak mengembalikan apa apa, jadi seluruh keyakinan
//! sebelum pengiriman berasal dari simulasi dan hitungan kita sendiri.
//! Satu satunya bukti bahwa posisi yang tercipta memang yang kita minta adalah
//! membacanya kembali dari rantai.
//!
//! Yang dibandingkan: PoolKey lengkap termasuk hook, kedua tick, likuiditas,
//! dan pemiliknya. PoolKey ikut karena poolId adalah hash dari kelimanya, jadi
//! satu field yang tertukar berarti posisi kita berada di pool yang lain.
//!
//! INVARIAN
//! - Selisih apa pun menghentikan otomasi, bukan sekadar memberi peringatan.
//! - Tick dibandingkan setelah dikeluarkan dari `PositionInfo`, yang mengemas
//!   keduanya sebagai int24 di dalam satu uint256.

use alloy::primitives::{Address, U256};
use alloy::providers::Provider;

use super::abi::IPositionManagerV4 as Pm;
use super::calldata::MintV4Request;

/// Posisi V4 seperti yang benar benar tercatat di rantai.
#[derive(Debug, Clone, PartialEq)]
pub struct PosisiV4 {
    pub token_id: U256,
    pub currency0: Address,
    pub currency1: Address,
    pub fee: u32,
    pub tick_spacing: i32,
    pub hooks: Address,
    pub tick_lower: i32,
    pub tick_upper: i32,
    pub liquidity: u128,
    pub owner: Address,
}

/// Bongkar tick dari `PositionInfo`.
///
/// Tata letaknya, dari bit tertinggi ke terendah:
/// 200 bit poolId terpotong, 24 bit tickUpper, 24 bit tickLower, 8 bit flag.
/// Keduanya int24, jadi bit ke-23 adalah tandanya.
pub fn bongkar_tick(info: U256) -> (i32, i32) {
    let ambil24 = |geser: u32| -> i32 {
        let mentah = ((info >> geser) & U256::from(0xFF_FFFFu32)).to::<u32>();
        // Dua pelengkap 24 bit: kalau bit tanda menyala, kurangi 2^24.
        if mentah & 0x80_0000 != 0 {
            mentah as i64 as i32 - (1 << 24)
        } else {
            mentah as i32
        }
    };
    // tickLower di bit 8, tickUpper di bit 32.
    (ambil24(8), ambil24(32))
}

/// Baca posisi V4 dari rantai.
pub async fn baca_posisi<P: Provider>(
    provider: &P,
    position_manager: Address,
    multicall: Address,
    token_id: U256,
) -> Result<PosisiV4, String> {
    let calls = vec![
        crate::chain::multicall::encode(
            position_manager,
            &Pm::getPoolAndPositionInfoCall { tokenId: token_id },
        ),
        crate::chain::multicall::encode(
            position_manager,
            &Pm::getPositionLiquidityCall { tokenId: token_id },
        ),
        crate::chain::multicall::encode(position_manager, &Pm::ownerOfCall { tokenId: token_id }),
    ];

    let hasil = crate::chain::multicall::batch_call(provider, multicall, calls)
        .await
        .map_err(|e| format!("gagal membaca posisi {token_id}: {e}"))?;
    let ambil = |i: usize| hasil.get(i).and_then(|x| x.as_ref());

    let info = crate::chain::multicall::decode::<Pm::getPoolAndPositionInfoCall>(ambil(0))
        .ok_or_else(|| format!("getPoolAndPositionInfo({token_id}) tidak terbaca"))?;
    let liquidity = crate::chain::multicall::decode::<Pm::getPositionLiquidityCall>(ambil(1))
        .ok_or_else(|| format!("getPositionLiquidity({token_id}) tidak terbaca"))?;
    let owner = crate::chain::multicall::decode::<Pm::ownerOfCall>(ambil(2))
        .ok_or_else(|| format!("ownerOf({token_id}) tidak terbaca"))?;

    let (tick_lower, tick_upper) = bongkar_tick(info._1);

    Ok(PosisiV4 {
        token_id,
        currency0: info._0.currency0,
        currency1: info._0.currency1,
        fee: info._0.fee.to::<u32>(),
        tick_spacing: info._0.tickSpacing.as_i32(),
        hooks: info._0.hooks,
        tick_lower,
        tick_upper,
        liquidity,
        owner,
    })
}

/// GATE G7. Bandingkan posisi yang tercipta dengan yang diminta.
///
/// Mengembalikan seluruh selisih sekaligus, karena beberapa field yang tertukar
/// biasanya berasal dari satu kesalahan yang sama, dan melihat semuanya
/// membantu menemukannya.
pub fn cocok_dengan_permintaan(
    aktual: &PosisiV4,
    diminta: &MintV4Request,
) -> Result<(), Vec<String>> {
    let mut selisih = Vec::new();

    if aktual.currency0 != diminta.pool_key.currency0 {
        selisih.push(format!(
            "currency0 {} berbeda dari yang diminta {}",
            aktual.currency0, diminta.pool_key.currency0
        ));
    }
    if aktual.currency1 != diminta.pool_key.currency1 {
        selisih.push(format!(
            "currency1 {} berbeda dari yang diminta {}",
            aktual.currency1, diminta.pool_key.currency1
        ));
    }
    let fee_diminta = diminta.pool_key.fee.to::<u32>();
    if aktual.fee != fee_diminta {
        selisih.push(format!(
            "fee {} berbeda dari yang diminta {fee_diminta}",
            aktual.fee
        ));
    }
    let spacing_diminta = diminta.pool_key.tickSpacing.as_i32();
    if aktual.tick_spacing != spacing_diminta {
        selisih.push(format!(
            "tickSpacing {} berbeda dari yang diminta {spacing_diminta}",
            aktual.tick_spacing
        ));
    }
    // Hook ikut diperiksa karena ia bagian dari poolId. Hook yang berbeda
    // berarti pool yang berbeda, betapapun mirip sisanya.
    if aktual.hooks != diminta.pool_key.hooks {
        selisih.push(format!(
            "hooks {} berbeda dari yang diminta {}",
            aktual.hooks, diminta.pool_key.hooks
        ));
    }
    if aktual.tick_lower != diminta.tick_lower {
        selisih.push(format!(
            "tickLower {} berbeda dari yang diminta {}",
            aktual.tick_lower, diminta.tick_lower
        ));
    }
    if aktual.tick_upper != diminta.tick_upper {
        selisih.push(format!(
            "tickUpper {} berbeda dari yang diminta {}",
            aktual.tick_upper, diminta.tick_upper
        ));
    }
    if aktual.liquidity != diminta.liquidity {
        selisih.push(format!(
            "likuiditas {} berbeda dari yang diminta {}",
            aktual.liquidity, diminta.liquidity
        ));
    }
    if aktual.owner != diminta.owner {
        selisih.push(format!(
            "pemilik {} berbeda dari yang diminta {}",
            aktual.owner, diminta.owner
        ));
    }

    if selisih.is_empty() {
        Ok(())
    } else {
        Err(selisih)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;
    use alloy::primitives::aliases::{I24, U24};

    use super::super::abi::IPositionManagerV4::PoolKey;

    fn diminta() -> MintV4Request {
        MintV4Request {
            pool_key: PoolKey {
                currency0: address!("5fc5360d0400a0fd4f2af552add042d716f1d168"),
                currency1: address!("791e750fa8b2237288a6fe6c47ea81c50093449a"),
                fee: U24::from(100_000u32),
                tickSpacing: I24::try_from(1000i32).unwrap(),
                hooks: Address::ZERO,
            },
            tick_lower: 357_000,
            tick_upper: 367_000,
            liquidity: 14_279_001_511_288_904,
            amount0_max: 100_000_000,
            amount1_max: 0,
            owner: address!("420ac1fcca78c4cdfed3216201216ed80e7b2680"),
            deadline: U256::from(1u8),
        }
    }

    fn aktual() -> PosisiV4 {
        let d = diminta();
        PosisiV4 {
            token_id: U256::from(7u8),
            currency0: d.pool_key.currency0,
            currency1: d.pool_key.currency1,
            fee: 100_000,
            tick_spacing: 1000,
            hooks: Address::ZERO,
            tick_lower: d.tick_lower,
            tick_upper: d.tick_upper,
            liquidity: d.liquidity,
            owner: d.owner,
        }
    }

    #[test]
    fn posisi_yang_sama_persis_lolos() {
        assert!(cocok_dengan_permintaan(&aktual(), &diminta()).is_ok());
    }

    #[test]
    fn hook_berbeda_ditolak() {
        // Hook bagian dari poolId. Hook yang berbeda berarti pool yang berbeda,
        // dan dana kita berada di tempat yang tidak kita maksud.
        let mut a = aktual();
        a.hooks = address!("00000000000000000000000000000000000000ff");
        let e = cocok_dengan_permintaan(&a, &diminta()).unwrap_err();
        assert!(e.iter().any(|s| s.contains("hooks")));
    }

    #[test]
    fn tick_tertukar_dilaporkan_keduanya() {
        let mut a = aktual();
        std::mem::swap(&mut a.tick_lower, &mut a.tick_upper);
        let e = cocok_dengan_permintaan(&a, &diminta()).unwrap_err();
        assert_eq!(e.len(), 2, "dua tick yang tertukar berarti dua selisih");
    }

    #[test]
    fn likuiditas_berbeda_ditolak() {
        let mut a = aktual();
        a.liquidity -= 1;
        assert!(cocok_dengan_permintaan(&a, &diminta()).is_err());
    }

    #[test]
    fn pemilik_berbeda_ditolak() {
        // Posisi yang tercipta atas nama orang lain adalah kehilangan penuh.
        let mut a = aktual();
        a.owner = Address::ZERO;
        let e = cocok_dengan_permintaan(&a, &diminta()).unwrap_err();
        assert!(e.iter().any(|s| s.contains("pemilik")));
    }

    #[test]
    fn bongkar_tick_dari_posisi_sungguhan() {
        // PositionInfo posisi V4 nyata di Robinhood Chain, tokenId 2100029,
        // dibaca lewat `cargo xtask v4probe`. Ini bukti terkuat bahwa tata
        // letak bitnya benar, karena ia datang dari kontrak, bukan dari kita.
        let info: U256 = "0x0f841897ebec50b6ea31047df4269dcb675f63d941b9014f8805822805728800"
            .parse()
            .unwrap();
        assert_eq!(bongkar_tick(info), (357_000, 361_000));
    }

    #[test]
    fn bongkar_tick_positif() {
        // tickLower 357000 di bit 8, tickUpper 367000 di bit 32.
        let info = (U256::from(367_000u32) << 32) | (U256::from(357_000u32) << 8);
        assert_eq!(bongkar_tick(info), (357_000, 367_000));
    }

    #[test]
    fn bongkar_tick_negatif() {
        // Tick negatif tersimpan sebagai dua pelengkap 24 bit. Salah membacanya
        // menghasilkan angka delapan juta, dan range di tempat yang sama sekali
        // berbeda.
        let neg = |v: i32| U256::from((v & 0xFF_FFFF) as u32);
        let info = (neg(-100) << 32) | (neg(-200) << 8);
        assert_eq!(bongkar_tick(info), (-200, -100));
    }

    #[test]
    fn bongkar_tick_mengabaikan_bit_lain() {
        // Delapan bit terbawah adalah flag, dan 200 bit teratas poolId. Tidak
        // satu pun boleh bocor ke nilai tick.
        let info = (U256::from(1u8) << 255)
            | (U256::from(367_000u32) << 32)
            | (U256::from(357_000u32) << 8)
            | U256::from(0xFFu8);
        assert_eq!(bongkar_tick(info), (357_000, 367_000));
    }
}
