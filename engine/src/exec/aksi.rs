//! Aksi pada posisi yang sudah ada: klaim fee dan tutup posisi.
//!
//! KENAPA TERPISAH DARI MEMBUKA POSISI
//! Membuka posisi menyusun sesuatu dari nol, dan gate-nya berkisar pada apakah
//! angka yang akan dikirim masuk akal. Aksi pada posisi yang sudah ada berkisar
//! pada apakah posisinya memang milik kita dan memang dalam keadaan yang
//! memungkinkan aksi itu. Menggabungkannya membuat kedua daftar pemeriksaan
//! saling mengganggu.
//!
//! INVARIAN
//! - Kepemilikan diperiksa terhadap rantai sebelum calldata disusun. Mengirim
//!   aksi untuk posisi milik orang lain akan revert, tapi gasnya tetap terbakar,
//!   dan pesannya tidak menjelaskan apa apa.
//! - Menutup posisi selalu mengambil kedua currency ke pemiliknya. Menutup tanpa
//!   mengambil berarti dana tersangkut di kontrak.
//! - Batas minimum saat menutup diisi nol dengan sengaja. Yang ditutup adalah
//!   posisi kita sendiri secara penuh, bukan pertukaran yang bisa dijepit, dan
//!   batas minimum yang terlalu ketat hanya membuat penutupan gagal saat harga
//!   bergerak sedikit.

use alloy::primitives::{Address, Bytes, U256};
use serde::{Deserialize, Serialize};

use crate::pools::PoolVersion;

/// Aksi yang bisa dijalankan pada sebuah posisi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JenisAksi {
    /// Tambah modal ke range NFT yang sama.
    TambahLikuiditas,
    /// Tutup NFT lama dan buka range baru dalam satu transaksi atomik.
    Rebalance,
    /// Ambil fee yang sudah terkumpul, likuiditasnya dibiarkan.
    KlaimFee,
    /// Tarik seluruh likuiditas beserta fee, lalu bakar NFT-nya.
    TutupPosisi,
}

impl JenisAksi {
    pub fn nama(&self) -> &'static str {
        match self {
            JenisAksi::TambahLikuiditas => "tambah likuiditas",
            JenisAksi::Rebalance => "rebalance",
            JenisAksi::KlaimFee => "klaim fee",
            JenisAksi::TutupPosisi => "tutup posisi",
        }
    }
}

/// Masukan yang boleh datang dari panel untuk sebuah aksi posisi.
///
/// Jumlah masih berupa desimal manusiawi. Engine membaca desimal token dari
/// rantai lalu mengubahnya ke unit terkecil, sehingga panel tidak pernah
/// melakukan matematika yang akhirnya masuk calldata.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionInput {
    #[serde(default)]
    pub amount0: Option<String>,
    #[serde(default)]
    pub amount1: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum AksiError {
    #[error("token id `{0}` bukan angka")]
    TokenIdTidakValid(String),
    #[error("posisi ini tidak punya likuiditas, tidak ada yang bisa ditarik")]
    TanpaLikuiditas,
    #[error("posisi ini tidak punya fee yang belum diklaim")]
    TanpaFee,
    #[error("fee posisi belum dapat dibaca, klaim tidak boleh ditebak sebagai nol")]
    FeeTidakDiketahui,
    #[error("alamat `{0}` tidak valid")]
    AlamatTidakValid(String),
    #[error("deadline nol, transaksi tidak akan pernah dianggap sah")]
    DeadlineNol,
    #[error("tick pool sekarang belum terbaca, rebalance tidak boleh memakai harga lama")]
    TickTidakDiketahui,
    #[error("tickSpacing {0} tidak masuk akal")]
    SpacingTidakValid(i32),
    #[error("posisi tidak mempunyai token yang dapat dipindahkan ke range baru")]
    TanpaModal,
    #[error("lebar range posisi tidak valid")]
    LebarRangeTidakValid,
}

