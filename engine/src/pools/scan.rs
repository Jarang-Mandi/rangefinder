//! Pemindaian pool paling ramai, sumber daftar kandidat untuk tab Signals.
//!
//! KENAPA DARI RANTAI, BUKAN DARI DAFTAR GMGN
//! PRD 3.4 semula membayangkan pemindaian lewat API gmgn. Itu mungkin, tapi
//! bergantung pada endpoint privat yang membawa parameter fingerprint dan bisa
//! berubah kapan saja tanpa pemberitahuan.
//!
//! Volume swap on chain adalah sumber yang lebih baik untuk keperluan ini, dan
//! alasannya bukan sekadar keandalan: yang kita cari adalah pool yang membayar
//! fee, dan yang membayar fee adalah swap yang benar benar terjadi di pool itu.
//! Daftar peringkat gmgn mengukur perhatian pasar, yang berkorelasi tapi bukan
//! hal yang sama.
//!
//! INVARIAN
//! - Jendela pemindaian dijaga pendek. Satu jam penuh menghasilkan ratusan ribu
//!   log dan ditolak RPC. Jendela disebutkan bersama hasilnya, karena volume
//!   tanpa jendela tidak bisa dibandingkan.
//! - PoolKey untuk pool yang ramai dibaca dari event `Initialize`, disaring
//!   menurut daftar poolId. Tidak ada pencarian terbalik dari poolId ke
//!   PoolKey selain itu: poolId adalah hash, dan hash tidak bisa dibalik.
//! - Pool berfee mustahil dibuang di sini juga, dengan alasan yang sama seperti
//!   di penemuan pool biasa. Pool fee 80 persen punya volume nol dan tidak akan
//!   muncul, tapi pool fee nol bisa sangat ramai sambil membayar LP nol.

use std::collections::HashMap;

use alloy::primitives::FixedBytes;
use alloy::providers::Provider;
use alloy::rpc::types::Filter;
use alloy::sol_types::SolEvent;
use anyhow::{Context, Result};
use serde::Serialize;

use super::abi;
use crate::chain::registry::Registry;

/// Jendela pemindaian awal, dalam blok.
///
/// Diukur, bukan ditebak: jendela 2.000 blok pernah menghasilkan sekitar 8.600
/// log, masih di bawah batas 10.000 yang ditegakkan RPC ini.
///
/// Tapi keramaian rantai berubah sepanjang hari, dan jendela yang sama pernah
/// melewati batas beberapa jam kemudian. Karena itu angka ini hanya titik
/// awal, dan jendelanya menyempit sendiri saat ditolak.
pub const JENDELA_SCAN_AWAL: u64 = 2_000;

/// Jendela terkecil yang masih dicoba.
///
/// Di bawah ini, hasilnya terlalu pendek untuk menggambarkan apa pun: dua ratus
/// blok kira kira dua puluh detik, dan pool yang ramai sekalipun bisa kebetulan
/// sepi selama itu.
pub const JENDELA_SCAN_MINIMUM: u64 = 200;

/// Berapa pool teratas yang dikembalikan.
pub const MAKS_KANDIDAT: usize = 25;

/// Jejak sebuah pool sepanjang jendela, dikumpulkan sambil membaca log.
///
/// KENAPA TICK IKUT DIKUMPULKAN
/// Event `Swap` membawa tick setelah swap itu. Deret tick sepanjang jendela
/// adalah deret harga yang sesungguhnya, dari sumber yang paling berwenang,
/// dan ia tersedia tanpa satu pun permintaan tambahan.
///
/// Dari deret itu bisa dihitung rasio efisiensi Kaufman, yang membedakan pasar
/// menyamping dari pasar yang bergerak searah. Tanpa itu, seluruh baris hasil
/// pemindaian hanya bisa diberi label `Pantau`, karena detektor sinyal butuh
/// candle yang baru datang setelah user membuka halaman tokennya.
#[derive(Debug, Clone, Default)]
struct Jejak {
    swap: usize,
    volume0: u128,
    volume1: u128,
    tick_awal: Option<i32>,
    tick_akhir: i32,
    tick_min: i32,
    tick_maks: i32,
    /// Jumlah perpindahan tick tanpa memandang arah. Penyebut rasio efisiensi.
    jarak_tempuh: u64,
}

