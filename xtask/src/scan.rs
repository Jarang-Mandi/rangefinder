//! Pindai pool paling ramai di rantai, lengkap dengan penilaiannya.
//!
//! Membuktikan sumber daftar kandidat tab Signals benar benar bekerja, dan
//! sekaligus memberi cara cepat melihat apa yang sedang ramai tanpa membuka
//! panel.
//!
//! Cara pakai: `cargo xtask scan`

use std::path::Path;

use anyhow::Result;

use rangefinder_engine::chain::registry::Registry;
use rangefinder_engine::chain::ChainClient;
use rangefinder_engine::config::Config;
use rangefinder_engine::pools::kandidat::{self, Label};
use rangefinder_engine::pools::scan;
use rangefinder_engine::risk::hook::{self, Kriteria};
use rangefinder_engine::store::Store;

/// Angka besar dalam bentuk yang bisa dibaca sekilas.
///
/// Market cap dan volume berbeda ribuan kali lipat antar baris, dan deretan
/// angka penuh membuat perbandingannya justru lebih sulit, bukan lebih mudah.
fn ringkas(v: f64) -> String {
    let a = v.abs();
    if a >= 1e9 {
        format!("{:.2}B", v / 1e9)
    } else if a >= 1e6 {
        format!("{:.2}M", v / 1e6)
    } else if a >= 1e3 {
        format!("{:.1}K", v / 1e3)
    } else if a >= 1.0 {
        format!("{v:.2}")
    } else {
        format!("{v:.4}")
    }
}

fn opsional(v: Option<f64>) -> String {
    v.map(ringkas).unwrap_or_else(|| "-".into())
}

/// APR dalam bentuk yang tidak menyiratkan ketelitian yang tidak ada.
///
/// Di atas ambang, angkanya ditulis sebagai batas bawah. Proyeksi setahun dari
/// jendela setengah jam mengalikan kesalahannya tujuh belas ribu kali, dan
/// tujuh angka penting pada hasil seperti itu adalah kebohongan yang rapi.
fn apr(v: Option<f64>) -> String {
    match v {
        None => "-".into(),
        Some(x) if x > kandidat::APR_TAK_BERARTI => {
            format!(">{:.0}%", kandidat::APR_TAK_BERARTI)
        }
        Some(x) => format!("{x:.0}%"),
    }
}

fn nama_label(l: Label) -> &'static str {
    match l {
        Label::LpSideways => "LP Sideways",
        Label::LpBeliTurun => "LP Beli Turun",
        Label::LpJualNaik => "LP Jual Naik",
        Label::Pantau => "Pantau",
        Label::Hindari => "Hindari",
    }
}

