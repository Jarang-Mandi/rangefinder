//! Doktrin penempatan range, PRD 5.7.
//!
//! KENAPA MODUL INI ADA
//! User menyatakan masalahnya sendiri: bingung menentukan di mana harus
//! menempatkan posisi supaya benar benar menghasilkan. Detektor sinyal menjawab
//! "kapan", modul ini menjawab "di mana".
//!
//! PRINSIP DASARNYA
//! Di concentrated liquidity, harga yang naik menembus range membuat kita
//! berakhir memegang seluruhnya token kutipan, artinya kita telah MENJUAL.
//! Harga yang turun menembus range membuat kita berakhir memegang seluruhnya
//! memecoin, artinya kita telah MEMBELI.
//!
//! Jadi menempatkan range sama dengan menyatakan dua hal sekaligus: pada harga
//! berapa saya bersedia membeli, dan pada harga berapa saya bersedia menjual.
//! Setiap saran di bawah kembali ke bentuk itu.
//!
//! INVARIAN
//! - Setiap saran WAJIB menyebutkan alasannya dalam bentuk itu. Angka tanpa
//!   alasan tidak membantu user belajar, dan user yang tidak belajar akan terus
//!   bergantung pada alat ini bahkan saat alatnya salah.
//! - Batas bawah tidak pernah ditaruh di bawah level pembatalan. Range yang
//!   melewati level pembatalan berarti bersedia terus membeli sesuatu yang
//!   anggapannya sudah terbukti salah.

use serde::Serialize;

/// Bentuk range yang disarankan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Bentuk {
    /// Mengelilingi harga sekarang. Untuk pasar yang sudah menyamping.
    DuaSisi,
    /// Seluruhnya di bawah harga. Tangga limit buy yang dibayar fee.
    BeliTurun,
    /// Seluruhnya di atas harga. Tangga limit sell yang dibayar fee.
    JualNaik,
}

impl Bentuk {
    pub fn nama(&self) -> &'static str {
        match self {
            Bentuk::DuaSisi => "dua sisi",
            Bentuk::BeliTurun => "beli turun",
            Bentuk::JualNaik => "jual naik",
        }
    }
}

/// Saran penempatan beserta alasannya.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Saran {
    pub bentuk: Bentuk,
    pub bawah: f64,
    pub atas: f64,
    /// Lebar range relatif terhadap harga sekarang, dalam persen.
    pub lebar_persen: f64,
    /// Berapa kali lipat konsentrasi likuiditas dibanding range penuh.
    ///
    /// Ini yang membuat range sempit menghasilkan lebih banyak fee: modal yang
    /// sama menyediakan kedalaman lebih besar pada rentang harga yang dipakai.
    pub konsentrasi: f64,
    /// Kalimat yang menjelaskan saran ini sebagai keputusan beli dan jual.
    pub alasan: Vec<String>,
    /// Harga di mana anggapan dasarnya terbukti salah.
    pub pembatalan: Option<f64>,
}

/// Perkiraan konsentrasi likuiditas sebuah range.
///
/// Rumus pendekatannya: modal yang tersebar pada rentang sempit menyediakan
/// kedalaman kira kira sebanding dengan akar harga dibagi lebar rangenya.
/// Angka ini untuk perbandingan antar pilihan, bukan janji hasil.
pub fn konsentrasi(harga: f64, bawah: f64, atas: f64) -> Option<f64> {
    if !(harga.is_finite() && bawah.is_finite() && atas.is_finite()) {
        return None;
    }
    if bawah <= 0.0 || atas <= bawah || harga <= 0.0 {
        return None;
    }
    let akar_b = bawah.sqrt();
    let akar_a = atas.sqrt();
    if akar_a <= akar_b {
        return None;
    }
    Some(harga.sqrt() / (akar_a - akar_b))
}

