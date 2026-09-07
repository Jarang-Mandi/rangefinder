//! Perintah verifikasi Rangefinder.
//!
//! ```text
//! cargo xtask verify              periksa registry terhadap chain, lalu dokumen
//! cargo xtask verify --pin        periksa lalu tulis code_hash ke chain.toml
//! cargo xtask docs-check          hanya periksa kesesuaian kode dengan dokumen
//! cargo xtask golden [jumlah]     ambil golden vector dari transaksi sungguhan
//! cargo xtask dryrun <token>      jalankan seluruh pipeline tanpa mengirim
//! ```
//!
//! INVARIAN
//! - Perintah ini yang menjaga dokumen tetap jujur. Aturan sinkronisasi di
//!   PRD 13.1 menyebutnya secara eksplisit, jadi ia tidak boleh dilewati
//!   hanya karena sedang buru buru.
//! - `--pin` menulis ulang chain.toml dengan penggantian berbasis baris, BUKAN
//!   dengan menyerialkan ulang struktur. Menyerialkan ulang akan menghapus
//!   seluruh komentar di berkas itu, padahal komentarnya memuat peringatan
//!   penting tentang UniversalRouter yang merupakan fork.

mod dryrun;
mod pairs;
mod v4probe;
mod zap;
mod golden;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rangefinder_engine::chain::registry::Registry;
use rangefinder_engine::chain::{registry_gate, ChainClient, RegistryStatus};
use rangefinder_engine::config::Config;

fn main() -> Result<()> {
    // Tanpa ini, setiap tracing::warn di dalam engine hilang tanpa jejak saat
    // dipanggil dari xtask. Itu persis bagaimana bug pasangan WETH bisa
    // melaporkan nol pool tanpa satu pun pesan. Lihat memory/003 dan 005.
    rangefinder_engine::telemetry::init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("verify");
    let pin = args.iter().any(|a| a == "--pin");

    let root = repo_root();

    match cmd {
        "verify" => {
            let rt = tokio::runtime::Runtime::new()?;
            let chain_ok = rt.block_on(verify_chain(&root, pin))?;
            let docs_ok = check_docs(&root)?;
            finish(chain_ok && docs_ok)
        }
        "docs-check" => {
            let ok = check_docs(&root)?;
            finish(ok)
        }
        "dryrun" => {
            let token = args.get(1).cloned().unwrap_or_default();
            if token.is_empty() {
                eprintln!("cara pakai: cargo xtask dryrun <alamat_token> [alamat_pengirim]");
                std::process::exit(2);
            }
            let from = args.get(2).cloned();
            let rt = tokio::runtime::Runtime::new()?;
            let ok = rt.block_on(dryrun::run(&root, &token, from.as_deref()))?;
            finish(ok)
        }
        "zap" => {
            let token = args.get(1).cloned().unwrap_or_default();
            if token.is_empty() {
                eprintln!("cara pakai: cargo xtask zap <alamat_token> [nominal]");
                std::process::exit(2);
            }
            let nominal = args.get(2).cloned();
            let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
            let ok = rt.block_on(zap::run(&root, &token, nominal.as_deref()))?;
            finish(ok)
        }
        "v4probe" => {
            let rt = tokio::runtime::Runtime::new()?;
            let ok = rt.block_on(v4probe::run(&root))?;
            finish(ok)
        }
        "pairs" => {
            let token = args.get(1).cloned().unwrap_or_default();
            if token.is_empty() {
                eprintln!("cara pakai: cargo xtask pairs <alamat_token>");
                std::process::exit(2);
            }
            let rt = tokio::runtime::Runtime::new()?;
            let ok = rt.block_on(pairs::run(&root, &token))?;
            finish(ok)
        }
        "golden" => {
            let jumlah = args
                .get(1)
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(3);
            let rt = tokio::runtime::Runtime::new()?;
            let ok = rt.block_on(golden::capture(&root, jumlah))?;
            finish(ok)
        }
        other => {
            eprintln!("perintah `{other}` tidak dikenal. Yang ada: verify, docs-check, golden, dryrun, pairs");
            std::process::exit(2);
        }
    }
}

fn finish(ok: bool) -> Result<()> {
    if ok {
        println!("\n  HIJAU. Semua pemeriksaan lolos.");
        Ok(())
    } else {
        eprintln!("\n  MERAH. Ada pemeriksaan yang gagal, lihat rincian di atas.");
        std::process::exit(1);
    }
}

fn repo_root() -> PathBuf {
    // xtask ada di <root>/xtask, jadi akarnya satu tingkat di atas.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask selalu punya direktori induk")
        .to_path_buf()
}

// ─── Verifikasi registry terhadap chain ─────────────────────────

