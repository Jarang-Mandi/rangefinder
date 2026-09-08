//! Distribusi likuiditas per pita harga, sumber histogram di tab Range.
//!
//! KENAPA INI PENTING BAGI LP
//! Porsi fee yang kita terima bukan ditentukan oleh besarnya modal kita,
//! melainkan oleh perbandingan modal kita terhadap likuiditas lain YANG AKTIF
//! di pita tempat harga sedang bergerak. Dua range selebar sama bisa berbeda
//! hasilnya berkali lipat hanya karena yang satu ditaruh di pita yang sudah
//! padat dan yang lain di pita yang masih lengang.
//!
//! Tanpa gambar ini, satu satunya cara mengetahuinya adalah menaruh dana lalu
//! melihat hasilnya, dan itu cara belajar yang paling mahal.
//!
//! CARA MEMBACANYA DARI RANTAI
//! Likuiditas tidak disimpan per pita. Yang disimpan adalah `liquidityNet` di
//! setiap tick yang terinisialisasi, yaitu berapa banyak likuiditas yang mulai
//! atau berhenti aktif tepat di situ. Likuiditas aktif sebuah pita didapat
//! dengan berjalan dari tick sekarang ke luar sambil menjumlahkannya.
//!
//! Tick mana saja yang terinisialisasi diketahui dari bitmap, satu kata untuk
//! 256 tick terkompresi. Tanpa bitmap, mencarinya berarti mencoba setiap tick
//! satu per satu, dan rentang tick V4 mencakup ratusan ribu kemungkinan.
//!
//! Keempat fungsi yang dipakai sudah dibuktikan ada di bytecode StateView yang
//! terpasang lewat `cargo xtask v4probe`, bukan disalin dari repo Uniswap dan
//! diandaikan sama. Rantai ini sudah terbukti memakai fork pada UniversalRouter.
//!
//! INVARIAN
//! - Tinggi batang dinyatakan dalam token kutipan, bukan dalam satuan `L`.
//!   Angka `L` tidak punya satuan dan tidak bisa dibandingkan antar pool,
//!   sedangkan modal yang tertanam di sebuah pita bisa.
//! - Likuiditas tidak pernah dibiarkan negatif. Penjumlahan `liquidityNet` yang
//!   menghasilkan angka negatif berarti pembacaannya bolong, bukan berarti pool
//!   berutang likuiditas, dan menampilkannya apa adanya akan menggambar batang
//!   ke arah yang salah.
//! - Pita yang tidak terbaca dilewati, tidak dianggap kosong. Kosong berarti
//!   tidak ada yang menaruh dana di sana, kesimpulan yang sangat berbeda dari
//!   tidak bisa membaca, dan keduanya menuntun ke keputusan yang berlawanan.

use std::collections::HashMap;

use alloy::primitives::{Address, FixedBytes, I256, U256};
use alloy::providers::Provider;
use anyhow::{Context, Result};
use serde::Serialize;

use super::abi;
use crate::math::{liquidity_math, tick_math};

/// Berapa kata bitmap paling banyak dibaca sekali jalan.
///
/// Satu kata mencakup 256 tick terkompresi, yang pada `tick_spacing` 60 berarti
/// 15.360 tick atau sekitar 366 persen gerakan harga. Dua belas kata sudah jauh
/// melampaui rentang yang masuk akal untuk sebuah range, dan membatasinya
/// menjaga biaya pembacaan tetap satu batch.
pub const MAKS_KATA: i32 = 12;

/// Lebar harga yang ingin dicakup histogram, dalam persen ke tiap arah.
///
/// Dipilih supaya seluruh preset yang ditawarkan panel, sampai kurang lebih 20
/// persen, muat seluruhnya beserta ruang di sekelilingnya. Melihat sedikit di
/// luar batas yang dipilih itulah yang memberi tahu apakah range kita berada di
/// tepi tebing likuiditas atau di tengah lapangan kosong.
pub const SPAN_PERSEN: f64 = 35.0;

/// Satu pita distribusi.
#[derive(Debug, Clone, Serialize)]
pub struct Pita {
    pub tick_bawah: i32,
    pub tick_atas: i32,
    /// Harga terendah pita, dalam kutipan per memecoin.
    ///
    /// DINAMAI MENURUT HARGA, BUKAN MENURUT TICK
    /// Pada pool yang memecoinnya token1, tick yang lebih besar berarti harga
    /// memecoin yang lebih RENDAH. Menamai batas menurut ticknya membuat
    /// `harga_bawah` memuat angka yang lebih besar daripada `harga_atas` pada
    /// separuh pool, dan histogramnya tergambar terbalik tanpa satu pun error.
    /// Kekeliruan yang sama pernah terjadi pada batas range di `range`.
    pub harga_bawah: f64,
    pub harga_atas: f64,
    /// Likuiditas aktif di pita ini, apa adanya dari rantai.
    pub liquidity: String,
    /// Modal yang tertanam di pita ini, dalam token kutipan.
    ///
    /// Inilah tinggi batang. Ia bisa dibandingkan antar pita dan antar pool,
    /// sedangkan `liquidity` tidak.
    pub nilai_quote: f64,
    /// Benar kalau harga sekarang berada di dalam pita ini.
    pub aktif: bool,
}

