//! Pengiriman transaksi, satu satunya tempat dana benar benar bergerak.
//!
//! INVARIAN
//! - Hanya rencana yang `is_sendable()` yang boleh masuk ke sini, dan itu
//!   diperiksa lagi di dalam fungsi ini, bukan dipercayakan pada pemanggil.
//!   Pemeriksaan ganda di satu tempat yang sempit jauh lebih murah daripada
//!   satu jalur lolos yang memindahkan uang.
//! - Chain id diperiksa terhadap `chain.toml` sebelum menandatangani. Transaksi
//!   yang ditandatangani untuk chain id yang salah bisa diputar ulang di rantai
//!   lain, dan tanda tangannya tetap sah selamanya.
//! - Nonce dipesan lewat `NonceKeeper`, dan dilepas kalau pengirimannya gagal.
//!   Nonce yang bocor membuat seluruh transaksi berikutnya menggantung.
//! - Receipt sukses BELUM berarti berhasil. Gate G7 membaca ulang posisinya
//!   dari rantai dan membandingkannya dengan yang diminta.
//! - Tidak ada percobaan ulang otomatis. Transaksi yang gagal karena alasan
//!   yang tidak dimengerti tidak boleh dikirim ulang oleh mesin, karena dua
//!   transaksi yang sama artinya dua posisi.

use std::time::Duration;

use alloy::network::{EthereumWallet, NetworkTransactionBuilder, TransactionBuilder};
use alloy::primitives::{Address, Bytes, TxHash, U256};
use alloy::providers::Provider;
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use serde::Serialize;

use super::nonce::NonceKeeper;
use super::pipeline::{GateResult, OpenPlan};
use super::verify;

/// Kelebihan gas di atas hasil estimasi, dalam persen.
///
/// Estimasi dijalankan pada keadaan rantai saat ini, sedangkan transaksi
/// dieksekusi satu atau beberapa blok kemudian. Keadaan yang berubah bisa
/// membuat eksekusi sedikit lebih mahal, misalnya karena slot penyimpanan yang
/// tadinya sudah panas menjadi dingin. Dua puluh persen adalah margin yang
/// lazim dipakai dompet dan cukup untuk pergeseran seperti itu.
pub const GAS_MARGIN_PERSEN: u64 = 20;

/// Batas atas gas limit, apa pun kata estimasi.
///
/// Sebuah `mint` V3 di rantai ini memakai sekitar 400 ribu gas. Estimasi yang
/// mengembalikan angka jauh di atas ini berarti ada yang tidak beres, dan
/// menandatanganinya berarti memberi izin membakar gas sebanyak itu.
pub const GAS_LIMIT_MAKSIMUM: u64 = 5_000_000;

/// Berapa lama menunggu receipt sebelum menyerah.
///
/// Waktu blok rantai ini sekitar 100 ms, jadi enam puluh detik adalah enam
/// ratus blok. Kalau selama itu transaksinya belum masuk, ada yang salah, dan
/// menunggu lebih lama tidak akan memperbaikinya.
pub const TUNGGU_RECEIPT_DETIK: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TxStatus {
    /// Masuk blok dan tidak revert.
    Sukses,
    /// Masuk blok tapi revert. Gas tetap terbakar.
    Revert,
    /// Tidak masuk blok dalam batas waktu. Belum tentu gagal, hanya belum
    /// diketahui, dan itu perbedaan yang penting.
    Menggantung,
}

/// Hasil satu pengiriman, lengkap dengan bukti apa yang terjadi.
#[derive(Debug, Clone, Serialize)]
pub struct SendOutcome {
    pub tx_hash: String,
    pub nonce: u64,
    pub gas_limit: u64,
    pub status: TxStatus,
    pub block_number: Option<u64>,
    pub gas_used: Option<u64>,
    /// Token id posisi yang tercipta, dibaca dari log, bukan dari simulasi.
    pub token_id: Option<String>,
    /// Gate G7. Kosong kalau transaksinya tidak sampai berhasil.
    pub verification: Option<GateResult>,
    pub sent_at: String,
}

