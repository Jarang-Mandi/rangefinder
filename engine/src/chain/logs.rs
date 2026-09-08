//! Pembacaan log yang bertahan terhadap dua penolakan RPC yang berbeda.
//!
//! DUA KEGAGALAN YANG MIRIP TAPI OBATNYA BERLAWANAN
//! RPC publik menolak permintaan log karena dua alasan, dan keduanya datang
//! sebagai error biasa yang hanya bisa dibedakan dari teksnya:
//!
//! | Penolakan | Artinya | Obatnya |
//! | --- | --- | --- |
//! | `logs matched by query exceeds limit` | jawabannya kebanyakan | persempit jendela |
//! | `429 Too Many Requests` | permintaannya kerapatan | tunggu, jangan persempit |
//!
//! Memperlakukan keduanya sama akan memperburuk salah satunya. Mempersempit
//! jendela saat dibatasi laju justru menambah jumlah permintaan, yaitu persis
//! hal yang sedang dikeluhkan RPC. Menunggu saat jawabannya kebanyakan tidak
//! mengubah apa pun, karena jawabannya akan tetap sama besar setelah menunggu.
//!
//! Modul ini pernah tersebar sebagai tiga salinan di `pools::scan`,
//! `pools::volume`, dan pemindaian `Initialize`. Salinan ketiganya sempat
//! berbeda perilaku, dan itulah kenapa ia disatukan.
//!
//! INVARIAN
//! - Jendela yang benar benar terpakai selalu dikembalikan bersama lognya.
//!   Hasil dari jendela yang menyempit tetap berguna, tapi hanya kalau
//!   pemakainya tahu jendelanya berapa. Volume tanpa jendela tidak berarti.
//! - Kegagalan yang bukan salah satu dari dua di atas dikembalikan apa adanya.
//!   Menelan error lain akan menyembunyikan masalah yang sesungguhnya sebagai
//!   "hasil kosong", dan hasil kosong berarti pool itu sepi, kesimpulan yang
//!   sangat berbeda dari tidak bisa membaca.

use std::time::Duration;

use alloy::providers::Provider;
use alloy::rpc::types::{Filter, Log};
use anyhow::{Context, Result};

/// Berapa kali menunggu saat dibatasi laju sebelum menyerah.
pub const SABAR_MAKSIMUM: u32 = 4;

/// Jeda saat dibatasi laju.
pub const JEDA_DIBATASI_MS: u64 = 800;

/// Jeda setelah mempersempit jendela.
///
/// Mempersempit berarti permintaan tambahan, dan permintaan yang datang
/// berturut turut tanpa jeda justru memicu batas laju.
pub const JEDA_PERSEMPIT_MS: u64 = 400;

/// Benar kalau RPC menolak karena hasilnya terlalu banyak.
///
/// Dikenali dari teks, karena lapisan transport tidak menyediakan jalan yang
/// lebih rapi. Kalau pengenalannya meleset, akibatnya hanya kembali ke
/// perilaku lama, yaitu gagal seketika.
pub fn terlalu_banyak(e: &impl std::fmt::Display) -> bool {
    let teks = e.to_string().to_lowercase();
    teks.contains("exceeds limit")
        || teks.contains("too many results")
        || teks.contains("query returned more than")
}

/// Berapa blok paling sedikit yang masih masuk akal untuk dipindai.
///
/// Endpoint yang membatasi di bawah ini tidak bisa dipakai alat ini sama
/// sekali: penemuan pool V4 menuntut pemindaian jutaan blok, dan memecahnya
/// menjadi potongan sepuluh blok berarti ratusan ribu permintaan.
pub const JENDELA_TIDAK_LAYAK: u64 = 100;

