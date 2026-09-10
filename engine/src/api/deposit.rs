//! Anggaran setoran dari satu kotak yang dipilih user. Saldo wallet adalah
//! batas kemampuan, bukan jumlah yang boleh dibelanjakan.
use crate::math::{liquidity_math as lm, tick_math};
use alloy::primitives::U256;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Meme,
    Quote,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Deposit {
    pub side: Side,
    pub amount: String,
    pub zap: bool,
}

/// Desimal ditafsirkan dengan bilangan bulat. Tidak melewati f64, termasuk
/// pecahan kecil dan angka di atas batas presisi JavaScript.
pub fn units(text: &str, decimals: u8) -> Result<U256, String> {
    let (whole, fraction) = text.trim().split_once('.').unwrap_or((text.trim(), ""));
    if decimals > 77
        || whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
        || fraction.len() > decimals as usize
        || whole.len() + fraction.len() > 78
    {
        return Err(format!(
            "jumlah harus desimal positif, maksimal {decimals} angka di belakang koma"
        ));
    }
    let digits = format!(
        "{whole}{fraction}{}",
        "0".repeat(decimals as usize - fraction.len())
    );
    let amount = U256::from_str_radix(&digits, 10).map_err(|_| "jumlah melampaui uint256")?;
    if amount.is_zero() {
        return Err("jumlah harus lebih besar dari nol".into());
    }
    Ok(amount)
}

/// Tampilkan unit terkecil sebagai desimal tanpa melewati `f64`.
pub fn display_units(value: U256, decimals: u8) -> String {
    if decimals == 0 {
        return value.to_string();
    }
    let mut digits = value.to_string();
    let decimals = usize::from(decimals);
    if digits.len() <= decimals {
        digits = format!("{}{}", "0".repeat(decimals + 1 - digits.len()), digits);
    }
    let split = digits.len() - decimals;
    let whole = &digits[..split];
    let fraction = digits[split..].trim_end_matches('0');
    if fraction.is_empty() {
        whole.to_string()
    } else {
        format!("{whole}.{fraction}")
    }
}

/// Sisi lain diturunkan dari likuiditas untuk range yang sama. Dengan zap,
/// seluruh anggaran berasal hanya dari aset yang dipilih.
pub fn budget(
    pool: &crate::pools::PoolInfo,
    lower: i32,
    upper: i32,
    amount: U256,
    source0: bool,
    zap: bool,
) -> Result<(U256, U256), String> {
    if zap {
        return Ok(if source0 {
            (amount, U256::ZERO)
        } else {
            (U256::ZERO, amount)
        });
    }
    let p: U256 = pool
        .sqrt_price_x96
        .parse()
        .map_err(|_| "harga pool tidak valid")?;
    let a = tick_math::get_sqrt_ratio_at_tick(lower)?;
    let b = tick_math::get_sqrt_ratio_at_tick(upper)?;
    let l = if source0 {
        if p >= b {
            return Err(
                "range ini hanya menerima token1, pilih sisi setoran yang lain atau aktifkan zap"
                    .into(),
            );
        }
        lm::liquidity_for_amount0(p.max(a), b, amount)?
    } else {
        if p <= a {
            return Err(
                "range ini hanya menerima token0, pilih sisi setoran yang lain atau aktifkan zap"
                    .into(),
            );
        }
        lm::liquidity_for_amount1(a, p.min(b), amount)?
    };
    if l == 0 {
        return Err("setoran terlalu kecil untuk range ini".into());
    }
    let (a0, a1) = lm::amounts_for_liquidity(p, a, b, l)?;
    // Tambah satu satuan di sisi turunan untuk pembulatan kontrak ke atas.
    Ok(if source0 {
        (amount, if a1.is_zero() { a1 } else { a1 + U256::from(1) })
    } else {
        (if a0.is_zero() { a0 } else { a0 + U256::from(1) }, amount)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn jumlah_besar_dan_pecahan_tidak_kehilangan_presisi() {
        assert_eq!(
            units("9007199254740993.000001", 6).unwrap().to_string(),
            "9007199254740993000001"
        );
        assert_eq!(units("0.000000000000000001", 18).unwrap(), U256::from(1));
        for input in ["1e3", "-1", "NaN", "0", "1.0000001"] {
            assert!(units(input, 6).is_err());
        }
    }
    #[test]
    fn unit_ditampilkan_tanpa_kehilangan_presisi() {
        assert_eq!(display_units(U256::from(1_230_000u64), 6), "1.23");
        assert_eq!(display_units(U256::from(1u8), 18), "0.000000000000000001");
        assert_eq!(display_units(U256::ZERO, 6), "0");
    }
    #[test]
    fn setoran_zap_tidak_memakai_saldo_sisi_lain() {
        let pool = crate::pools::PoolInfo {
            version: crate::pools::PoolVersion::V4,
            id: "uji".into(),
            token0: "meme".into(),
            token1: "quote".into(),
            fee: 3000,
            dynamic_fee: false,
            tick_spacing: 60,
            hooks: None,
            sqrt_price_x96: tick_math::get_sqrt_ratio_at_tick(0).unwrap().to_string(),
            tick: 0,
            liquidity: "1000000".into(),
            raw_price: 1.0,
            stats: None,
        };
        let amount = U256::from(100_000_000u64);
        for side0 in [true, false] {
            let (a, b) = budget(&pool, -600, 600, amount, side0, true).unwrap();
            assert_eq!(a + b, amount);
            assert_eq!(a.is_zero(), !side0);
            let (a, b) = budget(&pool, -600, 600, amount, side0, false).unwrap();
            assert!(a.abs_diff(b) <= U256::from(1));
            assert_eq!(if side0 { a } else { b }, amount);
        }
        let (a, b) = budget(&pool, -1200, -600, amount, false, false).unwrap();
        assert_eq!((a, b), (U256::ZERO, amount));
        assert!(budget(&pool, -1200, -600, amount, true, false).is_err());
    }
}