/// Distribusi lengkap beserta konteks yang dibutuhkan untuk menggambarnya.
#[derive(Debug, Clone, Serialize)]
pub struct Distribusi {
    pub pool_id: String,
    pub tick_sekarang: i32,
    pub harga_sekarang: f64,
    pub tick_spacing: i32,
    pub pita: Vec<Pita>,
    /// Benar kalau memecoin ada di sisi token0.
    ///
    /// Menentukan arah sumbu: kalau salah, histogramnya tergambar terbalik dan
    /// user akan menaruh range di sisi yang berlawanan dari yang dimaksud.
    pub quote_adalah_token1: bool,
    /// Terisi kalau distribusinya tidak bisa disusun.
    pub error: Option<String>,
}

/// Ubah tick menjadi indeks terkompresi, membulat ke bawah.
///
/// Pembulatan ke bawah harus berlaku juga untuk tick negatif. Pembagian bilangan
/// bulat di Rust membulat ke arah nol, sehingga `-59 / 60` menghasilkan 0
/// padahal jawabannya -1. Kekeliruan satu di sini menggeser seluruh bitmap satu
/// kata dan menghasilkan histogram yang rapi tapi salah tempat.
pub fn kompres(tick: i32, spacing: i32) -> i32 {
    let mut c = tick / spacing;
    if tick < 0 && tick % spacing != 0 {
        c -= 1;
    }
    c
}

/// Posisi kata dan bit sebuah tick terkompresi.
pub fn posisi(terkompresi: i32) -> (i16, u8) {
    // Pergeseran aritmetika, bukan logis, supaya tick negatif tetap benar.
    let kata = terkompresi >> 8;
    let bit = (terkompresi & 0xff) as u8;
    (kata as i16, bit)
}

/// Dari mana data per tick dibaca.
///
/// V3 dan V4 menyimpan bitmap dan likuiditas tick dengan bentuk yang sama,
/// hanya tempatnya berbeda: V3 di kontrak pool itu sendiri, V4 di PoolManager
/// yang dibaca lewat StateView beserta poolId. Perakitan pitanya sama persis,
/// jadi yang dipisahkan hanya pembacaannya.
///
/// Sebelum ini seluruh jalur mengandaikan V4, dan setiap pool V3 selalu
/// melaporkan distribusi gagal dibaca.
#[derive(Debug, Clone, Copy)]
pub enum SumberTick {
    /// Dibaca dari StateView memakai poolId.
    V4 { state_view: Address },
    /// Dibaca langsung dari kontrak poolnya.
    V3 { pool: Address },
}

impl SumberTick {
    fn panggil_bitmap(&self, id: FixedBytes<32>, kata: i16) -> (Address, alloy::primitives::Bytes) {
        match self {
            SumberTick::V4 { state_view } => crate::chain::multicall::encode(
                *state_view,
                &abi::IStateView::getTickBitmapCall { poolId: id, tick: kata },
            ),
            SumberTick::V3 { pool } => crate::chain::multicall::encode(
                *pool,
                &abi::IUniswapV3Pool::tickBitmapCall {
                    wordPosition: kata,
                },
            ),
        }
    }

    fn urai_bitmap(&self, keluaran: Option<&alloy::primitives::Bytes>) -> Option<U256> {
        match self {
            SumberTick::V4 { .. } => {
                crate::chain::multicall::decode::<abi::IStateView::getTickBitmapCall>(keluaran)
            }
            SumberTick::V3 { .. } => {
                crate::chain::multicall::decode::<abi::IUniswapV3Pool::tickBitmapCall>(keluaran)
            }
        }
    }

    fn panggil_tick(&self, id: FixedBytes<32>, tick: i32) -> (Address, alloy::primitives::Bytes) {
        let t = alloy::primitives::aliases::I24::try_from(tick).unwrap_or_default();
        match self {
            SumberTick::V4 { state_view } => crate::chain::multicall::encode(
                *state_view,
                &abi::IStateView::getTickLiquidityCall { poolId: id, tick: t },
            ),
            SumberTick::V3 { pool } => crate::chain::multicall::encode(
                *pool,
                &abi::IUniswapV3Pool::ticksCall { tick: t },
            ),
        }
    }

