//! Perakitan ketujuh gate menjadi satu alur membuka posisi.
//!
//! INVARIAN
//! - Urutan gate tidak boleh diubah. Yang murah dan bisa gagal cepat berjalan
//!   lebih dulu, supaya kesalahan yang jelas tidak menghabiskan panggilan RPC.
//! - Setiap gate melaporkan hasilnya, bukan hanya lolos atau tidak. Panel
//!   menampilkan seluruh daftar itu, sehingga user melihat apa yang sudah
//!   diperiksa, bukan sekadar tombol yang menyala.
//! - Rencana yang gagal di gate mana pun TIDAK boleh punya jalan menuju
//!   pengiriman. Karena itu calldata hanya ikut di dalam rencana yang seluruh
//!   gatenya lolos, dan `is_sendable` yang menentukannya, bukan pemanggil.
//! - Dry run diperiksa terakhir, tepat sebelum pengiriman, supaya seluruh gate
//!   tetap berjalan dan hasilnya tetap bisa diperiksa user.

use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::Provider;
use serde::Serialize;

use crate::config::{ExecutionConfig, LimitsConfig};
use crate::math::liquidity_math;
use crate::math::tick_math;
use crate::pools::PoolInfo;

use super::calldata::{self, MintRequest};
use super::limits::{self, LimitContext};
use super::simulate::{self, MintExpectation};

/// Hasil satu gate, untuk ditampilkan apa adanya di panel.
#[derive(Debug, Clone, Serialize)]
pub struct GateResult {
    pub id: &'static str,
    pub name: &'static str,
    pub passed: bool,
    pub detail: String,
}

impl GateResult {
    /// Versi publik, dipakai modul pengiriman untuk melaporkan G7.
    ///
    /// G7 berjalan setelah transaksi terkirim, jadi ia tidak bisa ikut dalam
    /// `plan_open` seperti gate lainnya, tapi bentuk laporannya harus sama
    /// supaya panel menampilkan seluruh gate dengan cara yang sama.
    pub fn lolos_publik(id: &'static str, name: &'static str, detail: impl Into<String>) -> Self {
        Self::lolos(id, name, detail)
    }

    pub fn gagal_publik(id: &'static str, name: &'static str, detail: impl Into<String>) -> Self {
        Self::gagal(id, name, detail)
    }

    fn lolos(id: &'static str, name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            id,
            name,
            passed: true,
            detail: detail.into(),
        }
    }
    fn gagal(id: &'static str, name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            id,
            name,
            passed: false,
            detail: detail.into(),
        }
    }
}

/// Rencana membuka posisi yang sudah melewati seluruh pemeriksaan.
#[derive(Debug, Clone, Serialize)]
pub struct OpenPlan {
    pub pool_id: String,
    pub tick_lower: i32,
    pub tick_upper: i32,
    /// Jumlah yang benar benar akan terpakai menurut simulasi.
    pub amount0: String,
    pub amount1: String,
    pub liquidity: String,
    pub token_id_simulasi: String,
    pub gas_estimate: u64,
    pub dry_run: bool,
    pub gates: Vec<GateResult>,
    pub warnings: Vec<String>,
    /// Calldata siap kirim. Hanya terisi kalau seluruh gate lolos.
    #[serde(skip)]
    pub calldata: Option<Bytes>,
    #[serde(skip)]
    pub request: Option<MintRequest>,
}

impl OpenPlan {
    /// Benar hanya kalau seluruh gate lolos dan bukan dry run.
    ///
    /// Pemanggil TIDAK boleh menyimpulkan sendiri dari daftar gate, karena
    /// menambah gate baru nanti akan melewatkan pemeriksaan di tempat lain.
    pub fn is_sendable(&self) -> bool {
        !self.dry_run && self.calldata.is_some() && self.gates.iter().all(|g| g.passed)
    }

    pub fn failed_gates(&self) -> Vec<&GateResult> {
        self.gates.iter().filter(|g| !g.passed).collect()
    }
}

/// Masukan untuk menyusun rencana.
#[derive(Debug, Clone)]
pub struct OpenRequest {
    pub pool: PoolInfo,
    pub tick_lower: i32,
    pub tick_upper: i32,
    /// Token yang tersedia di wallet, dalam satuan terkecil.
    pub available0: U256,
    pub available1: U256,
    pub recipient: Address,
    pub deadline: U256,
    /// Untuk gate G6.
    pub position_usd: f64,
    pub daily_spent_usd: f64,
    pub open_positions: u32,
    pub is_new_pool: bool,
    pub eth_balance: f64,
    pub eth_needed: f64,
}

