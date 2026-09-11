//! Jurnal ditulis sebelum broadcast. Receipt dapat direkonsiliasi tanpa signer.
//! Hash menjadi kunci idempoten; pembacaan ulang tidak menghitung transaksi dua kali.

use crate::store::Store;
use alloy::{
    primitives::{Address, TxHash},
    providers::Provider,
    rpc::types::{TransactionReceipt, TransactionRequest},
};
use anyhow::{ensure, Context as _, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Approve,
    Zap,
    Open,
    Claim,
    Close,
    Increase,
    Rebalance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Context {
    pub operation: Operation,
    pub plan_id: String,
    pub flow_id: String,
    pub pool_id: Option<String>,
    pub position_id: Option<String>,
    pub token0: Option<Address>,
    pub token1: Option<Address>,
    /// Nilai pada rencana yang disetujui, bukan klaim kurs aktual setelah eksekusi.
    pub deployment_usd: Option<f64>,
    /// Dasar penilaian yang dibekukan saat user menyetujui penutupan.
    ///
    /// Receipt memberi jumlah token aktual, tetapi tidak menyimpan harga pool
    /// atau token mana yang menjadi kutipan. Dasar ini membuat histori posisi
    /// tertutup dapat direkonstruksi setelah restart tanpa menebak satuan.
    #[serde(default)]
    pub close_basis: Option<CloseBasis>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseBasis {
    pub sqrt_price_x96: String,
    pub fee0: Option<String>,
    pub fee1: Option<String>,
    pub quote_token0: bool,
    pub quote_decimals: u8,
    pub currency_id: String,
    pub currency_symbol: String,
    #[serde(default)]
    pub symbol0: Option<String>,
    #[serde(default)]
    pub symbol1: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Prepared,
    Submitted,
    Unknown,
    Succeeded,
    Reverted,
    Replaced,
}
impl Status {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Reverted | Self::Replaced)
    }
}

/// Kedalaman yang harus dicapai sebelum receipt tidak perlu diperiksa lagi.
///
/// Receipt yang baru masuk blok belum final. Selama kedalaman ini belum
/// tercapai, rekonsiliasi tetap membaca ulang receipt agar reorg tidak berubah
/// menjadi histori sukses palsu.
pub const FINALITY_CONFIRMATIONS: u64 = 20;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub chain_id: u64,
    pub owner: Address,
    pub tx_hash: TxHash,
    pub context: Context,
    pub request: TransactionRequest,
    pub status: Status,
    pub created_at: String,
    pub updated_at: String,
    pub receipt: Option<TransactionReceipt>,
    pub broadcast_error: Option<String>,
    /// Waktu receipt mencapai kedalaman konfirmasi yang disyaratkan.
    #[serde(default)]
    pub confirmed_at: Option<String>,
    /// Token yang benar benar diterima wallet dari claim/close.
    #[serde(default)]
    pub received: Option<ReceivedAmounts>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceivedAmounts {
    pub amount0: String,
    pub amount1: String,
    /// `v3_collect_event` atau `wallet_balance_delta`.
    pub evidence: String,
}

#[derive(Debug, Default)]
pub struct ReconcileReport {
    pub changed: Vec<Entry>,
    pub errors: Vec<String>,
}

#[derive(Debug, Default, PartialEq)]
pub struct LimitUsage {
    /// Nilai rencana open hari ini yang sukses atau masih belum pasti.
    pub daily_committed_usd: f64,
    /// Pool yang pernah berhasil dibuka melalui engine ini.
    pub used_pools: HashSet<String>,
}

