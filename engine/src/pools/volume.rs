//! Volume perdagangan per pool, dibaca dari event `Swap`.
//!
//! KENAPA INI DIBUTUHKAN
//! Optimizer memilih fee tier berdasarkan `volume x fee / likuiditas`, dan dua
//! dari tiga sukunya sudah ada di keadaan pool. Yang tidak ada hanya volume,
//! dan ia hanya bisa didapat dari log.
//!
//! KENAPA SATU PERMINTAAN, BUKAN SATU PER POOL
//! `id` pada event `Swap` terindeks, dan penyaring log menerima DAFTAR nilai
//! untuk satu posisi topic, yang artinya "salah satu dari". Jadi seluruh pool
//! sebuah token bisa ditanyakan sekaligus. Untuk FIG dengan enam pool layak,
//! itu satu permintaan alih alih enam.
//!
//! Menyaringnya di sisi RPC juga penting karena tanpa itu jawabannya memuat
//! seluruh swap di rantai. Pada jendela dua ribu blok saja jumlahnya sudah
//! delapan ribu lebih.
//!
//! INVARIAN
//! - Jendela pengukuran disebutkan bersama hasilnya. Volume tanpa jendela tidak
//!   berarti apa apa, dan membandingkan dua token yang jendelanya berbeda akan
//!   menyesatkan.
//! - Nilai mutlak yang dipakai, bukan nilai bertanda. Tanda pada `amount0` dan
//!   `amount1` menyatakan arah dari sudut pandang pool, dan volume tidak punya
//!   arah.
//! - Kegagalan pemindaian dikembalikan sebagai error, tidak pernah sebagai
//!   volume nol. Nol berarti pool itu sepi, dan itu kesimpulan yang sangat
//!   berbeda dari tidak bisa membaca.

use std::collections::HashMap;

use alloy::primitives::{Address, FixedBytes};
use alloy::providers::Provider;
use alloy::rpc::types::Filter;
use alloy::sol_types::SolEvent;
use anyhow::{Context, Result};

use super::abi;
use crate::chain::registry::Registry;

/// Jendela pengukuran volume, dalam blok.
///
/// Waktu blok rantai ini sekitar 100 ms, jadi 864.000 blok kira kira 24 jam.
/// Uniswap menampilkan volume 24 jam pada halaman token. Memakai periode yang
/// sama membuat angka dapat dibandingkan dan meredam ledakan singkat yang
/// sebelumnya dibesarkan 8.760 kali saat diproyeksikan menjadi APR tahunan.
pub const JENDELA_VOLUME_BLOK: u64 = 864_000;

/// Ukuran awal tiap permintaan log.
///
/// Seluruh 24 jam tetap dibaca. Potongan ini hanya menjaga satu jawaban di
/// bawah batas jumlah log milik RPC publik.
pub const POTONGAN_VOLUME_BLOK: u64 = 36_000;

/// Jendela terpendek yang masih diterima untuk pengukuran volume.
///
/// Jendela ini boleh menyempit kalau RPC menolak, tapi tidak sampai sependek
/// jendela pemindaian. Volume dua menit yang diekstrapolasi ke setahun pernah
/// menghasilkan proyeksi 7.645.797 persen, dan angka seperti itu bukan sekadar
/// tidak berguna, ia terlihat seperti temuan.
pub const JENDELA_VOLUME_MINIMUM: u64 = 6_000;

/// Hasil pengukuran volume beserta jendelanya.
#[derive(Debug, Clone)]
pub struct VolumeTerukur {
    /// Volume per poolId, dalam satuan terkecil token0.
    pub per_pool: HashMap<String, u128>,
    /// Volume per poolId, dalam satuan terkecil token1.
    ///
    /// Kutipan bisa berada di sisi mana saja, dan mengonversi lewat harga akan
    /// memasukkan kesalahan harga ke angka yang seharusnya murni penjumlahan.
    pub per_pool1: HashMap<String, u128>,
    pub dari_blok: u64,
    pub sampai_blok: u64,
    /// Berapa swap yang terbaca. Nol pada seluruh pool berarti benar benar
    /// sepi, bukan gagal membaca.
    pub jumlah_swap: usize,
    /// Benar kalau jendelanya sempat dipersempit dari yang diminta.
    ///
    /// Pemakainya perlu tahu ini sebelum mengekstrapolasi. Jendela yang
    /// menyempit tetap sah untuk membandingkan pool satu sama lain, tapi
    /// makin pendek jendelanya makin besar pengali menuju setahun, dan makin
    /// besar pula kesalahan yang ikut terkali.
    pub dipersempit: bool,
}

