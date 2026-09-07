//! Jalur eksekusi yang dipakai panel: susun rencana, beri izin, buka posisi.
//!
//! INVARIAN
//! - Tiga langkah ini sengaja terpisah, dan panel harus melewatinya berurutan.
//!   Menggabungkannya jadi satu tombol berarti user menyetujui sesuatu yang
//!   belum pernah ia lihat. PRD baris 18 menyebutnya eksplisit: tidak ada
//!   transaksi tanpa persetujuan per aksi.
//! - Rencana disimpan di engine, bukan dikirim bolak balik. Rencana yang datang
//!   kembali dari panel bisa saja sudah diubah, dan yang diubah itu justru
//!   calldata. Panel hanya menyebut bahwa ia menyetujui rencana terakhir.
//! - Rencana kedaluwarsa kalau pool, tick, atau jumlahnya sudah berbeda dari
//!   keadaan sekarang. Menyetujui rencana lama berarti mengirim transaksi untuk
//!   harga yang sudah lewat.

use alloy::primitives::{Address, U256};
use alloy::providers::Provider;
use serde::Serialize;

use crate::exec::pipeline::{OpenPlan, OpenRequest};
use crate::exec::send::SendOutcome;

/// Berapa lama sebuah rencana masih boleh disetujui.
///
/// Harga memecoin bergerak cepat, dan rencana memuat jumlah token yang dihitung
/// dari harga saat rencana disusun. Tiga menit cukup untuk membaca kartunya
/// dengan tenang, dan terlalu pendek untuk berubah jauh.
pub const RENCANA_KEDALUWARSA_DETIK: i64 = 180;

/// Rencana yang tersimpan beserta konteks yang dibutuhkan untuk mengirimnya.
pub struct RencanaTersimpan {
    pub plan: OpenPlan,
    /// Calldata swap zap beserta nilainya, kalau zap diperlukan.
    ///
    /// Disimpan di engine dengan alasan yang sama dengan calldata mint:
    /// calldata yang bolak balik lewat panel bisa kembali dalam keadaan
    /// sudah diubah.
    pub zap_calldata: Option<(alloy::primitives::Bytes, U256)>,
    /// Versi pool yang direncanakan. Menentukan kontrak tujuan, bentuk
    /// izin, dan cara verifikasi pasca kirim.
    pub versi: crate::pools::PoolVersion,
    pub token0: Address,
    pub token1: Address,
    pub amount0: U256,
    pub amount1: U256,
    pub disusun_pada: chrono::DateTime<chrono::Utc>,
}

impl RencanaTersimpan {
    pub fn kedaluwarsa(&self) -> bool {
        (chrono::Utc::now() - self.disusun_pada).num_seconds() > RENCANA_KEDALUWARSA_DETIK
    }
}

/// Swap yang perlu dijalankan lebih dulu supaya modal cocok dengan rasio range.
///
/// KENAPA TRANSAKSI TERPISAH, BUKAN SATU YANG ATOMIK
/// PRD 5.1 menyebut zap LPZenith bersifat atomik. Di rantai ini itu tidak
/// bisa ditiru untuk V4: swap berjalan lewat UniversalRouter sedangkan posisi
/// dibuat lewat PositionManager, dan tidak ada kontrak yang bisa melakukan
/// keduanya dalam satu panggilan.
///
/// Konsekuensinya harga bisa bergerak di antara kedua transaksi. Itu ditahan
/// oleh `amount_out_minimum`, dan kalau pergerakannya melewati batas, swapnya
/// gagal tanpa memindahkan apa pun alih alih menghasilkan posisi timpang.
#[derive(Debug, Clone, Serialize)]
pub struct ZapView {
    /// Token yang ditukar.
    pub dari: String,
    pub ke: String,
    /// Jumlah yang ditukar, dalam satuan terkecil.
    pub jumlah: String,
    /// Perkiraan yang diterima menurut quoter.
    pub perkiraan_diterima: String,
    /// Batas bawah yang masih diterima. Satu satunya pengaman slippage.
    pub minimum_diterima: String,
    /// Berapa persen modal yang ditukar, untuk ditampilkan.
    pub porsi_bps: u32,
    /// Modal yang akan menganggur kalau zap TIDAK dijalankan.
    pub menganggur_tanpa_zap: String,
}

