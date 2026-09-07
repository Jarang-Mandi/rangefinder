//! Menggabungkan banyak panggilan baca menjadi satu permintaan RPC.
//!
//! KENAPA MODUL INI ADA
//! Pencarian pool FIG membutuhkan `getSlot0` dan `getLiquidity` untuk 39 pool,
//! yaitu 78 permintaan berurutan. Pada RPC publik dengan latensi sekitar 300 ms
//! itu berarti sekitar 25 detik hanya untuk membaca keadaan pool, dan user
//! melihat panel kosong selama itu. Zenith LP menampilkan hasil seketika, dan
//! selisihnya bukan pada kecerdasan melainkan pada jumlah round trip.
//!
//! Multicall3 mengeksekusi seluruh daftar panggilan di dalam satu transaksi
//! `eth_call`, jadi 78 permintaan menjadi satu.
//!
//! INVARIAN
//! - Alamat Multicall3 datang dari `chain.toml`, tidak pernah ditulis di sini.
//! - `allowFailure` selalu benar. Satu pool yang gagal dibaca tidak boleh
//!   menjatuhkan seluruh batch, karena hasilnya akan terlihat seperti token
//!   tanpa pool sama sekali.
//! - Kegagalan per panggilan dikembalikan apa adanya sebagai `None`, bukan
//!   diubah menjadi nilai nol. Nol yang dipalsukan pernah membuat gate saldo
//!   menolak transaksi yang sebenarnya sah. Lihat memory/003.

use alloy::primitives::{Address, Bytes};
use alloy::providers::Provider;
use alloy::sol;
use alloy::sol_types::SolCall;
use anyhow::Result;

sol! {
    #[sol(rpc)]
    interface IMulticall3 {
        struct Call3 {
            address target;
            bool allowFailure;
            bytes callData;
        }

        struct Result {
            bool success;
            bytes returnData;
        }

        function aggregate3(Call3[] calldata calls) external payable returns (Result[] memory returnData);
    }
}

/// Jumlah panggilan terbanyak dalam satu batch.
///
/// Batch yang terlalu besar membuat `eth_call` melewati batas gas node dan
/// ditolak seluruhnya. Dua ratus panggilan baca ringan masih jauh di bawah
/// batas itu, dan sudah cukup untuk menutup token dengan ratusan pool dalam
/// satu atau dua permintaan.
pub const MAX_BATCH: usize = 200;

/// Berapa kali menunggu saat RPC membatasi laju sebelum menyerah.
const SABAR_MAKSIMUM: u32 = 4;

/// Jeda dasar saat dibatasi laju. Naik berlipat tiap percobaan.
const JEDA_DASAR_MS: u64 = 400;

/// Benar kalau kegagalannya karena laju dibatasi.
///
/// Dikenali dari teks, karena lapisan transport membungkus kode HTTP-nya.
/// Kalau pengenalan ini meleset, akibatnya hanya kembali ke perilaku lama,
/// yaitu gagal seketika.
fn dibatasi_laju(e: &impl std::fmt::Display) -> bool {
    let teks = e.to_string();
    teks.contains("429") || teks.to_lowercase().contains("too many requests")
}

/// Jalankan sekumpulan panggilan baca dalam satu permintaan.
///
/// Hasilnya sejajar dengan masukan: indeks ke-n pada keluaran adalah jawaban
/// untuk panggilan ke-n. `None` berarti panggilan itu gagal, dan pemanggil yang
/// memutuskan apakah itu fatal.
pub async fn batch_call<P: Provider>(
    provider: &P,
    multicall: Address,
    calls: Vec<(Address, Bytes)>,
) -> Result<Vec<Option<Bytes>>> {
    if calls.is_empty() {
        return Ok(Vec::new());
    }

    let mc = IMulticall3::new(multicall, provider);
    let mut out = Vec::with_capacity(calls.len());

    for bagian in calls.chunks(MAX_BATCH) {
        let payload: Vec<IMulticall3::Call3> = bagian
            .iter()
            .map(|(target, data)| IMulticall3::Call3 {
                target: *target,
                allowFailure: true,
                callData: data.clone(),
            })
            .collect();

        // DIBATASI LAJU DITUNGGU, BUKAN DIANGGAP GAGAL
        //
        // RPC publik ini membalas 429 saat beberapa permintaan berat datang
        // berdekatan. Batch ini sendiri satu permintaan, jadi memecahnya
        // tidak menolong. Yang menolong hanya menunggu sebentar lalu
        // mengulang. Pelajaran yang sama sudah tercatat untuk pemindaian log
        // di memory/006.
        let mut sisa = SABAR_MAKSIMUM;
        let hasil = loop {
            match mc.aggregate3(payload.clone()).call().await {
                Ok(v) => break v,
                Err(e) if dibatasi_laju(&e) && sisa > 0 => {
                    let jeda = JEDA_DASAR_MS * (SABAR_MAKSIMUM - sisa + 1) as u64;
                    sisa -= 1;
                    tracing::debug!(jeda_ms = jeda, "multicall dibatasi laju, menunggu");
                    tokio::time::sleep(std::time::Duration::from_millis(jeda)).await;
                }
                Err(e) => {
                    return Err(anyhow::Error::new(e).context("multicall aggregate3 gagal"));
                }
            }
        };

        for r in hasil {
            out.push(if r.success && !r.returnData.is_empty() {
                Some(r.returnData)
            } else {
                None
            });
        }
    }

    Ok(out)
}

/// Bentuk satu entri batch dari panggilan alloy yang sudah diketik.
///
/// Dipakai supaya sisi pemanggil tetap memakai tipe yang dihasilkan `sol!`,
/// bukan menyusun calldata dengan tangan. Menyusun calldata manual adalah
/// tempat kesalahan paling mahal terjadi, dan gate G2 di PRD 6.4 ada persis
/// karena itu.
pub fn encode<C: SolCall>(target: Address, call: &C) -> (Address, Bytes) {
    (target, Bytes::from(call.abi_encode()))
}

/// Terjemahkan kembali hasil batch menjadi tipe kembalian yang benar.
pub fn decode<C: SolCall>(raw: Option<&Bytes>) -> Option<C::Return> {
    let data = raw?;
    C::abi_decode_returns(data).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pools::abi;
    use alloy::primitives::FixedBytes;

    #[test]
    fn encode_menghasilkan_selector_yang_benar() {
        let target = Address::ZERO;
        let call = abi::IStateView::getLiquidityCall {
            poolId: FixedBytes::<32>::ZERO,
        };
        let (t, data) = encode(target, &call);
        assert_eq!(t, target);
        assert_eq!(
            &data[..4],
            &abi::IStateView::getLiquidityCall::SELECTOR,
            "selector harus sama dengan yang dihasilkan alloy"
        );
        assert_eq!(data.len(), 4 + 32, "satu argumen bytes32 sesudah selector");
    }

    #[test]
    fn decode_menolak_data_kosong() {
        let hasil = decode::<abi::IStateView::getLiquidityCall>(None);
        assert!(hasil.is_none(), "panggilan gagal tidak boleh jadi nilai");
    }

    #[test]
    fn decode_menolak_data_yang_terlalu_pendek() {
        // Node yang mengembalikan data cacat tidak boleh berubah menjadi
        // likuiditas nol, karena nol terlihat sah dan pool yang sehat akan
        // ikut terbuang oleh penyaring.
        let cacat = Bytes::from(vec![0u8; 8]);
        let hasil = decode::<abi::IStateView::getLiquidityCall>(Some(&cacat));
        assert!(hasil.is_none());
    }
}
