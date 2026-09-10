//! Akses Robinhood Chain: penyedia RPC, pemeriksaan kesehatan, dan gate G1.
//!
//! INVARIAN
//! - Engine TIDAK BOLEH mengeksekusi transaksi saat kesehatan buruk. Yang
//!   dianggap buruk: chain id tidak cocok, lag blok melebihi ambang, atau
//!   seluruh endpoint gagal. Lihat PRD 7.5.
//! - Gate G1 di PRD 6.4 dijalankan di sini lewat `verify_registry`. Kalau ada
//!   `code_hash` terpin yang tidak cocok, engine menolak start. Ini bukan
//!   peringatan, ini penghentian.
//! - Chain ini secara resmi dinyatakan tidak stabil oleh gmgn, jadi kegagalan
//!   RPC adalah kondisi normal yang harus ditangani, bukan kejadian luar biasa.

pub mod endpoint;
pub mod logs;
pub mod multicall;
pub mod registry;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::keccak256;
use alloy::providers::{Provider, ProviderBuilder};
use anyhow::{Context, Result};
use serde::Serialize;
use tokio::sync::RwLock;

use crate::config::RpcConfig;
use registry::Registry;

/// Ringkasan kesehatan yang dikirim ke panel.
#[derive(Debug, Clone, Serialize)]
pub struct ChainHealth {
    pub healthy: bool,
    /// URL RPC yang sedang dipakai, SUDAH disamarkan.
    ///
    /// Field ini dikirim ke panel dan ikut tercetak di layar, sedangkan URL
    /// penyedia berbayar memuat API key di jalurnya. Penyamarannya wajib
    /// terjadi sebelum nilainya masuk ke sini, bukan saat ditampilkan, karena
    /// tempat penampilan bisa bertambah tanpa siapa pun ingat menyamarkannya.
    /// Lihat `chain::endpoint`.
    pub endpoint: String,
    pub chain_id: Option<u64>,
    pub block_number: Option<u64>,
    pub block_lag_secs: Option<u64>,
    pub max_block_lag_secs: u64,
    pub error: Option<String>,
    pub checked_at: String,
}

