//! Matematika likuiditas bilangan bulat, salinan LiquidityAmounts.sol.
//!
//! INVARIAN
//! - Nilai dari modul ini yang dikirim ke rantai. Pembulatannya harus sama
//!   persis dengan kontrak, karena selisih satu satuan terkecil bisa membuat
//!   `mint` ditolak oleh pemeriksaan slippage.
//! - Perkalian dua nilai 256 bit meluap. Semua perkalian di sini lewat 512 bit
//!   dulu, persis seperti `FullMath.mulDiv` di kontrak aslinya. Memakai U256
//!   langsung akan meluap diam diam pada nilai yang wajar, dan itu jenis bug
//!   yang baru terlihat saat dana sudah bergerak.
//! - Urutan batas tidak boleh diandalkan. Setiap fungsi menukar sendiri kalau
//!   `sqrt_a > sqrt_b`, sama seperti kontraknya.

use alloy::primitives::{U256, U512};

/// 2^96, basis format Q64.96.
pub fn q96() -> U256 {
    U256::from(1u8) << 96
}

/// Naikkan ke 512 bit lewat limb.
///
/// ruint tidak menyediakan konversi antar lebar `Uint`, jadi limbnya disalin
/// langsung. Empat limb bawah berisi nilainya, empat limb atas nol.
fn ke_u512(v: U256) -> U512 {
    let l = v.into_limbs();
    U512::from_limbs([l[0], l[1], l[2], l[3], 0, 0, 0, 0])
}

/// `a * b / denom` dengan hasil antara 512 bit.
///
/// Ini yang mencegah luapan. `sqrt_a * sqrt_b` saja sudah bisa melampaui 256
/// bit pada harga yang sepenuhnya wajar, dan luapan di sini tidak akan terlihat
/// sampai dana sudah bergerak.
pub fn mul_div(a: U256, b: U256, denom: U256) -> Result<U256, String> {
    if denom.is_zero() {
        return Err("pembagi nol pada mul_div".into());
    }

    let hasil = ke_u512(a) * ke_u512(b) / ke_u512(denom);
    let limbs = hasil.into_limbs();

    // Empat limb atas wajib nol. Kalau tidak, hasilnya tidak muat di 256 bit,
    // dan memotongnya diam diam akan menghasilkan angka yang salah total.
    if limbs[4..].iter().any(|&l| l != 0) {
        return Err(format!(
            "hasil mul_div melampaui 256 bit: {a} x {b} / {denom}"
        ));
    }

    Ok(U256::from_limbs([limbs[0], limbs[1], limbs[2], limbs[3]]))
}

fn urutkan(sqrt_a: U256, sqrt_b: U256) -> (U256, U256) {
    if sqrt_a > sqrt_b {
        (sqrt_b, sqrt_a)
    } else {
        (sqrt_a, sqrt_b)
    }
}

fn ke_u128(v: U256) -> Result<u128, String> {
    u128::try_from(v).map_err(|_| format!("likuiditas {v} melampaui u128"))
}

/// Likuiditas dari sejumlah token0 pada rentang tertentu.
pub fn liquidity_for_amount0(sqrt_a: U256, sqrt_b: U256, amount0: U256) -> Result<u128, String> {
    let (lo, hi) = urutkan(sqrt_a, sqrt_b);
    if lo == hi {
        return Err("rentang berlebar nol tidak punya likuiditas".into());
    }
    let antara = mul_div(lo, hi, q96())?;
    ke_u128(mul_div(amount0, antara, hi - lo)?)
}

/// Likuiditas dari sejumlah token1 pada rentang tertentu.
pub fn liquidity_for_amount1(sqrt_a: U256, sqrt_b: U256, amount1: U256) -> Result<u128, String> {
    let (lo, hi) = urutkan(sqrt_a, sqrt_b);
    if lo == hi {
        return Err("rentang berlebar nol tidak punya likuiditas".into());
    }
    ke_u128(mul_div(amount1, q96(), hi - lo)?)
}

/// Likuiditas terbesar yang muat pada kedua jumlah token sekaligus.
///
/// Saat harga berada di dalam rentang, yang menentukan adalah sisi yang paling
/// membatasi, jadi hasilnya minimum dari keduanya. Memakai yang lebih besar
/// akan meminta token yang tidak kita punya.
pub fn liquidity_for_amounts(
    sqrt_p: U256,
    sqrt_a: U256,
    sqrt_b: U256,
    amount0: U256,
    amount1: U256,
) -> Result<u128, String> {
    let (lo, hi) = urutkan(sqrt_a, sqrt_b);

    if sqrt_p <= lo {
        // Harga di bawah rentang, seluruhnya token0.
        liquidity_for_amount0(lo, hi, amount0)
    } else if sqrt_p < hi {
        let l0 = liquidity_for_amount0(sqrt_p, hi, amount0)?;
        let l1 = liquidity_for_amount1(lo, sqrt_p, amount1)?;
        Ok(l0.min(l1))
    } else {
        // Harga di atas rentang, seluruhnya token1.
        liquidity_for_amount1(lo, hi, amount1)
    }
}

