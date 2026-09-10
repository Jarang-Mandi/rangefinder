//! Manajemen wallet: pembuatan, buka kunci, kunci, dan kunci otomatis.
//!
//! INVARIAN
//! - Kunci privat TIDAK PERNAH keluar dari modul ini. Tidak ke API, tidak ke
//!   log, tidak ke panel. Yang boleh keluar hanya alamat dan status.
//! - Alamat bisa dibaca meski wallet terkunci, karena alamat bukan rahasia dan
//!   panel butuh menampilkannya sebelum user membuka wallet.
//! - Setiap operasi yang menandatangani WAJIB memanggil `touch()` supaya
//!   penghitung kunci otomatis mundur lagi.
//! - Satu wallet saja. Keputusan user yang sudah dikunci, lihat PRD 16.
//!   Struktur tetap menyimpan alamat secara eksplisit supaya penambahan wallet
//!   nanti tidak membutuhkan migrasi.

pub mod keystore;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use alloy::primitives::{Address, B256};
use alloy::signers::local::PrivateKeySigner;
use anyhow::{Context, Result};

use crate::error::EngineError;
use keystore::{KeystoreFile, Secret};

pub struct Wallet {
    path: PathBuf,
    auto_lock: Duration,
    /// Ada isinya hanya saat wallet terbuka.
    unlocked: Option<Unlocked>,
    /// Alamat dari berkas keystore. `None` berarti wallet belum dibuat.
    address: Option<Address>,
}

struct Unlocked {
    signer: PrivateKeySigner,
    last_activity: Instant,
}

impl std::fmt::Debug for Wallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Wallet")
            .field("address", &self.address)
            .field("locked", &self.unlocked.is_none())
            .finish()
    }
}

/// Status yang aman dikirim ke panel.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WalletStatus {
    pub exists: bool,
    pub locked: bool,
    pub address: Option<String>,
    pub auto_lock_minutes: u64,
    /// Detik tersisa sebelum kunci otomatis. `None` kalau sedang terkunci.
    pub seconds_until_auto_lock: Option<u64>,
}

impl Wallet {
    pub fn open(dir: &Path, auto_lock_minutes: u64) -> Result<Self> {
        let path = dir.join("wallet.keystore.json");
        let address =
            if path.exists() {
                let file = read_keystore(&path)?;
                Some(file.address.parse::<Address>().with_context(|| {
                    format!("alamat `{}` di keystore tidak valid", file.address)
                })?)
            } else {
                None
            };

        Ok(Self {
            path,
            auto_lock: Duration::from_secs(auto_lock_minutes.saturating_mul(60)),
            unlocked: None,
            address,
        })
    }

    pub fn exists(&self) -> bool {
        self.address.is_some()
    }

    pub fn is_locked(&self) -> bool {
        self.unlocked.is_none()
    }

    pub fn address(&self) -> Option<Address> {
        self.address
    }

    pub fn status(&self) -> WalletStatus {
        WalletStatus {
            exists: self.exists(),
            locked: self.is_locked(),
            address: self.address.map(|a| a.to_string()),
            auto_lock_minutes: self.auto_lock.as_secs() / 60,
            seconds_until_auto_lock: self.unlocked.as_ref().map(|u| {
                self.auto_lock
                    .saturating_sub(u.last_activity.elapsed())
                    .as_secs()
            }),
        }
    }

    /// Buat wallet baru dari kunci privat yang diberikan, atau acak kalau `None`.
    ///
    /// Menolak menimpa wallet yang sudah ada. Menimpa keystore sama dengan
    /// menghapus akses ke dana, jadi itu harus jadi tindakan manual yang sadar.
    pub fn create(
        &mut self,
        passphrase: &str,
        private_key: Option<B256>,
    ) -> Result<Address, EngineError> {
        // Bentrokan keadaan, bukan kesalahan internal. Panel memakai kode ini
        // untuk menawarkan buka kunci alih alih menampilkan pesan kegagalan.
        if self.exists() {
            return Err(EngineError::WalletExists(self.path.display().to_string()));
        }

        self.create_inner(passphrase, private_key)
            .map_err(EngineError::Other)
    }

    fn create_inner(&mut self, passphrase: &str, private_key: Option<B256>) -> Result<Address> {
        let key = match private_key {
            Some(k) => k,
            None => {
                let mut bytes = [0u8; 32];
                getrandom::fill(&mut bytes).context("gagal mengambil entropi untuk kunci baru")?;
                B256::from(bytes)
            }
        };

        let signer = PrivateKeySigner::from_bytes(&key)
            .context("kunci privat tidak valid untuk secp256k1")?;
        let address = signer.address();

        let file = keystore::encrypt(passphrase, key.as_slice(), &address.to_string())?;
        let json = serde_json::to_string_pretty(&file)?;

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, json)
            .with_context(|| format!("gagal menulis keystore ke {}", self.path.display()))?;