impl SendOutcome {
    /// Benar hanya kalau transaksinya masuk blok, tidak revert, dan posisinya
    /// terbukti sama dengan yang diminta.
    pub fn fully_verified(&self) -> bool {
        self.status == TxStatus::Sukses
            && self.verification.as_ref().is_some_and(|g| g.passed)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SendError {
    #[error("rencana ini tidak boleh dikirim: {0}")]
    TidakBolehDikirim(String),
    #[error("chain id tidak cocok: rantai menjawab {rantai}, chain.toml menyebut {diharapkan}")]
    ChainIdSalah { rantai: u64, diharapkan: u64 },
    #[error("estimasi gas gagal: {0}")]
    EstimasiGagal(String),
    #[error("estimasi gas {0} melebihi batas {GAS_LIMIT_MAKSIMUM}, ada yang tidak beres")]
    GasTerlaluBesar(u64),
    #[error("gagal menandatangani: {0}")]
    TandaTangan(String),
    #[error("rantai menolak transaksi: {0}")]
    Ditolak(String),
    #[error("gagal membaca keadaan rantai: {0}")]
    Rpc(String),
}

/// Kirim rencana yang sudah lolos seluruh gate.
///
/// Urutannya disengaja: yang bisa gagal tanpa biaya dikerjakan lebih dulu, dan
/// nonce baru dipesan setelah semua pemeriksaan lolos, supaya nonce tidak bocor
/// karena kesalahan yang seharusnya ketahuan lebih awal.
#[allow(clippy::too_many_arguments)]
pub async fn send_open<P: Provider>(
    provider: &P,
    keeper: &NonceKeeper,
    signer: &PrivateKeySigner,
    position_manager: Address,
    plan: &OpenPlan,
    chain_id_diharapkan: u64,
    value: U256,
) -> Result<SendOutcome, SendError> {
    // 1. Rencananya sendiri. Diperiksa ulang di sini, bukan dipercayakan.
    if !plan.is_sendable() {
        let alasan = if plan.dry_run {
            "engine sedang dry run".to_string()
        } else if plan.calldata.is_none() {
            "rencana tidak punya calldata".to_string()
        } else {
            let gagal: Vec<&str> = plan.failed_gates().iter().map(|g| g.id).collect();
            format!("gate belum lolos: {}", gagal.join(", "))
        };
        return Err(SendError::TidakBolehDikirim(alasan));
    }
    let calldata: Bytes = plan
        .calldata
        .clone()
        .ok_or_else(|| SendError::TidakBolehDikirim("calldata kosong".into()))?;

    // 2. Seluruh penjaga pengiriman ada di satu tempat: chain id, estimasi
    //    gas beserta batas atasnya, pemesanan nonce, dan pelepasannya kalau
    //    gagal. Menyalinnya ke sini akan membuat salah satu tertinggal saat
    //    salah satunya berubah.
    let terkirim = send_calldata(
        provider,
        keeper,
        signer,
        position_manager,
        calldata,
        value,
        chain_id_diharapkan,
    )
    .await?;

    let tx_hash = terkirim.tx_hash;
    let receipt = terkirim.receipt;
    let gas_limit = terkirim.gas_limit;
    let reservasi_nonce = terkirim.nonce;
    let sent_at = chrono::Utc::now().to_rfc3339();

    let Some(receipt) = receipt else {
        // Nonce TIDAK dilepas. Transaksinya sudah ada di rantai, hanya belum
        // terlihat masuk blok, dan melepasnya akan membuat pengiriman
        // berikutnya bertabrakan dengan yang ini.
        tracing::warn!(
            tx = %tx_hash,
            nonce = reservasi_nonce,
            "receipt tidak datang dalam batas waktu, status transaksi belum diketahui"
        );
        return Ok(SendOutcome {
            tx_hash: format!("{tx_hash:#x}"),
            nonce: reservasi_nonce,
            gas_limit,
            status: TxStatus::Menggantung,
            block_number: None,
            gas_used: None,
            token_id: None,
            verification: None,
            sent_at,
        });
    };

    let sukses = receipt.status();
    let block_number = receipt.block_number;
    let gas_used = Some(receipt.gas_used);

    if !sukses {
        return Ok(SendOutcome {
            tx_hash: format!("{tx_hash:#x}"),
            nonce: reservasi_nonce,
            gas_limit,
            status: TxStatus::Revert,
            block_number,
            gas_used,
            token_id: None,
            verification: None,
            sent_at,
        });
    }

    // 5. Gate G7. Token id diambil dari log, bukan dari simulasi, karena yang
    //    kita periksa justru apakah kenyataan sama dengan perkiraan.
    let token_id = token_id_dari_log(&receipt);
    let verification = match (token_id, plan.request.as_ref()) {
        (Some(id), Some(req)) => {
            let harapan: u128 = plan.liquidity.parse().unwrap_or(0);
            Some(verifikasi_posisi(provider, position_manager, id, req, harapan).await)
        }
        (None, _) => Some(GateResult::gagal_publik(
            "G7",
            "verifikasi pasca kirim",
            "transaksi sukses tapi tidak ada event IncreaseLiquidity, token id tidak diketahui",
        )),
        (_, None) => Some(GateResult::gagal_publik(
            "G7",
            "verifikasi pasca kirim",
            "rencana tidak menyimpan permintaan aslinya, tidak ada yang bisa dibandingkan",
        )),
    };

    Ok(SendOutcome {
        tx_hash: format!("{tx_hash:#x}"),
        nonce: reservasi_nonce,
        gas_limit,
        status: TxStatus::Sukses,
        block_number,
        gas_used,
        token_id: token_id.map(|t| t.to_string()),
        verification,
        sent_at,
    })
}

/// Kirim satu transaksi apa pun dengan seluruh penjaga yang sama.
///
/// Dipakai `approve` maupun `mint`. Menyalin urutan penjaga ini ke tempat
/// kedua adalah cara paling mudah kehilangan salah satunya, dan yang hilang
/// biasanya baru ketahuan saat sudah ada uang di dalamnya.
pub async fn send_calldata<P: Provider>(
    provider: &P,
    keeper: &NonceKeeper,
    signer: &PrivateKeySigner,
    to: Address,
    calldata: Bytes,
    value: U256,
    chain_id_diharapkan: u64,
) -> Result<Terkirim, SendError> {
    // Chain id lebih dulu. Tanda tangan untuk chain id yang salah berlaku
    // selamanya di rantai lain, jadi ini diperiksa sebelum kunci privat
    // menyentuh apa pun.
    let chain_id = provider
        .get_chain_id()
        .await
        .map_err(|e| SendError::Rpc(e.to_string()))?;
    if chain_id != chain_id_diharapkan {
        return Err(SendError::ChainIdSalah {
            rantai: chain_id,
            diharapkan: chain_id_diharapkan,
        });
    }

    let from = signer.address();
    let dasar = TransactionRequest::default()
        .with_from(from)
        .with_to(to)
        .with_input(calldata)
        .with_value(value)
        .with_chain_id(chain_id);

    // Estimasi dijalankan pada transaksi yang persis sama dengan yang akan
    // dikirim, bukan pada bentuk yang mirip.
    let estimasi = provider.estimate_gas(dasar.clone()).await.map_err(|e| {
        SendError::EstimasiGagal(super::simulate::jelaskan_revert(&e.to_string()))
    })?;

    let gas_limit = estimasi.saturating_mul(100 + GAS_MARGIN_PERSEN) / 100;
    if gas_limit > GAS_LIMIT_MAKSIMUM {
        return Err(SendError::GasTerlaluBesar(gas_limit));
    }

    // Nonce dipesan paling akhir supaya tidak bocor karena kegagalan di atas.
    // Mulai dari sini setiap jalur keluar wajib melepasnya.
    let reservasi = keeper
        .reserve(provider, from)
        .await
        .map_err(|e| SendError::Rpc(e.to_string()))?;

    let hasil = kirim_dan_tunggu(
        provider,
        signer,
        dasar.with_nonce(reservasi.nonce).with_gas_limit(gas_limit),
        chain_id,
    )
    .await;

    match hasil {
        Ok((tx_hash, receipt)) => Ok(Terkirim {
            tx_hash,
            nonce: reservasi.nonce,
            gas_limit,
            receipt,
        }),
        Err(e) => {
            // Transaksinya tidak pernah sampai rantai, jadi nonce ini masih
            // bebas. Tanpa pelepasan, transaksi berikutnya memakai nonce yang
            // lebih tinggi dan menggantung menunggu yang tidak akan datang.
            keeper.release(reservasi).await;
            if matches!(e, SendError::Ditolak(_)) {
                // Rantai menolak berarti anggapan lokal soal nonce sudah tidak
                // nyambung dengan kenyataan. Baca ulang dari rantai lain kali.
                keeper.reset().await;
            }
            Err(e)
        }
    }
}

/// Transaksi yang sudah masuk rantai, sebelum diartikan.
pub struct Terkirim {
    pub tx_hash: TxHash,
    pub nonce: u64,
    pub gas_limit: u64,
    /// Kosong berarti receipt belum datang dalam batas waktu, BUKAN gagal.
    pub receipt: Option<alloy::rpc::types::TransactionReceipt>,
}

/// Kirim `approve` ERC20, lalu buktikan izinnya benar benar naik.
///
/// Sebagian token mengembalikan `false` alih alih revert saat approve gagal,
/// jadi receipt sukses saja tidak cukup. Allowance dibaca ulang setelahnya.
#[allow(clippy::too_many_arguments)]
pub async fn send_approve<P: Provider>(
    provider: &P,
    keeper: &NonceKeeper,
    signer: &PrivateKeySigner,
    multicall: Address,
    token: Address,
    spender: Address,
    amount: U256,
    chain_id_diharapkan: u64,
) -> Result<SendOutcome, SendError> {
    let calldata = super::approve::approve_calldata(spender, amount);
    let terkirim = send_calldata(
        provider,
        keeper,
        signer,
        token,
        calldata,
        U256::ZERO,
        chain_id_diharapkan,
    )
    .await?;

    let sent_at = chrono::Utc::now().to_rfc3339();
    let tx_hash = format!("{:#x}", terkirim.tx_hash);

    let Some(receipt) = terkirim.receipt else {
        return Ok(SendOutcome {
            tx_hash,
            nonce: terkirim.nonce,
            gas_limit: terkirim.gas_limit,
            status: TxStatus::Menggantung,
            block_number: None,
            gas_used: None,
            token_id: None,
            verification: None,
            sent_at,
        });
    };

    let status = if receipt.status() {
        TxStatus::Sukses
    } else {
        TxStatus::Revert
    };

    // Bukti, bukan asumsi. Izin dibaca ulang dari rantai.
    let verification = if status == TxStatus::Sukses {
        let owner = signer.address();
        match super::approve::check_allowances(
            provider, multicall, owner, spender, token, amount, token, U256::ZERO,
        )
        .await
        {
            Ok(n) if !n[0].insufficient => Some(GateResult::lolos_publik(
                "G7",
                "verifikasi izin",
                format!("izin {token} sekarang {}", n[0].current),
            )),
            Ok(n) => Some(GateResult::gagal_publik(
                "G7",
                "verifikasi izin",
                format!(
                    "approve sukses tapi izin masih {} sedangkan yang dibutuhkan {}",
                    n[0].current, n[0].needed
                ),
            )),
            Err(e) => Some(GateResult::gagal_publik("G7", "verifikasi izin", e)),
        }
    } else {
        None
    };

    Ok(SendOutcome {
        tx_hash,
        nonce: terkirim.nonce,
        gas_limit: terkirim.gas_limit,
        status,
        block_number: receipt.block_number,
        gas_used: Some(receipt.gas_used),
        token_id: None,
        verification,
        sent_at,
    })
}

/// Tanda tangani, kirim, lalu tunggu receipt sampai batas waktu.
///
/// Batas waktu yang habis BUKAN kegagalan pengiriman. Transaksinya sudah ada di
/// rantai, hanya belum terlihat masuk blok, jadi `None` dikembalikan alih alih
/// error, dan pemanggil yang memutuskan artinya.
async fn kirim_dan_tunggu<P: Provider>(
    provider: &P,
    signer: &PrivateKeySigner,
    tx: TransactionRequest,
    chain_id: u64,
) -> Result<(TxHash, Option<alloy::rpc::types::TransactionReceipt>), SendError> {
    let wallet = EthereumWallet::from(signer.clone());
    let amplop = tx
        .build(&wallet)
        .await
        .map_err(|e| SendError::TandaTangan(e.to_string()))?;

    let pending = provider
        .send_tx_envelope(amplop)
        .await
        .map_err(|e| SendError::Ditolak(e.to_string()))?;

    let tx_hash = *pending.tx_hash();
    tracing::info!(tx = %tx_hash, chain_id, "transaksi terkirim");

    let receipt = match tokio::time::timeout(
        Duration::from_secs(TUNGGU_RECEIPT_DETIK),
        pending.get_receipt(),
    )
    .await
    {
        Ok(Ok(r)) => Some(r),
        Ok(Err(e)) => {
            tracing::warn!(tx = %tx_hash, error = %e, "gagal mengambil receipt");
            None
        }
        Err(_) => None,
    };

    Ok((tx_hash, receipt))
}

/// Ambil token id posisi dari event `IncreaseLiquidity`.
///
/// KENAPA DARI SINI, BUKAN DARI TRANSFER
/// `Transfer` juga membawa token id dan terbit lebih dulu, tapi ia terbit untuk
/// setiap NFT yang berpindah. `IncreaseLiquidity` terbit persis sekali per
/// penambahan likuiditas, jadi ia yang benar benar menandai posisi kita.
fn token_id_dari_log(receipt: &alloy::rpc::types::TransactionReceipt) -> Option<U256> {
    use super::abi_write::IncreaseLiquidity;
    use alloy::sol_types::SolEvent;

    receipt.inner.logs().iter().find_map(|log| {
        IncreaseLiquidity::decode_log(&log.inner)
            .ok()
            .map(|e| e.tokenId)
    })
}

/// Jalankan gate G7 dan bungkus hasilnya sebagai satu baris yang bisa
/// ditampilkan apa adanya di panel.
async fn verifikasi_posisi<P: Provider>(
    provider: &P,
    position_manager: Address,
    token_id: U256,
    req: &super::calldata::MintRequest,
    expected_liquidity: u128,
) -> GateResult {
    let posisi = match verify::read_position(provider, position_manager, token_id).await {
        Ok(p) => p,
        Err(e) => {
            return GateResult::gagal_publik("G7", "verifikasi pasca kirim", e);
        }
    };

    match verify::check_matches_request(&posisi, req, expected_liquidity) {
        Ok(()) => GateResult::lolos_publik(
            "G7",
            "verifikasi pasca kirim",
            format!("posisi {token_id} cocok dengan yang diminta"),
        ),
        Err(selisih) => GateResult::gagal_publik(
            "G7",
            "verifikasi pasca kirim",
            format!("posisi {token_id} berbeda dari yang diminta: {}", selisih.join("; ")),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn margin_gas_dua_puluh_persen() {
        let estimasi: u64 = 400_000;
        let limit = estimasi.saturating_mul(100 + GAS_MARGIN_PERSEN) / 100;
        assert_eq!(limit, 480_000);
    }

    #[test]
    fn margin_gas_tidak_meluap_pada_angka_ekstrem() {
        // Estimasi cacat dari node tidak boleh membungkus jadi angka kecil,
        // karena gas limit kecil membuat transaksi kehabisan gas dan tetap
        // membakar biaya.
        let limit = u64::MAX.saturating_mul(100 + GAS_MARGIN_PERSEN) / 100;
        assert!(limit > GAS_LIMIT_MAKSIMUM, "harus tertolak oleh batas atas");
    }

    #[test]
    fn hasil_terverifikasi_butuh_sukses_dan_g7_lolos() {
        let dasar = SendOutcome {
            tx_hash: "0x1".into(),
            nonce: 0,
            gas_limit: 1,
            status: TxStatus::Sukses,
            block_number: Some(1),
            gas_used: Some(1),
            token_id: Some("7".into()),
            verification: Some(GateResult::lolos_publik("G7", "verifikasi", "cocok")),
            sent_at: "2026-09-07T00:00:00Z".into(),
        };
        assert!(dasar.fully_verified());

        let mut g7_gagal = dasar.clone();
        g7_gagal.verification = Some(GateResult::gagal_publik("G7", "verifikasi", "beda"));
        assert!(!g7_gagal.fully_verified(), "G7 gagal berarti belum terbukti");

        let mut revert = dasar.clone();
        revert.status = TxStatus::Revert;
        assert!(!revert.fully_verified());

        let mut menggantung = dasar.clone();
        menggantung.status = TxStatus::Menggantung;
        menggantung.verification = None;
        assert!(
            !menggantung.fully_verified(),
            "belum diketahui bukan berarti berhasil"
        );
    }
}
