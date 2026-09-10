//! Baris daftar tab Signals: kandidat mentah yang sudah diberi arti.
//!
//! APA YANG DITAMBAHKAN DI SINI
//! `scan::pindai` menjawab pool mana yang ramai. Ia tidak menjawab pertanyaan
//! yang sebenarnya ingin dijawab user, yaitu apakah pool itu layak ditaruhi
//! modal. Untuk itu dibutuhkan simbol, market cap, kedalaman, umur, proyeksi
//! hasil, dan satu label yang tegas. Modul ini yang menyusunnya.
//!
//! SATUAN, DAN KENAPA IA SELALU KUTIPAN
//! Setiap angka uang di sini dinyatakan dalam token kutipan pool itu, bukan
//! dalam satuan terkecil dan bukan pula dalam dolar yang ditebak. Alasannya
//! sama dengan yang sudah dipakai di PnL: satuan terkecil menghasilkan angka
//! seperti 1392536216340532297728 yang terbaca seperti dolar padahal bukan,
//! dan kurs dolar yang ditebak memasukkan kesalahan ke angka yang seharusnya
//! murni hasil penjumlahan.
//!
//! Pool yang kedua sisinya bukan kutipan yang dikenal tidak diberi angka uang
//! sama sekali. Itu bukan kekurangan yang perlu ditambal, itu pengakuan bahwa
//! tidak ada satuan yang berarti untuk menyatakannya.
//!
//! INVARIAN
//! - Label `Hindari` hanya diberikan atas alasan yang bisa disebutkan, dan
//!   alasannya selalu ikut. Label tanpa alasan tidak mengajari user apa apa.
//! - Label berarah hanya diberikan kalau swapnya cukup banyak. Rasio efisiensi
//!   atas tiga swap adalah kebisingan, dan menyajikannya sebagai sinyal lebih
//!   buruk daripada tidak menyajikan apa apa.
//! - Arah harga dibalik kalau memecoin ada di sisi token1. Tick naik berarti
//!   token0 menguat, jadi pada pool yang memecoinnya token1, tick naik justru
//!   berarti memecoin melemah. Kekeliruan yang sama pernah terjadi pada batas
//!   range dan membuat separuh pool ditolak, jadi ia diuji di sini.
//! - Proyeksi fee APR selalu disertai jendelanya. Ia hasil ekstrapolasi jendela
//!   yang pendek, dan menyajikannya tanpa keterangan itu menyesatkan.

use std::collections::HashMap;

use alloy::primitives::{Address, U256};
use alloy::providers::Provider;
use anyhow::Result;
use serde::Serialize;

use super::scan::{HasilScan, Kandidat};
use super::{PoolInfo, PoolVersion};
use crate::chain::registry::Registry;
use crate::math::{liquidity_math, tick_math};

/// Berapa swap paling sedikit sebelum arah dianggap berarti.
///
/// Di bawah ini rasio efisiensi hanya kebisingan: dua swap selalu menghasilkan
/// rasio satu, karena tidak ada jalan lain dari titik pertama ke titik kedua.
pub const SWAP_MINIMUM_UNTUK_ARAH: usize = 20;

/// Batas rasio efisiensi untuk menyebut pasar menyamping.
///
/// Kaufman memakai 0,3 sebagai batas kasar antara pasar yang bergerak searah
/// dan pasar yang berputar putar. Angka yang sama dipakai detektor sideways,
/// dan menyamakannya di sini menjaga dua tempat itu tidak saling bertentangan.
pub const BATAS_MENYAMPING: f64 = 0.30;

/// Berapa tick setara satu persen gerakan harga.
///
/// 1,0001 pangkat 100 sama dengan 1,01005, jadi seratus tick kira kira satu
/// persen. Dipakai untuk mengukur kedalaman, bukan untuk menempatkan range,
/// sehingga selisih lima per sepuluh ribu itu tidak berpengaruh.
pub const TICK_PER_PERSEN: i32 = 100;

/// Perpindahan bersih paling kecil sebelum sebuah arah disebut, dalam persen.
///
/// KENAPA RASIO EFISIENSI SAJA TIDAK CUKUP
/// Rasio efisiensi mengukur kelurusan jalan, bukan panjangnya. Pool yang
/// hanya berpindah dua tick akan menghasilkan rasio 1,00 karena tidak ada
/// jalan lain dari titik pertama ke titik terakhir, dan hasil pemindaian
/// sungguhan pernah memberi label "LP Jual Naik" pada perpindahan 0,0 persen
/// karena alasan itu.
///
/// Bagi LP, yang menentukan bukan lurus atau tidaknya jalan, melainkan apakah
/// harga sampai keluar dari range. Perpindahan di bawah satu persen tidak
/// mengeluarkan range yang masuk akal mana pun, jadi ia menyamping apa pun
/// bentuk jalannya.
pub const BATAS_ARAH_PERSEN: f64 = 1.0;

/// Perpindahan yang sudah terlalu jauh untuk diikuti, dalam persen.
///
/// Setelah bergerak sebesar ini dalam satu jendela, range apa pun yang
/// dipasang sekarang akan langsung tertinggal. Hasil pemindaian sungguhan
/// pernah menyarankan "LP Jual Naik" pada perpindahan 1.888 persen, dan saran
/// itu hampir pasti merugikan: yang naik sebanyak itu dalam beberapa menit
/// biasanya turun kembali sebelum range sempat menghasilkan apa apa.
pub const BATAS_TERLALU_JAUH_PERSEN: f64 = 60.0;

/// Modal acuan untuk memproyeksikan hasil, dalam satuan kutipan.
///
/// KENAPA PERLU MODAL ACUAN
/// Bagian LP atas fee adalah modal dibagi (kedalaman ditambah modal), bukan
/// modal dibagi kedalaman. Penyederhanaan yang kedua hanya sah kalau modalnya
/// jauh lebih kecil daripada poolnya, dan pada pool tipis ia meledak: pool
/// dengan kedalaman 296 dan volume 35.500 menghasilkan proyeksi 1.890.130
/// persen, karena rumusnya diam diam mengandaikan kita menguasai seluruh pool
/// tanpa mengencerkannya sama sekali.
///
/// Dengan modal acuan yang disebutkan, angkanya berubah menjadi pernyataan
/// yang bisa diperiksa: sekian persen KALAU menaruh sekian.
pub const MODAL_ACUAN: f64 = 1_000.0;

/// Di atas ini, APR ditampilkan sebagai batas bawah, bukan sebagai angka pasti.
///
/// Proyeksi setahun dari jendela setengah jam mengalikan kesalahannya tujuh
/// belas ribu kali. Menuliskan tujuh angka penting pada hasil seperti itu
/// menyiratkan ketelitian yang tidak ada.
pub const APR_TAK_BERARTI: f64 = 1_000.0;

const DETIK_SETAHUN: f64 = 365.0 * 24.0 * 3_600.0;

/// Label aksi yang tegas untuk satu baris, PRD 3.4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Label {
    LpSideways,
    LpBeliTurun,
    LpJualNaik,
    Pantau,
    Hindari,
}

