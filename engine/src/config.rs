//! Konfigurasi engine dan lokasi berkas.
//!
//! INVARIAN
//! - Engine HANYA boleh mengikat ke alamat loopback. `Config::load` menolak
//!   alamat lain, dan itu bukan sekadar bawaan yang bisa ditimpa. Lihat PRD 9.3.
//! - Alamat kontrak tidak pernah ditulis di sini. Sumber tunggalnya adalah
//!   `chain.toml` di akar repo, dibaca `chain::registry`, supaya engine dan
//!   `cargo xtask verify` memakai daftar yang sama persis.
//! - Pembatas nominal bawaan memakai profil modal di bawah 50 USD. Profil ini
//!   menjaga satu posisi kecil, membatasi laju penempatan, dan menyisakan gas.

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub rpc: RpcConfig,
    #[serde(default)]
    pub wallet: WalletConfig,
    #[serde(default)]
    pub limits: LimitsConfig,
    #[serde(default)]
    pub execution: ExecutionConfig,
}

/// Pengaturan eksekusi transaksi.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionConfig {
    /// Kalau benar, engine menyusun dan menyimulasikan transaksi tapi TIDAK
    /// pernah mengirimnya.
    ///
    /// Menyala secara bawaan, dan itu disengaja. Proyek ini belum pernah
    /// mengirim satu transaksi pun ke rantai. Mengirim sungguhan harus jadi
    /// tindakan sadar yang dinyalakan user, bukan bawaan yang kebetulan aktif
    /// lalu memindahkan dana pada percobaan pertama.
    pub dry_run: bool,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self { dry_run: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 8787,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpcConfig {
    /// Endpoint utama untuk pemeriksaan kesehatan dan pengiriman transaksi.
    /// Yang pertama dipakai, sisanya cadangan.
    pub http: Vec<String>,
    /// Endpoint baca cepat. Setiap permintaan mulai dari endpoint yang terakhir
    /// berhasil lalu langsung mencoba endpoint berikutnya bila gagal.
    ///
    /// Kosong berarti memakai `http`. Pemisahan ini menjaga RPC transaksi dari
    /// beban metadata, saldo, harga, dan state pool.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read_http: Vec<String>,
    /// Endpoint khusus pemindaian log bersejarah. Kosong berarti memakai
    /// endpoint interaktif yang sedang aktif.
    ///
    /// Pemisahan ini penting karena endpoint cepat untuk `eth_call` belum
    /// tentu mengizinkan rentang `eth_getLogs` yang lebar.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub logs_http: Vec<String>,
    /// Endpoint WebSocket, opsional. Kosong berarti hanya polling HTTP.
    #[serde(default)]
    pub ws: Vec<String>,
    /// Chain id yang diharapkan. Engine menolak jalan jika RPC menjawab beda.
    pub chain_id: u64,
    /// Ambang lag blok dalam detik. Di atas ini eksekusi dinonaktifkan.
    pub max_block_lag_secs: u64,
    /// Selang pemeriksaan kesehatan dalam detik.
    pub health_interval_secs: u64,
}

/// Jalur pemakaian sebuah endpoint RPC.
///
/// Satu URL boleh dipasang pada lebih dari satu jalur. Identitas endpoint
/// menyertakan jalurnya supaya menghapus endpoint log tidak ikut menghapus URL
/// yang sama dari jalur transaksi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RpcPurpose {
    Transaction,
    Read,
    Logs,
}

impl RpcPurpose {
    pub fn label(self) -> &'static str {
        match self {
            Self::Transaction => "transaction",
            Self::Read => "read",
            Self::Logs => "logs",
        }
    }
}

impl RpcConfig {
    pub fn endpoints(&self, purpose: RpcPurpose) -> &[String] {
        match purpose {
            RpcPurpose::Transaction => &self.http,
            RpcPurpose::Read => &self.read_http,
            RpcPurpose::Logs => &self.logs_http,
        }
    }

    fn endpoints_mut(&mut self, purpose: RpcPurpose) -> &mut Vec<String> {
        match purpose {
            RpcPurpose::Transaction => &mut self.http,
            RpcPurpose::Read => &mut self.read_http,
            RpcPurpose::Logs => &mut self.logs_http,
        }
    }

