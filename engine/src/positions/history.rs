//! Rekonstruksi cost basis posisi yang dibuat di luar Rangefinder.
//!
//! V3 memancarkan jumlah token dan delta likuiditas pada setiap perubahan.
//! Seluruh histori satu NFT dapat dibangun kembali tanpa mempercayai catatan
//! lokal. Basis sisa memakai metode biaya rata rata per unit likuiditas: ketika
//! sebagian likuiditas dilepas, porsi basis yang sama juga dilepas.

use std::collections::HashMap;

use alloy::consensus::Transaction;
use alloy::primitives::{Address, Bytes, Sign, B256, U256};
use alloy::providers::Provider;
use alloy::rpc::types::{Filter, Log};
use alloy::sol_types::{SolCall, SolEvent, SolValue};
use anyhow::{Context, Result};

use crate::chain::registry::Registry;
use crate::exec::abi_write::{Collect, DecreaseLiquidity, IncreaseLiquidity};
use crate::exec::v4::abi::{IPositionManagerV4 as PmV4, ModifyLiquidity};
use crate::exec::v4::actions::kode;
use crate::pnl::{Catatan, RekonstruksiLog, RekonstruksiV4};
use crate::pools::PoolVersion;

use super::Posisi;

#[derive(Debug, Clone)]
enum PerubahanV3 {
    Increase {
        liquidity: u128,
        amount0: U256,
        amount1: U256,
    },
    Decrease {
        liquidity: u128,
        amount0: U256,
        amount1: U256,
    },
    Collect {
        amount0: U256,
        amount1: U256,
    },
}

#[derive(Debug, Clone)]
struct EventV3 {
    token_id: U256,
    block: u64,
    tx_index: u64,
    log_index: u64,
    timestamp: Option<u64>,
    tx_hash: Option<B256>,
    perubahan: PerubahanV3,
}

#[derive(Debug, PartialEq, Eq)]
struct BasisV3 {
    setor0: U256,
    setor1: U256,
    collected0: U256,
    collected1: U256,
    decreased0: U256,
    decreased1: U256,
    liquidity: u128,
    first_block: u64,
    first_timestamp: Option<u64>,
    first_tx: Option<B256>,
}

fn urutan(log: &Log) -> (u64, u64, u64) {
    (
        log.block_number.unwrap_or_default(),
        log.transaction_index.unwrap_or_default(),
        log.log_index.unwrap_or_default(),
    )
}

fn decode(log: Log) -> Option<EventV3> {
    if log.removed {
        return None;
    }
    let (block, tx_index, log_index) = urutan(&log);
    let timestamp = log.block_timestamp;
    let tx_hash = log.transaction_hash;
    if let Ok(event) = IncreaseLiquidity::decode_log(&log.inner) {
        return Some(EventV3 {
            token_id: event.tokenId,
            block,
            tx_index,
            log_index,
            timestamp,
            tx_hash,
            perubahan: PerubahanV3::Increase {
                liquidity: event.liquidity,
                amount0: event.amount0,
                amount1: event.amount1,
            },
        });
    }
    if let Ok(event) = DecreaseLiquidity::decode_log(&log.inner) {
        return Some(EventV3 {
            token_id: event.tokenId,
            block,
            tx_index,
            log_index,
            timestamp,
            tx_hash,
            perubahan: PerubahanV3::Decrease {
                liquidity: event.liquidity,
                amount0: event.amount0,
                amount1: event.amount1,
            },
        });
    }
    let event = Collect::decode_log(&log.inner).ok()?;
    Some(EventV3 {
        token_id: event.tokenId,
        block,
        tx_index,
        log_index,
        timestamp,
        tx_hash,
        perubahan: PerubahanV3::Collect {
            amount0: event.amount0,
            amount1: event.amount1,
        },
    })
}

fn jumlah(a: U256, b: U256, nama: &str) -> Result<U256> {
    a.checked_add(b)
        .with_context(|| format!("{nama} melampaui uint256"))
}

