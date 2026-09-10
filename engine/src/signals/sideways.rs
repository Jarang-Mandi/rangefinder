//! Detektor sideways bervolume tinggi, PRD 5.2.
//!
//! Yang dicari: harga yang banyak bergerak tapi tidak ke mana mana, dengan
//! volume yang masih ramai. Itu kondisi terbaik untuk LP, karena fee terus
//! mengalir sementara impermanent loss tetap kecil.
//!
//! INVARIAN
//! - Gate keras dijalankan SEBELUM skor dipakai. Kandidat yang gagal gate tidak
//!   ditampilkan dengan skor rendah, ia dibuang beserta alasannya. Skor rendah
//!   mengundang user menawar, dan gate ada justru supaya tidak ada tawar
//!   menawar.
//! - Bobot dan ambang bukan angka suci. Semuanya ada di satu tempat di bawah,
//!   diberi nama, dan dimaksudkan untuk dikalibrasi ulang. Menyebarnya ke dalam
//!   rumus membuat kalibrasi mustahil.

use serde::{Deserialize, Serialize};

use super::bar::Bar;
use super::metrik;

/// Ambang dan bobot. Titik awal, bukan angka suci.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Aturan {
    /// Jendela utama dalam bar. Bawaan 240, yaitu empat jam pada candle 1 menit.
    pub jendela: usize,
    /// Jendela pendek untuk mengukur kontraksi volatilitas.
    pub jendela_pendek: usize,
    /// Jendela pembanding volume.
    pub jendela_volume: usize,

    pub maks_er: f64,
    pub min_volume_z: f64,
    pub maks_bandwidth: f64,
    pub maks_kontraksi: f64,

    pub bobot_volume: f64,
    pub bobot_er: f64,
    pub bobot_containment: f64,
    pub bobot_tape: f64,
}

impl Default for Aturan {
    fn default() -> Self {
        Self {
            jendela: 240,
            jendela_pendek: 15,
            jendela_volume: 60,
            maks_er: 0.35,
            min_volume_z: 0.50,
            maks_bandwidth: 0.35,
            maks_kontraksi: 0.80,
            // Jumlahnya satu. Kalau ada yang menambah suku baru, ia harus
            // mengurangi yang lain, dan itu memaksa keputusan sadar.
            bobot_volume: 0.25,
            bobot_er: 0.35,
            bobot_containment: 0.25,
            bobot_tape: 0.15,
        }
    }
}

/// Hasil pengukuran, termasuk saat kandidatnya ditolak.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Sinyal {
    pub skor: f64,
    pub er: f64,
    pub volume_z: f64,
    pub containment: f64,
    pub bandwidth: f64,
    pub kontraksi: f64,
    pub tape: f64,
    /// Kosong berarti seluruh gate lolos.
    pub ditolak: Vec<String>,
    /// Batas kanal, dipakai doktrin penempatan.
    pub kanal_bawah: f64,
    pub kanal_atas: f64,
}

impl Sinyal {
    pub fn lolos(&self) -> bool {
        self.ditolak.is_empty()
    }
}

