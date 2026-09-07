//! Akuntansi hasil posisi LP.
//!
//! KENAPA FEE KOTOR MENIPU
//! Posisi yang mengumpulkan fee besar bisa tetap rugi. Impermanent loss, gas
//! buka, gas tutup, dan slippage zap semuanya mengurangi, dan tidak satu pun
//! terlihat kalau yang ditampilkan hanya fee. PRD 11 menyebutnya eksplisit:
//! kesalahan itu akan terulang karena tidak pernah terlihat.
//!
//! Karena itu modul ini selalu memisahkan komponennya, dan `net` selalu
//! dihitung dari selisihnya, tidak pernah disimpan terpisah.
//!
//! SATUAN
//! Seluruh perhitungan dilakukan dalam satuan token kutipan, yaitu token1 pada
//! pool yang kutipannya stablecoin. Alasannya, itu satuan yang paling stabil
//! yang tersedia tanpa sumber harga luar. Nilai token0 dinyatakan dalam token1
//! lewat harga pool.
//!
//! INVARIAN
//! - Impermanent loss dihitung terhadap MEMEGANG, bukan terhadap modal awal.
//!   Selisih terhadap modal awal adalah perubahan harga, bukan kerugian akibat
//!   menjadi LP, dan mencampurnya membuat strategi yang bagus terlihat buruk
//!   saat pasar turun.
//! - Catatan yang tidak lengkap ditandai sebagai perkiraan, bukan disembunyikan.
//!   Posisi yang dibuka di luar Rangefinder tetap harus punya angka.

use serde::{Deserialize, Serialize};

/// Catatan biaya sebuah posisi, disimpan saat posisi dibuka.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Catatan {
    pub token_id: String,
    /// Jumlah token0 dan token1 yang benar benar disetor.
    pub setor0: String,
    pub setor1: String,
    /// Harga token1 per token0 saat posisi dibuka, sebagai pecahan.
    pub harga_buka: f64,
    /// Biaya gas yang sudah dibayar, dalam satuan token kutipan.
    pub gas_kutipan: f64,
    /// Kerugian slippage zap, dalam satuan token kutipan.
    pub slippage_kutipan: f64,
    pub dibuka_pada: String,
    pub tx_buka: Option<String>,
}

/// Komponen hasil sebuah posisi, semuanya dalam satuan token kutipan.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hasil {
    /// Fee yang diperoleh, yang sudah diklaim maupun belum.
    pub fee: f64,
    /// Kerugian akibat menjadi LP dibandingkan sekadar memegang.
    ///
    /// Positif berarti rugi. Ditampilkan sebagai pengurang, bukan sebagai nilai
    /// negatif, supaya kartunya terbaca sebagai daftar biaya.
    pub impermanent_loss: f64,
    pub gas: f64,
    pub slippage: f64,
    /// Fee dikurangi seluruh biaya. Selalu dihitung, tidak pernah disimpan.
    pub net: f64,
    /// Benar kalau sebagian angkanya direkonstruksi, bukan dari catatan.
    pub perkiraan: bool,
}

/// Nilai sekumpulan token dalam satuan token kutipan.
///
/// `harga` adalah token1 per token0.
pub fn nilai(amount0: f64, amount1: f64, harga: f64) -> f64 {
    amount0 * harga + amount1
}

/// Hitung impermanent loss terhadap memegang.
///
/// Keduanya dinilai pada harga SEKARANG. Itu yang membuatnya benar benar
/// mengukur akibat menjadi LP: kalau harganya sama sekali tidak bergerak,
/// hasilnya nol berapa pun harganya.
pub fn impermanent_loss(
    setor0: f64,
    setor1: f64,
    posisi0: f64,
    posisi1: f64,
    harga_sekarang: f64,
) -> f64 {
    let memegang = nilai(setor0, setor1, harga_sekarang);
    let sebagai_lp = nilai(posisi0, posisi1, harga_sekarang);
    memegang - sebagai_lp
}

