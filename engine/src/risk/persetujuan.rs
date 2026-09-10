//! Persetujuan sadar user atas hook yang belum lolos otomatis, PRD 5.4.
//!
//! KENAPA INI ADA
//! Kriteria adopsi di `hook` sengaja ketat. Hook yang memenuhi kelimanya dicatat
//! otomatis selama 24 jam; hook lain tetap punya jalan persetujuan sadar user.
//!
//! PRD 5.4 sudah menyediakan jalan keluarnya: hook yang belum lolos tetap bisa
//! dipakai lewat pilihan sadar per hook, yang menampilkan peringatan dan
//! MENCATAT persetujuannya. Yang otomatis hanya penolakan bawaannya.
//!
//! INVARIAN
//! - Hook yang masih bisa diupgrade tidak bisa disetujui, dan pemeriksaan itu
//!   ada di sini juga, bukan hanya di panel. Panel bisa keliru atau diakali,
//!   engine tidak boleh ikut keliru.
//! - Persetujuan disimpan beserta waktunya dan beserta penilaian saat itu.
//!   Persetujuan tanpa catatan alasan tidak bisa ditinjau ulang belakangan,
//!   dan yang paling ingin ditinjau justru keputusan yang ternyata keliru.
//! - Alamat disimpan dalam huruf kecil. Ia datang dari log dalam bentuk
//!   checksum dan dari panel apa adanya, dan pencocokan yang sensitif huruf
//!   akan diam diam tidak pernah cocok.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use super::hook::PenilaianHook;
use crate::store::Store;

const AWALAN: &str = "risk/hook-disetujui/";

/// Catatan satu persetujuan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Persetujuan {
    pub hook: String,
    pub pada: String,
    /// Benar jika kelima kriteria adopsi yang membuat catatan ini, bukan klik
    /// persetujuan user. Catatan otomatis harus dinilai ulang secara berkala.
    #[serde(default)]
    pub otomatis: bool,
    /// Penilaian saat persetujuan diberikan.
    ///
    /// Disimpan utuh, bukan hanya kesimpulannya, supaya keputusannya bisa
    /// ditinjau ulang dengan angka yang user lihat saat itu, bukan dengan angka
    /// hari ini.
    pub penilaian: PenilaianHook,
}

fn kunci(hook: &str) -> String {
    format!("{AWALAN}{}", hook.to_lowercase())
}

/// Catat persetujuan user atas sebuah hook.
///
/// Menolak hook yang masih bisa diupgrade, berapa pun adopsinya dan sekeras apa
/// pun user memintanya. Itu satu satunya syarat yang tidak bisa ditawar, dan
/// menegakkannya di engine berarti panel yang keliru tidak bisa melewatinya.
pub fn setujui(store: &Store, penilaian: &PenilaianHook) -> Result<Persetujuan> {
    if !penilaian.boleh_disetujui_manual() {
        bail!(
            "hook {} tidak bisa disetujui: alamatnya masih bisa diupgrade, \
             sehingga kode yang diperiksa sekarang bukan kode yang akan berjalan \
             nanti",
            penilaian.hook
        );
    }
    if penilaian
        .code_hash
        .as_ref()
        .and_then(|hash| hash.parse::<alloy::primitives::B256>().ok())
        .is_none()
    {
        bail!("bytecode hook belum dibuktikan, nilai ulang sebelum menyetujui");
    }

    let catatan = Persetujuan {
        hook: penilaian.hook.to_lowercase(),
        pada: chrono::Utc::now().to_rfc3339(),
        otomatis: false,
        penilaian: penilaian.clone(),
    };
    store.put(&kunci(&penilaian.hook), &catatan)?;
    Ok(catatan)
}

/// Catat hook yang memenuhi seluruh syarat adopsi tanpa meminta persetujuan.
pub fn catat_otomatis(store: &Store, penilaian: &PenilaianHook) -> Result<Persetujuan> {
    if !penilaian.lolos_otomatis || penilaian.terlarang {
        bail!("hook belum memenuhi seluruh syarat allowlist otomatis");
    }
    if penilaian
        .code_hash
        .as_ref()
        .and_then(|hash| hash.parse::<alloy::primitives::B256>().ok())
        .is_none()
    {
        bail!("bytecode hook belum dibuktikan");
    }
    let catatan = Persetujuan {
        hook: penilaian.hook.to_lowercase(),
        pada: chrono::Utc::now().to_rfc3339(),
        otomatis: true,
        penilaian: penilaian.clone(),
    };
    store.put(&kunci(&penilaian.hook), &catatan)?;
    Ok(catatan)
}

/// Cabut persetujuan sebuah hook.
pub fn cabut(store: &Store, hook: &str) -> bool {
    store.delete(&kunci(hook)).unwrap_or(false)
}