/// Bentuk jurnal yang aman dikirim ke panel. Calldata dan request lengkap
/// tetap lokal karena panel hanya membutuhkan bukti dan statusnya.
#[derive(Debug, Clone, Serialize)]
pub struct View {
    pub tx_hash: String,
    pub operation: Operation,
    pub plan_id: String,
    pub flow_id: String,
    pub pool_id: Option<String>,
    pub position_id: Option<String>,
    pub status: Status,
    pub nonce: u64,
    pub gas_limit: u64,
    pub block_number: Option<u64>,
    pub gas_used: Option<u64>,
    pub gas_cost_wei: Option<String>,
    pub received: Option<ReceivedAmounts>,
    pub confirmed: bool,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct FlowCosts {
    pub gas_wei: alloy::primitives::U256,
    /// Salah jika ada transaksi flow yang belum final atau receipt gas tidak lengkap.
    pub complete: bool,
    pub transaction_count: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub struct PositionLedger {
    pub claimed0: alloy::primitives::U256,
    pub claimed1: alloy::primitives::U256,
    pub action_gas_wei: alloy::primitives::U256,
    pub complete: bool,
    pub closed_at: Option<String>,
    pub close_received: Option<ReceivedAmounts>,
}

impl Default for PositionLedger {
    fn default() -> Self {
        Self {
            claimed0: alloy::primitives::U256::ZERO,
            claimed1: alloy::primitives::U256::ZERO,
            action_gas_wei: alloy::primitives::U256::ZERO,
            complete: true,
            closed_at: None,
            close_received: None,
        }
    }
}

pub struct Journal<'a> {
    pub store: &'a Store,
    pub context: Context,
}
fn prefix(chain: u64, owner: Address) -> String {
    format!("tx:v1:{chain}:{owner:#x}:")
}
fn key(chain: u64, owner: Address, hash: TxHash) -> String {
    format!("{}{hash:#x}", prefix(chain, owner))
}

impl Journal<'_> {
    pub fn prepare(
        &self,
        chain: u64,
        owner: Address,
        hash: TxHash,
        request: TransactionRequest,
    ) -> Result<Entry> {
        ensure!(
            request.from == Some(owner) && request.chain_id == Some(chain),
            "identitas jurnal berbeda dari transaksi"
        );
        ensure!(
            request.nonce.is_some() && request.gas.is_some(),
            "transaksi jurnal belum lengkap"
        );
        let now = chrono::Utc::now().to_rfc3339();
        self.store
            .update(&key(chain, owner, hash), |previous: Option<Entry>| {
                ensure!(
                    previous.is_none(),
                    "hash transaksi sudah tercatat; rekonsiliasi sebelum mencoba lagi"
                );
                Ok(Entry {
                    chain_id: chain,
                    owner,
                    tx_hash: hash,
                    context: self.context.clone(),
                    request,
                    status: Status::Prepared,
                    created_at: now.clone(),
                    updated_at: now,
                    receipt: None,
                    broadcast_error: None,
                    confirmed_at: None,
                    received: None,
                })
            })
    }
}

pub fn entries(store: &Store, chain: u64, owner: Address) -> Result<Vec<Entry>> {
    Ok(store
        .prefix::<Entry>(&prefix(chain, owner))?
        .into_iter()
        .map(|(_, value)| value)
        .collect())
}

pub fn views(store: &Store, chain: u64, owner: Address) -> Result<Vec<View>> {
    let mut items = entries(store, chain, owner)?;
    items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    items
        .into_iter()
        .map(|entry| {
            let receipt = entry.receipt.as_ref();
            let gas_cost_wei = receipt.and_then(|r| {
                (r.gas_used > 0 && r.effective_gas_price > 0).then(|| {
                    (alloy::primitives::U256::from(r.gas_used)
                        * alloy::primitives::U256::from(r.effective_gas_price))
                    .to_string()
                })
            });
            Ok(View {
                tx_hash: format!("{:#x}", entry.tx_hash),
                operation: entry.context.operation,
                plan_id: entry.context.plan_id,
                flow_id: entry.context.flow_id,
                pool_id: entry.context.pool_id,
                position_id: entry.context.position_id,
                status: entry.status,
                nonce: entry.request.nonce.context("nonce jurnal hilang")?,
                gas_limit: entry.request.gas.context("gas limit jurnal hilang")?,
                block_number: receipt.and_then(|r| r.block_number),
                gas_used: receipt.map(|r| r.gas_used),
                gas_cost_wei,
                received: entry.received,
                confirmed: entry.confirmed_at.is_some(),
                // Detail transport tetap di jurnal lokal karena dapat memuat
                // URL RPC berkunci. Panel hanya perlu tahu sifat masalahnya.
                error: entry
                    .broadcast_error
                    .as_ref()
                    .map(|_| "hasil broadcast atau receipt belum dapat dipastikan".into()),
                created_at: entry.created_at,
                updated_at: entry.updated_at,
            })
        })
        .collect()
}

