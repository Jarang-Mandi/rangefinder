//! Metrik dasar yang dipakai seluruh detektor.
//!
//! Semuanya murni: masuk deret bar, keluar angka. Tidak ada yang menyentuh
//! jaringan, dan tidak ada yang menyimpan keadaan. Itu yang membuat bagian
//! paling mudah salah di proyek ini bisa diuji tanpa rantai.
//!
//! INVARIAN
//! - Jendela yang lebih panjang daripada data yang tersedia menghasilkan `None`,
//!   bukan hasil dari data yang dipendekkan diam diam. Menghitung Efficiency
//!   Ratio atas sepuluh bar lalu menyebutnya empat jam adalah kebohongan yang
//!   tidak akan pernah terlihat di layar.
//! - Pembagian yang penyebutnya nol menghasilkan `None`, bukan nol. Nol di sini
//!   berarti "harga tidak bergerak sama sekali", dan itu keadaan yang sangat
//!   berbeda dari "tidak bisa dihitung".

use super::bar::{rata_dan_simpangan, Bar};

/// Efficiency Ratio Kaufman atas jendela `w` bar terakhir.
///
/// Mendekati nol berarti harga banyak bergerak tapi tidak ke mana mana, yaitu
/// kondisi menyamping yang dicari. Mendekati satu berarti tren bersih.
pub fn efficiency_ratio(bars: &[Bar], w: usize) -> Option<f64> {
    if w < 2 || bars.len() < w {
        return None;
    }
    let jendela = &bars[bars.len() - w..];
    let arah = (jendela[w - 1].close - jendela[0].close).abs();
    let jalan: f64 = jendela
        .windows(2)
        .map(|p| (p[1].close - p[0].close).abs())
        .sum();

    if jalan <= 0.0 {
        // Harga sama sekali tidak bergerak. Itu bukan tren maupun chop, dan
        // memberinya nilai nol akan membuatnya terlihat seperti chop sempurna.
        return None;
    }
    Some((arah / jalan).clamp(0.0, 1.0))
}

/// Volatilitas realisasi Parkinson, memakai high dan low.
///
/// Lebih efisien daripada simpangan baku close karena memakai rentang penuh
/// tiap bar, bukan hanya titik penutupnya.
pub fn parkinson(bars: &[Bar], w: usize) -> Option<f64> {
    if w == 0 || bars.len() < w {
        return None;
    }
    let jendela = &bars[bars.len() - w..];
    let jumlah: f64 = jendela
        .iter()
        .map(|b| (b.high / b.low).ln().powi(2))
        .sum();
    let pembagi = 4.0 * w as f64 * std::f64::consts::LN_2;
    Some((jumlah / pembagi).sqrt())
}

/// Average True Range atas `w` bar terakhir.
///
/// True range memakai close bar sebelumnya, sehingga celah antar bar ikut
/// terhitung. Mengabaikannya akan meremehkan volatilitas persis pada saat
/// harga melompat, yaitu saat ukuran itu paling dibutuhkan.
pub fn atr(bars: &[Bar], w: usize) -> Option<f64> {
    if w == 0 || bars.len() < w + 1 {
        return None;
    }
    let mulai = bars.len() - w;
    let mut jumlah = 0.0;
    for i in mulai..bars.len() {
        let b = &bars[i];
        let sebelum = bars[i - 1].close;
        let tr = (b.high - b.low)
            .max((b.high - sebelum).abs())
            .max((b.low - sebelum).abs());
        jumlah += tr;
    }
    Some(jumlah / w as f64)
}

/// Hasil pengukuran kanal harga.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Kanal {
    /// Batas bawah, persentil 10 dari low.
    pub bawah: f64,
    /// Batas atas, persentil 90 dari high.
    pub atas: f64,
    /// Proporsi bar yang close-nya berada di dalam kanal.
    pub containment: f64,
    /// Lebar kanal relatif terhadap titik tengahnya.
    pub bandwidth: f64,
}