impl Label {
    /// Benar kalau baris ini boleh dibuka sebagai posisi.
    ///
    /// PRD 3.4 meminta baris `Hindari` tidak bisa diklik untuk LP. Pemeriksaan
    /// itu ada di sini supaya panel dan engine memakai jawaban yang sama.
    pub fn boleh_dibuka(self) -> bool {
        self != Label::Hindari
    }
}

/// Satu baris daftar kandidat, siap ditampilkan.
#[derive(Debug, Clone, Serialize)]
pub struct BarisScan {
    pub pool_id: String,
    /// Alamat memecoin, sisi yang bukan kutipan.
    pub token: String,
    pub symbol: String,
    pub quote: String,
    pub quote_symbol: String,
    /// Benar untuk ETH native atau alamat kutipan yang ada di registry.
    ///
    /// KENAPA INI HARUS DIBAWA SAMPAI KE TAMPILAN
    /// Simbol ditentukan penerbit token, jadi siapa pun bisa menerbitkan token
    /// bersimbol USDG. Pemindaian sungguhan memang menemukannya. Yang
    /// membedakan asli dari tiruan hanya alamatnya, dan kalau tampilan hanya
    /// menunjukkan simbol, keduanya terlihat persis sama.
    ///
    /// Masalah yang sama pernah muncul pada FIG, yang punya pool terhadap 32
    /// token berbeda dan sebagian besar tiruan.
    pub quote_dikenal: bool,

    pub fee: u32,
    pub dynamic_fee: bool,
    /// Alamat hook, kosong kalau tidak ada.
    pub hooks: Option<String>,
    pub tick: i32,
    pub tick_spacing: i32,
    pub liquidity: String,

    /// Harga memecoin dalam satuan kutipan, desimal kedua token sudah dihitung.
    pub harga: Option<f64>,
    /// Market cap dalam satuan kutipan. `None` kalau pasokan tidak terbaca.
    pub mcap: Option<f64>,
    /// Volume pada jendela, dalam satuan kutipan.
    pub volume: Option<f64>,
    pub swap: usize,
    /// Kutipan yang dibutuhkan untuk menggeser harga satu persen.
    ///
    /// Ini pengganti "likuiditas" yang bisa dibaca manusia. Angka likuiditas
    /// mentah tidak punya satuan yang berarti, sedangkan angka ini menjawab
    /// pertanyaan yang benar benar ditanyakan: seberapa besar pool ini.
    pub kedalaman: Option<f64>,
    /// Umur pool dalam jam, dihitung dari blok pembuatan.
    pub umur_jam: f64,
    /// Hasil fee sepanjang jendela pengukuran terhadap modal acuan, persen.
    ///
    /// Inilah angka yang benar benar terukur. APR di bawah hanyalah angka ini
    /// yang direntangkan menjadi setahun.
    pub hasil_jendela: Option<f64>,
    /// Proyeksi fee setahun terhadap modal acuan, dalam persen.
    ///
    /// Ekstrapolasi jendela yang pendek, jadi ia selalu ditampilkan bersama
    /// jendelanya. `None` untuk fee dinamis, yang besarannya baru diketahui
    /// saat swap terjadi.
    pub fee_apr: Option<f64>,
    /// Modal acuan yang dipakai menghitung dua angka di atas.
    ///
    /// Ikut dibawa supaya angkanya tidak pernah terbaca sebagai sifat pool.
    /// Ia sifat pool DAN modal sekaligus, dan berubah kalau modalnya berubah.
    pub modal_acuan: f64,

    /// Rasio efisiensi Kaufman atas deret tick pada jendela.
    pub efisiensi: Option<f64>,
    /// Lebar putaran harga pada jendela, dalam persen.
    pub lebar_persen: Option<f64>,
    /// Perpindahan harga bersih pada jendela, dalam persen, dari sudut pandang
    /// memecoin. Positif berarti memecoin menguat.
    pub perpindahan_persen: Option<f64>,

    pub risiko: Vec<String>,
    pub label: Label,
    pub alasan: String,
}

/// Daftar kandidat beserta jendela yang dipakai mengukurnya.
///
/// Jendela ikut hasilnya, tidak dihitung ulang di tempat lain. Volume dan
/// jendelanya pernah datang dari dua sumber yang berbeda, dan akibatnya
/// proyeksi APR meleset tiga puluh kali lipat tanpa satu pun error muncul.
#[derive(Debug, Clone, Serialize)]
pub struct DaftarKandidat {
    pub baris: Vec<BarisScan>,
    /// Lamanya jendela pengukuran volume, dalam detik.
    pub jendela_volume_detik: f64,
    /// Benar kalau volume terpaksa diukur pada jendela pemindaian yang pendek.
    ///
    /// Kalau benar, proyeksi APR jauh lebih kasar dan panel harus mengatakannya.
    pub volume_jendela_pendek: bool,
}

/// Satuan sebuah pasangan token, dipakai mengubah angka mentah menjadi kutipan.
///
/// Ketiganya selalu bepergian bersama, dan memisahkannya menjadi tiga parameter
/// membuat urutan argumen mudah tertukar. Dua di antaranya bertipe sama dan
/// nilainya berdekatan, sehingga tertukarnya tidak akan memunculkan error apa
/// pun, hanya angka yang salah.
#[derive(Debug, Clone, Copy)]
pub struct Satuan {
    /// Harga memecoin dalam kutipan, desimal kedua sisi sudah dihitung.
    pub harga: f64,
    /// Sepuluh pangkat desimal memecoin.
    pub skala_meme: f64,
    /// Sepuluh pangkat desimal kutipan.
    pub skala_quote: f64,
}

/// Umur pool dalam jam.
///
/// Umur adalah salah satu penyaring paling menentukan di playbook: pool yang
/// baru beberapa menit belum punya sejarah yang bisa dibaca, dan pool yang
/// sudah lama biasanya sudah kehilangan volumenya. Blok nol berarti event
/// `Initialize` tidak ditemukan, dan itu dilaporkan sebagai nol jam, bukan
/// sebagai umur raksasa dari blok nol.
pub fn umur_jam(blok_dibuat: u64, sampai_blok: u64, block_time_ms: u64) -> f64 {
    if blok_dibuat == 0 || sampai_blok <= blok_dibuat {
        return 0.0;
    }
    (sampai_blok - blok_dibuat) as f64 * block_time_ms as f64 / 1_000.0 / 3_600.0
}

/// Ubah tick menjadi persentase perubahan harga.
///
/// Dipisahkan supaya bisa diuji: kekeliruan tanda di sini akan membalik seluruh
/// label berarah tanpa memunculkan error apa pun.
pub fn tick_ke_persen(delta_tick: i32) -> f64 {
    (1.0001_f64.powi(delta_tick) - 1.0) * 100.0
}

