//! Token pairing antara engine dan ekstensi.
//!
//! INVARIAN
//! - Perbandingan token WAJIB memakai waktu konstan (`subtle`). Perbandingan
//!   biasa membocorkan panjang prefiks yang cocok lewat timing.
//! - Token tidak pernah masuk log, tidak pernah masuk pesan error, dan tidak
//!   pernah dikirim balik lewat API. Satu satunya tempat ia ditampilkan adalah
//!   stdout saat engine start, supaya user bisa menyalinnya sekali.
//! - Token disimpan di direktori data, bukan di repo. `.gitignore` menutupnya.

use std::path::PathBuf;

use anyhow::{Context, Result};
use subtle::ConstantTimeEq;

/// Panjang token dalam byte. 16 byte sama dengan 128 bit, cukup untuk melawan
/// tebakan sementara masih pendek untuk disalin manual sebagai 32 karakter hex.
const TOKEN_BYTES: usize = 16;

pub struct PairingToken {
    /// Disimpan sebagai byte, bukan String, supaya perbandingan konstan mudah.
    value: [u8; TOKEN_BYTES],
}

impl std::fmt::Debug for PairingToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Tidak pernah membocorkan isi, bahkan saat struct induk di-derive Debug.
        f.write_str("PairingToken([redacted])")
    }
}

impl PairingToken {
    /// Muat token dari disk, atau buat baru kalau belum ada.
    ///
    /// Mengembalikan `(token, baru_dibuat)` supaya pemanggil tahu kapan harus
    /// menampilkan instruksi pairing lengkap.
    pub fn load_or_create(dir: &std::path::Path) -> Result<(Self, bool)> {
        let path = Self::path(dir);

        if path.exists() {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("gagal membaca token pairing di {}", path.display()))?;
            let bytes = hex::decode(text.trim())
                .context("token pairing di disk bukan hex yang valid, hapus berkasnya untuk membuat ulang")?;
            let value: [u8; TOKEN_BYTES] = bytes.as_slice().try_into().map_err(|_| {
                anyhow::anyhow!(
                    "token pairing di disk panjangnya {} byte, seharusnya {}. Hapus berkasnya untuk membuat ulang",
                    bytes.len(),
                    TOKEN_BYTES
                )
            })?;
            return Ok((Self { value }, false));
        }

        let mut value = [0u8; TOKEN_BYTES];
        getrandom::fill(&mut value).context("gagal mengambil entropi dari sistem")?;

        std::fs::create_dir_all(dir)?;
        std::fs::write(&path, hex::encode(value))
            .with_context(|| format!("gagal menulis token pairing ke {}", path.display()))?;

        Ok((Self { value }, true))
    }

    pub fn path(dir: &std::path::Path) -> PathBuf {
        dir.join("pairing.token")
    }

    /// Representasi yang ditampilkan user sekali saat pairing.
    pub fn as_hex(&self) -> String {
        hex::encode(self.value)
    }

    /// Perbandingan waktu konstan terhadap token yang dikirim klien.
    pub fn verify(&self, presented: &str) -> bool {
        let Ok(bytes) = hex::decode(presented.trim()) else {
            return false;
        };
        if bytes.len() != TOKEN_BYTES {
            return false;
        }
        self.value.ct_eq(bytes.as_slice()).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_cocok_dengan_dirinya_sendiri() {
        let t = PairingToken {
            value: [7u8; TOKEN_BYTES],
        };
        assert!(t.verify(&t.as_hex()));
    }

    #[test]
    fn token_menolak_nilai_lain() {
        let t = PairingToken {
            value: [7u8; TOKEN_BYTES],
        };
        assert!(!t.verify(&hex::encode([8u8; TOKEN_BYTES])));
        assert!(!t.verify("bukan hex"));
        assert!(!t.verify(""));
        // Panjang salah harus ditolak, bukan panic.
        assert!(!t.verify(&hex::encode([7u8; TOKEN_BYTES - 1])));
    }

    #[test]
    fn debug_tidak_membocorkan_token() {
        let t = PairingToken {
            value: [0xABu8; TOKEN_BYTES],
        };
        let rendered = format!("{t:?}");
        assert!(!rendered.contains("ab"));
        assert!(rendered.contains("redacted"));
    }
}