/// Ringkasan rencana untuk panel, beserta status izin tiap token.
#[derive(Debug, Clone, Serialize)]
pub struct OpenPlanView {
    #[serde(flatten)]
    pub plan: OpenPlan,
    /// Langkah izin yang masih harus dikerjakan, berurutan. Kosong berarti
    /// posisi bisa langsung dibuka.
    ///
    /// Bentuknya seragam untuk V3 dan V4. V3 butuh satu langkah per token,
    /// V4 butuh dua karena izinnya lewat Permit2. Panel tidak perlu tahu
    /// bedanya, ia hanya menekan tombolnya sampai daftarnya kosong.
    pub allowances: Vec<crate::exec::plan::LangkahIzin>,
    /// `v3` atau `v4`, supaya panel bisa menyebutnya di layar.
    pub version: &'static str,
    /// Terisi kalau modal user tidak cocok dengan rasio range, sehingga
    /// sebagian perlu ditukar lebih dulu. Kosong berarti tidak perlu.
    pub zap: Option<ZapView>,
    pub expires_at: String,
}

/// Aksi pada posisi yang sudah disusun dan menunggu persetujuan.
pub struct AksiTersimpan {
    pub token_id: String,
    pub calldata: alloy::primitives::Bytes,
    /// Kontrak tujuan, dipilih menurut versi posisinya.
    pub tujuan: Address,
    pub menutup: bool,
    pub nama: &'static str,
    pub disusun_pada: chrono::DateTime<chrono::Utc>,
}

impl AksiTersimpan {
    pub fn kedaluwarsa(&self) -> bool {
        (chrono::Utc::now() - self.disusun_pada).num_seconds() > RENCANA_KEDALUWARSA_DETIK
    }
}

/// Rencana aksi beserta hasil pemeriksaannya, untuk ditampilkan panel.
#[derive(Debug, Clone, Serialize)]
pub struct ActionPlanView {
    pub token_id: String,
    pub action: crate::exec::aksi::JenisAksi,
    pub nama: &'static str,
    pub gates: Vec<crate::exec::pipeline::GateResult>,
    pub gas_estimate: u64,
    pub dry_run: bool,
    /// Benar kalau aksi ini akan menghabiskan posisinya.
    pub menutup: bool,
    /// Benar hanya kalau seluruh gate lolos dan bukan dry run.
    pub siap: bool,
    pub expires_at: String,
    /// Fee yang akan diambil, dalam satuan terkecil.
    pub fee0: String,
    pub fee1: String,
}

/// Hasil satu pengiriman, apa pun jenisnya.
#[derive(Debug, Clone, Serialize)]
pub struct SendResultView {
    /// `approve`, `zap`, `open`, atau `action`, supaya panel tahu kartu mana
    pub kind: &'static str,
    #[serde(flatten)]
    pub outcome: SendOutcome,
}