/// Bentuk kanal dari persentil low dan high, lalu ukur seberapa terkurung.
pub fn kanal(bars: &[Bar], w: usize) -> Option<Kanal> {
    if w == 0 || bars.len() < w {
        return None;
    }
    let jendela = &bars[bars.len() - w..];
    let lows: Vec<f64> = jendela.iter().map(|b| b.low).collect();
    let highs: Vec<f64> = jendela.iter().map(|b| b.high).collect();

    let bawah = super::bar::persentil(&lows, 0.10)?;
    let atas = super::bar::persentil(&highs, 0.90)?;
    if atas <= bawah {
        return None;
    }

    let di_dalam = jendela
        .iter()
        .filter(|b| b.close >= bawah && b.close <= atas)
        .count();
    let tengah = (atas + bawah) / 2.0;
    if tengah <= 0.0 {
        return None;
    }

    Some(Kanal {
        bawah,
        atas,
        containment: di_dalam as f64 / w as f64,
        bandwidth: (atas - bawah) / tengah,
    })
}

/// Z score volume bar terakhir terhadap `w` bar sebelumnya.
///
/// Bar terakhir TIDAK ikut dalam rata rata pembandingnya. Memasukkannya membuat
/// lonjakan besar menaikkan rata ratanya sendiri, sehingga skornya lebih kecil
/// daripada yang seharusnya, persis pada kejadian yang paling ingin ditangkap.
pub fn volume_z(bars: &[Bar], w: usize) -> Option<f64> {
    if w < 2 || bars.len() < w + 1 {
        return None;
    }
    let terakhir = bars[bars.len() - 1].volume;
    let pembanding: Vec<f64> = bars[bars.len() - 1 - w..bars.len() - 1]
        .iter()
        .map(|b| b.volume)
        .collect();

    let (rata, sd) = rata_dan_simpangan(&pembanding)?;
    if sd <= 0.0 {
        // Volume yang sama persis sepanjang jendela berarti tidak ada sebaran
        // untuk dibandingkan. Angka apa pun di sini akan mengada ada.
        return None;
    }
    Some((terakhir - rata) / sd)
}

/// Keseimbangan tape, diperkirakan dari arah tiap bar.
///
/// Kita tidak punya data buy dan sell terpisah dari chart, jadi volume bar naik
/// dianggap beli dan bar turun dianggap jual. Ini perkiraan, dan disebut
/// perkiraan: pada satu bar ia bisa jauh meleset, tapi atas jendela panjang ia
/// cukup untuk membedakan tape yang seimbang dari yang berat sebelah.
///
/// Mendekati satu berarti seimbang, yaitu ciri chop. Mendekati nol berarti berat
/// sebelah, yaitu ciri tren yang sedang berjalan.
pub fn keseimbangan_tape(bars: &[Bar], w: usize) -> Option<f64> {
    if w == 0 || bars.len() < w {
        return None;
    }
    let jendela = &bars[bars.len() - w..];
    let mut beli = 0.0;
    let mut jual = 0.0;
    for b in jendela {
        // Bar yang tutup sama dengan bukanya tidak membawa arah sama sekali.
        // Menganggapnya beli membuat deret yang penuh bar seperti itu
        // terbaca berat sebelah, padahal ia justru contoh paling seimbang.
        if b.close > b.open {
            beli += b.volume;
        } else if b.close < b.open {
            jual += b.volume;
        }
    }
    let total = beli + jual;
    if total <= 0.0 {
        return None;
    }
    Some(1.0 - (beli - jual).abs() / total)
}

/// Kemiringan regresi linear atas sederet nilai.
///
/// Dipakai penjaga dead cat: basis yang mirinya turun bukan basis, melainkan
/// pendarahan lambat yang akan terus mencetak low baru.
pub fn kemiringan(nilai: &[f64]) -> Option<f64> {
    let n = nilai.len();
    if n < 2 {
        return None;
    }
    let n_f = n as f64;
    let rata_x = (n_f - 1.0) / 2.0;
    let rata_y = nilai.iter().sum::<f64>() / n_f;

    let mut atas = 0.0;
    let mut bawah = 0.0;
    for (i, y) in nilai.iter().enumerate() {
        let dx = i as f64 - rata_x;
        atas += dx * (y - rata_y);
        bawah += dx * dx;
    }
    if bawah <= 0.0 {
        return None;
    }
    Some(atas / bawah)
}