fn rakit(events: &mut [EventV3]) -> Result<BasisV3> {
    events.sort_by_key(|e| (e.block, e.tx_index, e.log_index));
    let first = events
        .iter()
        .find(|e| matches!(e.perubahan, PerubahanV3::Increase { .. }))
        .context("histori tidak mempunyai IncreaseLiquidity")?;
    let mut out = BasisV3 {
        setor0: U256::ZERO,
        setor1: U256::ZERO,
        collected0: U256::ZERO,
        collected1: U256::ZERO,
        decreased0: U256::ZERO,
        decreased1: U256::ZERO,
        liquidity: 0,
        first_block: first.block,
        first_timestamp: first.timestamp,
        first_tx: first.tx_hash,
    };

    for event in events {
        match &event.perubahan {
            PerubahanV3::Increase {
                liquidity,
                amount0,
                amount1,
            } => {
                out.liquidity = out
                    .liquidity
                    .checked_add(*liquidity)
                    .context("likuiditas histori melampaui uint128")?;
                out.setor0 = jumlah(out.setor0, *amount0, "basis token0")?;
                out.setor1 = jumlah(out.setor1, *amount1, "basis token1")?;
            }
            PerubahanV3::Decrease {
                liquidity,
                amount0,
                amount1,
            } => {
                anyhow::ensure!(
                    *liquidity <= out.liquidity,
                    "delta decrease melebihi likuiditas yang sudah tercatat"
                );
                let lama = out.liquidity;
                let sisa = lama - *liquidity;
                if sisa == 0 {
                    out.setor0 = U256::ZERO;
                    out.setor1 = U256::ZERO;
                } else {
                    out.setor0 = crate::math::liquidity_math::mul_div(
                        out.setor0,
                        U256::from(sisa),
                        U256::from(lama),
                    )
                    .map_err(anyhow::Error::msg)?;
                    out.setor1 = crate::math::liquidity_math::mul_div(
                        out.setor1,
                        U256::from(sisa),
                        U256::from(lama),
                    )
                    .map_err(anyhow::Error::msg)?;
                }
                out.liquidity = sisa;
                out.decreased0 = jumlah(out.decreased0, *amount0, "decrease token0")?;
                out.decreased1 = jumlah(out.decreased1, *amount1, "decrease token1")?;
            }
            PerubahanV3::Collect { amount0, amount1 } => {
                out.collected0 = jumlah(out.collected0, *amount0, "collect token0")?;
                out.collected1 = jumlah(out.collected1, *amount1, "collect token1")?;
            }
        }
    }
    Ok(out)
}

