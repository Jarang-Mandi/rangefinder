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
//! - Nonce dipesan lewat `NonceKeeper`. Ia hanya dilepas bila transaksi pasti
//!   belum dibroadcast; hasil transport yang tidak pasti tetap menahan nonce
//!   sampai jurnal direkonsiliasi dengan rantai.
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

/// Jumlah yang dibuktikan event mint, dalam unit terkecil kedua token.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MintedAmounts {
    pub amount0: String,
    pub amount1: String,
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
    /// Biaya eksekusi aktual: gasUsed dikali effectiveGasPrice dari receipt.
    pub gas_cost_wei: Option<String>,
    /// Kosong jika jumlah aktual belum dapat dibuktikan. Jangan isi dari plan.
    pub minted_amounts: Option<MintedAmounts>,
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
        self.status == TxStatus::Sukses && self.verification.as_ref().is_some_and(|g| g.passed)
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
    #[error("jurnal transaksi belum dapat ditulis: {0}")]
    Jurnal(String),
    #[error("gagal membaca keadaan rantai: {0}")]
    Rpc(String),
}

fn periksa_mode(execution: &crate::config::ExecutionConfig) -> Result<(), SendError> {
    if execution.dry_run {
        return Err(SendError::TidakBolehDikirim("engine sedang dry run".into()));
    }
    Ok(())
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
    multicall: Address,
    plan: &OpenPlan,
    chain_id_diharapkan: u64,
    value: U256,
    execution: &crate::config::ExecutionConfig,
    limits: &crate::config::LimitsConfig,
    journal: &super::journal::Journal<'_>,
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
        execution,
        limits,
        journal,
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
            gas_cost_wei: None,
            minted_amounts: None,
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
            gas_cost_wei: biaya_gas_wei(&receipt),
            minted_amounts: None,
            token_id: None,
            verification: None,
            sent_at,
        });
    }

    // 5. Gate G7. Token id diambil dari log, bukan dari simulasi, karena yang
    //    kita periksa justru apakah kenyataan sama dengan perkiraan.
    //
    //    Cara membacanya berbeda per protokol. V3 memancarkan
    //    `IncreaseLiquidity` di PositionManager. V4 tidak, jadi penandanya
    //    adalah `Transfer` ERC721 dari alamat nol.
    let (token_id, verification) = if let Some(v4) = plan.v4_request.as_ref() {
        let id = crate::exec::v4::pipeline::token_id_dari_receipt(&receipt, position_manager);
        let gate = match id {
            Some(id) => {
                match crate::exec::v4::verify::baca_posisi(
                    provider,
                    position_manager,
                    multicall,
                    id,
                )
                .await
                {
                    Ok(posisi) => match crate::exec::v4::verify::cocok_dengan_permintaan(
                        &posisi, v4,
                    ) {
                        Ok(()) => GateResult::lolos_publik(
                            "G7",
                            "verifikasi pasca kirim",
                            format!("posisi V4 {id} cocok dengan yang diminta"),
                        ),
                        Err(selisih) => GateResult::gagal_publik(
                            "G7",
                            "verifikasi pasca kirim",
                            format!(
                                "posisi V4 {id} berbeda dari yang diminta: {}",
                                selisih.join("; ")
                            ),
                        ),
                    },
                    Err(e) => {
                        GateResult::gagal_publik("G7", "verifikasi pasca kirim", e)
                    }
                }
            }
            None => GateResult::gagal_publik(
                "G7",
                "verifikasi pasca kirim",
                "transaksi sukses tapi tidak ada Transfer NFT dari alamat nol, token id tidak diketahui",
            ),
        };
        (id, Some(gate))
    } else {
        let id = mint_dari_log(receipt.inner.logs(), position_manager).map(|(id, _)| id);
        let gate = match (id, plan.request.as_ref()) {
            (Some(id), Some(req)) => {
                let harapan: u128 = plan.liquidity.parse().unwrap_or(0);
                verifikasi_posisi(provider, position_manager, id, req, harapan).await
            }
            (None, _) => GateResult::gagal_publik(
                "G7",
                "verifikasi pasca kirim",
                "transaksi sukses tapi tidak ada event IncreaseLiquidity, token id tidak diketahui",
            ),
            (_, None) => GateResult::gagal_publik(
                "G7",
                "verifikasi pasca kirim",
                "rencana tidak menyimpan permintaan aslinya, tidak ada yang bisa dibandingkan",
            ),
        };
        (id, Some(gate))
    };
    Ok(SendOutcome {
        tx_hash: format!("{tx_hash:#x}"),
        nonce: reservasi_nonce,
        gas_limit,
        status: TxStatus::Sukses,
        block_number,
        gas_used,
        gas_cost_wei: biaya_gas_wei(&receipt),
        minted_amounts: if plan.v4_request.is_none() {
            mint_dari_log(receipt.inner.logs(), position_manager).map(|(_, amounts)| amounts)
        } else {
            None
        },
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
#[allow(clippy::too_many_arguments)]
pub async fn send_calldata<P: Provider>(
    provider: &P,
    keeper: &NonceKeeper,
    signer: &PrivateKeySigner,
    to: Address,
    calldata: Bytes,
    value: U256,
    chain_id_diharapkan: u64,
    execution: &crate::config::ExecutionConfig,
    limits: &crate::config::LimitsConfig,
    journal: &super::journal::Journal<'_>,
) -> Result<Terkirim, SendError> {
    // Seluruh jenis transaksi melewati pintu ini, termasuk approve dan zap.
    periksa_mode(execution)?;
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

    // build(wallet) tidak menjalankan filler provider. Fee harus sudah lengkap
    // sebelum request diberikan kepada signer, bukan setelah menjadi envelope.
    let fees = provider
        .estimate_eip1559_fees()
        .await
        .map_err(|e| SendError::Rpc(format!("estimasi biaya gas gagal: {e}")))?;
    let dasar = isi_biaya(dasar, fees.max_fee_per_gas, fees.max_priority_fee_per_gas)?;

    // Estimasi dijalankan pada transaksi yang persis sama dengan yang akan
    // dikirim, bukan pada bentuk yang mirip.
    let estimasi = provider
        .estimate_gas(dasar.clone())
        .await
        .map_err(|e| SendError::EstimasiGagal(super::simulate::jelaskan_revert(&e.to_string())))?;

    let gas_limit = estimasi.saturating_mul(100 + GAS_MARGIN_PERSEN) / 100;
    if gas_limit > GAS_LIMIT_MAKSIMUM {
        return Err(SendError::GasTerlaluBesar(gas_limit));
    }

    let saldo = provider
        .get_balance(from)
        .await
        .map_err(|e| SendError::Rpc(e.to_string()))?;
    periksa_cadangan(
        saldo,
        value,
        gas_limit,
        fees.max_fee_per_gas,
        limits.min_eth_reserve,
    )?;

    // Transaksi yang statusnya belum diketahui tetap menahan nonce setelah restart.
    let floor = super::journal::nonce_floor(journal.store, chain_id, from)
        .map_err(|e| SendError::Jurnal(e.to_string()))?;
    // Kesalahan sebelum broadcast melepas nonce; ketidakpastian setelahnya tidak.
    let reservasi = keeper
        .reserve_with_floor(provider, from, floor)
        .await
        .map_err(|e| SendError::Rpc(e.to_string()))?;

    let hasil = kirim_dan_tunggu(
        provider,
        signer,
        dasar.with_nonce(reservasi.nonce).with_gas_limit(gas_limit),
        chain_id,
        journal,
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
            // `kirim_dan_tunggu` mengembalikan error hanya sebelum broadcast.
            // Ketidakpastian transport sesudah broadcast dikembalikan sebagai
            // transaksi tanpa receipt, sehingga aman melepas nonce di cabang ini.
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
    execution: &crate::config::ExecutionConfig,
    limits: &crate::config::LimitsConfig,
    journal: &super::journal::Journal<'_>,
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
        execution,
        limits,
        journal,
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
            gas_cost_wei: None,
            minted_amounts: None,
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
            provider,
            multicall,
            owner,
            spender,
            token,
            amount,
            token,
            U256::ZERO,
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
        gas_cost_wei: biaya_gas_wei(&receipt),
        minted_amounts: None,
        token_id: None,
        verification,
        sent_at,
    })
}

/// Terjemahkan transaksi yang sudah terkirim menjadi hasil yang bisa ditampilkan.
///
/// Dipakai jalur yang tidak punya verifikasi khusus, seperti izin Permit2.
/// Statusnya tetap dibedakan tiga: sukses, revert, dan menggantung. Yang
/// terakhir bukan kegagalan, hanya belum diketahui, dan itu perbedaan penting.
pub fn jadikan_hasil(t: Terkirim) -> SendOutcome {
    let sent_at = chrono::Utc::now().to_rfc3339();
    let tx_hash = format!("{:#x}", t.tx_hash);

    match t.receipt {
        Some(r) => SendOutcome {
            tx_hash,
            nonce: t.nonce,
            gas_limit: t.gas_limit,
            status: if r.status() {
                TxStatus::Sukses
            } else {
                TxStatus::Revert
            },
            block_number: r.block_number,
            gas_used: Some(r.gas_used),
            gas_cost_wei: biaya_gas_wei(&r),
            minted_amounts: None,
            token_id: None,
            verification: None,
            sent_at,
        },
        None => SendOutcome {
            tx_hash,
            nonce: t.nonce,
            gas_limit: t.gas_limit,
            status: TxStatus::Menggantung,
            block_number: None,
            gas_used: None,
            gas_cost_wei: None,
            minted_amounts: None,
            token_id: None,
            verification: None,
            sent_at,
        },
    }
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
    journal: &super::journal::Journal<'_>,
) -> Result<(TxHash, Option<alloy::rpc::types::TransactionReceipt>), SendError> {
    let wallet = EthereumWallet::from(signer.clone());
    let amplop = tx
        .clone()
        .build(&wallet)
        .await
        .map_err(|e| SendError::TandaTangan(e.to_string()))?;

    let tx_hash = *amplop.tx_hash();
    let entry = journal
        .prepare(chain_id, signer.address(), tx_hash, tx)
        .map_err(|e| SendError::Jurnal(e.to_string()))?;
    let pending = match provider.send_tx_envelope(amplop).await {
        Ok(pending) => pending,
        Err(error) => {
            // RPC mungkin telah menerima transaksi sebelum koneksi terputus.
            // Simpan ketidakpastian dan pertahankan nonce; jangan kirim ulang.
            if let Err(e) =
                super::journal::broadcast_result(journal.store, &entry, Some(error.to_string()))
            {
                tracing::error!(tx = %tx_hash, error = %e, "status broadcast gagal disimpan; jurnal prepared tetap dapat direkonsiliasi");
            }
            tracing::warn!(tx = %tx_hash, error = %error, "status broadcast belum diketahui");
            return Ok((tx_hash, None));
        }
    };
    if let Err(e) = super::journal::broadcast_result(journal.store, &entry, None) {
        tracing::error!(tx = %tx_hash, error = %e, "status submitted gagal disimpan; jurnal prepared tetap ada");
    }

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

    if let Some(receipt) = receipt.as_ref() {
        if let Err(e) = super::journal::record_receipt(journal.store, &entry, receipt.clone()) {
            tracing::error!(tx = %tx_hash, error = %e, "receipt gagal disimpan; rekonsiliasi akan mencoba membacanya lagi");
        }
    }
    Ok((tx_hash, receipt))
}

fn isi_biaya(
    tx: TransactionRequest,
    maksimum: u128,
    prioritas: u128,
) -> Result<TransactionRequest, SendError> {
    if maksimum == 0 || prioritas > maksimum {
        return Err(SendError::Rpc("biaya gas dari RPC tidak valid".into()));
    }
    Ok(tx
        .with_max_fee_per_gas(maksimum)
        .with_max_priority_fee_per_gas(prioritas))
}

fn biaya_gas_wei(receipt: &alloy::rpc::types::TransactionReceipt) -> Option<String> {
    // Alloy memakai nol jika RPC menghilangkan effectiveGasPrice.
    if receipt.effective_gas_price == 0 || receipt.gas_used == 0 {
        return None;
    }
    // u64 dikali u128 selalu muat U256. Tidak lewat f64 atau gas limit.
    Some((U256::from(receipt.gas_used) * U256::from(receipt.effective_gas_price)).to_string())
}

/// Ambil token id dan setoran dari event `IncreaseLiquidity` milik manager.
///
/// KENAPA DARI SINI, BUKAN DARI TRANSFER
/// `Transfer` juga membawa token id dan terbit lebih dulu, tapi ia terbit untuk
/// setiap NFT yang berpindah. `IncreaseLiquidity` terbit persis sekali per
/// penambahan likuiditas, jadi ia yang benar benar menandai posisi kita.
pub(crate) fn mint_dari_log(
    logs: &[alloy::rpc::types::Log],
    manager: Address,
) -> Option<(U256, MintedAmounts)> {
    use super::abi_write::IncreaseLiquidity;
    use alloy::sol_types::SolEvent;

    let mut matches = logs
        .iter()
        .filter(|log| log.inner.address == manager)
        .filter_map(|log| {
            IncreaseLiquidity::decode_log(&log.inner).ok().map(|e| {
                (
                    e.tokenId,
                    MintedAmounts {
                        amount0: e.amount0.to_string(),
                        amount1: e.amount1.to_string(),
                    },
                )
            })
        });
    let minted = matches.next()?;
    // Jalur mint tunggal harus punya tepat satu event dari manager.
    matches.next().is_none().then_some(minted)
}

/// Ambil jumlah claim/close V3 dari event manager yang tepat.
pub(crate) fn collect_dari_log(
    logs: &[alloy::rpc::types::Log],
    manager: Address,
    token_id: U256,
    recipient: Address,
) -> Option<MintedAmounts> {
    use super::abi_write::Collect;
    use alloy::sol_types::SolEvent;

    let mut matches = logs
        .iter()
        .filter(|log| log.inner.address == manager)
        .filter_map(|log| Collect::decode_log(&log.inner).ok())
        .filter(|event| event.tokenId == token_id && event.recipient == recipient)
        .map(|event| MintedAmounts {
            amount0: event.amount0.to_string(),
            amount1: event.amount1.to_string(),
        });
    let collected = matches.next()?;
    matches.next().is_none().then_some(collected)
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
            format!(
                "posisi {token_id} berbeda dari yang diminta: {}",
                selisih.join("; ")
            ),
        ),
    }
}

