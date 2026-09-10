//! Pembuktian transfer token pada jalur LP dan jual kecil, PRD 5.4.
//!
//! INVARIAN
//! - Uji LP memakai simulasi calldata mint yang persis akan dikirim. Core
//!   Uniswap menolak penyelesaian bila saldo yang diterima kurang dari delta
//!   yang diminta, sehingga transfer tax pada jalur itu tidak bisa lolos diam
//!   diam.
//! - Uji jual memakai router yang benar benar dipakai protokol, dari alamat
//!   wallet, dengan allowance nyata dan batas keluaran dari quoter. `eth_call`
//!   menjalankan seluruh transfer tanpa menyimpan perubahan state.
//! - Kegagalan membaca atau mensimulasikan adalah penolakan, bukan nilai nol.

use alloy::primitives::aliases::U160;
use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::Provider;
use alloy::rpc::types::TransactionRequest;
use alloy::sol;
use alloy::sol_types::SolCall;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::chain::registry::Registry;
use crate::pools::{PoolInfo, PoolVersion};

sol! {
    #[sol(rpc)]
    interface IQuoterV2 {
        struct QuoteExactInputSingleParams {
            address tokenIn;
            address tokenOut;
            uint256 amountIn;
            uint24 fee;
            uint160 sqrtPriceLimitX96;
        }
        function quoteExactInputSingle(QuoteExactInputSingleParams params)
            external
            returns (
                uint256 amountOut,
                uint160 sqrtPriceX96After,
                uint32 initializedTicksCrossed,
                uint256 gasEstimate
            );
    }

    interface ISwapRouter02 {
        struct ExactInputSingleParams {
            address tokenIn;
            address tokenOut;
            uint24 fee;
            address recipient;
            uint256 amountIn;
            uint256 amountOutMinimum;
            uint160 sqrtPriceLimitX96;
        }
        function exactInputSingle(ExactInputSingleParams params)
            external
            payable
            returns (uint256 amountOut);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuktiToken {
    pub chain_id: u64,
    pub pool_id: String,
    pub token: String,
    pub jalur_lp_lolos: bool,
    /// Nol jika simulasi penyelesaian jumlah penuh berhasil. `None` berarti
    /// jalurnya belum terbukti, bukan angka pajak nol.
    pub transfer_tax_bps: Option<u32>,
    pub jual_kecil_lolos: bool,
    pub diperiksa_pada: String,
}

fn kunci_bukti(chain_id: u64, pool_id: &str, token: Address) -> String {
    format!(
        "risk/token:v1:{chain_id}:{}:{}",
        pool_id.to_lowercase(),
        token.to_string().to_lowercase()
    )
}

pub fn simpan_bukti(
    store: &crate::store::Store,
    chain_id: u64,
    pool_id: &str,
    token: Address,
    jalur_lp_lolos: bool,
    jual_kecil_lolos: bool,
) -> Result<()> {
    store.put(
        &kunci_bukti(chain_id, pool_id, token),
        &BuktiToken {
            chain_id,
            pool_id: pool_id.to_lowercase(),
            token: token.to_string().to_lowercase(),
            jalur_lp_lolos,
            transfer_tax_bps: jalur_lp_lolos.then_some(0),
            jual_kecil_lolos,
            diperiksa_pada: chrono::Utc::now().to_rfc3339(),
        },
    )
}

pub async fn uji_jual_kecil<P: Provider>(
    provider: &P,
    registry: &Registry,
    pool: &PoolInfo,
    pemilik: Address,
    memecoin: Address,
    jumlah: U256,
    slippage_bps: u32,
) -> Result<U256> {
    anyhow::ensure!(!jumlah.is_zero(), "jumlah uji jual kecil nol");
    let token0: Address = pool.token0.parse().context("token0 pool tidak valid")?;
    let token1: Address = pool.token1.parse().context("token1 pool tidak valid")?;
    anyhow::ensure!(
        memecoin == token0 || memecoin == token1,
        "memecoin bukan anggota pool"
    );
    let token_keluar = if memecoin == token0 { token1 } else { token0 };

    match pool.version {
        PoolVersion::V3 => {
            let quoter = registry.contract("uniswap_v3", "quoter_v2")?;
            let router = registry.contract("uniswap_v3", "swap_router_02")?;
            let q = IQuoterV2::new(quoter, provider)
                .quoteExactInputSingle(IQuoterV2::QuoteExactInputSingleParams {
                    tokenIn: memecoin,
                    tokenOut: token_keluar,
                    amountIn: jumlah,
                    fee: alloy::primitives::aliases::U24::from(pool.fee),
                    sqrtPriceLimitX96: U160::ZERO,
                })
                .call()
                .await
                .context("quoter V3 menolak jual kecil")?;
            anyhow::ensure!(!q.amountOut.is_zero(), "jual kecil menghasilkan nol");
            let minimum = crate::exec::limits::apply_slippage(q.amountOut, slippage_bps);
            let data = ISwapRouter02::exactInputSingleCall {
                params: ISwapRouter02::ExactInputSingleParams {
                    tokenIn: memecoin,
                    tokenOut: token_keluar,
                    fee: alloy::primitives::aliases::U24::from(pool.fee),
                    recipient: pemilik,
                    amountIn: jumlah,
                    amountOutMinimum: minimum,
                    sqrtPriceLimitX96: U160::ZERO,
                },
            }
            .abi_encode();
            provider
                .call(
                    TransactionRequest::default()
                        .from(pemilik)
                        .to(router)
                        .input(Bytes::from(data).into()),
                )
                .await
                .context("SwapRouter02 menolak jual kecil")?;
            Ok(q.amountOut)
        }
        PoolVersion::V4 => {
            let key = crate::exec::v4::pipeline::pool_key_dari(pool).map_err(anyhow::Error::msg)?;
            let quoter = registry.contract("uniswap_v4", "quoter")?;
            let router = registry.contract("uniswap_v4", "universal_router")?;
            let zero_for_one = memecoin == token0;
            let amount_out = crate::exec::v4::quote::kutip_exact_in(
                provider,
                quoter,
                &key,
                zero_for_one,
                jumlah,
            )
            .await
            .map_err(anyhow::Error::msg)?;
            anyhow::ensure!(!amount_out.is_zero(), "jual kecil menghasilkan nol");
            let minimum = crate::exec::limits::apply_slippage(amount_out, slippage_bps);
            let req = crate::exec::router_rbh::SwapExactIn {
                pool: crate::exec::router_rbh::KunciPool::dari_pool_key(&key),
                zero_for_one,
                amount_in: jumlah
                    .try_into()
                    .context("jumlah uji jual melampaui uint128")?,
                amount_out_minimum: minimum
                    .try_into()
                    .context("minimum jual melampaui uint128")?,
                deadline: U256::from(chrono::Utc::now().timestamp().max(0) as u64 + 180),
            };
            let value = req.value();
            let data = crate::exec::router_rbh::swap_exact_in(&req)?;
            provider
                .call(
                    TransactionRequest::default()
                        .from(pemilik)
                        .to(router)
                        .value(value)
                        .input(data.into()),
                )
                .await
                .context("UniversalRouter menolak jual kecil")?;
            Ok(amount_out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kunci_bukti_terikat_ke_chain_pool_dan_token() {
        let a = kunci_bukti(4663, "0xAB", Address::repeat_byte(1));
        let b = kunci_bukti(4663, "0xab", Address::repeat_byte(2));
        assert_ne!(a, b);
        assert!(a.contains("risk/token:v1:4663:0xab:"));
    }

    #[test]
    fn bukti_nol_hanya_disimpan_setelah_jalur_lp_lolos() {
        let dir =
            std::env::temp_dir().join(format!("rangefinder-risk-token-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = crate::store::Store::open(&dir).unwrap();
        let token = Address::repeat_byte(7);
        simpan_bukti(&store, 4663, "0xpool", token, true, false).unwrap();
        let bukti: BuktiToken = store
            .get(&kunci_bukti(4663, "0xpool", token))
            .unwrap()
            .unwrap();
        assert_eq!(bukti.transfer_tax_bps, Some(0));
        assert!(!bukti.jual_kecil_lolos);

        simpan_bukti(&store, 4663, "0xpool", token, false, false).unwrap();
        let bukti: BuktiToken = store
            .get(&kunci_bukti(4663, "0xpool", token))
            .unwrap()
            .unwrap();
        assert_eq!(bukti.transfer_tax_bps, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn selector_swap_router02_sesuai_interface_resmi() {
        assert_eq!(
            ISwapRouter02::exactInputSingleCall::SELECTOR,
            hex::decode("04e45aaf").unwrap().as_slice()
        );
    }
}