    fn urai_net(&self, keluaran: Option<&alloy::primitives::Bytes>) -> Option<i128> {
        match self {
            SumberTick::V4 { .. } => {
                crate::chain::multicall::decode::<abi::IStateView::getTickLiquidityCall>(keluaran)
                    .and_then(|v| v.liquidityNet.to_string().parse::<i128>().ok())
            }
            SumberTick::V3 { .. } => {
                crate::chain::multicall::decode::<abi::IUniswapV3Pool::ticksCall>(keluaran)
                    .and_then(|v| v.liquidityNet.to_string().parse::<i128>().ok())
            }
        }
    }
}

/// Susun distribusi likuiditas di sekitar harga sekarang.
/// Keadaan pool yang dibutuhkan untuk menyusun distribusinya.
///
/// Dikelompokkan supaya tanda tangannya tidak menjadi deretan argumen yang
/// mudah tertukar. Tiga di antaranya bertipe `i32` dan dua bertipe `f64`,
/// sehingga tertukarnya tidak akan memunculkan error apa pun.
#[derive(Debug, Clone, Copy)]
pub struct KeadaanPool {
    pub tick_spacing: i32,
    pub tick_sekarang: i32,
    pub likuiditas: u128,
    /// Benar kalau kutipannya ada di sisi token1.
    pub quote_adalah_token1: bool,
    /// Sepuluh pangkat desimal memecoin.
    pub skala_meme: f64,
    /// Sepuluh pangkat desimal kutipan.
    pub skala_quote: f64,
}

pub async fn susun<P: Provider>(
    provider: &P,
    sumber: SumberTick,
    multicall: Address,
    pool_id: &str,
    k: KeadaanPool,
) -> Result<Distribusi> {
    let KeadaanPool {
        tick_spacing,
        tick_sekarang,
        likuiditas: likuiditas_sekarang,
        quote_adalah_token1,
        skala_meme,
        skala_quote,
    } = k;

    // Pada V3 pengenal poolnya adalah ALAMAT, bukan bytes32, dan ia memang
    // tidak dipakai dalam panggilan mana pun karena datanya ada di kontrak
    // poolnya sendiri. Penguraian yang gagal di sana bukan kesalahan.
    let id: FixedBytes<32> = match sumber {
        SumberTick::V4 { .. } => pool_id
            .parse()
            .with_context(|| format!("`{pool_id}` bukan poolId bytes32"))?,
        SumberTick::V3 { .. } => FixedBytes::<32>::ZERO,
    };

    let harga_sekarang = harga_di_tick(tick_sekarang, quote_adalah_token1, skala_meme, skala_quote);

    let kosong = |error: Option<String>| Distribusi {
        pool_id: pool_id.to_string(),
        tick_sekarang,
        harga_sekarang,
        tick_spacing,
        pita: Vec::new(),
        quote_adalah_token1,
        error,
    };

    if tick_spacing <= 0 {
        return Ok(kosong(Some("tick spacing tidak masuk akal".into())));
    }

    // Rentang tick yang ingin dicakup, lalu kata bitmap yang memuatnya.
    let rentang_tick = (SPAN_PERSEN / 100.0 + 1.0).ln() / 1.0001_f64.ln();
    let rentang_tick = rentang_tick as i32;

    let kata_tengah = posisi(kompres(tick_sekarang, tick_spacing)).0 as i32;
    let kata_bawah = posisi(kompres(tick_sekarang - rentang_tick, tick_spacing)).0 as i32;
    let kata_atas = posisi(kompres(tick_sekarang + rentang_tick, tick_spacing)).0 as i32;

    let dari_kata = kata_bawah.max(kata_tengah - MAKS_KATA / 2);
    let sampai_kata = kata_atas.min(kata_tengah + MAKS_KATA / 2);

    // Satu batch untuk seluruh kata bitmap.
    let mut calls = Vec::new();
    for k in dari_kata..=sampai_kata {
        calls.push(sumber.panggil_bitmap(id, k as i16));
    }

    let hasil = crate::chain::multicall::batch_call(provider, multicall, calls).await?;

    let mut tick_aktif: Vec<i32> = Vec::new();
    for (n, k) in (dari_kata..=sampai_kata).enumerate() {
        let Some(kata) = sumber.urai_bitmap(hasil.get(n).and_then(|x| x.as_ref())) else {
            continue;
        };
        if kata.is_zero() {
            continue;
        }
        for bit in 0..256u32 {
            if kata.bit(bit as usize) {
                let terkompresi = k * 256 + bit as i32;
                tick_aktif.push(terkompresi * tick_spacing);
            }
        }
    }

    if tick_aktif.is_empty() {
        return Ok(kosong(Some(
            "tidak ada tick terinisialisasi pada rentang ini".into(),
        )));
    }

    tick_aktif.sort_unstable();

    // Satu batch lagi untuk liquidityNet setiap tick yang terinisialisasi.
    let mut calls = Vec::with_capacity(tick_aktif.len());
    for t in &tick_aktif {
        calls.push(sumber.panggil_tick(id, *t));
    }

    let hasil = crate::chain::multicall::batch_call(provider, multicall, calls).await?;

    let mut net: Vec<(i32, i128)> = Vec::with_capacity(tick_aktif.len());
    for (n, t) in tick_aktif.iter().enumerate() {
        // Dilewati, bukan dianggap nol. Nol berarti tick itu tidak mengubah
        // likuiditas apa pun, dan itu kesimpulan yang berbeda.
        let Some(v) = sumber.urai_net(hasil.get(n).and_then(|x| x.as_ref())) else {
            continue;
        };
        net.push((*t, v));
    }

    // PITA PENUTUP DIPOTONG DI BATAS TAMPILAN, BUKAN DI TEPI KATA BITMAP
    //
    // Satu kata bitmap bisa mencakup ratusan persen gerakan harga. Pita penutup
    // yang membentang sejauh itu akan menghasilkan batang tertinggi di seluruh
    // histogram semata mata karena lebarnya, bukan karena padat, dan itu
    // menyesatkan justru pada bagian yang paling ingin dibaca user.
    //
    // Likuiditasnya tetap benar. Yang dipotong hanya rentang yang ditampilkan,
    // sampai sejauh histogram ini memang berniat menggambarkan.
    let batas_bawah_pindai = (tick_sekarang - rentang_tick).max(dari_kata * 256 * tick_spacing);
    let batas_atas_pindai =
        (tick_sekarang + rentang_tick).min(((sampai_kata + 1) * 256 - 1) * tick_spacing);

    let pita = rangkai(
        &net,
        tick_sekarang,
        likuiditas_sekarang,
        batas_bawah_pindai,
        batas_atas_pindai,
        quote_adalah_token1,
        skala_meme,
        skala_quote,
    );

    Ok(Distribusi {
        pool_id: pool_id.to_string(),
        tick_sekarang,
        harga_sekarang,
        tick_spacing,
        pita,
        quote_adalah_token1,
        error: None,
    })
}