/// Jumlah token0 yang dibutuhkan untuk likuiditas tertentu.
pub fn amount0_for_liquidity(sqrt_a: U256, sqrt_b: U256, liquidity: u128) -> Result<U256, String> {
    let (lo, hi) = urutkan(sqrt_a, sqrt_b);
    if lo.is_zero() {
        return Err("batas bawah nol tidak sah".into());
    }
    let pembilang = U256::from(liquidity) << 96;
    Ok(mul_div(pembilang, hi - lo, hi)? / lo)
}

/// Jumlah token1 yang dibutuhkan untuk likuiditas tertentu.
pub fn amount1_for_liquidity(sqrt_a: U256, sqrt_b: U256, liquidity: u128) -> Result<U256, String> {
    let (lo, hi) = urutkan(sqrt_a, sqrt_b);
    mul_div(U256::from(liquidity), hi - lo, q96())
}

/// Kedua jumlah token untuk likuiditas tertentu pada harga tertentu.
pub fn amounts_for_liquidity(
    sqrt_p: U256,
    sqrt_a: U256,
    sqrt_b: U256,
    liquidity: u128,
) -> Result<(U256, U256), String> {
    let (lo, hi) = urutkan(sqrt_a, sqrt_b);

    if sqrt_p <= lo {
        Ok((amount0_for_liquidity(lo, hi, liquidity)?, U256::ZERO))
    } else if sqrt_p < hi {
        Ok((
            amount0_for_liquidity(sqrt_p, hi, liquidity)?,
            amount1_for_liquidity(lo, sqrt_p, liquidity)?,
        ))
    } else {
        Ok((U256::ZERO, amount1_for_liquidity(lo, hi, liquidity)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::tick_math::get_sqrt_ratio_at_tick;

    fn sqrt(tick: i32) -> U256 {
        get_sqrt_ratio_at_tick(tick).unwrap()
    }

    /// Satu token penuh dengan 18 desimal.
    fn satu_token() -> U256 {
        U256::from(10u64).pow(U256::from(18u8))
    }

    #[test]
    fn mul_div_tidak_meluap_pada_nilai_besar() {
        // Dua nilai yang perkaliannya jauh melampaui 256 bit. Tanpa hasil
        // antara 512 bit, ini akan meluap diam diam.
        let a = U256::MAX / U256::from(2u8);
        let b = U256::from(4u8);
        let hasil = mul_div(a, b, U256::from(8u8)).unwrap();
        assert_eq!(hasil, a / U256::from(2u8));
    }

    #[test]
    fn mul_div_menolak_pembagi_nol() {
        assert!(mul_div(U256::from(1u8), U256::from(1u8), U256::ZERO).is_err());
    }

    #[test]
    fn mul_div_melaporkan_hasil_yang_tidak_muat() {
        // Hasil yang melampaui 256 bit harus jadi error, bukan terpotong.
        let hasil = mul_div(U256::MAX, U256::from(2u8), U256::from(1u8));
        assert!(hasil.is_err());
    }

    #[test]
    fn urutan_batas_tidak_berpengaruh() {
        let (a, b) = (sqrt(-600), sqrt(600));
        let maju = liquidity_for_amount1(a, b, satu_token()).unwrap();
        let mundur = liquidity_for_amount1(b, a, satu_token()).unwrap();
        assert_eq!(maju, mundur);
    }

    #[test]
    fn rentang_berlebar_nol_ditolak() {
        let a = sqrt(0);
        assert!(liquidity_for_amount0(a, a, satu_token()).is_err());
        assert!(liquidity_for_amount1(a, a, satu_token()).is_err());
    }

    #[test]
    fn harga_di_bawah_rentang_hanya_butuh_token0() {
        let (a, b) = (sqrt(600), sqrt(1200));
        let p = sqrt(0);
        let l = liquidity_for_amounts(p, a, b, satu_token(), U256::ZERO).unwrap();
        assert!(l > 0);

        let (amount0, amount1) = amounts_for_liquidity(p, a, b, l).unwrap();
        assert!(amount0 > U256::ZERO);
        assert_eq!(amount1, U256::ZERO, "di bawah rentang tidak butuh token1");
    }

    #[test]
    fn harga_di_atas_rentang_hanya_butuh_token1() {
        let (a, b) = (sqrt(-1200), sqrt(-600));
        let p = sqrt(0);
        let l = liquidity_for_amounts(p, a, b, U256::ZERO, satu_token()).unwrap();
        assert!(l > 0);

        let (amount0, amount1) = amounts_for_liquidity(p, a, b, l).unwrap();
        assert_eq!(amount0, U256::ZERO, "di atas rentang tidak butuh token0");
        assert!(amount1 > U256::ZERO);
    }

    #[test]
    fn harga_di_dalam_rentang_butuh_keduanya() {
        let (a, b) = (sqrt(-600), sqrt(600));
        let p = sqrt(0);
        let l = liquidity_for_amounts(p, a, b, satu_token(), satu_token()).unwrap();
        assert!(l > 0);

        let (amount0, amount1) = amounts_for_liquidity(p, a, b, l).unwrap();
        assert!(amount0 > U256::ZERO && amount1 > U256::ZERO);
    }

    /// Uji putaran penuh: jumlah token menjadi likuiditas, lalu kembali.
    ///
    /// Hasilnya tidak akan sama persis karena kontraknya membulatkan ke bawah,
    /// tapi tidak boleh pernah LEBIH BESAR dari yang kita punya. Kalau lebih
    /// besar, `mint` akan meminta token yang tidak ada dan transaksinya gagal.
    #[test]
    fn putaran_penuh_tidak_pernah_meminta_lebih_dari_yang_ada() {
        let kasus = [
            (-600, 600, 0),
            (-60, 60, 0),
            (0, 1200, 600),
            (-1200, 0, -600),
            (76_200, 81_000, 78_600),
        ];

        for (tick_a, tick_b, tick_p) in kasus {
            let (a, b, p) = (sqrt(tick_a), sqrt(tick_b), sqrt(tick_p));
            let punya0 = satu_token();
            let punya1 = satu_token();

            let l = liquidity_for_amounts(p, a, b, punya0, punya1).unwrap();
            let (butuh0, butuh1) = amounts_for_liquidity(p, a, b, l).unwrap();

            assert!(
                butuh0 <= punya0,
                "rentang {tick_a}..{tick_b} pada {tick_p}: butuh token0 {butuh0} melebihi {punya0}"
            );
            assert!(
                butuh1 <= punya1,
                "rentang {tick_a}..{tick_b} pada {tick_p}: butuh token1 {butuh1} melebihi {punya1}"
            );
        }
    }

    #[test]
    fn likuiditas_naik_seiring_modal() {
        let (a, b, p) = (sqrt(-600), sqrt(600), sqrt(0));
        let kecil = liquidity_for_amounts(p, a, b, satu_token(), satu_token()).unwrap();
        let besar = liquidity_for_amounts(
            p,
            a,
            b,
            satu_token() * U256::from(10u8),
            satu_token() * U256::from(10u8),
        )
        .unwrap();
        assert!(besar > kecil);
        // Skalanya harus mendekati linear.
        let rasio = besar as f64 / kecil as f64;
        assert!((rasio - 10.0).abs() < 0.01, "rasio {rasio} tidak mendekati 10");
    }

    #[test]
    fn rentang_lebih_rapat_memberi_likuiditas_lebih_besar() {
        // Inti dari concentrated liquidity, dan dasar angka faktor konsentrasi
        // yang ditampilkan panel. Lihat PRD 5.7.
        let p = sqrt(0);
        let lebar = liquidity_for_amounts(p, sqrt(-6000), sqrt(6000), satu_token(), satu_token())
            .unwrap();
        let rapat =
            liquidity_for_amounts(p, sqrt(-600), sqrt(600), satu_token(), satu_token()).unwrap();
        assert!(
            rapat > lebar,
            "rentang rapat {rapat} seharusnya melebihi rentang lebar {lebar}"
        );
    }

    #[test]
    fn sisi_yang_paling_membatasi_yang_menentukan() {
        let (a, b, p) = (sqrt(-600), sqrt(600), sqrt(0));
        // Token1 dibuat sangat sedikit, jadi dialah yang membatasi.
        let l = liquidity_for_amounts(p, a, b, satu_token(), U256::from(1u8)).unwrap();
        let hanya1 = liquidity_for_amount1(a, p, U256::from(1u8)).unwrap();
        assert_eq!(l, hanya1, "hasilnya harus mengikuti sisi yang membatasi");
    }
}
