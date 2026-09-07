//! Notifikasi kejadian penting.
//!
//! KENAPA ADA PENDINGINAN
//! Harga yang bergetar di sekitar batas range akan memicu kejadian keluar dan
//! masuk berulang kali dalam hitungan detik. Tanpa pendinginan, layar user
//! dibanjiri toast yang semuanya benar tapi tidak satu pun berguna, dan yang
//! terjadi berikutnya adalah user mematikan notifikasi seluruhnya. Itu hasil
//! yang lebih buruk daripada tidak punya notifikasi sama sekali.
//!
//! Pendinginan berlaku per pasangan kode kejadian dan subjeknya, bukan per kode
//! saja. Dua posisi berbeda yang sama sama keluar range adalah dua kabar yang
//! berbeda, dan menahan yang kedua berarti menyembunyikan kejadian sungguhan.
//!
//! INVARIAN
//! - Riwayat menyimpan SEMUA kejadian, termasuk yang tertahan pendinginan.
//!   Toast boleh ditahan, catatan tidak, karena catatan itu yang dipakai untuk
//!   menjelaskan apa yang terjadi saat user tidak menatap layar.
//! - Kejadian kritis tetap ikut pendinginan. Kritis berarti layak menetap di
//!   layar sampai diklik, bukan layak diulang setiap detik.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Berapa lama kejadian yang sama untuk subjek yang sama ditahan.
///
/// Lima menit, sesuai PRD 3.7. Angkanya sengaja bisa diubah, karena ambang yang
/// tepat bergantung pada seberapa sempit range yang biasa dipakai user.
pub const PENDINGINAN_DETIK_BAWAAN: i64 = 300;

/// Berapa kejadian terakhir yang disimpan.
pub const RIWAYAT_MAKSIMUM: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tingkat {
    Info,
    Warning,
    Critical,
}

impl Tingkat {
    /// Berapa lama toast bertahan di layar, dalam milidetik.
    ///
    /// Nol berarti menetap sampai diklik. Angkanya mengikuti tabel di PRD 3.7.
    pub fn durasi_ms(&self) -> u32 {
        match self {
            Tingkat::Info => 6_000,
            Tingkat::Warning => 12_000,
            Tingkat::Critical => 0,
        }
    }

    /// Berapa ketukan nada yang dibunyikan.
    pub fn ketukan(&self) -> u8 {
        match self {
            Tingkat::Info => 1,
            Tingkat::Warning => 2,
            Tingkat::Critical => 3,
        }
    }
}

/// Satu kejadian yang layak diberitahukan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Kejadian {
    pub level: Tingkat,
    /// Kode mesin, dipakai untuk pendinginan dan penyaringan.
    pub code: String,
    pub message: String,
    /// Subjek kejadian, biasanya token id posisi. Kosong untuk kejadian yang
    /// tidak terikat posisi tertentu, misalnya RPC mati.
    pub subject: Option<String>,
    pub at: String,
    /// Benar kalau kejadian ini tertahan pendinginan sehingga tidak
    /// dimunculkan sebagai toast. Ia tetap masuk riwayat.
    pub ditahan: bool,
}

/// Penjaga pendinginan beserta riwayatnya.
#[derive(Debug)]
pub struct Notifier {
    pendinginan_detik: i64,
    terakhir: HashMap<String, i64>,
    riwayat: Vec<Kejadian>,
}

impl Default for Notifier {
    fn default() -> Self {
        Self::new(PENDINGINAN_DETIK_BAWAAN)
    }
}

impl Notifier {
    pub fn new(pendinginan_detik: i64) -> Self {
        Self {
            pendinginan_detik,
            terakhir: HashMap::new(),
            riwayat: Vec::new(),
        }
    }

    fn kunci(code: &str, subject: Option<&str>) -> String {
        format!("{code}|{}", subject.unwrap_or(""))
    }

    /// Catat sebuah kejadian, dan katakan apakah ia layak dimunculkan.
    ///
    /// `sekarang_unix` datang dari luar supaya perilakunya bisa diuji tanpa
    /// menunggu waktu berjalan.
    pub fn catat(
        &mut self,
        level: Tingkat,
        code: &str,
        message: String,
        subject: Option<String>,
        sekarang_unix: i64,
    ) -> Kejadian {
        let kunci = Self::kunci(code, subject.as_deref());
        let ditahan = match self.terakhir.get(&kunci) {
            Some(t) => sekarang_unix - t < self.pendinginan_detik,
            None => false,
        };
        if !ditahan {
            self.terakhir.insert(kunci, sekarang_unix);
        }

        let k = Kejadian {
            level,
            code: code.to_string(),
            message,
            subject,
            at: chrono::DateTime::from_timestamp(sekarang_unix, 0)
                .map(|d| d.to_rfc3339())
                .unwrap_or_default(),
            ditahan,
        };

        // Riwayat menyimpan yang tertahan juga. Toast boleh ditahan, catatan
        // tidak, karena catatan itu yang menjelaskan apa yang terjadi saat user
        // tidak menatap layar.
        self.riwayat.push(k.clone());
        if self.riwayat.len() > RIWAYAT_MAKSIMUM {
            let buang = self.riwayat.len() - RIWAYAT_MAKSIMUM;
            self.riwayat.drain(0..buang);
        }

        k
    }