/// Berapa kata bitmap paling banyak dibaca saat menjumlahkan TVL.
///
/// DULU JAUH LEBIH LEBAR, DAN ITU KELIRU
/// Percobaan pertama membaca delapan kata penuh, yang pada tick spacing 200
/// mencakup empat ratus ribu tick. Likuiditas yang dijumlahkan sejauh itu
/// berhenti menggambarkan apa pun: tick penutupnya berada di luar jendela,
/// sehingga likuiditasnya tidak pernah berkurang dan pita terluar memuat token
/// dalam jumlah yang mustahil. Satu pool V3 dilaporkan berisi 50 juta WETH,
/// jauh melebihi seluruh market cap tokennya.
///
/// Sekarang jendelanya sama dengan histogram, dan artinya menjadi jelas: modal
/// yang berada cukup dekat dengan harga untuk benar benar menerima fee.
pub const MAKS_KATA_TVL: i32 = 4;

/// Jumlahkan modal yang tertanam di beberapa pool sekaligus.
///
/// KENAPA SEKALIGUS
/// Satu pool butuh dua perjalanan ke rantai: bitmap dulu, lalu likuiditas tiap
/// tick yang terinisialisasi. Dikerjakan satu per satu, enam pool berarti dua
/// belas perjalanan. Dikerjakan bersama, tetap dua, berapa pun jumlah poolnya.
///
/// Nilainya dikembalikan dalam token kutipan, dengan alasan yang sama seperti
/// di seluruh modul ini: angka likuiditas mentah tidak punya satuan.
///
/// Pool yang gagal dibaca TIDAK muncul di hasil, bukan muncul dengan nilai nol.
/// Nol berarti poolnya kosong, kesimpulan yang sangat berbeda.
pub async fn tvl_beberapa<P: Provider>(
    provider: &P,
    multicall: Address,
    pools: &[(String, SumberTick, KeadaanPool)],
) -> HashMap<String, f64> {
    let mut out: HashMap<String, f64> = HashMap::new();
    if pools.is_empty() {
        return out;
    }

    // TAHAP SATU: bitmap seluruh pool dalam satu batch.
    let mut calls = Vec::new();
    let mut rencana: Vec<(usize, String, SumberTick, KeadaanPool, i32, i32)> = Vec::new();
    for (id, sumber, k) in pools {
        // Pengenal pool V3 adalah alamat, bukan bytes32, dan ia memang tidak
        // dipakai dalam panggilan mana pun di jalur itu.
        let bytes = match sumber {
            SumberTick::V4 { .. } => match id.parse::<FixedBytes<32>>() {
                Ok(b) => b,
                Err(_) => continue,
            },
            SumberTick::V3 { .. } => FixedBytes::<32>::ZERO,
        };
        if k.tick_spacing <= 0 {
            continue;
        }
        // Jendela yang sama dengan histogram, dibatasi jumlah kata supaya
        // biayanya tetap terkendali pada pool bertick spacing kecil.
        let rentang = ((SPAN_PERSEN / 100.0 + 1.0).ln() / 1.0001_f64.ln()) as i32;
        let tengah = posisi(kompres(k.tick_sekarang, k.tick_spacing)).0 as i32;
        let bawah = posisi(kompres(k.tick_sekarang - rentang, k.tick_spacing)).0 as i32;
        let atas = posisi(kompres(k.tick_sekarang + rentang, k.tick_spacing)).0 as i32;
        let dari = bawah.max(tengah - MAKS_KATA_TVL / 2);
        let sampai = atas.min(tengah + MAKS_KATA_TVL / 2);
        let awal = calls.len();
        for w in dari..=sampai {
            calls.push(sumber.panggil_bitmap(bytes, w as i16));
        }
        rencana.push((awal, id.clone(), *sumber, *k, dari, sampai));
    }

    let Ok(hasil) = crate::chain::multicall::batch_call(provider, multicall, calls).await else {
        tracing::warn!("bitmap TVL gagal dibaca, TVL dilewati");
        return out;
    };

    // TAHAP DUA: likuiditas tiap tick yang terinisialisasi, juga satu batch.
    let mut calls = Vec::new();
    let mut rencana2: Vec<(usize, usize, String, SumberTick, KeadaanPool, Vec<i32>)> = Vec::new();
    for (awal, id, sumber, k, dari, sampai) in rencana {
        let bytes = match sumber {
            SumberTick::V4 { .. } => match id.parse::<FixedBytes<32>>() {
                Ok(b) => b,
                Err(_) => continue,
            },
            SumberTick::V3 { .. } => FixedBytes::<32>::ZERO,
        };
        let mut ticks = Vec::new();
        for (n, w) in (dari..=sampai).enumerate() {
            let Some(kata) = sumber.urai_bitmap(hasil.get(awal + n).and_then(|x| x.as_ref()))
            else {
                continue;
            };
            if kata.is_zero() {
                continue;
            }
            for bit in 0..256usize {
                if kata.bit(bit) {
                    ticks.push((w * 256 + bit as i32) * k.tick_spacing);
                }
            }
        }
        ticks.sort_unstable();
        let rentang_tick = ((SPAN_PERSEN / 100.0 + 1.0).ln() / 1.0001_f64.ln()) as i32;
        ticks.retain(|t| (*t - k.tick_sekarang).abs() <= rentang_tick);

        // Pool yang tidak punya tick terinisialisasi di dalam jendela TIDAK
        // dilewati. Likuiditas yang sedang aktif membentang dari tick di luar
        // jendela, jadi modalnya tetap nyata dan tetap menerima fee. Melewatinya
        // membuat TVL terbaca nol, dan nol berarti pool kosong.
        let awal2 = calls.len();
        for t in &ticks {
            calls.push(sumber.panggil_tick(bytes, *t));
        }
        rencana2.push((awal2, ticks.len(), id, sumber, k, ticks));
    }

    // Daftar kosong berarti tidak ada satu pun tick terinisialisasi di dalam
    // jendela mana pun. Itu BUKAN alasan untuk berhenti: likuiditas yang sedang
    // aktif membentang dari tick di luar jendela, dan modalnya tetap nyata.
    // Percobaan sebelumnya keluar di sini, dan setiap pool seperti itu
    // melaporkan TVL nol, yang terbaca sebagai pool kosong.
    let hasil2 = if calls.is_empty() {
        Vec::new()
    } else {
        match crate::chain::multicall::batch_call(provider, multicall, calls).await {
            Ok(v) => v,
            Err(_) => {
                tracing::warn!("likuiditas tick TVL gagal dibaca, TVL dilewati");
                return out;
            }
        }
    };

    for (awal, jumlah, id, sumber, k, ticks) in rencana2 {
        let mut net: Vec<(i32, i128)> = Vec::with_capacity(jumlah);
        for (n, t) in ticks.iter().enumerate() {
            let Some(v) = sumber.urai_net(hasil2.get(awal + n).and_then(|x| x.as_ref())) else {
                continue;
            };
            net.push((*t, v));
        }

        // Pita penutup dipotong di batas jendela, bukan di tick terjauh yang
        // kebetulan terinisialisasi. Alasannya sama dengan di histogram: pita
        // yang sangat lebar menghasilkan angka besar karena lebarnya, bukan
        // karena isinya.
        let rentang = ((SPAN_PERSEN / 100.0 + 1.0).ln() / 1.0001_f64.ln()) as i32;
        let batas_bawah = k.tick_sekarang - rentang;
        let batas_atas = k.tick_sekarang + rentang;
        let pita = rangkai(
            &net,
            k.tick_sekarang,
            k.likuiditas,
            batas_bawah,
            batas_atas,
            k.quote_adalah_token1,
            k.skala_meme,
            k.skala_quote,
        );

        let total: f64 = pita.iter().map(|p| p.nilai_quote).sum();
        if total.is_finite() {
            out.insert(id.to_lowercase(), total);
        }
    }

    out
}

