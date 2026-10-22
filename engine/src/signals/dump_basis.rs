//! Detektor dump lalu basis, PRD 5.2b.
//!
//! KENAPA TERPISAH DARI 5.2
//! Detektor sideways mengukur kondisi menyamping yang SUDAH berjalan. Detektor
//! ini menangkap saat menyamping BARU SAJA terbentuk setelah penurunan tajam,
//! dan itu titik masuk yang paling bernilai: penjual panik sudah habis, harga
//! membangun basis, tapi volumenya masih ramai sehingga fee tetap mengalir.
//!
//! Pola ini juga yang paling sering ditinggalkan trader spot, karena secara
//! tampilan terlihat seperti koin yang sudah selesai.
//!
//! INVARIAN
//! - Penjaga dead cat adalah bagian terpenting. Tanpa syarat kemiringan basis
//!   yang tidak menurun, detektor ini akan terus menyarankan LP di tengah tren
//!   turun, dan itu cara tercepat kehilangan modal.
//! - Setiap syarat yang gagal dilaporkan beserta angkanya. Detektor yang hanya
//!   bilang tidak cocok tidak bisa dipercaya maupun diperbaiki.

use serde::{Deserialize, Serialize};

use super::bar::{median, Bar};
use super::metrik;

/// Ambang tiap syarat. Titik awal, dimaksudkan untuk dikalibrasi.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Aturan {
    /// Kedalaman penurunan minimum dari puncak ke dasar.
    pub min_drawdown: f64,
    /// Berapa bar paling lama dari puncak ke dasar, supaya benar benar dump.
    pub maks_bar_turun: usize,
    /// Berapa bar minimum sejak dasar terbentuk.
    pub min_usia_basis: usize,
    /// Seberapa dalam low baru masih ditoleransi, sebagai porsi dari dasar.
    pub toleransi_low_baru: f64,
    /// Rasio median volume basis terhadap volume sebelum pump.
    pub min_rasio_volume_hidup: f64,
    /// Rentang rasio median volume basis terhadap volume puncak dump.
    pub rasio_volume_mereda: (f64, f64),
    /// Rasio ATR basis terhadap ATR jendela dump.
    pub maks_kontraksi_atr: f64,
    /// Efficiency ratio maksimum pada jendela basis.
    pub maks_er_basis: f64,
    /// Keseimbangan tape minimum pada jendela basis.
    pub min_tape_basis: f64,
}

impl Default for Aturan {
    fn default() -> Self {
        Self {
            min_drawdown: 0.40,
            maks_bar_turun: 60,
            min_usia_basis: 30,
            toleransi_low_baru: 0.98,
            min_rasio_volume_hidup: 2.0,
            rasio_volume_mereda: (0.05, 0.60),
            maks_kontraksi_atr: 0.50,
            maks_er_basis: 0.35,
            min_tape_basis: 0.60,
        }
    }
}

/// Jangkar harga yang langsung dipakai doktrin penempatan.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Jangkar {
    pub l_basis: f64,
    pub h_basis: f64,
    pub poc: f64,
    /// Level pembatalan: di bawah ini, anggapan basisnya salah.
    pub p_invalid: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hasil {
    pub skor: f64,
    pub drawdown: f64,
    pub bar_turun: usize,
    pub usia_basis: usize,
    pub kemiringan_basis: f64,
    pub rasio_volume_hidup: f64,
    pub rasio_volume_mereda: f64,
    pub kontraksi_atr: f64,
    pub er_basis: f64,
    pub tape_basis: f64,
    /// Kosong berarti seluruh syarat terpenuhi.
    pub gagal: Vec<String>,
    pub jangkar: Jangkar,
    /// Sinyal penguat yang ditemukan. Menaikkan peringkat, tidak wajib.
    pub penguat: Vec<String>,
}

impl Hasil {
    pub fn lolos(&self) -> bool {
        self.gagal.is_empty()
    }
}