/// Saran untuk pasar yang sudah menyamping, memakai kanal dari detektor 5.2.
///
/// Range ditaruh sedikit di dalam kanal, bukan tepat di batasnya. Batas kanal
/// adalah tempat harga berbalik, dan menaruh batas range persis di situ berarti
/// posisinya keluar range tepat saat harga menyentuhnya.
pub fn dari_kanal(harga: f64, kanal_bawah: f64, kanal_atas: f64) -> Option<Saran> {
    if !(harga > 0.0 && kanal_atas > kanal_bawah && kanal_bawah > 0.0) {
        return None;
    }

    // Tarik masuk sepersepuluh lebar kanal dari tiap sisi.
    let lebar = kanal_atas - kanal_bawah;
    let bawah = kanal_bawah + lebar * 0.10;
    let atas = kanal_atas - lebar * 0.10;
    if atas <= bawah {
        return None;
    }

    let konsentrasi = konsentrasi(harga, bawah, atas)?;
    let lebar_persen = (atas - bawah) / harga * 100.0;

    Some(Saran {
        bentuk: Bentuk::DuaSisi,
        bawah,
        atas,
        lebar_persen,
        konsentrasi,
        alasan: vec![
            format!(
                "Batas bawah {bawah:.6} berarti bersedia membeli di situ, dan itu sedikit di atas \
                 dasar kanal {kanal_bawah:.6} tempat harga selama ini berbalik"
            ),
            format!(
                "Batas atas {atas:.6} berarti bersedia menjual di situ, sedikit di bawah puncak \
                 kanal {kanal_atas:.6}"
            ),
            "Ditarik masuk sepersepuluh lebar kanal dari tiap sisi, karena batas kanal adalah \
             tempat harga berbalik, dan menaruh batas range persis di situ membuat posisi keluar \
             range tepat saat harga menyentuhnya"
                .to_string(),
        ],
        pembatalan: Some(kanal_bawah),
    })
}

/// Saran untuk pola dump lalu basis, memakai jangkar dari detektor 5.2b.
///
/// Inilah kasus yang paling bernilai. Basis memberi jangkar yang jauh lebih
/// dapat dipercaya daripada kanal statistik, karena ia terbentuk dari perilaku
/// sungguhan: penjual habis di satu harga, dan pembeli mempertahankannya.
pub fn dari_basis(
    harga: f64,
    l_basis: f64,
    h_basis: f64,
    poc: f64,
    p_invalid: f64,
) -> Option<Saran> {
    if !(harga > 0.0 && h_basis > l_basis && l_basis > 0.0) {
        return None;
    }

    // Batas bawah sedikit DI ATAS dasar basis, bukan di dasarnya.
    //
    // Dasar basis adalah tempat penjual habis. Menaruh batas di situ berarti
    // posisinya keluar range tepat saat harga mengujinya lagi, dan pengujian
    // ulang adalah hal yang normal terjadi.
    let lebar = h_basis - l_basis;
    let bawah = (l_basis + lebar * 0.15).max(p_invalid.max(0.0) + f64::EPSILON);
    let atas = h_basis + lebar * 0.10;
    if atas <= bawah {
        return None;
    }

    let konsentrasi = konsentrasi(harga, bawah, atas)?;
    let lebar_persen = (atas - bawah) / harga * 100.0;

    let mut alasan = vec![
        format!(
            "Batas bawah {bawah:.6} berarti bersedia membeli sedikit di atas dasar basis \
             {l_basis:.6}, tempat penjual sudah habis"
        ),
        format!(
            "Batas atas {atas:.6} berarti bersedia menjual sedikit di atas puncak basis \
             {h_basis:.6}, karena keluar dari basis ke atas adalah hasil yang memang diinginkan"
        ),
        format!(
            "Harga paling ramai di basis ada di {poc:.6}. Range yang mencakupnya memungut fee \
             paling banyak, karena di situlah perdagangan benar benar terjadi"
        ),
    ];

    if p_invalid > 0.0 {
        alasan.push(format!(
            "Di bawah {p_invalid:.6} anggapan basisnya terbukti salah. Batas bawah tidak pernah \
             ditaruh di bawah itu, karena range yang melewatinya berarti bersedia terus membeli \
             sesuatu yang sudah terbukti keliru"
        ));
    }

    Some(Saran {
        bentuk: Bentuk::DuaSisi,
        bawah,
        atas,
        lebar_persen,
        konsentrasi,
        alasan,
        pembatalan: (p_invalid > 0.0).then_some(p_invalid),
    })
}