    pub fn add_endpoint(&mut self, purpose: RpcPurpose, endpoint: &str) -> Result<()> {
        let endpoint = endpoint.trim();
        if !(endpoint.starts_with("http://") || endpoint.starts_with("https://")) {
            bail!("`{endpoint}` bukan endpoint HTTP");
        }
        let list = self.endpoints_mut(purpose);
        if list.iter().any(|item| item.eq_ignore_ascii_case(endpoint)) {
            bail!("endpoint itu sudah ada pada jalur {}", purpose.label());
        }
        list.push(endpoint.to_owned());
        Ok(())
    }

    pub fn remove_endpoint(&mut self, id: &str) -> Result<RpcPurpose> {
        for purpose in [RpcPurpose::Transaction, RpcPurpose::Read, RpcPurpose::Logs] {
            let list = self.endpoints_mut(purpose);
            if let Some(index) = list
                .iter()
                .position(|url| rpc_endpoint_id(purpose, url) == id)
            {
                if purpose == RpcPurpose::Transaction && list.len() == 1 {
                    bail!("minimal satu RPC transaksi harus tetap ada");
                }
                list.remove(index);
                return Ok(purpose);
            }
        }
        bail!("endpoint RPC tidak ditemukan; muat ulang Settings")
    }
}

/// Identitas aman untuk operasi hapus. Hash ini tidak membuka URL atau API key.
pub fn rpc_endpoint_id(purpose: RpcPurpose, endpoint: &str) -> String {
    let material = format!("{}:{}", purpose.label(), endpoint.trim());
    format!("{:#x}", alloy::primitives::keccak256(material.as_bytes()))
}

impl Default for RpcConfig {
    fn default() -> Self {
        Self {
            http: vec!["https://rpc.mainnet.chain.robinhood.com".into()],
            read_http: vec![
                "https://robinhood-rpc.publicnode.com".into(),
                "https://rpc.ordofi.network".into(),
                "https://rpc.mainnet.chain.robinhood.com".into(),
            ],
            logs_http: vec![],
            ws: vec![],
            chain_id: 4663,
            max_block_lag_secs: 5,
            health_interval_secs: 5,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalletConfig {
    /// Menit tanpa aktivitas sebelum wallet mengunci diri. Nol berarti sesi
    /// lokal tetap terbuka selama engine hidup.
    pub auto_lock_minutes: u64,
    /// Simpan passphrase di Windows Credential Manager supaya engine bisa
    /// membuka wallet lagi sesudah restart tanpa meminta input berulang.
    pub use_credential_manager: bool,
}

impl Default for WalletConfig {
    fn default() -> Self {
        Self {
            auto_lock_minutes: 30,
            use_credential_manager: false,
        }
    }
}

/// Pembatas risiko. `None` berarti tidak terbatas.
///
/// Nilai bawaan dibuat untuk modal operasional di bawah 50 USD. Pembatas tetap
/// berupa konfigurasi agar dapat diubah tanpa mengompilasi ulang engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LimitsConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_position_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_daily_deploy_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_open_positions: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_pool_first_size_usd: Option<f64>,
    /// Saldo ETH yang selalu disisakan untuk gas. Aktif secara bawaan.
    pub min_eth_reserve: f64,
    /// Batas slippage zap dalam basis poin. Aktif secara bawaan.
    pub max_slippage_bps: u32,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            max_position_usd: Some(5.0),
            max_daily_deploy_usd: Some(5.0),
            max_open_positions: Some(2),
            new_pool_first_size_usd: Some(2.5),
            min_eth_reserve: 0.002,
            max_slippage_bps: 200,
        }
    }
}

impl Config {
    /// Baca `config.toml` dari direktori data. Kalau belum ada, tulis bawaan
    /// supaya user punya berkas yang bisa disunting, lalu pakai bawaan itu.
    pub fn load() -> Result<Self> {
        let dir = data_dir()?;
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("gagal membuat direktori data {}", dir.display()))?;