impl Jejak {
    fn catat(&mut self, tick: i32, a0: u128, a1: u128) {
        self.swap += 1;
        self.volume0 += a0;
        self.volume1 += a1;
        match self.tick_awal {
            None => {
                self.tick_awal = Some(tick);
                self.tick_min = tick;
                self.tick_maks = tick;
            }
            Some(_) => {
                self.jarak_tempuh += (tick - self.tick_akhir).unsigned_abs() as u64;
                self.tick_min = self.tick_min.min(tick);
                self.tick_maks = self.tick_maks.max(tick);
            }
        }
        self.tick_akhir = tick;
    }

    /// Rasio efisiensi Kaufman atas deret tick.
    ///
    /// Nol berarti harga kembali ke tempatnya semula setelah banyak bergerak,
    /// yaitu menyamping. Satu berarti setiap langkah menuju arah yang sama.
    ///
    /// `None` kalau jarak tempuhnya nol, yang berarti harga tidak bergerak sama
    /// sekali. Itu bukan menyamping, itu tidak ada perdagangan, dan menyebutnya
    /// nol akan membuat pool mati terlihat seperti pool paling ideal.
    fn efisiensi(&self) -> Option<f64> {
        let awal = self.tick_awal?;
        if self.jarak_tempuh == 0 {
            return None;
        }
        let bersih = (self.tick_akhir - awal).unsigned_abs() as f64;
        Some(bersih / self.jarak_tempuh as f64)
    }
}

/// Satu pool yang sedang ramai.
#[derive(Debug, Clone, Serialize)]
pub struct Kandidat {
    pub pool_id: String,
    pub token0: String,
    pub token1: String,
    pub fee: u32,
    pub dynamic_fee: bool,
    pub tick_spacing: i32,
    pub hooks: String,
    /// Berapa swap terjadi pada jendela yang diukur.
    pub swap: usize,
    /// Volume token0 pada jendela itu, dalam satuan terkecil.
    pub volume: String,
    /// Volume token1 pada jendela itu, dalam satuan terkecil.
    ///
    /// KENAPA KEDUA SISI DISIMPAN
    /// Kutipan bisa berada di sisi mana saja. Menyimpan satu sisi saja berarti
    /// separuh pool volumenya harus dikonversi lewat harga, dan konversi itu
    /// memasukkan kesalahan harga ke dalam angka yang seharusnya murni hasil
    /// penjumlahan.
    pub volume1: String,
    /// Blok saat pool dibuat, dari event `Initialize`.
    pub blok_dibuat: u64,
    /// Rasio efisiensi Kaufman atas deret tick sepanjang jendela.
    ///
    /// `None` berarti harga tidak bergerak sama sekali, yang berbeda dari
    /// menyamping dan tidak boleh diperlakukan sebagai nol.
    pub efisiensi: Option<f64>,
    pub tick_awal: i32,
    pub tick_akhir: i32,
    pub tick_min: i32,
    pub tick_maks: i32,
}

/// Hasil pemindaian beserta jendelanya.
#[derive(Debug, Clone, Serialize)]
pub struct HasilScan {
    pub kandidat: Vec<Kandidat>,
    pub dari_blok: u64,
    pub sampai_blok: u64,
    /// Total swap yang terbaca sebelum dipangkas ke pool teratas.
    pub total_swap: usize,
    pub dipindai_pada: String,
}