/// Baca saldo dua currency sekaligus.
///
/// KENAPA TIDAK CUKUP `balanceOf` DUA KALI
/// Di V4, currency `0x0000...0000` berarti ETH asli, bukan alamat kosong. Ia
/// bukan kontrak dan tidak punya `balanceOf`, jadi saldonya harus dibaca lewat
/// `eth_getBalance`. Memanggil `balanceOf` ke alamat nol hanya menghasilkan
/// kegagalan, dan kegagalan itu akan terlihat seperti tokennya bermasalah.
///
/// FIG di rantai ini punya lima pool berpasangan ETH asli, jadi ini bukan
/// kasus pinggiran.
///
/// Kegagalan TIDAK diubah menjadi nol. Saldo nol palsu membuat gate pembatas
/// menolak transaksi yang sebenarnya sah, dan pesannya menyesatkan. Ini
/// kesalahan yang sudah pernah terjadi, tercatat di memory/003.
pub async fn saldo_dua_token<P: Provider>(
    provider: &P,
    multicall: Address,
    pemilik: Address,
    token0: Address,
    token1: Address,
) -> Result<(U256, U256), String> {
    use crate::exec::abi_write::IERC20Write;

    // ETH asli dibaca terpisah, dan hanya kalau memang ikut.
    let saldo_native = if token0.is_zero() || token1.is_zero() {
        Some(
            provider
                .get_balance(pemilik)
                .await
                .map_err(|e| format!("gagal membaca saldo ETH: {e}"))?,
        )
    } else {
        None
    };

    let mut calls = Vec::new();
    let mut ikut: Vec<Address> = Vec::new();
    for t in [token0, token1] {
        if t.is_zero() {
            continue;
        }
        calls.push(crate::chain::multicall::encode(
            t,
            &IERC20Write::balanceOfCall { account: pemilik },
        ));
        ikut.push(t);
    }

    let hasil = if calls.is_empty() {
        Vec::new()
    } else {
        crate::chain::multicall::batch_call(provider, multicall, calls)
            .await
            .map_err(|e| format!("gagal membaca saldo: {e}"))?
    };

    let mut per_token: std::collections::HashMap<Address, U256> = Default::default();
    for (i, t) in ikut.into_iter().enumerate() {
        let v = crate::chain::multicall::decode::<IERC20Write::balanceOfCall>(
            hasil.get(i).and_then(|x| x.as_ref()),
        )
        .ok_or_else(|| format!("{t} tidak menjawab balanceOf()"))?;
        per_token.insert(t, v);
    }

    let ambil = |t: Address| -> U256 {
        if t.is_zero() {
            saldo_native.unwrap_or(U256::ZERO)
        } else {
            per_token.get(&t).copied().unwrap_or(U256::ZERO)
        }
    };

    Ok((ambil(token0), ambil(token1)))
}
/// Ubah nominal USD menjadi jumlah token, memakai kurs yang dikalibrasi chart.
///
/// Mengembalikan `None` kalau salah satu masukannya tidak masuk akal. Nilai
/// yang tidak masuk akal di sini akan menjadi jumlah token yang dikirim ke
/// rantai, jadi lebih baik menolak daripada menebak.
pub fn usd_ke_satuan_terkecil(usd: f64, harga_usd_per_token: f64, decimals: u8) -> Option<U256> {
    if !(usd.is_finite() && usd > 0.0) {
        return None;
    }
    if !(harga_usd_per_token.is_finite() && harga_usd_per_token > 0.0) {
        return None;
    }
    let jumlah = usd / harga_usd_per_token;
    if !(jumlah.is_finite() && jumlah > 0.0) {
        return None;
    }
    // Jumlah token sebesar ini bukan ukuran posisi yang wajar, dan f64 sudah
    // kehilangan presisi bilangan bulat di atas 2^53. Ditolak alih alih
    // dibulatkan diam diam, karena hasilnya akan menjadi jumlah token yang
    // benar benar dikirim ke rantai.
    if jumlah >= 9.0e15 {
        return None;
    }

    // Penskalaan dilakukan dengan bilangan bulat, bukan dengan mengalikan f64
    // dengan 10^18. Perkalian f64 itu melewati batas presisi dan menghasilkan
    // angka yang meleset di digit terakhir, dan digit terakhir pada 18 desimal
    // tetap uang.
    let bulat = jumlah.trunc() as u128;
    let pecahan = (jumlah.fract() * 10f64.powi(PRESISI_PECAHAN as i32)).round() as u128;

    let sepuluh = U256::from(10u8);
    let mut out = U256::from(bulat) * sepuluh.pow(U256::from(decimals));

    let d = decimals as u32;
    out += if d >= PRESISI_PECAHAN {
        U256::from(pecahan) * sepuluh.pow(U256::from(d - PRESISI_PECAHAN))
    } else {
        // Token berdesimal sedikit tidak bisa menampung seluruh pecahan, dan
        // sisanya memang hilang. Itu keadaan rantai, bukan kesalahan kita.
        U256::from(pecahan / 10u128.pow(PRESISI_PECAHAN - d))
    };

    (out > U256::ZERO).then_some(out)
}

