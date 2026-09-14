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
pub mod history;
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
    /// Identitas global: chain:position-manager:token-id.
    pub position_id: String,
    /// Token id NFT posisi.
    pub token_id: String,
    pub version: PoolVersion,
    pub token0: String,
    pub token1: String,
    /// Metadata dan harga manusiawi untuk panel. Kosong hanya kalau token
    /// tidak menjawab metadata ERC20; unit mentah tetap tersedia untuk mesin.
    pub symbol0: Option<String>,
    pub symbol1: Option<String>,
    pub decimals0: Option<u8>,
    pub decimals1: Option<u8>,
    /// Harga selalu dinyatakan sebagai quote per base dan batasnya sudah
    /// diurutkan dari kecil ke besar, termasuk saat quote berada di token0.
    pub base_symbol: Option<String>,
    pub quote_symbol: Option<String>,
    pub price_current: Option<f64>,
    pub price_lower: Option<f64>,
    pub price_upper: Option<f64>,
    pub fee: u32,
    pub tick_spacing: i32,
    pub tick_lower: i32,
    pub tick_upper: i32,
    pub liquidity: String,
    /// Tick pool sekarang. Kosong kalau poolnya tidak terbaca.
    pub tick_sekarang: Option<i32>,
    pub in_range: bool,
    /// Fee yang belum diklaim, dalam satuan terkecil.
    pub fee0: Option<String>,
    pub fee1: Option<String>,
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

pub fn position_id(chain: u64, manager: Address, token_id: U256) -> String {
    format!("{chain}:{}:{token_id}", manager.to_string().to_lowercase())
}

/// Kunci posisi V4 yang tersimpan, supaya tidak memindai log berulang kali.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheV4 {
    pub scanned_to: u64,
    /// Percobaan pemindaian penuh terakhir. Dipakai untuk menahan RPC yang
    /// diketahui menolak rentang sejarah agar tidak dipukul tiap 30 detik.
    #[serde(default)]
    pub last_scan_attempt_unix: i64,
    /// Token id yang pernah terlihat berpindah ke wallet ini.
    ///
    /// Kepemilikan sekarang TIDAK disimpan, karena ia bisa berubah kapan saja.
    /// Yang disimpan hanya daftar calon, dan pemiliknya selalu dibaca ulang.
    pub kandidat: Vec<String>,
}

fn pemindaian_ditahan(cache: &CacheV4, kandidat: &[String], sekarang_unix: i64) -> bool {
    !kandidat.is_empty()
        && cache.last_scan_attempt_unix > 0
        && sekarang_unix.saturating_sub(cache.last_scan_attempt_unix) < 300
}

fn kunci_cache(chain: u64, manager: Address, owner: &str) -> String {
    format!(
        "posisi_v4:v2:{chain}:{}:{}",
        manager.to_string().to_lowercase(),
        owner.to_lowercase()
    )
}