/// Rangkai pita dari daftar `liquidityNet`, berjalan keluar dari tick sekarang.
///
/// Murni, tanpa jaringan, karena inilah bagian yang paling mudah salah arah dan
/// paling sulit dilihat salahnya dari gambar yang sudah jadi. Histogram yang
/// terbalik tetap terlihat masuk akal.
#[allow(clippy::too_many_arguments)]
pub fn rangkai(
    net: &[(i32, i128)],
    tick_sekarang: i32,
    likuiditas_sekarang: u128,
    batas_bawah_pindai: i32,
    batas_atas_pindai: i32,
    quote_adalah_token1: bool,
    skala_meme: f64,
    skala_quote: f64,
) -> Vec<Pita> {
    let mut pita = Vec::new();

    // KE ATAS
    // Menyeberangi tick ke atas berarti likuiditas yang dimulai di situ menjadi
    // aktif dan yang berakhir di situ berhenti, yaitu `L += liquidityNet`.
    let mut l = likuiditas_sekarang as i128;
    let atas: Vec<&(i32, i128)> = net.iter().filter(|(t, _)| *t > tick_sekarang).collect();
    let mut batas_bawah = tick_sekarang;
    for (t, n) in &atas {
        dorong(
            &mut pita,
            batas_bawah,
            *t,
            l,
            tick_sekarang,
            quote_adalah_token1,
            skala_meme,
            skala_quote,
        );
        l += n;
        batas_bawah = *t;
    }
    // PITA PENUTUP
    //
    // Tanpa ini, pool yang seluruh tick terinisialisasinya berada di satu sisi
    // harga tidak punya pita yang memuat harga sekarang sama sekali. Hasil
    // sungguhan pada FIG menampilkan empat pita yang semuanya di atas harga,
    // dan tidak satu pun ditandai aktif.
    //
    // Likuiditas di sisa rentang itu bukan nol: ia justru likuiditas yang
    // sedang aktif sekarang, yang membentang sampai tick terinisialisasi
    // berikutnya di luar jendela yang kita baca.
    dorong(
        &mut pita,
        batas_bawah,
        batas_atas_pindai,
        l,
        tick_sekarang,
        quote_adalah_token1,
        skala_meme,
        skala_quote,
    );

    // KE BAWAH
    // Arah sebaliknya: menyeberangi tick ke bawah berarti `L -= liquidityNet`.
    let mut l = likuiditas_sekarang as i128;
    let mut bawah: Vec<&(i32, i128)> = net.iter().filter(|(t, _)| *t <= tick_sekarang).collect();
    bawah.sort_by_key(|(t, _)| std::cmp::Reverse(*t));
    let mut batas_atas = tick_sekarang;
    for (t, n) in &bawah {
        dorong(
            &mut pita,
            *t,
            batas_atas,
            l,
            tick_sekarang,
            quote_adalah_token1,
            skala_meme,
            skala_quote,
        );
        l -= n;
        batas_atas = *t;
    }
    dorong(
        &mut pita,
        batas_bawah_pindai,
        batas_atas,
        l,
        tick_sekarang,
        quote_adalah_token1,
        skala_meme,
        skala_quote,
    );

    pita.sort_by_key(|p: &Pita| p.tick_bawah);
    pita
}