pub fn link_position(
    store: &Store,
    chain: u64,
    owner: Address,
    tx_hash: TxHash,
    position_id: &str,
) -> Result<Entry> {
    ensure!(!position_id.trim().is_empty(), "position_id kosong");
    store.update(&key(chain, owner, tx_hash), |old: Option<Entry>| {
        let mut old = old.context("transaksi open tidak ada dalam jurnal")?;
        ensure!(
            old.context.operation == Operation::Open,
            "transaksi bukan open"
        );
        ensure!(
            old.context
                .position_id
                .as_deref()
                .is_none_or(|id| id == position_id),
            "transaksi sudah terikat ke posisi lain"
        );
        old.context.position_id = Some(position_id.to_owned());
        old.updated_at = chrono::Utc::now().to_rfc3339();
        Ok(old)
    })
}

pub fn record_received(
    store: &Store,
    chain: u64,
    owner: Address,
    tx_hash: TxHash,
    received: ReceivedAmounts,
) -> Result<Entry> {
    store.update(&key(chain, owner, tx_hash), |old: Option<Entry>| {
        let mut old = old.context("transaksi aksi tidak ada dalam jurnal")?;
        ensure!(
            matches!(old.context.operation, Operation::Claim | Operation::Close),
            "transaksi bukan claim atau close"
        );
        ensure!(
            old.status == Status::Succeeded,
            "transaksi aksi belum sukses"
        );
        ensure!(
            old.received.as_ref().is_none_or(|value| value == &received),
            "hasil aksi sudah tercatat dengan jumlah berbeda"
        );
        old.received = Some(received);
        old.updated_at = chrono::Utc::now().to_rfc3339();
        Ok(old)
    })
}

pub fn flow_costs(store: &Store, chain: u64, owner: Address, flow_id: &str) -> Result<FlowCosts> {
    let related: Vec<_> = entries(store, chain, owner)?
        .into_iter()
        .filter(|e| e.context.flow_id == flow_id)
        .collect();
    if related.is_empty() {
        return Ok(FlowCosts::default());
    }
    let mut out = FlowCosts {
        complete: true,
        ..Default::default()
    };
    for entry in related {
        out.transaction_count += 1;
        let Some(receipt) = entry.receipt else {
            out.complete = false;
            continue;
        };
        if receipt.gas_used == 0 || receipt.effective_gas_price == 0 {
            out.complete = false;
            continue;
        }
        let biaya = alloy::primitives::U256::from(receipt.gas_used)
            * alloy::primitives::U256::from(receipt.effective_gas_price);
        out.gas_wei = out
            .gas_wei
            .checked_add(biaya)
            .context("total gas flow melampaui uint256")?;
        if entry.confirmed_at.is_none() || entry.status == Status::Replaced {
            out.complete = false;
        }
    }
    Ok(out)
}

