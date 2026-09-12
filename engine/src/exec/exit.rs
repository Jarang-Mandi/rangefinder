//! Konversi aset hasil penutupan posisi ke ETH.
//!
//! User menutup posisi untuk keluar dari token spekulatif, bukan untuk
//! memindahkan risiko itu dari NFT ke saldo wallet. Karena itu token biasa
//! dijual melalui pool V4 langsung ke ETH setelah close berhasil. ETH, WETH,
//! dan USDG dipertahankan sebagai aset utama yang memang berguna.
//!
//! INVARIAN
//! - Jumlah swap berasal dari delta saldo close, bukan seluruh saldo wallet.
//! - Pool dipilih dari kutipan on chain untuk jumlah yang benar benar diterima.
//! - Batas keluaran selalu menerapkan slippage maksimum konfigurasi.
//! - Tidak ada swap jika rute langsung ke ETH tidak dapat dibuktikan.

use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::Provider;

use crate::chain::registry::Registry;

#[derive(Debug, Clone)]
pub struct RencanaKeluar {
    pub token: Address,
    pub jumlah: U256,
    pub perkiraan_eth: U256,
    pub minimum_eth: U256,
    pub pool_id: String,
    pub calldata: Bytes,
}

/// Token yang perlu dijual dari dua sisi hasil close.
///
/// ETH, WETH, dan USDG dipertahankan. Token lain, termasuk kedua sisi pada
/// pasangan seperti PONS/ZZZ, masuk daftar secara terpisah.
pub fn token_yang_ditukar(
    registry: &Registry,
    token0: Address,
    amount0: U256,
    token1: Address,
    amount1: U256,
) -> Vec<(Address, U256)> {
    let weth = registry
        .tokens
        .get("weth")
        .and_then(|entry| entry.address.parse::<Address>().ok());
    let usdg = registry
        .tokens
        .get("usdg")
        .and_then(|entry| entry.address.parse::<Address>().ok());
    let dipertahankan =
        |token: Address| token.is_zero() || Some(token) == weth || Some(token) == usdg;

    [(token0, amount0), (token1, amount1)]
        .into_iter()
        .filter(|(token, amount)| !amount.is_zero() && !dipertahankan(*token))
        .collect()
}

/// Cari pool langsung token/ETH yang memberi keluaran terbesar untuk jumlah
/// aktual, lalu bangun calldata UniversalRouter.
pub async fn susun_ke_eth<P: Provider>(
    provider: &P,
    registry: &Registry,
    token: Address,
    jumlah: U256,
    slippage_bps: u32,
    deadline: U256,
) -> Result<RencanaKeluar, String> {
    if jumlah.is_zero() {
        return Err("jumlah token hasil close nol".into());
    }
    let quoter = registry
        .contract("uniswap_v4", "quoter")
        .map_err(|e| e.to_string())?;
    let pools = crate::pools::discover_v4(provider, registry, token, Address::ZERO)
        .await
        .map_err(|e| format!("pool langsung ke ETH tidak dapat dicari: {e}"))?;
    let pools = crate::pools::usable_pools(pools)
        .into_iter()
        // Close tidak boleh diam diam memberi wewenang kepada hook asing.
        // Rute tanpa hook masih melalui simulasi calldata lengkap di API.
        .filter(|pool| !crate::pools::has_hook(pool));

    let mut terbaik: Option<(crate::pools::PoolInfo, U256)> = None;
    for pool in pools {
        let key = crate::exec::v4::pipeline::pool_key_dari(&pool)?;
        let zero_for_one = key.currency0 == token;
        match crate::exec::v4::quote::kutip_exact_in(provider, quoter, &key, zero_for_one, jumlah)
            .await
        {
            Ok(out) if !out.is_zero() => {
                if terbaik.as_ref().is_none_or(|(_, lama)| out > *lama) {
                    terbaik = Some((pool, out));
                }
            }
            Ok(_) => {}
            Err(error) => {
                tracing::debug!(pool = %pool.id, %error, "pool keluar tidak memberi kutipan");
            }
        }
    }

    let (pool, perkiraan_eth) =
        terbaik.ok_or_else(|| format!("tidak ada rute langsung dari {token} ke ETH"))?;
    let minimum_eth = crate::exec::limits::apply_slippage(perkiraan_eth, slippage_bps);
    if minimum_eth.is_zero() {
        return Err("minimum ETH hasil konversi nol".into());
    }
    let key = crate::exec::v4::pipeline::pool_key_dari(&pool)?;
    let req = crate::exec::router_rbh::SwapExactIn {
        pool: crate::exec::router_rbh::KunciPool::dari_pool_key(&key),
        zero_for_one: key.currency0 == token,
        amount_in: jumlah
            .try_into()
            .map_err(|_| "jumlah token hasil close melampaui uint128".to_string())?,
        amount_out_minimum: minimum_eth
            .try_into()
            .map_err(|_| "minimum ETH hasil close melampaui uint128".to_string())?,
        deadline,
    };
    let calldata = crate::exec::router_rbh::swap_exact_in(&req).map_err(|e| e.to_string())?;

    Ok(RencanaKeluar {
        token,
        jumlah,
        perkiraan_eth,
        minimum_eth,
        pool_id: pool.id,
        calldata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> Registry {
        Registry::load_from(&Registry::default_path(&crate::config::repo_root())).unwrap()
    }

    #[test]
    fn usdg_eth_dan_weth_dipertahankan() {
        let r = registry();
        let weth = r.tokens["weth"].address.parse().unwrap();
        let usdg = r.tokens["usdg"].address.parse().unwrap();
        let biasa = Address::repeat_byte(0x77);

        assert!(
            token_yang_ditukar(&r, usdg, U256::from(1), biasa, U256::from(2))
                .iter()
                .all(|(token, _)| *token == biasa)
        );
        assert!(
            token_yang_ditukar(&r, Address::ZERO, U256::from(1), biasa, U256::from(2))
                .iter()
                .all(|(token, _)| *token == biasa)
        );
        assert!(
            token_yang_ditukar(&r, weth, U256::from(1), biasa, U256::from(2))
                .iter()
                .all(|(token, _)| *token == biasa)
        );
    }

    #[test]
    fn dua_token_biasa_keduanya_ditukar() {
        let r = registry();
        let daftar = token_yang_ditukar(
            &r,
            Address::repeat_byte(0x11),
            U256::from(3),
            Address::repeat_byte(0x22),
            U256::from(4),
        );
        assert_eq!(daftar.len(), 2);
        assert_eq!(daftar[0].1, U256::from(3));
        assert_eq!(daftar[1].1, U256::from(4));
    }

    #[test]
    fn saldo_nol_tidak_memicu_swap() {
        let r = registry();
        assert!(token_yang_ditukar(
            &r,
            Address::repeat_byte(0x11),
            U256::ZERO,
            Address::ZERO,
            U256::from(1)
        )
        .is_empty());
    }
}