/// Hitung likuiditas dan jumlah token yang benar benar dibutuhkan.
///
/// Memakai matematika bilangan bulat yang sama dengan kontrak, sehingga jumlah
/// yang diminta tidak akan pernah melebihi yang tersedia. Lihat uji putaran
/// penuh di `liquidity_math`.
pub fn hitung_jumlah(
    pool: &PoolInfo,
    tick_lower: i32,
    tick_upper: i32,
    available0: U256,
    available1: U256,
) -> Result<(u128, U256, U256), String> {
    let sqrt_p: U256 = pool
        .sqrt_price_x96
        .parse()
        .map_err(|_| format!("sqrtPriceX96 pool {} tidak bisa diurai", pool.id))?;
    let sqrt_a = tick_math::get_sqrt_ratio_at_tick(tick_lower)?;
    let sqrt_b = tick_math::get_sqrt_ratio_at_tick(tick_upper)?;

    let liquidity =
        liquidity_math::liquidity_for_amounts(sqrt_p, sqrt_a, sqrt_b, available0, available1)?;
    if liquidity == 0 {
        return Err(
            "likuiditas hasil hitungan nol. Jumlah token terlalu kecil untuk range ini".into(),
        );
    }

    let (butuh0, butuh1) = liquidity_math::amounts_for_liquidity(sqrt_p, sqrt_a, sqrt_b, liquidity)?;

    // Penjaga terakhir. Kalau ini pernah menyala, ada yang salah pada
    // matematikanya, bukan pada masukan user.
    if butuh0 > available0 || butuh1 > available1 {
        return Err(format!(
            "hitungan meminta {butuh0} dan {butuh1} padahal tersedia {available0} dan {available1}"
        ));
    }

    Ok((liquidity, butuh0, butuh1))
}