/// Tentukan label dan alasannya.
///
/// Murni, tanpa jaringan, supaya seluruh cabang keputusannya bisa diuji. Inilah
/// bagian yang paling menentukan bagi user, dan bagian yang paling mudah salah
/// tanpa disadari.
pub fn tentukan_label(
    swap: usize,
    efisiensi: Option<f64>,
    perpindahan_persen: Option<f64>,
    kutipan_dikenal: bool,
    risiko: &[String],
) -> (Label, String) {
    if !risiko.is_empty() {
        return (
            Label::Hindari,
            format!("Ditolak karena {}.", risiko.join(", ")),
        );
    }

    if !kutipan_dikenal {
        // Tanpa kutipan yang dikenal tidak ada satuan untuk menilai hasilnya,
        // dan tanpa itu label apa pun hanya menebak. Barisnya tetap muncul,
        // karena user berhak tahu apa yang sedang ramai.
        return (
            Label::Pantau,
            "Tidak ada sisi kutipan yang dikenal pada pool ini, jadi hasilnya \
             tidak bisa dinyatakan dalam satuan apa pun. Ia tetap ditampilkan \
             karena ramai, tapi bukan sasaran alat ini."
                .to_string(),
        );
    }

    if swap < SWAP_MINIMUM_UNTUK_ARAH {
        return (
            Label::Pantau,
            format!(
                "Baru {swap} swap pada jendela ini, terlalu sedikit untuk \
                 menyimpulkan arah. Buka halaman tokennya kalau ingin dinilai \
                 dari candle."
            ),
        );
    }

    let Some(er) = efisiensi else {
        return (
            Label::Pantau,
            "Harga tidak bergerak sama sekali pada jendela ini. Ramai tapi \
             tanpa gerakan biasanya berarti perdagangan bolak balik di satu \
             tick, dan itu tidak membayar fee sebanyak yang terlihat."
                .to_string(),
        );
    };

    let pindah = perpindahan_persen.unwrap_or(0.0);
    if pindah.abs() < BATAS_ARAH_PERSEN {
        return (
            Label::LpSideways,
            format!(
                "Perpindahan bersih hanya {pindah:.2}% sepanjang \
                 jendela, di bawah {BATAS_ARAH_PERSEN:.1}%. Harga belum \
                 ke mana mana, jadi range dua sisi yang masuk akal tidak akan \
                 tersentuh."
            ),
        );
    }

    if er < BATAS_MENYAMPING {
        return (
            Label::LpSideways,
            format!(
                "Rasio efisiensi {er:.2}, di bawah {BATAS_MENYAMPING:.2}. Harga \
                 banyak bergerak tapi kembali ke tempatnya, yaitu keadaan yang \
                 paling menguntungkan untuk LP dua sisi."
            ),
        );
    }

    if pindah.abs() > BATAS_TERLALU_JAUH_PERSEN {
        return (
            Label::Pantau,
            format!(
                "Perpindahan {pindah:.0}% dalam satu jendela. Range yang \
                 dipasang sekarang akan langsung tertinggal, dan gerakan \
                 sebesar ini biasanya berbalik sebelum fee sempat terkumpul."
            ),
        );
    }

    if pindah < 0.0 {
        (
            Label::LpBeliTurun,
            format!(
                "Rasio efisiensi {er:.2} dengan perpindahan {pindah:.1}%. \
                 Harga bergerak searah ke bawah, jadi range ditaruh di bawah \
                 harga supaya memecoin terkumpul saat turun."
            ),
        )
    } else {
        (
            Label::LpJualNaik,
            format!(
                "Rasio efisiensi {er:.2} dengan perpindahan +{pindah:.1}%. \
                 Harga bergerak searah ke atas, jadi range ditaruh di atas harga \
                 supaya memecoin terjual saat naik."
            ),
        )
    }
}

/// Kumpulkan alasan menolak sebuah pool.
///
/// Terpisah dari `tentukan_label` supaya daftarnya bisa diuji sendiri, dan
/// supaya panel bisa menampilkan seluruh alasan, bukan hanya yang pertama.
pub fn risiko_pool(k: &Kandidat, likuiditas: u128, allowlist_hook: &[String]) -> Vec<String> {
    let mut out = Vec::new();

    if likuiditas == 0 {
        out.push("likuiditasnya nol, tidak ada yang bisa dibagi fee".to_string());
    }

    if k.hooks != super::ZERO_ADDRESS
        && !allowlist_hook
            .iter()
            .any(|h| h.eq_ignore_ascii_case(&k.hooks))
    {
        // Hook bisa menolak penarikan. Itu bukan risiko harga yang bisa
        // ditanggung, itu kehilangan modal seluruhnya, jadi ia menghalangi
        // pembukaan posisi alih alih sekadar diberi tanda.
        out.push(format!(
            "hook {} belum ada di allowlist dan bisa menghalangi penarikan",
            k.hooks
        ));
    }

    out
}

/// Kedalaman satu persen, dalam satuan terkecil token kutipan.
///
/// Menjawab berapa banyak kutipan yang dibutuhkan untuk menggeser harga satu
/// persen. Ini ukuran besar pool yang bisa dibaca manusia, tidak seperti angka
/// likuiditas mentah yang tidak punya satuan.
pub fn kedalaman_1_persen(
    tick: i32,
    likuiditas: u128,
    quote_adalah_token1: bool,
    satuan: Satuan,
) -> Option<f64> {
    kedalaman_rentang(
        tick,
        tick - TICK_PER_PERSEN,
        tick + TICK_PER_PERSEN,
        likuiditas,
        quote_adalah_token1,
        satuan,
    )
}

/// Kutipan yang tertanam pada rentang tick tertentu, di sisi yang mengikat.
///
/// KENAPA RENTANG YANG DILEWATI, BUKAN PITA TETAP
/// Pita satu persen adalah ukuran yang seragam, dan keseragaman itu justru
/// yang membuatnya menyesatkan. Pool yang harganya berayun sepuluh persen
/// membutuhkan range sepuluh persen kalau ingin tetap aktif, dan modal untuk
/// range selebar itu jauh lebih besar daripada untuk pita satu persen.
/// Mengukur modal pada pita yang lebih sempit daripada yang sesungguhnya
/// dibutuhkan membuat proyeksi hasilnya terlihat jauh lebih besar daripada
/// yang bisa dicapai.
pub fn kedalaman_rentang(
    tick: i32,
    bawah: i32,
    atas: i32,
    likuiditas: u128,
    quote_adalah_token1: bool,
    satuan: Satuan,
) -> Option<f64> {
    if likuiditas == 0 || atas <= bawah || !satuan.harga.is_finite() {
        return None;
    }
    // Batas dijaga tetap mengapit harga sekarang. Rentang yang seluruhnya di
    // satu sisi harga menghasilkan posisi satu sisi, dan modalnya tidak bisa
    // dibandingkan dengan posisi dua sisi.
    let bawah = bawah.min(tick);
    let atas = atas.max(tick);

    let sekarang = tick_math::get_sqrt_ratio_at_tick(tick).ok()?;
    let sqrt_bawah = tick_math::get_sqrt_ratio_at_tick(bawah).ok()?;
    let sqrt_atas = tick_math::get_sqrt_ratio_at_tick(atas).ok()?;

    // KEDUA KAKI DIHITUNG, BUKAN SATU
    //
    // Posisi yang mengapit harga selalu memuat dua token: token0 untuk bagian
    // di atas harga, token1 untuk bagian di bawahnya. Menghitung satu kaki
    // saja membuat modalnya terlihat nol setiap kali harga kebetulan berada
    // di ujung rentang yang dilewatinya, karena kaki di sisi itu memang
    // menyusut sampai nol.
    //
    // Pemindaian sungguhan menunjukkannya: HOUND dengan modal 0,0000 dan
    // hasil 142 persen, yang berarti pembaginya hampir nol, bukan bahwa
    // poolnya gratis.
    let amount0 = liquidity_math::amount0_for_liquidity(sekarang, sqrt_atas, likuiditas).ok()?;
    let amount1 = liquidity_math::amount1_for_liquidity(sqrt_bawah, sekarang, likuiditas).ok()?;

    let (mentah_quote, mentah_meme) = if quote_adalah_token1 {
        (amount1, amount0)
    } else {
        (amount0, amount1)
    };

    let sisi_quote = f64::from(mentah_quote) / satuan.skala_quote;
    let sisi_meme = f64::from(mentah_meme) / satuan.skala_meme * satuan.harga;
    let total = sisi_quote + sisi_meme;
    total.is_finite().then_some(total)
}