#[allow(clippy::too_many_arguments)]
fn dorong(
    keluaran: &mut Vec<Pita>,
    tick_bawah: i32,
    tick_atas: i32,
    l: i128,
    tick_sekarang: i32,
    quote_adalah_token1: bool,
    skala_meme: f64,
    skala_quote: f64,
) {
    if tick_atas <= tick_bawah {
        return;
    }
    // Likuiditas negatif berarti pembacaannya bolong, bukan berarti pool
    // berutang likuiditas. Menggambarnya apa adanya akan membalik batangnya.
    let l = l.max(0) as u128;

    // Kedua ujung dihitung lalu diurutkan menurut nilainya, bukan menurut
    // ticknya, karena arah keduanya berlawanan pada pool yang memecoinnya
    // token1.
    let a = harga_di_tick(tick_bawah, quote_adalah_token1, skala_meme, skala_quote);
    let b = harga_di_tick(tick_atas, quote_adalah_token1, skala_meme, skala_quote);
    let (harga_min, harga_maks) = if a <= b { (a, b) } else { (b, a) };

    keluaran.push(Pita {
        tick_bawah,
        tick_atas,
        harga_bawah: harga_min,
        harga_atas: harga_maks,
        liquidity: l.to_string(),
        nilai_quote: nilai_pita(
            tick_bawah,
            tick_atas,
            l,
            tick_sekarang,
            quote_adalah_token1,
            skala_meme,
            skala_quote,
        ),
        aktif: tick_sekarang >= tick_bawah && tick_sekarang < tick_atas,
    });
}

