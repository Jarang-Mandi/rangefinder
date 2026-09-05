//! Penyusunan tampilan range: kalibrasi kurs, konversi satuan, dan snap tick.
//!
//! INVARIAN
//! - Seluruh matematika satuan hidup di sini dan di `math`. Panel TIDAK PERNAH
//!   menghitung sendiri, ia hanya menampilkan. Dua tempat yang menghitung hal
//!   yang sama pasti menyimpang, dan pada proyek yang mengirim transaksi itu
//!   berarti angka di layar berbeda dari yang tercatat on chain.
//! - Angka yang ditampilkan SELALU hasil pembulatan tick, bukan angka mentah
//!   yang digeser user. Selisihnya dilaporkan terpisah sebagai basis poin.
//!   Lihat PRD 4.3.
//! - Kurs ETH ke USD diturunkan dari chart, bukan dari sumber luar. Alasannya
//!   ada di `math::calibrate_eth_usd`.

use serde::Serialize;

use crate::math::{
    self, calibrate_eth_usd, mcap_to_price_weth, price_weth_to_mcap, price_weth_to_usd,
    range_from_prices, raw_price_to_price_weth, snap_drift_bps, tick_to_price_weth,
    PoolOrientation, TickRange,
};
use crate::pools::{PoolInfo, TokenMeta};

/// Lebar range bawaan saat user belum menggeser apa pun.
///
/// Ini nilai sementara yang sengaja sederhana. Doktrin penempatan sebenarnya di
/// PRD 5.7 memakai struktur harga, yaitu dasar basis, area nilai, dan point of
/// control, dan itu butuh data lilin yang baru masuk di fase 6. Sampai saat itu
/// tiba, lebih jujur memakai lebar tetap yang jelas dinyatakan sebagai bawaan
/// daripada berpura pura sudah pintar.
pub const DEFAULT_WIDTH_PCT: f64 = 0.15;

/// Satu batas range, ditampilkan dalam tiga satuan sekaligus.
#[derive(Debug, Clone, Serialize)]
pub struct RangeBound {
    pub tick: i32,
    pub price_weth: f64,
    pub price_usd: f64,
    pub mcap_usd: f64,
    /// Selisih antara yang diminta user dan hasil pembulatan tick.
    pub drift_bps: f64,
}