/// Rakit seluruh komponen menjadi satu hasil.
#[allow(clippy::too_many_arguments)]
pub fn hitung(
    setor0: f64,
    setor1: f64,
    posisi0: f64,
    posisi1: f64,
    fee0: f64,
    fee1: f64,
    harga_sekarang: f64,
    gas: f64,
    slippage: f64,
    perkiraan: bool,
) -> Hasil {
    let fee = nilai(fee0, fee1, harga_sekarang);
    let il = impermanent_loss(setor0, setor1, posisi0, posisi1, harga_sekarang);
    Hasil {
        fee,
        impermanent_loss: il,
        gas,
        slippage,
        net: fee - il - gas - slippage,
        perkiraan,
    }
}

/// Ringkasan sekumpulan posisi.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Ringkasan {
    pub jumlah: usize,
    pub total_net: f64,
    pub total_fee: f64,
    pub total_biaya: f64,
    /// Porsi posisi yang net-nya positif, dalam basis poin.
    pub win_rate_bps: u32,
    /// Berapa yang sebagian angkanya direkonstruksi.
    pub perkiraan: usize,
}

pub fn ringkas(hasil: &[Hasil]) -> Ringkasan {
    if hasil.is_empty() {
        return Ringkasan {
            jumlah: 0,
            total_net: 0.0,
            total_fee: 0.0,
            total_biaya: 0.0,
            win_rate_bps: 0,
            perkiraan: 0,
        };
    }

    let total_net: f64 = hasil.iter().map(|h| h.net).sum();
    let total_fee: f64 = hasil.iter().map(|h| h.fee).sum();
    let total_biaya: f64 = hasil
        .iter()
        .map(|h| h.impermanent_loss + h.gas + h.slippage)
        .sum();
    let menang = hasil.iter().filter(|h| h.net > 0.0).count();

    Ringkasan {
        jumlah: hasil.len(),
        total_net,
        total_fee,
        total_biaya,
        win_rate_bps: ((menang as f64 / hasil.len() as f64) * 10_000.0) as u32,
        perkiraan: hasil.iter().filter(|h| h.perkiraan).count(),
    }
}

fn kunci_catatan(token_id: &str) -> String {
    format!("pnl:v1:{}", token_id.to_lowercase())
}

/// Simpan catatan biaya sebuah posisi.
pub fn simpan(store: &crate::store::Store, c: &Catatan) {
    if let Err(e) = store.put(&kunci_catatan(&c.token_id), c) {
        // Catatan yang gagal disimpan hanya membuat PnL-nya jadi perkiraan
        // nanti. Itu tidak sepadan dengan menggagalkan pembukaan posisi.
        tracing::warn!(token_id = %c.token_id, error = %e, "catatan biaya gagal disimpan");
    }
}