/// Pindai pool paling ramai pada jendela terakhir.
pub async fn pindai<P: Provider>(provider: &P, registry: &Registry) -> Result<HasilScan> {
    let pool_manager = registry.contract("uniswap_v4", "pool_manager")?;

    let latest = provider
        .get_block_number()
        .await
        .context("gagal membaca blok terakhir")?;
    // Penyempitan jendela dan penantian batas laju ditangani helper bersama.
    // Lihat chain::logs untuk alasan kenapa dua penolakan itu obatnya berbeda.
    let terbaca = crate::chain::logs::baca_mundur(
        provider,
        latest,
        registry.chain.deploy_block,
        JENDELA_SCAN_AWAL,
        JENDELA_SCAN_MINIMUM,
        |dari, sampai| {
            Filter::new()
                .address(pool_manager)
                .event_signature(abi::Swap::SIGNATURE_HASH)
                .from_block(dari)
                .to_block(sampai)
        },
    )
    .await
    .context("pemindaian Swap gagal")?;
    let dari = terbaca.dari_blok;
    let logs = terbaca.logs;

    let total_swap = logs.len();

    let mut hitung: HashMap<FixedBytes<32>, Jejak> = HashMap::new();
    for log in &logs {
        let Ok(d) = abi::Swap::decode_log(&log.inner) else {
            continue;
        };
        hitung.entry(d.id).or_default().catat(
            d.tick.as_i32(),
            d.amount0.unsigned_abs(),
            d.amount1.unsigned_abs(),
        );
    }

    // Diurutkan menurut jumlah swap, bukan volume. Volume didominasi satu dua
    // transaksi besar yang bisa saja pemindahan antar dompet, sedangkan jumlah
    // swap menggambarkan seberapa hidup pool itu.
    let mut urut: Vec<(FixedBytes<32>, Jejak)> = hitung.into_iter().collect();
    urut.sort_by(|a, b| {
        b.1.swap
            .cmp(&a.1.swap)
            .then_with(|| b.1.volume0.cmp(&a.1.volume0))
    });
    urut.truncate(MAKS_KANDIDAT);

    if urut.is_empty() {
        return Ok(HasilScan {
            kandidat: Vec::new(),
            dari_blok: dari,
            sampai_blok: latest,
            total_swap,
            dipindai_pada: chrono::Utc::now().to_rfc3339(),
        });
    }

    // PoolKey dibaca dari `Initialize`, disaring menurut daftar poolId. poolId
    // adalah hash dari PoolKey, dan hash tidak bisa dibalik, jadi ini satu
    // satunya jalan.
    let ids: Vec<FixedBytes<32>> = urut.iter().map(|(id, _)| *id).collect();
    let init = crate::chain::logs::baca_rentang(
        provider,
        &Filter::new()
            .address(pool_manager)
            .event_signature(abi::Initialize::SIGNATURE_HASH)
            .topic1(ids)
            .from_block(registry.chain.deploy_block)
            .to_block(latest),
        "pembacaan PoolKey pool ramai gagal",
    )
    .await?;

    let mut kunci: HashMap<FixedBytes<32>, (abi::Initialize, u64)> = HashMap::new();
    for log in &init {
        if let Ok(d) = abi::Initialize::decode_log(&log.inner) {
            // Blok pembuatan dicatat di sini karena inilah satu satunya tempat
            // ia tersedia. Umur pool termasuk penyaring paling menentukan di
            // playbook, dan membacanya nanti berarti satu pemindaian log lagi.
            kunci.insert(d.id, (d.data, log.block_number.unwrap_or(0)));
        }
    }

    let kandidat: Vec<Kandidat> = urut
        .into_iter()
        .filter_map(|(id, j)| {
            let (k, blok_dibuat) = kunci.get(&id)?;
            let fee = k.fee.to::<u32>();
            // Pool fee nol bisa sangat ramai sambil membayar LP nol. Ia harus
            // dibuang di sini juga, bukan hanya di penemuan pool biasa.
            if super::implausible_fee_raw(fee) {
                return None;
            }
            Some(Kandidat {
                pool_id: format!("0x{}", hex::encode(id)),
                token0: k.currency0.to_string(),
                token1: k.currency1.to_string(),
                fee,
                dynamic_fee: fee & super::DYNAMIC_FEE_FLAG != 0,
                tick_spacing: k.tickSpacing.as_i32(),
                hooks: k.hooks.to_string(),
                swap: j.swap,
                volume: j.volume0.to_string(),
                volume1: j.volume1.to_string(),
                blok_dibuat: *blok_dibuat,
                efisiensi: j.efisiensi(),
                tick_awal: j.tick_awal.unwrap_or(0),
                tick_akhir: j.tick_akhir,
                tick_min: j.tick_min,
                tick_maks: j.tick_maks,
            })
        })
        .collect();

    Ok(HasilScan {
        kandidat,
        dari_blok: dari,
        sampai_blok: latest,
        total_swap,
        dipindai_pada: chrono::Utc::now().to_rfc3339(),
    })
}