/// Hasil fee sepanjang jendela terhadap modal acuan, dalam persen.
///
/// CARA MENGHITUNGNYA
/// Modal acuan yang ditaruh pada rentang yang harga lewati mendapat bagian
/// `modal / (kedalaman + modal)` dari likuiditas aktif di sana. Fee yang
/// diterima sepanjang jendela adalah `volume * fee * bagian`, dan hasilnya
/// dinyatakan terhadap modal acuan itu sendiri.
///
/// Penyebutnya memuat modal kita sendiri, dan itu bukan kerapian belaka.
/// Tanpa suku itu, pool yang kedalamannya mendekati nol menghasilkan proyeksi
/// yang mendekati tak hingga, padahal kenyataannya modal kita akan menjadi
/// hampir seluruh pool dan bagiannya tidak pernah melewati satu.
///
/// APA YANG DIANDAIKAN
/// Bahwa volume berlanjut seperti pada jendela, dan bahwa harga tetap berada
/// di dalam rentang yang sama. Keduanya sering tidak terjadi, dan itulah
/// kenapa angka ini disebut proyeksi, bukan hasil.
pub fn hasil_per_jendela(volume_quote: f64, fee_ppm: u32, kedalaman_quote: f64) -> Option<f64> {
    if kedalaman_quote < 0.0 || jendela_tak_masuk_akal(volume_quote) {
        return None;
    }
    let fee = fee_ppm as f64 / 1_000_000.0;
    // Bagian kita atas likuiditas aktif setelah modal kita ikut menambahnya.
    // Inilah yang membedakannya dari pembagian lurus: pada pool tipis, modal
    // acuan mengencerkan poolnya sendiri dan bagiannya tidak pernah melewati
    // satu, berapa pun tipisnya pool itu.
    let bagian = MODAL_ACUAN / (kedalaman_quote + MODAL_ACUAN);
    Some(volume_quote * fee * bagian / MODAL_ACUAN * 100.0)
}

fn jendela_tak_masuk_akal(volume_quote: f64) -> bool {
    !volume_quote.is_finite() || volume_quote < 0.0
}

/// Hasil per jendela yang direntangkan menjadi setahun, dalam persen.
///
/// Selalu sajikan bersama jendelanya. Jendela setengah jam mengalikan apa pun
/// tujuh belas ribu kali, termasuk kesalahannya, sehingga angka di atas
/// `APR_TAK_BERARTI` sebaiknya ditampilkan sebagai batas bawah saja.
pub fn proyeksi_apr(
    volume_quote: f64,
    fee_ppm: u32,
    kedalaman_quote: f64,
    jendela_detik: f64,
) -> Option<f64> {
    if jendela_detik <= 0.0 {
        return None;
    }
    let per_jendela = hasil_per_jendela(volume_quote, fee_ppm, kedalaman_quote)?;
    Some(per_jendela * (DETIK_SETAHUN / jendela_detik))
}