/// Range satu sisi di bawah harga, sebagai tangga limit buy yang dibayar fee.
///
/// Berguna setiap hari, bukan hanya saat pasar menyamping. Kalau harga tidak
/// pernah turun ke sana, tidak ada yang terbeli dan tidak ada yang rugi. Kalau
/// turun, kita membeli pada harga yang memang kita mau, sambil dibayar fee
/// selama menunggu.
pub fn beli_turun(harga: f64, target_bawah: f64, target_atas: f64) -> Option<Saran> {
    if !(harga > 0.0 && target_atas > target_bawah && target_bawah > 0.0) {
        return None;
    }
    if target_atas >= harga {
        // Kalau batas atasnya sudah menyentuh harga sekarang, ini bukan lagi
        // satu sisi, dan sebagian modal akan langsung jadi memecoin.
        return None;
    }

    let konsentrasi = konsentrasi(harga, target_bawah, target_atas)?;
    Some(Saran {
        bentuk: Bentuk::BeliTurun,
        bawah: target_bawah,
        atas: target_atas,
        lebar_persen: (target_atas - target_bawah) / harga * 100.0,
        konsentrasi,
        alasan: vec![
            format!(
                "Seluruh range berada di bawah harga sekarang {harga:.6}, jadi isinya hanya token \
                 kutipan. Ini tangga limit buy dari {target_bawah:.6} sampai {target_atas:.6}"
            ),
            "Kalau harga tidak pernah turun ke sana, tidak ada yang terbeli dan tidak ada yang \
             rugi. Kalau turun, kita membeli pada harga yang memang dimau, sambil dibayar fee \
             selama menunggu"
                .to_string(),
        ],
        pembatalan: None,
    })
}

/// Range satu sisi di atas harga, sebagai tangga limit sell yang dibayar fee.
pub fn jual_naik(harga: f64, target_bawah: f64, target_atas: f64) -> Option<Saran> {
    if !(harga > 0.0 && target_atas > target_bawah && target_bawah > 0.0) {
        return None;
    }
    if target_bawah <= harga {
        return None;
    }

    let konsentrasi = konsentrasi(harga, target_bawah, target_atas)?;
    Some(Saran {
        bentuk: Bentuk::JualNaik,
        bawah: target_bawah,
        atas: target_atas,
        lebar_persen: (target_atas - target_bawah) / harga * 100.0,
        konsentrasi,
        alasan: vec![
            format!(
                "Seluruh range berada di atas harga sekarang {harga:.6}, jadi isinya hanya \
                 memecoin. Ini tangga limit sell dari {target_bawah:.6} sampai {target_atas:.6}"
            ),
            "Kalau harga tidak pernah naik ke sana, tidak ada yang terjual. Kalau naik, kita \
             menjual pada harga yang memang dimau, sambil dibayar fee selama menunggu"
                .to_string(),
        ],
        pembatalan: None,
    })
}

