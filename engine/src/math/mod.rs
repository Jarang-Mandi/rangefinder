//! Konversi satuan antara chart gmgn dan tick Uniswap.
//!
//! Chart bekerja dalam market cap USD. Uniswap bekerja dalam tick, yang
//! berbasis harga mentah token1 per token0 pada satuan terkecil. Rantainya:
//!
//! ```text
//! mcap_usd -> price_usd -> price_weth -> harga manusiawi pool -> raw -> tick
//! ```
//!
//! INVARIAN
//! - **Urutan token menentukan arah.** `token0` adalah alamat yang lebih kecil
//!   secara numerik. Kalau memecoin ternyata token1, maka harga pool adalah
//!   memecoin per WETH, sehingga harga memecoin yang NAIK berarti harga pool
//!   TURUN. Akibatnya batas atas dan batas bawah bertukar tempat. Ini bug
//!   klasik yang gampang lolos dari uji manual karena hasilnya tetap terlihat
//!   masuk akal, hanya terbalik.
//! - **Selalu snap ke `tickSpacing` sebelum menampilkan.** Panel wajib
//!   menunjukkan harga hasil pembulatan tick, bukan harga mentah yang digeser
//!   user, supaya angka di layar sama persis dengan yang tercatat on chain.
//!   Lihat PRD 4.3.
//! - `tickSpacing` V4 dibaca dari PoolKey, tidak boleh diasumsikan dari fee
//!   tier seperti di V3.
//!
//! PRESISI
//! Konversi memakai `f64`. Untuk turunan tick dari harga itu memadai, karena
//! hasilnya selalu dibulatkan ke bilangan bulat lalu di-snap ke grid. Nilai
//! `sqrtPriceX96` yang dikirim ke rantai TIDAK boleh dihitung dari sini, dan
//! wajib memakai matematika bilangan bulat pada modul eksekusi.

pub mod liquidity_math;
pub mod tick_math;

/// Batas tick Uniswap V3 dan V4.
pub const MIN_TICK: i32 = -887_272;
pub const MAX_TICK: i32 = 887_272;

const TICK_BASE: f64 = 1.0001;

/// Orientasi pool terhadap memecoin yang sedang dilihat user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolOrientation {
    /// Benar kalau memecoin menempati slot token0.
    pub memecoin_is_token0: bool,
    pub decimals_token0: u8,
    pub decimals_token1: u8,
}

impl PoolOrientation {
    /// Tentukan orientasi dari dua alamat, mengikuti aturan Uniswap bahwa
    /// token0 adalah alamat yang lebih kecil secara numerik.
    pub fn from_addresses(
        memecoin: alloy::primitives::Address,
        quote: alloy::primitives::Address,
        memecoin_decimals: u8,
        quote_decimals: u8,
    ) -> Self {
        let memecoin_is_token0 = memecoin < quote;
        let (decimals_token0, decimals_token1) = if memecoin_is_token0 {
            (memecoin_decimals, quote_decimals)
        } else {
            (quote_decimals, memecoin_decimals)
        };
        Self {
            memecoin_is_token0,
            decimals_token0,
            decimals_token1,
        }
    }

    fn decimal_shift(&self) -> i32 {
        self.decimals_token1 as i32 - self.decimals_token0 as i32
    }
}

/// Fee tier V3 ke tick spacing.
///
/// Hanya berlaku untuk V3. Di V4, `tickSpacing` adalah bagian dari PoolKey dan
/// bisa bernilai apa saja, jadi wajib dibaca dari pool.
pub fn v3_tick_spacing(fee: u32) -> Option<i32> {
    match fee {
        100 => Some(1),
        500 => Some(10),
        3_000 => Some(60),
        10_000 => Some(200),
        _ => None,
    }
}

/// Harga USD per token dari market cap.
pub fn mcap_to_price_usd(mcap_usd: f64, total_supply: f64) -> Option<f64> {
    if total_supply <= 0.0 || !mcap_usd.is_finite() || mcap_usd < 0.0 {
        return None;
    }
    Some(mcap_usd / total_supply)
}

/// Kebalikannya, untuk menggambar garis kembali ke chart.
pub fn price_usd_to_mcap(price_usd: f64, total_supply: f64) -> Option<f64> {
    if total_supply <= 0.0 || !price_usd.is_finite() || price_usd < 0.0 {
        return None;
    }
    Some(price_usd * total_supply)
}

