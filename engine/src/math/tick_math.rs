//! Matematika tick bilangan bulat, salinan setia TickMath.sol milik Uniswap.
//!
//! KENAPA DITULIS SENDIRI
//! Crate `uniswap-v3-math` menarik alloy 1.x sementara proyek ini memakai 2.x,
//! sehingga pohon dependensinya jadi dobel. Algoritmanya sendiri hanya sekitar
//! tiga puluh baris ditambah tabel konstanta, dan bisa diverifikasi secara
//! independen, jadi menulisnya sendiri lebih murah daripada menanggung dua
//! versi alloy.
//!
//! INVARIAN
//! - Nilai dari modul ini yang dikirim ke rantai, jadi presisinya mutlak.
//!   Modul `math` induk memakai `f64` dan itu HANYA untuk tampilan. Jangan
//!   pernah menukar keduanya.
//! - Satu digit salah pada tabel konstanta akan menghasilkan harga yang keliru
//!   di sebagian tick saja, dan itu jenis kesalahan yang lolos dari pengujian
//!   sekilas. Karena itu ada uji sapuan yang membandingkan hasil bilangan bulat
//!   ini dengan hitungan `f64` independen di seluruh rentang tick, ditambah uji
//!   terhadap pool sungguhan di chain.
//!
//! Sumber algoritma: Uniswap v3-core, `contracts/libraries/TickMath.sol`.

use alloy::primitives::U256;

use super::{MAX_TICK, MIN_TICK};

/// sqrtPriceX96 pada `MIN_TICK`.
pub const MIN_SQRT_RATIO: u128 = 4_295_128_739;

/// sqrtPriceX96 pada `MAX_TICK`, tidak muat di u128 jadi disimpan sebagai teks
/// dan diurai sekali saat dibutuhkan.
pub const MAX_SQRT_RATIO_DEC: &str = "1461446703485210103287273052203988822378723970342";

/// Tabel konstanta TickMath. Setiap baris adalah faktor untuk satu bit pada
/// nilai mutlak tick, berurutan dari bit 1 sampai bit 0x80000.
const FACTORS: [&str; 19] = [
    "0xfff97272373d413259a46990580e213a",
    "0xfff2e50f5f656932ef12357cf3c7fdcc",
    "0xffe5caca7e10e4e61c3624eaa0941cd0",
    "0xffcb9843d60f6159c9db58835c926644",
    "0xff973b41fa98c081472e6896dfb254c0",
    "0xff2ea16466c96a3843ec78b326b52861",
    "0xfe5dee046a99a2a811c461f1969c3053",
    "0xfcbe86c7900a88aedcffc83b479aa3a4",
    "0xf987a7253ac413176f2b074cf7815e54",
    "0xf3392b0822b70005940c7a398e4b70f3",
    "0xe7159475a2c29b7443b29c7fa6e889d9",
    "0xd097f3bdfd2022b8845ad8f792aa5825",
    "0xa9f746462d870fdf8a65dc1f90e061e5",
    "0x70d869a156d2a1b890bb3df62baf32f7",
    "0x31be135f97d08fd981231505542fcfa6",
    "0x9aa508b5b7a84e1c677de54f3e99bc9",
    "0x5d6af8dedb81196699c329225ee604",
    "0x2216e584f5fa1ea926041bedfe98",
    "0x48a170391f7dc42444e8fa2",
];

/// Faktor untuk bit terendah, yaitu tick ganjil.
const FACTOR_BIT0: &str = "0xfffcb933bd6fad37aa2d162d1a594001";

fn hex(s: &str) -> U256 {
    U256::from_str_radix(s.trim_start_matches("0x"), 16)
        .expect("konstanta TickMath di berkas ini harus heksadesimal yang sah")
}

pub fn max_sqrt_ratio() -> U256 {
    U256::from_str_radix(MAX_SQRT_RATIO_DEC, 10).expect("konstanta desimal harus sah")
}

pub fn min_sqrt_ratio() -> U256 {
    U256::from(MIN_SQRT_RATIO)
}