pub fn position_ledger(
    store: &Store,
    chain: u64,
    owner: Address,
    position_id: &str,
) -> Result<PositionLedger> {
    let mut out = PositionLedger::default();
    for entry in entries(store, chain, owner)?.into_iter().filter(|entry| {
        entry.context.position_id.as_deref() == Some(position_id)
            && matches!(
                entry.context.operation,
                Operation::Approve
                    | Operation::Claim
                    | Operation::Close
                    | Operation::Increase
                    | Operation::Rebalance
            )
    }) {
        let Some(receipt) = entry.receipt.as_ref() else {
            out.complete = false;
            continue;
        };
        if receipt.gas_used == 0 || receipt.effective_gas_price == 0 {
            out.complete = false;
        } else {
            let gas = alloy::primitives::U256::from(receipt.gas_used)
                * alloy::primitives::U256::from(receipt.effective_gas_price);
            out.action_gas_wei = out
                .action_gas_wei
                .checked_add(gas)
                .context("total gas aksi posisi melampaui uint256")?;
        }
        if entry.confirmed_at.is_none() || entry.status == Status::Replaced {
            out.complete = false;
        }
        if entry.status != Status::Succeeded {
            continue;
        }
        if matches!(
            entry.context.operation,
            Operation::Approve | Operation::Increase | Operation::Rebalance
        ) {
            continue;
        }
        let Some(received) = entry.received.as_ref() else {
            out.complete = false;
            continue;
        };
        let amount0 = received
            .amount0
            .parse::<alloy::primitives::U256>()
            .context("amount0 hasil aksi tidak valid")?;
        let amount1 = received
            .amount1
            .parse::<alloy::primitives::U256>()
            .context("amount1 hasil aksi tidak valid")?;
        match entry.context.operation {
            Operation::Claim => {
                out.claimed0 = out
                    .claimed0
                    .checked_add(amount0)
                    .context("total claim token0 melampaui uint256")?;
                out.claimed1 = out
                    .claimed1
                    .checked_add(amount1)
                    .context("total claim token1 melampaui uint256")?;
            }
            Operation::Close => {
                out.closed_at = Some(entry.updated_at.clone());
                out.close_received = Some(received.clone());
            }
            Operation::Approve | Operation::Increase | Operation::Rebalance => unreachable!(),
            Operation::Zap | Operation::Open => unreachable!(),
        }
    }
    Ok(out)
}

pub fn nonce_floor(store: &Store, chain: u64, owner: Address) -> Result<u64> {
    entries(store, chain, owner)?
        .into_iter()
        .filter(|e| e.receipt.is_none() && !e.status.terminal())
        .try_fold(0, |floor, entry| {
            let next = entry
                .request
                .nonce
                .context("nonce jurnal hilang")?
                .checked_add(1)
                .context("nonce jurnal melampaui batas")?;
            Ok(floor.max(next))
        })
}