        self.address = Some(address);
        tracing::info!(%address, path = %self.path.display(), "wallet dibuat");
        Ok(address)
    }

    pub fn unlock(&mut self, passphrase: &str) -> Result<Address, EngineError> {
        let file = read_keystore(&self.path).map_err(|_| EngineError::WalletMissing)?;
        let secret: Secret = keystore::decrypt(&file, passphrase)?;

        let key = B256::from_slice(secret.as_slice());
        let signer = PrivateKeySigner::from_bytes(&key)
            .map_err(|e| EngineError::KeystoreCorrupt(format!("kunci tidak valid: {e}")))?;
        let address = signer.address();

        // Pemeriksaan silang: alamat hasil dekripsi harus sama dengan yang
        // tercatat di berkas. Kalau beda, berkasnya sudah diutak atik.
        if let Some(expected) = self.address {
            if expected != address {
                return Err(EngineError::KeystoreCorrupt(format!(
                    "alamat hasil dekripsi {address} tidak cocok dengan {expected} di keystore"
                )));
            }
        }

        self.unlocked = Some(Unlocked {
            signer,
            last_activity: Instant::now(),
        });
        self.address = Some(address);
        tracing::info!(%address, "wallet dibuka");
        Ok(address)
    }

    pub fn lock(&mut self) {
        if self.unlocked.take().is_some() {
            tracing::info!("wallet dikunci");
        }
    }

    /// Kunci wallet kalau sudah melewati batas tidak aktif.
    /// Mengembalikan `true` kalau kunci otomatis baru saja terjadi.
    pub fn tick_auto_lock(&mut self) -> bool {
        let expired = self
            .unlocked
            .as_ref()
            .is_some_and(|u| u.last_activity.elapsed() >= self.auto_lock);
        if expired {
            self.unlocked = None;
            tracing::info!("wallet terkunci otomatis karena tidak ada aktivitas");
        }
        expired
    }

    /// Mundurkan penghitung kunci otomatis. Wajib dipanggil setiap ada aksi.
    pub fn touch(&mut self) {
        if let Some(u) = self.unlocked.as_mut() {
            u.last_activity = Instant::now();
        }
    }

    /// Pinjam signer untuk menandatangani. Otomatis memanggil `touch`.
    pub fn signer(&mut self) -> Result<&PrivateKeySigner, EngineError> {
        if !self.exists() {
            return Err(EngineError::WalletMissing);
        }
        self.touch();
        self.unlocked
            .as_ref()
            .map(|u| &u.signer)
            .ok_or(EngineError::WalletLocked)
    }
}

fn read_keystore(path: &Path) -> Result<KeystoreFile> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("gagal membaca keystore di {}", path.display()))?;
    serde_json::from_str(&text)
        .with_context(|| format!("keystore di {} bukan JSON yang valid", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir_sementara(nama: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("rangefinder-test-{nama}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn siklus_buat_buka_kunci() {
        let dir = dir_sementara("wallet-siklus");
        let mut w = Wallet::open(&dir, 30).unwrap();
        assert!(!w.exists());
        assert!(w.is_locked());

        let addr = w.create("passphrase kuat", None).unwrap();
        assert!(w.exists());
        // Membuat wallet tidak otomatis membukanya.
        assert!(w.is_locked());

        let opened = w.unlock("passphrase kuat").unwrap();
        assert_eq!(addr, opened);
        assert!(!w.is_locked());

        w.lock();
        assert!(w.is_locked());
        // Alamat tetap terbaca meski terkunci.
        assert_eq!(w.address(), Some(addr));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn menolak_menimpa_wallet_yang_ada() {
        let dir = dir_sementara("wallet-timpa");
        let mut w = Wallet::open(&dir, 30).unwrap();
        w.create("p", None).unwrap();

        // Harus `WalletExists`, bukan error umum. Panel memakai kode itu untuk
        // menawarkan buka kunci, dan HTTP-nya 409 bukan 500.
        match w.create("p", None) {
            Err(EngineError::WalletExists(path)) => {
                assert!(path.contains("wallet.keystore.json"));
            }
            other => panic!("harusnya WalletExists, dapat {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn signer_ditolak_saat_terkunci() {
        let dir = dir_sementara("wallet-signer");
        let mut w = Wallet::open(&dir, 30).unwrap();
        w.create("p", None).unwrap();
        assert!(matches!(w.signer(), Err(EngineError::WalletLocked)));
        w.unlock("p").unwrap();
        assert!(w.signer().is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn kunci_otomatis_langsung_saat_batas_nol() {
        let dir = dir_sementara("wallet-autolock");
        let mut w = Wallet::open(&dir, 0).unwrap();
        w.create("p", None).unwrap();
        w.unlock("p").unwrap();
        assert!(w.tick_auto_lock());
        assert!(w.is_locked());
        // Panggilan kedua tidak boleh melaporkan penguncian lagi.
        assert!(!w.tick_auto_lock());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn status_tidak_memuat_rahasia() {
        let dir = dir_sementara("wallet-status");
        let mut w = Wallet::open(&dir, 30).unwrap();
        w.create("p", None).unwrap();
        w.unlock("p").unwrap();
        let json = serde_json::to_string(&w.status()).unwrap();
        assert!(json.contains("address"));
        assert!(!json.to_lowercase().contains("key"));
        assert!(!json.to_lowercase().contains("passphrase"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