/// Benar kalau hook itu sudah pernah disetujui user.
pub fn disetujui(store: &Store, hook: &str) -> bool {
    store
        .get::<Persetujuan>(&kunci(hook))
        .ok()
        .flatten()
        .is_some_and(|p| p.penilaian.code_hash.is_some() && p.penilaian.boleh_disetujui_manual())
}

/// Baca catatan persetujuan sebuah hook.
pub fn baca(store: &Store, hook: &str) -> Option<Persetujuan> {
    store.get::<Persetujuan>(&kunci(hook)).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::risk::hook;

    const HOOK: &str = "0x4e3468951D49f2EEa976eD0D6e75fFCb44a9a544";

    // Pola yang sama dengan uji cache pool: satu direktori per nama uji,
    // dibersihkan lebih dulu supaya uji tidak saling mewarisi keadaan.
    fn store_uji(nama: &str) -> Store {
        let d = std::env::temp_dir().join(format!(
            "rangefinder-persetujuan-{nama}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        Store::open(&d).expect("store terbuka")
    }

    #[test]
    fn persetujuan_bertahan_dan_tidak_peduli_besar_kecil_huruf() {
        // Alamat datang dari log dalam bentuk checksum dan dari panel apa
        // adanya. Pencocokan yang sensitif huruf akan diam diam tidak pernah
        // cocok, dan akibatnya persetujuan user seolah olah hilang.
        let store = store_uji("bertahan");
        let mut p = hook::susun(HOOK, Some(100), Some(30.0), Some(false), true).unwrap();
        p.code_hash = Some(format!("{:#x}", alloy::primitives::keccak256([0x60, 0x00])));

        assert!(!disetujui(&store, HOOK));
        setujui(&store, &p).expect("harus bisa disetujui");
        assert!(disetujui(&store, HOOK));
        assert!(disetujui(&store, &HOOK.to_lowercase()));
        assert!(disetujui(&store, &HOOK.to_uppercase()));
    }

    #[test]
    fn hook_bisa_diupgrade_ditolak_di_engine_bukan_hanya_di_panel() {
        // Panel bisa keliru atau diakali. Kalau penolakannya hanya ada di sana,
        // satu bug di panel cukup untuk menaruh modal di hook yang isinya bisa
        // diganti sewaktu waktu.
        let store = store_uji("upgrade");
        let mut p = hook::susun(HOOK, Some(10_000), Some(365.0), Some(true), true).unwrap();
        p.code_hash = Some(format!("{:#x}", alloy::primitives::keccak256([0x60, 0x00])));

        let hasil = setujui(&store, &p);
        assert!(hasil.is_err(), "harus ditolak");
        assert!(!disetujui(&store, HOOK), "tidak boleh tercatat");
    }

    #[test]
    fn persetujuan_menyimpan_penilaian_saat_itu() {
        // Keputusan yang paling ingin ditinjau ulang adalah yang ternyata
        // keliru, dan meninjaunya butuh angka yang user lihat saat menyetujui,
        // bukan angka hari ini.
        let store = store_uji("penilaian");
        let mut p = hook::susun(HOOK, Some(7), Some(2.0), Some(false), true).unwrap();
        p.code_hash = Some(format!("{:#x}", alloy::primitives::keccak256([0x60, 0x00])));
        setujui(&store, &p).unwrap();

        let catatan = baca(&store, HOOK).expect("catatan harus ada");
        assert_eq!(catatan.penilaian.jumlah_pool, Some(7));
        assert!(!catatan.pada.is_empty());
    }

    #[test]
    fn persetujuan_bisa_dicabut() {
        let store = store_uji("cabut");
        let mut p = hook::susun(HOOK, Some(100), Some(30.0), Some(false), true).unwrap();
        p.code_hash = Some(format!("{:#x}", alloy::primitives::keccak256([0x60, 0x00])));
        setujui(&store, &p).unwrap();
        assert!(cabut(&store, HOOK));
        assert!(!disetujui(&store, HOOK));
    }

    #[test]
    fn allowlist_otomatis_hanya_menerima_lima_kriteria_lolos() {
        let store = store_uji("otomatis");
        let mut belum = hook::susun(HOOK, Some(100), Some(30.0), Some(false), true).unwrap();
        belum.code_hash = Some(format!("{:#x}", alloy::primitives::keccak256([0x60, 0x00])));
        assert!(catat_otomatis(&store, &belum).is_err());

        let mut lolos = hook::susun_dengan_metrik(
            HOOK,
            Some(100),
            Some(30.0),
            Some(false),
            true,
            hook::MetrikAdopsi {
                tvl_usd: Some(300_000.0),
                tvl_lengkap: true,
                lp_unik: Some(250),
                lp_lengkap: true,
            },
        )
        .unwrap();
        lolos.code_hash = belum.code_hash;
        let catatan = catat_otomatis(&store, &lolos).unwrap();
        assert!(catatan.otomatis);
        assert!(catatan.penilaian.lolos_otomatis);
    }
}
