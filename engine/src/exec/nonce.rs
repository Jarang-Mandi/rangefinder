//! Penjaga nonce untuk satu wallet.
//!
//! KENAPA TIDAK CUKUP MEMBACA DARI RANTAI
//! `eth_getTransactionCount` dengan tag `pending` sudah menghitung transaksi
//! yang masih di mempool, tapi ia tidak instan. Antara transaksi terkirim dan
//! node memperbaruinya ada jeda, dan pada rantai dengan blok 100 ms jeda itu
//! cukup untuk membuat dua pengiriman berturut turut memakai nonce yang sama.
//! Yang kedua akan ditolak dengan `nonce too low`, atau lebih buruk,
//! menggantikan yang pertama.
//!
//! Jadi nonce dipesan di sini lebih dulu, dan nilai dari rantai hanya dipakai
//! sebagai lantai.
//!
//! INVARIAN
//! - Satu nonce tidak pernah diberikan dua kali selama pemesanannya belum
//!   dilepas. Pemesanan dijaga mutex, bukan atomic, karena pemesanan melibatkan
//!   panggilan jaringan dan harus utuh dari awal sampai akhir.
//! - Kalau pengiriman gagal, nonce yang sudah dipesan WAJIB dilepas. Nonce yang
//!   bocor membuat seluruh transaksi berikutnya menggantung selamanya, karena
//!   rantai menunggu nonce yang tidak akan pernah datang.
//! - Cadangan lokal dibuang setiap kali rantai menyusul, supaya kesalahan kita
//!   tidak menumpuk tanpa batas.

use alloy::primitives::Address;
use alloy::providers::Provider;
use anyhow::{Context, Result};
use tokio::sync::Mutex;

/// Nonce berikutnya yang boleh dipakai, per alamat.
#[derive(Debug, Default)]
pub struct NonceKeeper {
    berikutnya: Mutex<Option<(Address, u64)>>,
}

/// Nonce yang sudah dipesan dan belum dilepas.
///
/// Sengaja tidak mengimplementasikan `Drop` yang melepas otomatis, karena
/// melepas setelah pengiriman berhasil justru salah: nonce itu sudah terpakai
/// di rantai. Pemanggil harus menyatakan hasilnya secara eksplisit.
#[derive(Debug, Clone, Copy)]
pub struct Reservasi {
    pub nonce: u64,
    pub address: Address,
}

impl NonceKeeper {
    pub fn new() -> Self {
        Self::default()
    }

    /// Pesan nonce berikutnya untuk sebuah alamat.
    ///
    /// Nilai dari rantai dipakai sebagai lantai. Kalau cadangan lokal lebih
    /// tinggi, itu berarti ada transaksi kita yang belum terlihat node, dan
    /// cadangan lokal yang menang.
    pub async fn reserve<P: Provider>(&self, provider: &P, address: Address) -> Result<Reservasi> {
        self.reserve_with_floor(provider, address, 0).await
    }

    pub async fn reserve_with_floor<P: Provider>(
        &self,
        provider: &P,
        address: Address,
        floor: u64,
    ) -> Result<Reservasi> {
        let mut guard = self.berikutnya.lock().await;

        let dari_rantai = provider
            .get_transaction_count(address)
            .pending()
            .await
            .with_context(|| format!("gagal membaca nonce untuk {address}"))?;

        let lokal = match *guard {
            Some((a, n)) if a == address => Some(n),
            _ => None,
        };

        let nonce = match lokal {
            Some(n) if n > dari_rantai => n,
            _ => dari_rantai,
        }
        .max(floor);

        let berikutnya = nonce.checked_add(1).context("nonce melampaui batas u64")?;
        *guard = Some((address, berikutnya));
        Ok(Reservasi { nonce, address })
    }

    /// Batalkan pemesanan karena transaksinya tidak jadi dikirim.
    ///
    /// Hanya berlaku kalau tidak ada pemesanan lain yang sudah menyusul, karena
    /// mundur setelah nonce berikutnya terpakai akan membuat dua transaksi
    /// bertabrakan.
    pub async fn release(&self, r: Reservasi) {
        let mut guard = self.berikutnya.lock().await;
        if let Some((a, n)) = *guard {
            if a == r.address && n == r.nonce + 1 {
                *guard = if r.nonce == 0 {
                    None
                } else {
                    Some((a, r.nonce))
                };
            }
        }
    }

