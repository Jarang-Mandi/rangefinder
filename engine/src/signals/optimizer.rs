//! Pemilihan fee tier otomatis, PRD 11.5.
//!
//! MASALAHNYA
//! Satu pasangan bisa punya pool di beberapa fee tier sekaligus. Yang paling
//! likuid belum tentu paling menguntungkan untuk LP, dan pada memecoin pool 8
//! persen sering mengalahkan pool 0,3 persen meski volumenya jauh lebih kecil.
//!
//! Yang menentukan bukan volume, bukan pula fee tier, melainkan hasilnya bagi
//! setiap dolar modal:
//!
//! ```text
//! proyeksi = volume * fee_tier / likuiditas
//! ```
//!
//! Angka itu tidak berarti sebagai nilai mutlak. Ia hanya berarti sebagai
//! perbandingan antar pool pada token yang sama, dan itulah cara ia dipakai.
//!
//! INVARIAN
//! - Pemeringkatan berjalan SETELAH penyaringan fee di luar akal sehat pada
//!   `pools::implausible_fee`. Tanpa urutan itu, rumus di atas justru memilih
//!   pool fee 80 persen yang volumenya nol, karena penyebutnya kecil dan
//!   pembilangnya dikali fee raksasa.
//! - Pool tanpa volume TIDAK diberi proyeksi nol lalu diurutkan di bawah. Ia
//!   ditandai secara terpisah, karena nol di sini berarti belum ada perdagangan
//!   pada jendela yang diukur, dan itu berbeda dari pool yang ramai tapi
//!   likuiditasnya terlalu tebal.
//! - Fee dinamis tidak bisa diproyeksikan, karena besarannya baru diketahui saat
//!   swap terjadi. Pool seperti itu ditandai, bukan ditebak.

use serde::Serialize;

use crate::pools::{PoolInfo, ASSUMED_DYNAMIC_FEE};

/// Satu pool beserta proyeksi hasilnya.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Peringkat {
    pub pool_id: String,
    pub version: crate::pools::PoolVersion,
    pub fee: u32,
    /// Benar kalau feenya ditentukan hook saat swap, sehingga proyeksinya
    /// hanya perkiraan kasar.
    pub dynamic_fee: bool,
    pub liquidity: String,
    /// Volume pada jendela yang diukur, dalam satuan terkecil token0.
    pub volume: String,
    /// Fee per satuan modal. Hanya berarti sebagai perbandingan antar pool.
    pub proyeksi: f64,
    /// Kenapa pool ini ada di posisi itu, dalam kalimat.
    pub catatan: String,
}

/// Volume satu pool pada jendela yang diukur.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VolumePool<'a> {
    pub pool_id: &'a str,
    /// Jumlah token0 yang berpindah, dalam satuan terkecil.
    pub volume: u128,
}