/// Harga memecoin dalam WETH.
pub fn price_usd_to_weth(price_usd: f64, eth_usd: f64) -> Option<f64> {
    if eth_usd <= 0.0 || !price_usd.is_finite() || price_usd <= 0.0 {
        return None;
    }
    Some(price_usd / eth_usd)
}

/// Harga manusiawi pool, yaitu token1 per token0.
///
/// Di sinilah pembalikan terjadi kalau memecoin menempati token1.
fn pool_price_from_memecoin_weth(price_weth: f64, o: PoolOrientation) -> Option<f64> {
    if !price_weth.is_finite() || price_weth <= 0.0 {
        return None;
    }
    Some(if o.memecoin_is_token0 {
        price_weth
    } else {
        1.0 / price_weth
    })
}

/// Harga memecoin dalam WETH dari harga mentah pool.
///
/// Kebalikan dari jalur yang dipakai `price_weth_to_tick`, dan ini yang dipakai
/// saat membaca keadaan pool dari `slot0`. Pembalikan akibat orientasi token
/// ditangani di sini juga, sehingga pemanggil selalu berpikir dalam harga
/// memecoin.
pub fn raw_price_to_price_weth(raw_price: f64, o: PoolOrientation) -> Option<f64> {
    if !raw_price.is_finite() || raw_price <= 0.0 {
        return None;
    }
    let human = raw_price * 10f64.powi(-o.decimal_shift());
    if !human.is_finite() || human <= 0.0 {
        return None;
    }
    let price = if o.memecoin_is_token0 {
        human
    } else {
        1.0 / human
    };
    price.is_finite().then_some(price)
}

/// Turunkan kurs ETH ke USD dari chart, bukan dari sumber luar.
///
/// Chart menampilkan market cap dalam USD, sedangkan pool memberi harga dalam
/// WETH. Membandingkan keduanya menghasilkan kurs yang tersirat:
///
/// ```text
/// eth_usd = mcap_usd / (total_supply x price_weth)
/// ```
///
/// KENAPA BUKAN SUMBER LUAR
/// Sumber harga mana pun akan sedikit berbeda dari yang dipakai gmgn, dan
/// selisih sekecil apa pun membuat garis mendarat di tempat yang bukan
/// ditunjukkan skala chart. Kalibrasi dari chart itu sendiri menghilangkan
/// seluruh kelas masalah itu.
pub fn calibrate_eth_usd(mcap_usd: f64, total_supply: f64, price_weth: f64) -> Option<f64> {
    if !(mcap_usd.is_finite() && mcap_usd > 0.0) {
        return None;
    }
    if !(total_supply.is_finite() && total_supply > 0.0) {
        return None;
    }
    if !(price_weth.is_finite() && price_weth > 0.0) {
        return None;
    }
    let eth_usd = mcap_usd / (total_supply * price_weth);
    (eth_usd.is_finite() && eth_usd > 0.0).then_some(eth_usd)
}

/// Market cap USD dari harga memecoin dalam WETH.
pub fn price_weth_to_mcap(price_weth: f64, eth_usd: f64, total_supply: f64) -> Option<f64> {
    let usd = price_weth_to_usd(price_weth, eth_usd)?;
    price_usd_to_mcap(usd, total_supply)
}

/// Harga USD dari harga dalam WETH.
pub fn price_weth_to_usd(price_weth: f64, eth_usd: f64) -> Option<f64> {
    if !(price_weth.is_finite() && price_weth > 0.0) {
        return None;
    }
    if !(eth_usd.is_finite() && eth_usd > 0.0) {
        return None;
    }
    Some(price_weth * eth_usd)
}

/// Harga memecoin dalam WETH dari market cap yang dibaca di chart.
pub fn mcap_to_price_weth(mcap_usd: f64, total_supply: f64, eth_usd: f64) -> Option<f64> {
    let usd = mcap_to_price_usd(mcap_usd, total_supply)?;
    price_usd_to_weth(usd, eth_usd)
}