/// Bentuk keadaan G6 dari jurnal yang persisten.
///
/// Open prepared/submitted/unknown ikut komitmen harian agar restart atau
/// koneksi putus tidak membuka celah melampaui limit. Revert tidak dihitung.
/// Pool dianggap pernah dipakai hanya setelah receipt sukses terlihat.
pub fn limit_usage(
    store: &Store,
    chain: u64,
    owner: Address,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<LimitUsage> {
    let mut usage = LimitUsage::default();
    for entry in entries(store, chain, owner)? {
        if !matches!(
            entry.context.operation,
            Operation::Open | Operation::Increase
        ) {
            continue;
        }
        if entry.status == Status::Succeeded {
            if let Some(pool) = entry.context.pool_id.as_ref() {
                usage.used_pools.insert(pool.clone());
            }
        }
        if matches!(entry.status, Status::Reverted | Status::Replaced) {
            continue;
        }
        let created = chrono::DateTime::parse_from_rfc3339(&entry.created_at)
            .with_context(|| format!("waktu jurnal {} tidak valid", entry.tx_hash))?
            .with_timezone(&chrono::Utc);
        if created.date_naive() == now.date_naive() {
            let usd = entry
                .context
                .deployment_usd
                .context("catatan open tidak mempunyai deployment_usd")?;
            ensure!(
                usd.is_finite() && usd >= 0.0,
                "deployment_usd jurnal tidak valid"
            );
            usage.daily_committed_usd += usd;
            ensure!(
                usage.daily_committed_usd.is_finite(),
                "total deployment harian melampaui f64"
            );
        }
    }
    Ok(usage)
}

#[cfg(test)]
pub(crate) fn test_context() -> Context {
    Context {
        operation: Operation::Open,
        plan_id: "plan".into(),
        flow_id: "flow".into(),
        pool_id: Some("pool".into()),
        position_id: None,
        token0: None,
        token1: None,
        deployment_usd: Some(100.0),
        close_basis: None,
    }
}

// Helper rekonsiliasi berada setelah fixture uji karena ia ditambahkan tanpa
// memindahkan blok uji besar yang sudah stabil. Urutan item tidak memengaruhi
// resolusi Rust.
#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod tests {
    use super::*;
    use alloy::{
        network::TransactionBuilder,
        primitives::{B256, U256},
        providers::ProviderBuilder,
        transports::mock::Asserter,
    };

    fn setup(name: &str) -> (std::path::PathBuf, Store, Entry) {
        let dir =
            std::env::temp_dir().join(format!("rangefinder-journal-{name}-{}", std::process::id()));
        let store = Store::open(&dir).unwrap();
        let owner = Address::repeat_byte(1);
        let journal = Journal {
            store: &store,
            context: test_context(),
        };
        let request = TransactionRequest::default()
            .with_from(owner)
            .with_to(Address::repeat_byte(2))
            .with_chain_id(4663)
            .with_nonce(7)
            .with_gas_limit(100_000)
            .with_value(U256::ZERO);
        let entry = journal
            .prepare(4663, owner, B256::repeat_byte(3), request)
            .unwrap();
        (dir, store, entry)
    }
    fn receipt(entry: &Entry, success: bool) -> TransactionReceipt {
        serde_json::from_value(serde_json::json!({ "transactionHash": entry.tx_hash,
            "from": entry.owner, "to": Address::repeat_byte(2), "gasUsed": "0x5208",
            "cumulativeGasUsed": "0x5208", "effectiveGasPrice": "0x3b9aca00", "status": if success { "0x1" } else { "0x0" },
            "type": "0x2", "logs": [], "logsBloom": format!("0x{}", "00".repeat(256)),
            "blockNumber": "0x10", "blockHash": B256::repeat_byte(4) })).unwrap()
    }

    #[tokio::test]
    async fn restart_memulihkan_receipt_tanpa_signer_dan_tidak_menggandakan_catatan() {
        let (dir, store, entry) = setup("restart");
        broadcast_result(&store, &entry, Some("koneksi putus".into())).unwrap();
        drop(store);
        let store = Store::open(&dir).unwrap();
        assert_eq!(nonce_floor(&store, 4663, entry.owner).unwrap(), 8);
        let rpc = Asserter::new();
        rpc.push_success(&4663u64);
        rpc.push_success(&100u64);
        rpc.push_success(&receipt(&entry, true));
        let provider = ProviderBuilder::new().connect_mocked_client(rpc.clone());
        let changed = reconcile(&provider, &store, 4663, entry.owner)
            .await
            .unwrap();
        assert_eq!(changed.changed.len(), 1);
        assert!(changed.errors.is_empty());
        assert_eq!(changed.changed[0].status, Status::Succeeded);
        assert!(changed.changed[0].confirmed_at.is_some());
        rpc.push_success(&4663u64);
        rpc.push_success(&100u64);
        assert!(reconcile(&provider, &store, 4663, entry.owner)
            .await
            .unwrap()
            .changed
            .is_empty());
        assert_eq!(entries(&store, 4663, entry.owner).unwrap().len(), 1);
        assert_eq!(nonce_floor(&store, 4663, entry.owner).unwrap(), 0);
        let persisted = entries(&store, 4663, entry.owner).unwrap().pop().unwrap();
        assert_eq!(persisted.receipt.unwrap().gas_used, 21_000);
    }

    #[test]
    fn receipt_asing_ditolak_revert_tersimpan_dan_namespace_terisolasi() {
        let (_, store, entry) = setup("isolation");
        let mut wrong = receipt(&entry, true);
        wrong.transaction_hash = B256::repeat_byte(9);
        assert!(record_receipt(&store, &entry, wrong).is_err());
        assert_eq!(
            entries(&store, 4663, entry.owner).unwrap()[0].status,
            Status::Prepared
        );
        let reverted = record_receipt(&store, &entry, receipt(&entry, false)).unwrap();
        assert_eq!(reverted.status, Status::Reverted);
        assert_eq!(
            broadcast_result(&store, &entry, None).unwrap().status,
            Status::Reverted
        );
        assert!(entries(&store, 1, entry.owner).unwrap().is_empty());
        assert!(entries(&store, 4663, Address::repeat_byte(9))
            .unwrap()
            .is_empty());
        assert!(Journal {
            store: &store,
            context: test_context()
        }
        .prepare(4663, entry.owner, entry.tx_hash, entry.request)
        .is_err());
    }

    #[tokio::test]
    async fn receipt_belum_ada_tetap_menahan_nonce() {
        let (_, store, entry) = setup("pending");
        let rpc = Asserter::new();
        rpc.push_success(&4663u64);
        rpc.push_success(&100u64);
        rpc.push_success(&Option::<TransactionReceipt>::None);
        rpc.push_success(&7u64);
        let provider = ProviderBuilder::new().connect_mocked_client(rpc);
        let report = reconcile(&provider, &store, 4663, entry.owner)
            .await
            .unwrap();
        assert!(report.changed.is_empty());
        assert!(report.errors.is_empty());
        assert_eq!(nonce_floor(&store, 4663, entry.owner).unwrap(), 8);
    }

    #[tokio::test]
    async fn receipt_diperiksa_sampai_final_dan_reorg_memulihkan_status_unknown() {
        let (_, store, entry) = setup("reorg");
        let included = record_receipt(&store, &entry, receipt(&entry, true)).unwrap();
        assert!(included.confirmed_at.is_none());
        assert_eq!(nonce_floor(&store, 4663, entry.owner).unwrap(), 0);

        let rpc = Asserter::new();
        rpc.push_success(&4663u64);
        rpc.push_success(&17u64);
        rpc.push_success(&Option::<TransactionReceipt>::None);
        rpc.push_success(&7u64);
        let provider = ProviderBuilder::new().connect_mocked_client(rpc);
        let report = reconcile(&provider, &store, 4663, entry.owner)
            .await
            .unwrap();
        assert_eq!(report.changed.len(), 1);
        assert_eq!(report.changed[0].status, Status::Unknown);
        assert!(report.changed[0].receipt.is_none());
        assert_eq!(nonce_floor(&store, 4663, entry.owner).unwrap(), 8);
    }

    #[tokio::test]
    async fn nonce_yang_sudah_mined_oleh_hash_lain_ditandai_replaced() {
        let (_, store, entry) = setup("replaced");
        let rpc = Asserter::new();
        rpc.push_success(&4663u64);
        rpc.push_success(&100u64);
        rpc.push_success(&Option::<TransactionReceipt>::None);
        rpc.push_success(&8u64);
        let provider = ProviderBuilder::new().connect_mocked_client(rpc);
        let report = reconcile(&provider, &store, 4663, entry.owner)
            .await
            .unwrap();
        assert_eq!(report.changed[0].status, Status::Replaced);
        assert_eq!(nonce_floor(&store, 4663, entry.owner).unwrap(), 0);
    }

    #[test]
    fn penggunaan_limit_menghitung_open_pending_dan_sukses_tapi_bukan_revert() {
        let (_, store, pending) = setup("limits");
        let owner = pending.owner;
        let mut konteks = test_context();
        konteks.pool_id = Some("pool-sukses".into());
        konteks.deployment_usd = Some(25.0);
        let request = pending.request.clone().with_nonce(8);
        let sukses = Journal {
            store: &store,
            context: konteks,
        }
        .prepare(4663, owner, B256::repeat_byte(4), request)
        .unwrap();
        record_receipt(&store, &sukses, receipt(&sukses, true)).unwrap();

        let mut konteks = test_context();
        konteks.pool_id = Some("pool-revert".into());
        konteks.deployment_usd = Some(70.0);
        let revert = Journal {
            store: &store,
            context: konteks,
        }
        .prepare(
            4663,
            owner,
            B256::repeat_byte(5),
            pending.request.clone().with_nonce(9),
        )
        .unwrap();
        record_receipt(&store, &revert, receipt(&revert, false)).unwrap();

        let usage = limit_usage(&store, 4663, owner, chrono::Utc::now()).unwrap();
        assert_eq!(usage.daily_committed_usd, 125.0);
        assert_eq!(usage.used_pools, HashSet::from(["pool-sukses".into()]));
    }

    #[test]
    fn tambah_likuiditas_masuk_batas_harian_tapi_approvalnya_tidak() {
        let (_, store, template) = setup("limit-increase");
        let owner = template.owner;
        for (index, operation) in [Operation::Approve, Operation::Increase]
            .into_iter()
            .enumerate()
        {
            let mut context = test_context();
            context.operation = operation;
            context.deployment_usd = Some(40.0);
            let entry = Journal {
                store: &store,
                context,
            }
            .prepare(
                4663,
                owner,
                B256::repeat_byte(20 + index as u8),
                template.request.clone().with_nonce(20 + index as u64),
            )
            .unwrap();
            record_receipt(&store, &entry, receipt(&entry, true)).unwrap();
        }
        let usage = limit_usage(&store, 4663, owner, chrono::Utc::now()).unwrap();
        assert_eq!(usage.daily_committed_usd, 140.0);
    }

    #[test]
    fn posisi_dan_gas_flow_terikat_tanpa_menggandakan_biaya() {
        let (_, store, pending) = setup("flow-costs");
        let owner = pending.owner;
        let linked = link_position(&store, 4663, owner, pending.tx_hash, "4663:manager:7").unwrap();
        assert_eq!(
            linked.context.position_id.as_deref(),
            Some("4663:manager:7")
        );
        record_receipt(&store, &linked, receipt(&linked, true)).unwrap();
        let costs = flow_costs(&store, 4663, owner, "flow").unwrap();
        assert_eq!(costs.transaction_count, 1);
        assert_eq!(costs.gas_wei, U256::from(21_000_000_000_000u64));
        assert!(!costs.complete, "receipt belum mencapai finalitas");
        assert!(link_position(&store, 4663, owner, pending.tx_hash, "lain").is_err());
    }

    #[test]
    fn ledger_posisi_menjumlahkan_claim_dan_gas_setelah_final() {
        let (_, store, template) = setup("position-ledger");
        let owner = template.owner;
        let mut context = test_context();
        context.operation = Operation::Claim;
        context.position_id = Some("4663:manager:7".into());
        context.deployment_usd = None;
        let action = Journal {
            store: &store,
            context,
        }
        .prepare(
            4663,
            owner,
            B256::repeat_byte(6),
            template.request.clone().with_nonce(8),
        )
        .unwrap();
        let action = record_receipt(&store, &action, receipt(&action, true)).unwrap();
        record_received(
            &store,
            4663,
            owner,
            action.tx_hash,
            ReceivedAmounts {
                amount0: "13".into(),
                amount1: "17".into(),
                evidence: "v3_collect_event".into(),
            },
        )
        .unwrap();
        store
            .update(&key(4663, owner, action.tx_hash), |old: Option<Entry>| {
                let mut old = old.unwrap();
                old.confirmed_at = Some(chrono::Utc::now().to_rfc3339());
                Ok(old)
            })
            .unwrap();
        let ledger = position_ledger(&store, 4663, owner, "4663:manager:7").unwrap();
        assert_eq!(ledger.claimed0, U256::from(13));
        assert_eq!(ledger.claimed1, U256::from(17));
        assert_eq!(ledger.action_gas_wei, U256::from(21_000_000_000_000u64));
        assert!(ledger.complete);
    }
}