/// Baca histori posisi V3 eksternal aktif yang belum dikenal dan simpan basisnya.
///
/// Posisi yang sudah mempunyai catatan tidak dipindai lagi. Setelah ditemukan,
/// perubahan yang dijalankan Rangefinder diterapkan ke catatan dan jurnal lokal.
/// Pembatasan ini mencegah pemindaian dari blok deployment setiap refresh UI.
pub async fn rekonstruksi_basis_v3<P: Provider>(
    provider: &P,
    registry: &Registry,
    store: &crate::store::Store,
    positions: &[Posisi],
) -> Result<()> {
    let npm = registry.contract("uniswap_v3", "position_manager")?;
    let mut eligible = Vec::new();
    for position in positions.iter().filter(|p| p.version == PoolVersion::V3) {
        let existing = crate::pnl::ambil(store, &position.position_id);
        if existing.is_some() {
            continue;
        }
        let token_id = position
            .token_id
            .parse::<U256>()
            .with_context(|| format!("token id V3 {} tidak valid", position.token_id))?;
        eligible.push((position, token_id));
    }
    if eligible.is_empty() {
        return Ok(());
    }

    let latest = provider
        .get_block_number()
        .await
        .context("blok terakhir untuk rekonstruksi tidak terbaca")?;
    let ids: Vec<B256> = eligible
        .iter()
        .map(|(_, id)| B256::from(id.to_be_bytes::<32>()))
        .collect();
    let signatures = vec![
        IncreaseLiquidity::SIGNATURE_HASH,
        DecreaseLiquidity::SIGNATURE_HASH,
        Collect::SIGNATURE_HASH,
    ];
    let mut logs = Vec::new();
    let mut sampai = latest;
    let lantai = registry.chain.deploy_block;
    while lantai <= sampai {
        let hasil = crate::chain::logs::baca_mundur(
            provider,
            sampai,
            lantai,
            super::LOOKBACK_TRANSFER,
            100,
            |lo, hi| {
                Filter::new()
                    .address(npm)
                    .event_signature(signatures.clone())
                    .topic1(ids.clone())
                    .from_block(lo)
                    .to_block(hi)
            },
        )
        .await?;
        logs.extend(hasil.logs);
        if hasil.dari_blok <= lantai {
            break;
        }
        sampai = hasil.dari_blok - 1;
    }

    let mut per_token = HashMap::<U256, Vec<EventV3>>::new();
    for event in logs.into_iter().filter_map(decode) {
        per_token.entry(event.token_id).or_default().push(event);
    }

    for (position, token_id) in eligible {
        let events = per_token
            .get_mut(&token_id)
            .with_context(|| format!("histori V3 #{} kosong", position.token_id))?;
        let basis = rakit(events)
            .with_context(|| format!("histori V3 #{} tidak konsisten", position.token_id))?;
        let current_liquidity = position
            .liquidity
            .parse::<u128>()
            .with_context(|| format!("likuiditas V3 #{} tidak valid", position.token_id))?;
        anyhow::ensure!(
            basis.liquidity == current_liquidity,
            "likuiditas hasil histori V3 #{} adalah {}, state sekarang {}",
            position.token_id,
            basis.liquidity,
            current_liquidity
        );
        let dibuka_pada = basis
            .first_timestamp
            .and_then(|timestamp| chrono::DateTime::from_timestamp(timestamp as i64, 0))
            .map(|timestamp| timestamp.to_rfc3339())
            .unwrap_or_else(|| format!("blok {}", basis.first_block));
        crate::pnl::simpan(
            store,
            &Catatan {
                position_id: position.position_id.clone(),
                token_id: position.token_id.clone(),
                flow_id: None,
                setor0: basis.setor0.to_string(),
                setor1: basis.setor1.to_string(),
                harga_buka: None,
                lengkap: false,
                setoran_aktual: true,
                gas_wei: None,
                gas_kutipan: None,
                slippage_kutipan: None,
                dibuka_pada,
                tx_buka: basis.first_tx.map(|hash| format!("{hash:#x}")),
                rekonstruksi_log: Some(RekonstruksiLog {
                    collected0: basis.collected0.to_string(),
                    collected1: basis.collected1.to_string(),
                    decreased0: basis.decreased0.to_string(),
                    decreased1: basis.decreased1.to_string(),
                    sampai_blok: latest,
                }),
                rekonstruksi_v4: None,
            },
        );
    }
    Ok(())
}

#[derive(Debug, Clone)]
enum PerubahanV4 {
    Increase {
        liquidity: u128,
        amount0_max: U256,
        amount1_max: U256,
    },
    Decrease {
        liquidity: u128,
    },
}

#[derive(Debug, Clone)]
struct EventV4 {
    block: u64,
    tx_index: u64,
    log_index: u64,
    timestamp: Option<u64>,
    tx_hash: Option<B256>,
    perubahan: PerubahanV4,
}

#[derive(Debug, PartialEq, Eq)]
struct BasisV4 {
    setor0_max: U256,
    setor1_max: U256,
    liquidity: u128,
    first_block: u64,
    first_timestamp: Option<u64>,
    first_tx: Option<B256>,
}

#[derive(Clone, Copy)]
struct KonteksTambahV4 {
    token_id: U256,
    pool_id: B256,
    tick_lower: i32,
    tick_upper: i32,
    liquidity: u128,
}