async fn verify_chain(root: &Path, pin: bool) -> Result<bool> {
    println!("  Registry kontrak");
    println!("  ----------------");

    let path = Registry::default_path(root);
    let registry = Registry::load_from(&path)?;
    let cfg = Config::load().context("gagal memuat config engine untuk mengambil URL RPC")?;

    println!("  chain    {} (id {})", registry.chain.name, registry.chain.id);
    println!("  rpc      {}", cfg.rpc.http.first().cloned().unwrap_or_default());
    println!("  berkas   {}", path.display());
    println!();

    let client = ChainClient::new(cfg.rpc.clone());

    // RPC publik dibatasi laju dan chain-nya memang sering tersendat, jadi satu
    // kegagalan bukan alasan menggagalkan verifikasi. Tanpa percobaan ulang,
    // perintah ini gagal secara acak, dan perintah yang gagal secara acak akan
    // berhenti dipercaya lalu dilewati orang.
    const PERCOBAAN: u32 = 3;
    let mut health = client.check_health().await;
    for percobaan in 1..PERCOBAAN {
        if health.healthy {
            break;
        }
        eprintln!(
            "  RPC belum sehat ({}), mencoba lagi {} dari {}",
            health.error.as_deref().unwrap_or("alasan tidak diketahui"),
            percobaan + 1,
            PERCOBAAN
        );
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        health = client.check_health().await;
    }

    if !health.healthy {
        eprintln!(
            "  RPC tidak sehat setelah {PERCOBAAN} percobaan: {}",
            health.error.as_deref().unwrap_or("alasan tidak diketahui")
        );
        eprintln!("  Verifikasi registry butuh RPC yang jalan. Coba lagi nanti.");
        return Ok(false);
    }

    let checks = client.verify_registry(&registry).await?;

    let mut pins: Vec<(String, String)> = Vec::new();
    for c in &checks {
        let tanda = match c.status {
            RegistryStatus::Verified => "cocok  ",
            RegistryStatus::Unpinned => "belum  ",
            RegistryStatus::NoCode => "KOSONG ",
            RegistryStatus::Mismatch => "BEDA   ",
        };
        println!(
            "  {tanda} {:<12} {:<22} {}",
            c.section,
            c.name,
            short_hash(&c.code_hash)
        );
        if c.status == RegistryStatus::Unpinned {
            pins.push((c.address.clone(), c.code_hash.clone()));
        }
    }
    println!();

    // Gate G1. Mismatch adalah kegagalan keras, sama seperti di engine.
    let report = match registry_gate(&checks) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("  {e}");
            return Ok(false);
        }
    };

    println!(
        "  total {}, cocok {}, belum dipin {}, tanpa kode {}",
        report.total, report.verified, report.unpinned, report.no_code
    );

    if pin && !pins.is_empty() {
        let jumlah = pin_hashes(&path, &pins)?;
        println!("  {jumlah} code_hash ditulis ke chain.toml");
    } else if !pins.is_empty() {
        println!("  jalankan ulang dengan --pin untuk mengunci {} hash", pins.len());
    }

    // Alamat tanpa kode adalah kegagalan: artinya alamat di chain.toml salah,
    // atau kontraknya belum ter-deploy di chain ini.
    if report.no_code > 0 {
        eprintln!(
            "  {} alamat tidak punya bytecode. Periksa terhadap explorer, lihat docs/CHAIN.md",
            report.no_code
        );
        return Ok(false);
    }

    Ok(true)
}

fn short_hash(h: &str) -> String {
    if h.is_empty() {
        "-".into()
    } else if h.len() > 16 {
        format!("{}...{}", &h[..8], &h[h.len() - 8..])
    } else {
        h.to_string()
    }
}

/// Tulis `code_hash` ke baris yang memuat alamat bersangkutan.
///
/// Penggantian berbasis baris supaya seluruh komentar di chain.toml utuh.
fn pin_hashes(path: &Path, pins: &[(String, String)]) -> Result<usize> {
    let text = std::fs::read_to_string(path)?;
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut ditulis = 0usize;

    for (address, hash) in pins {
        if hash.is_empty() {
            continue;
        }
        let needle = address.to_lowercase();
        for line in lines.iter_mut() {
            if !line.to_lowercase().contains(&needle) {
                continue;
            }
            if let Some(pos) = line.find("code_hash = \"\"") {
                let ganti = format!("code_hash = \"{hash}\"");
                line.replace_range(pos..pos + "code_hash = \"\"".len(), &ganti);
                ditulis += 1;
            } else if !line.contains("code_hash") {
                // Entri tanpa field code_hash sama sekali. Sisipkan sebelum
                // kurung tutup tabel inline.
                if let Some(pos) = line.rfind('}') {
                    let ganti = format!(", code_hash = \"{hash}\" ");
                    line.insert_str(pos, &ganti);
                    ditulis += 1;
                }
            }
            break;
        }
    }

    if ditulis > 0 {
        let mut out = lines.join("\n");
        if text.ends_with('\n') {
            out.push('\n');
        }
        std::fs::write(path, out)?;
    }
    Ok(ditulis)
}