/// Bandingkan konsentrasi sebuah saran terhadap range yang jauh lebih lebar.
///
/// Dipakai panel untuk menjelaskan kenapa range sempit lebih menghasilkan.
/// Mengembalikan berapa kali lipat, atau `None` kalau tidak bisa dihitung.
pub fn dibanding_lebar(harga: f64, saran: &Saran, kelipatan_lebar: f64) -> Option<f64> {
    if kelipatan_lebar <= 1.0 {
        return None;
    }
    let tengah = (saran.bawah + saran.atas) / 2.0;
    let setengah = (saran.atas - saran.bawah) / 2.0 * kelipatan_lebar;
    let lebar = konsentrasi(harga, (tengah - setengah).max(1e-18), tengah + setengah)?;
    if lebar <= 0.0 {
        return None;
    }
    Some(saran.konsentrasi / lebar)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hampir(a: f64, b: f64, toleransi: f64) -> bool {
        (a - b).abs() < toleransi
    }

    #[test]
    fn range_sempit_lebih_terkonsentrasi() {
        // Ini alasan seluruh alat ini ada. Modal yang sama pada range yang lebih
        // sempit menyediakan kedalaman lebih besar, jadi memungut fee lebih
        // banyak dari volume yang sama.
        let sempit = konsentrasi(100.0, 95.0, 105.0).unwrap();
        let lebar = konsentrasi(100.0, 50.0, 200.0).unwrap();
        assert!(
            sempit > lebar * 2.0,
            "sempit {sempit} seharusnya jauh di atas lebar {lebar}"
        );
    }

    #[test]
    fn konsentrasi_menolak_masukan_tidak_masuk_akal() {
        assert_eq!(konsentrasi(100.0, 105.0, 95.0), None, "batas terbalik");
        assert_eq!(konsentrasi(100.0, 0.0, 105.0), None, "batas bawah nol");
        assert_eq!(konsentrasi(0.0, 95.0, 105.0), None, "harga nol");
        assert_eq!(konsentrasi(f64::NAN, 95.0, 105.0), None);
    }

    #[test]
    fn saran_kanal_ditarik_masuk_dari_batas() {
        // Batas kanal adalah tempat harga berbalik. Menaruh batas range persis
        // di situ membuat posisi keluar range tepat saat harga menyentuhnya.
        let s = dari_kanal(100.0, 90.0, 110.0).unwrap();
        assert!(s.bawah > 90.0, "batas bawah {} harus di atas dasar kanal", s.bawah);
        assert!(s.atas < 110.0, "batas atas {} harus di bawah puncak kanal", s.atas);
        assert_eq!(s.bentuk, Bentuk::DuaSisi);
    }

    #[test]
    fn setiap_saran_menjelaskan_beli_dan_jual() {
        // Angka tanpa alasan tidak membantu user belajar.
        let s = dari_kanal(100.0, 90.0, 110.0).unwrap();
        let semua = s.alasan.join(" ");
        assert!(semua.contains("membeli"), "alasan harus menyebut membeli");
        assert!(semua.contains("menjual"), "alasan harus menyebut menjual");
    }

    #[test]
    fn saran_basis_tidak_pernah_di_bawah_level_pembatalan() {
        // Range yang melewati level pembatalan berarti bersedia terus membeli
        // sesuatu yang anggapannya sudah terbukti salah.
        let s = dari_basis(95.0, 90.0, 100.0, 94.0, 93.0).unwrap();
        assert!(
            s.bawah >= 93.0,
            "batas bawah {} tidak boleh di bawah pembatalan 93",
            s.bawah
        );
        assert_eq!(s.pembatalan, Some(93.0));
    }

    #[test]
    fn saran_basis_menaruh_batas_bawah_di_atas_dasar() {
        // Dasar basis adalah tempat penjual habis, dan pengujian ulang adalah
        // hal normal. Menaruh batas persis di dasar membuat posisi keluar range
        // tepat saat itu terjadi.
        let s = dari_basis(95.0, 90.0, 100.0, 94.0, 0.0).unwrap();
        assert!(s.bawah > 90.0, "batas bawah {}", s.bawah);
    }

    #[test]
    fn saran_basis_menyebut_titik_paling_ramai() {
        let s = dari_basis(95.0, 90.0, 100.0, 94.0, 88.0).unwrap();
        assert!(
            s.alasan.iter().any(|a| a.contains("paling ramai")),
            "dapat {:?}",
            s.alasan
        );
    }

    #[test]
    fn beli_turun_menolak_range_yang_menyentuh_harga() {
        // Kalau batas atasnya menyentuh harga sekarang, ini bukan lagi satu
        // sisi, dan sebagian modal langsung berubah jadi memecoin.
        assert_eq!(beli_turun(100.0, 90.0, 100.0), None);
        assert_eq!(beli_turun(100.0, 95.0, 105.0), None);
        assert!(beli_turun(100.0, 85.0, 95.0).is_some());
    }

    #[test]
    fn jual_naik_menolak_range_yang_menyentuh_harga() {
        assert_eq!(jual_naik(100.0, 100.0, 110.0), None);
        assert_eq!(jual_naik(100.0, 95.0, 110.0), None);
        assert!(jual_naik(100.0, 105.0, 115.0).is_some());
    }

    #[test]
    fn satu_sisi_menjelaskan_kenapa_tidak_rugi_kalau_tidak_kena() {
        let s = beli_turun(100.0, 85.0, 95.0).unwrap();
        let semua = s.alasan.join(" ");
        assert!(semua.contains("tidak ada yang rugi") || semua.contains("tidak ada yang terbeli"));
    }

    #[test]
    fn perbandingan_terhadap_range_lebar() {
        let s = dari_kanal(100.0, 90.0, 110.0).unwrap();
        let kali = dibanding_lebar(100.0, &s, 5.0).unwrap();
        assert!(kali > 1.0, "range sempit harus lebih terkonsentrasi, dapat {kali}");
        assert!(
            hampir(kali, 5.0, 0.5),
            "lima kali lebih sempit kira kira lima kali lebih terkonsentrasi, dapat {kali}"
        );
    }

    #[test]
    fn perbandingan_menolak_kelipatan_tidak_masuk_akal() {
        let s = dari_kanal(100.0, 90.0, 110.0).unwrap();
        assert_eq!(dibanding_lebar(100.0, &s, 1.0), None);
        assert_eq!(dibanding_lebar(100.0, &s, 0.5), None);
    }

    #[test]
    fn lebar_persen_dihitung_terhadap_harga_sekarang() {
        let s = dari_kanal(100.0, 90.0, 110.0).unwrap();
        // Kanal 90 sampai 110 ditarik masuk 2 dari tiap sisi, jadi 92 sampai 108.
        assert!(
            hampir(s.lebar_persen, 16.0, 0.1),
            "lebar persen {}",
            s.lebar_persen
        );
    }
}