/// Keadaan harga sekarang, dalam tiga satuan yang sama.
#[derive(Debug, Clone, Serialize)]
pub struct CurrentPrice {
    pub tick: i32,
    pub price_weth: f64,
    pub price_usd: f64,
    pub mcap_usd: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RangeView {
    pub pool_id: String,
    pub tick_spacing: i32,
    /// Kurs yang tersirat dari chart. Ditampilkan supaya user bisa menilai
    /// apakah kalibrasinya masuk akal.
    pub eth_usd: f64,
    pub current: CurrentPrice,
    pub lower: RangeBound,
    pub upper: RangeBound,
    /// Lebar range relatif terhadap harga sekarang, dalam persen.
    pub width_pct: f64,
    /// Benar kalau harga sekarang berada di dalam range.
    pub in_range: bool,
    pub warnings: Vec<String>,
}

/// Tentukan orientasi pool terhadap memecoin yang sedang dilihat.
///
/// Mengembalikan `None` kalau token yang diminta ternyata bukan salah satu dari
/// kedua sisi pool, yang berarti pemanggilnya salah memasangkan pool dan token.
pub fn orientation_for(
    pool: &PoolInfo,
    token_address: &str,
    token_decimals: u8,
    quote_decimals: u8,
) -> Option<PoolOrientation> {
    let token = token_address.to_lowercase();
    let t0 = pool.token0.to_lowercase();
    let t1 = pool.token1.to_lowercase();

    if token == t0 {
        Some(PoolOrientation {
            memecoin_is_token0: true,
            decimals_token0: token_decimals,
            decimals_token1: quote_decimals,
        })
    } else if token == t1 {
        Some(PoolOrientation {
            memecoin_is_token0: false,
            decimals_token0: quote_decimals,
            decimals_token1: token_decimals,
        })
    } else {
        None
    }
}

/// Range bawaan di sekitar harga sekarang.
pub fn default_range(price_weth: f64, o: PoolOrientation, spacing: i32) -> Option<TickRange> {
    let bawah = price_weth * (1.0 - DEFAULT_WIDTH_PCT);
    let atas = price_weth * (1.0 + DEFAULT_WIDTH_PCT);
    range_from_prices(bawah, atas, o, spacing)
}

/// Susun tampilan range lengkap dari keadaan pool dan market cap di chart.
///
/// `requested` berisi batas dalam market cap USD kalau user sudah menggeser
/// garis. Kalau `None`, range bawaan dipakai.
pub fn build_view(
    pool: &PoolInfo,
    meta: &TokenMeta,
    quote_decimals: u8,
    chart_mcap_usd: f64,
    requested: Option<(f64, f64)>,
) -> Result<RangeView, String> {
    let mut warnings = Vec::new();

    let orientation = orientation_for(pool, &meta.address, meta.decimals, quote_decimals)
        .ok_or_else(|| {
            format!(
                "token {} bukan salah satu sisi pool {}",
                meta.address, pool.id
            )
        })?;

    let price_weth = raw_price_to_price_weth(pool.raw_price, orientation)
        .ok_or_else(|| format!("harga pool {} tidak masuk akal", pool.id))?;

    let eth_usd = calibrate_eth_usd(chart_mcap_usd, meta.total_supply, price_weth).ok_or_else(
        || "kalibrasi kurs gagal, market cap atau suplai tidak masuk akal".to_string(),
    )?;

    // Kurs yang jauh di luar akal sehat berarti salah satu masukannya keliru,
    // paling sering karena suplai token dilaporkan berbeda oleh chart.
    if !(100.0..=100_000.0).contains(&eth_usd) {
        warnings.push(format!(
            "kurs tersirat {eth_usd:.0} USD per ETH di luar rentang wajar. \
             Periksa suplai token, chart dan rantai mungkin tidak sepakat"
        ));
    }

    let spacing = pool.tick_spacing;
    if spacing <= 0 {
        return Err(format!("tick spacing pool {} tidak valid", pool.id));
    }

    // Batas yang diminta user datang dalam market cap, satuan yang dipakai
    // chart. Diubah ke WETH dulu supaya seluruh perhitungan tick memakai
    // satuan yang sama.
    let (minta_bawah_weth, minta_atas_weth) = match requested {
        Some((bawah_mcap, atas_mcap)) => {
            let b = mcap_to_price_weth(bawah_mcap, meta.total_supply, eth_usd)
                .ok_or_else(|| "batas bawah tidak masuk akal".to_string())?;
            let a = mcap_to_price_weth(atas_mcap, meta.total_supply, eth_usd)
                .ok_or_else(|| "batas atas tidak masuk akal".to_string())?;
            (b, a)
        }
        None => (
            price_weth * (1.0 - DEFAULT_WIDTH_PCT),
            price_weth * (1.0 + DEFAULT_WIDTH_PCT),
        ),
    };

    let range = range_from_prices(minta_bawah_weth, minta_atas_weth, orientation, spacing)
        .ok_or_else(|| "batas range tidak bisa diubah menjadi tick".to_string())?;

    // Tick bawah dan atas mengikuti sumbu pool. Kalau memecoin menempati
    // token1, tick yang lebih besar justru berarti harga memecoin lebih rendah,
    // jadi sisi tampilannya bertukar.
    let (tick_bawah_tampilan, tick_atas_tampilan) = if orientation.memecoin_is_token0 {
        (range.lower, range.upper)
    } else {
        (range.upper, range.lower)
    };

    let lower = bound_from_tick(
        tick_bawah_tampilan,
        orientation,
        eth_usd,
        meta.total_supply,
        minta_bawah_weth,
    )?;
    let upper = bound_from_tick(
        tick_atas_tampilan,
        orientation,
        eth_usd,
        meta.total_supply,
        minta_atas_weth,
    )?;

    if lower.mcap_usd >= upper.mcap_usd {
        return Err("batas bawah tidak boleh berada di atas batas atas".into());
    }

    let current = CurrentPrice {
        tick: pool.tick,
        price_weth,
        price_usd: price_weth_to_usd(price_weth, eth_usd).unwrap_or(f64::NAN),
        mcap_usd: chart_mcap_usd,
    };

    let in_range = chart_mcap_usd >= lower.mcap_usd && chart_mcap_usd <= upper.mcap_usd;
    if !in_range {
        warnings.push("harga sekarang berada di luar range".into());
    }

    let width_pct = (upper.mcap_usd - lower.mcap_usd) / chart_mcap_usd * 100.0;

    // Range selebar satu sel grid berarti pembulatan menelan seluruh permintaan
    // user. Itu bukan kesalahan, tapi wajib terlihat.
    if range.width() <= spacing {
        warnings.push(format!(
            "range hanya selebar satu sel grid ({spacing} tick). \
             Pool dengan fee tier lebih rendah punya grid lebih halus"
        ));
    }

    Ok(RangeView {
        pool_id: pool.id.clone(),
        tick_spacing: spacing,
        eth_usd,
        current,
        lower,
        upper,
        width_pct,
        in_range,
        warnings,
    })
}

fn bound_from_tick(
    tick: i32,
    o: PoolOrientation,
    eth_usd: f64,
    total_supply: f64,
    requested_weth: f64,
) -> Result<RangeBound, String> {
    let price_weth =
        tick_to_price_weth(tick, o).ok_or_else(|| format!("tick {tick} di luar batas Uniswap"))?;
    let price_usd = price_weth_to_usd(price_weth, eth_usd)
        .ok_or_else(|| "harga USD tidak bisa dihitung".to_string())?;
    let mcap_usd = price_weth_to_mcap(price_weth, eth_usd, total_supply)
        .ok_or_else(|| "market cap tidak bisa dihitung".to_string())?;

    Ok(RangeBound {
        tick,
        price_weth,
        price_usd,
        mcap_usd,
        drift_bps: snap_drift_bps(requested_weth, price_weth).unwrap_or(0.0),
    })
}

/// Faktor konsentrasi range ini dibanding range lain, untuk modal yang sama.
///
/// Dipakai panel untuk menunjukkan berapa banyak fee yang hilang karena batas
/// dipasang terlalu jauh. Lihat contoh terpakai di PRD 5.7.
pub fn concentration_vs(a: (f64, f64), b: (f64, f64)) -> Option<f64> {
    math::concentration_ratio(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pools::PoolVersion;

    fn meta() -> TokenMeta {
        TokenMeta {
            address: "0x39dbed3a2bd333467115de45665cc57f813c4571".into(),
            symbol: "PONS".into(),
            decimals: 18,
            total_supply: 1e9,
        }
    }

    /// Pool dengan memecoin sebagai token1, seperti PONS/WETH yang sebenarnya,
    /// karena alamat WETH lebih kecil.
    fn pool_memecoin_token1(raw_price: f64) -> PoolInfo {
        PoolInfo {
            version: PoolVersion::V3,
            id: "0xpool".into(),
            token0: "0x0Bd7D308f8E1639FAb988df18A8011f41EAcAD73".into(),
            token1: "0x39dbed3a2bd333467115de45665cc57f813c4571".into(),
            fee: 3_000,
            dynamic_fee: false,
            tick_spacing: 60,
            hooks: None,
            sqrt_price_x96: "0".into(),
            tick: 78_782,
            liquidity: "1000000".into(),
            raw_price,
        }
    }

    #[test]
    fn orientasi_dikenali_dari_kedua_sisi() {
        let p = pool_memecoin_token1(2638.0);
        let o = orientation_for(&p, &meta().address, 18, 18).unwrap();
        assert!(!o.memecoin_is_token0, "PONS menempati token1");

        // Huruf besar kecil tidak boleh berpengaruh.
        let o = orientation_for(&p, &meta().address.to_uppercase(), 18, 18);
        assert!(o.is_some());
    }

    #[test]
    fn token_asing_ditolak() {
        let p = pool_memecoin_token1(2638.0);
        assert!(orientation_for(&p, "0xdeadbeef00000000000000000000000000000000", 18, 18).is_none());
    }

    #[test]
    fn tampilan_range_konsisten_dengan_chart() {
        // Angka nyata: PONS pada 937,23 juta market cap, pool fee 3000.
        let p = pool_memecoin_token1(2638.0);
        let m = meta();
        let view = build_view(&p, &m, 18, 937_234_340.0, None).unwrap();

        // Harga sekarang harus persis sama dengan yang dibaca chart.
        assert_eq!(view.current.mcap_usd, 937_234_340.0);

        // Batas bawah selalu di bawah batas atas dalam satuan tampilan, meski
        // memecoin menempati token1 sehingga sumbu ticknya terbalik.
        assert!(view.lower.mcap_usd < view.upper.mcap_usd);
        assert!(view.lower.tick > view.upper.tick, "sumbu tick terbalik untuk token1");

        // Harga sekarang harus berada di dalam range bawaan.
        assert!(view.in_range, "range bawaan harus melingkupi harga sekarang");

        // Lebar bawaan sekitar 30 persen, yaitu 15 persen ke tiap arah.
        assert!(
            (view.width_pct - 30.0).abs() < 5.0,
            "lebar {} persen di luar dugaan",
            view.width_pct
        );

        // Kurs tersirat harus masuk akal untuk ETH.
        assert!(view.eth_usd > 100.0 && view.eth_usd < 100_000.0, "kurs {}", view.eth_usd);
        assert!(view.warnings.is_empty(), "tidak ada peringatan: {:?}", view.warnings);
    }

    #[test]
    fn batas_yang_diminta_dihormati_setelah_snap() {
        let p = pool_memecoin_token1(2638.0);
        let m = meta();
        let dasar = build_view(&p, &m, 18, 937_234_340.0, None).unwrap();

        let minta_bawah = 8.0e8;
        let minta_atas = 1.1e9;
        let view = build_view(&p, &m, 18, 937_234_340.0, Some((minta_bawah, minta_atas))).unwrap();

        // Hasilnya harus dekat dengan yang diminta, tapi tidak wajib sama
        // persis karena tick dibulatkan ke grid.
        assert!((view.lower.mcap_usd - minta_bawah).abs() / minta_bawah < 0.02);
        assert!((view.upper.mcap_usd - minta_atas).abs() / minta_atas < 0.02);

        // Selisih pembulatan wajib dilaporkan, bukan disembunyikan.
        assert!(view.lower.drift_bps.abs() < 200.0);
        assert!(view.upper.drift_bps.abs() < 200.0);

        // Range yang diminta lebih lebar dari bawaan.
        assert!(view.width_pct > dasar.width_pct);
    }

    #[test]
    fn harga_di_luar_range_ditandai() {
        let p = pool_memecoin_token1(2638.0);
        let m = meta();
        // Range seluruhnya di atas harga sekarang.
        let view = build_view(&p, &m, 18, 937_234_340.0, Some((1.5e9, 2.0e9))).unwrap();
        assert!(!view.in_range);
        assert!(view.warnings.iter().any(|w| w.contains("luar range")));
    }

    #[test]
    fn batas_terbalik_ditolak_bukan_diam() {
        let p = pool_memecoin_token1(2638.0);
        let m = meta();
        // Dua batas yang jatuh di sel grid yang sama pada pool spacing lebar
        // tetap harus menghasilkan range yang sah, bukan lebar nol.
        let view = build_view(&p, &m, 18, 937_234_340.0, Some((9.3e8, 9.3000001e8))).unwrap();
        assert!(view.lower.mcap_usd < view.upper.mcap_usd);
    }

    #[test]
    fn suplai_salah_memicu_peringatan_kurs() {
        let p = pool_memecoin_token1(2638.0);
        let mut m = meta();
        // Suplai seribu kali lebih besar membuat kurs tersirat jadi janggal.
        m.total_supply = 1e12;
        let view = build_view(&p, &m, 18, 937_234_340.0, None).unwrap();
        assert!(
            view.warnings.iter().any(|w| w.contains("kurs tersirat")),
            "peringatan kurs harus muncul: {:?}",
            view.warnings
        );
    }

    #[test]
    fn pool_rusak_menghasilkan_error_yang_menjelaskan() {
        let mut p = pool_memecoin_token1(2638.0);
        p.tick_spacing = 0;
        let e = build_view(&p, &meta(), 18, 9.3e8, None).unwrap_err();
        assert!(e.contains("tick spacing"), "pesan: {e}");

        let mut p = pool_memecoin_token1(0.0);
        p.tick_spacing = 60;
        let e = build_view(&p, &meta(), 18, 9.3e8, None).unwrap_err();
        assert!(e.contains("harga pool"), "pesan: {e}");
    }
}