/// Benar kalau endpointnya membatasi lebar rentang log, bukan jumlah hasilnya.
///
/// KENAPA INI DIBEDAKAN
/// Batas jumlah hasil bisa diatasi dengan mempersempit jendela. Batas LEBAR
/// tidak: mempersempitnya sampai batas yang diizinkan menghasilkan ratusan
/// ribu permintaan, dan penyempitan otomatis akan terus mencoba sampai
/// menyerah dengan pesan yang tidak menjelaskan apa apa.
///
/// Alchemy paket gratis membatasi `eth_getLogs` pada sepuluh blok, sedangkan
/// RPC publik Robinhood Chain menerima tiga juta. Alat ini tidak bisa berjalan
/// di atas endpoint jenis pertama, dan itu harus dikatakan apa adanya.
pub fn lebar_dibatasi(e: &impl std::fmt::Display) -> bool {
    let teks = e.to_string().to_lowercase();
    teks.contains("block range")
        || teks.contains("range should work")
        || teks.contains("free tier")
}

/// Benar kalau kegagalannya karena laju dibatasi.
pub fn dibatasi_laju(e: &impl std::fmt::Display) -> bool {
    let teks = e.to_string().to_lowercase();
    teks.contains("429") || teks.contains("too many requests")
}

/// Log beserta jendela yang benar benar terpakai untuk mendapatkannya.
#[derive(Debug, Clone)]
pub struct LogTerjendela {
    pub logs: Vec<Log>,
    pub dari_blok: u64,
    pub sampai_blok: u64,
    /// Benar kalau jendelanya sempat dipersempit dari yang diminta.
    ///
    /// Pemakainya perlu tahu ini untuk memutuskan apakah hasilnya masih layak
    /// dipakai memproyeksikan sesuatu. Volume satu jam boleh diekstrapolasi ke
    /// setahun, volume dua menit tidak.
    pub dipersempit: bool,
}

/// Baca log pada jendela mundur dari `sampai`, menyempit sendiri bila perlu.
///
/// `bangun` menerima batas bawah dan batas atas, lalu mengembalikan filter yang
/// sudah lengkap. Ia dipanggil ulang setiap kali jendelanya berubah, sehingga
/// pemakainya tidak perlu tahu apa apa soal penyempitan.
pub async fn baca_mundur<P, F>(
    provider: &P,
    sampai: u64,
    lantai: u64,
    jendela_awal: u64,
    jendela_minimum: u64,
    bangun: F,
) -> Result<LogTerjendela>
where
    P: Provider,
    F: Fn(u64, u64) -> Filter,
{
    let mut jendela = jendela_awal;
    let mut sabar = SABAR_MAKSIMUM;

    loop {
        let dari = sampai.saturating_sub(jendela).max(lantai);

        match provider.get_logs(&bangun(dari, sampai)).await {
            Ok(logs) => {
                return Ok(LogTerjendela {
                    logs,
                    dari_blok: dari,
                    sampai_blok: sampai,
                    dipersempit: jendela < jendela_awal,
                })
            }
            // Batas lebar rentang tidak bisa diatasi dengan menyempit. Yang
            // benar adalah berhenti dan mengatakan endpointnya yang salah.
            Err(e) if lebar_dibatasi(&e) => {
                return Err(anyhow::Error::new(e).context(format!(
                    "endpoint RPC ini membatasi lebar rentang eth_getLogs, dan \
                     alat ini butuh pemindaian jutaan blok. Ganti ke endpoint \
                     yang mengizinkan rentang lebar, misalnya RPC publik \
                     Robinhood Chain. Gagal pada blok {dari} sampai {sampai}"
                )))
            }
            Err(e) if jendela > jendela_minimum && terlalu_banyak(&e) => {
                jendela = (jendela / 2).max(jendela_minimum);
                tracing::debug!(jendela, "jendela log dipersempit");
                tokio::time::sleep(Duration::from_millis(JEDA_PERSEMPIT_MS)).await;
            }
            Err(e) if sabar > 0 && dibatasi_laju(&e) => {
                sabar -= 1;
                tracing::debug!(sabar, "pembacaan log dibatasi laju, menunggu");
                tokio::time::sleep(Duration::from_millis(JEDA_DIBATASI_MS)).await;
            }
            Err(e) => {
                return Err(anyhow::Error::new(e).context(format!(
                    "pembacaan log gagal pada blok {dari} sampai {sampai}"
                )))
            }
        }
    }
}