/// Modal yang tertanam di sebuah pita, dalam token kutipan.
///
/// Kedua sisi dihitung. Satu sisi saja akan membaca nol pada pita yang
/// seluruhnya berada di satu sisi harga, dan itu justru mayoritas pita.
///
/// SISI MEMECOIN DINILAI PADA HARGA SEKARANG
/// Percobaan pertama menilainya pada harga tengah pita, dan itu meledak pada
/// pita yang lebar: satu pool V3 dilaporkan berisi 100 juta WETH, karena pita
/// selebar puluhan ribu tick memuat memecoin dalam jumlah raksasa yang dinilai
/// pada harga puluhan kali lipat harga sekarang.
///
/// TVL berarti nilai pasar sekarang, bukan nilai pada harga hipotetis di masa
/// depan. Menilai seluruh isi pool pada harga sekarang juga membuat pita yang
/// jauh dari harga terlihat sebagaimana adanya: banyak token, nilai kecil.
fn nilai_pita(
    tick_bawah: i32,
    tick_atas: i32,
    l: u128,
    tick_sekarang: i32,
    quote_adalah_token1: bool,
    skala_meme: f64,
    skala_quote: f64,
) -> f64 {
    if l == 0 {
        return 0.0;
    }
    let (Ok(a), Ok(b)) = (
        tick_math::get_sqrt_ratio_at_tick(tick_bawah),
        tick_math::get_sqrt_ratio_at_tick(tick_atas),
    ) else {
        return 0.0;
    };

    let amount0 = liquidity_math::amount0_for_liquidity(a, b, l).unwrap_or(U256::ZERO);
    let amount1 = liquidity_math::amount1_for_liquidity(a, b, l).unwrap_or(U256::ZERO);

    let harga = harga_di_tick(tick_sekarang, quote_adalah_token1, skala_meme, skala_quote);

    let (quote, meme) = if quote_adalah_token1 {
        (amount1, amount0)
    } else {
        (amount0, amount1)
    };

    f64::from(quote) / skala_quote + f64::from(meme) / skala_meme * harga
}

/// Harga memecoin dalam kutipan pada sebuah tick.
///
/// Arahnya dibalik kalau memecoin ada di token1, dengan alasan yang sama seperti
/// di batas range: tick yang lebih besar berarti token0 menguat.
pub fn harga_di_tick(
    tick: i32,
    quote_adalah_token1: bool,
    skala_meme: f64,
    skala_quote: f64,
) -> f64 {
    let mentah = 1.0001_f64.powi(tick);
    if quote_adalah_token1 {
        mentah * skala_meme / skala_quote
    } else {
        (1.0 / mentah) * skala_meme / skala_quote
    }
}

