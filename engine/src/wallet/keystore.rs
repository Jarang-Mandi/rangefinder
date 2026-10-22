//! Keystore terenkripsi untuk kunci privat wallet.
//!
//! Skema: Argon2id menurunkan kunci 32 byte dari passphrase dan salt acak,
//! lalu AES-256-GCM mengenkripsi kunci privat. Nonce acak per penulisan.
//!
//! INVARIAN
//! - Kunci privat dan kunci turunan dibungkus `Secret`, yang di-zeroize saat
//!   drop dan mencetak `[redacted]` pada Debug. Jangan pernah menyalinnya ke
//!   `Vec<u8>` biasa yang berumur panjang.
//! - Nonce TIDAK PERNAH dipakai ulang. Setiap penulisan keystore membuat nonce
//!   baru. Nonce yang dipakai ulang dengan kunci sama merusak keamanan GCM
//!   secara total.
//! - Passphrase salah harus menghasilkan `BadPassphrase`, bukan `KeystoreCorrupt`.
//!   AES-GCM menggagalkan autentikasi untuk keduanya, jadi pembedaannya
//!   dilakukan di lapisan ini: gagal dekripsi selalu berarti passphrase salah,
//!   karena berkas yang rusak sudah tersaring saat parsing JSON.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{Context, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::EngineError;

const KEYSTORE_VERSION: u32 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const DERIVED_KEY_LEN: usize = 32;
const PRIVATE_KEY_LEN: usize = 32;

/// Parameter Argon2id. 64 MiB dan 3 putaran adalah titik yang wajar untuk
/// desktop: cukup mahal bagi penyerang, masih di bawah satu detik bagi user.
const ARGON_M_COST: u32 = 65_536;
const ARGON_T_COST: u32 = 3;
const ARGON_P_COST: u32 = 1;

/// Pembungkus rahasia yang di-zeroize saat drop dan tidak pernah tercetak.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct Secret(pub Vec<u8>);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret([redacted])")
    }
}

impl Secret {
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct KdfParams {
    pub algo: String,
    pub m_cost: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

/// Isi berkas keystore di disk. Semua nilai biner disimpan sebagai hex supaya
/// berkasnya bisa dibaca manusia saat memeriksa masalah.
#[derive(Debug, Serialize, Deserialize)]
pub struct KeystoreFile {
    pub version: u32,
    /// Alamat disimpan apa adanya supaya panel bisa menampilkannya tanpa
    /// membuka wallet. Alamat bukan rahasia.
    pub address: String,
    pub cipher: String,
    pub kdf: KdfParams,
    pub salt: String,
    pub nonce: String,
    pub ciphertext: String,
    pub created_at: String,
}

fn derive_key(passphrase: &str, salt: &[u8], kdf: &KdfParams) -> Result<Secret> {
    let params = Params::new(kdf.m_cost, kdf.t_cost, kdf.p_cost, Some(DERIVED_KEY_LEN))
        .map_err(|e| anyhow::anyhow!("parameter argon2 tidak valid: {e}"))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

    let mut out = vec![0u8; DERIVED_KEY_LEN];
    argon
        .hash_password_into(passphrase.as_bytes(), salt, &mut out)
        .map_err(|e| anyhow::anyhow!("argon2 gagal menurunkan kunci: {e}"))?;
    Ok(Secret(out))
}

/// Enkripsi kunci privat menjadi isi berkas keystore.
pub fn encrypt(passphrase: &str, private_key: &[u8], address: &str) -> Result<KeystoreFile> {
    anyhow::ensure!(
        private_key.len() == PRIVATE_KEY_LEN,
        "kunci privat harus {PRIVATE_KEY_LEN} byte, dapat {}",
        private_key.len()
    );
    anyhow::ensure!(!passphrase.is_empty(), "passphrase tidak boleh kosong");

    let mut salt = [0u8; SALT_LEN];
    getrandom::fill(&mut salt).context("gagal mengambil entropi untuk salt")?;

    // Nonce baru setiap penulisan. Lihat invarian di kepala modul.
    let mut nonce_bytes = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce_bytes).context("gagal mengambil entropi untuk nonce")?;

    let kdf = KdfParams {
        algo: "argon2id".into(),
        m_cost: ARGON_M_COST,
        t_cost: ARGON_T_COST,
        p_cost: ARGON_P_COST,
    };

    let key = derive_key(passphrase, &salt, &kdf)?;
    let cipher = Aes256Gcm::new_from_slice(key.as_slice())
        .map_err(|e| anyhow::anyhow!("panjang kunci AES salah: {e}"))?;
    let nonce = Nonce::try_from(&nonce_bytes[..])
        .map_err(|_| anyhow::anyhow!("panjang nonce salah, seharusnya {NONCE_LEN} byte"))?;

    let ciphertext = cipher
        .encrypt(&nonce, private_key)
        .map_err(|_| anyhow::anyhow!("enkripsi keystore gagal"))?;

    Ok(KeystoreFile {
        version: KEYSTORE_VERSION,
        address: address.to_string(),
        cipher: "aes-256-gcm".into(),
        kdf,
        salt: hex::encode(salt),
        nonce: hex::encode(nonce_bytes),
        ciphertext: hex::encode(ciphertext),
        created_at: chrono::Utc::now().to_rfc3339(),
    })
}

/// Dekripsi keystore memakai passphrase. Gagal berarti passphrase salah.
pub fn decrypt(file: &KeystoreFile, passphrase: &str) -> Result<Secret, EngineError> {
    if file.version != KEYSTORE_VERSION {
        return Err(EngineError::KeystoreCorrupt(format!(
            "versi keystore {} tidak didukung, engine ini hanya mengerti versi {KEYSTORE_VERSION}",
            file.version
        )));
    }
    if file.cipher != "aes-256-gcm" {
        return Err(EngineError::KeystoreCorrupt(format!(
            "cipher `{}` tidak dikenal",
            file.cipher
        )));
    }
    if file.kdf.algo != "argon2id" {
        return Err(EngineError::KeystoreCorrupt(format!(
            "kdf `{}` tidak dikenal",
            file.kdf.algo
        )));
    }

    let salt = hex::decode(&file.salt)
        .map_err(|_| EngineError::KeystoreCorrupt("salt bukan hex".into()))?;
    let nonce_bytes = hex::decode(&file.nonce)
        .map_err(|_| EngineError::KeystoreCorrupt("nonce bukan hex".into()))?;
    let ciphertext = hex::decode(&file.ciphertext)
        .map_err(|_| EngineError::KeystoreCorrupt("ciphertext bukan hex".into()))?;

    if nonce_bytes.len() != NONCE_LEN {
        return Err(EngineError::KeystoreCorrupt(format!(
            "nonce panjangnya {} byte, seharusnya {NONCE_LEN}",
            nonce_bytes.len()
        )));
    }

    let key = derive_key(passphrase, &salt, &file.kdf)
        .map_err(|e| EngineError::KeystoreCorrupt(e.to_string()))?;
    let cipher = Aes256Gcm::new_from_slice(key.as_slice())
        .map_err(|e| EngineError::KeystoreCorrupt(format!("panjang kunci AES salah: {e}")))?;
    let nonce = Nonce::try_from(&nonce_bytes[..])
        .map_err(|_| EngineError::KeystoreCorrupt("panjang nonce salah".into()))?;

    // Sampai titik ini berkasnya sudah terbukti berbentuk benar, jadi satu
    // satunya penyebab gagal autentikasi yang tersisa adalah passphrase salah.
    let plaintext = cipher
        .decrypt(&nonce, ciphertext.as_slice())
        .map_err(|_| EngineError::BadPassphrase)?;

    if plaintext.len() != PRIVATE_KEY_LEN {
        return Err(EngineError::KeystoreCorrupt(format!(
            "kunci hasil dekripsi panjangnya {} byte, seharusnya {PRIVATE_KEY_LEN}",
            plaintext.len()
        )));
    }

    Ok(Secret(plaintext))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kunci_contoh() -> [u8; 32] {
        let mut k = [0u8; 32];
        for (i, b) in k.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(7).wrapping_add(3);
        }
        k
    }

