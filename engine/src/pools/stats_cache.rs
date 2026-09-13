//! Cache singkat untuk APR, TVL, dan volume pool.
//!
//! Statistik 24 jam mahal karena harus membaca log swap dan distribusi tick.
//! Angka itu berubah jauh lebih lambat daripada harga spot. Hasil terakhir
//! karena itu boleh ditampilkan lebih dulu saat token dibuka kembali, lalu
//! diperbarui di latar jika sudah lewat batas segar.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::{urutkan_pool_terukur, PoolInfo, PoolStats};
use crate::store::Store;

const VERSI: u32 = 3;

/// Dalam batas ini cache tidak perlu dihitung ulang.
pub const SEGAR_DETIK: i64 = 5 * 60;

/// Cache yang lebih lama masih boleh tampil sambil penyegaran berjalan.
/// Setelah satu jam, perubahan pasar sudah terlalu besar untuk ditampilkan.
pub const MAKS_TAMPIL_DETIK: i64 = 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusCache {
    Kosong,
    Segar,
    PerluPenyegaran,
}

impl StatusCache {
    pub fn perlu_penyegaran(self) -> bool {
        self != Self::Segar
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Rekaman {
    diukur_pada_unix: i64,
    pools: HashMap<String, PoolStats>,
}

fn kunci(token: &str, quote: &str) -> String {
    format!(
        "poolstats:v{VERSI}:{}:{}",
        token.to_lowercase(),
        quote.to_lowercase()
    )
}

fn statistik_masuk_akal(stats: &PoolStats) -> bool {
    [stats.volume, stats.jendela_menit, stats.tvl, stats.apr]
        .into_iter()
        .flatten()
        .all(|v| v.is_finite() && v >= 0.0)
}

fn statistik_layak_disimpan(stats: &PoolStats) -> bool {
    statistik_masuk_akal(stats) && stats.volume.is_some() && stats.jendela_menit.is_some()
}

fn baca(store: &Store, token: &str, quote: &str) -> Option<Rekaman> {
    match store.get::<Rekaman>(&kunci(token, quote)) {
        Ok(v) => v,
        Err(error) => {
            tracing::warn!(%error, token, quote, "cache statistik tidak terbaca");
            None
        }
    }
}

fn tercakup(rekaman: &Rekaman, pools: &[PoolInfo]) -> bool {
    !pools.is_empty()
        && pools.iter().all(|pool| {
            rekaman
                .pools
                .get(&pool.id.to_lowercase())
                .is_some_and(statistik_layak_disimpan)
        })
}

/// Isi statistik pool yang ID-nya masih ada di snapshot terbaru.
pub fn terapkan(
    store: &Store,
    token: &str,
    quote: &str,
    pools: &mut [PoolInfo],
    sekarang_unix: i64,
) -> StatusCache {
    let Some(rekaman) = baca(store, token, quote) else {
        return StatusCache::Kosong;
    };
    let umur = sekarang_unix.saturating_sub(rekaman.diukur_pada_unix);
    if !(0..=MAKS_TAMPIL_DETIK).contains(&umur) {
        return StatusCache::Kosong;
    }
    let semua_tercakup = tercakup(&rekaman, pools);
    let mut terpakai = 0usize;
    for pool in pools.iter_mut() {
        if let Some(stats) = rekaman
            .pools
            .get(&pool.id.to_lowercase())
            .filter(|stats| statistik_layak_disimpan(stats))
        {
            pool.stats = Some(stats.clone());
            terpakai += 1;
        }
    }
    if terpakai == 0 {
        return StatusCache::Kosong;
    }
    urutkan_pool_terukur(pools);
    if semua_tercakup && umur <= SEGAR_DETIK {
        StatusCache::Segar
    } else {
        StatusCache::PerluPenyegaran
    }
}

/// Periksa apakah hasil yang tersimpan masih segar untuk snapshot ini.
pub fn status(
    store: &Store,
    token: &str,
    quote: &str,
    pools: &[PoolInfo],
    sekarang_unix: i64,
) -> StatusCache {
    let Some(rekaman) = baca(store, token, quote) else {
        return StatusCache::Kosong;
    };
    let umur = sekarang_unix.saturating_sub(rekaman.diukur_pada_unix);
    if umur < 0
        || !pools
            .iter()
            .any(|pool| rekaman.pools.contains_key(&pool.id.to_lowercase()))
    {
        StatusCache::Kosong
    } else if tercakup(&rekaman, pools) && umur <= SEGAR_DETIK {
        StatusCache::Segar
    } else if umur <= MAKS_TAMPIL_DETIK {
        StatusCache::PerluPenyegaran
    } else {
        StatusCache::Kosong
    }
}

/// Simpan snapshot kalau volume seluruh pool selesai dibaca. TVL satu pool
/// boleh kosong karena pembacaan tick-nya gagal sendiri; pool lain tetap punya
/// hasil yang sah dan tidak perlu memindai ulang 24 jam log.
pub fn simpan(store: &Store, token: &str, quote: &str, pools: &[PoolInfo], sekarang_unix: i64) {
    let hasil: Option<HashMap<String, PoolStats>> = pools
        .iter()
        .map(|pool| {
            let stats = pool.stats.as_ref()?;
            statistik_layak_disimpan(stats).then(|| (pool.id.to_lowercase(), stats.clone()))
        })
        .collect();
    let Some(pools) = hasil.filter(|pools| !pools.is_empty()) else {
        return;
    };
    let rekaman = Rekaman {
        diukur_pada_unix: sekarang_unix,
        pools,
    };
    if let Err(error) = store.put(&kunci(token, quote), &rekaman) {
        tracing::warn!(%error, token, quote, "cache statistik gagal disimpan");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pools::{PoolInfo, PoolVersion};

    fn store_sementara(nama: &str) -> (Store, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "rangefinder-stats-cache-{nama}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        (Store::open(&dir).unwrap(), dir)
    }

    fn pool(id: &str, volume: Option<f64>) -> PoolInfo {
        PoolInfo {
            version: PoolVersion::V4,
            id: id.into(),
            token0: "0x1".into(),
            token1: "0x2".into(),
            fee: 3_000,
            dynamic_fee: false,
            tick_spacing: 60,
            hooks: None,
            sqrt_price_x96: "1".into(),
            tick: 0,
            liquidity: "1".into(),
            raw_price: 1.0,
            stats: volume.map(|volume| PoolStats {
                volume: Some(volume),
                jendela_menit: Some(1_440.0),
                tvl: Some(100.0),
                apr: Some(volume),
            }),
        }
    }

    #[test]
    fn cache_segar_dipakai_per_pool_dan_pool_baru_meminta_penyegaran() {
        let (store, dir) = store_sementara("segar");
        simpan(
            &store,
            "0xToken",
            "0xQuote",
            &[pool("0xA", Some(12.0)), pool("0xC", Some(8.0))],
            1_000,
        );

        let mut sama = vec![pool("0xa", None)];
        assert_eq!(
            terapkan(&store, "0xtoken", "0xquote", &mut sama, 1_010),
            StatusCache::Segar
        );
        assert_eq!(sama[0].stats.as_ref().and_then(|s| s.volume), Some(12.0));

        let mut berubah = vec![pool("0xA", None), pool("0xB", None)];
        assert_eq!(
            terapkan(&store, "0xToken", "0xQuote", &mut berubah, 1_010),
            StatusCache::PerluPenyegaran
        );
        assert!(berubah[0].stats.is_some());
        assert!(berubah[1].stats.is_none());

        let mut menyusut = vec![pool("0xC", None)];
        assert_eq!(
            terapkan(&store, "0xToken", "0xQuote", &mut menyusut, 1_010),
            StatusCache::Segar
        );
        assert_eq!(menyusut[0].stats.as_ref().and_then(|s| s.volume), Some(8.0));
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn cache_lama_tampil_sambil_meminta_penyegaran_lalu_kedaluwarsa() {
        let (store, dir) = store_sementara("lama");
        simpan(&store, "token", "quote", &[pool("pool", Some(7.0))], 1_000);

        let mut lama = vec![pool("pool", None)];
        assert_eq!(
            terapkan(&store, "token", "quote", &mut lama, 1_000 + SEGAR_DETIK + 1),
            StatusCache::PerluPenyegaran
        );
        assert!(lama[0].stats.is_some());

        let mut kedaluwarsa = vec![pool("pool", None)];
        assert_eq!(
            terapkan(
                &store,
                "token",
                "quote",
                &mut kedaluwarsa,
                1_000 + MAKS_TAMPIL_DETIK + 1
            ),
            StatusCache::Kosong
        );
        assert!(kedaluwarsa[0].stats.is_none());
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn kegagalan_volume_tidak_dibekukan_sebagai_cache_segar() {
        let (store, dir) = store_sementara("parsial");
        let mut parsial = pool("pool", Some(7.0));
        parsial.stats.as_mut().unwrap().volume = None;
        simpan(&store, "token", "quote", &[parsial], 1_000);

        let mut tujuan = vec![pool("pool", None)];
        assert_eq!(
            terapkan(&store, "token", "quote", &mut tujuan, 1_010),
            StatusCache::Kosong
        );
        assert!(tujuan[0].stats.is_none());
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn tvl_satu_pool_boleh_kosong_tanpa_membuang_volume_kelompok() {
        let (store, dir) = store_sementara("tvl-parsial");
        let lengkap = pool("lengkap", Some(9.0));
        let mut tanpa_tvl = pool("tanpa-tvl", Some(4.0));
        let stats = tanpa_tvl.stats.as_mut().unwrap();
        stats.tvl = None;
        stats.apr = None;
        simpan(&store, "token", "quote", &[lengkap, tanpa_tvl], 1_000);

        let mut tujuan = vec![pool("lengkap", None), pool("tanpa-tvl", None)];
        assert_eq!(
            terapkan(&store, "token", "quote", &mut tujuan, 1_010),
            StatusCache::Segar
        );
        assert!(tujuan
            .iter()
            .all(|pool| { pool.stats.as_ref().and_then(|stats| stats.volume).is_some() }));
        assert!(tujuan.iter().any(|pool| pool
            .stats
            .as_ref()
            .and_then(|stats| stats.tvl)
            .is_none()));
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }
}
