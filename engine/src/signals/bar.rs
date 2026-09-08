//! Deret candle yang dipakai seluruh detektor sinyal.
//!
//! INVARIAN
//! - Bar datang dari chart gmgn lewat content script, bukan dari sumber luar.
//!   Alasannya sama dengan kalibrasi kurs: angka yang dipakai untuk keputusan
//!   harus angka yang sama dengan yang dilihat user di layar. Sumber lain akan
//!   selalu sedikit berbeda, dan selisih itu muncul persis saat harga bergerak
//!   cepat, yaitu saat keputusannya paling penting.
//! - Deret yang tidak lengkap TIDAK diisi dengan tebakan. Detektor yang butuh
//!   jendela lebih panjang daripada yang tersedia mengembalikan `None`, bukan
//!   menghitung dari data yang dipendekkan diam diam.
//! - Bar dengan high lebih kecil daripada low, atau volume negatif, dibuang.
//!   Data cacat dari chart lebih baik hilang daripada ikut dihitung.

use serde::{Deserialize, Serialize};

/// Satu candle. Waktu dalam detik unix.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bar {
    pub time: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

impl Bar {
    /// Benar kalau bentuknya masuk akal sebagai candle.
    pub fn waras(&self) -> bool {
        [self.open, self.high, self.low, self.close, self.volume]
            .iter()
            .all(|v| v.is_finite())
            && self.high >= self.low
            && self.low > 0.0
            && self.volume >= 0.0
    }

    /// Panjang badan candle.
    pub fn badan(&self) -> f64 {
        (self.close - self.open).abs()
    }

    /// Panjang sumbu bawah.
    pub fn sumbu_bawah(&self) -> f64 {
        self.open.min(self.close) - self.low
    }

    /// Panjang sumbu atas.
    pub fn sumbu_atas(&self) -> f64 {
        self.high - self.open.max(self.close)
    }
}

/// Buang bar yang bentuknya tidak masuk akal, lalu urutkan menurut waktu.
///
/// Chart bisa mengirim bar ganda saat bar terakhir masih terbentuk. Yang lebih
/// baru menang, karena ia yang mencerminkan keadaan sekarang.
pub fn bersihkan(mut bars: Vec<Bar>) -> Vec<Bar> {
    bars.retain(|b| b.waras());
    bars.sort_by_key(|b| b.time);
    bars.dedup_by_key(|b| b.time);
    bars
}

/// Persentil sebuah deret, dengan interpolasi linear.
///
/// `p` dalam rentang 0 sampai 1. Deret kosong menghasilkan `None`, bukan nol,
/// karena nol adalah nilai yang sah dan tidak bisa dibedakan dari tidak ada.
pub fn persentil(nilai: &[f64], p: f64) -> Option<f64> {
    if nilai.is_empty() || !p.is_finite() {
        return None;
    }
    let mut urut: Vec<f64> = nilai.iter().copied().filter(|v| v.is_finite()).collect();
    if urut.is_empty() {
        return None;
    }
    urut.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let p = p.clamp(0.0, 1.0);
    let posisi = p * (urut.len() - 1) as f64;
    let bawah = posisi.floor() as usize;
    let atas = posisi.ceil() as usize;
    if bawah == atas {
        return Some(urut[bawah]);
    }
    let bobot = posisi - bawah as f64;
    Some(urut[bawah] * (1.0 - bobot) + urut[atas] * bobot)
}

/// Median, yaitu persentil ke-50.
pub fn median(nilai: &[f64]) -> Option<f64> {
    persentil(nilai, 0.5)
}

/// Rata rata dan simpangan baku sampel.
///
/// Mengembalikan `None` kalau datanya kurang dari dua, karena simpangan baku
/// sampel tidak terdefinisi di bawah itu.
pub fn rata_dan_simpangan(nilai: &[f64]) -> Option<(f64, f64)> {
    let bersih: Vec<f64> = nilai.iter().copied().filter(|v| v.is_finite()).collect();
    if bersih.len() < 2 {
        return None;
    }
    let n = bersih.len() as f64;
    let rata = bersih.iter().sum::<f64>() / n;
    let ragam = bersih.iter().map(|v| (v - rata).powi(2)).sum::<f64>() / (n - 1.0);
    Some((rata, ragam.sqrt()))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn bar(t: i64, o: f64, h: f64, l: f64, c: f64, v: f64) -> Bar {
        Bar {
            time: t,
            open: o,
            high: h,
            low: l,
            close: c,
            volume: v,
        }
    }

    #[test]
    fn bar_dengan_high_di_bawah_low_dibuang() {
        // Data cacat dari chart lebih baik hilang daripada ikut dihitung.
        let hasil = bersihkan(vec![
            bar(1, 10.0, 12.0, 9.0, 11.0, 5.0),
            bar(2, 10.0, 8.0, 9.0, 11.0, 5.0),
        ]);
        assert_eq!(hasil.len(), 1);
        assert_eq!(hasil[0].time, 1);
    }

    #[test]
    fn harga_nol_atau_negatif_dibuang() {
        // Harga nol membuat setiap pembagian menghasilkan tak hingga, dan itu
        // menjalar ke seluruh metrik tanpa terlihat.
        let hasil = bersihkan(vec![bar(1, 0.0, 1.0, 0.0, 1.0, 5.0)]);
        assert!(hasil.is_empty());
    }

    #[test]
    fn bar_diurutkan_dan_yang_ganda_dibuang() {
        let hasil = bersihkan(vec![
            bar(3, 1.0, 2.0, 1.0, 1.5, 1.0),
            bar(1, 1.0, 2.0, 1.0, 1.5, 1.0),
            bar(3, 1.0, 2.0, 1.0, 1.9, 1.0),
        ]);
        assert_eq!(hasil.len(), 2);
        assert_eq!(hasil[0].time, 1);
        assert_eq!(hasil[1].time, 3);
    }

    #[test]
    fn persentil_deret_kosong_bukan_nol() {
        // Nol adalah nilai yang sah. Membedakannya dari tidak ada itu penting.
        assert_eq!(persentil(&[], 0.5), None);
    }

    #[test]
    fn persentil_menginterpolasi() {
        let v = [1.0, 2.0, 3.0, 4.0];
        assert_eq!(persentil(&v, 0.0), Some(1.0));
        assert_eq!(persentil(&v, 1.0), Some(4.0));
        // Posisi 0,5 dari tiga langkah adalah 1,5, jadi antara 2 dan 3.
        assert_eq!(persentil(&v, 0.5), Some(2.5));
    }

    #[test]
    fn median_deret_ganjil() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
    }

    #[test]
    fn simpangan_butuh_minimal_dua_titik() {
        assert_eq!(rata_dan_simpangan(&[5.0]), None);
        let (rata, sd) = rata_dan_simpangan(&[2.0, 4.0]).unwrap();
        assert!((rata - 3.0).abs() < 1e-9);
        assert!((sd - std::f64::consts::SQRT_2).abs() < 1e-9);
    }

    #[test]
    fn sumbu_dan_badan_dihitung_dari_open_close() {
        let b = bar(1, 10.0, 14.0, 6.0, 12.0, 1.0);
        assert!((b.badan() - 2.0).abs() < 1e-9);
        // Sumbu bawah diukur dari yang terendah antara open dan close.
        assert!((b.sumbu_bawah() - 4.0).abs() < 1e-9);
        assert!((b.sumbu_atas() - 2.0).abs() < 1e-9);
    }

    #[test]
    fn nilai_tak_hingga_diabaikan_persentil() {
        assert_eq!(persentil(&[1.0, f64::NAN, 3.0], 0.5), Some(2.0));
    }
}