/// Tick dari harga memecoin dalam WETH, belum di-snap.
pub fn price_weth_to_tick(price_weth: f64, o: PoolOrientation) -> Option<i32> {
    let human = pool_price_from_memecoin_weth(price_weth, o)?;
    let raw = human * 10f64.powi(o.decimal_shift());
    if !raw.is_finite() || raw <= 0.0 {
        return None;
    }
    let tick = (raw.ln() / TICK_BASE.ln()).round();
    if !tick.is_finite() || tick < MIN_TICK as f64 || tick > MAX_TICK as f64 {
        return None;
    }
    Some(tick as i32)
}

/// Harga memecoin dalam WETH dari sebuah tick.
pub fn tick_to_price_weth(tick: i32, o: PoolOrientation) -> Option<f64> {
    if !(MIN_TICK..=MAX_TICK).contains(&tick) {
        return None;
    }
    let raw = TICK_BASE.powi(tick);
    let human = raw * 10f64.powi(-o.decimal_shift());
    if !human.is_finite() || human <= 0.0 {
        return None;
    }
    Some(if o.memecoin_is_token0 {
        human
    } else {
        1.0 / human
    })
}

/// Bulatkan tick ke bawah pada grid.
pub fn floor_to_spacing(tick: i32, spacing: i32) -> i32 {
    debug_assert!(spacing > 0, "tick spacing harus positif");
    (tick as f64 / spacing as f64).floor() as i32 * spacing
}

/// Bulatkan tick ke atas pada grid.
pub fn ceil_to_spacing(tick: i32, spacing: i32) -> i32 {
    debug_assert!(spacing > 0, "tick spacing harus positif");
    (tick as f64 / spacing as f64).ceil() as i32 * spacing
}

/// Range yang sudah siap dikirim ke rantai.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickRange {
    pub lower: i32,
    pub upper: i32,
}

impl TickRange {
    pub fn width(&self) -> i32 {
        self.upper - self.lower
    }
}

/// Ubah dua harga memecoin dalam WETH menjadi range tick yang sudah di-snap.
///
/// Menerima kedua harga dalam urutan apa pun. Pembalikan akibat orientasi token
/// ditangani di sini, sehingga pemanggil cukup memikirkan harga memecoin dan
/// tidak perlu tahu siapa token0.
pub fn range_from_prices(
    price_a_weth: f64,
    price_b_weth: f64,
    o: PoolOrientation,
    spacing: i32,
) -> Option<TickRange> {
    if spacing <= 0 {
        return None;
    }
    let tick_a = price_weth_to_tick(price_a_weth, o)?;
    let tick_b = price_weth_to_tick(price_b_weth, o)?;

    let (raw_lower, raw_upper) = if tick_a <= tick_b {
        (tick_a, tick_b)
    } else {
        (tick_b, tick_a)
    };

    let lower = floor_to_spacing(raw_lower, spacing).max(MIN_TICK);
    let upper = ceil_to_spacing(raw_upper, spacing).min(MAX_TICK);

    // Kedua harga bisa jatuh di sel grid yang sama pada pool dengan spacing
    // lebar. Range dengan lebar nol tidak sah di Uniswap, jadi dilebarkan satu
    // sel. Pemanggil melihatnya lewat perbedaan harga hasil pembulatan.
    let upper = if upper <= lower { lower + spacing } else { upper };

    Some(TickRange { lower, upper })
}

/// Selisih antara harga yang diminta dan harga hasil pembulatan tick,
/// dinyatakan dalam basis poin. Panel menampilkannya supaya user tahu berapa
/// jauh grid menggeser permintaannya.
pub fn snap_drift_bps(requested_weth: f64, snapped_weth: f64) -> Option<f64> {
    if requested_weth <= 0.0 || !requested_weth.is_finite() || !snapped_weth.is_finite() {
        return None;
    }
    Some((snapped_weth - requested_weth) / requested_weth * 10_000.0)
}

// ─── Matematika likuiditas ──────────────────────────────────────
//
// Rumusnya identik untuk V3 dan V4, jadi satu modul melayani keduanya.
// Lihat PRD 6.1.