pub fn broadcast_result(store: &Store, entry: &Entry, error: Option<String>) -> Result<Entry> {
    store.update(
        &key(entry.chain_id, entry.owner, entry.tx_hash),
        |old: Option<Entry>| {
            let mut old = old.context("transaksi tidak ada dalam jurnal")?;
            if old.status.terminal() {
                return Ok(old);
            }
            old.status = if error.is_some() {
                Status::Unknown
            } else {
                Status::Submitted
            };
            old.broadcast_error = error;
            old.updated_at = chrono::Utc::now().to_rfc3339();
            Ok(old)
        },
    )
}

pub fn record_receipt(store: &Store, entry: &Entry, receipt: TransactionReceipt) -> Result<Entry> {
    ensure!(
        receipt.transaction_hash == entry.tx_hash && receipt.from == entry.owner,
        "receipt bukan milik transaksi jurnal"
    );
    ensure!(
        receipt.block_number.is_some() && receipt.block_hash.is_some(),
        "receipt belum mempunyai identitas blok"
    );
    store.update(
        &key(entry.chain_id, entry.owner, entry.tx_hash),
        |old: Option<Entry>| {
            let mut old = old.context("transaksi tidak ada dalam jurnal")?;
            old.status = if receipt.status() {
                Status::Succeeded
            } else {
                Status::Reverted
            };
            old.receipt = Some(receipt);
            old.confirmed_at = None;
            old.updated_at = chrono::Utc::now().to_rfc3339();
            Ok(old)
        },
    )
}

