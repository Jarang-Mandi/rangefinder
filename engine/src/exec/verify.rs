//! Gate G7: verifikasi pasca kirim.
//!
//! INVARIAN
//! - Receipt yang berstatus sukses BELUM cukup. Transaksi bisa berhasil sambil
//!   menghasilkan posisi yang berbeda dari niat kita, misalnya karena
//!   parameternya tertukar. Satu satunya bukti adalah membaca ulang posisinya
//!   dari rantai dan membandingkannya.
//! - Selisih apa pun di sini menghentikan otomasi, bukan sekadar memberi
//!   peringatan. Kalau apa yang tercipta berbeda dari yang diminta, ada yang
//!   salah pada jalur eksekusi, dan meneruskan berarti mengulangi kesalahan
//!   yang sama dengan uang lagi.

use alloy::primitives::{Address, U256};
use alloy::providers::Provider;

use super::abi_write::INonfungiblePositionManager as Npm;
use super::calldata::MintRequest;

/// Posisi seperti yang benar benar tercatat di rantai.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnChainPosition {
    pub token_id: U256,
    pub token0: Address,
    pub token1: Address,
    pub fee: u32,
    pub tick_lower: i32,
    pub tick_upper: i32,
    pub liquidity: u128,
    pub tokens_owed0: u128,
    pub tokens_owed1: u128,
}

/// Baca posisi dari NonfungiblePositionManager.
pub async fn read_position<P: Provider>(
    provider: &P,
    position_manager: Address,
    token_id: U256,
) -> Result<OnChainPosition, String> {
    let npm = Npm::new(position_manager, provider);
    let p = npm
        .positions(token_id)
        .call()
        .await
        .map_err(|e| format!("gagal membaca posisi {token_id}: {e}"))?;

    Ok(OnChainPosition {
        token_id,
        token0: p.token0,
        token1: p.token1,
        fee: p.fee.to::<u32>(),
        tick_lower: p.tickLower.as_i32(),
        tick_upper: p.tickUpper.as_i32(),
        liquidity: p.liquidity,
        tokens_owed0: p.tokensOwed0,
        tokens_owed1: p.tokensOwed1,
    })
}