/// Jumlah token0 dan token1 yang dibutuhkan untuk likuiditas `l` pada range
/// `[pa, pb]` saat harga pool berada di `p`.
///
/// Seluruh harga di sini adalah harga pool mentah, bukan harga memecoin.
pub fn amounts_for_liquidity(l: f64, p: f64, pa: f64, pb: f64) -> Option<(f64, f64)> {
    if !(pa > 0.0 && pb > pa && p > 0.0 && l >= 0.0) {
        return None;
    }
    let (sp, spa, spb) = (p.sqrt(), pa.sqrt(), pb.sqrt());

    let (amount0, amount1) = if p <= pa {
        // Seluruhnya token0. Harga masih di bawah range.
        (l * (1.0 / spa - 1.0 / spb), 0.0)
    } else if p >= pb {
        // Seluruhnya token1. Harga sudah di atas range.
        (0.0, l * (spb - spa))
    } else {
        (l * (1.0 / sp - 1.0 / spb), l * (sp - spa))
    };

    Some((amount0, amount1))
}

/// Faktor konsentrasi, yaitu berapa kali lipat likuiditas yang didapat modal
/// yang sama dibanding range lain.
///
/// Nilainya berbanding terbalik dengan `sqrt(pb) - sqrt(pa)`. Ini yang dipakai
/// panel untuk menunjukkan berapa banyak fee yang hilang karena batas bawah
/// dipasang terlalu jauh. Lihat contoh terpakai di PRD 5.7.
pub fn concentration_factor(pa: f64, pb: f64) -> Option<f64> {
    if !(pa > 0.0 && pb > pa) {
        return None;
    }
    Some(1.0 / (pb.sqrt() - pa.sqrt()))
}

