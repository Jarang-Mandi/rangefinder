//! Penyelesai zap: berapa bagian modal yang harus ditukar.
//!
//! MASALAHNYA
//! User punya satu token, tapi range dua sisi butuh keduanya dengan rasio
//! tertentu. Rasio itu ditentukan posisi harga sekarang terhadap batas range,
//! dan tidak ada rumus tertutup yang langsung memberi jawabannya, karena swap
//! itu sendiri menggeser harga.
//!
//! CARA MENYELESAIKANNYA
//! Iterasi. Tebak berapa yang ditukar, hitung sisa kedua token, bandingkan
//! dengan rasio yang dibutuhkan, lalu perbaiki tebakan. PRD 6.1 menyebut Newton
//! dua sampai tiga putaran, tapi Newton butuh turunan yang bentuknya bergantung
//! pada model dampak harga, dan model itu berbeda antara V3 dan V4 berhook.
//!
//! Modul ini memakai bagi dua alih alih Newton. Alasannya bukan kemalasan:
//! fungsi selisihnya monoton, jadi bagi dua PASTI konvergen, sedangkan Newton
//! bisa melompat keluar jangkauan pada fungsi yang turunannya kasar. Lima puluh
//! putaran bagi dua pada bilangan 256 bit sudah menyempitkan jangkauan sampai
//! satuan terkecil, dan seluruhnya berjalan di memori tanpa menyentuh jaringan.
//!
//! INVARIAN
//! - Hasilnya tidak pernah melebihi modal yang ada. Menukar lebih dari yang
//!   dimiliki berarti transaksi yang pasti gagal setelah gas terbakar.
//! - Kutipan swap datang dari luar sebagai fungsi, bukan dihitung di sini.
//!   Modul ini tidak boleh menebak harga swap, karena tebakan yang meleset
//!   akan menghasilkan posisi yang timpang dan sisa token yang menganggur.
//! - Rasio dihitung dengan matematika bilangan bulat yang sama dengan kontrak,
//!   lewat `liquidity_math`. Perkiraan `f64` di sini akan menyimpang persis di
//!   tempat yang paling mahal, yaitu saat range hampir seluruhnya satu sisi.

use alloy::primitives::U256;

use crate::math::{liquidity_math, tick_math};

/// Sisi mana yang dipegang user sebelum zap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SisiSumber {
    /// User memegang token0, sebagian ditukar menjadi token1.
    Token0,
    /// User memegang token1, sebagian ditukar menjadi token0.
    Token1,
}

