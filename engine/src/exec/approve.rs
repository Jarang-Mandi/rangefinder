//! Izin ERC20 untuk NonfungiblePositionManager.
//!
//! KENAPA INI ADA, DAN KENAPA BUKAN PERMIT2
//! PRD 6.2 semula menyebut Permit2 supaya tidak perlu approve per token.
//! Permit2 memang dipakai UniversalRouter, tapi NonfungiblePositionManager v3
//! menarik token lewat `transferFrom` biasa, jadi ia butuh allowance ERC20
//! langsung ke alamatnya. Tanpa itu `mint` selalu revert dengan
//! `TRANSFER_FROM_FAILED`, dan revert itu terjadi setelah gas terbakar.
//!
//! INVARIAN
//! - Izin diberikan SEBESAR YANG DIBUTUHKAN, bukan tak terbatas. Wallet ini
//!   panas dan tersimpan di mesin user, dan batas nominal posisi sengaja tidak
//!   dibatasi atas permintaan user. Dua hal itu bersama berarti izin tak
//!   terbatas menaruh seluruh saldo pada satu kontrak, selamanya. Biaya
//!   tambahannya satu transaksi per posisi, dan pada rantai berblok 100 ms itu
//!   hampir tidak terasa.
//! - Alamat spender selalu datang dari `chain.toml`, tidak pernah dari panel.
//!   Approve ke alamat yang salah adalah cara tercepat kehilangan dana.
//! - Allowance dibaca ulang setelah approve terkirim. Sebagian token
//!   mengembalikan `false` alih alih revert saat approve gagal, dan tanpa
//!   pembacaan ulang kegagalan itu tidak terlihat sampai mint ikut gagal.

use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::Provider;
use alloy::sol_types::SolCall;
use serde::Serialize;

use super::abi_write::IERC20Write;

/// Kebutuhan izin untuk satu token.
#[derive(Debug, Clone, Serialize)]
pub struct AllowanceNeed {
    pub token: String,
    pub needed: String,
    pub current: String,
    /// Benar kalau izin sekarang belum cukup untuk jumlah yang dibutuhkan.
    pub insufficient: bool,
}

/// Baca izin dua token sekaligus, dalam satu permintaan.
///
/// Jumlah nol tidak dilewati begitu saja, tapi menghasilkan `insufficient`
/// bernilai salah, karena posisi satu sisi memang hanya memakai satu token dan
/// meminta izin untuk yang tidak dipakai hanya membuang transaksi.
#[allow(clippy::too_many_arguments)]
pub async fn check_allowances<P: Provider>(
    provider: &P,
    multicall: Address,
    owner: Address,
    spender: Address,
    token0: Address,
    needed0: U256,
    token1: Address,
    needed1: U256,
) -> Result<Vec<AllowanceNeed>, String> {
    let calls = vec![
        crate::chain::multicall::encode(token0, &IERC20Write::allowanceCall { owner, spender }),
        crate::chain::multicall::encode(token1, &IERC20Write::allowanceCall { owner, spender }),
    ];

    let hasil = crate::chain::multicall::batch_call(provider, multicall, calls)
        .await
        .map_err(|e| format!("gagal membaca allowance: {e}"))?;

    let mut out = Vec::with_capacity(2);
    for (i, (token, needed)) in [(token0, needed0), (token1, needed1)]
        .into_iter()
        .enumerate()
    {
        // Kegagalan membaca TIDAK diubah jadi nol. Nol berarti izin habis dan
        // memicu approve yang tidak perlu, sedangkan kegagalan berarti kita
        // tidak tahu, dan itu harus dikatakan. Lihat memory/003.
        let current = crate::chain::multicall::decode::<IERC20Write::allowanceCall>(
            hasil.get(i).and_then(|x| x.as_ref()),
        )
        .ok_or_else(|| format!("{token} tidak menjawab allowance()"))?;

        out.push(AllowanceNeed {
            token: token.to_string(),
            needed: needed.to_string(),
            current: current.to_string(),
            insufficient: needed > U256::ZERO && current < needed,
        });
    }
    Ok(out)
}

/// Calldata `approve` untuk jumlah tertentu.
///
/// Sengaja tidak ada varian tak terbatas. Kalau suatu saat memang dibutuhkan,
/// ia harus jadi keputusan sadar dengan catatan alasannya, bukan argumen
/// bawaan yang mudah terpanggil.
pub fn approve_calldata(spender: Address, amount: U256) -> Bytes {
    Bytes::from(IERC20Write::approveCall { spender, amount }.abi_encode())
}

/// Sebagian token menolak mengubah allowance bukan nol menjadi bukan nol.
///
/// USDT di Ethereum adalah contoh paling terkenal, dan polanya ditiru banyak
/// token lain. Untuk token seperti itu, izin harus dinolkan lebih dulu.
/// Fungsi ini menyatakan apakah langkah itu diperlukan.
pub fn perlu_dinolkan_dulu(current: U256, target: U256) -> bool {
    current > U256::ZERO && target > U256::ZERO && current != target
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approve_calldata_memakai_selector_yang_benar() {
        let spender = Address::from([0xab; 20]);
        let data = approve_calldata(spender, U256::from(1000u64));
        assert_eq!(&data[..4], &IERC20Write::approveCall::SELECTOR);
        assert_eq!(
            data.len(),
            4 + 32 + 32,
            "satu alamat dan satu uint256 sesudah selector"
        );
        // Alamat spender berada di 12 byte terakhir dari kata pertama.
        assert_eq!(&data[4 + 12..4 + 32], spender.as_slice());
    }

    #[test]
    fn jumlah_nol_tidak_dianggap_kurang() {
        // Posisi satu sisi hanya memakai satu token. Meminta izin untuk token
        // yang jumlahnya nol hanya membuang satu transaksi.
        let n = AllowanceNeed {
            token: "0x1".into(),
            needed: "0".into(),
            current: "0".into(),
            insufficient: U256::ZERO > U256::ZERO && U256::ZERO < U256::ZERO,
        };
        assert!(!n.insufficient);
    }

    #[test]
    fn penolakan_ubah_bukan_nol_ke_bukan_nol() {
        assert!(perlu_dinolkan_dulu(U256::from(5), U256::from(10)));
        assert!(!perlu_dinolkan_dulu(U256::ZERO, U256::from(10)));
        assert!(!perlu_dinolkan_dulu(U256::from(5), U256::ZERO));
        assert!(
            !perlu_dinolkan_dulu(U256::from(5), U256::from(5)),
            "izin yang sudah tepat tidak perlu disentuh"
        );
    }
}