fn batas_dari_calldata_v4(input: &[u8], target: KonteksTambahV4) -> Result<(U256, U256)> {
    use alloy::primitives::aliases::I24;

    let call = PmV4::modifyLiquiditiesCall::abi_decode(input)
        .context("calldata bukan modifyLiquidities langsung")?;
    let (actions, params) = <(Bytes, Vec<Bytes>)>::abi_decode_params(&call.unlockData)
        .context("unlockData V4 tidak dapat diurai")?;
    anyhow::ensure!(
        actions.len() == params.len(),
        "jumlah aksi dan parameter V4 berbeda"
    );
    let mut matches = Vec::new();
    for (action, params) in actions.iter().copied().zip(params.iter()) {
        if action == kode::INCREASE_LIQUIDITY {
            let (token_id, liquidity, amount0_max, amount1_max, _hook_data) =
                <(U256, U256, u128, u128, Bytes)>::abi_decode_params(params)
                    .context("parameter INCREASE_LIQUIDITY tidak dapat diurai")?;
            if token_id == target.token_id && liquidity == U256::from(target.liquidity) {
                matches.push((U256::from(amount0_max), U256::from(amount1_max)));
            }
        } else if action == kode::MINT_POSITION {
            let (key, lower, upper, liquidity, amount0_max, amount1_max, _owner, _hook_data) =
                <(PmV4::PoolKey, I24, I24, U256, u128, u128, Address, Bytes)>::abi_decode_params(
                    params,
                )
                .context("parameter MINT_POSITION tidak dapat diurai")?;
            if super::muat::pool_id_dari(&key) == target.pool_id
                && lower.as_i32() == target.tick_lower
                && upper.as_i32() == target.tick_upper
                && liquidity == U256::from(target.liquidity)
            {
                matches.push((U256::from(amount0_max), U256::from(amount1_max)));
            }
        }
    }
    anyhow::ensure!(
        matches.len() == 1,
        "delta tambah V4 cocok dengan {} aksi, harus tepat satu",
        matches.len()
    );
    Ok(matches[0])
}

fn rakit_v4(events: &mut [EventV4]) -> Result<BasisV4> {
    events.sort_by_key(|e| (e.block, e.tx_index, e.log_index));
    let first = events
        .iter()
        .find(|e| matches!(e.perubahan, PerubahanV4::Increase { .. }))
        .context("histori tidak mempunyai penambahan likuiditas")?;
    let mut out = BasisV4 {
        setor0_max: U256::ZERO,
        setor1_max: U256::ZERO,
        liquidity: 0,
        first_block: first.block,
        first_timestamp: first.timestamp,
        first_tx: first.tx_hash,
    };
    for event in events {
        match &event.perubahan {
            PerubahanV4::Increase {
                liquidity,
                amount0_max,
                amount1_max,
            } => {
                out.liquidity = out
                    .liquidity
                    .checked_add(*liquidity)
                    .context("likuiditas histori V4 melampaui uint128")?;
                out.setor0_max = jumlah(out.setor0_max, *amount0_max, "basis maksimum V4 token0")?;
                out.setor1_max = jumlah(out.setor1_max, *amount1_max, "basis maksimum V4 token1")?;
            }
            PerubahanV4::Decrease { liquidity } => {
                anyhow::ensure!(
                    *liquidity <= out.liquidity,
                    "delta decrease V4 melebihi likuiditas yang sudah tercatat"
                );
                let lama = out.liquidity;
                let sisa = lama - *liquidity;
                if sisa == 0 {
                    out.setor0_max = U256::ZERO;
                    out.setor1_max = U256::ZERO;
                } else {
                    out.setor0_max = crate::math::liquidity_math::mul_div(
                        out.setor0_max,
                        U256::from(sisa),
                        U256::from(lama),
                    )
                    .map_err(anyhow::Error::msg)?;
                    out.setor1_max = crate::math::liquidity_math::mul_div(
                        out.setor1_max,
                        U256::from(sisa),
                        U256::from(lama),
                    )
                    .map_err(anyhow::Error::msg)?;
                }
                out.liquidity = sisa;
            }
        }
    }
    Ok(out)
}