/// Ambil catatan biaya kalau ada.
pub fn ambil(store: &crate::store::Store, token_id: &str) -> Option<Catatan> {
    store.get(&kunci_catatan(token_id)).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hampir(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn nilai_menjumlahkan_kedua_sisi() {
        // Dua token0 pada harga tiga, ditambah satu token1, sama dengan tujuh.
        assert!(hampir(nilai(2.0, 1.0, 3.0), 7.0));
    }

    #[test]
    fn harga_tidak_bergerak_berarti_tanpa_impermanent_loss() {
        // INI DEFINISI YANG BENAR.
        //
        // Impermanent loss mengukur akibat menjadi LP, bukan akibat harga
        // bergerak. Kalau posisinya masih memegang persis yang disetor, hasilnya
        // nol berapa pun harganya.
        for harga in [0.5, 1.0, 2.0, 100.0] {
            assert!(hampir(impermanent_loss(10.0, 20.0, 10.0, 20.0, harga), 0.0));
        }
    }

    #[test]
    fn komposisi_bergeser_menimbulkan_kerugian() {
        // Harga token0 naik, jadi posisi menjual sebagian token0. Dibandingkan
        // memegang, nilainya lebih rendah.
        //
        // Setor 10 token0 dan 0 token1. Harga naik jadi 2. Posisi sekarang
        // memegang 5 token0 dan 10 token1, yang nilainya 5*2+10 = 20.
        // Memegang saja bernilai 10*2 = 20. Sama, jadi belum ada kerugian.
        assert!(hampir(impermanent_loss(10.0, 0.0, 5.0, 10.0, 2.0), 0.0));

        // Kalau posisi hanya memegang 5 token0 dan 8 token1, nilainya 18,
        // sedangkan memegang bernilai 20. Kerugiannya 2.
        assert!(hampir(impermanent_loss(10.0, 0.0, 5.0, 8.0, 2.0), 2.0));
    }

    #[test]
    fn net_adalah_fee_dikurangi_seluruh_biaya() {
        let h = hitung(10.0, 0.0, 5.0, 8.0, 0.0, 5.0, 2.0, 0.5, 0.3, false);
        assert!(hampir(h.fee, 5.0));
        assert!(hampir(h.impermanent_loss, 2.0));
        assert!(hampir(h.net, 5.0 - 2.0 - 0.5 - 0.3));
    }

    #[test]
    fn fee_besar_tetap_bisa_rugi() {
        // Ini kasus yang paling penting ditampilkan. Fee lima terlihat bagus,
        // tapi impermanent loss enam membuatnya rugi. Tanpa pemisahan komponen,
        // user hanya melihat angka fee dan mengira untung.
        let h = hitung(10.0, 0.0, 4.0, 6.0, 0.0, 5.0, 2.0, 0.2, 0.0, false);
        assert!(h.fee > 0.0);
        assert!(h.net < 0.0, "net {} seharusnya negatif", h.net);
    }

    #[test]
    fn fee_dinilai_pada_harga_sekarang() {
        // Fee yang terkumpul dalam token0 harus dikonversi, kalau tidak posisi
        // yang fee-nya di sisi token0 akan terlihat jauh lebih kecil.
        let h = hitung(0.0, 0.0, 0.0, 0.0, 3.0, 0.0, 4.0, 0.0, 0.0, false);
        assert!(hampir(h.fee, 12.0));
    }

    #[test]
    fn ringkasan_kosong_tidak_membagi_dengan_nol() {
        let r = ringkas(&[]);
        assert_eq!(r.jumlah, 0);
        assert_eq!(r.win_rate_bps, 0);
    }

    #[test]
    fn win_rate_dihitung_dari_net_bukan_fee() {
        // Posisi yang fee-nya besar tapi net-nya negatif bukan kemenangan.
        let menang = hitung(0.0, 0.0, 0.0, 0.0, 0.0, 10.0, 1.0, 0.0, 0.0, false);
        let kalah = hitung(10.0, 0.0, 4.0, 6.0, 0.0, 5.0, 2.0, 0.2, 0.0, false);
        let r = ringkas(&[menang, kalah]);
        assert_eq!(r.win_rate_bps, 5_000, "satu dari dua menang");
    }

    #[test]
    fn ringkasan_menghitung_yang_perkiraan() {
        let a = hitung(0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, true);
        let b = hitung(0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, false);
        assert_eq!(ringkas(&[a, b]).perkiraan, 1);
    }

    #[test]
    fn total_biaya_menjumlahkan_ketiganya() {
        let h = hitung(10.0, 0.0, 5.0, 8.0, 0.0, 0.0, 2.0, 0.5, 0.3, false);
        let r = ringkas(&[h]);
        assert!(hampir(r.total_biaya, 2.0 + 0.5 + 0.3));
    }

    #[test]
    fn kunci_catatan_tidak_peduli_besar_kecil_huruf() {
        assert_eq!(kunci_catatan("0xABC"), kunci_catatan("0xabc"));
    }
}
