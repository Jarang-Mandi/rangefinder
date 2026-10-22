//! Perakitan gate untuk membuka posisi V4.
//!
//! Bentuk hasilnya sengaja sama dengan jalur V3, yaitu `OpenPlan`, supaya panel
//! tidak perlu tahu protokol mana yang sedang dipakai. Yang berbeda hanya isi
//! `detail` tiap gate.
//!
//! INVARIAN
//! - Urutan gate sama dengan V3: yang murah dan pasti berjalan lebih dulu.
//! - Batas atas jumlah adalah SATU SATUNYA pengaman slippage di V4. Tidak ada
//!   `amountMin` seperti V3. Likuiditas sengaja dibangun dari 90 persen modal,
//!   sedangkan batas atas tetap tidak boleh melewati anggaran user.
//! - Rencana yang gagal di gate mana pun TIDAK membawa calldata.

use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::Provider;
use alloy::rpc::types::TransactionRequest;

use crate::config::{ExecutionConfig, LimitsConfig};
use crate::exec::limits::{self, LimitContext};
use crate::exec::pipeline::{hitung_jumlah, GateResult, OpenPlan, OpenRequest};

use super::abi::IPositionManagerV4::PoolKey;
use super::calldata::{self, MintV4Request};

/// Ruang perubahan rasio antara simulasi dan blok masuk pada pasar cepat.
const CADANGAN_VOLATIL_BPS: u32 = 1_000;