    /// Lupakan cadangan lokal sepenuhnya.
    ///
    /// Dipakai kalau rantai menolak nonce kita, yang berarti anggapan lokal
    /// sudah tidak nyambung dengan kenyataan.
    pub async fn reset(&self) {
        *self.berikutnya.lock().await = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::{providers::ProviderBuilder, transports::mock::Asserter};

    fn alamat(b: u8) -> Address {
        Address::from([b; 20])
    }

    #[tokio::test]
    async fn melepas_mengembalikan_nonce_yang_sama() {
        let k = NonceKeeper::new();
        let a = alamat(1);

        // Tanpa provider, keadaan diatur langsung supaya perilaku pelepasan
        // bisa diuji tanpa menyentuh jaringan.
        *k.berikutnya.lock().await = Some((a, 5));
        k.release(Reservasi {
            nonce: 4,
            address: a,
        })
        .await;
        assert_eq!(*k.berikutnya.lock().await, Some((a, 4)));
    }

    #[tokio::test]
    async fn melepas_diabaikan_kalau_sudah_disusul() {
        // Pemesanan lain sudah mengambil nonce berikutnya. Mundur di sini akan
        // membuat dua transaksi memakai nonce yang sama.
        let k = NonceKeeper::new();
        let a = alamat(2);
        *k.berikutnya.lock().await = Some((a, 9));
        k.release(Reservasi {
            nonce: 4,
            address: a,
        })
        .await;
        assert_eq!(
            *k.berikutnya.lock().await,
            Some((a, 9)),
            "pemesanan yang lebih baru tidak boleh terganggu"
        );
    }

    #[tokio::test]
    async fn melepas_alamat_lain_tidak_berpengaruh() {
        let k = NonceKeeper::new();
        let a = alamat(3);
        let b = alamat(4);
        *k.berikutnya.lock().await = Some((a, 7));
        k.release(Reservasi {
            nonce: 6,
            address: b,
        })
        .await;
        assert_eq!(*k.berikutnya.lock().await, Some((a, 7)));
    }

    #[tokio::test]
    async fn melepas_nonce_nol_mengosongkan_cadangan() {
        let k = NonceKeeper::new();
        let a = alamat(5);
        *k.berikutnya.lock().await = Some((a, 1));
        k.release(Reservasi {
            nonce: 0,
            address: a,
        })
        .await;
        assert_eq!(
            *k.berikutnya.lock().await,
            None,
            "wallet baru kembali ke keadaan belum tahu apa apa"
        );
    }

    #[tokio::test]
    async fn reset_membuang_cadangan() {
        let k = NonceKeeper::new();
        *k.berikutnya.lock().await = Some((alamat(6), 12));
        k.reset().await;
        assert_eq!(*k.berikutnya.lock().await, None);
    }

    #[tokio::test]
    async fn lantai_persisten_mengalahkan_nonce_rpc_dan_lokal() {
        let rpc = Asserter::new();
        rpc.push_success(&2u64);
        let provider = ProviderBuilder::new().connect_mocked_client(rpc);
        let k = NonceKeeper::new();
        *k.berikutnya.lock().await = Some((alamat(7), 4));
        let reservasi = k.reserve_with_floor(&provider, alamat(7), 9).await.unwrap();
        assert_eq!(reservasi.nonce, 9);
        assert_eq!(*k.berikutnya.lock().await, Some((alamat(7), 10)));
    }

    #[tokio::test]
    async fn nonce_maksimum_ditolak_tanpa_merusak_keadaan_lokal() {
        let rpc = Asserter::new();
        rpc.push_success(&u64::MAX);
        let provider = ProviderBuilder::new().connect_mocked_client(rpc);
        let k = NonceKeeper::new();
        assert!(k.reserve(&provider, alamat(8)).await.is_err());
        assert_eq!(*k.berikutnya.lock().await, None);
    }
}