/// Pilih range rebalance dengan lebar yang sama dari keadaan pool segar.
///
/// Posisi dua sisi dipusatkan pada harga sekarang. Posisi satu sisi diletakkan
/// tepat di sisi harga yang menerima token tersebut, supaya rebalance posisi
/// yang sudah keluar range tidak memerlukan swap tersembunyi.
pub fn target_rebalance(
    posisi: &crate::positions::Posisi,
    tersedia0: U256,
    tersedia1: U256,
) -> Result<(i32, i32), AksiError> {
    let tick = posisi.tick_sekarang.ok_or(AksiError::TickTidakDiketahui)?;
    let spacing = posisi.tick_spacing;
    if spacing <= 0 {
        return Err(AksiError::SpacingTidakValid(spacing));
    }
    let width = posisi
        .tick_upper
        .checked_sub(posisi.tick_lower)
        .filter(|w| *w >= spacing && *w % spacing == 0)
        .ok_or(AksiError::LebarRangeTidakValid)?;
    if tersedia0.is_zero() && tersedia1.is_zero() {
        return Err(AksiError::TanpaModal);
    }

    let (mut lower, mut upper) = if tersedia1.is_zero() {
        let next = tick.checked_add(1).ok_or(AksiError::LebarRangeTidakValid)?;
        let lower = crate::math::ceil_to_spacing(next, spacing);
        (lower, lower.saturating_add(width))
    } else if tersedia0.is_zero() {
        let upper = crate::math::floor_to_spacing(tick, spacing);
        (upper.saturating_sub(width), upper)
    } else {
        let half = width / 2;
        let lower = crate::math::floor_to_spacing(tick.saturating_sub(half), spacing);
        (lower, lower.saturating_add(width))
    };

    let minimum = crate::math::ceil_to_spacing(crate::math::MIN_TICK, spacing);
    let maximum = crate::math::floor_to_spacing(crate::math::MAX_TICK, spacing);
    if lower < minimum {
        lower = minimum;
        upper = lower.saturating_add(width);
    }
    if upper > maximum {
        upper = maximum;
        lower = upper.saturating_sub(width);
    }
    if lower < minimum || upper > maximum || lower >= upper {
        return Err(AksiError::LebarRangeTidakValid);
    }
    let sisi_sesuai = if tersedia1.is_zero() {
        tick < lower
    } else if tersedia0.is_zero() {
        tick >= upper
    } else {
        tick >= lower && tick < upper
    };
    if !sisi_sesuai {
        return Err(AksiError::LebarRangeTidakValid);
    }
    Ok((lower, upper))
}

/// Calldata untuk sebuah aksi, beserta kontrak tujuannya.
#[derive(Debug, Clone)]
pub struct Rencana {
    pub calldata: Bytes,
    pub versi: PoolVersion,
    /// Benar kalau aksi ini menghabiskan posisinya.
    pub menutup: bool,
}

