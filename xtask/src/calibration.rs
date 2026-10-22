//! Operasi manual loop self-improvement pada basis data lokal.
//!
//! Cara pakai:
//! `cargo xtask calibration status`
//! `cargo xtask calibration train [sideways|dump_basis|trend|volatile|unknown]`
//! `cargo xtask calibration replay [version]`
//! `cargo xtask calibration replay-bars <bars.json> [version]`
//! `cargo xtask calibration rollback <version>`

use anyhow::{bail, Context, Result};

pub fn run(args: &[String]) -> Result<bool> {
    let data_dir = rangefinder_engine::config::data_dir()?;
    let store = rangefinder_engine::store::Store::open(&data_dir).with_context(|| {
        format!(
            "basis data {} tidak bisa dibuka. Hentikan engine sebelum menjalankan perintah kalibrasi",
            data_dir.display()
        )
    })?;
    let action = args.first().map(String::as_str).unwrap_or("status");
    match action {
        "status" => {
            let view = rangefinder_engine::learning::view(&store, &[], &[])?;
            println!("  Rekaman       {}", view.records);
            println!("  Sudah ditutup {}", view.closed_records);
            if view.calibrations.is_empty() {
                println!("  Kalibrasi     belum ada versi tersimpan");
            }
            for calibration in view.calibrations {
                println!(
                    "  {}  {:?}  {:?}  latihan {}  bayangan {}/{}  menang {}",
                    calibration.version,
                    calibration.regime,
                    calibration.status,
                    calibration.training_positions,
                    calibration.shadow_count,
                    rangefinder_engine::learning::SHADOW_POSITIONS,
                    calibration.shadow_wins,
                );
            }
        }
        "train" => {
            let regime = args
                .get(1)
                .map(|value| {
                    rangefinder_engine::learning::Regime::parse(value)
                        .with_context(|| format!("rezim `{value}` tidak dikenal"))
                })
                .transpose()?;
            let calibration = rangefinder_engine::learning::train_manual(&store, regime)?;
            println!(
                "  Penantang {} dibuat dari {} posisi rezim {}",
                calibration.version,
                calibration.training_positions,
                calibration.regime.as_str()
            );
            println!(
                "  Ia berjalan sebagai bayangan sampai {} hasil baru tersedia.",
                rangefinder_engine::learning::SHADOW_POSITIONS
            );
        }
        "replay" => {
            let report =
                rangefinder_engine::learning::replay(&store, args.get(1).map(String::as_str))?;
            println!("  Versi          {}", report.version);
            println!("  Rezim          {}", report.regime.as_str());
            println!("  Posisi         {}", report.positions);
            println!(
                "  Median galat fee   {}",
                persen(report.median_fee_error_percent)
            );
            println!(
                "  Median galat waktu {}",
                persen(report.median_hours_error_percent)
            );
            println!(
                "  Akurasi net positif {}",
                persen(report.positive_net_accuracy_percent)
            );
        }
        "replay-bars" => {
            let path = args
                .get(1)
                .context("cara pakai: cargo xtask calibration replay-bars <bars.json> [version]")?;
            let bytes = std::fs::read(path)
                .with_context(|| format!("gagal membaca candle historis dari {path}"))?;
            let bars: Vec<rangefinder_engine::signals::bar::Bar> =
                serde_json::from_slice(&bytes).context("berkas candle harus berupa array Bar JSON")?;
            let report = rangefinder_engine::learning::replay_bars(
                &store,
                bars,
                args.get(2).map(String::as_str),
            )?;
            println!("  Versi       {}", report.version);
            println!("  Bar         {}", report.bars);
            println!("  Sampel      {}", report.samples);
            println!(
                "  Median galat waktu keluar {}",
                persen(report.median_exit_time_error_percent)
            );
            println!(
                "  Target <30% {}",
                if report.target_below_30_percent {
                    "lolos"
                } else {
                    "belum lolos"
                }
            );
        }
        "rollback" => {
            let version = args
                .get(1)
                .context("cara pakai: cargo xtask calibration rollback <version>")?;
            let calibration = rangefinder_engine::learning::rollback(&store, version)?;
            println!(
                "  Rezim {} kembali memakai {}",
                calibration.regime.as_str(),
                calibration.version
            );
        }
        other => bail!(
            "aksi kalibrasi `{other}` tidak dikenal. Gunakan status, train, replay, replay-bars, atau rollback"
        ),
    }
    Ok(true)
}

fn persen(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.2}%"))
        .unwrap_or_else(|| "belum terukur".into())
}