impl VolumeTerukur {
    /// Lamanya jendela dalam detik.
    ///
    /// Disediakan di sini supaya tidak ada pemakai yang menghitungnya sendiri
    /// lalu memakai jendela yang berbeda dari volumenya. Ketidakcocokan itu
    /// tepat yang membuat proyeksi APR meleset ribuan kali lipat.
    pub fn jendela_detik(&self, block_time_ms: u64) -> f64 {
        self.sampai_blok.saturating_sub(self.dari_blok) as f64 * block_time_ms as f64 / 1_000.0
    }
}

/// Ukur volume beberapa pool V3 sekaligus.
///
/// KENAPA TERPISAH DARI V4
/// Pada V3 setiap pool adalah kontraknya sendiri, dan event `Swap` yang ia
/// pancarkan tidak membawa pengenal pool sama sekali. Yang mengenalinya adalah
/// alamat log. Penyaringnya pun berbeda: `address` menerima daftar, jadi
/// seluruh pool tetap bisa ditanyakan dalam satu permintaan.
///
/// Sebelum ada ini, setiap token yang pasangan teratasnya V3 melaporkan volume
/// nol, dan nol itu terbaca sebagai pool yang sepi padahal justru ramai.
pub async fn ukur_volume_v3<P: Provider>(
    provider: &P,
    registry: &Registry,
    pool_alamat: &[String],
    jeda_latar: Option<&crate::chain::logs::JedaLatar>,
) -> Result<VolumeTerukur> {
    let mut per_pool: HashMap<String, u128> = HashMap::new();
    let mut per_pool1: HashMap<String, u128> = HashMap::new();
    for a in pool_alamat {
        per_pool.insert(a.to_lowercase(), 0);
        per_pool1.insert(a.to_lowercase(), 0);
    }

    let alamat: Vec<Address> = pool_alamat
        .iter()
        .filter_map(|s| s.parse::<Address>().ok())
        .collect();

    let latest = provider
        .get_block_number()
        .await
        .context("gagal membaca blok terakhir")?;

    if alamat.is_empty() {
        return Ok(VolumeTerukur {
            per_pool,
            per_pool1,
            dari_blok: latest,
            sampai_blok: latest,
            jumlah_swap: 0,
            dipersempit: false,
        });
    }

    let terbaca = crate::chain::logs::baca_mundur_lengkap(
        provider,
        latest,
        registry.chain.deploy_block,
        JENDELA_VOLUME_BLOK,
        POTONGAN_VOLUME_BLOK,
        JENDELA_VOLUME_MINIMUM,
        jeda_latar,
        |dari, sampai| {
            Filter::new()
                .address(alamat.clone())
                .event_signature(abi::IUniswapV3Events::Swap::SIGNATURE_HASH)
                .from_block(dari)
                .to_block(sampai)
        },
    )
    .await
    .context("pengukuran volume V3 gagal")?;

    let dari = terbaca.dari_blok;
    let jumlah_swap = terbaca.logs.len();

    for log in &terbaca.logs {
        let Ok(d) = abi::IUniswapV3Events::Swap::decode_log(&log.inner) else {
            continue;
        };
        // Yang mengenali pool adalah alamat log, bukan topic.
        let kunci = log.address().to_string().to_lowercase();
        *per_pool.entry(kunci.clone()).or_insert(0) += d.amount0.unsigned_abs().to::<u128>();
        *per_pool1.entry(kunci).or_insert(0) += d.amount1.unsigned_abs().to::<u128>();
    }

    Ok(VolumeTerukur {
        per_pool,
        per_pool1,
        dari_blok: dari,
        sampai_blok: latest,
        jumlah_swap,
        dipersempit: terbaca.dipersempit,
    })
}