/// Baca log pada rentang tetap, tanpa mempersempit.
///
/// Dipakai saat rentangnya sudah ditentukan pemanggil dan tidak boleh berubah,
/// misalnya pemindaian `Initialize` yang harus menutup seluruh sejarah. Batas
/// laju tetap ditunggu, karena menunggu tidak mengubah arti hasilnya.
pub async fn baca_rentang<P: Provider>(
    provider: &P,
    filter: &Filter,
    keterangan: &str,
) -> Result<Vec<Log>> {
    let mut sabar = SABAR_MAKSIMUM;
    loop {
        match provider.get_logs(filter).await {
            Ok(l) => return Ok(l),
            Err(e) if sabar > 0 && dibatasi_laju(&e) => {
                sabar -= 1;
                tracing::debug!(sabar, "pembacaan log dibatasi laju, menunggu");
                tokio::time::sleep(Duration::from_millis(JEDA_DIBATASI_MS)).await;
            }
            Err(e) => return Err(anyhow::Error::new(e)).context(keterangan.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Palsu(&'static str);
    impl std::fmt::Display for Palsu {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    #[test]
    fn dua_penolakan_tidak_tertukar() {
        // Inilah kekeliruan yang membuat pemindaian gagal berulang kali:
        // dipersempit padahal seharusnya ditunggu, dan sebaliknya. Keduanya
        // datang sebagai error biasa dan hanya teksnya yang membedakan.
        let banyak = Palsu("error code -32000: logs matched by query exceeds limit of 10000");
        let laju = Palsu("Max retries exceeded server returned an error response: error code 429: Too Many Requests");

        assert!(terlalu_banyak(&banyak));
        assert!(!dibatasi_laju(&banyak));

        assert!(dibatasi_laju(&laju));
        assert!(!terlalu_banyak(&laju));
    }

    #[test]
    fn batas_lebar_dibedakan_dari_batas_jumlah() {
        // Batas jumlah hasil diatasi dengan menyempit. Batas lebar rentang
        // tidak: menyempitnya sampai sepuluh blok berarti ratusan ribu
        // permintaan, dan penyempitan otomatis hanya akan menyerah dengan
        // pesan yang tidak menjelaskan apa apa.
        let lebar = Palsu(
            "error code -32600: Under the Free tier plan, you can make \
             eth_getLogs requests with up to a 10 block range",
        );
        assert!(lebar_dibatasi(&lebar));
        assert!(!dibatasi_laju(&lebar));

        let jumlah = Palsu("logs matched by query exceeds limit of 10000");
        assert!(terlalu_banyak(&jumlah));
        assert!(!lebar_dibatasi(&jumlah));
    }

    #[test]
    fn kegagalan_lain_bukan_keduanya() {
        // Kegagalan yang tidak dikenali harus dikembalikan apa adanya, bukan
        // diulang selamanya atau diubah jadi hasil kosong.
        let lain = Palsu("connection reset by peer");
        assert!(!terlalu_banyak(&lain));
        assert!(!dibatasi_laju(&lain));
        assert!(!lebar_dibatasi(&lain));
    }

    #[test]
    fn penyempitan_berhenti_di_minimum() {
        // Pembagian berulang tanpa dasar akan sampai ke nol, dan jendela nol
        // selalu berhasil sambil tidak memuat apa apa. Hasil kosong yang selalu
        // berhasil jauh lebih berbahaya daripada kegagalan yang terlihat.
        let mut jendela = 2_000u64;
        let minimum = 200u64;
        for _ in 0..20 {
            jendela = (jendela / 2).max(minimum);
        }
        assert_eq!(jendela, minimum);
    }
}
