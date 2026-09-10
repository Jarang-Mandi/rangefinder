//! Gate G6: pembatas nilai dan mode dry run.
//!
//! INVARIAN
//! - Pembatas ditegakkan DI ENGINE, bukan di panel. Panel bisa dimanipulasi,
//!   engine tidak. Lihat PRD 9.2.
//! - Keempat pembatas nominal bawaannya tidak terbatas atas keputusan user yang
//!   sudah dikunci. Jangan menawarkan ulang untuk menyalakannya. Mekanismenya
//!   tetap ada supaya user bisa menyalakan sendiri kapan saja.
//! - Dua pembatas operasional, yaitu cadangan ETH dan slippage, TETAP aktif
//!   secara bawaan. Sifatnya berbeda: kehabisan gas berarti posisi tidak bisa
//!   ditutup, dan itu kegagalan operasional, bukan pilihan strategi.
//! - Dry run menyala secara bawaan. Proyek ini belum pernah mengirim satu
//!   transaksi pun, jadi mengirim sungguhan harus jadi tindakan sadar yang
//!   dinyalakan user, bukan bawaan yang kebetulan aktif.

use crate::config::LimitsConfig;

/// Keadaan yang diperiksa terhadap pembatas.
#[derive(Debug, Clone)]
pub struct LimitContext {
    /// Nilai posisi yang akan dibuka, dalam USD.
    pub position_usd: f64,
    /// Total yang sudah dikerahkan hari ini, belum termasuk posisi ini.
    pub daily_spent_usd: f64,
    pub open_positions: u32,
    /// Benar kalau pool ini belum pernah dipakai sebelumnya.
    pub is_new_pool: bool,
    /// Saldo ETH sekarang, dalam satuan ETH.
    pub eth_balance: f64,
    /// Perkiraan ETH yang akan terpakai, termasuk gas.
    pub eth_needed: f64,
}

/// GATE G6. Periksa seluruh pembatas sekaligus.
///
/// Mengembalikan semua pelanggaran, bukan yang pertama saja, supaya user tidak
/// memperbaiki satu lalu menemukan yang berikutnya.
pub fn check(cfg: &LimitsConfig, ctx: &LimitContext) -> Result<(), Vec<String>> {
    let mut pelanggaran = Vec::new();

    if let Some(batas) = cfg.max_position_usd {
        if ctx.position_usd > batas {
            pelanggaran.push(format!(
                "nilai posisi {:.2} USD melebihi batas per posisi {:.2} USD",
                ctx.position_usd, batas
            ));
        }
    }

    if let Some(batas) = cfg.max_daily_deploy_usd {
        let total = ctx.daily_spent_usd + ctx.position_usd;
        if total > batas {
            pelanggaran.push(format!(
                "total hari ini akan menjadi {total:.2} USD, melebihi batas harian {batas:.2} USD"
            ));
        }
    }

    if let Some(batas) = cfg.max_open_positions {
        if ctx.open_positions >= batas {
            pelanggaran.push(format!(
                "sudah ada {} posisi terbuka, batasnya {batas}",
                ctx.open_positions
            ));
        }
    }

    if ctx.is_new_pool {
        if let Some(batas) = cfg.new_pool_first_size_usd {
            if ctx.position_usd > batas {
                pelanggaran.push(format!(
                    "posisi pertama di pool ini dibatasi {batas:.2} USD, diminta {:.2} USD",
                    ctx.position_usd
                ));
            }
        }
    }

    // Pembatas operasional. Ini yang aktif secara bawaan.
    let sisa = ctx.eth_balance - ctx.eth_needed;
    if sisa < cfg.min_eth_reserve {
        pelanggaran.push(format!(
            "sisa ETH setelah transaksi hanya {sisa:.6}, di bawah cadangan {:.6}. \
             Tanpa cadangan gas, posisi tidak bisa ditutup nanti",
            cfg.min_eth_reserve
        ));
    }

    if pelanggaran.is_empty() {
        Ok(())
    } else {
        Err(pelanggaran)
    }
}