/// Lengkapi hasil pemindaian menjadi baris siap tampil.
///
/// Dua batch: satu untuk metadata token, satu untuk keadaan pool. Pool yang
/// keadaannya tidak terbaca dilewati, tidak dianggap kosong, karena likuiditas
/// nol dan likuiditas tidak terbaca adalah dua kesimpulan yang sangat berbeda.
pub async fn perkaya<P: Provider>(
    provider: &P,
    registry: &Registry,
    hasil: &HasilScan,
    allowlist_hook: &[String],
) -> Result<DaftarKandidat> {
    if hasil.kandidat.is_empty() {
        return Ok(DaftarKandidat {
            baris: Vec::new(),
            jendela_volume_detik: 0.0,
            volume_jendela_pendek: false,
        });
    }

    // VOLUME DIUKUR ULANG PADA JENDELA YANG LEBIH PANJANG
    //
    // Jendela pemindaian sengaja pendek, sekitar dua menit, karena ia harus
    // membaca setiap swap di rantai untuk menemukan pool mana yang ramai.
    // Jendela sependek itu tidak boleh dipakai memproyeksikan hasil setahun:
    // pengalinya sekitar tiga ratus ribu, sehingga satu ledakan volume dua
    // menit menghasilkan APR jutaan persen. Hasil pemindaian sungguhan pernah
    // menampilkan 7.645.797 persen karena hal itu.
    //
    // Pada tahap ini poolnya sudah diketahui, jadi lognya bisa disaring menurut
    // poolId dan jendela satu jam menjadi murah: satu permintaan untuk seluruh
    // kandidat sekaligus.
    let ids: Vec<String> = hasil.kandidat.iter().map(|k| k.pool_id.clone()).collect();
    let panjang = match super::volume::ukur_volume(provider, registry, &ids).await {
        Ok(v) => Some(v),
        Err(e) => {
            // Kegagalan di sini TIDAK boleh diam diam menjadi jendela pendek.
            // Angka yang dihasilkannya masih terlihat masuk akal, hanya saja
            // tiga puluh kali lipat terlalu besar, dan itu jenis kekeliruan
            // yang paling sulit ditemukan belakangan.
            tracing::warn!(
                error = %e,
                "pengukuran volume satu jam gagal, proyeksi memakai jendela pemindaian"
            );
            None
        }
    };

    let jendela_detik = match &panjang {
        Some(v) => v.jendela_detik(registry.chain.block_time_ms),
        None => {
            hasil.sampai_blok.saturating_sub(hasil.dari_blok) as f64
                * registry.chain.block_time_ms as f64
                / 1_000.0
        }
    };
    let volume_jendela_pendek = panjang.is_none();

    // Sisi memecoin dulu, karena baris tanpa sisi memecoin tetap ditampilkan
    // (user berhak tahu apa yang ramai) tapi tidak diberi angka uang.
    let mut alamat: Vec<Address> = Vec::new();
    for k in &hasil.kandidat {
        for a in [&k.token0, &k.token1] {
            if let Ok(p) = a.parse::<Address>() {
                if !alamat.contains(&p) {
                    alamat.push(p);
                }
            }
        }
    }

    let meta = super::token_meta_batch(provider, registry, &alamat)
        .await
        .unwrap_or_default();
    let mut peta_meta: HashMap<String, super::TokenMeta> = HashMap::new();
    for (i, m) in meta.into_iter().enumerate() {
        if let (Some(m), Some(a)) = (m, alamat.get(i)) {
            peta_meta.insert(a.to_string().to_lowercase(), m);
        }
    }

    // Keadaan pool dibaca ulang, tidak diambil dari log. Tick pada log terakhir
    // sudah benar saat swap itu, tapi blok bisa sudah bertambah sejak itu, dan
    // angka basi pada alat yang mengirim transaksi lebih berbahaya daripada
    // satu permintaan tambahan.
    let keadaan = baca_keadaan(provider, registry, &hasil.kandidat).await?;

    let ctx = Konteks {
        registry,
        meta: &peta_meta,
        jendela_detik,
        sampai_blok: hasil.sampai_blok,
        allowlist_hook,
    };

    let mut out = Vec::with_capacity(hasil.kandidat.len());
    for k in &hasil.kandidat {
        let Some(p) = keadaan.get(&k.pool_id.to_lowercase()) else {
            continue;
        };
        // Volume jendela panjang kalau terbaca, jendela pemindaian kalau tidak.
        // Kembali ke jendela pendek lebih baik daripada tidak memberi angka
        // sama sekali, asalkan jendelanya yang dipakai ikut disebutkan.
        let kunci = k.pool_id.to_lowercase();
        let vol = panjang.as_ref().map(|v| {
            (
                v.per_pool.get(&kunci).copied().unwrap_or(0),
                v.per_pool1.get(&kunci).copied().unwrap_or(0),
            )
        });

        out.push(susun_baris(k, p, vol, &ctx));
    }

    // Diurutkan menurut proyeksi, bukan menurut keramaian. Yang dicari user
    // adalah hasil per modal, dan pool paling ramai belum tentu yang membayar
    // paling banyak untuk setiap dolar yang ditaruh di sana.
    out.sort_by(|a, b| {
        b.fee_apr
            .unwrap_or(-1.0)
            .partial_cmp(&a.fee_apr.unwrap_or(-1.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(DaftarKandidat {
        baris: out,
        jendela_volume_detik: jendela_detik,
        volume_jendela_pendek,
    })
}

/// Baca slot0 dan likuiditas seluruh kandidat dalam satu batch.
async fn baca_keadaan<P: Provider>(
    provider: &P,
    registry: &Registry,
    kandidat: &[Kandidat],
) -> Result<HashMap<String, PoolInfo>> {
    use alloy::primitives::FixedBytes;

    let state_view = registry.contract("uniswap_v4", "state_view")?;
    let multicall = registry.contract("common", "multicall3")?;

    let mut calls = Vec::with_capacity(kandidat.len() * 2);
    let mut terpakai: Vec<&Kandidat> = Vec::with_capacity(kandidat.len());
    for k in kandidat {
        let Ok(id) = k.pool_id.parse::<FixedBytes<32>>() else {
            continue;
        };
        calls.push(crate::chain::multicall::encode(
            state_view,
            &super::abi::IStateView::getSlot0Call { poolId: id },
        ));
        calls.push(crate::chain::multicall::encode(
            state_view,
            &super::abi::IStateView::getLiquidityCall { poolId: id },
        ));
        terpakai.push(k);
    }

    let hasil = crate::chain::multicall::batch_call(provider, multicall, calls).await?;

    let mut out = HashMap::new();
    for (n, k) in terpakai.into_iter().enumerate() {
        let dasar = n * 2;
        let Some(slot0) = crate::chain::multicall::decode::<super::abi::IStateView::getSlot0Call>(
            hasil.get(dasar).and_then(|x| x.as_ref()),
        ) else {
            continue;
        };
        let Some(likuiditas) = crate::chain::multicall::decode::<
            super::abi::IStateView::getLiquidityCall,
        >(hasil.get(dasar + 1).and_then(|x| x.as_ref())) else {
            continue;
        };
        let sqrt = U256::from(slot0.sqrtPriceX96);
        out.insert(
            k.pool_id.to_lowercase(),
            PoolInfo {
                version: PoolVersion::V4,
                id: k.pool_id.clone(),
                token0: k.token0.clone(),
                token1: k.token1.clone(),
                fee: k.fee,
                dynamic_fee: k.dynamic_fee,
                tick_spacing: k.tick_spacing,
                hooks: Some(k.hooks.clone()),
                sqrt_price_x96: sqrt.to_string(),
                tick: slot0.tick.as_i32(),
                liquidity: likuiditas.to_string(),
                raw_price: super::raw_price_from_sqrt_x96(sqrt),
                stats: None,
            },
        );
    }

    Ok(out)
}

/// Susun satu baris dari kandidat, keadaan pool, dan metadata token.
/// Segala sesuatu yang berlaku sama untuk seluruh baris dalam satu pemindaian.
///
/// Dikelompokkan supaya tanda tangan `susun_baris` tidak menjadi deretan
/// argumen yang mudah tertukar, dan supaya jelas mana yang milik satu baris
/// dan mana yang milik pemindaiannya.
struct Konteks<'a> {
    registry: &'a Registry,
    meta: &'a HashMap<String, super::TokenMeta>,
    jendela_detik: f64,
    sampai_blok: u64,
    allowlist_hook: &'a [String],
}

fn susun_baris(
    k: &Kandidat,
    p: &PoolInfo,
    volume_panjang: Option<(u128, u128)>,
    ctx: &Konteks,
) -> BarisScan {
    let registry = ctx.registry;
    let meta = ctx.meta;
    let jendela_detik = ctx.jendela_detik;
    let sampai_blok = ctx.sampai_blok;
    let allowlist_hook = ctx.allowlist_hook;

    let likuiditas: u128 = p.liquidity.parse().unwrap_or(0);
    let memecoin = super::scan::sisi_memecoin(k, registry);

    // Sisi mana yang kutipan menentukan seluruh satuan di bawah ini. Kalau
    // memecoin ada di token0 maka kutipan ada di token1, dan sebaliknya.
    let quote_adalah_token1 = memecoin
        .as_ref()
        .map(|m| m.eq_ignore_ascii_case(&k.token0))
        .unwrap_or(true);

    let alamat_quote = if quote_adalah_token1 {
        k.token1.clone()
    } else {
        k.token0.clone()
    };
    let alamat_meme = memecoin.clone().unwrap_or_else(|| k.token0.clone());

    let m_meme = meta.get(&alamat_meme.to_lowercase());
    let m_quote = meta.get(&alamat_quote.to_lowercase());

    // Kutipan dikenal lewat alamat native atau ALAMAT di registry. Simbol
    // tidak pernah cukup: ia ditentukan penerbit token, bukan oleh siapa pun
    // yang bisa dipercaya.
    let quote_dikenal = memecoin.is_some() && super::scan::kutipan_dikenal(&alamat_quote, registry);

    let simbol =
        |m: Option<&super::TokenMeta>| m.map(|x| x.symbol.clone()).unwrap_or_else(|| "?".into());

    // ARAH DIBALIK KALAU MEMECOIN ADA DI TOKEN1
    // Tick naik berarti token0 menguat terhadap token1. Pada pool yang
    // memecoinnya token1, tick naik justru berarti memecoin melemah.
    let arah = if quote_adalah_token1 { 1 } else { -1 };
    let perpindahan_persen = (memecoin.is_some() && k.swap > 0)
        .then(|| tick_ke_persen(arah * (k.tick_akhir - k.tick_awal)));
    let lebar_persen =
        (memecoin.is_some() && k.swap > 0).then(|| tick_ke_persen(k.tick_maks - k.tick_min).abs());

    // Angka uang hanya disusun kalau ada kutipan yang dikenal. Tanpa itu tidak
    // ada satuan yang berarti, dan menuliskan angka tanpa satuan menyesatkan.
    let (harga, mcap, volume_quote, kedalaman_quote) = match (quote_dikenal, m_meme, m_quote) {
        (true, Some(mm), Some(mq)) => {
            let skala_meme = 10f64.powi(mm.decimals as i32);
            let skala_quote = 10f64.powi(mq.decimals as i32);

            // raw_price adalah token1 per token0 dalam satuan terkecil. Harga
            // memecoin dalam kutipan butuh koreksi desimal kedua sisi.
            let harga = if quote_adalah_token1 {
                p.raw_price * skala_meme / skala_quote
            } else {
                (1.0 / p.raw_price) * skala_meme / skala_quote
            };

            let (v0, v1) = match volume_panjang {
                Some(v) => v,
                None => (
                    k.volume.parse().unwrap_or(0),
                    k.volume1.parse().unwrap_or(0),
                ),
            };
            let vol_mentah = if quote_adalah_token1 { v1 } else { v0 } as f64;

            // Rentang yang harga benar benar lewati, bukan pita tetap. Kalau
            // harga tidak bergerak sama sekali, pita satu persen dipakai
            // sebagai dasar, karena range selebar nol tidak bisa dipasang.
            let (bawah, atas) = if k.tick_maks > k.tick_min {
                (k.tick_min, k.tick_maks)
            } else {
                (p.tick - TICK_PER_PERSEN, p.tick + TICK_PER_PERSEN)
            };
            let kedalaman = kedalaman_rentang(
                p.tick,
                bawah,
                atas,
                likuiditas,
                quote_adalah_token1,
                Satuan {
                    harga,
                    skala_meme,
                    skala_quote,
                },
            );

            (
                Some(harga),
                (mm.total_supply > 0.0).then_some(harga * mm.total_supply),
                Some(vol_mentah / skala_quote),
                kedalaman,
            )
        }
        _ => (None, None, None, None),
    };

    let terukur = (!k.dynamic_fee)
        .then_some(())
        .and(volume_quote)
        .zip(kedalaman_quote);
    let hasil_jendela = terukur.and_then(|(v, d)| hasil_per_jendela(v, k.fee, d));
    let fee_apr = terukur.and_then(|(v, d)| proyeksi_apr(v, k.fee, d, jendela_detik));

    // Umur diukur sampai ujung jendela pemindaian, bukan sampai sekarang,
    // supaya seluruh baris pada satu hasil memakai patokan yang sama.
    let umur_jam = umur_jam(k.blok_dibuat, sampai_blok, registry.chain.block_time_ms);

    let risiko = risiko_pool(k, likuiditas, allowlist_hook);
    let (label, alasan) = tentukan_label(
        k.swap,
        k.efisiensi,
        perpindahan_persen,
        quote_dikenal,
        &risiko,
    );

    BarisScan {
        pool_id: k.pool_id.clone(),
        token: alamat_meme,
        symbol: simbol(m_meme),
        quote: alamat_quote,
        quote_symbol: simbol(m_quote),
        quote_dikenal,
        fee: k.fee,
        dynamic_fee: k.dynamic_fee,
        hooks: (k.hooks != super::ZERO_ADDRESS).then(|| k.hooks.clone()),
        tick: p.tick,
        tick_spacing: k.tick_spacing,
        liquidity: p.liquidity.clone(),
        harga,
        mcap,
        volume: volume_quote,
        swap: k.swap,
        kedalaman: kedalaman_quote,
        umur_jam,
        hasil_jendela,
        fee_apr,
        modal_acuan: MODAL_ACUAN,
        efisiensi: k.efisiensi,
        lebar_persen,
        perpindahan_persen,
        risiko,
        label,
        alasan,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pool_native_memiliki_metadata_satuan_dan_volume_scan() {
        use alloy::sol_types::SolValue;
        let registry =
            Registry::load_from(&Registry::default_path(&crate::config::repo_root())).unwrap();
        let rpc = alloy::transports::mock::Asserter::new();
        rpc.push_success(&crate::chain::multicall::respons_uji(vec![
            Some(U256::from(18).abi_encode().into()),
            Some(
                (U256::from(100_000) * U256::from(10u64.pow(18)))
                    .abi_encode()
                    .into(),
            ),
            Some("MEME".to_string().abi_encode().into()),
        ]));
        let provider = alloy::providers::ProviderBuilder::new().connect_mocked_client(rpc);
        let token = Address::repeat_byte(0x11);
        // Native di awal dan akhir tidak menggeser indeks respons ERC20.
        let metadata = super::super::token_meta_batch(
            &provider,
            &registry,
            &[Address::ZERO, token, Address::ZERO],
        )
        .await
        .unwrap();
        assert_eq!(metadata[0].as_ref().unwrap().symbol, "ETH");
        assert_eq!(metadata[1].as_ref().unwrap().symbol, "MEME");
        assert_eq!(metadata[2].as_ref().unwrap().decimals, 18);
        let meta = metadata
            .into_iter()
            .flatten()
            .map(|m| (m.address.to_lowercase(), m))
            .collect();
        let mut candidate = kandidat_uji();
        candidate.token0 = super::super::ZERO_ADDRESS.into();
        candidate.token1 = token.to_string();
        candidate.volume = "1000000000000000000".into();
        let pool = PoolInfo {
            version: PoolVersion::V4,
            id: candidate.pool_id.clone(),
            token0: candidate.token0.clone(),
            token1: candidate.token1.clone(),
            fee: candidate.fee,
            dynamic_fee: false,
            tick_spacing: candidate.tick_spacing,
            hooks: None,
            sqrt_price_x96: crate::math::tick_math::get_sqrt_ratio_at_tick(0)
                .unwrap()
                .to_string(),
            tick: 0,
            liquidity: "1000000000000000000".into(),
            raw_price: 1.0,
            stats: None,
        };
        let row = susun_baris(
            &candidate,
            &pool,
            None,
            &Konteks {
                registry: &registry,
                meta: &meta,
                jendela_detik: 200.0,
                sampai_blok: 200,
                allowlist_hook: &[],
            },
        );
        assert!(row.quote_dikenal);
        assert_eq!(row.quote_symbol, "ETH");
        assert_eq!(row.harga, Some(1.0));
        assert_eq!(row.volume, Some(1.0));
    }

    /// Harga satu banding satu dengan skala yang sama, supaya uji kedalaman
    /// menguji geometrinya saja, bukan konversi desimalnya.
    const SATUAN_UJI: Satuan = Satuan {
        harga: 1.0,
        skala_meme: 1.0,
        skala_quote: 1.0,
    };

    fn kandidat_uji() -> Kandidat {
        Kandidat {
            pool_id: "0xaa".into(),
            token0: "0x1111111111111111111111111111111111111111".into(),
            token1: "0x2222222222222222222222222222222222222222".into(),
            fee: 10_000,
            dynamic_fee: false,
            tick_spacing: 200,
            hooks: super::super::ZERO_ADDRESS.into(),
            swap: 50,
            volume: "0".into(),
            volume1: "0".into(),
            blok_dibuat: 100,
            efisiensi: Some(0.1),
            tick_awal: 0,
            tick_akhir: 0,
            tick_min: 0,
            tick_maks: 0,
        }
    }

    #[test]
    fn risiko_menghalangi_pembukaan() {
        // Label `Hindari` harus benar benar menghalangi, bukan sekadar menandai.
        // PRD 3.4 menyebutnya sebagai baris yang tidak bisa diklik untuk LP.
        let (label, alasan) = tentukan_label(
            100,
            Some(0.1),
            Some(0.0),
            true,
            &["hooknya asing".to_string()],
        );
        assert_eq!(label, Label::Hindari);
        assert!(!label.boleh_dibuka());
        assert!(alasan.contains("hooknya asing"), "alasan harus ikut");
    }

    #[test]
    fn swap_sedikit_tidak_diberi_arah() {
        // Rasio efisiensi atas dua swap selalu satu, karena tidak ada jalan lain
        // dari titik pertama ke titik kedua. Menyajikannya sebagai tren adalah
        // kebisingan yang berpakaian sinyal.
        let (label, _) = tentukan_label(2, Some(1.0), Some(50.0), true, &[]);
        assert_eq!(label, Label::Pantau);
    }

    #[test]
    fn efisiensi_rendah_berarti_menyamping() {
        // Perpindahannya harus di atas ambang arah, kalau tidak yang menjawab
        // adalah cabang perpindahan kecil dan rasio efisiensinya tidak diuji.
        let (label, alasan) = tentukan_label(100, Some(0.05), Some(6.0), true, &[]);
        assert_eq!(label, Label::LpSideways);
        assert!(alasan.contains("0.05"), "rasio harus disebut: {alasan}");
    }

    #[test]
    fn perpindahan_kecil_bukan_tren_meski_jalannya_lurus() {
        // Kasus nyata dari pemindaian: rasio efisiensi 1,00 dengan perpindahan
        // 0,0 persen diberi label "LP Jual Naik". Rasio efisiensi mengukur
        // kelurusan jalan, bukan panjangnya, dan dua tick selalu lurus.
        let (label, alasan) = tentukan_label(100, Some(1.0), Some(0.04), true, &[]);
        assert_eq!(label, Label::LpSideways);
        assert!(
            alasan.contains("0.04"),
            "perpindahan harus disebut: {alasan}"
        );
    }

    #[test]
    fn perpindahan_besar_tetap_diberi_arah() {
        let (label, _) = tentukan_label(100, Some(0.9), Some(8.0), true, &[]);
        assert_eq!(label, Label::LpJualNaik);
    }

    #[test]
    fn arah_menentukan_sisi_range() {
        let (turun, _) = tentukan_label(100, Some(0.8), Some(-12.0), true, &[]);
        assert_eq!(turun, Label::LpBeliTurun);
        let (naik, _) = tentukan_label(100, Some(0.8), Some(12.0), true, &[]);
        assert_eq!(naik, Label::LpJualNaik);
    }

    #[test]
    fn harga_diam_bukan_menyamping() {
        // Pool yang ramai tapi tidak bergerak sama sekali biasanya perdagangan
        // bolak balik di satu tick. Menyebutnya menyamping akan menempatkannya
        // di puncak daftar, padahal ia justru yang paling tidak menarik.
        let (label, _) = tentukan_label(100, None, None, true, &[]);
        assert_eq!(label, Label::Pantau);
    }

    #[test]
    fn hook_di_allowlist_tidak_dianggap_risiko() {
        let mut k = kandidat_uji();
        k.hooks = "0xAbCd000000000000000000000000000000000000".into();
        // Pencocokan harus buta huruf besar kecil: alamat datang dari log dalam
        // bentuk checksum, sedangkan allowlist ditulis tangan di berkas config.
        assert!(
            risiko_pool(&k, 1_000, &[k.hooks.to_lowercase()]).is_empty(),
            "hook yang sudah diizinkan tidak boleh dianggap risiko"
        );
        assert!(
            !risiko_pool(&k, 1_000, &[]).is_empty(),
            "hook asing harus ditolak"
        );
    }

    #[test]
    fn umur_pool_tanpa_initialize_dilaporkan_nol() {
        // Blok nol berarti eventnya tidak ketemu. Menghitungnya apa adanya akan
        // menghasilkan umur ribuan jam dan membuat pool baru terlihat matang.
        assert_eq!(umur_jam(0, 57_000_000, 100), 0.0);
        assert!((umur_jam(56_964_000, 57_000_000, 100) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn likuiditas_nol_ditolak() {
        let k = kandidat_uji();
        let r = risiko_pool(&k, 0, &[]);
        assert!(r.iter().any(|x| x.contains("nol")));
    }

    #[test]
    fn tick_ke_persen_punya_tanda_yang_benar() {
        // Kekeliruan tanda di sini membalik setiap label berarah tanpa
        // memunculkan error apa pun, jadi ia diuji langsung.
        assert!(tick_ke_persen(100) > 0.9 && tick_ke_persen(100) < 1.1);
        assert!(tick_ke_persen(-100) < -0.9 && tick_ke_persen(-100) > -1.1);
        assert_eq!(tick_ke_persen(0), 0.0);
    }

    #[test]
    fn kutipan_tak_dikenal_tidak_diberi_label_apa_apa() {
        // Tanpa kutipan yang dikenal tidak ada satuan untuk menilai hasilnya.
        // Pemindaian sungguhan pernah memberi label "LP Sideways" pada pool
        // yang kedua sisinya token asing, dan label itu tidak berarti apa apa.
        let (label, alasan) = tentukan_label(500, Some(0.05), Some(5.0), false, &[]);
        assert_eq!(label, Label::Pantau);
        assert!(
            alasan.contains("kutipan"),
            "alasan harus menyebutnya: {alasan}"
        );
    }

    #[test]
    fn gerakan_ekstrem_tidak_dikejar() {
        // Kasus nyata: perpindahan 1.888 persen dalam satu jendela diberi
        // label "LP Jual Naik". Range yang dipasang setelah gerakan sebesar
        // itu akan langsung tertinggal.
        let (label, alasan) = tentukan_label(500, Some(0.52), Some(1888.1), true, &[]);
        assert_eq!(label, Label::Pantau);
        assert!(
            alasan.contains("tertinggal"),
            "alasan harus jelas: {alasan}"
        );

        // Tepat di bawah ambang masih boleh diberi arah.
        let (label, _) = tentukan_label(
            500,
            Some(0.9),
            Some(BATAS_TERLALU_JAUH_PERSEN - 1.0),
            true,
            &[],
        );
        assert_eq!(label, Label::LpJualNaik);
    }

    #[test]
    fn modal_acuan_mengencerkan_pool_tipis() {
        // Inilah kekeliruan yang menghasilkan proyeksi 1.890.130 persen: pada
        // pool yang kedalamannya jauh lebih kecil daripada modal, bagian kita
        // mendekati satu dan hasilnya tidak boleh melewati volume dikali fee
        // dibagi modal kita sendiri.
        let tipis = hasil_per_jendela(35_500.0, 9_000, 296.0).unwrap();
        let batas = 35_500.0 * 0.009 / MODAL_ACUAN * 100.0;
        assert!(
            tipis <= batas,
            "hasil {tipis} melewati batas {batas} yang mustahil dilampaui"
        );
    }

    #[test]
    fn pool_tebal_mendekati_pembagian_lurus() {
        // Kalau poolnya jauh lebih besar daripada modal acuan, pengenceran
        // hampir tidak terasa, dan rumusnya harus kembali ke bentuk sederhana.
        let kedalaman = MODAL_ACUAN * 10_000.0;
        let dengan = hasil_per_jendela(1_000_000.0, 3_000, kedalaman).unwrap();
        let lurus = 1_000_000.0 * 0.003 / kedalaman * 100.0;
        assert!(
            (dengan / lurus - 1.0).abs() < 0.001,
            "dengan pengenceran {dengan}, tanpa {lurus}"
        );
    }

    #[test]
    fn kedalaman_rentang_lebar_butuh_modal_lebih_besar() {
        // Range yang lebih lebar butuh modal lebih besar untuk likuiditas yang
        // sama. Mengukur modal pada pita yang lebih sempit daripada range yang
        // sesungguhnya dibutuhkan membuat proyeksi hasilnya terlihat besar.
        let sempit = kedalaman_rentang(0, -100, 100, 1_000_000_000, true, SATUAN_UJI).unwrap();
        let lebar = kedalaman_rentang(0, -1_000, 1_000, 1_000_000_000, true, SATUAN_UJI).unwrap();
        assert!(
            lebar > sempit,
            "lebar {lebar} harus di atas sempit {sempit}"
        );
    }

    #[test]
    fn modal_tidak_nol_saat_harga_di_ujung_rentang() {
        // Kekeliruan nyata: hanya satu kaki yang dihitung, sehingga setiap kali
        // harga kebetulan berada di ujung rentang yang dilewatinya, kaki di
        // sisi itu menyusut sampai nol dan modalnya terbaca nol. HOUND tampil
        // dengan modal 0,0000 dan hasil 142 persen karena ini.
        //
        // Harga di batas ATAS rentang: kaki token0 kosong, kaki token1 penuh.
        let di_atas = kedalaman_rentang(0, -1_000, 0, 1_000_000_000, true, SATUAN_UJI)
            .expect("harus ada nilainya");
        assert!(di_atas > 0.0, "modal {di_atas} tidak boleh nol");

        // Harga di batas BAWAH rentang: kebalikannya.
        let di_bawah = kedalaman_rentang(0, 0, 1_000, 1_000_000_000, true, SATUAN_UJI)
            .expect("harus ada nilainya");
        assert!(di_bawah > 0.0, "modal {di_bawah} tidak boleh nol");
    }

    #[test]
    fn modal_sama_dari_sisi_kutipan_mana_pun() {
        // Pada harga satu banding satu dengan skala yang sama, modal total
        // harus sama apakah kutipannya token0 atau token1. Kalau tidak, ada
        // kaki yang dihitung dua kali atau tidak sama sekali.
        let a = kedalaman_rentang(0, -500, 500, 1_000_000_000, true, SATUAN_UJI).unwrap();
        let b = kedalaman_rentang(0, -500, 500, 1_000_000_000, false, SATUAN_UJI).unwrap();
        assert!((a - b).abs() / a < 1e-6, "token1 {a}, token0 {b}");
    }

    #[test]
    fn kedalaman_rentang_menolak_rentang_terbalik() {
        assert!(kedalaman_rentang(0, 100, -100, 1_000, true, SATUAN_UJI).is_none());
    }

    #[test]
    fn apr_ikut_lamanya_jendela() {
        // Volume yang sama pada jendela yang setengah lebih pendek berarti laju
        // dua kali lipat, jadi proyeksinya harus dua kali lipat juga.
        let sejam = proyeksi_apr(1_000.0, 10_000, 100_000.0, 3_600.0).unwrap();
        let setengah_jam = proyeksi_apr(1_000.0, 10_000, 100_000.0, 1_800.0).unwrap();
        assert!((setengah_jam / sejam - 2.0).abs() < 1e-9);
    }

    #[test]
    fn kedalaman_nol_tetap_menghasilkan_angka_terbatas() {
        // Rumus lama membagi dengan kedalaman, sehingga kedalaman nol berarti
        // tak hingga, dan tak hingga yang ditampilkan sebagai APR menempatkan
        // pool mati di puncak daftar. Rumus sekarang memuat modal kita sendiri
        // di penyebut, jadi kasus itu tidak bisa muncul lagi: kalau poolnya
        // kosong, kitalah seluruh poolnya, dan bagian kita tepat satu.
        let h = hasil_per_jendela(1_000.0, 10_000, 0.0).expect("harus ada nilainya");
        assert!(h.is_finite(), "hasil {h} harus terbatas");
        // Bagian satu penuh: seluruh fee volume itu jatuh ke modal acuan.
        let penuh = 1_000.0 * 0.01 / MODAL_ACUAN * 100.0;
        assert!((h - penuh).abs() < 1e-9, "hasil {h}, seharusnya {penuh}");
    }

    #[test]
    fn apr_menolak_jendela_nol() {
        // Jendela nol berarti tidak ada yang terukur, dan merentangkannya ke
        // setahun berarti membagi dengan nol.
        assert!(proyeksi_apr(1_000.0, 10_000, 500.0, 0.0).is_none());
    }

    #[test]
    fn kedalaman_nol_kalau_tidak_ada_likuiditas() {
        assert!(kedalaman_1_persen(0, 0, true, SATUAN_UJI).is_none());
    }

    #[test]
    fn kedalaman_naik_seiring_likuiditas() {
        let kecil = kedalaman_1_persen(0, 1_000_000, true, SATUAN_UJI).unwrap();
        let besar = kedalaman_1_persen(0, 2_000_000, true, SATUAN_UJI).unwrap();
        assert!(besar > kecil);
    }
}