    /// Riwayat terbaru lebih dulu.
    pub fn riwayat(&self) -> Vec<Kejadian> {
        self.riwayat.iter().rev().cloned().collect()
    }

    pub fn bersihkan(&mut self) {
        self.riwayat.clear();
        self.terakhir.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notifier() -> Notifier {
        Notifier::new(300)
    }

    #[test]
    fn kejadian_pertama_tidak_tertahan() {
        let mut n = notifier();
        let k = n.catat(Tingkat::Info, "x", "halo".into(), None, 1000);
        assert!(!k.ditahan);
    }

    #[test]
    fn kejadian_sama_dalam_pendinginan_tertahan() {
        let mut n = notifier();
        n.catat(Tingkat::Info, "x", "a".into(), None, 1000);
        let k = n.catat(Tingkat::Info, "x", "b".into(), None, 1100);
        assert!(k.ditahan, "kejadian yang sama dalam 300 detik harus tertahan");
    }

    #[test]
    fn kejadian_sama_setelah_pendinginan_lewat() {
        let mut n = notifier();
        n.catat(Tingkat::Info, "x", "a".into(), None, 1000);
        let k = n.catat(Tingkat::Info, "x", "b".into(), None, 1301);
        assert!(!k.ditahan);
    }

    #[test]
    fn subjek_berbeda_tidak_saling_menahan() {
        // INI YANG PALING MUDAH SALAH.
        //
        // Dua posisi berbeda yang sama sama keluar range adalah dua kabar
        // berbeda. Pendinginan per kode saja akan menyembunyikan yang kedua,
        // dan posisi itu bisa jadi justru yang paling besar.
        let mut n = notifier();
        n.catat(
            Tingkat::Critical,
            "out_of_range",
            "posisi 1".into(),
            Some("1".into()),
            1000,
        );
        let k = n.catat(
            Tingkat::Critical,
            "out_of_range",
            "posisi 2".into(),
            Some("2".into()),
            1001,
        );
        assert!(!k.ditahan);
    }

    #[test]
    fn yang_tertahan_tetap_masuk_riwayat() {
        // Toast boleh ditahan, catatan tidak. Riwayat itu yang menjelaskan apa
        // yang terjadi saat user tidak menatap layar.
        let mut n = notifier();
        n.catat(Tingkat::Info, "x", "a".into(), None, 1000);
        n.catat(Tingkat::Info, "x", "b".into(), None, 1010);
        assert_eq!(n.riwayat().len(), 2);
        assert!(n.riwayat()[0].ditahan, "yang terbaru adalah yang tertahan");
    }

    #[test]
    fn riwayat_terbaru_lebih_dulu() {
        let mut n = notifier();
        n.catat(Tingkat::Info, "a", "pertama".into(), None, 1000);
        n.catat(Tingkat::Info, "b", "kedua".into(), None, 1001);
        assert_eq!(n.riwayat()[0].message, "kedua");
    }

    #[test]
    fn riwayat_dibatasi_lima_puluh() {
        let mut n = notifier();
        for i in 0..80 {
            n.catat(Tingkat::Info, &format!("k{i}"), "x".into(), None, 1000 + i);
        }
        assert_eq!(n.riwayat().len(), RIWAYAT_MAKSIMUM);
        // Yang tersisa adalah yang terbaru, bukan yang terlama.
        assert_eq!(n.riwayat()[0].code, "k79");
    }

    #[test]
    fn kritis_tetap_ikut_pendinginan() {
        // Kritis berarti layak menetap di layar sampai diklik, bukan layak
        // diulang setiap detik.
        let mut n = notifier();
        n.catat(Tingkat::Critical, "rpc", "mati".into(), None, 1000);
        let k = n.catat(Tingkat::Critical, "rpc", "mati".into(), None, 1005);
        assert!(k.ditahan);
    }

    #[test]
    fn durasi_toast_mengikuti_tingkat() {
        assert_eq!(Tingkat::Info.durasi_ms(), 6_000);
        assert_eq!(Tingkat::Warning.durasi_ms(), 12_000);
        assert_eq!(
            Tingkat::Critical.durasi_ms(),
            0,
            "kritis menetap sampai diklik"
        );
    }

    #[test]
    fn ketukan_nada_bertambah_dengan_tingkat() {
        assert_eq!(Tingkat::Info.ketukan(), 1);
        assert_eq!(Tingkat::Warning.ketukan(), 2);
        assert_eq!(Tingkat::Critical.ketukan(), 3);
    }

    #[test]
    fn pendinginan_nol_tidak_pernah_menahan() {
        // Dipakai saat user mematikan pendinginan sepenuhnya.
        let mut n = Notifier::new(0);
        n.catat(Tingkat::Info, "x", "a".into(), None, 1000);
        let k = n.catat(Tingkat::Info, "x", "b".into(), None, 1000);
        assert!(!k.ditahan);
    }
}