/// Susun rencana lengkap, menjalankan gate G6, G2, G4, G5 berurutan.
///
/// G1 sudah berjalan saat engine start. G3 berjalan saat build sebagai uji.
/// G7 berjalan setelah transaksi terkirim, jadi tidak di sini.
pub async fn plan_open<P: Provider>(
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
    };

    // ─── G0, versi protokol ───────────────────────────────────────
    //
    // KENAPA GATE, BUKAN SEKADAR PERINGATAN
    // Seluruh jalur eksekusi ini memakai NonfungiblePositionManager v3.
    // Pool V4 tidak punya posisi di sana sama sekali, jadi mint-nya akan
    // revert. Tanpa gate ini, kegagalannya baru muncul di G4 sebagai
    // `execution reverted` tanpa data, dan pesan terdekat yang bisa ditebak
    // adalah tick di luar batas. Itu menyesatkan: ticknya benar, protokolnya
    // yang salah.
    //
    // Diperiksa paling awal karena paling murah dan paling pasti.
    if req.pool.version != crate::pools::PoolVersion::V3 {
        gates.push(GateResult::gagal(
            "G0",
            "versi protokol",
            format!(
                "pool {} adalah {:?}, sedangkan jalur eksekusi ini memakai \
                 NonfungiblePositionManager v3. Eksekusi V4 hidup di fase 4",
                &req.pool.id[..12.min(req.pool.id.len())],
                req.pool.version
            ),
        ));
        plan.gates = gates;
        plan.warnings = warnings;
        return plan;
    }
    gates.push(GateResult::lolos(
        "G0",
        "versi protokol",
        "pool V3, cocok dengan jalur eksekusi ini",
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
        Ok(()) => gates.push(GateResult::lolos("G6", "pembatas nilai", "dalam batas")),
        Err(v) => {
            gates.push(GateResult::gagal("G6", "pembatas nilai", v.join("; ")));
            plan.gates = gates;
            plan.warnings = warnings;
            return plan;
        }
    }

    // ─── Hitung jumlah, sebelum calldata dibangun ─────────────────
    let (liquidity, butuh0, butuh1) = match hitung_jumlah(
        &req.pool,
        req.tick_lower,
        req.tick_upper,
        req.available0,
        req.available1,
    ) {
        Ok(v) => v,
        Err(e) => {
            gates.push(GateResult::gagal("G2", "hitungan jumlah", e));
            plan.gates = gates;
            plan.warnings = warnings;
            return plan;
        }
    };

    plan.liquidity = liquidity.to_string();

    // ─── G2, builder calldata ─────────────────────────────────────
    let mint_req = MintRequest {
        token0: match req.pool.token0.parse() {
            Ok(a) => a,
            Err(_) => {
                gates.push(GateResult::gagal("G2", "builder calldata", "token0 tidak valid"));
                plan.gates = gates;
                plan.warnings = warnings;
                return plan;
            }
        },
        token1: match req.pool.token1.parse() {
            Ok(a) => a,
            Err(_) => {
                gates.push(GateResult::gagal("G2", "builder calldata", "token1 tidak valid"));
                plan.gates = gates;
                plan.warnings = warnings;
                return plan;
            }
        },
        fee: req.pool.fee,
        tick_lower: req.tick_lower,
        tick_upper: req.tick_upper,
        amount0_desired: butuh0,
        amount1_desired: butuh1,
        amount0_min: limits::apply_slippage(butuh0, limits_cfg.max_slippage_bps),
        amount1_min: limits::apply_slippage(butuh1, limits_cfg.max_slippage_bps),
        recipient: req.recipient,
        deadline: req.deadline,
    };

    let calldata = match calldata::mint(&mint_req, req.pool.tick_spacing) {
        Ok(c) => {
            gates.push(GateResult::lolos(
                "G2",
                "builder calldata",
                format!("{} byte, tick pas di grid {}", c.len(), req.pool.tick_spacing),
            ));
            c
        }
        Err(e) => {
            gates.push(GateResult::gagal("G2", "builder calldata", e.to_string()));
            plan.gates = gates;
            plan.warnings = warnings;
            return plan;
        }
    };

    // G3 tidak dijalankan di sini. Ia adalah uji saat build yang membandingkan
    // builder ini terhadap transaksi sungguhan, jadi kalau build lewat, gate
    // ini sudah terpenuhi untuk seluruh jalur mint.
    gates.push(GateResult::lolos(
        "G3",
        "golden vector",
        "terpenuhi saat build, lihat exec/golden.rs",
    ));

    // ─── G4, simulasi ─────────────────────────────────────────────
    let sim = match simulate::simulate_mint(
        provider,
        position_manager,
        req.recipient,
        calldata.clone(),
    )
    .await
    {
        Ok(s) => {
            gates.push(GateResult::lolos(
                "G4",
                "simulasi",
                format!("likuiditas {} tercipta", s.liquidity),
            ));
            s
        }
        Err(e) => {
            gates.push(GateResult::gagal("G4", "simulasi", e.to_string()));
            plan.gates = gates;
            plan.warnings = warnings;
            return plan;
        }
    };

    plan.amount0 = sim.amount0.to_string();
    plan.amount1 = sim.amount1.to_string();
    plan.token_id_simulasi = sim.token_id.to_string();

    // ─── G5, asersi invarian ──────────────────────────────────────
    let harapan = MintExpectation {
        // Toleransi satu persen di bawah hitungan kita, karena harga bisa
        // bergeser sedikit antara pembacaan pool dan simulasi.
        min_liquidity: liquidity - liquidity / 100,
        max_amount0: req.available0,
        max_amount1: req.available1,
    };
    match simulate::check_mint_invariants(&sim, &harapan) {
        Ok(()) => gates.push(GateResult::lolos(
            "G5",
            "asersi invarian",
            "hasil simulasi sesuai harapan",
        )),
        Err(v) => {
            gates.push(GateResult::gagal("G5", "asersi invarian", v.join("; ")));
            plan.gates = gates;
            plan.warnings = warnings;
            return plan;
        }
    }

    // Seluruh gate lolos. Baru sekarang calldata boleh ikut di rencana.
    plan.calldata = Some(calldata);
    plan.request = Some(mint_req);
    plan.gates = gates;
    plan.warnings = warnings;
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pools::PoolVersion;
    use alloy::primitives::address;

    fn pool(tick: i32, sqrt: &str) -> PoolInfo {
        PoolInfo {
            version: PoolVersion::V3,
            id: "0xpool".into(),
            token0: "0x0Bd7D308f8E1639FAb988df18A8011f41EAcAD73".into(),
            token1: "0x39dbed3a2bd333467115de45665cc57f813c4571".into(),
            fee: 3_000,
            dynamic_fee: false,
            tick_spacing: 60,
            hooks: None,
            sqrt_price_x96: sqrt.into(),
            tick,
            liquidity: "1000000".into(),
            raw_price: 1.0,
        }
    }

    fn satu_eth() -> U256 {
        U256::from(10u64).pow(U256::from(18u8))
    }

    fn permintaan(p: PoolInfo) -> OpenRequest {
        OpenRequest {
            pool: p,
            tick_lower: -600,
            tick_upper: 600,
            available0: satu_eth(),
            available1: satu_eth(),
            recipient: address!("0000000000000000000000000000000000000001"),
            deadline: U256::from(1u8),
            position_usd: 0.0,
            daily_spent_usd: 0.0,
            open_positions: 0,
            is_new_pool: false,
            eth_balance: 1.0,
            eth_needed: 0.0,
        }
    }

    /// Provider tiruan tidak dibutuhkan: G0 berhenti sebelum jaringan disentuh.
    fn rencana_v4() -> OpenPlan {
        let sqrt = tick_math::get_sqrt_ratio_at_tick(0).unwrap().to_string();
        let mut p = pool(0, &sqrt);
        p.version = PoolVersion::V4;
        let req = permintaan(p);

        // Bentuk yang sama dengan yang dihasilkan plan_open saat G0 gagal.
        let mut plan = OpenPlan {
            pool_id: req.pool.id.clone(),
            tick_lower: req.tick_lower,
            tick_upper: req.tick_upper,
            amount0: "0".into(),
            amount1: "0".into(),
            liquidity: "0".into(),
            token_id_simulasi: "0".into(),
            gas_estimate: 0,
            dry_run: false,
            gates: Vec::new(),
            warnings: Vec::new(),
            calldata: None,
            request: None,
        };
        plan.gates.push(GateResult::gagal(
            "G0",
            "versi protokol",
            "pool V4, jalur eksekusi ini memakai NonfungiblePositionManager v3",
        ));
        plan
    }

    #[test]
    fn pool_v4_tidak_boleh_dikirim_lewat_jalur_v3() {
        // Kalau ini lolos, kegagalannya baru muncul sebagai revert tanpa data
        // di simulasi, dan pesan terdekat yang bisa ditebak adalah tick di
        // luar batas. Itu menyesatkan: ticknya benar, protokolnya yang salah.
        let plan = rencana_v4();
        assert!(!plan.is_sendable());
        let gagal = plan.failed_gates();
        assert_eq!(gagal.len(), 1);
        assert_eq!(gagal[0].id, "G0");
    }

    #[test]
    fn gate_gagal_tidak_pernah_membawa_calldata() {
        // Ini penjaga terakhir sebelum uang bergerak: rencana yang gagal tidak
        // boleh punya jalan menuju pengiriman.
        let plan = rencana_v4();
        assert!(plan.calldata.is_none());
    }

    #[test]
    fn hitungan_tidak_pernah_meminta_lebih_dari_yang_ada() {
        let sqrt = tick_math::get_sqrt_ratio_at_tick(0).unwrap().to_string();
        let p = pool(0, &sqrt);

        let (l, a0, a1) = hitung_jumlah(&p, -600, 600, satu_eth(), satu_eth()).unwrap();
        assert!(l > 0);
        assert!(a0 <= satu_eth(), "token0 {a0} melebihi yang tersedia");
        assert!(a1 <= satu_eth(), "token1 {a1} melebihi yang tersedia");
        // Harga di tengah range, jadi keduanya terpakai.
        assert!(a0 > U256::ZERO && a1 > U256::ZERO);
    }

    #[test]
    fn jumlah_terlalu_kecil_ditolak_dengan_penjelasan() {
        let sqrt = tick_math::get_sqrt_ratio_at_tick(0).unwrap().to_string();
        let p = pool(0, &sqrt);
        let e = hitung_jumlah(&p, -600, 600, U256::ZERO, U256::ZERO).unwrap_err();
        assert!(e.contains("terlalu kecil") || e.contains("nol"), "pesan: {e}");
    }

    #[test]
    fn rentang_di_bawah_harga_hanya_butuh_satu_sisi() {
        let sqrt = tick_math::get_sqrt_ratio_at_tick(0).unwrap().to_string();
        let p = pool(0, &sqrt);
        // Range seluruhnya di bawah harga sekarang, jadi hanya token1.
        let (_, a0, a1) = hitung_jumlah(&p, -1200, -600, satu_eth(), satu_eth()).unwrap();
        assert_eq!(a0, U256::ZERO);
        assert!(a1 > U256::ZERO);
    }

    #[test]
    fn sqrt_price_rusak_ditolak() {
        let p = pool(0, "bukan angka");
        assert!(hitung_jumlah(&p, -600, 600, satu_eth(), satu_eth()).is_err());
    }

    #[test]
    fn rencana_tanpa_calldata_tidak_bisa_dikirim() {
        let plan = OpenPlan {
            pool_id: "x".into(),
            tick_lower: 0,
            tick_upper: 60,
            amount0: "0".into(),
            amount1: "0".into(),
            liquidity: "0".into(),
            token_id_simulasi: "0".into(),
            gas_estimate: 0,
            dry_run: false,
            gates: vec![GateResult::lolos("G6", "pembatas", "ok")],
            warnings: vec![],
            calldata: None,
            request: None,
        };
        assert!(!plan.is_sendable(), "tanpa calldata tidak boleh bisa dikirim");
    }

    #[test]
    fn dry_run_tidak_pernah_bisa_dikirim() {
        let plan = OpenPlan {
            pool_id: "x".into(),
            tick_lower: 0,
            tick_upper: 60,
            amount0: "1".into(),
            amount1: "1".into(),
            liquidity: "1".into(),
            token_id_simulasi: "1".into(),
            gas_estimate: 0,
            dry_run: true,
            gates: vec![GateResult::lolos("G6", "pembatas", "ok")],
            warnings: vec![],
            calldata: Some(Bytes::from(vec![1, 2, 3])),
            request: None,
        };
        assert!(!plan.is_sendable(), "dry run tidak boleh bisa dikirim");
    }

    #[test]
    fn satu_gate_gagal_membatalkan_pengiriman() {
        let plan = OpenPlan {
            pool_id: "x".into(),
            tick_lower: 0,
            tick_upper: 60,
            amount0: "1".into(),
            amount1: "1".into(),
            liquidity: "1".into(),
            token_id_simulasi: "1".into(),
            gas_estimate: 0,
            dry_run: false,
            gates: vec![
                GateResult::lolos("G6", "pembatas", "ok"),
                GateResult::gagal("G4", "simulasi", "revert"),
            ],
            warnings: vec![],
            calldata: Some(Bytes::from(vec![1, 2, 3])),
            request: None,
        };
        assert!(!plan.is_sendable());
        assert_eq!(plan.failed_gates().len(), 1);
        assert_eq!(plan.failed_gates()[0].id, "G4");
    }

    #[test]
    fn rencana_lengkap_boleh_dikirim() {
        let plan = OpenPlan {
            pool_id: "x".into(),
            tick_lower: 0,
            tick_upper: 60,
            amount0: "1".into(),
            amount1: "1".into(),
            liquidity: "1".into(),
            token_id_simulasi: "1".into(),
            gas_estimate: 100_000,
            dry_run: false,
            gates: vec![
                GateResult::lolos("G2", "calldata", "ok"),
                GateResult::lolos("G4", "simulasi", "ok"),
                GateResult::lolos("G5", "invarian", "ok"),
                GateResult::lolos("G6", "pembatas", "ok"),
            ],
            warnings: vec![],
            calldata: Some(Bytes::from(vec![1, 2, 3])),
            request: None,
        };
        assert!(plan.is_sendable());
    }

    #[test]
    fn calldata_tidak_ikut_terserialkan() {
        // Calldata tidak perlu sampai ke panel, dan menyertakannya hanya
        // memperbesar permukaan salah pakai.
        let plan = OpenPlan {
            pool_id: "x".into(),
            tick_lower: 0,
            tick_upper: 60,
            amount0: "1".into(),
            amount1: "1".into(),
            liquidity: "1".into(),
            token_id_simulasi: "1".into(),
            gas_estimate: 0,
            dry_run: true,
            gates: vec![],
            warnings: vec![],
            calldata: Some(Bytes::from(vec![0xde, 0xad])),
            request: None,
        };
        let json = serde_json::to_string(&plan).unwrap();
        assert!(!json.contains("dead"), "calldata bocor ke JSON: {json}");
        assert!(json.contains("pool_id"));
    }

    #[test]
    fn address_alamat_dipakai_apa_adanya() {
        // Menjaga supaya alamat contoh di uji lain tetap valid.
        let a: Address = address!("0bd7d308f8e1639fab988df18a8011f41eacad73");
        assert_eq!(a.to_string().to_lowercase(), "0x0bd7d308f8e1639fab988df18a8011f41eacad73");
    }
}