/// Harga dengan volume terbanyak pada sebuah jendela, dibagi ke dalam `kotak`
/// tingkatan harga.
///
/// Ini titik acuan penempatan yang paling berguna: harga yang paling banyak
/// diperdagangkan cenderung ditengok lagi.
pub fn point_of_control(bars: &[Bar], kotak: usize) -> Option<f64> {
    if bars.is_empty() || kotak == 0 {
        return None;
    }
    let terendah = bars.iter().map(|b| b.low).fold(f64::INFINITY, f64::min);
    let tertinggi = bars.iter().map(|b| b.high).fold(f64::NEG_INFINITY, f64::max);
    if !(terendah.is_finite() && tertinggi.is_finite()) || tertinggi <= terendah {
        return None;
    }

    let lebar = (tertinggi - terendah) / kotak as f64;
    let mut ember = vec![0.0f64; kotak];
    for b in bars {
        // Volume ditaruh di tingkatan harga tengah bar. Membaginya rata ke
        // seluruh rentang bar lebih tepat, tapi jauh lebih mahal, dan untuk
        // menempatkan garis perbedaannya tidak berarti.
        let tengah = (b.high + b.low) / 2.0;
        let idx = (((tengah - terendah) / lebar) as usize).min(kotak - 1);
        ember[idx] += b.volume;
    }

    let (idx, _) = ember
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))?;
    Some(terendah + lebar * (idx as f64 + 0.5))
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

    /// Deret naik lurus. Efficiency Ratio-nya harus satu.
    fn tren_naik(n: usize) -> Vec<Bar> {
        (0..n)
            .map(|i| {
                let p = 100.0 + i as f64;
                bar(i as i64, p, p + 0.5, p - 0.5, p + 1.0, 10.0)
            })
            .collect()
    }

    /// Deret bolak balik di sekitar satu harga. Efficiency Ratio-nya mendekati nol.
    fn menyamping(n: usize) -> Vec<Bar> {
        (0..n)
            .map(|i| {
                // Bergantian naik dan turun di sekitar satu harga. Close
                // sengaja berbeda dari open supaya arah tiap bar nyata.
                let (o, c) = if i % 2 == 0 {
                    (100.5, 100.0)
                } else {
                    (100.0, 100.5)
                };
                bar(i as i64, o, 101.0, 99.5, c, 10.0)
            })
            .collect()
    }

    #[test]
    fn efficiency_ratio_tren_mendekati_satu() {
        let er = efficiency_ratio(&tren_naik(50), 40).unwrap();
        assert!(er > 0.9, "tren bersih seharusnya mendekati satu, dapat {er}");
    }

    #[test]
    fn efficiency_ratio_menyamping_mendekati_nol() {
        let er = efficiency_ratio(&menyamping(50), 40).unwrap();
        assert!(er < 0.2, "chop seharusnya mendekati nol, dapat {er}");
    }

    #[test]
    fn jendela_lebih_panjang_dari_data_mengembalikan_none() {
        // Menghitung atas data yang dipendekkan lalu menyebutnya empat jam
        // adalah kebohongan yang tidak akan terlihat di layar.
        assert_eq!(efficiency_ratio(&tren_naik(10), 240), None);
        assert_eq!(parkinson(&tren_naik(10), 240), None);
        assert_eq!(atr(&tren_naik(10), 240), None);
        assert_eq!(kanal(&tren_naik(10), 240), None);
    }

    #[test]
    fn harga_diam_total_bukan_chop_sempurna() {
        // Nol pada Efficiency Ratio berarti chop sempurna, dan harga yang sama
        // sekali tidak bergerak bukan itu. Ia berarti datanya tidak berguna.
        let diam: Vec<Bar> = (0..20).map(|i| bar(i, 100.0, 100.0, 100.0, 100.0, 1.0)).collect();
        assert_eq!(efficiency_ratio(&diam, 10), None);
    }

    #[test]
    fn atr_menghitung_celah_antar_bar() {
        // Bar kedua melompat jauh di atas close bar pertama. True range harus
        // mengukur dari close sebelumnya, bukan hanya rentang bar itu sendiri.
        let bars = vec![
            bar(1, 100.0, 101.0, 99.0, 100.0, 1.0),
            bar(2, 110.0, 111.0, 109.0, 110.0, 1.0),
        ];
        let hasil = atr(&bars, 1).unwrap();
        assert!(
            (hasil - 11.0).abs() < 1e-9,
            "celah dari 100 ke 111 harus terhitung, dapat {hasil}"
        );
    }

    #[test]
    fn kanal_menyamping_sangat_terkurung() {
        let k = kanal(&menyamping(60), 50).unwrap();
        assert!(k.containment > 0.9, "chop harus terkurung, dapat {}", k.containment);
        assert!(k.bandwidth < 0.05);
    }

    #[test]
    fn volume_z_tidak_memasukkan_bar_terakhir_ke_pembanding() {
        // Kalau bar terakhir ikut menaikkan rata ratanya sendiri, lonjakan besar
        // akan terlihat lebih kecil daripada yang sebenarnya.
        let mut bars: Vec<Bar> = (0..30).map(|i| bar(i, 100.0, 101.0, 99.0, 100.0, 10.0)).collect();
        bars.push(bar(30, 100.0, 101.0, 99.0, 100.0, 100.0));
        // Pembandingnya seragam, jadi simpangannya nol dan hasilnya None.
        assert_eq!(volume_z(&bars, 20), None);

        // Dengan sedikit sebaran, lonjakan harus terbaca sangat besar.
        let mut bars2: Vec<Bar> = (0..30)
            .map(|i| bar(i, 100.0, 101.0, 99.0, 100.0, 10.0 + (i % 3) as f64))
            .collect();
        bars2.push(bar(30, 100.0, 101.0, 99.0, 100.0, 100.0));
        let z = volume_z(&bars2, 20).unwrap();
        assert!(z > 50.0, "lonjakan sepuluh kali harus terbaca besar, dapat {z}");
    }

    #[test]
    fn keseimbangan_tape_seimbang_mendekati_satu() {
        let b = keseimbangan_tape(&menyamping(40), 40).unwrap();
        assert!(b > 0.8, "chop seharusnya seimbang, dapat {b}");
    }

    #[test]
    fn keseimbangan_tape_tren_berat_sebelah() {
        let b = keseimbangan_tape(&tren_naik(40), 40).unwrap();
        assert!(b < 0.2, "tren naik seharusnya berat sebelah, dapat {b}");
    }

    #[test]
    fn kemiringan_naik_positif_turun_negatif() {
        assert!(kemiringan(&[1.0, 2.0, 3.0, 4.0]).unwrap() > 0.0);
        assert!(kemiringan(&[4.0, 3.0, 2.0, 1.0]).unwrap() < 0.0);
        assert!(kemiringan(&[2.0, 2.0, 2.0]).unwrap().abs() < 1e-9);
    }

    #[test]
    fn point_of_control_menemukan_harga_paling_ramai() {
        let bars = vec![
            bar(1, 100.0, 101.0, 99.0, 100.0, 1.0),
            bar(2, 110.0, 111.0, 109.0, 110.0, 100.0),
            bar(3, 120.0, 121.0, 119.0, 120.0, 1.0),
        ];
        let poc = point_of_control(&bars, 10).unwrap();
        assert!(
            (poc - 110.0).abs() < 3.0,
            "POC harus dekat 110 tempat volumenya menumpuk, dapat {poc}"
        );
    }

    #[test]
    fn point_of_control_harga_datar_mengembalikan_none() {
        let bars = vec![bar(1, 100.0, 100.0, 100.0, 100.0, 5.0)];
        assert_eq!(point_of_control(&bars, 10), None);
    }
}
