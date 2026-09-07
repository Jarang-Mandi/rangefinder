//! Posisi LP milik wallet, V3 maupun V4.
//!
//! INVARIAN
//! - Posisi yang dibuka di luar Rangefinder diperlakukan sama persis dengan
//!   yang dibuka dari sini. Ini permintaan user yang eksplisit, dan konsekuensi
//!   teknisnya besar: tidak ada satu pun bagian yang boleh bergantung pada
//!   catatan lokal untuk sekadar menampilkan posisi.
//! - Fee yang belum diklaim dihitung dari pertumbuhan fee, bukan dibaca dari
//!   `tokensOwed`. Lihat `fees`.
//! - Kegagalan membaca satu posisi tidak menjatuhkan yang lain, tapi juga tidak
//!   dipalsukan menjadi nol. Posisi yang tidak terbaca dilewati dan dicatat.

pub mod fees;
pub mod muat;

use alloy::primitives::{Address, FixedBytes, U256};
use alloy::providers::Provider;
use alloy::rpc::types::Filter;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::chain::registry::Registry;
use crate::pools::PoolVersion;

/// Satu posisi LP beserta keadaannya sekarang.
#[derive(Debug, Clone, Serialize)]
pub struct Posisi {
    /// Token id NFT posisi.
    pub token_id: String,
    pub version: PoolVersion,
    pub token0: String,
    pub token1: String,
    pub fee: u32,
    pub tick_lower: i32,
    pub tick_upper: i32,
    pub liquidity: String,
    /// Tick pool sekarang. Kosong kalau poolnya tidak terbaca.
    pub tick_sekarang: Option<i32>,
    pub in_range: bool,
    /// Fee yang belum diklaim, dalam satuan terkecil.
    pub fee0: String,
    pub fee1: String,
    /// Hook pool, hanya ada di V4.
    pub hooks: Option<String>,
    /// Harga pool sekarang dalam format Q64.96. Kosong kalau tidak terbaca.
    pub sqrt_price_x96: Option<String>,
    /// Jumlah token yang benar benar ada di dalam posisi pada harga
    /// sekarang. Ini yang dipakai menghitung impermanent loss, bukan jumlah
    /// yang disetor dulu.
    pub amount0: String,
    pub amount1: String,
    /// Hasil posisi, kalau bisa dihitung. Kosong berarti harga pool tidak
    /// terbaca sehingga tidak ada dasar untuk menghitungnya.
    pub pnl: Option<crate::pnl::Hasil>,
    /// Benar kalau posisinya sudah tidak punya likuiditas sama sekali.
    pub kosong: bool,
}

impl Posisi {
    /// Seberapa jauh harga sekarang di dalam range, sebagai porsi 0 sampai 1.
    ///
    /// Dipakai panel untuk bar horizontal. Di luar range hasilnya dijepit ke
    /// ujung, karena bar tidak bisa menggambarkan posisi di luar dirinya.
    pub fn porsi_dalam_range(&self) -> f64 {
        let Some(tick) = self.tick_sekarang else {
            return 0.5;
        };
        if self.tick_upper <= self.tick_lower {
            return 0.5;
        }
        let lebar = (self.tick_upper - self.tick_lower) as f64;
        (((tick - self.tick_lower) as f64) / lebar).clamp(0.0, 1.0)
    }
}

/// Kunci posisi V4 yang tersimpan, supaya tidak memindai log berulang kali.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheV4 {
    pub scanned_to: u64,
    /// Token id yang pernah terlihat berpindah ke wallet ini.
    ///
    /// Kepemilikan sekarang TIDAK disimpan, karena ia bisa berubah kapan saja.
    /// Yang disimpan hanya daftar calon, dan pemiliknya selalu dibaca ulang.
    pub kandidat: Vec<String>,
}

fn kunci_cache(owner: &str) -> String {
    format!("posisi_v4:v1:{}", owner.to_lowercase())
}

/// Jendela mundur pemindaian Transfer, dalam blok.
///
/// Sama dengan jendela pencarian pool. Posisi yang lebih tua dari ini tidak akan
/// ditemukan pada pemindaian pertama, tapi sekali ketemu ia tersimpan di cache
/// dan tidak hilang lagi.
pub const LOOKBACK_TRANSFER: u64 = 3_000_000;

/// Topic0 event `Transfer(address,address,uint256)`.
const TOPIC_TRANSFER: [u8; 32] = [
    0xdd, 0xf2, 0x52, 0xad, 0x1b, 0xe2, 0xc8, 0x9b, 0x69, 0xc2, 0xb0, 0x68, 0xfc, 0x37, 0x8d, 0xaa,
    0x95, 0x2b, 0xa7, 0xf1, 0x63, 0xc4, 0xa1, 0x16, 0x28, 0xf5, 0x5a, 0x4d, 0xf5, 0x23, 0xb3, 0xef,
];