/// Terjemahkan batas slippage menjadi jumlah minimum yang masih diterima.
///
/// Dipakai saat menyusun `amount0Min` dan `amount1Min`. Nilai minimum inilah
/// yang membuat kontrak menolak transaksi kalau harga bergerak terlalu jauh
/// antara kutipan dan eksekusi.
pub fn apply_slippage(
    amount: alloy::primitives::U256,
    max_slippage_bps: u32,
) -> alloy::primitives::U256 {
    use alloy::primitives::U256;
    let bps = U256::from(10_000u32);
    let potongan = U256::from(max_slippage_bps.min(10_000));
    amount * (bps - potongan) / bps
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::U256;

    fn konteks() -> LimitContext {
        LimitContext {
            position_usd: 100.0,
            daily_spent_usd: 0.0,
            open_positions: 0,
            is_new_pool: false,
            eth_balance: 1.0,
            eth_needed: 0.5,
        }
    }

    #[test]
    fn bawaan_tanpa_batas_meloloskan_posisi_besar() {
        // Keputusan user yang sudah dikunci. Kalau uji ini gagal, seseorang
        // mengubah kebijakan tanpa membaca PRD 9.2.
        let cfg = LimitsConfig::default();
        let mut ctx = konteks();
        ctx.position_usd = 1_000_000.0;
        ctx.daily_spent_usd = 5_000_000.0;
        ctx.open_positions = 99;
        assert!(check(&cfg, &ctx).is_ok());
    }

    #[test]
    fn cadangan_eth_tetap_aktif_meski_pembatas_lain_mati() {
        // Ini yang membedakan pembatas operasional dari pembatas strategi.
        let cfg = LimitsConfig::default();
        let mut ctx = konteks();
        ctx.eth_balance = 0.5;
        ctx.eth_needed = 0.4999;
        let e = check(&cfg, &ctx).unwrap_err();
        assert!(e.iter().any(|m| m.contains("cadangan")));
    }

    #[test]
    fn batas_per_posisi_ditegakkan_saat_dinyalakan() {
        let cfg = LimitsConfig {
            max_position_usd: Some(50.0),
            ..Default::default()
        };
        let e = check(&cfg, &konteks()).unwrap_err();
        assert!(e.iter().any(|m| m.contains("batas per posisi")));
    }

    #[test]
    fn batas_harian_menghitung_posisi_yang_akan_dibuka() {
        let cfg = LimitsConfig {
            max_daily_deploy_usd: Some(150.0),
            ..Default::default()
        };
        let mut ctx = konteks();
        ctx.daily_spent_usd = 100.0;
        // 100 sudah terpakai ditambah 100 yang diminta melebihi 150.
        let e = check(&cfg, &ctx).unwrap_err();
        assert!(e.iter().any(|m| m.contains("batas harian")));

        // Tapat di batas masih boleh.
        ctx.daily_spent_usd = 50.0;
        assert!(check(&cfg, &ctx).is_ok());
    }

    #[test]
    fn batas_jumlah_posisi_memakai_lebih_besar_sama() {
        let cfg = LimitsConfig {
            max_open_positions: Some(3),
            ..Default::default()
        };
        let mut ctx = konteks();
        ctx.open_positions = 3;
        // Sudah tiga berarti membuka yang keempat, jadi ditolak.
        assert!(check(&cfg, &ctx).is_err());
        ctx.open_positions = 2;
        assert!(check(&cfg, &ctx).is_ok());
    }

    #[test]
    fn batas_pool_baru_hanya_berlaku_untuk_pool_baru() {
        let cfg = LimitsConfig {
            new_pool_first_size_usd: Some(10.0),
            ..Default::default()
        };
        let mut ctx = konteks();

        ctx.is_new_pool = false;
        assert!(check(&cfg, &ctx).is_ok(), "pool lama tidak kena batas ini");

        ctx.is_new_pool = true;
        let e = check(&cfg, &ctx).unwrap_err();
        assert!(e.iter().any(|m| m.contains("posisi pertama di pool ini")));
    }

    #[test]
    fn semua_pelanggaran_dilaporkan_sekaligus() {
        let cfg = LimitsConfig {
            max_position_usd: Some(10.0),
            max_daily_deploy_usd: Some(10.0),
            max_open_positions: Some(1),
            ..Default::default()
        };
        let mut ctx = konteks();
        ctx.open_positions = 5;
        ctx.eth_balance = 0.0;
        ctx.eth_needed = 0.0;

        let e = check(&cfg, &ctx).unwrap_err();
        assert!(e.len() >= 4, "harusnya banyak pelanggaran, dapat {e:?}");
    }

    #[test]
    fn slippage_menghasilkan_minimum_yang_benar() {
        let seribu = U256::from(1_000u32);
        // 300 basis poin sama dengan 3 persen, jadi minimumnya 970.
        assert_eq!(apply_slippage(seribu, 300), U256::from(970u32));
        // Nol slippage berarti minimum sama dengan jumlahnya.
        assert_eq!(apply_slippage(seribu, 0), seribu);
        // Slippage penuh berarti minimum nol.
        assert_eq!(apply_slippage(seribu, 10_000), U256::ZERO);
        // Nilai di atas seratus persen dijepit, bukan meluap jadi angka besar.
        assert_eq!(apply_slippage(seribu, 50_000), U256::ZERO);
    }

    #[test]
    fn slippage_tidak_meluap_pada_jumlah_besar() {
        let besar = U256::MAX / U256::from(20_000u32);
        let hasil = apply_slippage(besar, 300);
        assert!(hasil < besar);
        assert!(hasil > U256::ZERO);
    }
}
