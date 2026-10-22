//! Perhitungan fee yang belum diklaim.
//!
//! KENAPA TIDAK CUKUP MEMBACA `tokensOwed`
//! Field `tokensOwed0` dan `tokensOwed1` pada posisi V3 hanya diperbarui saat
//! posisinya disentuh, yaitu saat likuiditas berubah atau fee diklaim. Posisi
//! yang dibuka lalu didiamkan akan melaporkan nol selamanya, padahal fee-nya
//! terus bertambah. Menampilkan nol di kartu berarti user mengira posisinya
//! tidak menghasilkan apa apa.
//!
//! Yang benar adalah menghitung dari pertumbuhan fee di dalam range, yaitu
//! selisih antara pertumbuhan sekarang dan pertumbuhan saat terakhir dicatat,
//! dikalikan likuiditas.
//!
//! INVARIAN
//! - Pengurangan pertumbuhan fee WAJIB membungkus, bukan jenuh. Kontrak
//!   Uniswap memakai aritmetika `unchecked`, dan nilainya memang dirancang
//!   melampaui batas lalu berputar. Memakai `saturating_sub` di sini akan
//!   melaporkan fee nol persis pada posisi yang paling lama berjalan, yaitu
//!   yang fee-nya paling besar.
//! - Pembagi `2^128` adalah bagian dari format Q128.128 yang dipakai Uniswap.
//!   Melupakannya menghasilkan angka sebesar 3 x 10^38 kali lipat.

use alloy::primitives::U256;

/// Hitung fee yang belum diklaim untuk satu sisi.
///
/// `sekarang` dan `terakhir` adalah pertumbuhan fee per satuan likuiditas dalam
/// format Q128.128.
pub fn fee_belum_diklaim(sekarang: U256, terakhir: U256, liquidity: u128) -> U256 {
    if liquidity == 0 {
        return U256::ZERO;
    }
    // Wrapping, bukan saturating. Lihat catatan invarian di atas.
    let selisih = sekarang.wrapping_sub(terakhir);
    let hasil = selisih.wrapping_mul(U256::from(liquidity));
    hasil >> 128
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q128(x: u64) -> U256 {
        U256::from(x) << 128
    }

    #[test]
    fn pertumbuhan_satu_satuan_dikali_likuiditas() {
        // Pertumbuhan satu satuan penuh per likuiditas, likuiditas seribu.
        let hasil = fee_belum_diklaim(q128(1), U256::ZERO, 1_000);
        assert_eq!(hasil, U256::from(1_000u64));
    }

    #[test]
    fn tanpa_likuiditas_tidak_ada_fee() {
        assert_eq!(fee_belum_diklaim(q128(5), U256::ZERO, 0), U256::ZERO);
    }

    #[test]
    fn tidak_ada_pertumbuhan_berarti_nol() {
        assert_eq!(fee_belum_diklaim(q128(7), q128(7), 1_000), U256::ZERO);
    }

    #[test]
    fn pengurangan_membungkus_bukan_jenuh() {
        // INI KASUS YANG PALING MUDAH SALAH.
        //
        // Kontrak Uniswap memakai aritmetika unchecked, dan pertumbuhan fee
        // memang dirancang melampaui batas lalu berputar. Posisi yang paling
        // lama berjalan justru yang paling mungkin mengalaminya, dan itu posisi
        // yang fee-nya paling besar.
        //
        // Dengan saturating_sub, hasilnya akan nol dan user mengira posisinya
        // tidak menghasilkan apa apa.
        let terakhir = U256::MAX - q128(1) + U256::from(1u8);
        let sekarang = q128(2);
        let hasil = fee_belum_diklaim(sekarang, terakhir, 1_000);
        assert_eq!(
            hasil,
            U256::from(3_000u64),
            "selisih yang membungkus harus tetap terbaca sebagai tiga satuan"
        );
    }

    #[test]
    fn format_q128_dibagi_bukan_dibiarkan() {
        // Melupakan pembagian 2^128 menghasilkan angka sekitar 3 x 10^38 kali
        // lipat, yang akan tampil sebagai fee raksasa yang tidak masuk akal.
        let hasil = fee_belum_diklaim(q128(1), U256::ZERO, 1);
        assert_eq!(hasil, U256::from(1u8));
    }

    #[test]
    fn pertumbuhan_pecahan_dibulatkan_ke_bawah() {
        // Setengah satuan pertumbuhan dengan likuiditas satu menghasilkan
        // setengah, yang pada bilangan bulat menjadi nol. Itu benar: fee yang
        // belum genap satu satuan terkecil memang belum bisa diklaim.
        let setengah = U256::from(1u8) << 127;
        assert_eq!(fee_belum_diklaim(setengah, U256::ZERO, 1), U256::ZERO);
        // Dengan likuiditas dua, setengah satuan menjadi satu penuh.
        assert_eq!(fee_belum_diklaim(setengah, U256::ZERO, 2), U256::from(1u8));
    }
}