/// Berapa digit pecahan yang dipertahankan saat mengubah jumlah token menjadi
/// satuan terkecil.
///
/// Sembilan digit jauh melampaui ketelitian harga yang dibaca dari chart, dan
/// tetap aman di dalam presisi bilangan bulat f64.
const PRESISI_PECAHAN: u32 = 9;

/// Urutkan kedua batas range menjadi tick yang menaik.
///
/// KENAPA TIDAK BISA DIPAKAI APA ADANYA
/// `RangeView` memakai orientasi TAMPILAN: `lower` adalah harga yang lebih
/// rendah bagi user. Kalau memecoin menempati token1, tick yang lebih besar
/// justru berarti harga memecoin lebih rendah, sehingga `lower.tick` menjadi
/// yang lebih BESAR.
///
/// MintParams menuntut `tickLower < tickUpper` dalam ruang tick, bukan dalam
/// ruang harga. Memakai nama `lower` dan `upper` apa adanya menolak setiap
/// posisi pada pool yang memecoinnya token1, yaitu kira kira separuh dari
/// semua pool, dengan pesan yang terdengar seperti kesalahan user.
pub fn tick_menaik(a: i32, b: i32) -> Option<(i32, i32)> {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    (lo < hi).then_some((lo, hi))
}

/// Susun `OpenRequest` dari keadaan yang sedang aktif.
#[allow(clippy::too_many_arguments)]
pub fn susun_permintaan(
    pool: &crate::pools::PoolInfo,
    tick_lower: i32,
    tick_upper: i32,
    available0: U256,
    available1: U256,
    recipient: Address,
    position_usd: f64,
    eth_balance: f64,
    eth_needed: f64,
    deadline_detik: u64,
) -> OpenRequest {
    OpenRequest {
        pool: pool.clone(),
        tick_lower,
        tick_upper,
        available0,
        available1,
        recipient,
        deadline: U256::from(
            chrono::Utc::now().timestamp() as u64 + deadline_detik,
        ),
        position_usd,
        // Penghitung harian dan jumlah posisi terbuka menyusul di fase 5, saat
        // posisi benar benar tercatat. Sampai itu ada, keduanya nol, dan gate
        // G6 hanya menegakkan batas per posisi. Ini dicatat supaya tidak
        // terlihat seperti sudah selesai.
        daily_spent_usd: 0.0,
        open_positions: 0,
        is_new_pool: false,
        eth_balance,
        eth_needed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saldo_eth_asli_disisakan_untuk_gas_bukan_dipakai_habis() {
        // Bukan uji jaringan, hanya penegasan aturan: kalau salah satu
        // currency adalah ETH asli, seluruh saldo ETH tidak boleh dipakai,
        // karena gas transaksinya juga dibayar dari sana. Gate G6 yang
        // menegakkannya lewat min_eth_reserve, dan uji ini menandai bahwa
        // hubungan itu disengaja.
        let cfg = crate::config::LimitsConfig::default();
        assert!(
            cfg.min_eth_reserve > 0.0,
            "cadangan ETH wajib bukan nol, kalau tidak wallet bisa kehabisan gas"
        );
    }

    #[test]
    fn konversi_usd_ke_satuan_terkecil() {
        // 100 dolar pada harga 2 dolar per token, 18 desimal.
        let hasil = usd_ke_satuan_terkecil(100.0, 2.0, 18).unwrap();
        assert_eq!(hasil, U256::from(50_000_000_000_000_000_000u128));
    }

    #[test]
    fn desimal_enam_tidak_diperlakukan_seperti_delapan_belas() {
        // USDG memakai 6 desimal. Salah di sini menggeser ukuran posisi
        // sepuluh pangkat dua belas kali.
        let hasil = usd_ke_satuan_terkecil(100.0, 1.0, 6).unwrap();
        assert_eq!(hasil, U256::from(100_000_000u128));
    }

    #[test]
    fn pecahan_dipertahankan() {
        // 1 dolar pada harga 3 dolar per token, 18 desimal.
        let hasil = usd_ke_satuan_terkecil(1.0, 3.0, 18).unwrap();
        let harap = U256::from(333_333_333u64) * U256::from(10u64).pow(U256::from(9u64));
        assert_eq!(hasil, harap);
    }

    #[test]
    fn pecahan_yang_tidak_muat_di_desimal_kecil_hilang_bukan_meluap() {
        // Token berdesimal 2 tidak bisa menampung 0,001 token. Yang penting
        // hasilnya bukan angka besar yang salah.
        let hasil = usd_ke_satuan_terkecil(0.001, 1.0, 2).unwrap_or(U256::ZERO);
        assert_eq!(hasil, U256::ZERO);
    }

    #[test]
    fn masukan_tidak_masuk_akal_ditolak() {
        assert!(usd_ke_satuan_terkecil(0.0, 1.0, 18).is_none());
        assert!(usd_ke_satuan_terkecil(-5.0, 1.0, 18).is_none());
        assert!(usd_ke_satuan_terkecil(f64::NAN, 1.0, 18).is_none());
        assert!(usd_ke_satuan_terkecil(100.0, 0.0, 18).is_none(), "harga nol");
        assert!(usd_ke_satuan_terkecil(100.0, f64::INFINITY, 18).is_none());
    }

    #[test]
    fn nilai_di_luar_presisi_f64_ditolak_bukan_dibulatkan() {
        // Pembulatan diam diam di sini akan menjadi jumlah token yang salah
        // pada transaksi sungguhan.
        assert!(usd_ke_satuan_terkecil(1e12, 1e-9, 18).is_none());
    }

    #[test]
    fn tick_menaik_membiarkan_urutan_yang_sudah_benar() {
        assert_eq!(tick_menaik(-600, 600), Some((-600, 600)));
    }

    #[test]
    fn tick_menaik_menukar_saat_memecoin_token1() {
        // Pada pool yang memecoinnya token1, batas bawah menurut tampilan
        // justru tick yang lebih besar. Tanpa penukaran, setiap posisi pada
        // pool seperti itu ditolak dengan pesan yang menyalahkan user.
        assert_eq!(tick_menaik(114800, 111000), Some((111000, 114800)));
    }

    #[test]
    fn tick_yang_sama_ditolak() {
        // Range selebar nol tidak punya likuiditas, dan kontraknya menolak.
        assert_eq!(tick_menaik(200, 200), None);
    }

    #[test]
    fn rencana_kedaluwarsa_setelah_batas() {
        let plan = RencanaTersimpan {
            plan: contoh_plan(),
            zap_calldata: None,
            versi: crate::pools::PoolVersion::V3,
            token0: Address::ZERO,
            token1: Address::ZERO,
            amount0: U256::ZERO,
            amount1: U256::ZERO,
            disusun_pada: chrono::Utc::now()
                - chrono::Duration::seconds(RENCANA_KEDALUWARSA_DETIK + 1),
        };
        assert!(plan.kedaluwarsa());
    }

    #[test]
    fn rencana_baru_belum_kedaluwarsa() {
        let plan = RencanaTersimpan {
            plan: contoh_plan(),
            zap_calldata: None,
            versi: crate::pools::PoolVersion::V3,
            token0: Address::ZERO,
            token1: Address::ZERO,
            amount0: U256::ZERO,
            amount1: U256::ZERO,
            disusun_pada: chrono::Utc::now(),
        };
        assert!(!plan.kedaluwarsa());
    }

    fn contoh_plan() -> OpenPlan {
        OpenPlan {
            pool_id: "0x1".into(),
            tick_lower: -60,
            tick_upper: 60,
            amount0: "0".into(),
            amount1: "0".into(),
            liquidity: "0".into(),
            token_id_simulasi: "0".into(),
            gas_estimate: 0,
            dry_run: true,
            gates: Vec::new(),
            warnings: Vec::new(),
            calldata: None,
            request: None,
            v4_request: None,
        }
    }
}