/// Ukur sebuah deret bar.
///
/// Mengembalikan `None` kalau datanya belum cukup untuk jendela yang diminta.
/// Itu berbeda dari skor rendah: yang satu berarti belum bisa diperiksa, yang
/// lain berarti sudah diperiksa dan tidak menarik.
pub fn ukur(bars: &[Bar], a: &Aturan) -> Option<Sinyal> {
    let er = metrik::efficiency_ratio(bars, a.jendela)?;
    let k = metrik::kanal(bars, a.jendela)?;
    let tape = metrik::keseimbangan_tape(bars, a.jendela)?;
    let vz = metrik::volume_z(bars, a.jendela_volume)?;

    let atr_pendek = metrik::atr(bars, a.jendela_pendek)?;
    let atr_panjang = metrik::atr(bars, a.jendela)?;
    if atr_panjang <= 0.0 {
        return None;
    }
    let kontraksi = atr_pendek / atr_panjang;

    let mut ditolak = Vec::new();
    if er >= a.maks_er {
        ditolak.push(format!(
            "harga masih tren, efficiency ratio {er:.2} di atas ambang {:.2}",
            a.maks_er
        ));
    }
    if vz <= a.min_volume_z {
        ditolak.push(format!(
            "volume terlalu sepi, z score {vz:.2} di bawah ambang {:.2}",
            a.min_volume_z
        ));
    }
    if k.bandwidth >= a.maks_bandwidth {
        ditolak.push(format!(
            "kanal terlalu lebar, {:.2} di atas ambang {:.2}",
            k.bandwidth, a.maks_bandwidth
        ));
    }
    if kontraksi >= a.maks_kontraksi {
        ditolak.push(format!(
            "volatilitas belum menyempit, kontraksi {kontraksi:.2} di atas ambang {:.2}",
            a.maks_kontraksi
        ));
    }

    // Volume z bisa bernilai apa saja, jadi dijepit sebelum dipakai sebagai
    // porsi. Tanpa penjepitan, satu lonjakan ekstrem akan mendominasi skor dan
    // membuat suku lain tidak berarti.
    let norm_v = (vz / 3.0).clamp(0.0, 1.0);

    let skor = 100.0
        * (a.bobot_volume * norm_v
            + a.bobot_er * (1.0 - er)
            + a.bobot_containment * k.containment
            + a.bobot_tape * tape);

    Some(Sinyal {
        skor,
        er,
        volume_z: vz,
        containment: k.containment,
        bandwidth: k.bandwidth,
        kontraksi,
        tape,
        ditolak,
        kanal_bawah: k.bawah,
        kanal_atas: k.atas,
    })
}