        let path = dir.join("config.toml");
        let cfg: Config = if path.exists() {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("gagal membaca {}", path.display()))?;
            toml::from_str(&text).with_context(|| format!("{} tidak valid", path.display()))?
        } else {
            let cfg = Config::default();
            let text = toml::to_string_pretty(&cfg)?;
            std::fs::write(&path, text)
                .with_context(|| format!("gagal menulis {}", path.display()))?;
            tracing::info!(path = %path.display(), "config bawaan dibuat");
            cfg
        };

        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<()> {
        let ip: IpAddr = self
            .server
            .host
            .parse()
            .with_context(|| format!("host `{}` bukan alamat IP", self.server.host))?;

        // Invarian keamanan, bukan preferensi. Lihat PRD 9.3.
        if !ip.is_loopback() {
            bail!(
                "engine hanya boleh mengikat ke loopback, tapi config meminta `{}`. \
                 Ubah server.host menjadi 127.0.0.1",
                self.server.host
            );
        }

        if self.rpc.http.is_empty() {
            bail!("rpc.http tidak boleh kosong, minimal satu endpoint dibutuhkan");
        }

        if self.rpc.chain_id == 0 {
            bail!("rpc.chain_id tidak boleh nol");
        }

        if self.limits.max_slippage_bps > 5_000 {
            bail!(
                "limits.max_slippage_bps = {} terlalu longgar, batas atas yang masuk akal 5000 (50 persen)",
                self.limits.max_slippage_bps
            );
        }

        Ok(())
    }

    pub fn bind_addr(&self) -> Result<SocketAddr> {
        let ip: IpAddr = self.server.host.parse()?;
        Ok(SocketAddr::new(ip, self.server.port))
    }
}

/// Direktori data engine. Bisa ditimpa lewat `RANGEFINDER_DATA_DIR`,
/// yang dipakai tes integrasi supaya tidak menyentuh data asli user.
/// Sunting satu endpoint di `config.toml` dan kembalikan konfigurasi barunya.
/// Bagian di luar `[rpc]` tidak disentuh.
pub fn ubah_rpc(
    purpose: Option<RpcPurpose>,
    endpoint: Option<&str>,
    remove_id: Option<&str>,
) -> Result<(PathBuf, RpcConfig)> {
    let path = data_dir()?.join("config.toml");
    let teks = std::fs::read_to_string(&path)
        .with_context(|| format!("gagal membaca {}", path.display()))?;
    let mut config: Config =
        toml::from_str(&teks).with_context(|| format!("{} tidak valid", path.display()))?;
    match (purpose, endpoint, remove_id) {
        (Some(purpose), Some(endpoint), None) => config.rpc.add_endpoint(purpose, endpoint)?,
        (None, None, Some(id)) => {
            config.rpc.remove_endpoint(id)?;
        }
        _ => bail!("perubahan RPC tidak valid"),
    }
    config.validate()?;
    let keluaran = toml::to_string_pretty(&config)?;
    std::fs::write(&path, keluaran).with_context(|| format!("gagal menulis {}", path.display()))?;
    Ok((path, config.rpc))
}

pub fn data_dir() -> Result<PathBuf> {
    if let Ok(custom) = std::env::var("RANGEFINDER_DATA_DIR") {
        return Ok(PathBuf::from(custom));
    }
    let base = dirs::data_local_dir()
        .context("tidak bisa menentukan direktori data lokal untuk platform ini")?;
    Ok(base.join("Rangefinder"))
}