/// Susun rencana membuka posisi V4, jalankan gate G0, G6, G2, G4, G5.
///
/// G1 berjalan saat engine start. G3 berjalan saat build sebagai uji terhadap
/// transaksi sungguhan. G7 berjalan setelah pengiriman.
pub async fn plan_open_v4<P: Provider>(
    provider: &P,
    position_manager: Address,
    req: &OpenRequest,
    limits_cfg: &LimitsConfig,
    exec_cfg: &ExecutionConfig,
) -> OpenPlan {
    let mut gates = Vec::new();
    let mut warnings = Vec::new();

    let mut plan = OpenPlan {
        pool_id: req.pool.id.clone(),
        tick_lower: req.tick_lower,
        tick_upper: req.tick_upper,
        amount0: "0".into(),
        amount1: "0".into(),
        liquidity: "0".into(),
        token_id_simulasi: "0".into(),
        gas_estimate: 0,
        dry_run: exec_cfg.dry_run,
        gates: Vec::new(),
        warnings: Vec::new(),
        calldata: None,
        request: None,
        v4_request: None,
    };

    // ─── G0, versi protokol ───────────────────────────────────────
    if req.pool.version != crate::pools::PoolVersion::V4 {
        gates.push(GateResult::gagal_publik(
            "G0",
            "versi protokol",
            format!("pool {:?} bukan V4, jalur ini khusus V4", req.pool.version),
        ));
        plan.gates = gates;
        return plan;
    }
    gates.push(GateResult::lolos_publik(
        "G0",
        "versi protokol",
        "pool V4, cocok dengan jalur eksekusi ini",
    ));

    if req.pool.dynamic_fee {
        warnings.push(
            "pool ini memakai dynamic fee, jadi besaran fee ditentukan hook saat swap dan \
             tidak bisa diketahui dari keadaan pool"
                .into(),
        );
    }
    if crate::pools::has_hook(&req.pool) {
        warnings.push(format!(
            "pool ini memakai hook {}. Periksa allowlist sebelum menaruh dana",
            req.pool.hooks.as_deref().unwrap_or("?")
        ));
    }

    // ─── G6, paling murah dan paling sering menolak ───────────────
    let limit_ctx = LimitContext {
        position_usd: req.position_usd,
        daily_spent_usd: req.daily_spent_usd,
        open_positions: req.open_positions,
        is_new_pool: req.is_new_pool,
        eth_balance: req.eth_balance,
        eth_needed: req.eth_needed,
    };
    match limits::check(limits_cfg, &limit_ctx) {
        Ok(()) => gates.push(GateResult::lolos_publik(
            "G6",
            "pembatas nilai",
            "dalam batas",
        )),
        Err(alasan) => {
            gates.push(GateResult::gagal_publik(
                "G6",
                "pembatas nilai",
                alasan.join("; "),
            ));
            plan.gates = gates;
            plan.warnings = warnings;
            return plan;
        }
    }

    // ─── Hitungan likuiditas, sama untuk V3 dan V4 ────────────────
    let aman0 = modal_setelah_cadangan(req.available0);
    let aman1 = modal_setelah_cadangan(req.available1);
    let (liquidity, butuh0, butuh1) =
        match hitung_jumlah(&req.pool, req.tick_lower, req.tick_upper, aman0, aman1) {
            Ok(v) => v,
            Err(e) => {
                gates.push(GateResult::gagal_publik("G2", "builder calldata", e));
                plan.gates = gates;
                plan.warnings = warnings;
                return plan;
            }
        };

    plan.liquidity = liquidity.to_string();
    plan.amount0 = butuh0.to_string();
    plan.amount1 = butuh1.to_string();

    // Likuiditas hanya memakai modal aman di atas, tetapi batas kontrak boleh
    // memakai seluruh anggaran yang sudah disetujui. Selisihnya menyerap
    // perubahan rasio antartoken sampai transaksi masuk blok.
    let maks0 = req.available0;
    let maks1 = req.available1;

    // ─── G2, builder calldata ─────────────────────────────────────
    let pool_key = match pool_key_dari(&req.pool) {
        Ok(k) => k,
        Err(e) => {
            gates.push(GateResult::gagal_publik("G2", "builder calldata", e));
            plan.gates = gates;
            plan.warnings = warnings;
            return plan;
        }
    };

    let v4_req = MintV4Request {
        pool_key: pool_key.clone(),
        tick_lower: req.tick_lower,
        tick_upper: req.tick_upper,
        liquidity,
        amount0_max: potong_u128(maks0),
        amount1_max: potong_u128(maks1),
        owner: req.recipient,
        deadline: req.deadline,
    };

    let data = match calldata::mint(&v4_req) {
        Ok(d) => d,
        Err(e) => {
            gates.push(GateResult::gagal_publik(
                "G2",
                "builder calldata",
                e.to_string(),
            ));
            plan.gates = gates;
            plan.warnings = warnings;
            return plan;
        }
    };
    gates.push(GateResult::lolos_publik(
        "G2",
        "builder calldata",
        format!(
            "{} byte, dua aksi mint lalu settle, tick pas di grid {}",
            data.len(),
            req.pool.tick_spacing
        ),
    ));

    if let Err(e) = crate::exec::golden::verify_embedded() {
        gates.push(GateResult::gagal_publik("G3", "golden vector", e));
        plan.gates = gates;
        plan.warnings = warnings;
        return plan;
    }
    gates.push(GateResult::lolos_publik(
        "G3",
        "golden vector",
        "builder cocok byte per byte dengan transaksi acuan tertanam",
    ));

    // ─── G4, simulasi ─────────────────────────────────────────────
    //
    // `modifyLiquidities` tidak mengembalikan apa apa. `eth_estimateGas`
    // menjalankan transaksi yang sama dan gagal jika kontrak revert, jadi satu
    // jawaban membuktikan G4 sekaligus memberi estimasi gas. Menjalankan
    // `eth_call` lebih dulu hanya mengulang simulasi yang identik.
    let value = v4_req.value();
    let tx = TransactionRequest::default()
        .from(req.recipient)
        .to(position_manager)
        .value(value)
        .input(data.clone().into());

    match provider.estimate_gas(tx).await {
        Ok(gas) => {
            plan.gas_estimate = gas;
            gates.push(GateResult::lolos_publik(
                "G4",
                "simulasi",
                format!("kontrak menerima, likuiditas {liquidity} akan tercipta"),
            ));
        }
        Err(e) => {
            let teks = e.to_string();
            let pesan = if teks.contains("revert") || teks.contains("execution reverted") {
                crate::exec::simulate::jelaskan_revert(&teks)
            } else {
                format!("RPC gagal: {teks}")
            };
            gates.push(GateResult::gagal_publik("G4", "simulasi", pesan));
            plan.gates = gates;
            plan.warnings = warnings;
            return plan;
        }
    }

    // ─── G5, asersi invarian ──────────────────────────────────────
    let mut pelanggaran = Vec::new();
    if liquidity == 0 {
        pelanggaran.push("likuiditas nol, tidak ada posisi yang tercipta".to_string());
    }
    if butuh0 > req.available0 {
        pelanggaran.push(format!(
            "token0 yang dibutuhkan {butuh0} melebihi yang tersedia {}",
            req.available0
        ));
    }
    if butuh1 > req.available1 {
        pelanggaran.push(format!(
            "token1 yang dibutuhkan {butuh1} melebihi yang tersedia {}",
            req.available1
        ));
    }
    if maks0 < butuh0 || maks1 < butuh1 {
        pelanggaran.push(
            "batas atas jumlah lebih kecil dari yang dibutuhkan, kontrak pasti menolak".to_string(),
        );
    }
    // ETH asli ikut sebagai msg.value. Kalau pool-nya tidak memakai ETH asli,
    // nilai yang ikut akan tersangkut di kontrak.
    let pakai_native = pool_key.currency0.is_zero() || pool_key.currency1.is_zero();
    if !pakai_native && !value.is_zero() {
        pelanggaran.push("nilai ETH ikut padahal pool tidak memakai ETH asli".to_string());
    }

    if pelanggaran.is_empty() {
        gates.push(GateResult::lolos_publik(
            "G5",
            "asersi invarian",
            "jumlah, batas atas, dan nilai ETH konsisten",
        ));
    } else {
        gates.push(GateResult::gagal_publik(
            "G5",
            "asersi invarian",
            pelanggaran.join("; "),
        ));
        plan.gates = gates;
        plan.warnings = warnings;
        return plan;
    }

    plan.gates = gates;
    plan.warnings = warnings;
    plan.calldata = Some(data);
    plan.v4_request = Some(v4_req);
    plan
}