/// Pengali persistensi rezim.
///
/// Chop mengelompok. Kalau jendela jendela sebelumnya juga menyamping, sinyal
/// sekarang lebih layak dipercaya. Ini yang membuatnya bergeser dari sekadar
/// menggambarkan masa lalu menjadi sedikit meramal.
///
/// `sebelumnya` adalah skor pada jendela jendela terdahulu, terbaru lebih dulu.
pub fn pengali_persistensi(sebelumnya: &[f64], ambang: f64) -> f64 {
    if sebelumnya.len() < 6 {
        return 1.0;
    }
    let cocok = sebelumnya[..6].iter().filter(|s| **s >= ambang).count();
    if cocok >= 4 {
        1.15
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(t: i64, o: f64, h: f64, l: f64, c: f64, v: f64) -> Bar {
        Bar {
            time: t,
            open: o,
            high: h,
            low: l,
            close: c,
            volume: v,
        }
    }

    fn aturan_uji() -> Aturan {
        Aturan {
            jendela: 60,
            jendela_pendek: 10,
            jendela_volume: 30,
            ..Aturan::default()
        }
    }

    /// Chop dengan volume yang membesar di akhir.
    fn chop_ramai(n: usize) -> Vec<Bar> {
        (0..n)
            .map(|i| {
                // Bergantian naik dan turun di sekitar satu harga. Close
                // sengaja berbeda dari open supaya arah tiap bar nyata, karena
                // bar tanpa arah tidak dihitung keseimbangan tape.
                let (o, c) = if i % 2 == 0 {
                    (100.4, 100.0)
                } else {
                    (100.0, 100.4)
                };
                // Volume bervariasi supaya simpangannya tidak nol, lalu naik di
                // bar terakhir supaya z score-nya positif.
                let v = if i == n - 1 {
                    40.0
                } else {
                    10.0 + (i % 5) as f64
                };
                // Rentang menyempit di sepertiga terakhir. Chop yang layak
                // dimasuki memang didahului penyempitan, dan gate kontraksi
                // menegakkan itu.
                let lebar = if i * 3 >= n * 2 { 0.15 } else { 0.6 };
                bar(i as i64, o, 100.2 + lebar, 100.2 - lebar, c, v)
            })
            .collect()
    }

    fn tren_naik(n: usize) -> Vec<Bar> {
        (0..n)
            .map(|i| {
                let p = 100.0 + i as f64 * 0.5;
                bar(
                    i as i64,
                    p,
                    p + 0.5,
                    p - 0.2,
                    p + 0.4,
                    10.0 + (i % 5) as f64,
                )
            })
            .collect()
    }

    #[test]
    fn data_kurang_mengembalikan_none() {
        // Berbeda dari skor rendah. Yang satu belum bisa diperiksa, yang lain
        // sudah diperiksa dan tidak menarik.
        assert_eq!(ukur(&chop_ramai(20), &aturan_uji()), None);
    }

    #[test]
    fn chop_ramai_lolos_gate() {
        let s = ukur(&chop_ramai(120), &aturan_uji()).unwrap();
        assert!(
            s.lolos(),
            "chop bervolume seharusnya lolos, ditolak karena {:?}",
            s.ditolak
        );
        assert!(s.er < 0.35, "efficiency ratio {}", s.er);
        assert!(s.skor > 50.0, "skor {}", s.skor);
    }

    #[test]
    fn tren_ditolak_dengan_alasan() {
        let s = ukur(&tren_naik(120), &aturan_uji()).unwrap();
        assert!(!s.lolos());
        assert!(
            s.ditolak.iter().any(|a| a.contains("tren")),
            "alasan penolakan harus menyebut tren, dapat {:?}",
            s.ditolak
        );
    }

    #[test]
    fn setiap_gate_yang_gagal_dilaporkan() {
        // Melaporkan hanya yang pertama membuat user memperbaiki satu lalu
        // menemukan yang berikutnya, satu per satu.
        let mut a = aturan_uji();
        a.maks_er = 0.0;
        a.min_volume_z = 1e9;
        a.maks_bandwidth = 0.0;
        a.maks_kontraksi = 0.0;
        let s = ukur(&chop_ramai(120), &a).unwrap();
        assert_eq!(s.ditolak.len(), 4, "keempat gate harus dilaporkan");
    }

    #[test]
    fn bobot_berjumlah_satu() {
        // Kalau ada yang menambah suku baru, ia harus mengurangi yang lain.
        let a = Aturan::default();
        let jumlah = a.bobot_volume + a.bobot_er + a.bobot_containment + a.bobot_tape;
        assert!(
            (jumlah - 1.0).abs() < 1e-9,
            "bobot berjumlah {jumlah}, seharusnya satu"
        );
    }

    #[test]
    fn skor_tidak_melebihi_seratus() {
        let s = ukur(&chop_ramai(120), &aturan_uji()).unwrap();
        assert!(s.skor <= 100.0 + 1e-9, "skor {}", s.skor);
    }

    #[test]
    fn volume_z_ekstrem_tidak_mendominasi() {
        // Tanpa penjepitan, satu lonjakan ekstrem membuat suku lain tidak
        // berarti, dan skornya jadi tinggi meski harganya sedang tren.
        let mut bars = tren_naik(120);
        let n = bars.len();
        bars[n - 1].volume = 1_000_000.0;
        let s = ukur(&bars, &aturan_uji()).unwrap();
        assert!(
            !s.lolos(),
            "tren tetap harus ditolak meski volumenya meledak"
        );
    }

    #[test]
    fn persistensi_butuh_empat_dari_enam() {
        assert_eq!(pengali_persistensi(&[80.0; 6], 70.0), 1.15);
        assert_eq!(
            pengali_persistensi(&[80.0, 80.0, 80.0, 60.0, 60.0, 60.0], 70.0),
            1.0,
            "tiga dari enam belum cukup"
        );
    }

    #[test]
    fn persistensi_tanpa_riwayat_cukup_tidak_mengubah_apa_apa() {
        // Token yang baru dipantau tidak boleh mendapat pengali hanya karena
        // riwayatnya belum ada.
        assert_eq!(pengali_persistensi(&[90.0, 90.0], 70.0), 1.0);
    }

    #[test]
    fn kanal_ikut_dikembalikan_untuk_penempatan() {
        let s = ukur(&chop_ramai(120), &aturan_uji()).unwrap();
        assert!(s.kanal_atas > s.kanal_bawah);
    }
}