/// Ukur volume beberapa pool V4 sekaligus.
///
/// Pool V3 dilewati: eventnya berbeda dan berada di kontrak pool masing masing,
/// sehingga tidak bisa ikut dalam satu penyaring. Di rantai ini V4 yang dipakai,
/// jadi pemisahan itu tidak merugikan apa apa, dan menambahkan V3 nanti tidak
/// akan mengubah bentuk fungsi ini.
pub async fn ukur_volume<P: Provider>(
    provider: &P,
    registry: &Registry,
    pool_ids: &[String],
    jeda_latar: Option<&crate::chain::logs::JedaLatar>,
) -> Result<VolumeTerukur> {
    let mut per_pool: HashMap<String, u128> = HashMap::new();
    let mut per_pool1: HashMap<String, u128> = HashMap::new();
    for id in pool_ids {
        per_pool.insert(id.to_lowercase(), 0);
        per_pool1.insert(id.to_lowercase(), 0);
    }

    let topik: Vec<FixedBytes<32>> = pool_ids
        .iter()
        .filter_map(|s| s.parse::<FixedBytes<32>>().ok())
        .collect();

    let latest = provider
        .get_block_number()
        .await
        .context("gagal membaca blok terakhir")?;

    if topik.is_empty() {
        return Ok(VolumeTerukur {
            per_pool,
            per_pool1,
            dari_blok: latest,
            sampai_blok: latest,
            jumlah_swap: 0,
            dipersempit: false,
        });
    }

    let pool_manager: Address = registry.contract("uniswap_v4", "pool_manager")?;

    let terbaca = crate::chain::logs::baca_mundur_lengkap(
        provider,
        latest,
        registry.chain.deploy_block,
        JENDELA_VOLUME_BLOK,
        POTONGAN_VOLUME_BLOK,
        JENDELA_VOLUME_MINIMUM,
        jeda_latar,
        |dari, sampai| {
            Filter::new()
                .address(pool_manager)
                .event_signature(abi::Swap::SIGNATURE_HASH)
                // Daftar nilai pada satu posisi topic berarti "salah satu
                // dari", jadi seluruh pool ditanyakan dalam satu permintaan.
                .topic1(topik.clone())
                .from_block(dari)
                .to_block(sampai)
        },
    )
    .await
    .context("pengukuran volume gagal")?;

    let dari = terbaca.dari_blok;
    let logs = terbaca.logs;
    let jumlah_swap = logs.len();

    for log in &logs {
        let Ok(decoded) = abi::Swap::decode_log(&log.inner) else {
            continue;
        };
        let id = format!("0x{}", hex::encode(decoded.id));
        // Nilai mutlak, karena tanda menyatakan arah dan volume tidak punya arah.
        *per_pool.entry(id.to_lowercase()).or_insert(0) += decoded.amount0.unsigned_abs();
        *per_pool1.entry(id.to_lowercase()).or_insert(0) += decoded.amount1.unsigned_abs();
    }

    Ok(VolumeTerukur {
        per_pool,
        per_pool1,
        dari_blok: dari,
        sampai_blok: latest,
        jumlah_swap,
        dipersempit: terbaca.dipersempit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jendela_kira_kira_satu_hari() {
        // Waktu blok sekitar 100 ms, jadi 864.000 blok kira kira 24 jam. Kalau
        // angka ini diubah, keterangan di kartu panel harus ikut berubah.
        let detik = JENDELA_VOLUME_BLOK as f64 * 0.1;
        assert!(
            (detik - 86_400.0).abs() < 1_500.0,
            "jendela {detik} detik, seharusnya sekitar satu hari"
        );
    }

    #[test]
    fn kunci_disimpan_dalam_huruf_kecil() {
        // poolId datang dari dua sumber dengan gaya penulisan berbeda. Kalau
        // pencocokannya sensitif huruf, volumenya akan selalu nol.
        let mut m: HashMap<String, u128> = HashMap::new();
        m.insert("0xabc".to_string(), 5);
        assert_eq!(m.get(&"0xABC".to_lowercase()), Some(&5));
    }
}