pub(crate) fn modal_setelah_cadangan(tersedia: U256) -> U256 {
    let bps = U256::from(10_000u32);
    let dipakai = bps - U256::from(CADANGAN_VOLATIL_BPS);
    let aman = crate::math::liquidity_math::mul_div(tersedia, dipakai, bps).unwrap_or(tersedia);
    if aman.is_zero() && !tersedia.is_zero() {
        tersedia
    } else {
        aman
    }
}

/// Potong ke `u128` yang diterima kontrak.
///
/// Nilai di atas `u128` tidak mungkin muncul dari saldo token sungguhan, tapi
/// kalau muncul, membiarkannya membungkus jadi angka kecil jauh lebih berbahaya
/// daripada memakai batas maksimum.
fn potong_u128(v: U256) -> u128 {
    v.try_into().unwrap_or(u128::MAX)
}

/// Susun PoolKey dari `PoolInfo`.
///
/// `fee` di sini adalah angka apa adanya dari pool, termasuk penanda dynamic
/// fee `0x800000`. Itu memang yang ada di PoolKey, dan menggantinya dengan fee
/// tebakan akan menghasilkan poolId yang berbeda.
pub fn pool_key_dari(pool: &crate::pools::PoolInfo) -> Result<PoolKey, String> {
    use alloy::primitives::aliases::{I24, U24};

    let c0: Address = pool
        .token0
        .parse()
        .map_err(|_| format!("currency0 `{}` bukan alamat EVM", pool.token0))?;
    let c1: Address = pool
        .token1
        .parse()
        .map_err(|_| format!("currency1 `{}` bukan alamat EVM", pool.token1))?;
    let hooks: Address = match pool.hooks.as_deref() {
        Some(h) => h
            .parse()
            .map_err(|_| format!("hooks `{h}` bukan alamat EVM"))?,
        None => Address::ZERO,
    };

    Ok(PoolKey {
        currency0: c0,
        currency1: c1,
        fee: U24::from(pool.fee),
        tickSpacing: I24::try_from(pool.tick_spacing)
            .map_err(|_| format!("tickSpacing {} di luar jangkauan int24", pool.tick_spacing))?,
        hooks,
    })
}

