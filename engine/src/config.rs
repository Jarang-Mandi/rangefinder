//! Konfigurasi engine dan lokasi berkas.
//!
//! INVARIAN
//! - Engine HANYA boleh mengikat ke alamat loopback. `Config::load` menolak
//!   alamat lain, dan itu bukan sekadar bawaan yang bisa ditimpa. Lihat PRD 9.3.
//! - Alamat kontrak tidak pernah ditulis di sini. Sumber tunggalnya adalah
//!   `chain.toml` di akar repo, dibaca `chain::registry`, supaya engine dan
//!   `cargo xtask verify` memakai daftar yang sama persis.
//! - Pembatas nominal bawaannya tidak terbatas. Ini keputusan user yang sudah
//!   dikunci, lihat PRD 9.2. Mekanismenya tetap ada supaya bisa dinyalakan.

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpcConfig {
    /// Endpoint HTTP berurutan. Yang pertama dipakai, sisanya cadangan.
    pub http: Vec<String>,
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

impl Default for RpcConfig {
    fn default() -> Self {
        Self {
            http: vec!["https://rpc.mainnet.chain.robinhood.com".into()],
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
    /// Menit tanpa aktivitas sebelum wallet mengunci diri.
    pub auto_lock_minutes: u64,
    /// Simpan passphrase di Windows Credential Manager. Mati secara bawaan.
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
/// Empat pembatas nominal bawaannya `None` atas keputusan user. Dua pembatas
/// terakhir tetap aktif karena sifatnya operasional, bukan pilihan strategi.
/// CATATAN SERIALISASI
/// Crate `toml` tidak bisa menserialkan `None`, ia melempar `UnsupportedNoneValue`.
/// Karena keempat pembatas di bawah bawaannya `None`, tanpa `skip_serializing_if`
/// penulisan config.toml bawaan akan gagal pada saat start pertama. Field yang
/// dilewati tetap terbaca sebagai `None` saat dimuat, karena semuanya `default`.
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
            max_position_usd: None,
            max_daily_deploy_usd: None,
            max_open_positions: None,
            new_pool_first_size_usd: None,
            min_eth_reserve: 0.002,
            max_slippage_bps: 300,
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
/// Tulis ulang daftar endpoint HTTP di `config.toml`.
///
/// KENAPA HANYA MENULIS, TIDAK MENERAPKAN LANGSUNG
/// `ChainClient` dibangun sekali saat start dan dipegang seluruh engine. Menukar
/// endpointnya saat sedang jalan berarti ada permintaan yang berangkat ke satu
/// RPC dan jawabannya ditunggu dari yang lain, dan itu bukan jenis kekeliruan
/// yang layak ditanggung demi menghindari satu kali start ulang.
///
/// Berkasnya dibaca ulang, disunting bagian `rpc.http` saja, lalu ditulis
/// kembali. Bagian lain tidak disentuh, karena user bisa saja sudah menyunting
/// batas nominal atau `dry_run` sendiri, dan menimpanya diam diam akan
/// mengembalikan `dry_run` ke keadaan bawaan tanpa ada yang memberi tahu.
pub fn tulis_rpc(http: &[String]) -> Result<PathBuf> {
    let bersih: Vec<String> = http
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    if bersih.is_empty() {
        anyhow::bail!("daftar endpoint kosong, engine tidak akan bisa jalan");
    }
    for e in &bersih {
        if !(e.starts_with("http://") || e.starts_with("https://")) {
            anyhow::bail!("`{e}` bukan endpoint HTTP");
        }
    }

    let path = data_dir()?.join("config.toml");
    let teks = std::fs::read_to_string(&path)
        .with_context(|| format!("gagal membaca {}", path.display()))?;
    let mut doc: toml::Value = toml::from_str(&teks)
        .with_context(|| format!("{} tidak valid", path.display()))?;

    let rpc = doc
        .get_mut("rpc")
        .and_then(|v| v.as_table_mut())
        .ok_or_else(|| anyhow::anyhow!("bagian [rpc] tidak ada di config.toml"))?;
    rpc.insert(
        "http".to_string(),
        toml::Value::Array(bersih.into_iter().map(toml::Value::String).collect()),
    );

    let keluaran = toml::to_string_pretty(&doc)?;
    std::fs::write(&path, keluaran)
        .with_context(|| format!("gagal menulis {}", path.display()))?;

    Ok(path)
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
        // Pembatas yang tidak terbatas memang sengaja tidak muncul di berkas.
        assert!(!text.contains("max_position_usd"));
    }

    #[test]
    fn config_bawaan_bolak_balik_utuh() {
        let cfg = Config::default();
        let text = toml::to_string_pretty(&cfg).unwrap();
        let kembali: Config = toml::from_str(&text).unwrap();

        assert_eq!(kembali.server.port, cfg.server.port);
        assert_eq!(kembali.rpc.chain_id, cfg.rpc.chain_id);
        assert_eq!(kembali.limits.max_slippage_bps, cfg.limits.max_slippage_bps);
        // Field yang dilewati saat menulis harus kembali sebagai None.
        assert!(kembali.limits.max_position_usd.is_none());
        assert!(kembali.limits.max_daily_deploy_usd.is_none());
    }

    #[test]
    fn pembatas_nominal_bawaannya_tidak_terbatas() {
        // Keputusan user yang sudah dikunci, lihat PRD 9.2. Kalau uji ini gagal,
        // seseorang mengubah kebijakan tanpa membaca dokumen.
        let l = LimitsConfig::default();
        assert!(l.max_position_usd.is_none());
        assert!(l.max_daily_deploy_usd.is_none());
        assert!(l.max_open_positions.is_none());
        assert!(l.new_pool_first_size_usd.is_none());
        // Dua ini sifatnya operasional dan tetap aktif.
        assert!(l.min_eth_reserve > 0.0);
        assert!(l.max_slippage_bps > 0);
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
}
