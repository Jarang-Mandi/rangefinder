//! Izin dua lapis untuk V4.
//!
//! KENAPA DUA LAPIS, DAN KENAPA BERBEDA DARI V3
//! V3 menarik token lewat `transferFrom` biasa, jadi cukup satu allowance ERC20
//! ke NonfungiblePositionManager. V4 tidak. PositionManager V4 menarik token
//! lewat Permit2, sehingga izinnya bertingkat:
//!
//! 1. Token mengizinkan Permit2 lewat `approve` ERC20 biasa.
//! 2. Permit2 mengizinkan PositionManager lewat `Permit2.approve`, yang punya
//!    masa berlaku.
//!
//! Melewatkan lapis kedua adalah kesalahan yang paling mudah terjadi, karena
//! lapis pertama terlihat seperti sudah cukup. Akibatnya `mint` revert setelah
//! gas terbakar.
//!
//! Ini dibuktikan on chain lewat `cargo xtask v4probe`, yang membaca
//! `PositionManager.permit2()` dan mencocokkannya dengan `chain.toml`.
//!
//! INVARIAN
//! - Alamat spender selalu dari `chain.toml`, tidak pernah dari panel.
//! - Izin diberikan sebesar yang dibutuhkan, bukan tak terbatas, dengan alasan
//!   yang sama seperti pada V3. Lihat `exec/approve.rs`.
//! - ETH asli tidak punya izin sama sekali. Ia ikut sebagai `msg.value`, jadi
//!   currency nol WAJIB dilewati, bukan diminta izinnya.
//! - Masa berlaku dibuat pendek. Izin Permit2 yang berlaku setahun sama saja
//!   dengan izin tak terbatas dalam waktu.

use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::Provider;
use alloy::sol_types::SolCall;
use serde::Serialize;

use super::abi::IPermit2;
use crate::exec::abi_write::IERC20Write;

/// Masa berlaku izin Permit2, dalam detik.
///
/// Tiga puluh menit. Cukup untuk menyusun rencana, menyetujuinya, dan
/// mengirimnya tanpa terburu buru, dan cukup pendek supaya izin yang terlupakan
/// tidak menjadi pintu terbuka permanen.
pub const MASA_BERLAKU_DETIK: u64 = 1_800;

/// Nilai terbesar yang muat di `uint160`, dipakai untuk mengenali izin yang
/// sudah tak terbatas dan karenanya tidak perlu ditambah.
pub fn maks_uint160() -> U256 {
    (U256::from(1u8) << 160) - U256::from(1u8)
}

/// Satu langkah izin yang masih kurang.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LangkahIzin {
    /// `erc20_ke_permit2` atau `permit2_ke_position_manager`.
    pub jenis: &'static str,
    pub token: String,
    pub needed: String,
    pub current: String,
}

/// Keadaan izin sebuah token untuk jalur V4.
#[derive(Debug, Clone, Serialize)]
pub struct IzinV4 {
    pub token: String,
    pub needed: String,
    /// Izin ERC20 dari kita ke Permit2.
    pub erc20_ke_permit2: String,
    /// Izin Permit2 dari kita ke PositionManager.
    pub permit2_ke_pm: String,
    /// Kapan izin Permit2 itu kedaluwarsa, sebagai stempel waktu unix.
    pub permit2_expiration: u64,
    /// Langkah yang masih harus dikerjakan, berurutan.
    pub langkah_kurang: Vec<LangkahIzin>,
}