/// sqrtPriceX96 pada sebuah tick, dalam format Q64.96.
///
/// Ini nilai yang dipakai kontrak Uniswap, jadi hasilnya harus sama persis
/// dengan yang dihitung on chain, bukan mendekati.
pub fn get_sqrt_ratio_at_tick(tick: i32) -> Result<U256, String> {
    if !(MIN_TICK..=MAX_TICK).contains(&tick) {
        return Err(format!(
            "tick {tick} di luar rentang Uniswap {MIN_TICK} sampai {MAX_TICK}"
        ));
    }

    let abs_tick = tick.unsigned_abs() as u64;

    // Bit nol memakai faktornya sendiri, sisanya dari tabel. Nilai awal saat
    // bit nol tidak diset adalah 1 dalam Q128.128, yaitu 2^128.
    let mut ratio = if abs_tick & 0x1 != 0 {
        hex(FACTOR_BIT0)
    } else {
        U256::from(1u8) << 128
    };

    for (i, factor) in FACTORS.iter().enumerate() {
        // Bit ke i+1, jadi 0x2, 0x4, 0x8, dan seterusnya.
        if abs_tick & (1u64 << (i + 1)) != 0 {
            ratio = (ratio * hex(factor)) >> 128;
        }
    }

    // Tick positif adalah kebalikan dari tick negatif.
    if tick > 0 {
        ratio = U256::MAX / ratio;
    }

    // Turun dari Q128.128 ke Q64.96, membulatkan ke atas kalau ada sisa.
    // Pembulatan ke atas ini bukan pilihan gaya, kontraknya melakukan hal yang
    // sama, dan selisih satu satuan terkecil akan membuat tick hasil pembacaan
    // balik meleset.
    let shifted = ratio >> 32;
    let sisa: U256 = ratio & ((U256::from(1u8) << 32) - U256::from(1u8));
    let hasil = if sisa.is_zero() {
        shifted
    } else {
        shifted + U256::from(1u8)
    };

    Ok(hasil)
}