/// Peringkatkan pool menurut proyeksi fee per satuan modal.
///
/// `volume` boleh tidak memuat seluruh pool. Pool yang tidak ada di sana
/// dianggap belum diperdagangkan pada jendela itu, dan ditandai, bukan
/// dianggap buruk.
pub fn peringkat(pools: &[PoolInfo], volume: &[VolumePool]) -> Vec<Peringkat> {
    let mut out: Vec<Peringkat> = pools
        .iter()
        .map(|p| {
            let vol = volume
                .iter()
                .find(|v| v.pool_id.eq_ignore_ascii_case(&p.id))
                .map(|v| v.volume)
                .unwrap_or(0);

            let likuiditas: u128 = p.liquidity.parse().unwrap_or(0);

            // Fee dinamis dipakai dengan perkiraan konservatif, dan ditandai.
            // Menebaknya lebih tinggi akan menaikkan peringkat pool yang justru
            // paling tidak kita ketahui.
            let fee_efektif = if p.dynamic_fee {
                ASSUMED_DYNAMIC_FEE
            } else {
                p.fee
            };

            let proyeksi = if likuiditas == 0 {
                // Pool tanpa likuiditas seharusnya sudah tersaring lebih awal.
                // Kalau lolos sampai sini, ia tidak bisa dibandingkan.
                0.0
            } else {
                (vol as f64) * (fee_efektif as f64 / 1_000_000.0) / (likuiditas as f64)
            };

            let catatan = if p.dynamic_fee {
                format!(
                    "fee ditentukan hook saat swap, diproyeksikan dengan perkiraan {:.2} persen",
                    ASSUMED_DYNAMIC_FEE as f64 / 10_000.0
                )
            } else if vol == 0 {
                "belum ada perdagangan pada jendela yang diukur".to_string()
            } else {
                format!(
                    "fee {:.2} persen atas volume {vol} dengan likuiditas {likuiditas}",
                    p.fee as f64 / 10_000.0
                )
            };

            Peringkat {
                pool_id: p.id.clone(),
                version: p.version,
                fee: p.fee,
                dynamic_fee: p.dynamic_fee,
                liquidity: p.liquidity.clone(),
                volume: vol.to_string(),
                proyeksi,
                catatan,
            }
        })
        .collect();

    // Pool yang belum diperdagangkan turun ke bawah, apa pun proyeksinya.
    // Proyeksi nol dari volume nol tidak sebanding dengan proyeksi kecil dari
    // volume nyata: yang satu belum diketahui, yang lain sudah diukur.
    out.sort_by(|a, b| {
        let punya_volume = |p: &Peringkat| p.volume != "0";
        punya_volume(b)
            .cmp(&punya_volume(a))
            .then_with(|| {
                b.proyeksi
                    .partial_cmp(&a.proyeksi)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.pool_id.cmp(&b.pool_id))
    });

    out
}

/// Berapa kali lipat pool teratas mengungguli pool paling likuid.
///
/// Ini angka yang menjelaskan kenapa optimizer ada. Kalau keduanya pool yang
/// sama, hasilnya `None` dan tidak ada yang perlu dijelaskan.
pub fn keunggulan_terhadap_paling_likuid(hasil: &[Peringkat]) -> Option<(String, f64)> {
    let teratas = hasil.first()?;
    let paling_likuid = hasil
        .iter()
        .max_by_key(|p| p.liquidity.parse::<u128>().unwrap_or(0))?;

    if teratas.pool_id == paling_likuid.pool_id {
        return None;
    }
    if paling_likuid.proyeksi <= 0.0 {
        return None;
    }
    Some((
        paling_likuid.pool_id.clone(),
        teratas.proyeksi / paling_likuid.proyeksi,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pools::PoolVersion;

    fn pool(id: &str, fee: u32, liquidity: &str) -> PoolInfo {
        PoolInfo {
            version: PoolVersion::V4,
            id: id.into(),
            token0: "0x0".into(),
            token1: "0x1".into(),
            fee,
            dynamic_fee: false,
            tick_spacing: 60,
            hooks: None,
            sqrt_price_x96: "1".into(),
            tick: 0,
            liquidity: liquidity.into(),
            raw_price: 1.0,
            stats: None,
        }
    }

    #[test]
    fn fee_tinggi_bisa_mengalahkan_volume_besar() {
        // INI ALASAN OPTIMIZER ADA.
        //
        // Pool 0,3 persen punya volume sepuluh kali lipat, tapi likuiditasnya
        // seratus kali lebih tebal. Pool 1 persen memberi lebih banyak fee per
        // satuan modal, dan itu yang benar benar diterima LP.
        let pools = vec![
            pool("0xtipis", 10_000, "1000"),
            pool("0xtebal", 3_000, "100000"),
        ];
        let volume = vec![
            VolumePool {
                pool_id: "0xtipis",
                volume: 100_000,
            },
            VolumePool {
                pool_id: "0xtebal",
                volume: 1_000_000,
            },
        ];
        let hasil = peringkat(&pools, &volume);
        assert_eq!(
            hasil[0].pool_id, "0xtipis",
            "pool berfee tinggi dengan likuiditas tipis seharusnya menang"
        );
    }

    #[test]
    fn likuiditas_tebal_menurunkan_hasil_per_modal() {
        let pools = vec![pool("0xa", 3_000, "1000"), pool("0xb", 3_000, "10000")];
        let volume = vec![
            VolumePool {
                pool_id: "0xa",
                volume: 500_000,
            },
            VolumePool {
                pool_id: "0xb",
                volume: 500_000,
            },
        ];
        let hasil = peringkat(&pools, &volume);
        assert_eq!(hasil[0].pool_id, "0xa");
        assert!(hasil[0].proyeksi > hasil[1].proyeksi * 9.0);
    }

    #[test]
    fn pool_tanpa_volume_turun_ke_bawah_dan_ditandai() {
        // Nol di sini berarti belum ada perdagangan pada jendela yang diukur,
        // dan itu berbeda dari pool yang ramai tapi likuiditasnya terlalu tebal.
        let pools = vec![
            pool("0xsepi", 10_000, "1"),
            pool("0xramai", 3_000, "100000"),
        ];
        let volume = vec![VolumePool {
            pool_id: "0xramai",
            volume: 1_000,
        }];
        let hasil = peringkat(&pools, &volume);
        assert_eq!(hasil[0].pool_id, "0xramai");
        assert_eq!(hasil[1].pool_id, "0xsepi");
        assert!(hasil[1].catatan.contains("belum ada perdagangan"));
    }

    #[test]
    fn fee_dinamis_ditandai_bukan_ditebak_tinggi() {
        // Menebaknya tinggi akan menaikkan peringkat pool yang justru paling
        // tidak kita ketahui.
        let mut p = pool("0xdinamis", crate::pools::DYNAMIC_FEE_FLAG, "1000");
        p.dynamic_fee = true;
        let hasil = peringkat(
            &[p],
            &[VolumePool {
                pool_id: "0xdinamis",
                volume: 1_000_000,
            }],
        );
        assert!(hasil[0].dynamic_fee);
        assert!(hasil[0].catatan.contains("hook"));
        // Proyeksinya memakai perkiraan konservatif, bukan penanda 0x800000.
        let dengan_penanda =
            1_000_000.0 * (crate::pools::DYNAMIC_FEE_FLAG as f64 / 1_000_000.0) / 1000.0;
        assert!(hasil[0].proyeksi < dengan_penanda / 100.0);
    }

    #[test]
    fn likuiditas_nol_tidak_membagi_dengan_nol() {
        let hasil = peringkat(
            &[pool("0xkosong", 3_000, "0")],
            &[VolumePool {
                pool_id: "0xkosong",
                volume: 1_000,
            }],
        );
        assert_eq!(hasil[0].proyeksi, 0.0);
    }

    #[test]
    fn keunggulan_dilaporkan_kalau_bukan_yang_paling_likuid() {
        let pools = vec![
            pool("0xtipis", 10_000, "1000"),
            pool("0xtebal", 3_000, "100000"),
        ];
        let volume = vec![
            VolumePool {
                pool_id: "0xtipis",
                volume: 100_000,
            },
            VolumePool {
                pool_id: "0xtebal",
                volume: 1_000_000,
            },
        ];
        let hasil = peringkat(&pools, &volume);
        let (id, kali) = keunggulan_terhadap_paling_likuid(&hasil).unwrap();
        assert_eq!(id, "0xtebal");
        assert!(kali > 1.0, "keunggulan {kali}");
    }

    #[test]
    fn tanpa_keunggulan_kalau_teratas_juga_paling_likuid() {
        let pools = vec![pool("0xa", 10_000, "100000"), pool("0xb", 3_000, "1000")];
        let volume = vec![
            VolumePool {
                pool_id: "0xa",
                volume: 10_000_000,
            },
            VolumePool {
                pool_id: "0xb",
                volume: 1,
            },
        ];
        let hasil = peringkat(&pools, &volume);
        assert_eq!(hasil[0].pool_id, "0xa");
        assert_eq!(keunggulan_terhadap_paling_likuid(&hasil), None);
    }

    #[test]
    fn urutan_stabil_saat_proyeksinya_sama() {
        // Tanpa pemutus seri, urutannya bisa berubah tiap pemuatan dan kartunya
        // terlihat berkedip tanpa alasan.
        let pools = vec![pool("0xb", 3_000, "1000"), pool("0xa", 3_000, "1000")];
        let volume = vec![
            VolumePool {
                pool_id: "0xa",
                volume: 1_000,
            },
            VolumePool {
                pool_id: "0xb",
                volume: 1_000,
            },
        ];
        let hasil = peringkat(&pools, &volume);
        assert_eq!(hasil[0].pool_id, "0xa");
        assert_eq!(hasil[1].pool_id, "0xb");
    }

    #[test]
    fn pencocokan_id_tidak_peduli_besar_kecil_huruf() {
        let hasil = peringkat(
            &[pool("0xAbC", 3_000, "1000")],
            &[VolumePool {
                pool_id: "0xabc",
                volume: 5_000,
            }],
        );
        assert_eq!(hasil[0].volume, "5000");
    }
}