    #[test]
    fn putaran_penuh_enkripsi_dekripsi() {
        let key = kunci_contoh();
        let file = encrypt("passphrase yang benar", &key, "0xabc").unwrap();
        let out = decrypt(&file, "passphrase yang benar").unwrap();
        assert_eq!(out.as_slice(), &key[..]);
    }

    #[test]
    fn passphrase_salah_menghasilkan_bad_passphrase() {
        let file = encrypt("benar", &kunci_contoh(), "0xabc").unwrap();
        match decrypt(&file, "salah") {
            Err(EngineError::BadPassphrase) => {}
            other => panic!("harusnya BadPassphrase, dapat {other:?}"),
        }
    }

    #[test]
    fn nonce_berbeda_setiap_penulisan() {
        let key = kunci_contoh();
        let a = encrypt("p", &key, "0xabc").unwrap();
        let b = encrypt("p", &key, "0xabc").unwrap();
        // Invarian paling penting di modul ini.
        assert_ne!(a.nonce, b.nonce, "nonce tidak boleh dipakai ulang");
        assert_ne!(a.salt, b.salt, "salt sebaiknya juga berbeda");
        assert_ne!(a.ciphertext, b.ciphertext);
    }

    #[test]
    fn keystore_rusak_bukan_bad_passphrase() {
        let mut file = encrypt("p", &kunci_contoh(), "0xabc").unwrap();
        file.cipher = "chacha20".into();
        match decrypt(&file, "p") {
            Err(EngineError::KeystoreCorrupt(_)) => {}
            other => panic!("harusnya KeystoreCorrupt, dapat {other:?}"),
        }
    }

    #[test]
    fn secret_tidak_membocorkan_isi_saat_debug() {
        let s = Secret(vec![0xDE, 0xAD, 0xBE, 0xEF]);
        let rendered = format!("{s:?}");
        assert!(rendered.contains("redacted"));
        assert!(!rendered.contains("222"));
    }

    #[test]
    fn kunci_panjang_salah_ditolak() {
        assert!(encrypt("p", &[0u8; 31], "0xabc").is_err());
        assert!(encrypt("", &kunci_contoh(), "0xabc").is_err());
    }
}
