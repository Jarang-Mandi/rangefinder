//! Registry kontrak, dibaca dari `chain.toml` di akar repo.
//!
//! INVARIAN
//! - `chain.toml` adalah SUMBER TUNGGAL alamat kontrak. Jangan pernah menulis
//!   alamat langsung di kode Rust. Kalau butuh alamat baru, tambahkan di TOML.
//! - Modul ini dipakai bersama oleh engine (gate G1 saat start) dan oleh
//!   `cargo xtask verify`. Keduanya membaca berkas yang sama persis, sehingga
//!   tidak mungkin ada dua daftar yang berbeda.
//! - `code_hash` kosong berarti belum pernah dipin. Itu peringatan, bukan
//!   kegagalan. `code_hash` terisi tapi tidak cocok adalah kegagalan keras.

use std::collections::BTreeMap;
use std::path::Path;

use alloy::primitives::Address;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChainMeta {
    pub name: String,
    pub id: u64,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub gas_token: String,
    #[serde(default)]
    pub explorer: String,
    #[serde(default)]
    pub block_time_ms: u64,
    /// Blok saat protokol dipasang di chain ini.
    ///
    /// Dipakai sebagai batas bawah saat memindai log `Initialize` milik
    /// PoolManager V4. Tanpa batas ini, pemindaian akan menyapu dari blok nol
    /// dan ditolak RPC publik.
    #[serde(default)]
    pub deploy_block: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ContractEntry {
    pub address: String,
    /// keccak256 dari bytecode terpasang, hex tanpa awalan. Kosong = belum dipin.
    #[serde(default)]
    pub code_hash: String,
    /// Ditandai untuk kontrak yang merupakan fork dan tidak kompatibel dengan
    /// SDK standar. Saat ini hanya UniversalRouter. Lihat PRD 6.3.
    #[serde(default)]
    pub forked: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TokenEntry {
    pub address: String,
    pub decimals: u8,
    #[serde(default)]
    pub note: String,
}

/// Bentuk `chain.toml` apa adanya.
///
/// Setiap bagian memakai map, bukan struct dengan field tetap, supaya
/// menambahkan kontrak baru cukup menyunting TOML tanpa menyentuh Rust.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Registry {
    pub chain: ChainMeta,
    #[serde(default)]
    pub tokens: BTreeMap<String, TokenEntry>,
    #[serde(default)]
    pub common: BTreeMap<String, ContractEntry>,
    #[serde(default)]
    pub uniswap_v3: BTreeMap<String, ContractEntry>,
    #[serde(default)]
    pub uniswap_v4: BTreeMap<String, ContractEntry>,
    #[serde(default)]
    pub third_party: BTreeMap<String, ContractEntry>,
}

/// Satu kontrak beserta asal bagiannya, untuk pelaporan yang jelas.
#[derive(Debug, Clone)]
pub struct NamedContract {
    pub section: &'static str,
    pub name: String,
    pub address: Address,
    pub code_hash: String,
    pub forked: bool,
}

impl Registry {
    pub fn load_from(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| {
            format!(
                "gagal membaca registry kontrak di {}. Berkas ini wajib ada, \
                 lihat docs/CHAIN.md",
                path.display()
            )
        })?;
        let reg: Registry = toml::from_str(&text)
            .with_context(|| format!("{} bukan TOML yang valid", path.display()))?;
        reg.validate()?;
        Ok(reg)
    }

    /// Nama berkas baku, relatif terhadap akar repo.
    pub fn default_path(repo_root: &Path) -> std::path::PathBuf {
        repo_root.join("chain.toml")
    }

    fn validate(&self) -> Result<()> {
        // Alamat harus parse. Lebih baik gagal saat start daripada saat mengirim
        // transaksi ke alamat yang salah bentuk.
        for c in self.all_contracts()? {
            let _ = c;
        }
        for (name, t) in &self.tokens {
            t.address
                .parse::<Address>()
                .with_context(|| format!("alamat token `{name}` tidak valid: {}", t.address))?;
        }
        anyhow::ensure!(self.chain.id != 0, "chain.id tidak boleh nol");
        Ok(())
    }

    /// Semua kontrak dari semua bagian, sudah diparse jadi `Address`.
    pub fn all_contracts(&self) -> Result<Vec<NamedContract>> {
        let mut out = Vec::new();
        let sections: [(&'static str, &BTreeMap<String, ContractEntry>); 4] = [
            ("common", &self.common),
            ("uniswap_v3", &self.uniswap_v3),
            ("uniswap_v4", &self.uniswap_v4),
            ("third_party", &self.third_party),
        ];
        for (section, map) in sections {
            for (name, entry) in map {
                let address = entry.address.parse::<Address>().with_context(|| {
                    format!(
                        "alamat kontrak `{section}.{name}` tidak valid: {}",
                        entry.address
                    )
                })?;
                out.push(NamedContract {
                    section,
                    name: name.clone(),
                    address,
                    code_hash: entry.code_hash.clone(),
                    forked: entry.forked,
                });
            }
        }
        Ok(out)
    }

    /// Ambil satu kontrak berdasarkan bagian dan nama. Dipakai kode eksekusi
    /// supaya tidak perlu menghafal alamat.
    pub fn contract(&self, section: &str, name: &str) -> Result<Address> {
        let map = match section {
            "common" => &self.common,
            "uniswap_v3" => &self.uniswap_v3,
            "uniswap_v4" => &self.uniswap_v4,
            "third_party" => &self.third_party,
            other => anyhow::bail!("bagian registry `{other}` tidak dikenal"),
        };
        let entry = map
            .get(name)
            .with_context(|| format!("kontrak `{section}.{name}` tidak ada di chain.toml"))?;
        entry
            .address
            .parse::<Address>()
            .with_context(|| format!("alamat `{section}.{name}` tidak valid"))
    }

    /// Berapa kontrak yang belum punya `code_hash` terpin.
    pub fn unpinned_count(&self) -> usize {
        self.all_contracts()
            .map(|v| v.iter().filter(|c| c.code_hash.is_empty()).count())
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTOH: &str = r#"
[chain]
name = "Test"
id = 4663

[tokens]
weth = { address = "0x0Bd7D308f8E1639FAb988df18A8011f41EAcAD73", decimals = 18 }

[common]
permit2 = { address = "0x000000000022D473030F116dDEE9F6B43aC78BA3" }

[uniswap_v3]
factory = { address = "0x1f7d7550b1b028f7571e69a784071f0205fd2efa", code_hash = "aabb" }

[uniswap_v4]
universal_router = { address = "0x8876789976dEcBfCbBbe364623C63652db8C0904", forked = true }
"#;

    #[test]
    fn membaca_registry_contoh() {
        let reg: Registry = toml::from_str(CONTOH).unwrap();
        reg.validate().unwrap();
        assert_eq!(reg.chain.id, 4663);

        let all = reg.all_contracts().unwrap();
        assert_eq!(all.len(), 3);

        let router = all.iter().find(|c| c.name == "universal_router").unwrap();
        assert!(router.forked, "UniversalRouter wajib ditandai sebagai fork");

        // Dua dari tiga belum dipin.
        assert_eq!(reg.unpinned_count(), 2);
    }

    #[test]
    fn mencari_kontrak_berdasarkan_nama() {
        let reg: Registry = toml::from_str(CONTOH).unwrap();
        assert!(reg.contract("uniswap_v3", "factory").is_ok());
        assert!(reg.contract("uniswap_v3", "tidak_ada").is_err());
        assert!(reg.contract("bagian_ngawur", "factory").is_err());
    }

    #[test]
    fn alamat_rusak_ditolak_saat_validasi() {
        let rusak = CONTOH.replace("0x1f7d7550b1b028f7571e69a784071f0205fd2efa", "bukan-alamat");
        let reg: Registry = toml::from_str(&rusak).unwrap();
        assert!(reg.validate().is_err());
    }

    #[test]
    fn chain_toml_asli_valid() {
        // Menjaga supaya berkas nyata di repo tidak pernah rusak tanpa ketahuan.
        let path = crate::config::repo_root().join("chain.toml");
        let reg = Registry::load_from(&path).expect("chain.toml di repo harus valid");
        assert_eq!(reg.chain.id, 4663);
        assert!(reg.contract("uniswap_v3", "position_manager").is_ok());
        assert!(reg.contract("uniswap_v4", "pool_manager").is_ok());
    }
}