/// Cari token id posisi V4 milik sebuah wallet.
///
/// KENAPA LEWAT LOG, BUKAN ENUMERASI
/// NonfungiblePositionManager v3 mengimplementasikan ERC721Enumerable, sehingga
/// posisinya bisa dihitung lewat `balanceOf` lalu `tokenOfOwnerByIndex`.
/// PositionManager V4 TIDAK: pemeriksaan bytecode menunjukkan
/// `tokenOfOwnerByIndex`, `totalSupply`, dan `tokenByIndex` tidak ada sama
/// sekali. Satu satunya cara adalah memindai event `Transfer` yang menuju
/// wallet itu.
///
/// Hasil pemindaian disimpan supaya pemuatan berikutnya hanya melihat blok baru.
/// Yang disimpan hanya daftar calon, bukan kepemilikannya, karena kepemilikan
/// bisa berubah kapan saja dan cache yang menyimpannya akan menampilkan posisi
/// milik orang lain.
pub async fn kandidat_v4<P: Provider>(
    provider: &P,
    registry: &Registry,
    store: Option<&crate::store::Store>,
    owner: Address,
) -> Result<Vec<U256>> {
    let pm = registry.contract("uniswap_v4", "position_manager")?;
    let alamat_owner = owner.to_string();

    let latest = provider
        .get_block_number()
        .await
        .context("gagal membaca blok terakhir")?;

    let tersimpan: CacheV4 = store
        .and_then(|s| s.get(&kunci_cache(&alamat_owner)).ok().flatten())
        .unwrap_or_default();

    let jendela = latest
        .saturating_sub(LOOKBACK_TRANSFER)
        .max(registry.chain.deploy_block);
    let bersambung = tersimpan.scanned_to > 0 && tersimpan.scanned_to + 1 >= jendela;
    let dari = if bersambung {
        (tersimpan.scanned_to + 1).min(latest)
    } else {
        jendela
    };

    let mut kandidat: Vec<String> = tersimpan.kandidat.clone();

    if dari <= latest {
        let filter = Filter::new()
            .address(pm)
            .event_signature(FixedBytes::<32>::from(TOPIC_TRANSFER))
            .topic2(owner.into_word())
            .from_block(dari)
            .to_block(latest);

        let logs = provider
            .get_logs(&filter)
            .await
            .with_context(|| format!("pemindaian Transfer posisi V4 gagal pada {dari}..{latest}"))?;

        for log in logs {
            let topics = log.inner.data.topics();
            if topics.len() != 4 {
                continue;
            }
            let id = U256::from_be_bytes(topics[3].0).to_string();
            if !kandidat.contains(&id) {
                kandidat.push(id);
            }
        }
    }

    if let Some(s) = store {
        let baru = CacheV4 {
            scanned_to: latest,
            kandidat: kandidat.clone(),
        };
        if let Err(e) = s.put(&kunci_cache(&alamat_owner), &baru) {
            tracing::warn!(error = %e, "cache posisi V4 gagal disimpan");
        }
    }

    Ok(kandidat
        .into_iter()
        .filter_map(|s| s.parse::<U256>().ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn posisi(lower: i32, upper: i32, tick: Option<i32>) -> Posisi {
        Posisi {
            token_id: "1".into(),
            version: PoolVersion::V4,
            token0: "0x0".into(),
            token1: "0x1".into(),
            fee: 3000,
            tick_lower: lower,
            tick_upper: upper,
            liquidity: "100".into(),
            tick_sekarang: tick,
            in_range: tick.is_some_and(|t| t >= lower && t < upper),
            fee0: "0".into(),
            fee1: "0".into(),
            hooks: None,
            sqrt_price_x96: None,
            amount0: "0".into(),
            amount1: "0".into(),
            pnl: None,
            kosong: false,
        }
    }

    #[test]
    fn porsi_di_tengah_range() {
        assert!((posisi(-100, 100, Some(0)).porsi_dalam_range() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn porsi_di_batas_bawah_nol() {
        assert_eq!(posisi(-100, 100, Some(-100)).porsi_dalam_range(), 0.0);
    }

    #[test]
    fn porsi_di_batas_atas_satu() {
        assert_eq!(posisi(-100, 100, Some(100)).porsi_dalam_range(), 1.0);
    }

    #[test]
    fn di_luar_range_dijepit_ke_ujung() {
        // Bar tidak bisa menggambarkan posisi di luar dirinya. Menjepitnya lebih
        // jujur daripada menggambar bar yang meluber keluar kotak.
        assert_eq!(posisi(-100, 100, Some(-500)).porsi_dalam_range(), 0.0);
        assert_eq!(posisi(-100, 100, Some(500)).porsi_dalam_range(), 1.0);
    }

    #[test]
    fn tanpa_tick_kembali_ke_tengah() {
        // Pool yang tidak terbaca tidak boleh membuat bar melompat ke ujung,
        // karena itu terlihat seperti posisi yang keluar range.
        assert_eq!(posisi(-100, 100, None).porsi_dalam_range(), 0.5);
    }

    #[test]
    fn range_selebar_nol_tidak_membagi_dengan_nol() {
        assert_eq!(posisi(50, 50, Some(50)).porsi_dalam_range(), 0.5);
    }

    #[test]
    fn batas_atas_dianggap_di_luar_range() {
        // Uniswap memperlakukan range sebagai setengah terbuka: tick sama dengan
        // batas atas berarti sudah di luar. Salah di sini membuat posisi yang
        // baru saja keluar terlihat masih aman.
        assert!(!posisi(-100, 100, Some(100)).in_range);
        assert!(posisi(-100, 100, Some(99)).in_range);
        assert!(posisi(-100, 100, Some(-100)).in_range);
    }

    #[test]
    fn kunci_cache_tidak_peduli_besar_kecil_huruf() {
        assert_eq!(kunci_cache("0xAbC"), kunci_cache("0xabc"));
    }
}