impl ChainHealth {
    fn unknown(endpoint: &str, max_lag: u64) -> Self {
        Self {
            healthy: false,
            endpoint: endpoint::aman(endpoint),
            chain_id: None,
            block_number: None,
            block_lag_secs: None,
            max_block_lag_secs: max_lag,
            error: Some("belum ada pemeriksaan".into()),
            checked_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

/// Hasil satu kontrak pada gate G1.
#[derive(Debug, Clone, Serialize)]
pub struct RegistryCheck {
    pub section: String,
    pub name: String,
    pub address: String,
    pub has_code: bool,
    pub code_hash: String,
    pub pinned: Option<String>,
    pub status: RegistryStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistryStatus {
    /// Terpin dan cocok. Satu satunya status yang benar benar hijau.
    Verified,
    /// Ada kode di alamat itu tapi hash belum pernah dipin.
    Unpinned,
    /// Tidak ada kode sama sekali di alamat itu.
    NoCode,
    /// Terpin tapi tidak cocok. Ini kegagalan keras.
    Mismatch,
}

pub struct ChainClient {
    cfg: RpcConfig,
    /// Endpoint aktif saat ini. Failover mengubah indeks ini.
    endpoint: RwLock<usize>,
    health: RwLock<ChainHealth>,
}

impl ChainClient {
    pub fn new(cfg: RpcConfig) -> Arc<Self> {
        let first = cfg.http.first().cloned().unwrap_or_default();
        let max_lag = cfg.max_block_lag_secs;
        Arc::new(Self {
            cfg,
            endpoint: RwLock::new(0),
            health: RwLock::new(ChainHealth::unknown(&first, max_lag)),
        })
    }

    pub async fn health(&self) -> ChainHealth {
        self.health.read().await.clone()
    }

    async fn current_endpoint(&self) -> String {
        let idx = *self.endpoint.read().await;
        self.cfg
            .http
            .get(idx)
            .or_else(|| self.cfg.http.first())
            .cloned()
            .unwrap_or_default()
    }

    /// Pindah ke endpoint berikutnya. Dipanggil setelah kegagalan.
    async fn rotate_endpoint(&self) {
        if self.cfg.http.len() <= 1 {
            return;
        }
        let mut idx = self.endpoint.write().await;
        *idx = (*idx + 1) % self.cfg.http.len();
        tracing::warn!(
            endpoint = %endpoint::aman(&self.cfg.http[*idx]),
            "beralih ke RPC cadangan"
        );
    }

    /// Penyedia RPC untuk endpoint yang sedang aktif.
    ///
    /// Dipakai modul lain yang perlu memanggil kontrak. Endpoint yang dipilih
    /// mengikuti failover, sehingga pemanggil tidak perlu tahu ada berapa
    /// endpoint cadangan.
    pub async fn provider(&self) -> Result<impl Provider> {
        let endpoint = self.current_endpoint().await;
        Self::provider_for(&endpoint)
    }

    /// Bangun penyedia dengan percobaan ulang otomatis.
    ///
    /// RPC publik Robinhood Chain membatasi laju dan menjawab 429 pada
    /// pemakaian yang sepenuhnya wajar, misalnya membaca saldo beberapa token
    /// berturut turut. Tanpa lapisan ini, kegagalan rate limit terlihat seperti
    /// kegagalan sungguhan, dan pemanggil yang menelannya akan melaporkan
    /// keadaan yang salah, misalnya saldo nol padahal sebenarnya ada.
    ///
    /// Ini pernah terjadi dan sempat menyesatkan. Lihat memory/003.
    fn provider_for(url: &str) -> Result<impl Provider> {
        use alloy::rpc::client::ClientBuilder;
        use alloy::transports::layers::RetryBackoffLayer;

        let parsed = url
            .parse()
            .with_context(|| format!("URL RPC `{url}` tidak valid"))?;

        // Lima percobaan, jeda awal 200 ms, dan anggaran laju yang longgar.
        // Angka ini dipilih supaya lonjakan pendek seperti membaca beberapa
        // saldo sekaligus tetap lewat, tanpa membuat kegagalan sungguhan
        // menggantung lama.
        let client = ClientBuilder::default()
            .layer(RetryBackoffLayer::new(5, 200, 1_000))
            .http(parsed);

        Ok(ProviderBuilder::new().connect_client(client))
    }

    /// Satu putaran pemeriksaan kesehatan. Memperbarui state internal.
    pub async fn check_health(&self) -> ChainHealth {
        let endpoint = self.current_endpoint().await;
        let result = self.probe(&endpoint).await;

        let health = match result {
            Ok(h) => h,
            Err(e) => {
                self.rotate_endpoint().await;
                ChainHealth {
                    healthy: false,
                    endpoint: endpoint::aman(&endpoint),
                    chain_id: None,
                    block_number: None,
                    block_lag_secs: None,
                    max_block_lag_secs: self.cfg.max_block_lag_secs,
                    error: Some(e.to_string()),
                    checked_at: chrono::Utc::now().to_rfc3339(),
                }
            }
        };

        *self.health.write().await = health.clone();
        health
    }

    async fn probe(&self, endpoint: &str) -> Result<ChainHealth> {
        let provider = Self::provider_for(endpoint)?;

        let chain_id = provider
            .get_chain_id()
            .await
            .context("RPC tidak menjawab eth_chainId")?;

        // Chain id yang tidak cocok berarti kita bicara dengan jaringan yang
        // salah. Ini tidak boleh dianggap sehat dalam kondisi apa pun.
        if chain_id != self.cfg.chain_id {
            anyhow::bail!(
                "chain id RPC {} tidak cocok dengan {} yang diharapkan. \
                 Periksa rpc.http di config.toml",
                chain_id,
                self.cfg.chain_id
            );
        }

        let block_number = provider
            .get_block_number()
            .await
            .context("RPC tidak menjawab eth_blockNumber")?;

        let lag = self.block_lag(&provider, block_number).await;

        let healthy = lag.is_none_or(|l| l <= self.cfg.max_block_lag_secs);

        Ok(ChainHealth {
            healthy,
            endpoint: endpoint::aman(endpoint),
            chain_id: Some(chain_id),
            block_number: Some(block_number),
            block_lag_secs: lag,
            max_block_lag_secs: self.cfg.max_block_lag_secs,
            error: if healthy {
                None
            } else {
                Some(format!(
                    "lag blok {} detik melebihi ambang {} detik",
                    lag.unwrap_or_default(),
                    self.cfg.max_block_lag_secs
                ))
            },
            checked_at: chrono::Utc::now().to_rfc3339(),
        })
    }

    /// Selisih antara waktu blok terakhir dan jam lokal.
    ///
    /// Mengembalikan `None` kalau blok tidak bisa dibaca. Ketidaktahuan tidak
    /// diperlakukan sebagai lag nol, tapi juga tidak menggagalkan kesehatan,
    /// karena beberapa RPC membatasi endpoint ini.
    async fn block_lag<P: Provider>(&self, provider: &P, number: u64) -> Option<u64> {
        let block = provider
            .get_block_by_number(number.into())
            .await
            .ok()
            .flatten()?;
        let ts = block.header.timestamp;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
        Some(now.saturating_sub(ts))
    }

    /// Gate G1: verifikasi setiap alamat di registry punya kode, dan hash-nya
    /// cocok dengan yang dipin.
    pub async fn verify_registry(&self, reg: &Registry) -> Result<Vec<RegistryCheck>> {
        let endpoint = self.current_endpoint().await;
        let provider = Self::provider_for(&endpoint)?;
        let mut out = Vec::new();

        for c in reg.all_contracts()? {
            let code = provider
                .get_code_at(c.address)
                .await
                .with_context(|| format!("gagal membaca kode di {}", c.address))?;

            let has_code = !code.is_empty();
            let hash = if has_code {
                hex::encode(keccak256(&code))
            } else {
                String::new()
            };

            let pinned = if c.code_hash.is_empty() {
                None
            } else {
                Some(c.code_hash.clone())
            };

            let status = match (&pinned, has_code) {
                (_, false) => RegistryStatus::NoCode,
                (None, true) => RegistryStatus::Unpinned,
                (Some(p), true) if p.eq_ignore_ascii_case(&hash) => RegistryStatus::Verified,
                (Some(_), true) => RegistryStatus::Mismatch,
            };

            out.push(RegistryCheck {
                section: c.section.to_string(),
                name: c.name,
                address: c.address.to_string(),
                has_code,
                code_hash: hash,
                pinned,
                status,
            });
        }

        Ok(out)
    }
}

/// Ringkas hasil G1 menjadi keputusan boleh jalan atau tidak.
///
/// Aturannya sengaja tegas: `Mismatch` menghentikan engine, karena artinya
/// kontrak di alamat itu bukan yang kita harapkan, dan mengirim transaksi ke
/// sana bisa berarti kehilangan dana.
pub fn registry_gate(checks: &[RegistryCheck]) -> Result<RegistryReport> {
    let mismatched: Vec<_> = checks
        .iter()
        .filter(|c| c.status == RegistryStatus::Mismatch)
        .collect();

    if !mismatched.is_empty() {
        let daftar = mismatched
            .iter()
            .map(|c| format!("{}.{} di {}", c.section, c.name, c.address))
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::bail!(
            "gate G1 gagal, bytecode tidak cocok dengan yang dipin untuk: {daftar}. \
             Engine tidak akan jalan. Jalankan `cargo xtask verify` untuk rinciannya"
        );
    }

    Ok(RegistryReport {
        total: checks.len(),
        verified: checks
            .iter()
            .filter(|c| c.status == RegistryStatus::Verified)
            .count(),
        unpinned: checks
            .iter()
            .filter(|c| c.status == RegistryStatus::Unpinned)
            .count(),
        no_code: checks
            .iter()
            .filter(|c| c.status == RegistryStatus::NoCode)
            .count(),
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct RegistryReport {
    pub total: usize,
    pub verified: usize,
    pub unpinned: usize,
    pub no_code: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(status: RegistryStatus) -> RegistryCheck {
        RegistryCheck {
            section: "uniswap_v3".into(),
            name: "factory".into(),
            address: "0x1f7d7550B1b028f7571E69a784071f0205fd2eFA".into(),
            has_code: status != RegistryStatus::NoCode,
            code_hash: "aabb".into(),
            pinned: None,
            status,
        }
    }

    #[test]
    fn mismatch_menghentikan_engine() {
        let checks = vec![check(RegistryStatus::Mismatch)];
        assert!(registry_gate(&checks).is_err());
    }

    #[test]
    fn unpinned_dan_no_code_tidak_menghentikan() {
        let checks = vec![
            check(RegistryStatus::Unpinned),
            check(RegistryStatus::NoCode),
        ];
        let report = registry_gate(&checks).unwrap();
        assert_eq!(report.total, 2);
        assert_eq!(report.unpinned, 1);
        assert_eq!(report.no_code, 1);
        assert_eq!(report.verified, 0);
    }

    #[test]
    fn semua_terverifikasi() {
        let checks = vec![check(RegistryStatus::Verified); 3];
        let report = registry_gate(&checks).unwrap();
        assert_eq!(report.verified, 3);
    }
    #[test]
    fn api_key_tidak_pernah_masuk_status_yang_dikirim_ke_panel() {
        // Ini penjaga struktural, bukan sekadar uji fungsi. Kalau suatu saat
        // ada yang menaruh URL mentah ke ChainHealth lagi, uji ini yang
        // menangkapnya sebelum key user ikut tercetak di layar.
        let h = ChainHealth::unknown(
            "https://robinhood-mainnet.g.alchemy.com/v2/kunci_rahasia_user",
            5,
        );
        assert!(
            !h.endpoint.contains("kunci_rahasia_user"),
            "API key bocor ke status: {}",
            h.endpoint
        );
        assert!(
            h.endpoint.contains("alchemy.com"),
            "host tetap harus terlihat supaya status kesehatan berguna"
        );
    }
}