fn periksa_cadangan(
    saldo: U256,
    value: U256,
    gas: u64,
    fee: u128,
    reserve: f64,
) -> Result<(), SendError> {
    if !reserve.is_finite() || reserve < 0.0 {
        return Err(SendError::TidakBolehDikirim(
            "cadangan gas tidak valid".into(),
        ));
    }
    let cadangan = if reserve == 0.0 {
        U256::ZERO
    } else {
        crate::api::deposit::units(&format!("{reserve:.18}"), 18)
            .map_err(SendError::TidakBolehDikirim)?
    };
    let biaya = U256::from(gas) * U256::from(fee);
    let perlu = value
        .checked_add(biaya)
        .and_then(|v| v.checked_add(cadangan))
        .ok_or_else(|| SendError::TidakBolehDikirim("jumlah ETH melampaui uint256".into()))?;
    if saldo < perlu {
        return Err(SendError::TidakBolehDikirim(format!("saldo ETH {saldo} wei tidak cukup untuk value {value}, gas maksimum {biaya}, dan cadangan {cadangan}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_mint_dari_kontrak_lain_dan_event_ambigu_tidak_dipercaya() {
        use alloy::sol_types::SolEvent;
        let manager = Address::repeat_byte(1);
        let event = super::super::abi_write::IncreaseLiquidity {
            tokenId: U256::from(7),
            liquidity: 100,
            amount0: U256::from(91),
            amount1: U256::from(82),
        };
        let log = |address| alloy::rpc::types::Log {
            inner: alloy::primitives::Log {
                address,
                data: event.encode_log_data(),
            },
            ..Default::default()
        };
        let spoof = log(Address::repeat_byte(2));
        let benar = log(manager);
        let (id, amounts) = mint_dari_log(&[spoof.clone(), benar.clone()], manager).unwrap();
        assert_eq!(id, U256::from(7));
        assert_eq!(amounts.amount0, "91");
        assert_eq!(amounts.amount1, "82");
        assert!(mint_dari_log(&[spoof], manager).is_none());
        assert!(mint_dari_log(&[benar.clone(), benar], manager).is_none());
    }

    #[test]
    fn event_collect_harus_dari_manager_token_dan_penerima_yang_tepat() {
        use alloy::sol_types::SolEvent;
        let manager = Address::repeat_byte(1);
        let owner = Address::repeat_byte(2);
        let event = super::super::abi_write::Collect {
            tokenId: U256::from(7),
            recipient: owner,
            amount0: U256::from(11),
            amount1: U256::from(12),
        };
        let log = |address| alloy::rpc::types::Log {
            inner: alloy::primitives::Log {
                address,
                data: event.encode_log_data(),
            },
            ..Default::default()
        };
        let valid = log(manager);
        let amounts =
            collect_dari_log(std::slice::from_ref(&valid), manager, U256::from(7), owner).unwrap();
        assert_eq!(amounts.amount0, "11");
        assert!(collect_dari_log(
            &[log(Address::repeat_byte(9))],
            manager,
            U256::from(7),
            owner
        )
        .is_none());
        assert!(
            collect_dari_log(std::slice::from_ref(&valid), manager, U256::from(8), owner).is_none()
        );
        assert!(collect_dari_log(&[valid.clone(), valid], manager, U256::from(7), owner).is_none());
    }

    #[test]
    fn biaya_receipt_aktual_presisi_dan_receipt_tak_lengkap_bukan_nol() {
        let mut value = serde_json::json!({
            "transactionHash": format!("{:#x}", TxHash::ZERO),
            "from": format!("{:#x}", Address::repeat_byte(1)),
            "gasUsed": "0x5208", "effectiveGasPrice": "0x3b9aca01",
            "cumulativeGasUsed": "0x5208", "status": "0x0", "type": "0x2",
            "logs": [], "logsBloom": format!("0x{}", "00".repeat(256))
        });
        let receipt = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(biaya_gas_wei(&receipt).as_deref(), Some("21000000021000"));
        value.as_object_mut().unwrap().remove("effectiveGasPrice");
        let missing = serde_json::from_value(value).unwrap();
        assert_eq!(biaya_gas_wei(&missing), None);
        let result = jadikan_hasil(Terkirim {
            tx_hash: TxHash::ZERO,
            nonce: 0,
            gas_limit: 50_000,
            receipt: Some(receipt),
        });
        assert_eq!(result.status, TxStatus::Revert);
        assert_eq!(result.gas_cost_wei.as_deref(), Some("21000000021000"));
        let pending = jadikan_hasil(Terkirim {
            tx_hash: TxHash::ZERO,
            nonce: 1,
            gas_limit: 50_000,
            receipt: None,
        });
        assert_eq!(pending.gas_cost_wei, None);
        assert_eq!(pending.minted_amounts, None);
    }

    #[test]
    fn cadangan_memakai_value_dan_biaya_maksimum_transaksi_aktual() {
        let saldo = U256::from(10u64).pow(U256::from(18));
        let reserve = U256::from(2_000_000_000_000_000u64);
        let gas = U256::from(100_000_000_000_000u64);
        let value = saldo - reserve - gas;
        assert!(periksa_cadangan(saldo, value, 100_000, 1_000_000_000, 0.002).is_ok());
        assert!(
            periksa_cadangan(saldo, value + U256::from(1), 100_000, 1_000_000_000, 0.002).is_err()
        );
        assert!(periksa_cadangan(saldo, U256::MAX, 100_000, 1_000_000_000, 0.002).is_err());
    }

    #[tokio::test]
    async fn request_dengan_biaya_bisa_ditandatangani_tanpa_filler() {
        let signer =
            PrivateKeySigner::from_bytes(&alloy::primitives::B256::repeat_byte(1)).unwrap();
        let tx = TransactionRequest::default()
            .with_from(signer.address())
            .with_to(Address::repeat_byte(2))
            .with_value(U256::ZERO)
            .with_chain_id(4663)
            .with_nonce(0)
            .with_gas_limit(100_000);
        assert!(!tx.can_build());
        let lengkap = isi_biaya(tx, 1_000_000_000, 0).unwrap();
        assert!(lengkap.can_build());
        assert!(lengkap.build(&EthereumWallet::from(signer)).await.is_ok());
    }

    #[tokio::test]
    async fn dry_run_menolak_sebelum_rpc_nonce_dan_signing() {
        let store = crate::store::Store::open(
            &std::env::temp_dir().join(format!("rangefinder-dry-journal-{}", std::process::id())),
        )
        .unwrap();
        let journal = super::super::journal::Journal {
            store: &store,
            context: super::super::journal::test_context(),
        };
        let provider =
            alloy::providers::ProviderBuilder::new().connect_mocked_client(Default::default());
        let signer =
            PrivateKeySigner::from_bytes(&alloy::primitives::B256::repeat_byte(1)).unwrap();
        let hasil = send_calldata(
            &provider,
            &NonceKeeper::new(),
            &signer,
            Address::repeat_byte(2),
            Bytes::new(),
            U256::ZERO,
            4663,
            &crate::config::ExecutionConfig::default(),
            &crate::config::LimitsConfig::default(),
            &journal,
        )
        .await;
        assert!(matches!(hasil, Err(SendError::TidakBolehDikirim(_))));
    }

    #[tokio::test]
    async fn koneksi_putus_saat_broadcast_tetap_mencatat_hash_dan_menahan_nonce() {
        use alloy::transports::mock::Asserter;

        let dir = std::env::temp_dir().join(format!(
            "rangefinder-broadcast-journal-{}",
            std::process::id()
        ));
        let store = crate::store::Store::open(&dir).unwrap();
        let journal = super::super::journal::Journal {
            store: &store,
            context: super::super::journal::test_context(),
        };
        let signer =
            PrivateKeySigner::from_bytes(&alloy::primitives::B256::repeat_byte(8)).unwrap();
        let tx = TransactionRequest::default()
            .with_from(signer.address())
            .with_to(Address::repeat_byte(2))
            .with_value(U256::ZERO)
            .with_chain_id(4663)
            .with_nonce(12)
            .with_gas_limit(100_000)
            .with_max_fee_per_gas(1_000_000_000)
            .with_max_priority_fee_per_gas(0);
        let rpc = Asserter::new();
        rpc.push_failure_msg("koneksi terputus setelah request dikirim");
        let provider = alloy::providers::ProviderBuilder::new().connect_mocked_client(rpc);

        let (hash, receipt) = kirim_dan_tunggu(&provider, &signer, tx, 4663, &journal)
            .await
            .unwrap();
        assert!(receipt.is_none());
        let catatan = super::super::journal::entries(&store, 4663, signer.address()).unwrap();
        assert_eq!(catatan.len(), 1);
        assert_eq!(catatan[0].tx_hash, hash);
        assert_eq!(catatan[0].request.nonce, Some(12));
        assert_eq!(catatan[0].status, super::super::journal::Status::Unknown);
        assert!(catatan[0]
            .broadcast_error
            .as_deref()
            .is_some_and(|e| e.contains("koneksi terputus")));
        assert_eq!(
            super::super::journal::nonce_floor(&store, 4663, signer.address()).unwrap(),
            13
        );
    }

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
            gas_cost_wei: Some("1".into()),
            minted_amounts: None,
            token_id: Some("7".into()),
            verification: Some(GateResult::lolos_publik("G7", "verifikasi", "cocok")),
            sent_at: "2026-09-07T00:00:00Z".into(),
        };
        assert!(dasar.fully_verified());

        let mut g7_gagal = dasar.clone();
        g7_gagal.verification = Some(GateResult::gagal_publik("G7", "verifikasi", "beda"));
        assert!(
            !g7_gagal.fully_verified(),
            "G7 gagal berarti belum terbukti"
        );

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