/// Ambil kandidat yang sudah dibuktikan receipt pembukaan Rangefinder.
///
/// Ini hanya jalur cepat tambahan. Posisi yang dibuat aplikasi lain tetap
/// ditemukan lewat log, dan setiap kandidat tetap diperiksa dengan `ownerOf`
/// sebelum ditampilkan.
fn kandidat_v4_dari_jurnal(
    store: &crate::store::Store,
    chain: u64,
    owner: Address,
    manager: Address,
) -> Result<Vec<String>> {
    let manager = manager.to_string().to_lowercase();
    let mut out = Vec::new();
    for entry in crate::exec::journal::entries(store, chain, owner)? {
        if entry.context.operation != crate::exec::journal::Operation::Open {
            continue;
        }
        let Some(position_id) = entry.context.position_id.as_deref() else {
            continue;
        };
        let mut bagian = position_id.splitn(3, ':');
        let cocok = bagian
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .is_some_and(|value| value == chain)
            && bagian
                .next()
                .is_some_and(|value| value.eq_ignore_ascii_case(&manager));
        let Some(token_id) = bagian.next().filter(|_| cocok) else {
            continue;
        };
        if token_id.parse::<U256>().is_ok() && !out.iter().any(|id| id == token_id) {
            out.push(token_id.to_owned());
        }
    }
    Ok(out)
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

    let cache_key = kunci_cache(registry.chain.id, pm, &alamat_owner);
    let tersimpan: CacheV4 = store
        .and_then(|s| s.get(&cache_key).ok().flatten())
        .unwrap_or_default();

    let jendela = registry.chain.deploy_block;
    let bersambung = tersimpan.scanned_to > 0 && tersimpan.scanned_to + 1 >= jendela;
    let dari = if bersambung {
        (tersimpan.scanned_to + 1).min(latest)
    } else {
        jendela
    };

    let mut kandidat: Vec<String> = tersimpan.kandidat.clone();
    if let Some(store) = store {
        match kandidat_v4_dari_jurnal(store, registry.chain.id, owner, pm) {
            Ok(dari_jurnal) => {
                for id in dari_jurnal {
                    if !kandidat.contains(&id) {
                        kandidat.push(id);
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%error, "kandidat posisi dari jurnal tidak terbaca");
            }
        }
    }

    let sekarang_unix = chrono::Utc::now().timestamp();
    if pemindaian_ditahan(&tersimpan, &kandidat, sekarang_unix) {
        return Ok(kandidat
            .into_iter()
            .filter_map(|s| s.parse::<U256>().ok())
            .collect());
    }

    let mut sampai = latest;
    while dari <= sampai {
        let baca = crate::chain::logs::baca_mundur(
            provider,
            sampai,
            dari,
            LOOKBACK_TRANSFER,
            100,
            |lo, hi| {
                Filter::new()
                    .address(pm)
                    .event_signature(FixedBytes::<32>::from(TOPIC_TRANSFER))
                    .topic2(owner.into_word())
                    .from_block(lo)
                    .to_block(hi)
            },
        );
        // Kalau kandidat aman sudah tersedia, pemindaian historis tetap dicoba
        // untuk menemukan posisi dari aplikasi lain tetapi tidak boleh menahan
        // posisi yang baru dibuka di belakang failover RPC yang lambat.
        let hasil = if kandidat.is_empty() {
            baca.await
        } else {
            match tokio::time::timeout(std::time::Duration::from_secs(2), baca).await {
                Ok(hasil) => hasil,
                Err(_) => Err(anyhow::anyhow!(
                    "pemindaian posisi V4 melewati batas respons dua detik"
                )),
            }
        };
        let hasil = match hasil {
            Ok(hasil) => hasil,
            Err(error) if !kandidat.is_empty() => {
                // RPC publik sering menolak rentang log sejarah. Kandidat yang
                // sudah ada tetap aman dipakai karena muat_v4 membaca ownerOf
                // dan metadata posisi langsung dari rantai sesudah fungsi ini.
                tracing::warn!(
                    %error,
                    kandidat = kandidat.len(),
                    "pemindaian posisi V4 gagal; memakai kandidat tersimpan"
                );
                if let Some(s) = store {
                    let sebagian = CacheV4 {
                        scanned_to: tersimpan.scanned_to,
                        last_scan_attempt_unix: sekarang_unix,
                        kandidat: kandidat.clone(),
                    };
                    if let Err(e) = s.put(&cache_key, &sebagian) {
                        tracing::warn!(error = %e, "cache kandidat posisi V4 gagal disimpan");
                    }
                }
                return Ok(kandidat
                    .into_iter()
                    .filter_map(|s| s.parse::<U256>().ok())
                    .collect());
            }
            Err(error) => return Err(error),
        };

        for log in hasil.logs {
            let topics = log.inner.data.topics();
            if topics.len() != 4 {
                continue;
            }
            let id = U256::from_be_bytes(topics[3].0).to_string();
            if !kandidat.contains(&id) {
                kandidat.push(id);
            }
        }
        if hasil.dari_blok <= dari {
            break;
        }
        sampai = hasil.dari_blok - 1;
    }

    if let Some(s) = store {
        let baru = CacheV4 {
            scanned_to: latest,
            last_scan_attempt_unix: sekarang_unix,
            kandidat: kandidat.clone(),
        };
        if let Err(e) = s.put(&cache_key, &baru) {
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

    #[test]
    fn jurnal_open_menjadi_kandidat_v4_tanpa_memercayai_manager_lain() {
        use alloy::{
            network::TransactionBuilder, primitives::B256, rpc::types::TransactionRequest,
        };

        let dir = std::env::temp_dir().join(format!(
            "rangefinder-position-candidate-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let store = crate::store::Store::open(&dir).unwrap();
        let owner = Address::repeat_byte(1);
        let manager = Address::repeat_byte(2);
        let journal = crate::exec::journal::Journal {
            store: &store,
            context: crate::exec::journal::test_context(),
        };
        let request = TransactionRequest::default()
            .with_from(owner)
            .with_to(manager)
            .with_chain_id(4663)
            .with_nonce(7)
            .with_gas_limit(100_000);
        let hash = B256::repeat_byte(3);
        journal.prepare(4663, owner, hash, request).unwrap();
        crate::exec::journal::link_position(
            &store,
            4663,
            owner,
            hash,
            &position_id(4663, manager, U256::from(2_671_313u64)),
        )
        .unwrap();

        assert_eq!(
            kandidat_v4_dari_jurnal(&store, 4663, owner, manager).unwrap(),
            vec!["2671313"]
        );
        assert!(
            kandidat_v4_dari_jurnal(&store, 4663, owner, Address::repeat_byte(9))
                .unwrap()
                .is_empty()
        );

        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn kegagalan_log_ditahan_meski_cache_punya_blok_lama_nonzero() {
        let cache = CacheV4 {
            scanned_to: 62_472_850,
            last_scan_attempt_unix: 1_000,
            kandidat: vec!["2671313".into()],
        };
        assert!(pemindaian_ditahan(&cache, &cache.kandidat, 1_299));
        assert!(!pemindaian_ditahan(&cache, &cache.kandidat, 1_300));
        assert!(!pemindaian_ditahan(&cache, &[], 1_001));
    }

    fn posisi(lower: i32, upper: i32, tick: Option<i32>) -> Posisi {
        Posisi {
            position_id: "4663:uji:1".into(),
            token_id: "1".into(),
            version: PoolVersion::V4,
            token0: "0x0".into(),
            token1: "0x1".into(),
            symbol0: None,
            symbol1: None,
            decimals0: None,
            decimals1: None,
            base_symbol: None,
            quote_symbol: None,
            price_current: None,
            price_lower: None,
            price_upper: None,
            fee: 3000,
            tick_spacing: 60,
            tick_lower: lower,
            tick_upper: upper,
            liquidity: "100".into(),
            tick_sekarang: tick,
            in_range: tick.is_some_and(|t| t >= lower && t < upper),
            fee0: Some("0".into()),
            fee1: Some("0".into()),
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
        assert_eq!(
            kunci_cache(4663, Address::ZERO, "0xAbC"),
            kunci_cache(4663, Address::ZERO, "0xabc")
        );
        assert_ne!(
            kunci_cache(4663, Address::ZERO, "0xabc"),
            kunci_cache(1, Address::ZERO, "0xabc")
        );
    }
}