/// Sisi mana dari sebuah kandidat yang merupakan memecoin, bukan kutipan.
///
/// Mengembalikan `None` kalau kedua sisinya kutipan yang dikenal, misalnya pool
/// stablecoin terhadap ETH. Pool seperti itu bukan sasaran alat ini.
pub fn sisi_memecoin(k: &Kandidat, registry: &Registry) -> Option<String> {
    let kutipan = |a: &str| -> bool {
        a.eq_ignore_ascii_case(super::ZERO_ADDRESS)
            || registry
                .tokens
                .values()
                .any(|e| e.address.eq_ignore_ascii_case(a))
    };

    match (kutipan(&k.token0), kutipan(&k.token1)) {
        (true, false) => Some(k.token1.clone()),
        (false, true) => Some(k.token0.clone()),
        // Dua duanya kutipan, atau dua duanya bukan. Yang pertama bukan sasaran,
        // yang kedua tidak punya satuan yang berarti untuk menilai hasilnya.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kandidat(t0: &str, t1: &str) -> Kandidat {
        Kandidat {
            pool_id: "0x1".into(),
            token0: t0.into(),
            token1: t1.into(),
            fee: 3000,
            dynamic_fee: false,
            tick_spacing: 60,
            hooks: super::super::ZERO_ADDRESS.into(),
            swap: 10,
            volume: "1000".into(),
            volume1: "1000".into(),
            blok_dibuat: 1,
            efisiensi: Some(0.2),
            tick_awal: 0,
            tick_akhir: 0,
            tick_min: 0,
            tick_maks: 0,
        }
    }

    fn registry_uji() -> Registry {
        Registry::load_from(&Registry::default_path(&crate::config::repo_root()))
            .expect("chain.toml harus terbaca")
    }

    #[test]
    fn jendela_menyempit_punya_dasar() {
        // Jendela awal harus di atas minimumnya, kalau tidak penyempitannya
        // tidak pernah berjalan dan hasilnya selalu dari jendela terpendek.
        const { assert!(JENDELA_SCAN_AWAL > JENDELA_SCAN_MINIMUM) };
        // Minimum tidak boleh terlalu pendek: dua ratus blok kira kira dua
        // puluh detik, dan pool ramai pun bisa kebetulan sepi selama itu.
        const { assert!(JENDELA_SCAN_MINIMUM >= 100) };
    }

    #[test]
    fn memecoin_dikenali_dari_sisi_yang_bukan_kutipan() {
        let r = registry_uji();
        let usdg = r
            .tokens
            .get("usdg")
            .map(|e| e.address.clone())
            .expect("usdg ada di chain.toml");
        let meme = "0xdc4dd8839f02f2242e9552fe4652635fb7f95cc6";

        assert_eq!(sisi_memecoin(&kandidat(&usdg, meme), &r).as_deref(), Some(meme));
        assert_eq!(sisi_memecoin(&kandidat(meme, &usdg), &r).as_deref(), Some(meme));
    }

    #[test]
    fn eth_asli_dihitung_sebagai_kutipan() {
        let r = registry_uji();
        let meme = "0xdc4dd8839f02f2242e9552fe4652635fb7f95cc6";
        assert_eq!(
            sisi_memecoin(&kandidat(super::super::ZERO_ADDRESS, meme), &r).as_deref(),
            Some(meme)
        );
    }

    #[test]
    fn pool_dua_kutipan_bukan_sasaran() {
        // Pool stablecoin terhadap ETH bukan yang dicari alat ini.
        let r = registry_uji();
        let usdg = r.tokens.get("usdg").map(|e| e.address.clone()).unwrap();
        assert_eq!(sisi_memecoin(&kandidat(super::super::ZERO_ADDRESS, &usdg), &r), None);
    }

    #[test]
    fn pool_dua_memecoin_tidak_bisa_dinilai() {
        // Tidak ada satuan yang berarti untuk menilai hasilnya.
        let r = registry_uji();
        assert_eq!(
            sisi_memecoin(&kandidat("0xaaa1", "0xbbb2"), &r),
            None
        );
    }
}