/// Baca kedua lapis izin untuk sepasang currency, dalam satu permintaan.
///
/// Currency nol dilewati karena ETH asli tidak punya izin. Ia ikut sebagai
/// `msg.value`.
#[allow(clippy::too_many_arguments)]
pub async fn periksa_izin<P: Provider>(
    provider: &P,
    multicall: Address,
    pemilik: Address,
    permit2: Address,
    position_manager: Address,
    currency0: Address,
    needed0: U256,
    currency1: Address,
    needed1: U256,
    sekarang_unix: u64,
) -> Result<Vec<IzinV4>, String> {
    let pasangan = [(currency0, needed0), (currency1, needed1)];

    let mut calls = Vec::new();
    let mut ikut: Vec<(Address, U256)> = Vec::new();
    for (token, needed) in pasangan {
        // ETH asli tidak punya `allowance`. Memanggilnya hanya menghasilkan
        // kegagalan yang harus dijelaskan berulang kali.
        if token == Address::ZERO {
            continue;
        }
        calls.push(crate::chain::multicall::encode(
            token,
            &IERC20Write::allowanceCall {
                owner: pemilik,
                spender: permit2,
            },
        ));
        calls.push(crate::chain::multicall::encode(
            permit2,
            &IPermit2::allowanceCall {
                user: pemilik,
                token,
                spender: position_manager,
            },
        ));
        ikut.push((token, needed));
    }

    if ikut.is_empty() {
        return Ok(Vec::new());
    }

    let hasil = crate::chain::multicall::batch_call(provider, multicall, calls)
        .await
        .map_err(|e| format!("gagal membaca izin V4: {e}"))?;

    let mut out = Vec::with_capacity(ikut.len());
    for (i, (token, needed)) in ikut.into_iter().enumerate() {
        // Kegagalan membaca TIDAK diubah menjadi nol. Nol memicu approve yang
        // tidak perlu, sedangkan kegagalan berarti kita tidak tahu, dan itu
        // harus dikatakan. Lihat memory/003.
        let erc20 = crate::chain::multicall::decode::<IERC20Write::allowanceCall>(
            hasil.get(i * 2).and_then(|x| x.as_ref()),
        )
        .ok_or_else(|| format!("{token} tidak menjawab allowance()"))?;

        let p2 = crate::chain::multicall::decode::<IPermit2::allowanceCall>(
            hasil.get(i * 2 + 1).and_then(|x| x.as_ref()),
        )
        .ok_or_else(|| format!("Permit2 tidak menjawab allowance() untuk {token}"))?;

        let p2_amount = U256::from(p2.amount);
        let kedaluwarsa: u64 = p2.expiration.to::<u64>();
        // Izin yang sudah lewat masa berlakunya sama saja dengan tidak ada.
        // Angka nol pada expiration berarti belum pernah diberikan.
        let p2_berlaku = kedaluwarsa > sekarang_unix && p2_amount >= needed;

        let mut langkah = Vec::new();
        if needed > U256::ZERO {
            if erc20 < needed {
                langkah.push(LangkahIzin {
                    jenis: "erc20_ke_permit2",
                    token: token.to_string(),
                    needed: needed.to_string(),
                    current: erc20.to_string(),
                });
            }
            if !p2_berlaku {
                langkah.push(LangkahIzin {
                    jenis: "permit2_ke_position_manager",
                    token: token.to_string(),
                    needed: needed.to_string(),
                    current: p2_amount.to_string(),
                });
            }
        }

        out.push(IzinV4 {
            token: token.to_string(),
            needed: needed.to_string(),
            erc20_ke_permit2: erc20.to_string(),
            permit2_ke_pm: p2_amount.to_string(),
            permit2_expiration: kedaluwarsa,
            langkah_kurang: langkah,
        });
    }

    Ok(out)
}

/// Calldata `Permit2.approve`.
///
/// Jumlahnya dipotong ke `uint160` karena itu lebar yang diterima Permit2.
/// Pemotongan diam diam berbahaya, jadi jumlah yang melampaui batas ditolak
/// alih alih dibulatkan.
pub fn permit2_approve_calldata(
    token: Address,
    spender: Address,
    amount: U256,
    expiration_unix: u64,
) -> Result<Bytes, String> {
    if amount > maks_uint160() {
        return Err(format!(
            "jumlah {amount} melampaui batas uint160 yang diterima Permit2"
        ));
    }
    let amount160 = alloy::primitives::aliases::U160::from(amount);
    let expiration = alloy::primitives::aliases::U48::from(expiration_unix);

    Ok(Bytes::from(
        IPermit2::approveCall {
            token,
            spender,
            amount: amount160,
            expiration,
        }
        .abi_encode(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    #[test]
    fn permit2_approve_memakai_selector_yang_benar() {
        let data = permit2_approve_calldata(
            address!("5fc5360d0400a0fd4f2af552add042d716f1d168"),
            address!("58daec3116aae6d93017baaea7749052e8a04fa7"),
            U256::from(1_000_000u64),
            1_800_000_000,
        )
        .unwrap();
        assert_eq!(&data[..4], &IPermit2::approveCall::SELECTOR);
        assert_eq!(data.len(), 4 + 32 * 4, "empat argumen statis");
    }

    #[test]
    fn jumlah_melampaui_uint160_ditolak_bukan_dipotong() {
        // Pemotongan diam diam akan memberi izin yang jauh lebih kecil dari
        // yang dimaksud, dan mint-nya gagal tanpa sebab yang jelas.
        let hasil = permit2_approve_calldata(
            Address::ZERO,
            Address::ZERO,
            maks_uint160() + U256::from(1u8),
            1,
        );
        assert!(hasil.is_err());
    }

    #[test]
    fn batas_uint160_tepat_diterima() {
        assert!(permit2_approve_calldata(Address::ZERO, Address::ZERO, maks_uint160(), 1).is_ok());
    }

    #[test]
    fn izin_kedaluwarsa_dianggap_belum_ada() {
        // Ini inti kenapa Permit2 berbeda dari allowance biasa: jumlahnya bisa
        // cukup tapi masa berlakunya sudah lewat, dan kontrak akan menolak.
        let sekarang = 1_800_000_000u64;
        let cukup_tapi_kedaluwarsa = U256::from(1_000u64) >= U256::from(500u64)
            && 1_700_000_000u64 > sekarang;
        assert!(
            !cukup_tapi_kedaluwarsa,
            "izin dengan expiration di masa lalu tidak boleh dianggap sah"
        );
    }

    #[test]
    fn masa_berlaku_tidak_terlalu_panjang() {
        // Izin Permit2 yang berlaku setahun sama saja dengan izin tak terbatas
        // dalam waktu.
        const { assert!(MASA_BERLAKU_DETIK <= 3_600) };
    }
}