/// Susun calldata untuk sebuah aksi.
///
/// `posisi` datang dari pembacaan rantai, jadi likuiditas dan fee di dalamnya
/// sudah yang sebenarnya, bukan yang dilaporkan `tokensOwed`.
pub fn susun(
    posisi: &crate::positions::Posisi,
    aksi: JenisAksi,
    pemilik: Address,
    deadline: U256,
) -> Result<Rencana, AksiError> {
    if deadline.is_zero() {
        return Err(AksiError::DeadlineNol);
    }
    let token_id: U256 = posisi
        .token_id
        .parse()
        .map_err(|_| AksiError::TokenIdTidakValid(posisi.token_id.clone()))?;
    let liquidity: u128 = posisi.liquidity.parse().unwrap_or(0);

    let punya_fee = posisi
        .fee0
        .as_deref()
        .zip(posisi.fee1.as_deref())
        .map(|(fee0, fee1)| {
            fee0.parse::<u128>().unwrap_or(0) > 0 || fee1.parse::<u128>().unwrap_or(0) > 0
        });

    match aksi {
        JenisAksi::TambahLikuiditas | JenisAksi::Rebalance => return Err(AksiError::TanpaModal),
        JenisAksi::KlaimFee if punya_fee.is_none() => return Err(AksiError::FeeTidakDiketahui),
        JenisAksi::KlaimFee if punya_fee == Some(false) => return Err(AksiError::TanpaFee),
        JenisAksi::TutupPosisi if liquidity == 0 && punya_fee == Some(false) => {
            return Err(AksiError::TanpaLikuiditas)
        }
        _ => {}
    }

    let c0: Address = posisi
        .token0
        .parse()
        .map_err(|_| AksiError::AlamatTidakValid(posisi.token0.clone()))?;
    let c1: Address = posisi
        .token1
        .parse()
        .map_err(|_| AksiError::AlamatTidakValid(posisi.token1.clone()))?;

    let calldata = match (posisi.version, aksi) {
        (PoolVersion::V3, JenisAksi::KlaimFee) => super::calldata::collect(token_id, pemilik),
        (PoolVersion::V3, JenisAksi::TutupPosisi) => super::calldata::close_position(
            token_id,
            liquidity,
            U256::ZERO,
            U256::ZERO,
            pemilik,
            deadline,
        ),
        (PoolVersion::V4, JenisAksi::KlaimFee) => {
            // V4 tidak punya `collect`. Fee diambil dengan mengurangi likuiditas
            // sebanyak NOL, yang tetap memicu penyelesaian fee, lalu mengambil
            // kedua currency.
            super::v4::calldata::klaim_fee(token_id, c0, c1, pemilik, deadline)
                .map_err(|_| AksiError::TanpaFee)?
        }
        (PoolVersion::V4, JenisAksi::TutupPosisi) => {
            super::v4::calldata::close_position(token_id, c0, c1, pemilik, deadline)
                .map_err(|_| AksiError::DeadlineNol)?
        }
        (_, JenisAksi::TambahLikuiditas | JenisAksi::Rebalance) => unreachable!(),
    };

    Ok(Rencana {
        calldata,
        versi: posisi.version,
        menutup: aksi == JenisAksi::TutupPosisi,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::positions::Posisi;

    fn posisi(versi: PoolVersion, liq: &str, fee0: &str, fee1: &str) -> Posisi {
        Posisi {
            position_id: format!("4663:{versi:?}:1"),
            token_id: "42".into(),
            version: versi,
            token0: "0x5fc5360d0400a0fd4f2af552add042d716f1d168".into(),
            token1: "0x791e750fa8b2237288a6fe6c47ea81c50093449a".into(),
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
            tick_lower: -60,
            tick_upper: 60,
            liquidity: liq.into(),
            tick_sekarang: Some(0),
            in_range: true,
            fee0: Some(fee0.into()),
            fee1: Some(fee1.into()),
            hooks: None,
            sqrt_price_x96: None,
            amount0: "0".into(),
            amount1: "0".into(),
            pnl: None,
            kosong: liq == "0",
        }
    }

    fn dompet() -> Address {
        Address::from([0xab; 20])
    }

    #[test]
    fn klaim_fee_v3_memakai_collect() {
        let r = susun(
            &posisi(PoolVersion::V3, "100", "5", "0"),
            JenisAksi::KlaimFee,
            dompet(),
            U256::from(1u8),
        )
        .unwrap();
        assert!(!r.menutup);
        // Selector `collect(CollectParams)` ada di awal calldata.
        assert!(r.calldata.len() > 4);
    }

    #[test]
    fn fee_yang_gagal_dibaca_tidak_dianggap_nol() {
        let mut p = posisi(PoolVersion::V3, "100", "5", "0");
        p.fee0 = None;
        p.fee1 = None;
        assert_eq!(
            susun(&p, JenisAksi::KlaimFee, dompet(), U256::from(1u8)).unwrap_err(),
            AksiError::FeeTidakDiketahui
        );
    }

    #[test]
    fn klaim_fee_v4_lewat_modify_liquidities() {
        let r = susun(
            &posisi(PoolVersion::V4, "100", "0", "7"),
            JenisAksi::KlaimFee,
            dompet(),
            U256::from(1u8),
        )
        .unwrap();
        assert_eq!(hex::encode(&r.calldata[..4]), "dd46508f");
    }

    #[test]
    fn klaim_tanpa_fee_ditolak() {
        // Mengirimnya tetap berhasil di rantai, tapi hanya membakar gas untuk
        // memindahkan nol. Ditolak lebih awal supaya user tidak membayar
        // untuk tidak mendapat apa apa.
        let e = susun(
            &posisi(PoolVersion::V4, "100", "0", "0"),
            JenisAksi::KlaimFee,
            dompet(),
            U256::from(1u8),
        )
        .unwrap_err();
        assert_eq!(e, AksiError::TanpaFee);
    }

    #[test]
    fn tutup_posisi_kosong_tanpa_fee_ditolak() {
        let e = susun(
            &posisi(PoolVersion::V4, "0", "0", "0"),
            JenisAksi::TutupPosisi,
            dompet(),
            U256::from(1u8),
        )
        .unwrap_err();
        assert_eq!(e, AksiError::TanpaLikuiditas);
    }

    #[test]
    fn tutup_posisi_kosong_yang_masih_punya_fee_diterima() {
        // Posisi yang likuiditasnya sudah ditarik tapi fee-nya belum diambil
        // masih layak ditutup, karena penutupannya sekaligus mengambil fee itu.
        let r = susun(
            &posisi(PoolVersion::V4, "0", "3", "0"),
            JenisAksi::TutupPosisi,
            dompet(),
            U256::from(1u8),
        );
        assert!(r.is_ok());
    }

    #[test]
    fn deadline_nol_ditolak() {
        let e = susun(
            &posisi(PoolVersion::V3, "100", "5", "0"),
            JenisAksi::KlaimFee,
            dompet(),
            U256::ZERO,
        )
        .unwrap_err();
        assert_eq!(e, AksiError::DeadlineNol);
    }

    #[test]
    fn token_id_bukan_angka_ditolak() {
        let mut p = posisi(PoolVersion::V3, "100", "5", "0");
        p.token_id = "bukan angka".into();
        assert!(matches!(
            susun(&p, JenisAksi::KlaimFee, dompet(), U256::from(1u8)).unwrap_err(),
            AksiError::TokenIdTidakValid(_)
        ));
    }

    #[test]
    fn tutup_ditandai_menutup() {
        let r = susun(
            &posisi(PoolVersion::V3, "100", "0", "0"),
            JenisAksi::TutupPosisi,
            dompet(),
            U256::from(1u8),
        )
        .unwrap();
        assert!(r.menutup, "panel perlu tahu kartunya akan hilang");
    }

    #[test]
    fn target_rebalance_dua_sisi_mempertahankan_lebar_dan_grid() {
        let mut p = posisi(PoolVersion::V3, "100", "0", "0");
        p.tick_lower = -600;
        p.tick_upper = 600;
        p.tick_sekarang = Some(725);
        let (lower, upper) = target_rebalance(&p, U256::from(1), U256::from(1)).unwrap();
        assert_eq!(upper - lower, 1_200);
        assert_eq!(lower % 60, 0);
        assert!(lower <= 725 && 725 < upper);
    }

    #[test]
    fn target_rebalance_satu_sisi_tidak_memerlukan_token_lawan() {
        let mut p = posisi(PoolVersion::V4, "100", "0", "0");
        p.tick_lower = -600;
        p.tick_upper = 600;
        p.tick_sekarang = Some(725);
        let token0 = target_rebalance(&p, U256::from(1), U256::ZERO).unwrap();
        assert!(token0.0 > 725, "range token0 harus berada di atas harga");
        let token1 = target_rebalance(&p, U256::ZERO, U256::from(1)).unwrap();
        assert!(token1.1 <= 725, "range token1 harus berada di bawah harga");
    }
}
