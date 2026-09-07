//! Kutipan swap V4, dipakai penyelesai zap.
//!
//! KENAPA HARUS DARI RANTAI
//! Berapa token lawan yang diterima dari sebuah swap bergantung pada bentuk
//! likuiditas di sepanjang jalur harga, dan pada pool berhook juga pada apa yang
//! hook itu lakukan. Tidak ada rumus yang bisa menebaknya dari luar. Tebakan
//! yang meleset menghasilkan posisi timpang beserta sisa token menganggur, dan
//! sisa itu tidak menghasilkan fee apa pun.
//!
//! INVARIAN
//! - `quoteExactInputSingle` TIDAK bertanda `view` di kontraknya. Ia benar benar
//!   menjalankan swap lalu revert untuk membatalkannya, dan hasilnya dibaca dari
//!   data revert. Karena itu ia hanya boleh dipanggil lewat `eth_call`, tidak
//!   pernah dikirim sebagai transaksi. Mengirimnya berarti membakar gas untuk
//!   sesuatu yang memang dirancang gagal.
//! - Kegagalan kutipan dikembalikan sebagai kesalahan, tidak pernah sebagai
//!   nol. Nol berarti swap tidak menghasilkan apa apa, dan penyelesai zap akan
//!   memperlakukannya sebagai jawaban yang sah.

use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::Provider;

use super::abi::{IPositionManagerV4::PoolKey, IV4Quoter};

/// Ubah PoolKey jalur posisi menjadi bentuk yang diterima quoter.
///
/// Keduanya struct yang isinya sama persis, tapi dihasilkan dari dua blok `sol!`
/// yang berbeda sehingga tipenya berbeda bagi Rust. Konversi ditulis di satu
/// tempat supaya tidak ada yang menyusunnya ulang dengan urutan field yang
/// keliru.
pub fn key_untuk_quoter(k: &PoolKey) -> IV4Quoter::PoolKeyQ {
    IV4Quoter::PoolKeyQ {
        currency0: k.currency0,
        currency1: k.currency1,
        fee: k.fee,
        tickSpacing: k.tickSpacing,
        hooks: k.hooks,
    }
}

/// Berapa token lawan yang diterima kalau menukar `jumlah_masuk`.
///
/// `zero_for_one` benar berarti menukar currency0 menjadi currency1.
pub async fn kutip_exact_in<P: Provider>(
    provider: &P,
    quoter: Address,
    pool_key: &PoolKey,
    zero_for_one: bool,
    jumlah_masuk: U256,
) -> Result<U256, String> {
    if jumlah_masuk.is_zero() {
        // Menukar nol memang menghasilkan nol, dan quoter bisa menolaknya.
        // Dijawab di sini supaya penyelesai zap tidak perlu menangani kasus itu.
        return Ok(U256::ZERO);
    }

    let exact: u128 = jumlah_masuk
        .try_into()
        .map_err(|_| format!("jumlah {jumlah_masuk} melampaui uint128 yang diterima quoter"))?;

    let q = IV4Quoter::new(quoter, provider);
    let hasil = q
        .quoteExactInputSingle(IV4Quoter::QuoteExactSingleParams {
            poolKey: key_untuk_quoter(pool_key),
            zeroForOne: zero_for_one,
            exactAmount: exact,
            hookData: Bytes::new(),
        })
        .call()
        .await
        .map_err(|e| {
            let teks = e.to_string();
            if teks.contains("revert") || teks.contains("execution reverted") {
                format!(
                    "quoter menolak swap {jumlah_masuk}: {}",
                    crate::exec::simulate::jelaskan_revert(&teks)
                )
            } else {
                format!("quoter tidak bisa dihubungi: {teks}")
            }
        })?;

    Ok(hasil.amountOut)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;
    use alloy::primitives::aliases::{I24, U24};

    fn key() -> PoolKey {
        PoolKey {
            currency0: address!("5fc5360d0400a0fd4f2af552add042d716f1d168"),
            currency1: address!("791e750fa8b2237288a6fe6c47ea81c50093449a"),
            fee: U24::from(100_000u32),
            tickSpacing: I24::try_from(1000i32).unwrap(),
            hooks: Address::ZERO,
        }
    }

    #[test]
    fn konversi_key_mempertahankan_setiap_field() {
        // poolId adalah hash dari kelima field ini. Satu yang tertukar berarti
        // kutipan untuk pool yang sama sekali lain.
        let k = key();
        let q = key_untuk_quoter(&k);
        assert_eq!(q.currency0, k.currency0);
        assert_eq!(q.currency1, k.currency1);
        assert_eq!(q.fee, k.fee);
        assert_eq!(q.tickSpacing, k.tickSpacing);
        assert_eq!(q.hooks, k.hooks);
    }
}