// ─── Pemeriksaan dokumen ────────────────────────────────────────

/// Pastikan apa yang ada di kode juga ada di PRD.
///
/// Arahnya sengaja satu: setiap hal yang diimplementasikan WAJIB terdokumentasi.
/// Sebaliknya tidak diwajibkan, karena PRD memang memuat rencana fase berikutnya
/// yang belum ditulis kodenya.
fn check_docs(root: &Path) -> Result<bool> {
    println!();
    println!("  Kesesuaian kode dengan dokumen");
    println!("  ------------------------------");

    let prd = std::fs::read_to_string(root.join("docs/PRD.md")).context("docs/PRD.md tidak ada")?;
    let protocol = std::fs::read_to_string(root.join("engine/src/api/protocol.rs"))
        .context("engine/src/api/protocol.rs tidak ada")?;

    let mut ok = true;

    // 1. Setiap varian pesan protokol harus disebut di PRD.
    let mut hilang = Vec::new();
    for name in extract_message_variants(&protocol) {
        if prd.contains(&name) {
            println!("  ada    pesan {name}");
        } else {
            println!("  HILANG pesan {name}");
            hilang.push(name);
        }
    }
    if !hilang.is_empty() {
        eprintln!(
            "  {} pesan protokol belum tercantum di PRD bagian 7.3: {}",
            hilang.len(),
            hilang.join(", ")
        );
        ok = false;
    }

    // 2. Berkas dokumen wajib harus ada.
    for doc in [
        "CLAUDE.md",
        "docs/PRD.md",
        "docs/DECISIONS.md",
        "docs/GMGN.md",
        "docs/CHAIN.md",
        "docs/RUNBOOK.md",
    ] {
        if root.join(doc).exists() {
            println!("  ada    dokumen {doc}");
        } else {
            println!("  HILANG dokumen {doc}");
            ok = false;
        }
    }

    Ok(ok)
}

/// Ambil nama varian dari enum `ClientMessage` dan `ServerMessage`, lalu
/// ubah ke snake_case seperti yang dihasilkan serde.
fn extract_message_variants(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut di_dalam_enum = false;
    let mut kedalaman = 0i32;

    for baris in source.lines() {
        let t = baris.trim();

        if t.starts_with("pub enum ClientMessage") || t.starts_with("pub enum ServerMessage") {
            di_dalam_enum = true;
            kedalaman = 0;
            continue;
        }
        if !di_dalam_enum {
            continue;
        }

        kedalaman += t.matches('{').count() as i32;
        kedalaman -= t.matches('}').count() as i32;
        if kedalaman < 0 {
            di_dalam_enum = false;
            continue;
        }

        // Varian selalu diawali huruf kapital dan berada di kedalaman nol.
        if kedalaman == 0 || t.ends_with('{') {
            if let Some(first) = t.chars().next() {
                if first.is_ascii_uppercase() {
                    let nama: String = t
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric())
                        .collect();
                    if !nama.is_empty() {
                        out.push(to_snake_case(&nama));
                    }
                }
            }
        }
    }

    out.sort();
    out.dedup();
    out
}

fn to_snake_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_case_dasar() {
        assert_eq!(to_snake_case("EngineStatus"), "engine_status");
        assert_eq!(to_snake_case("Ping"), "ping");
        assert_eq!(to_snake_case("GetStatus"), "get_status");
    }

    #[test]
    fn mengambil_varian_dari_sumber() {
        let src = r#"
#[derive(Debug)]
pub enum ClientMessage {
    GetStatus,
    Ping,
}

#[derive(Debug)]
pub enum ServerMessage {
    Hello {
        engine_version: String,
    },
    Pong,
}
"#;
        let v = extract_message_variants(src);
        assert!(v.contains(&"get_status".to_string()));
        assert!(v.contains(&"ping".to_string()));
        assert!(v.contains(&"hello".to_string()));
        assert!(v.contains(&"pong".to_string()));
        // Field di dalam varian tidak boleh ikut terambil.
        assert!(!v.contains(&"engine_version".to_string()));
    }

    #[test]
    fn hash_pendek_dipersingkat() {
        assert_eq!(short_hash(""), "-");
        assert_eq!(short_hash("abcd"), "abcd");
        let panjang = "0123456789abcdef0123456789abcdef";
        assert_eq!(short_hash(panjang), "01234567...89abcdef");
    }
}