/// Cari pola dump lalu basis pada deret bar.
///
/// Mengembalikan `None` kalau datanya belum cukup, atau kalau tidak ada puncak
/// dan dasar yang bisa dikenali sama sekali.
pub fn cari(bars: &[Bar], a: &Aturan) -> Option<Hasil> {
    // Butuh cukup bar untuk puncak, penurunan, dan basis sekaligus.
    let minimum = a.maks_bar_turun + a.min_usia_basis + 10;
    if bars.len() < minimum {
        return None;
    }

    // Dasar adalah low terendah pada bagian yang cukup baru untuk masih relevan.
    let jangkauan = a.maks_bar_turun + a.min_usia_basis * 3;
    let awal_dasar = bars.len().saturating_sub(jangkauan);
    let (idx_dasar, p_dasar) = bars[awal_dasar..]
        .iter()
        .enumerate()
        .min_by(|x, y| {
            x.1.low
                .partial_cmp(&y.1.low)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, b)| (awal_dasar + i, b.low))?;

    let usia_basis = bars.len() - 1 - idx_dasar;
    if idx_dasar == 0 {
        // Tidak ada satu pun bar sebelum dasar, jadi tidak ada puncak yang
        // bisa dibandingkan. Ini benar benar tidak bisa dinilai.
        return None;
    }

    // Dasar yang jatuh di bar terakhir berarti harga masih mencetak low
    // baru, dan basisnya belum terbentuk. Itu TIDAK dikembalikan sebagai
    // None: keadaan itu bisa dinilai, jawabannya belum layak, dan user
    // perlu tahu alasannya. None berarti belum bisa diperiksa, bukan belum
    // layak, dan menyamakan keduanya menyembunyikan token yang sedang jatuh.

    // Puncak adalah high tertinggi SEBELUM dasar, dalam jangkauan yang wajar.
    let awal_cari = idx_dasar.saturating_sub(a.maks_bar_turun * 3);
    let (idx_puncak, p_puncak) = bars[awal_cari..idx_dasar]
        .iter()
        .enumerate()
        .max_by(|x, y| {
            x.1.high
                .partial_cmp(&y.1.high)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, b)| (awal_cari + i, b.high))?;

    if p_puncak <= 0.0 {
        return None;
    }

    let drawdown = (p_puncak - p_dasar) / p_puncak;
    let bar_turun = idx_dasar - idx_puncak;
    let basis = &bars[idx_dasar..];
    let jendela_dump = &bars[idx_puncak..=idx_dasar];

    // ─── Ukuran ───────────────────────────────────────────────────
    let lows_basis: Vec<f64> = basis.iter().map(|b| b.low).collect();
    let kemiringan_basis = metrik::kemiringan(&lows_basis).unwrap_or(0.0);

    let vol_basis = median(&basis.iter().map(|b| b.volume).collect::<Vec<_>>()).unwrap_or(0.0);
    let vol_sebelum = median(
        &bars[awal_cari..idx_puncak]
            .iter()
            .map(|b| b.volume)
            .collect::<Vec<_>>(),
    )
    .unwrap_or(0.0);
    let vol_puncak = jendela_dump.iter().map(|b| b.volume).fold(0.0f64, f64::max);

    let rasio_volume_hidup = if vol_sebelum > 0.0 {
        vol_basis / vol_sebelum
    } else {
        f64::INFINITY
    };
    let rasio_volume_mereda = if vol_puncak > 0.0 {
        vol_basis / vol_puncak
    } else {
        0.0
    };

    let atr_basis = metrik::atr(basis, basis.len().saturating_sub(1).max(1)).unwrap_or(0.0);
    let atr_dump =
        metrik::atr(jendela_dump, jendela_dump.len().saturating_sub(1).max(1)).unwrap_or(0.0);
    let kontraksi_atr = if atr_dump > 0.0 {
        atr_basis / atr_dump
    } else {
        f64::INFINITY
    };

    // Basis yang terlalu pendek tidak punya metrik yang berarti. Nilai
    // penggantinya sengaja dipilih yang PALING BURUK, supaya syarat di bawah
    // menolaknya alih alih meloloskannya karena kebetulan.
    let er_basis = metrik::efficiency_ratio(basis, basis.len()).unwrap_or(1.0);
    let tape_basis = metrik::keseimbangan_tape(basis, basis.len()).unwrap_or(0.0);

    let low_terbaru = basis
        .iter()
        .rev()
        .take(30)
        .map(|b| b.low)
        .fold(f64::INFINITY, f64::min);

    // ─── Syarat ───────────────────────────────────────────────────
    let mut gagal = Vec::new();

    if drawdown < a.min_drawdown {
        gagal.push(format!(
            "penurunan hanya {:.0} persen, di bawah ambang {:.0} persen. Ini bukan dump",
            drawdown * 100.0,
            a.min_drawdown * 100.0
        ));
    }
    if bar_turun > a.maks_bar_turun {
        gagal.push(format!(
            "turun selama {bar_turun} bar, lebih lambat dari {}. Ini pendarahan, bukan dump",
            a.maks_bar_turun
        ));
    }
    if usia_basis < a.min_usia_basis {
        gagal.push(format!(
            "basis baru {usia_basis} bar, belum cukup dari {} untuk dipercaya",
            a.min_usia_basis
        ));
    }
    if low_terbaru < p_dasar * a.toleransi_low_baru {
        gagal.push(format!(
            "masih mencetak low baru di {low_terbaru:.6}, di bawah dasar {p_dasar:.6}"
        ));
    }
    // INI PENJAGA DEAD CAT YANG PALING PENTING.
    if kemiringan_basis < 0.0 {
        gagal.push(format!(
            "basis miring turun dengan kemiringan {kemiringan_basis:.8}. Ini pendarahan lambat, \
             dan menaruh LP di sini adalah cara tercepat kehilangan modal"
        ));
    }
    if rasio_volume_hidup < a.min_rasio_volume_hidup {
        gagal.push(format!(
            "volume basis hanya {rasio_volume_hidup:.2} kali sebelum pump, di bawah {:.2}. \
             Koinnya sudah mati dan basis tanpa volume tidak membayar fee",
            a.min_rasio_volume_hidup
        ));
    }
    let (mereda_min, mereda_maks) = a.rasio_volume_mereda;
    if rasio_volume_mereda < mereda_min || rasio_volume_mereda > mereda_maks {
        gagal.push(format!(
            "volume basis {rasio_volume_mereda:.2} kali puncak dump, di luar rentang \
             {mereda_min:.2} sampai {mereda_maks:.2}. Kepanikan belum selesai atau sudah terlalu sepi"
        ));
    }
    if kontraksi_atr > a.maks_kontraksi_atr {
        gagal.push(format!(
            "volatilitas belum mengendap, {kontraksi_atr:.2} di atas ambang {:.2}",
            a.maks_kontraksi_atr
        ));
    }
    if er_basis >= a.maks_er_basis {
        gagal.push(format!(
            "basis belum menyamping, efficiency ratio {er_basis:.2} di atas {:.2}",
            a.maks_er_basis
        ));
    }
    if tape_basis < a.min_tape_basis {
        gagal.push(format!(
            "tape masih berat sebelah, keseimbangan {tape_basis:.2} di bawah {:.2}",
            a.min_tape_basis
        ));
    }

    // ─── Sinyal penguat ───────────────────────────────────────────
    let mut penguat = Vec::new();
    let bar_dasar = &bars[idx_dasar];
    if bar_dasar.sumbu_bawah() > bar_dasar.badan() * 2.0 {
        penguat.push(
            "sumbu kapitulasi di bar dasar, jangkar batas bawah yang jauh lebih dapat dipercaya"
                .to_string(),
        );
    }
    if rasio_volume_mereda < 0.25 {
        penguat.push("volume jual sudah jauh mereda dibanding puncak dump".to_string());
    }
    if kemiringan_basis > 0.0 {
        penguat.push("basis sedikit menanjak, bukan sekadar mendatar".to_string());
    }

    // ─── Jangkar dan skor ─────────────────────────────────────────
    let h_basis = basis
        .iter()
        .map(|b| b.high)
        .fold(f64::NEG_INFINITY, f64::max);
    let poc = metrik::point_of_control(basis, 24).unwrap_or((p_dasar + h_basis) / 2.0);

    let jangkar = Jangkar {
        l_basis: p_dasar,
        h_basis,
        poc,
        // Di bawah ini, anggapan basisnya salah dan posisinya harus ditutup.
        p_invalid: p_dasar - atr_basis,
    };

    // Skor hanya berarti kalau seluruh syarat lolos, tapi tetap dihitung supaya
    // kandidat yang nyaris lolos bisa diurutkan saat ambangnya dikalibrasi.
    let skor = 100.0
        * (0.30 * (1.0 - er_basis).clamp(0.0, 1.0)
            + 0.25 * (rasio_volume_hidup / 4.0).clamp(0.0, 1.0)
            + 0.20 * tape_basis.clamp(0.0, 1.0)
            + 0.15 * (1.0 - kontraksi_atr).clamp(0.0, 1.0)
            + 0.10 * drawdown.clamp(0.0, 1.0));

    Some(Hasil {
        skor,
        drawdown,
        bar_turun,
        usia_basis,
        kemiringan_basis,
        rasio_volume_hidup,
        rasio_volume_mereda,
        kontraksi_atr,
        er_basis,
        tape_basis,
        gagal,
        jangkar,
        penguat,
    })
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

    /// Bentuk yang dicari: tenang, dump tajam, lalu basis mendatar yang ramai.
    fn dump_lalu_basis() -> Vec<Bar> {
        let mut v = Vec::new();
        let mut t = 0i64;

        // Fase tenang sebelum pump, volume rendah.
        for i in 0..80 {
            let p = 100.0 + (i % 3) as f64 * 0.1;
            v.push(bar(t, p, p + 0.2, p - 0.2, p, 5.0 + (i % 4) as f64));
            t += 60;
        }
        // Puncak.
        v.push(bar(t, 100.0, 200.0, 100.0, 195.0, 200.0));
        t += 60;

        // Dump tajam, 40 bar dari 195 ke sekitar 91.
        for i in 0..40 {
            let p = 195.0 - (i as f64 + 1.0) * 2.6;
            v.push(bar(t, p + 2.0, p + 2.5, p - 0.5, p, 150.0 - i as f64));
            t += 60;
        }
        // Basis mendatar sedikit menanjak, volume masih ramai tapi sudah mereda.
        for i in 0..60 {
            // Arah bar bergantian supaya tapenya terbaca seimbang. Bar yang
            // tutup sama dengan bukanya tidak membawa arah sama sekali.
            let dasar = 91.5 + (i as f64) * 0.004;
            let (o, c) = if i % 2 == 0 {
                (dasar + 0.2, dasar)
            } else {
                (dasar, dasar + 0.2)
            };
            v.push(bar(
                t,
                o,
                dasar + 0.35,
                dasar - 0.15,
                c,
                20.0 + (i % 5) as f64,
            ));
            t += 60;
        }
        v
    }

    /// Pendarahan lambat: turun terus tanpa basis. Ini yang wajib ditolak.
    fn pendarahan_lambat() -> Vec<Bar> {
        (0..250)
            .map(|i| {
                let p = 200.0 - i as f64 * 0.5;
                bar(
                    i as i64 * 60,
                    p,
                    p + 0.3,
                    p - 0.5,
                    p - 0.2,
                    20.0 + (i % 5) as f64,
                )
            })
            .collect()
    }

    #[test]
    fn data_kurang_mengembalikan_none() {
        let pendek: Vec<Bar> = (0..30)
            .map(|i| bar(i, 100.0, 101.0, 99.0, 100.0, 10.0))
            .collect();
        assert_eq!(cari(&pendek, &Aturan::default()), None);
    }

    #[test]
    fn pola_yang_dicari_dikenali() {
        let h = cari(&dump_lalu_basis(), &Aturan::default()).unwrap();
        assert!(
            h.lolos(),
            "pola dump lalu basis seharusnya lolos, gagal karena {:?}",
            h.gagal
        );
        assert!(h.drawdown > 0.4, "drawdown {}", h.drawdown);
        assert!(h.usia_basis >= 30, "usia basis {}", h.usia_basis);
    }

    #[test]
    fn pendarahan_lambat_ditolak() {
        // INI SYARAT PALING PENTING DI SELURUH MODUL.
        //
        // Tanpa penjaga ini, detektor akan terus menyarankan LP di tengah tren
        // turun, dan itu cara tercepat kehilangan modal.
        let h = cari(&pendarahan_lambat(), &Aturan::default())
            .expect("keadaan ini bisa dinilai, jawabannya belum layak");
        assert!(!h.lolos(), "pendarahan lambat wajib ditolak");
    }

    #[test]
    fn basis_miring_turun_ditolak_dengan_alasan_yang_jelas() {
        let mut bars = dump_lalu_basis();
        let n = bars.len();
        for (i, b) in bars[n - 60..].iter_mut().enumerate() {
            let turun = i as f64 * 0.05;
            b.open -= turun;
            b.high -= turun;
            b.low -= turun;
            b.close -= turun;
        }
        let h = cari(&bars, &Aturan::default()).unwrap();
        assert!(!h.lolos());
        // Basis yang menurun terus mencetak low baru, sehingga dasarnya selalu
        // bar terakhir dan basisnya tidak pernah berumur. Penolakannya bisa
        // datang dari beberapa syarat sekaligus, dan semuanya benar. Yang wajib
        // adalah alasannya menyebut basis, bukan menolak tanpa penjelasan.
        assert!(
            h.gagal.iter().any(|g| g.contains("basis")),
            "alasan harus menyebut basis, dapat {:?}",
            h.gagal
        );
    }

    #[test]
    fn basis_tanpa_volume_ditolak() {
        // Basis tanpa volume tidak membayar fee. Koinnya sudah mati.
        let mut bars = dump_lalu_basis();
        let n = bars.len();
        for b in bars[n - 60..].iter_mut() {
            b.volume = 0.5;
        }
        let h = cari(&bars, &Aturan::default()).unwrap();
        assert!(!h.lolos());
        assert!(
            h.gagal
                .iter()
                .any(|g| g.contains("sudah mati") || g.contains("terlalu sepi")),
            "dapat {:?}",
            h.gagal
        );
    }

    #[test]
    fn volume_masih_sekencang_puncak_ditolak() {
        // Volume yang masih sekencang puncak dump berarti penjualan belum
        // selesai, dan harga kemungkinan besar masih akan turun.
        let mut bars = dump_lalu_basis();
        let n = bars.len();
        for b in bars[n - 60..].iter_mut() {
            b.volume = 200.0;
        }
        let h = cari(&bars, &Aturan::default()).unwrap();
        assert!(!h.lolos());
    }

    #[test]
    fn jangkar_masuk_akal() {
        let h = cari(&dump_lalu_basis(), &Aturan::default()).unwrap();
        let j = h.jangkar;
        assert!(
            j.h_basis > j.l_basis,
            "batas atas harus di atas batas bawah"
        );
        assert!(
            j.poc >= j.l_basis && j.poc <= j.h_basis,
            "POC {} harus di dalam basis {} sampai {}",
            j.poc,
            j.l_basis,
            j.h_basis
        );
        assert!(
            j.p_invalid < j.l_basis,
            "level pembatalan harus di bawah dasar"
        );
    }

    #[test]
    fn setiap_syarat_yang_gagal_dilaporkan_dengan_angkanya() {
        // Detektor yang hanya bilang tidak cocok tidak bisa dipercaya maupun
        // diperbaiki.
        let h = cari(&pendarahan_lambat(), &Aturan::default()).unwrap();
        assert!(!h.gagal.is_empty());
        for g in &h.gagal {
            assert!(
                g.chars().any(|c| c.is_ascii_digit()),
                "alasan `{g}` tidak menyebut angka"
            );
        }
    }
}