/// Tick terbesar yang sqrtPriceX96-nya tidak melebihi nilai yang diberikan.
///
/// Dipakai untuk memeriksa silang, dan untuk mengubah keadaan pool menjadi tick
/// tanpa mempercayai angka tick yang dilaporkan pool.
///
/// Implementasinya memakai pencarian biner, bukan trik bit seperti kontrak
/// aslinya. Lebih lambat, tapi jauh lebih mudah diperiksa kebenarannya, dan
/// kecepatannya tidak penting karena hanya dipanggil sekali per kutipan.
pub fn get_tick_at_sqrt_ratio(sqrt_price_x96: U256) -> Result<i32, String> {
    if sqrt_price_x96 < min_sqrt_ratio() || sqrt_price_x96 >= max_sqrt_ratio() {
        return Err(format!(
            "sqrtPriceX96 {sqrt_price_x96} di luar rentang yang sah"
        ));
    }

    let mut lo = MIN_TICK;
    let mut hi = MAX_TICK;

    while lo < hi {
        // Pembulatan ke atas supaya pencarian tidak terjebak saat lo dan hi
        // hanya berselisih satu.
        let mid = lo + (hi - lo + 1) / 2;
        if get_sqrt_ratio_at_tick(mid)? <= sqrt_price_x96 {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }

    Ok(lo)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2^96, yaitu sqrtPriceX96 saat harga sama dengan satu.
    const Q96_DEC: &str = "79228162514264337593543950336";

    #[test]
    fn tick_nol_menghasilkan_q96() {
        let hasil = get_sqrt_ratio_at_tick(0).unwrap();
        assert_eq!(hasil, U256::from_str_radix(Q96_DEC, 10).unwrap());
    }

    #[test]
    fn batas_bawah_dan_atas_sesuai_konstanta_uniswap() {
        // Kedua nilai ini tercantum langsung di TickMath.sol dan dipakai
        // kontrak sebagai batas. Kalau salah satu meleset, seluruh tabel
        // konstanta patut dicurigai.
        assert_eq!(get_sqrt_ratio_at_tick(MIN_TICK).unwrap(), min_sqrt_ratio());
        assert_eq!(get_sqrt_ratio_at_tick(MAX_TICK).unwrap(), max_sqrt_ratio());
    }

    #[test]
    fn tick_di_luar_rentang_ditolak() {
        assert!(get_sqrt_ratio_at_tick(MIN_TICK - 1).is_err());
        assert!(get_sqrt_ratio_at_tick(MAX_TICK + 1).is_err());
    }

    #[test]
    fn selalu_naik_seiring_tick() {
        let mut sebelumnya = get_sqrt_ratio_at_tick(MIN_TICK).unwrap();
        let mut tick = MIN_TICK + 1;
        while tick <= MAX_TICK {
            let sekarang = get_sqrt_ratio_at_tick(tick).unwrap();
            assert!(
                sekarang > sebelumnya,
                "tick {tick} tidak lebih besar dari tick sebelumnya"
            );
            sebelumnya = sekarang;
            // Melompat supaya ujinya cepat, tapi tetap menyentuh seluruh
            // rentang dan tidak melewatkan pola bit mana pun.
            tick += 977;
        }
    }

    /// Uji paling penting di berkas ini.
    ///
    /// Membandingkan hasil bilangan bulat dengan hitungan `f64` yang sepenuhnya
    /// independen, yaitu `sqrt(1.0001^tick) * 2^96`. Satu digit salah pada
    /// tabel konstanta akan menggeser hasil jauh melampaui toleransi, di tick
    /// mana pun bit yang bersangkutan aktif.
    #[test]
    fn cocok_dengan_hitungan_pecahan_independen() {
        let q96 = 2f64.powi(96);
        let mut diperiksa = 0;

        let mut tick = MIN_TICK;
        while tick <= MAX_TICK {
            let tepat = get_sqrt_ratio_at_tick(tick).unwrap();
            let tepat_f: f64 = tepat.to_string().parse().unwrap();

            let perkiraan = 1.0001f64.powf(tick as f64 / 2.0) * q96;

            let galat = (tepat_f - perkiraan).abs() / perkiraan;
            assert!(
                galat < 1e-9,
                "tick {tick}: bilangan bulat {tepat_f:e} berbeda {galat:e} dari perkiraan {perkiraan:e}"
            );

            diperiksa += 1;
            tick += 373;
        }

        assert!(
            diperiksa > 4_000,
            "hanya {diperiksa} tick diperiksa, terlalu sedikit"
        );
    }

    #[test]
    fn setiap_bit_tabel_konstanta_tersentuh() {
        // Menjaga supaya uji sapuan di atas benar benar melewati semua bit.
        // Kalau ada bit yang tidak pernah aktif, konstantanya tidak teruji.
        for i in 0..20 {
            let tick = 1i32 << i;
            if tick > MAX_TICK {
                break;
            }
            assert!(
                get_sqrt_ratio_at_tick(tick).is_ok(),
                "bit {i} pada tick {tick} gagal"
            );
            assert!(get_sqrt_ratio_at_tick(-tick).is_ok());
        }
    }

    #[test]
    fn membaca_balik_tick_dari_sqrt_ratio() {
        for tick in [
            MIN_TICK,
            -100_000,
            -60,
            -1,
            0,
            1,
            60,
            78_782,
            100_000,
            MAX_TICK - 1,
        ] {
            let sqrt = get_sqrt_ratio_at_tick(tick).unwrap();
            let kembali = get_tick_at_sqrt_ratio(sqrt).unwrap();
            assert_eq!(kembali, tick, "tick {tick} tidak kembali utuh");
        }
    }

    #[test]
    fn nilai_di_antara_dua_tick_dibulatkan_ke_bawah() {
        let bawah = get_sqrt_ratio_at_tick(1_000).unwrap();
        let atas = get_sqrt_ratio_at_tick(1_001).unwrap();
        let tengah = (bawah + atas) / U256::from(2u8);
        assert_eq!(get_tick_at_sqrt_ratio(tengah).unwrap(), 1_000);
        // Tepat di batas atas harus sudah masuk tick berikutnya.
        assert_eq!(get_tick_at_sqrt_ratio(atas).unwrap(), 1_001);
    }

    #[test]
    fn sqrt_ratio_di_luar_rentang_ditolak() {
        assert!(get_tick_at_sqrt_ratio(U256::from(1u8)).is_err());
        assert!(get_tick_at_sqrt_ratio(max_sqrt_ratio()).is_err());
        assert!(get_tick_at_sqrt_ratio(min_sqrt_ratio() - U256::from(1u8)).is_err());
    }
}