/// Hanya baca receipt. Prepared/Unknown tidak pernah dibroadcast ulang otomatis.
pub async fn reconcile<P: Provider>(
    provider: &P,
    store: &Store,
    chain: u64,
    owner: Address,
) -> Result<ReconcileReport> {
    let rpc_chain = provider
        .get_chain_id()
        .await
        .context("chain id rekonsiliasi tidak terbaca")?;
    ensure!(
        rpc_chain == chain,
        "rekonsiliasi meminta chain {chain}, RPC menjawab {rpc_chain}"
    );
    let latest = provider
        .get_block_number()
        .await
        .context("blok rekonsiliasi tidak terbaca")?;
    let mut report = ReconcileReport::default();
    for entry in entries(store, chain, owner)?
        .into_iter()
        .filter(|e| e.confirmed_at.is_none())
    {
        match provider.get_transaction_receipt(entry.tx_hash).await {
            Ok(Some(receipt)) => {
                let receipt_block = receipt
                    .block_number
                    .context("receipt belum mempunyai nomor blok")?;
                let mut updated = record_receipt(store, &entry, receipt)?;
                let confirmations = latest.saturating_sub(receipt_block).saturating_add(1);
                if confirmations >= FINALITY_CONFIRMATIONS {
                    updated =
                        store.update(&key(chain, owner, entry.tx_hash), |old: Option<Entry>| {
                            let mut old = old.context("transaksi tidak ada dalam jurnal")?;
                            if old.confirmed_at.is_none() {
                                let now = chrono::Utc::now().to_rfc3339();
                                old.confirmed_at = Some(now.clone());
                                old.updated_at = now;
                            }
                            Ok(old)
                        })?;
                }
                if updated.status != entry.status || updated.confirmed_at != entry.confirmed_at {
                    report.changed.push(updated);
                }
            }
            Ok(None) => {
                let nonce = entry.request.nonce.context("nonce jurnal hilang")?;
                let confirmed_nonce = match provider.get_transaction_count(owner).latest().await {
                    Ok(value) => value,
                    Err(error) => {
                        report.errors.push(format!("nonce {owner}: {error}"));
                        continue;
                    }
                };
                if confirmed_nonce > nonce {
                    let updated =
                        store.update(&key(chain, owner, entry.tx_hash), |old: Option<Entry>| {
                            let mut old = old.context("transaksi tidak ada dalam jurnal")?;
                            old.status = Status::Replaced;
                            old.receipt = None;
                            old.confirmed_at = Some(chrono::Utc::now().to_rfc3339());
                            old.broadcast_error =
                                Some("nonce telah mined oleh hash transaksi lain".into());
                            old.updated_at = chrono::Utc::now().to_rfc3339();
                            Ok(old)
                        })?;
                    report.changed.push(updated);
                } else if entry.receipt.is_some() {
                    let updated =
                        store.update(&key(chain, owner, entry.tx_hash), |old: Option<Entry>| {
                            let mut old = old.context("transaksi tidak ada dalam jurnal")?;
                            old.status = Status::Unknown;
                            old.receipt = None;
                            old.confirmed_at = None;
                            old.broadcast_error =
                                Some("receipt hilang sebelum finalitas; kemungkinan reorg".into());
                            old.updated_at = chrono::Utc::now().to_rfc3339();
                            Ok(old)
                        })?;
                    report.changed.push(updated);
                }
            }
            Err(error) => report.errors.push(format!("{}: {error}", entry.tx_hash)),
        }
    }
    Ok(report)
}
