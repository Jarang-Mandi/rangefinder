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
    /// Ambil fee yang sudah terkumpul, likuiditasnya dibiarkan.
    KlaimFee,
    /// Tarik seluruh likuiditas beserta fee, lalu bakar NFT-nya.
    TutupPosisi,
}

impl JenisAksi {
    pub fn nama(&self) -> &'static str {
        match self {
            JenisAksi::KlaimFee => "klaim fee",
            JenisAksi::TutupPosisi => "tutup posisi",
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum AksiError {
    #[error("token id `{0}` bukan angka")]
    TokenIdTidakValid(String),
    #[error("posisi ini tidak punya likuiditas, tidak ada yang bisa ditarik")]
    TanpaLikuiditas,
    #[error("posisi ini tidak punya fee yang belum diklaim")]
    TanpaFee,
    #[error("alamat `{0}` tidak valid")]
    AlamatTidakValid(String),
    #[error("deadline nol, transaksi tidak akan pernah dianggap sah")]
    DeadlineNol,
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

    let punya_fee = posisi.fee0.parse::<u128>().unwrap_or(0) > 0
        || posisi.fee1.parse::<u128>().unwrap_or(0) > 0;

    match aksi {
        JenisAksi::KlaimFee if !punya_fee => return Err(AksiError::TanpaFee),
        JenisAksi::TutupPosisi if liquidity == 0 && !punya_fee => {
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
        (PoolVersion::V3, JenisAksi::KlaimFee) => {
            super::calldata::collect(token_id, pemilik)
        }
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
            token_id: "42".into(),
            version: versi,
            token0: "0x5fc5360d0400a0fd4f2af552add042d716f1d168".into(),
            token1: "0x791e750fa8b2237288a6fe6c47ea81c50093449a".into(),
            fee: 3000,
            tick_lower: -60,
            tick_upper: 60,
            liquidity: liq.into(),
            tick_sekarang: Some(0),
            in_range: true,
            fee0: fee0.into(),
            fee1: fee1.into(),
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
}