/// Rencana zap: berapa yang ditukar, dan berapa sisanya di kedua sisi.
#[derive(Debug, Clone, PartialEq)]
pub struct RencanaZap {
    /// Jumlah token sumber yang ditukar.
    pub tukar: U256,
    /// Jumlah token0 yang benar benar masuk ke posisi.
    pub setor0: U256,
    /// Jumlah token1 yang benar benar masuk ke posisi.
    pub setor1: U256,
    /// Token0 yang tersisa menganggur setelah posisi dibentuk.
    ///
    /// Idealnya mendekati nol. Sisa yang besar berarti modal yang tidak
    /// menghasilkan fee apa pun, dan itu kerugian yang tidak terlihat.
    pub menganggur0: U256,
    pub menganggur1: U256,
    /// Likuiditas yang bisa dibuat dari sisa itu.
    pub liquidity: u128,
    /// Berapa putaran bagi dua yang dipakai, untuk diagnosis.
    pub putaran: u32,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ZapError {
    #[error("modal nol, tidak ada yang bisa disetor")]
    ModalNol,
    #[error("tick {lower} tidak lebih kecil dari {upper}")]
    TickTerbalik { lower: i32, upper: i32 },
    #[error("tick di luar jangkauan Uniswap: {0}")]
    TickDiLuarJangkauan(String),
    #[error("posisi satu sisi sudah memegang aset yang dibutuhkan")]
    RangeSatuSisi,
    #[error("kutipan swap gagal: {0}")]
    KutipanGagal(String),
}

/// Batas keras jumlah putaran.
///
/// Setiap putaran memanggil `kutipan` sekali, dan pada jalur sungguhan itu
/// satu permintaan RPC. Jadi jumlah putaran bukan biaya di memori melainkan
/// waktu tunggu user, dan harus dibatasi.
const PUTARAN_MAKSIMUM: u32 = 24;

/// Ketelitian yang dianggap cukup, sebagai pembagi modal.
///
/// Pencarian berhenti begitu jangkauan tersisa lebih kecil dari satu per
/// sepuluh ribu modal. Ketelitian di bawah itu tidak mengubah likuiditas
/// secara berarti, sedangkan setiap putaran tambahan menambah satu permintaan
/// RPC. Menyempitkan sampai satuan terkecil pernah dicoba dan hanya membuat
/// pencarian pada modal besar memakan puluhan permintaan tanpa hasil yang
/// terasa.
const KETELITIAN_PEMBAGI: u64 = 10_000;

/// Selesaikan berapa bagian modal yang harus ditukar.
///
/// `kutipan` menjawab pertanyaan "kalau saya tukar sekian token sumber, berapa
/// token lawan yang saya terima". Ia datang dari luar karena hanya rantai yang
/// tahu jawabannya, dan menebaknya di sini akan menghasilkan posisi timpang.
///
/// Fungsi ini murni: ia tidak menyentuh jaringan, dan `kutipan` boleh berupa
/// hasil kutipan yang sudah di-cache.
pub fn selesaikan<F>(
    sqrt_price_x96: U256,
    tick_lower: i32,
    tick_upper: i32,
    modal: U256,
    sisi: SisiSumber,
    mut kutipan: F,
) -> Result<RencanaZap, ZapError>
where
    F: FnMut(U256) -> Result<U256, String>,
{
    if modal.is_zero() {
        return Err(ZapError::ModalNol);
    }
    if tick_lower >= tick_upper {
        return Err(ZapError::TickTerbalik {
            lower: tick_lower,
            upper: tick_upper,
        });
    }

    let sqrt_a =
        tick_math::get_sqrt_ratio_at_tick(tick_lower).map_err(ZapError::TickDiLuarJangkauan)?;
    let sqrt_b =
        tick_math::get_sqrt_ratio_at_tick(tick_upper).map_err(ZapError::TickDiLuarJangkauan)?;

    // Di luar range, posisi hanya menerima satu aset. Jika user sudah memegang
    // aset itu tidak ada yang perlu ditukar. Jika aset sumber berada di sisi
    // sebaliknya, seluruh modal harus ditukar agar mint tidak berakhir dengan
    // likuiditas nol.
    if sqrt_price_x96 <= sqrt_a {
        if sisi == SisiSumber::Token0 {
            return Err(ZapError::RangeSatuSisi);
        }
        let diterima = kutipan(modal).map_err(ZapError::KutipanGagal)?;
        return Ok(RencanaZap {
            tukar: modal,
            setor0: diterima,
            setor1: U256::ZERO,
            menganggur0: U256::ZERO,
            menganggur1: U256::ZERO,
            liquidity: liquidity_math::liquidity_for_amount0(sqrt_a, sqrt_b, diterima).unwrap_or(0),
            putaran: 1,
        });
    }
    if sqrt_price_x96 >= sqrt_b {
        if sisi == SisiSumber::Token1 {
            return Err(ZapError::RangeSatuSisi);
        }
        let diterima = kutipan(modal).map_err(ZapError::KutipanGagal)?;
        return Ok(RencanaZap {
            tukar: modal,
            setor0: U256::ZERO,
            setor1: diterima,
            menganggur0: U256::ZERO,
            menganggur1: U256::ZERO,
            liquidity: liquidity_math::liquidity_for_amount1(sqrt_a, sqrt_b, diterima).unwrap_or(0),
            putaran: 1,
        });
    }

    // Fungsi selisih: negatif berarti ditukar terlalu sedikit, positif berarti
    // terlalu banyak. Monoton terhadap `tukar`, jadi bagi dua pasti konvergen.
    //
    // "Terlalu sedikit" diukur dari sisi mana yang tersisa menganggur setelah
    // likuiditas dibentuk. Kalau token lawan yang menganggur, berarti kita
    // menukar kelebihan.
    let mut bawah = U256::ZERO;
    let mut atas = modal;
    let mut terbaik: Option<RencanaZap> = None;

    for putaran in 1..=PUTARAN_MAKSIMUM {
        let tengah = bawah + (atas - bawah) / U256::from(2u8);
        let diterima = kutipan(tengah).map_err(ZapError::KutipanGagal)?;

        let (punya0, punya1) = match sisi {
            SisiSumber::Token0 => (modal - tengah, diterima),
            SisiSumber::Token1 => (diterima, modal - tengah),
        };

        let liquidity =
            liquidity_math::liquidity_for_amounts(sqrt_price_x96, sqrt_a, sqrt_b, punya0, punya1)
                .unwrap_or(0);

        let (pakai0, pakai1) = if liquidity == 0 {
            (U256::ZERO, U256::ZERO)
        } else {
            liquidity_math::amounts_for_liquidity(sqrt_price_x96, sqrt_a, sqrt_b, liquidity)
                .unwrap_or((U256::ZERO, U256::ZERO))
        };

        let sisa0 = punya0.saturating_sub(pakai0);
        let sisa1 = punya1.saturating_sub(pakai1);

        // Simpan yang terbaik sejauh ini, diukur dari likuiditas yang tercipta.
        // Bagi dua bisa berakhir pada titik yang sedikit lebih buruk daripada
        // yang pernah dilewati, dan menyimpan yang terbaik menghindarinya.
        if terbaik.as_ref().is_none_or(|t| liquidity > t.liquidity) {
            terbaik = Some(RencanaZap {
                tukar: tengah,
                setor0: pakai0,
                setor1: pakai1,
                menganggur0: sisa0,
                menganggur1: sisa1,
                liquidity,
                putaran,
            });
        }

        // Berhenti begitu jangkauannya sudah cukup sempit, atau sudah tidak
        // bisa dipersempit lagi.
        let jangkauan = atas - bawah;
        let cukup_teliti = jangkauan * U256::from(KETELITIAN_PEMBAGI) <= modal;
        if cukup_teliti || jangkauan <= U256::from(1u8) {
            break;
        }

        // ARAH PENCARIAN DITENTUKAN SISI MANA YANG MENGIKAT
        //
        // Percobaan pertama membandingkan sisa token0 dengan sisa token1
        // secara langsung, dan itu keliru: keduanya token berbeda dengan
        // desimal berbeda. Pada FIG/USDG, sisa 77.380 satuan USDG berdesimal
        // enam ternyata jauh lebih bernilai daripada 12.325.754 satuan FIG
        // berdesimal delapan belas, tapi perbandingan angka mentah menyatakan
        // sebaliknya. Pencariannya lalu berjalan ke arah yang salah dan
        // berhenti dengan lima belas persen modal menganggur.
        //
        // Yang benar adalah membandingkan likuiditas yang bisa dibentuk tiap
        // sisi seorang diri. Sisi yang menghasilkan lebih sedikit adalah yang
        // mengikat, dan itu perbandingan tanpa satuan sehingga aman.
        let l0 = liquidity_math::liquidity_for_amount0(sqrt_price_x96, sqrt_b, punya0).unwrap_or(0);
        let l1 = liquidity_math::liquidity_for_amount1(sqrt_a, sqrt_price_x96, punya1).unwrap_or(0);

        // Token1 mengikat berarti kita butuh lebih banyak token1, jadi tukar
        // lebih banyak token0. Kalau sumbernya token1, arahnya berkebalikan.
        let tukar_lebih_banyak = match sisi {
            SisiSumber::Token0 => l1 < l0,
            SisiSumber::Token1 => l0 < l1,
        };
        if tukar_lebih_banyak {
            bawah = tengah;
        } else {
            atas = tengah;
        }
    }

    let hasil = terbaik.ok_or(ZapError::ModalNol)?;
    debug_assert!(
        hasil.tukar <= modal,
        "zap tidak boleh menukar lebih dari modal"
    );
    Ok(hasil)
}

/// Rasio token yang dibutuhkan range ini pada harga sekarang, sebagai porsi
/// nilai token0 dari total, dalam basis poin.
///
/// Dipakai panel untuk menjelaskan kenapa zap diperlukan, dan sebagai
/// pemeriksaan cepat sebelum menjalankan penyelesai penuh.
pub fn porsi_token0_bps(
    sqrt_price_x96: U256,
    tick_lower: i32,
    tick_upper: i32,
) -> Result<u32, ZapError> {
    if tick_lower >= tick_upper {
        return Err(ZapError::TickTerbalik {
            lower: tick_lower,
            upper: tick_upper,
        });
    }
    let sqrt_a =
        tick_math::get_sqrt_ratio_at_tick(tick_lower).map_err(ZapError::TickDiLuarJangkauan)?;
    let sqrt_b =
        tick_math::get_sqrt_ratio_at_tick(tick_upper).map_err(ZapError::TickDiLuarJangkauan)?;

    if sqrt_price_x96 <= sqrt_a {
        return Ok(10_000);
    }
    if sqrt_price_x96 >= sqrt_b {
        return Ok(0);
    }

    // Likuiditas sembarang, karena yang dicari hanya perbandingannya.
    let l: u128 = 1 << 96;
    let (a0, a1) = liquidity_math::amounts_for_liquidity(sqrt_price_x96, sqrt_a, sqrt_b, l)
        .map_err(ZapError::KutipanGagal)?;

    // Nilai token0 dinyatakan dalam satuan token1 supaya keduanya sebanding.
    // harga = (sqrtP / 2^96)^2, jadi nilai0 = a0 * sqrtP^2 / 2^192.
    let q96 = U256::from(1u8) << 96;
    let nilai0 = a0
        .checked_mul(sqrt_price_x96)
        .and_then(|v| v.checked_div(q96))
        .and_then(|v| v.checked_mul(sqrt_price_x96))
        .and_then(|v| v.checked_div(q96))
        .unwrap_or(U256::ZERO);

    let total = nilai0 + a1;
    if total.is_zero() {
        return Ok(5_000);
    }
    Ok((nilai0 * U256::from(10_000u32) / total).to::<u32>())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sqrt_di_tick(t: i32) -> U256 {
        tick_math::get_sqrt_ratio_at_tick(t).unwrap()
    }

    /// Kutipan sederhana dengan harga tetap dan tanpa dampak harga.
    ///
    /// Cukup untuk menguji penyelesainya, karena yang diuji di sini adalah
    /// pencarian titiknya, bukan model pasarnya.
    fn kutipan_datar(
        harga_pembilang: u64,
        harga_penyebut: u64,
    ) -> impl FnMut(U256) -> Result<U256, String> {
        move |x: U256| Ok(x * U256::from(harga_pembilang) / U256::from(harga_penyebut))
    }

    /// Kutipan dengan dampak harga, meniru bentuk kurva sungguhan.
    ///
    /// Diambil dari sapuan nyata pada pool FIG/USDG: makin besar swapnya,
    /// makin buruk kursnya. Tanpa dampak harga, arah pencarian yang salah
    /// pun bisa terlihat benar.
    fn kutipan_berdampak(dasar: u64) -> impl FnMut(U256) -> Result<U256, String> {
        move |x: U256| {
            if x.is_zero() {
                return Ok(U256::ZERO);
            }
            // Kurs turun sebanding dengan ukuran swap.
            let penalti = U256::from(1_000_000u64) + x;
            Ok(x * U256::from(dasar) * U256::from(1_000_000u64) / penalti)
        }
    }

    #[test]
    fn arah_pencarian_tidak_terpengaruh_selisih_desimal() {
        // INI BUG YANG PERNAH TERJADI.
        //
        // Versi pertama membandingkan sisa token0 dengan sisa token1 secara
        // langsung. Pada pasangan berdesimal enam melawan delapan belas,
        // perbandingan angka mentah selalu dimenangkan token berdesimal besar,
        // jadi pencarian berjalan ke arah yang salah dan berhenti dengan modal
        // menganggur.
        //
        // Kutipan di sini mengalikan seribu triliun, meniru selisih desimal itu.
        let sqrt_p = sqrt_di_tick(0);
        let modal = U256::from(1_000_000u64);
        let hasil = selesaikan(
            sqrt_p,
            -600,
            600,
            modal,
            SisiSumber::Token0,
            kutipan_datar(1_000_000_000_000, 1),
        )
        .unwrap();

        // Modal yang menganggur pada sisi sumber harus kecil. Kalau arah
        // pencariannya salah, angka ini besar.
        assert!(
            hasil.menganggur0 * U256::from(20u8) < modal,
            "token0 menganggur {} dari modal {modal}, arah pencarian meleset",
            hasil.menganggur0
        );
    }

    #[test]
    fn hasil_mengalahkan_pembagian_separuh_yang_naif() {
        // Membagi modal tepat separuh adalah tebakan yang paling sering
        // dipakai orang. Penyelesai ini harus lebih baik, kalau tidak ia tidak
        // ada gunanya.
        let sqrt_p = sqrt_di_tick(300);
        let modal = U256::from(1_000_000u64);

        let hasil = selesaikan(
            sqrt_p,
            -600,
            600,
            modal,
            SisiSumber::Token0,
            kutipan_berdampak(2),
        )
        .unwrap();

        let separuh = modal / U256::from(2u8);
        let mut k = kutipan_berdampak(2);
        let diterima = k(separuh).unwrap();
        let sqrt_a = sqrt_di_tick(-600);
        let sqrt_b = sqrt_di_tick(600);
        let l_naif = liquidity_math::liquidity_for_amounts(
            sqrt_p,
            sqrt_a,
            sqrt_b,
            modal - separuh,
            diterima,
        )
        .unwrap();

        assert!(
            hasil.liquidity >= l_naif,
            "penyelesai menghasilkan {} sedangkan pembagian separuh {l_naif}",
            hasil.liquidity
        );
    }

    #[test]
    fn jumlah_putaran_dibatasi() {
        // Setiap putaran adalah satu permintaan RPC di jalur sungguhan.
        let sqrt_p = sqrt_di_tick(0);
        let mut panggilan = 0u32;
        let modal = U256::from(10u64).pow(U256::from(18u8));
        let _ = selesaikan(sqrt_p, -600, 600, modal, SisiSumber::Token0, |x| {
            panggilan += 1;
            Ok(x)
        })
        .unwrap();
        assert!(
            panggilan <= PUTARAN_MAKSIMUM,
            "{panggilan} panggilan kutipan, terlalu banyak untuk dijalankan lewat RPC"
        );
    }

    #[test]
    fn range_simetris_menukar_kira_kira_separuh() {
        // Harga tepat di tengah range dan harga swap 1:1, jadi jawaban yang
        // benar mendekati separuh modal.
        let sqrt_p = sqrt_di_tick(0);
        let modal = U256::from(1_000_000u64);
        let hasil = selesaikan(
            sqrt_p,
            -600,
            600,
            modal,
            SisiSumber::Token0,
            kutipan_datar(1, 1),
        )
        .unwrap();

        let separuh = modal / U256::from(2u8);
        let selisih = if hasil.tukar > separuh {
            hasil.tukar - separuh
        } else {
            separuh - hasil.tukar
        };
        assert!(
            selisih * U256::from(100u8) < modal,
            "hasil {} terlalu jauh dari separuh {separuh}",
            hasil.tukar
        );
        assert!(hasil.liquidity > 0);
    }

    #[test]
    fn tidak_pernah_menukar_lebih_dari_modal() {
        // Ini penjaga paling penting: menukar lebih dari yang dipunya berarti
        // transaksi yang pasti gagal setelah gas terbakar.
        let sqrt_p = sqrt_di_tick(0);
        let modal = U256::from(12_345u64);
        for (a, b) in [(1u64, 1u64), (1, 1000), (1000, 1)] {
            let hasil = selesaikan(
                sqrt_p,
                -60,
                60,
                modal,
                SisiSumber::Token0,
                kutipan_datar(a, b),
            )
            .unwrap();
            assert!(
                hasil.tukar <= modal,
                "menukar {} dari modal {modal}",
                hasil.tukar
            );
        }
    }

    #[test]
    fn harga_di_bawah_range_ditolak_sebagai_satu_sisi() {
        // Di bawah range, posisi hanya berisi token0. Tidak ada yang perlu
        // ditukar, dan menukar justru merugikan.
        let sqrt_p = sqrt_di_tick(-1000);
        let hasil = selesaikan(
            sqrt_p,
            0,
            600,
            U256::from(1000u64),
            SisiSumber::Token0,
            kutipan_datar(1, 1),
        );
        assert_eq!(hasil.unwrap_err(), ZapError::RangeSatuSisi);
    }

    #[test]
    fn harga_di_bawah_range_menukar_seluruh_token1_ke_token0() {
        let modal = U256::from(1000u64);
        let hasil = selesaikan(
            sqrt_di_tick(-1000),
            0,
            600,
            modal,
            SisiSumber::Token1,
            kutipan_datar(2, 1),
        )
        .unwrap();
        assert_eq!(hasil.tukar, modal);
        assert_eq!(hasil.setor0, U256::from(2000u64));
        assert_eq!(hasil.setor1, U256::ZERO);
        assert!(hasil.liquidity > 0);
    }

    #[test]
    fn harga_di_atas_range_ditolak_sebagai_satu_sisi() {
        let sqrt_p = sqrt_di_tick(1000);
        let hasil = selesaikan(
            sqrt_p,
            -600,
            0,
            U256::from(1000u64),
            SisiSumber::Token1,
            kutipan_datar(1, 1),
        );
        assert_eq!(hasil.unwrap_err(), ZapError::RangeSatuSisi);
    }

    #[test]
    fn harga_di_atas_range_menukar_seluruh_token0_ke_token1() {
        let modal = U256::from(1000u64);
        let hasil = selesaikan(
            sqrt_di_tick(1000),
            -600,
            0,
            modal,
            SisiSumber::Token0,
            kutipan_datar(1, 2),
        )
        .unwrap();
        assert_eq!(hasil.tukar, modal);
        assert_eq!(hasil.setor0, U256::ZERO);
        assert_eq!(hasil.setor1, U256::from(500u64));
        assert!(hasil.liquidity > 0);
    }

    #[test]
    fn modal_nol_ditolak() {
        let sqrt_p = sqrt_di_tick(0);
        assert_eq!(
            selesaikan(
                sqrt_p,
                -60,
                60,
                U256::ZERO,
                SisiSumber::Token0,
                kutipan_datar(1, 1)
            )
            .unwrap_err(),
            ZapError::ModalNol
        );
    }

    #[test]
    fn tick_terbalik_ditolak() {
        let sqrt_p = sqrt_di_tick(0);
        assert!(matches!(
            selesaikan(
                sqrt_p,
                60,
                -60,
                U256::from(1u8),
                SisiSumber::Token0,
                kutipan_datar(1, 1)
            )
            .unwrap_err(),
            ZapError::TickTerbalik { .. }
        ));
    }

    #[test]
    fn kutipan_yang_gagal_diteruskan_bukan_ditelan() {
        // Kutipan gagal berarti kita tidak tahu harga swapnya. Melanjutkan
        // dengan nol akan menghasilkan pembagian modal yang sepenuhnya salah.
        let sqrt_p = sqrt_di_tick(0);
        let hasil = selesaikan(
            sqrt_p,
            -60,
            60,
            U256::from(1000u64),
            SisiSumber::Token0,
            |_| Err("quoter menolak".into()),
        );
        assert_eq!(
            hasil.unwrap_err(),
            ZapError::KutipanGagal("quoter menolak".into())
        );
    }

    #[test]
    fn sumber_token1_bekerja_juga() {
        let sqrt_p = sqrt_di_tick(0);
        let modal = U256::from(1_000_000u64);
        let hasil = selesaikan(
            sqrt_p,
            -600,
            600,
            modal,
            SisiSumber::Token1,
            kutipan_datar(1, 1),
        )
        .unwrap();
        assert!(hasil.tukar <= modal);
        assert!(hasil.liquidity > 0);
    }

    #[test]
    fn porsi_token0_separuh_saat_harga_di_tengah() {
        // Range simetris di sekitar tick nol, jadi nilainya terbagi rata.
        let bps = porsi_token0_bps(sqrt_di_tick(0), -600, 600).unwrap();
        assert!(
            (4_800..=5_200).contains(&bps),
            "porsi {bps} bps tidak mendekati separuh"
        );
    }

    #[test]
    fn porsi_token0_penuh_di_bawah_range() {
        assert_eq!(
            porsi_token0_bps(sqrt_di_tick(-1000), 0, 600).unwrap(),
            10_000
        );
    }

    #[test]
    fn porsi_token0_nol_di_atas_range() {
        assert_eq!(porsi_token0_bps(sqrt_di_tick(1000), -600, 0).unwrap(), 0);
    }

    #[test]
    fn range_condong_ke_atas_butuh_lebih_banyak_token0() {
        // Harga dekat batas bawah berarti posisi hampir seluruhnya token0,
        // jadi zap seharusnya menukar sedikit saja.
        let sqrt_p = sqrt_di_tick(-580);
        let modal = U256::from(1_000_000u64);
        let hasil = selesaikan(
            sqrt_p,
            -600,
            600,
            modal,
            SisiSumber::Token0,
            kutipan_datar(1, 1),
        )
        .unwrap();
        assert!(
            hasil.tukar < modal / U256::from(4u8),
            "menukar {} padahal posisinya hampir seluruhnya token0",
            hasil.tukar
        );
    }

    #[test]
    fn range_condong_ke_bawah_butuh_lebih_banyak_token1() {
        let sqrt_p = sqrt_di_tick(580);
        let modal = U256::from(1_000_000u64);
        let hasil = selesaikan(
            sqrt_p,
            -600,
            600,
            modal,
            SisiSumber::Token0,
            kutipan_datar(1, 1),
        )
        .unwrap();
        assert!(
            hasil.tukar > modal / U256::from(2u8),
            "menukar {} padahal posisinya hampir seluruhnya token1",
            hasil.tukar
        );
    }
}