/// Perbandingan kepadatan likuiditas antara dua range untuk modal yang sama.
///
/// Hasil 2.45 berarti range kedua menghasilkan sekitar 2,45 kali lipat fee
/// selama harga berada di dalamnya.
pub fn concentration_ratio(
    base: (f64, f64),
    candidate: (f64, f64),
) -> Option<f64> {
    let a = concentration_factor(base.0, base.1)?;
    let b = concentration_factor(candidate.0, candidate.1)?;
    if a <= 0.0 {
        return None;
    }
    Some(b / a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    /// Memecoin 18 desimal sebagai token0, WETH sebagai token1.
    fn orientasi_token0() -> PoolOrientation {
        PoolOrientation {
            memecoin_is_token0: true,
            decimals_token0: 18,
            decimals_token1: 18,
        }
    }

    /// WETH sebagai token0, memecoin 18 desimal sebagai token1.
    fn orientasi_token1() -> PoolOrientation {
        PoolOrientation {
            memecoin_is_token0: false,
            decimals_token0: 18,
            decimals_token1: 18,
        }
    }

    #[test]
    fn urutan_token_diambil_dari_alamat() {
        let kecil = address!("0000000000000000000000000000000000000001");
        let besar = address!("ffffffffffffffffffffffffffffffffffffffff");

        let o = PoolOrientation::from_addresses(kecil, besar, 18, 18);
        assert!(o.memecoin_is_token0);

        let o = PoolOrientation::from_addresses(besar, kecil, 9, 18);
        assert!(!o.memecoin_is_token0);
        // Desimal ikut bertukar mengikuti slotnya.
        assert_eq!(o.decimals_token0, 18);
        assert_eq!(o.decimals_token1, 9);
    }

    #[test]
    fn putaran_penuh_harga_ke_tick_ke_harga() {
        let o = orientasi_token0();
        for harga in [1e-9, 1e-6, 1e-3, 0.5, 1.0, 25.0] {
            let tick = price_weth_to_tick(harga, o).unwrap();
            let kembali = tick_to_price_weth(tick, o).unwrap();
            let selisih = (kembali - harga).abs() / harga;
            assert!(
                selisih < 1e-4,
                "harga {harga} kembali sebagai {kembali}, selisih {selisih}"
            );
        }
    }

    #[test]
    fn orientasi_terbalik_membalik_arah_tick() {
        let naik = 2e-6;
        let turun = 1e-6;

        let t0 = orientasi_token0();
        assert!(
            price_weth_to_tick(naik, t0).unwrap() > price_weth_to_tick(turun, t0).unwrap(),
            "saat memecoin token0, harga naik berarti tick naik"
        );

        let t1 = orientasi_token1();
        assert!(
            price_weth_to_tick(naik, t1).unwrap() < price_weth_to_tick(turun, t1).unwrap(),
            "saat memecoin token1, harga naik berarti tick TURUN. Ini yang gampang terbalik"
        );
    }

    #[test]
    fn range_tetap_benar_meski_orientasi_terbalik() {
        // Pemanggil selalu memberi harga memecoin, tanpa peduli siapa token0.
        for o in [orientasi_token0(), orientasi_token1()] {
            let r = range_from_prices(1e-6, 2e-6, o, 60).unwrap();
            assert!(r.lower < r.upper, "lower harus selalu di bawah upper");
            assert_eq!(r.lower % 60, 0, "lower wajib pas di grid");
            assert_eq!(r.upper % 60, 0, "upper wajib pas di grid");
        }
    }

    #[test]
    fn urutan_argumen_tidak_mempengaruhi_hasil() {
        let o = orientasi_token0();
        let a = range_from_prices(1e-6, 2e-6, o, 60).unwrap();
        let b = range_from_prices(2e-6, 1e-6, o, 60).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn range_tidak_pernah_berlebar_nol() {
        let o = orientasi_token0();
        // Dua harga yang nyaris sama pada pool dengan spacing lebar.
        let r = range_from_prices(1.0e-6, 1.0000001e-6, o, 200).unwrap();
        assert!(r.width() >= 200, "lebar nol tidak sah di Uniswap");
    }

    #[test]
    fn snap_membulatkan_ke_arah_yang_benar() {
        assert_eq!(floor_to_spacing(125, 60), 120);
        assert_eq!(ceil_to_spacing(125, 60), 180);
        // Nilai negatif adalah tempat pembulatan integer biasa salah.
        assert_eq!(floor_to_spacing(-125, 60), -180);
        assert_eq!(ceil_to_spacing(-125, 60), -120);
        assert_eq!(floor_to_spacing(120, 60), 120);
        assert_eq!(ceil_to_spacing(120, 60), 120);
    }

    #[test]
    fn tick_spacing_v3_sesuai_fee_tier() {
        assert_eq!(v3_tick_spacing(100), Some(1));
        assert_eq!(v3_tick_spacing(500), Some(10));
        assert_eq!(v3_tick_spacing(3_000), Some(60));
        assert_eq!(v3_tick_spacing(10_000), Some(200));
        assert_eq!(v3_tick_spacing(1_234), None);
    }

    #[test]
    fn rantai_mcap_sampai_tick() {
        // Angka dari gambar contoh di PRD 5.7.
        let total_supply = 1e9;
        let eth_usd = 4_000.0;

        let price_usd = mcap_to_price_usd(239_220.0, total_supply).unwrap();
        let price_weth = price_usd_to_weth(price_usd, eth_usd).unwrap();
        let tick = price_weth_to_tick(price_weth, orientasi_token0()).unwrap();

        // Kembali lagi ke market cap harus mendekati titik awal.
        let kembali_weth = tick_to_price_weth(tick, orientasi_token0()).unwrap();
        let kembali_mcap = price_usd_to_mcap(kembali_weth * eth_usd, total_supply).unwrap();
        let selisih = (kembali_mcap - 239_220.0).abs() / 239_220.0;
        assert!(selisih < 1e-3, "market cap kembali sebagai {kembali_mcap}");
    }

    #[test]
    fn harga_mentah_kembali_ke_harga_weth() {
        for o in [orientasi_token0(), orientasi_token1()] {
            let harga_weth = 3.5e-6;
            let tick = price_weth_to_tick(harga_weth, o).unwrap();
            let dari_tick = tick_to_price_weth(tick, o).unwrap();

            // Bentuk harga mentah dari tick, lalu putar balik lewat jalur pool.
            let raw = 1.0001f64.powi(tick);
            let dari_raw = raw_price_to_price_weth(raw, o).unwrap();

            let selisih = (dari_raw - dari_tick).abs() / dari_tick;
            assert!(
                selisih < 1e-6,
                "jalur raw dan jalur tick harus sepakat, selisih {selisih}"
            );
        }
    }

    #[test]
    fn kalibrasi_kurs_dari_chart() {
        // Angka nyata dari PONS: market cap 937,23 juta, suplai 1 miliar.
        let mcap = 937_234_340.0;
        let suplai = 1e9;
        // Kalau harga memecoin 3,8e-4 WETH, kursnya sekitar 2466.
        let price_weth = 3.8e-4;
        let eth_usd = calibrate_eth_usd(mcap, suplai, price_weth).unwrap();
        assert!((eth_usd - 2466.4).abs() < 1.0, "kurs tersirat {eth_usd}");

        // Kalibrasi harus konsisten: kembali ke market cap semula.
        let kembali = price_weth_to_mcap(price_weth, eth_usd, suplai).unwrap();
        assert!((kembali - mcap).abs() / mcap < 1e-9);
    }

    #[test]
    fn kalibrasi_menolak_masukan_tidak_masuk_akal() {
        assert!(calibrate_eth_usd(0.0, 1e9, 1e-4).is_none());
        assert!(calibrate_eth_usd(1e6, 0.0, 1e-4).is_none());
        assert!(calibrate_eth_usd(1e6, 1e9, 0.0).is_none());
        assert!(calibrate_eth_usd(f64::NAN, 1e9, 1e-4).is_none());
        assert!(calibrate_eth_usd(-1.0, 1e9, 1e-4).is_none());
    }

    #[test]
    fn mcap_bolak_balik_lewat_weth() {
        let suplai = 1e9;
        let eth_usd = 2500.0;
        let mcap = 8.5e8;

        let weth = mcap_to_price_weth(mcap, suplai, eth_usd).unwrap();
        let kembali = price_weth_to_mcap(weth, eth_usd, suplai).unwrap();
        assert!((kembali - mcap).abs() / mcap < 1e-9);
    }

    #[test]
    fn masukan_tidak_masuk_akal_ditolak_bukan_panic() {
        let o = orientasi_token0();
        assert!(price_weth_to_tick(0.0, o).is_none());
        assert!(price_weth_to_tick(-1.0, o).is_none());
        assert!(price_weth_to_tick(f64::NAN, o).is_none());
        assert!(price_weth_to_tick(f64::INFINITY, o).is_none());
        assert!(mcap_to_price_usd(100.0, 0.0).is_none());
        assert!(price_usd_to_weth(1.0, 0.0).is_none());
        assert!(range_from_prices(1e-6, 2e-6, o, 0).is_none());
        assert!(tick_to_price_weth(MAX_TICK + 1, o).is_none());
    }

    #[test]
    fn jumlah_token_mengikuti_posisi_harga() {
        // Di bawah range: seluruhnya token0.
        let (a0, a1) = amounts_for_liquidity(1000.0, 0.5, 1.0, 4.0).unwrap();
        assert!(a0 > 0.0 && a1 == 0.0);

        // Di atas range: seluruhnya token1.
        let (a0, a1) = amounts_for_liquidity(1000.0, 9.0, 1.0, 4.0).unwrap();
        assert!(a0 == 0.0 && a1 > 0.0);

        // Di dalam range: dua duanya.
        let (a0, a1) = amounts_for_liquidity(1000.0, 2.0, 1.0, 4.0).unwrap();
        assert!(a0 > 0.0 && a1 > 0.0);

        assert!(amounts_for_liquidity(1000.0, 2.0, 4.0, 1.0).is_none());
    }

    #[test]
    fn faktor_konsentrasi_cocok_dengan_contoh_prd() {
        // Angka dari PRD 5.7, dalam ribuan.
        let lebar = concentration_ratio((150.0, 300.0), (200.0, 300.0)).unwrap();
        assert!(
            (lebar - 1.6).abs() < 0.05,
            "menaikkan batas bawah ke 200K seharusnya sekitar 1,6 kali, dapat {lebar}"
        );

        let inti = concentration_ratio((150.0, 300.0), (215.0, 280.0)).unwrap();
        assert!(
            (inti - 2.45).abs() < 0.05,
            "range inti seharusnya sekitar 2,45 kali, dapat {inti}"
        );
    }

    #[test]
    fn selisih_pembulatan_dalam_basis_poin() {
        assert_eq!(snap_drift_bps(100.0, 101.0).unwrap().round(), 100.0);
        assert_eq!(snap_drift_bps(100.0, 99.0).unwrap().round(), -100.0);
        assert!(snap_drift_bps(0.0, 1.0).is_none());
    }
}