pub async fn run(repo_root: &Path) -> Result<bool> {
    println!("  Pemindaian pool ramai");
    println!("  ---------------------");

    let registry = Registry::load_from(&Registry::default_path(repo_root))?;
    let cfg = Config::load()?;
    let client = ChainClient::new(cfg.rpc.clone());
    let provider = client.provider().await?;

    let jam = std::time::Instant::now();
    let hasil = scan::pindai(&provider, &registry).await?;
    let waktu_pindai = jam.elapsed().as_millis();

    let jendela_menit = (hasil.sampai_blok.saturating_sub(hasil.dari_blok)) as f64
        * registry.chain.block_time_ms as f64
        / 60_000.0;

    println!(
        "  {} swap pada blok {} sampai {}, jendela {:.1} menit, dipindai dalam {} ms",
        hasil.total_swap, hasil.dari_blok, hasil.sampai_blok, jendela_menit, waktu_pindai
    );

    if hasil.kandidat.is_empty() {
        println!("  Tidak ada pool yang ramai pada jendela ini.");
        return Ok(true);
    }

    // Allowlist kosong di sini dengan sengaja, supaya perintah ini selalu
    // menampilkan penilaian hook apa adanya, tanpa dipengaruhi persetujuan yang
    // sudah tercatat di store. Engine sendiri membaca persetujuan itu.
    let jam = std::time::Instant::now();
    let daftar = kandidat::perkaya(&provider, &registry, &hasil, &[]).await?;
    let baris = &daftar.baris;
    println!(
        "  {} baris dinilai dalam {} ms",
        baris.len(),
        jam.elapsed().as_millis()
    );
    // Jendela volume disebutkan bersama hasilnya. Proyeksi APR adalah
    // ekstrapolasi dari jendela ini, dan tanpa menyebutkannya angka itu
    // terbaca seolah olah terukur.
    println!(
        "  volume diukur pada jendela {:.0} menit{}",
        daftar.jendela_volume_detik / 60.0,
        if daftar.volume_jendela_pendek {
            ", jendela panjang gagal dibaca sehingga proyeksinya kasar"
        } else {
            ""
        }
    );
    println!();

    println!(
        "  {:<10} {:<7} {:>9} {:>9} {:>9} {:>8} {:>8} {:>5} {:>6}  {}",
        "simbol",
        "pasangan",
        "mcap",
        "volume",
        "modal",
        "hasil",
        "APR",
        "ER",
        "umur",
        "aksi"
    );
    println!("  {}", "-".repeat(104));

    for b in baris {
        println!(
            "  {:<10} {:<7} {:>9} {:>9} {:>9} {:>8} {:>8} {:>5} {:>6}  {}",
            &b.symbol[..10.min(b.symbol.len())],
            // Tanda tanya di depan simbol kutipan berarti alamatnya bukan yang
            // ada di registry. Simbolnya boleh sama persis dengan yang asli.
            if b.quote_dikenal {
                b.quote_symbol[..7.min(b.quote_symbol.len())].to_string()
            } else {
                format!("?{}", &b.quote_symbol[..6.min(b.quote_symbol.len())])
            },
            opsional(b.mcap),
            opsional(b.volume),
            opsional(b.kedalaman),
            b.hasil_jendela
                .map(|x| format!("{x:.2}%"))
                .unwrap_or_else(|| "-".into()),
            apr(b.fee_apr),
            b.efisiensi
                .map(|x| format!("{x:.2}"))
                .unwrap_or_else(|| "-".into()),
            if b.umur_jam >= 24.0 {
                format!("{:.0}h", b.umur_jam / 24.0)
            } else {
                format!("{:.1}j", b.umur_jam)
            },
            nama_label(b.label),
        );
    }

    // Alasan dicetak terpisah supaya tabelnya tetap terbaca. Alasan adalah
    // bagian yang mengajari user membaca pasar, jadi ia tidak boleh dibuang
    // hanya karena tidak muat dalam satu baris.
    println!();
    println!(
        "  modal   kutipan yang dibutuhkan untuk menutup rentang yang harga lewati",
    );
    println!(
        "  ?SIMBOL alamat kutipannya bukan yang ada di registry, simbol bisa ditiru",
    );
    println!(
        "  hasil   fee sepanjang jendela atas modal acuan {:.0} kutipan",
        kandidat::MODAL_ACUAN
    );
    println!();
    println!("  Alasan");
    for b in baris.iter().take(8) {
        println!(
            "  {} {}  {}",
            b.symbol,
            if b.dynamic_fee {
                "fee dinamis".to_string()
            } else {
                format!("fee {:.2}%", b.fee as f64 / 10_000.0)
            },
            b.alasan
        );
    }

    // PENILAIAN HOOK, PRD 5.4
    //
    // Dicetak di sini supaya kriteria adopsinya benar benar diuji terhadap
    // rantai, bukan hanya terhadap angka buatan di uji satuan.
    let alamat_hook: std::collections::BTreeSet<String> =
        baris.iter().filter_map(|b| b.hooks.clone()).collect();

    if !alamat_hook.is_empty() {
        println!();
        let jam = std::time::Instant::now();
        let adopsi = {
            // Store yang sama dengan engine, supaya peta adopsi menumpuk antar
            // pemanggilan alih alih dibayar ulang setiap kali. Sejarah penuh
            // butuh puluhan potongan, dan satu jalan hanya membaca sebagian.
            let store = Store::open(&rangefinder_engine::config::data_dir()?)?;
            hook::hitung_adopsi(&provider, &registry, Some(&store), hasil.sampai_blok).await?
        };
        println!(
            "  Penilaian hook, {} hook berbeda terlihat pada blok {} sampai {}{}, dihitung dalam {} ms",
            adopsi.peta.len(),
            adopsi.dari_blok,
            adopsi.sampai_blok,
            if adopsi.lengkap {
                ""
            } else {
                ", sejarahnya belum selesai dipindai"
            },
            jam.elapsed().as_millis()
        );
        let tanda = |k: Kriteria| match k {
            Kriteria::Lolos => "lolos",
            Kriteria::Gagal => "tidak",
            Kriteria::BelumTerukur => "belum diukur",
        };
        for h in &alamat_hook {
            match hook::nilai(&provider, &registry, &adopsi, h, hasil.sampai_blok).await {
                Ok(p) => {
                    println!(
                        "  {}  {} pool, umur {}",
                        &h[..10.min(h.len())],
                        p.jumlah_pool
                            .map(|n| n.to_string())
                            .unwrap_or_else(|| "?".into()),
                        p.umur_hari
                            .map(|d| format!("{d:.0} hari"))
                            .unwrap_or_else(|| "?".into()),
                    );
                    println!(
                        "    pool {}, umur {}, tidak bisa diupgrade {}, TVL {}, LP unik {}",
                        tanda(p.kriteria_pool),
                        tanda(p.kriteria_umur),
                        tanda(p.kriteria_upgrade),
                        tanda(p.kriteria_tvl),
                        tanda(p.kriteria_lp),
                    );
                    println!(
                        "    lolos otomatis {}, bisa disetujui user {}",
                        p.lolos_otomatis,
                        p.boleh_disetujui_manual(),
                    );
                }
                Err(e) => println!("  {h}  gagal dinilai: {e}"),
            }
        }
    }

    let bisa_dibuka = baris.iter().filter(|b| b.label.boleh_dibuka()).count();
    println!();
    println!(
        "  {} dari {} baris boleh dibuka sebagai posisi.",
        bisa_dibuka,
        baris.len()
    );

    Ok(!baris.is_empty())
}