/// Akar repo, dipakai untuk menemukan `chain.toml`.
///
/// Saat dijalankan lewat cargo, `CARGO_MANIFEST_DIR` menunjuk ke `engine/`,
/// jadi akarnya satu tingkat di atas. Saat dijalankan sebagai biner lepas,
/// pakai `RANGEFINDER_REPO_ROOT` atau direktori kerja.
pub fn repo_root() -> PathBuf {
    if let Ok(custom) = std::env::var("RANGEFINDER_REPO_ROOT") {
        return PathBuf::from(custom);
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| manifest.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_bawaan_bisa_ditulis_sebagai_toml() {
        // Ini yang menangkap `UnsupportedNoneValue`. Tanpa uji ini, kegagalannya
        // baru terlihat saat engine pertama kali dijalankan di mesin bersih.
        let cfg = Config::default();
        let text = toml::to_string_pretty(&cfg).expect("config bawaan harus bisa diserialkan");
        assert!(text.contains("[server]"));
        assert!(text.contains("[limits]"));
        assert!(text.contains("max_position_usd = 5.0"));
        assert!(text.contains("max_open_positions = 2"));
    }

    #[test]
    fn config_bawaan_bolak_balik_utuh() {
        let cfg = Config::default();
        let text = toml::to_string_pretty(&cfg).unwrap();
        let kembali: Config = toml::from_str(&text).unwrap();

        assert_eq!(kembali.server.port, cfg.server.port);
        assert_eq!(kembali.rpc.chain_id, cfg.rpc.chain_id);
        assert_eq!(kembali.rpc.read_http.len(), 3);
        assert!(kembali.rpc.logs_http.is_empty());
        assert_eq!(kembali.limits.max_slippage_bps, cfg.limits.max_slippage_bps);
        assert_eq!(kembali.limits.max_position_usd, Some(5.0));
        assert_eq!(kembali.limits.max_daily_deploy_usd, Some(5.0));
    }

    #[test]
    fn profil_bawaan_melindungi_modal_di_bawah_lima_puluh_dolar() {
        let l = LimitsConfig::default();
        assert_eq!(l.max_position_usd, Some(5.0));
        assert_eq!(l.max_daily_deploy_usd, Some(5.0));
        assert_eq!(l.max_open_positions, Some(2));
        assert_eq!(l.new_pool_first_size_usd, Some(2.5));
        assert_eq!(l.min_eth_reserve, 0.002);
        assert_eq!(l.max_slippage_bps, 200);
    }

    #[test]
    fn host_bukan_loopback_ditolak() {
        let mut cfg = Config::default();
        cfg.server.host = "0.0.0.0".into();
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("loopback"), "pesan error: {err}");

        cfg.server.host = "192.168.1.10".into();
        assert!(cfg.validate().is_err());

        // Loopback lain selain 127.0.0.1 tetap boleh.
        cfg.server.host = "127.0.0.2".into();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn config_tidak_masuk_akal_ditolak() {
        let mut cfg = Config::default();
        cfg.rpc.http.clear();
        assert!(cfg.validate().is_err());

        let mut cfg = Config::default();
        cfg.rpc.chain_id = 0;
        assert!(cfg.validate().is_err());

        let mut cfg = Config::default();
        cfg.limits.max_slippage_bps = 9_000;
        assert!(cfg.validate().is_err());

        let mut cfg = Config::default();
        cfg.server.host = "bukan-ip".into();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn dry_run_menyala_secara_bawaan() {
        // Kalau uji ini gagal, engine akan mengirim transaksi sungguhan pada
        // percobaan pertama di mesin bersih. Lihat komentar di ExecutionConfig.
        assert!(Config::default().execution.dry_run);
        assert!(ExecutionConfig::default().dry_run);

        // Dan config yang tidak menyebut bagian execution sama sekali tetap
        // mendapat dry run menyala, bukan mati.
        let cfg: Config = toml::from_str("[server]\nhost = \"127.0.0.1\"\nport = 8787").unwrap();
        assert!(cfg.execution.dry_run, "bagian yang hilang harus tetap aman");
    }

    #[test]
    fn config_bawaan_menunjuk_robinhood_chain() {
        let cfg = Config::default();
        assert_eq!(cfg.rpc.chain_id, 4663);
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.bind_addr().unwrap().port(), 8787);
    }

    #[test]
    fn rpc_bisa_ditambah_dan_dihapus_per_jalur() {
        let mut rpc = RpcConfig::default();
        rpc.add_endpoint(RpcPurpose::Logs, "https://logs.example")
            .unwrap();
        rpc.add_endpoint(RpcPurpose::Read, "https://read.example")
            .unwrap();
        let id = rpc_endpoint_id(RpcPurpose::Logs, "https://logs.example");
        assert_eq!(rpc.remove_endpoint(&id).unwrap(), RpcPurpose::Logs);
        assert!(rpc.logs_http.is_empty());
        assert!(rpc
            .read_http
            .iter()
            .any(|url| url == "https://read.example"));
    }

    #[test]
    fn rpc_transaksi_terakhir_tidak_bisa_dihapus() {
        let mut rpc = RpcConfig::default();
        let id = rpc_endpoint_id(RpcPurpose::Transaction, &rpc.http[0]);
        assert!(rpc.remove_endpoint(&id).is_err());
        assert_eq!(rpc.http.len(), 1);
    }

    #[test]
    fn rpc_menolak_skema_dan_duplikat() {
        let mut rpc = RpcConfig::default();
        assert!(rpc
            .add_endpoint(RpcPurpose::Read, "ws://bukan-http.example")
            .is_err());
        let existing = rpc.read_http[0].clone();
        assert!(rpc.add_endpoint(RpcPurpose::Read, &existing).is_err());
    }
}