/// Pemakaian `I256` dijaga tetap dipakai supaya impor tidak menganggur kalau
/// bentuk decoding berubah.
#[allow(dead_code)]
fn _tipe_terpakai(_: I256) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kompresi_membulat_ke_bawah_juga_untuk_tick_negatif() {
        // Pembagian bilangan bulat Rust membulat ke arah nol, sehingga -59 / 60
        // menghasilkan 0 padahal jawabannya -1. Kekeliruan satu di sini
        // menggeser seluruh bitmap satu kata, dan histogramnya tetap tergambar
        // rapi tapi di tempat yang salah.
        assert_eq!(kompres(120, 60), 2);
        assert_eq!(kompres(60, 60), 1);
        assert_eq!(kompres(59, 60), 0);
        assert_eq!(kompres(0, 60), 0);
        assert_eq!(kompres(-1, 60), -1);
        assert_eq!(kompres(-59, 60), -1);
        assert_eq!(kompres(-60, 60), -1);
        assert_eq!(kompres(-61, 60), -2);
    }

    #[test]
    fn posisi_memakai_geseran_aritmetika() {
        // Geseran logis pada bilangan negatif menghasilkan kata raksasa yang
        // positif, dan bitmapnya akan dibaca dari tempat yang sama sekali lain.
        assert_eq!(posisi(0), (0, 0));
        assert_eq!(posisi(255), (0, 255));
        assert_eq!(posisi(256), (1, 0));
        assert_eq!(posisi(-1), (-1, 255));
        assert_eq!(posisi(-256), (-1, 0));
        assert_eq!(posisi(-257), (-2, 255));
    }

    #[test]
    fn likuiditas_bertambah_ke_atas_dan_berkurang_ke_bawah() {
        // Inilah arah yang paling mudah tertukar, dan histogram yang terbalik
        // tetap terlihat masuk akal. Tick 100 menambah 50, tick -100 menambah
        // 30 saat diseberangi ke atas, yang berarti mengurangi 30 saat kita
        // berjalan ke bawah melewatinya.
        let net = [(-100i32, 30i128), (100i32, 50i128)];
        let pita = rangkai(&net, 0, 1_000, -5_000, 5_000, true, 1.0, 1.0);

        let atas = pita.iter().find(|p| p.tick_bawah == 0).expect("pita atas");
        assert_eq!(atas.liquidity, "1000", "di atas harga, sebelum tick 100");

        let bawah = pita
            .iter()
            .find(|p| p.tick_atas == 0)
            .expect("pita bawah");
        assert_eq!(bawah.liquidity, "1000", "tepat di bawah harga");
    }

    #[test]
    fn likuiditas_tidak_pernah_negatif() {
        // Penjumlahan yang menghasilkan negatif berarti pembacaannya bolong,
        // bukan berarti pool berutang likuiditas.
        let net = [(100i32, 5_000i128), (200i32, -9_000i128)];
        let pita = rangkai(&net, 0, 100, -5_000, 5_000, true, 1.0, 1.0);
        assert!(
            pita.iter().all(|p| p.liquidity.parse::<u128>().is_ok()),
            "seluruh likuiditas harus terbaca sebagai bilangan tak bertanda"
        );
    }

    #[test]
    fn harga_sekarang_selalu_tercakup_walau_tick_hanya_di_satu_sisi() {
        // Kasus nyata dari FIG: seluruh tick terinisialisasi berada di atas
        // harga, dan hasilnya empat pita yang tidak satu pun memuat harga
        // sekarang. Likuiditas di sisa rentang bukan nol, ia justru yang
        // sedang aktif.
        let net = [(100i32, 10i128), (200i32, 10i128)];
        let pita = rangkai(&net, 0, 1_000, -5_000, 5_000, true, 1.0, 1.0);
        assert_eq!(
            pita.iter().filter(|p| p.aktif).count(),
            1,
            "harga sekarang harus tercakup tepat satu pita"
        );

        // Sisi sebaliknya juga.
        let net = [(-200i32, 10i128), (-100i32, 10i128)];
        let pita = rangkai(&net, 0, 1_000, -5_000, 5_000, true, 1.0, 1.0);
        assert_eq!(pita.iter().filter(|p| p.aktif).count(), 1);
    }

    #[test]
    fn harga_bawah_selalu_lebih_kecil_daripada_harga_atas() {
        // Pada pool yang memecoinnya token1, tick yang lebih besar berarti
        // harga yang lebih rendah. Menamai batas menurut ticknya membuat
        // histogram tergambar terbalik tanpa satu pun error muncul.
        let net = [(-100i32, 10i128), (100i32, 10i128)];
        for terbalik in [true, false] {
            let pita = rangkai(&net, 0, 1_000, -5_000, 5_000, terbalik, 1.0, 1.0);
            for p in &pita {
                assert!(
                    p.harga_bawah <= p.harga_atas,
                    "pita {}..{} punya harga {} sampai {}",
                    p.tick_bawah,
                    p.tick_atas,
                    p.harga_bawah,
                    p.harga_atas
                );
            }
        }
    }

    #[test]
    fn tepat_satu_pita_yang_aktif() {
        // Pita aktif adalah tempat harga sekarang berada, dan hanya ada satu.
        // Kalau ada dua, batasnya tumpang tindih dan modal akan dihitung ganda.
        let net = [(-100i32, 10i128), (100i32, 10i128), (200i32, 10i128)];
        let pita = rangkai(&net, 0, 1_000, -5_000, 5_000, true, 1.0, 1.0);
        assert_eq!(pita.iter().filter(|p| p.aktif).count(), 1);
    }

    #[test]
    fn arah_harga_dibalik_saat_memecoin_di_token1() {
        // Tick naik berarti token0 menguat. Pada pool yang memecoinnya token1,
        // tick naik justru berarti memecoin melemah, jadi sumbunya terbalik.
        let naik = harga_di_tick(1_000, true, 1.0, 1.0);
        let turun = harga_di_tick(1_000, false, 1.0, 1.0);
        assert!(naik > 1.0, "memecoin di token0 menguat saat tick naik");
        assert!(turun < 1.0, "memecoin di token1 melemah saat tick naik");
    }

    #[test]
    fn pita_tidak_saling_tumpang_tindih() {
        let net = [(-200i32, 10i128), (-100i32, 10i128), (100i32, 10i128)];
        let pita = rangkai(&net, 0, 1_000, -5_000, 5_000, true, 1.0, 1.0);
        for w in pita.windows(2) {
            assert!(
                w[0].tick_atas <= w[1].tick_bawah,
                "pita {:?} dan {:?} tumpang tindih",
                w[0].tick_bawah,
                w[1].tick_bawah
            );
        }
    }
}