/// Rekonstruksi basis V4 dari ModifyLiquidity dan calldata PositionManager.
///
/// Event V4 hanya membawa delta likuiditas. Jumlah token diambil dari batas
/// maksimum pada aksi mint/increase dan karena itu disimpan sebagai perkiraan.
/// Transaksi yang dibungkus router lain tidak ditebak: posisi tetap tampil,
/// tetapi basisnya dibiarkan tidak diketahui.
pub async fn rekonstruksi_basis_v4<P: Provider>(
    provider: &P,
    registry: &Registry,
    store: &crate::store::Store,
    positions: &[Posisi],
) -> Result<()> {
    let pm = registry.contract("uniswap_v4", "position_manager")?;
    let pool_manager = registry.contract("uniswap_v4", "pool_manager")?;
    let mut eligible = Vec::new();
    for position in positions.iter().filter(|p| p.version == PoolVersion::V4) {
        let existing = crate::pnl::ambil(store, &position.position_id);
        if existing.is_some() {
            continue;
        }
        let token_id = position
            .token_id
            .parse::<U256>()
            .with_context(|| format!("token id V4 {} tidak valid", position.token_id))?;
        let key = PmV4::PoolKey {
            currency0: position
                .token0
                .parse()
                .context("currency0 V4 tidak valid")?,
            currency1: position
                .token1
                .parse()
                .context("currency1 V4 tidak valid")?,
            fee: alloy::primitives::aliases::U24::from(position.fee),
            tickSpacing: alloy::primitives::aliases::I24::try_from(position.tick_spacing)
                .context("tick spacing V4 tidak valid")?,
            hooks: position
                .hooks
                .as_deref()
                .unwrap_or(crate::pools::ZERO_ADDRESS)
                .parse()
                .context("hook V4 tidak valid")?,
        };
        eligible.push((position, token_id, super::muat::pool_id_dari(&key)));
    }
    if eligible.is_empty() {
        return Ok(());
    }

    let latest = provider
        .get_block_number()
        .await
        .context("blok terakhir untuk rekonstruksi V4 tidak terbaca")?;
    let pool_ids: Vec<B256> = eligible.iter().map(|(_, _, pool_id)| *pool_id).collect();
    let target_ids: std::collections::HashSet<U256> =
        eligible.iter().map(|(_, id, _)| *id).collect();
    let mut logs = Vec::new();
    let lantai = registry.chain.deploy_block;
    let mut sampai = latest;
    while lantai <= sampai {
        let hasil = crate::chain::logs::baca_mundur(
            provider,
            sampai,
            lantai,
            super::LOOKBACK_TRANSFER,
            100,
            |lo, hi| {
                Filter::new()
                    .address(pool_manager)
                    .event_signature(ModifyLiquidity::SIGNATURE_HASH)
                    .topic1(pool_ids.clone())
                    .topic2(pm.into_word())
                    .from_block(lo)
                    .to_block(hi)
            },
        )
        .await?;
        logs.extend(hasil.logs);
        if hasil.dari_blok <= lantai {
            break;
        }
        sampai = hasil.dari_blok - 1;
    }

    let mut input_cache = HashMap::<B256, Bytes>::new();
    let mut per_token = HashMap::<U256, Vec<EventV4>>::new();
    for log in logs {
        if log.removed {
            continue;
        }
        let Ok(decoded) = ModifyLiquidity::decode_log(&log.inner) else {
            continue;
        };
        let token_id = U256::from_be_bytes(decoded.salt.0);
        if !target_ids.contains(&token_id) {
            continue;
        }
        let (sign, magnitude) = decoded.liquidityDelta.into_sign_and_abs();
        let liquidity =
            u128::try_from(magnitude).context("delta likuiditas V4 melampaui uint128")?;
        if liquidity == 0 {
            continue;
        }
        let (block, tx_index, log_index) = urutan(&log);
        let perubahan = if sign == Sign::Negative {
            PerubahanV4::Decrease { liquidity }
        } else {
            let hash = log
                .transaction_hash
                .context("log tambah V4 tidak mempunyai transaction hash")?;
            let input = if let Some(input) = input_cache.get(&hash) {
                input.clone()
            } else {
                let tx = provider
                    .get_transaction_by_hash(hash)
                    .await
                    .context("transaksi tambah V4 tidak terbaca")?
                    .context("transaksi tambah V4 tidak ditemukan")?;
                let input = tx.input().clone();
                input_cache.insert(hash, input.clone());
                input
            };
            let (amount0_max, amount1_max) = batas_dari_calldata_v4(
                &input,
                KonteksTambahV4 {
                    token_id,
                    pool_id: decoded.id,
                    tick_lower: decoded.tickLower.as_i32(),
                    tick_upper: decoded.tickUpper.as_i32(),
                    liquidity,
                },
            )?;
            PerubahanV4::Increase {
                liquidity,
                amount0_max,
                amount1_max,
            }
        };
        per_token.entry(token_id).or_default().push(EventV4 {
            block,
            tx_index,
            log_index,
            timestamp: log.block_timestamp,
            tx_hash: log.transaction_hash,
            perubahan,
        });
    }

    for (position, token_id, _) in eligible {
        let events = per_token
            .get_mut(&token_id)
            .with_context(|| format!("histori V4 #{} kosong", position.token_id))?;
        let basis = rakit_v4(events)
            .with_context(|| format!("histori V4 #{} tidak konsisten", position.token_id))?;
        let current_liquidity = position
            .liquidity
            .parse::<u128>()
            .with_context(|| format!("likuiditas V4 #{} tidak valid", position.token_id))?;
        anyhow::ensure!(
            basis.liquidity == current_liquidity,
            "likuiditas hasil histori V4 #{} adalah {}, state sekarang {}",
            position.token_id,
            basis.liquidity,
            current_liquidity
        );
        let dibuka_pada = basis
            .first_timestamp
            .and_then(|timestamp| chrono::DateTime::from_timestamp(timestamp as i64, 0))
            .map(|timestamp| timestamp.to_rfc3339())
            .unwrap_or_else(|| format!("blok {}", basis.first_block));
        crate::pnl::simpan(
            store,
            &Catatan {
                position_id: position.position_id.clone(),
                token_id: position.token_id.clone(),
                flow_id: None,
                setor0: basis.setor0_max.to_string(),
                setor1: basis.setor1_max.to_string(),
                harga_buka: None,
                lengkap: false,
                setoran_aktual: false,
                gas_wei: None,
                gas_kutipan: None,
                slippage_kutipan: None,
                dibuka_pada,
                tx_buka: basis.first_tx.map(|hash| format!("{hash:#x}")),
                rekonstruksi_log: None,
                rekonstruksi_v4: Some(RekonstruksiV4 {
                    sampai_blok: latest,
                }),
            },
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::aliases::{I24, U24};

    fn event(block: u64, perubahan: PerubahanV3) -> EventV3 {
        EventV3 {
            token_id: U256::from(7),
            block,
            tx_index: 0,
            log_index: 0,
            timestamp: Some(1_700_000_000 + block),
            tx_hash: Some(B256::repeat_byte(block as u8)),
            perubahan,
        }
    }

    fn key_v4() -> PmV4::PoolKey {
        PmV4::PoolKey {
            currency0: Address::repeat_byte(1),
            currency1: Address::repeat_byte(2),
            fee: U24::from(3_000),
            tickSpacing: I24::try_from(60).unwrap(),
            hooks: Address::ZERO,
        }
    }

    #[test]
    fn batas_mint_v4_dibaca_dari_calldata_bertipe() {
        let key = key_v4();
        let mint = crate::exec::v4::actions::MintPosition {
            pool_key: key.clone(),
            tick_lower: -120,
            tick_upper: 120,
            liquidity: 900,
            amount0_max: 1_234,
            amount1_max: 5_678,
            owner: Address::repeat_byte(3),
            hook_data: Bytes::new(),
        };
        let unlock = crate::exec::v4::actions::unlock_data(&[
            mint.encode(),
            crate::exec::v4::actions::settle_pair(key.currency0, key.currency1),
        ])
        .unwrap();
        let calldata = PmV4::modifyLiquiditiesCall {
            unlockData: unlock,
            deadline: U256::from(1),
        }
        .abi_encode();
        let amounts = batas_dari_calldata_v4(
            &calldata,
            KonteksTambahV4 {
                token_id: U256::from(77),
                pool_id: super::super::muat::pool_id_dari(&key),
                tick_lower: -120,
                tick_upper: 120,
                liquidity: 900,
            },
        )
        .unwrap();
        assert_eq!(amounts, (U256::from(1_234), U256::from(5_678)));
    }

    #[test]
    fn batas_increase_v4_terikat_ke_token_id_dan_likuiditas() {
        let key = key_v4();
        let unlock = crate::exec::v4::actions::unlock_data(&[
            crate::exec::v4::actions::increase_liquidity(
                U256::from(77),
                300,
                444,
                555,
                Bytes::new(),
            ),
            crate::exec::v4::actions::settle_pair(key.currency0, key.currency1),
        ])
        .unwrap();
        let calldata = PmV4::modifyLiquiditiesCall {
            unlockData: unlock,
            deadline: U256::from(1),
        }
        .abi_encode();
        let target = KonteksTambahV4 {
            token_id: U256::from(77),
            pool_id: super::super::muat::pool_id_dari(&key),
            tick_lower: -120,
            tick_upper: 120,
            liquidity: 300,
        };
        assert_eq!(
            batas_dari_calldata_v4(&calldata, target).unwrap(),
            (U256::from(444), U256::from(555))
        );
        let salah = KonteksTambahV4 {
            token_id: U256::from(78),
            ..target
        };
        assert!(batas_dari_calldata_v4(&calldata, salah).is_err());
    }

    #[test]
    fn basis_maksimum_v4_berkurang_sebanding_dengan_likuiditas() {
        let make = |block, perubahan| EventV4 {
            block,
            tx_index: 0,
            log_index: 0,
            timestamp: None,
            tx_hash: Some(B256::repeat_byte(block as u8)),
            perubahan,
        };
        let mut events = vec![
            make(
                1,
                PerubahanV4::Increase {
                    liquidity: 1_000,
                    amount0_max: U256::from(10_000),
                    amount1_max: U256::from(20_000),
                },
            ),
            make(2, PerubahanV4::Decrease { liquidity: 250 }),
        ];
        let basis = rakit_v4(&mut events).unwrap();
        assert_eq!(basis.liquidity, 750);
        assert_eq!(basis.setor0_max, U256::from(7_500));
        assert_eq!(basis.setor1_max, U256::from(15_000));
    }

    #[test]
    fn basis_v3_mengikuti_lot_likuiditas_dan_bukan_komposisi_withdraw() {
        let mut events = vec![
            event(
                3,
                PerubahanV3::Increase {
                    liquidity: 40,
                    amount0: U256::from(400),
                    amount1: U256::from(800),
                },
            ),
            event(
                1,
                PerubahanV3::Increase {
                    liquidity: 100,
                    amount0: U256::from(1_000),
                    amount1: U256::from(2_000),
                },
            ),
            event(
                2,
                PerubahanV3::Decrease {
                    liquidity: 40,
                    amount0: U256::from(500),
                    amount1: U256::from(600),
                },
            ),
        ];
        let basis = rakit(&mut events).unwrap();
        assert_eq!(basis.liquidity, 100);
        assert_eq!(basis.setor0, U256::from(1_000));
        assert_eq!(basis.setor1, U256::from(2_000));
        assert_eq!(basis.decreased0, U256::from(500));
        assert_eq!(basis.decreased1, U256::from(600));
        assert_eq!(basis.first_block, 1);
    }

    #[test]
    fn collect_dan_decrease_disimpan_terpisah_untuk_fee_seumur_posisi() {
        let mut events = vec![
            event(
                1,
                PerubahanV3::Increase {
                    liquidity: 100,
                    amount0: U256::from(1_000),
                    amount1: U256::from(2_000),
                },
            ),
            event(
                2,
                PerubahanV3::Collect {
                    amount0: U256::from(100),
                    amount1: U256::from(50),
                },
            ),
            event(
                3,
                PerubahanV3::Decrease {
                    liquidity: 40,
                    amount0: U256::from(500),
                    amount1: U256::from(600),
                },
            ),
            event(
                4,
                PerubahanV3::Collect {
                    amount0: U256::from(520),
                    amount1: U256::from(630),
                },
            ),
        ];
        let basis = rakit(&mut events).unwrap();
        assert_eq!(basis.collected0, U256::from(620));
        assert_eq!(basis.collected1, U256::from(680));
        let log = RekonstruksiLog {
            collected0: basis.collected0.to_string(),
            collected1: basis.collected1.to_string(),
            decreased0: basis.decreased0.to_string(),
            decreased1: basis.decreased1.to_string(),
            sampai_blok: 4,
        };
        let fee = crate::pnl::total_fee_dari_log(U256::from(30), U256::from(20), &log).unwrap();
        assert_eq!(fee, (U256::from(150), U256::from(100)));
    }

    #[test]
    fn histori_yang_kehilangan_increase_ditolak() {
        let mut events = vec![event(
            1,
            PerubahanV3::Collect {
                amount0: U256::from(1),
                amount1: U256::ZERO,
            },
        )];
        assert!(rakit(&mut events).is_err());
    }
}