/// GATE G7. Bandingkan posisi yang tercipta dengan yang diminta.
///
/// Mengembalikan semua selisih sekaligus, karena beberapa field yang tertukar
/// biasanya berasal dari satu kesalahan yang sama, dan melihat semuanya
/// membantu menemukannya.
pub fn check_matches_request(
    actual: &OnChainPosition,
    requested: &MintRequest,
    expected_liquidity: u128,
) -> Result<(), Vec<String>> {
    let mut selisih = Vec::new();

    if actual.token0 != requested.token0 {
        selisih.push(format!(
            "token0 tercatat {} padahal diminta {}",
            actual.token0, requested.token0
        ));
    }
    if actual.token1 != requested.token1 {
        selisih.push(format!(
            "token1 tercatat {} padahal diminta {}",
            actual.token1, requested.token1
        ));
    }
    if actual.fee != requested.fee {
        selisih.push(format!(
            "fee tier tercatat {} padahal diminta {}",
            actual.fee, requested.fee
        ));
    }
    if actual.tick_lower != requested.tick_lower {
        selisih.push(format!(
            "tick bawah tercatat {} padahal diminta {}",
            actual.tick_lower, requested.tick_lower
        ));
    }
    if actual.tick_upper != requested.tick_upper {
        selisih.push(format!(
            "tick atas tercatat {} padahal diminta {}",
            actual.tick_upper, requested.tick_upper
        ));
    }

    if actual.liquidity == 0 {
        selisih.push("posisi tercipta tanpa likuiditas".into());
    } else if actual.liquidity != expected_liquidity {
        // Simulasi dan hasil sebenarnya bisa berbeda tipis kalau ada transaksi
        // lain di blok yang sama. Selisih kecil ditoleransi, selisih besar
        // berarti ada yang salah.
        let beda = actual.liquidity.abs_diff(expected_liquidity);
        let toleransi = expected_liquidity / 100; // satu persen
        if beda > toleransi {
            selisih.push(format!(
                "likuiditas tercatat {} menyimpang {} dari perkiraan {}, melebihi toleransi {}",
                actual.liquidity, beda, expected_liquidity, toleransi
            ));
        }
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

    fn t0() -> Address {
        address!("0bd7d308f8e1639fab988df18a8011f41eacad73")
    }
    fn t1() -> Address {
        address!("39dbed3a2bd333467115de45665cc57f813c4571")
    }

    fn permintaan() -> MintRequest {
        MintRequest {
            token0: t0(),
            token1: t1(),
            fee: 3_000,
            tick_lower: 76_200,
            tick_upper: 81_000,
            amount0_desired: U256::from(1_000u32),
            amount1_desired: U256::from(2_000u32),
            amount0_min: U256::ZERO,
            amount1_min: U256::ZERO,
            recipient: t0(),
            deadline: U256::from(1u8),
        }
    }

    fn posisi(liquidity: u128) -> OnChainPosition {
        OnChainPosition {
            token_id: U256::from(7u8),
            token0: t0(),
            token1: t1(),
            fee: 3_000,
            tick_lower: 76_200,
            tick_upper: 81_000,
            liquidity,
            tokens_owed0: 0,
            tokens_owed1: 0,
        }
    }

    #[test]
    fn posisi_yang_cocok_lolos() {
        assert!(check_matches_request(&posisi(100_000), &permintaan(), 100_000).is_ok());
    }

    #[test]
    fn selisih_likuiditas_kecil_ditoleransi() {
        // Transaksi lain di blok yang sama bisa menggeser hasilnya sedikit.
        assert!(check_matches_request(&posisi(100_500), &permintaan(), 100_000).is_ok());
        assert!(check_matches_request(&posisi(99_500), &permintaan(), 100_000).is_ok());
    }

    #[test]
    fn selisih_likuiditas_besar_ditolak() {
        let e = check_matches_request(&posisi(150_000), &permintaan(), 100_000).unwrap_err();
        assert!(e.iter().any(|m| m.contains("menyimpang")));
    }

    #[test]
    fn likuiditas_nol_ditolak() {
        let e = check_matches_request(&posisi(0), &permintaan(), 100_000).unwrap_err();
        assert!(e.iter().any(|m| m.contains("tanpa likuiditas")));
    }

    #[test]
    fn tick_yang_berbeda_ditolak() {
        let mut p = posisi(100_000);
        p.tick_lower = 76_260;
        let e = check_matches_request(&p, &permintaan(), 100_000).unwrap_err();
        assert!(e.iter().any(|m| m.contains("tick bawah")));
    }

    #[test]
    fn token_tertukar_ditolak() {
        // Inilah kesalahan yang paling mungkin terjadi dan paling mahal, karena
        // posisinya tetap tercipta dan transaksinya tetap sukses.
        let mut p = posisi(100_000);
        std::mem::swap(&mut p.token0, &mut p.token1);
        let e = check_matches_request(&p, &permintaan(), 100_000).unwrap_err();
        assert_eq!(e.len(), 2, "kedua token harus dilaporkan: {e:?}");
        assert!(e.iter().any(|m| m.contains("token0")));
        assert!(e.iter().any(|m| m.contains("token1")));
    }

    #[test]
    fn fee_tier_berbeda_ditolak() {
        let mut p = posisi(100_000);
        p.fee = 10_000;
        let e = check_matches_request(&p, &permintaan(), 100_000).unwrap_err();
        assert!(e.iter().any(|m| m.contains("fee tier")));
    }

    #[test]
    fn semua_selisih_dilaporkan_sekaligus() {
        let mut p = posisi(0);
        p.fee = 500;
        p.tick_lower = 1;
        p.tick_upper = 2;
        let e = check_matches_request(&p, &permintaan(), 100_000).unwrap_err();
        assert_eq!(e.len(), 4, "fee, dua tick, dan likuiditas: {e:?}");
    }
}