/// Ambil tokenId posisi V4 dari log `Transfer` NFT di receipt.
///
/// V4 tidak memancarkan `IncreaseLiquidity` di PositionManager seperti V3, jadi
/// penanda posisi barunya adalah `Transfer` dari alamat nol.
pub fn token_id_dari_receipt(
    receipt: &alloy::rpc::types::TransactionReceipt,
    position_manager: Address,
) -> Option<U256> {
    const TRANSFER: [u8; 32] = [
        0xdd, 0xf2, 0x52, 0xad, 0x1b, 0xe2, 0xc8, 0x9b, 0x69, 0xc2, 0xb0, 0x68, 0xfc, 0x37, 0x8d,
        0xaa, 0x95, 0x2b, 0xa7, 0xf1, 0x63, 0xc4, 0xa1, 0x16, 0x28, 0xf5, 0x5a, 0x4d, 0xf5, 0x23,
        0xb3, 0xef,
    ];

    receipt.inner.logs().iter().find_map(|log| {
        let inner = &log.inner;
        if inner.address != position_manager {
            return None;
        }
        let topics = inner.data.topics();
        // Transfer ERC721 punya empat topic: signature, from, to, tokenId.
        // Yang menandai posisi baru adalah `from` bernilai nol.
        if topics.len() != 4 || topics[0].as_slice() != TRANSFER {
            return None;
        }
        if !topics[1].iter().all(|b| *b == 0) {
            return None;
        }
        Some(U256::from_be_bytes(topics[3].0))
    })
}

/// Calldata kosong yang bisa dipakai saat rencana ditolak, supaya tipe
/// kembaliannya tetap seragam.
pub const CALLDATA_KOSONG: Bytes = Bytes::new();

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pools::{PoolInfo, PoolVersion};

    fn pool_v4() -> PoolInfo {
        PoolInfo {
            version: PoolVersion::V4,
            id: "0xabc".into(),
            token0: "0x5fc5360d0400a0fd4f2af552add042d716f1d168".into(),
            token1: "0x791e750fa8b2237288a6fe6c47ea81c50093449a".into(),
            fee: 100_000,
            dynamic_fee: false,
            tick_spacing: 1000,
            hooks: Some("0x0000000000000000000000000000000000000000".into()),
            sqrt_price_x96: "0".into(),
            tick: 357_500,
            liquidity: "1".into(),
            raw_price: 1.0,
            stats: None,
        }
    }

    #[test]
    fn pool_key_membawa_fee_apa_adanya() {
        // Penanda dynamic fee 0x800000 adalah bagian dari PoolKey. Menggantinya
        // dengan fee tebakan menghasilkan poolId yang berbeda, yaitu pool lain.
        let mut p = pool_v4();
        p.fee = crate::pools::DYNAMIC_FEE_FLAG;
        p.dynamic_fee = true;
        let key = pool_key_dari(&p).unwrap();
        assert_eq!(key.fee.to::<u32>(), crate::pools::DYNAMIC_FEE_FLAG);
    }

    #[test]
    fn hooks_kosong_jadi_alamat_nol() {
        let mut p = pool_v4();
        p.hooks = None;
        assert_eq!(pool_key_dari(&p).unwrap().hooks, Address::ZERO);
    }

    #[test]
    fn cadangan_volatil_menyisakan_sepuluh_persen() {
        assert_eq!(
            modal_setelah_cadangan(U256::from(10_000u64)),
            U256::from(9_000u64)
        );
        assert_eq!(modal_setelah_cadangan(U256::from(1u64)), U256::from(1u64));
    }

    #[test]
    fn potong_u128_memakai_batas_bukan_membungkus() {
        assert_eq!(potong_u128(U256::MAX), u128::MAX);
        assert_eq!(potong_u128(U256::from(42u8)), 42);
    }

    #[test]
    fn tick_spacing_di_luar_int24_ditolak() {
        let mut p = pool_v4();
        p.tick_spacing = 10_000_000;
        assert!(pool_key_dari(&p).is_err());
    }
}
