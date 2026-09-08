//! Penilaian hook V4 berdasarkan adopsi on chain, PRD 5.4 butir 3.
//!
//! MASALAHNYA
//! Hook V4 berjalan di dalam setiap operasi pool, termasuk penarikan. Hook yang
//! jahat atau sekadar keliru bisa menolak `BURN_POSITION`, dan akibatnya bukan
//! kerugian harga yang bisa ditanggung, melainkan modal yang tidak bisa diambil
//! kembali sama sekali. Karena itu penolakan adalah sikap bawaan, dan yang
//! perlu dibuktikan adalah kelayakannya, bukan sebaliknya.
//!
//! KENAPA ADOPSI, BUKAN PENILAIAN MANUAL
//! Keputusan user: hook yang sudah ramai dipakai orang boleh masuk allowlist.
//! Supaya itu bisa dijalankan mesin dan bukan tebakan, adopsinya diukur.
//! Kriterianya ada di PRD 5.4 dan disalin utuh sebagai konstanta di bawah.
//!
//! APA YANG BISA DAN BELUM BISA DIUKUR DI SINI
//!
//! | Kriteria | Keadaan |
//! | --- | --- |
//! | jumlah pool >= 50 | terukur, dari event `Initialize` |
//! | umur >= 14 hari | terukur, dari blok `Initialize` paling awal |
//! | tidak bisa diupgrade | terukur, dari slot proxy EIP-1967 dan EIP-1822 |
//! | TVL >= 250.000 USD | BELUM terukur |
//! | alamat LP unik >= 200 | BELUM terukur |
//!
//! Dua yang terakhir butuh pembacaan yang jauh lebih mahal: TVL menuntut
//! distribusi likuiditas per pool, dan jumlah LP unik menuntut penelusuran
//! setiap tokenId PositionManager sampai ke pemiliknya.
//!
//! KENAPA ITU TIDAK DIBULATKAN MENJADI LOLOS
//! PRD meminta SELURUH syarat terpenuhi. Kriteria yang belum bisa diukur berarti
//! syaratnya belum terbukti, dan yang belum terbukti tidak boleh lolos otomatis.
//! Akibatnya `lolos_otomatis` selalu bernilai salah untuk sekarang, dan itu
//! bukan kekurangan yang perlu ditambal: PRD sendiri menyediakan jalan keluar
//! yang benar, yaitu persetujuan sadar per hook dari user, yang dicatat.
//!
//! INVARIAN
//! - Hook yang masih bisa diupgrade DITOLAK berapa pun adopsinya, dan penolakan
//!   itu tidak bisa dilewati persetujuan user. Kode yang diperiksa hari ini
//!   bukan kode yang akan berjalan besok.
//! - Kriteria yang gagal dibaca dilaporkan sebagai gagal dibaca, bukan sebagai
//!   nol. Nol berarti hooknya tidak dipakai siapa pun, kesimpulan yang sangat
//!   berbeda dari tidak bisa membaca.
//! - BagsV4Hook dinilai dengan kriteria yang sama, tanpa perlakuan khusus,
//!   meski alamatnya tercatat di `chain.toml` untuk pengenalan.

use std::collections::HashMap;

use alloy::primitives::{Address, B256, U256};
use alloy::providers::Provider;
use alloy::rpc::types::Filter;
use alloy::sol_types::SolEvent;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::chain::registry::Registry;
use crate::pools::abi;

/// Jumlah pool paling sedikit sebelum sebuah hook dianggap teradopsi.
pub const MINIMUM_POOL: usize = 50;

/// Umur paling muda sebelum sebuah hook dianggap teradopsi, dalam hari.
pub const MINIMUM_UMUR_HARI: f64 = 14.0;

/// TVL paling kecil, dalam USD. Belum terukur, dicantumkan supaya kriterianya
/// tetap terlihat di kode dan tidak diam diam hilang.
pub const MINIMUM_TVL_USD: f64 = 250_000.0;

/// Jumlah alamat LP unik paling sedikit. Belum terukur, alasan yang sama.
pub const MINIMUM_LP_UNIK: usize = 200;

/// Lebar satu potongan saat menghitung adopsi, dalam blok.
///
/// DIUKUR, BUKAN DITEBAK
/// Nilai pertama 3.000.000 dipilih tanpa dasar, dan setiap potongan lalu
/// menghabiskan tiga permintaan yang ditolak sebelum menemukan lebar yang muat.
/// Pengukuran langsung menunjukkan angka sebenarnya:
///
/// | Lebar | Log |
/// | --- | --- |
/// | 375.000 | 7.089, 8.153, 8.675, 7.259, 8.853, 8.936 |
///
/// Semuanya di bawah batas 10.000, dengan sisa yang cukup untuk hari yang lebih
/// ramai. Penyempitan tetap dipasang sebagai jaring pengaman, karena kepadatan
/// pool baru bisa berubah kapan saja.
pub const POTONGAN_ADOPSI: u64 = 375_000;

/// Potongan terkecil yang masih dicoba.
pub const POTONGAN_ADOPSI_MINIMUM: u64 = 25_000;

/// Berapa potongan paling banyak dibaca dalam satu pemanggilan.
///
/// Sejarah penuh butuh ratusan potongan, dan membacanya sekali jalan langsung
/// dibalas 429: percobaan pertama memakai tujuh puluh enam permintaan berturut
/// turut dan gagal di tengah jalan. Sisanya diselesaikan pemanggilan
/// berikutnya, dan sementara itu hitungannya tetap sah sebagai batas bawah.
///
/// Dengan lebar potongan yang sudah diukur, satu permintaan per potongan sudah
/// cukup, jadi angka ini bisa lebih besar daripada saat lebarnya masih ditebak.
///
/// Dua belas potongan kira kira sepuluh detik terhadap RPC publik, dan itu
/// batas yang masih wajar untuk tombol yang user tekan sendiri. Sejarah penuh
/// rantai ini butuh sekitar sebelas kali jalan, dan sementara itu hitungannya
/// tetap sah sebagai batas bawah.
pub const MAKS_POTONGAN_PER_JALAN: usize = 12;

/// Kunci penyimpanan peta adopsi.
const KUNCI_ADOPSI: &str = "risk/adopsi-hook";

/// Slot implementasi proxy EIP-1967.
///
/// `keccak256("eip1967.proxy.implementation") - 1`.
const SLOT_IMPLEMENTASI: &str =
    "0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc";

/// Slot admin proxy EIP-1967.
const SLOT_ADMIN: &str = "0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103";

/// Slot beacon EIP-1967, dipakai proxy yang implementasinya ditunjuk beacon.
const SLOT_BEACON: &str = "0xa3f0ad74e5423aebfd80d3ef4346578335a9a72aeaee59ff6cb3582b35133d50";

/// Slot EIP-1822, pola UUPS generasi awal. `keccak256("PROXIABLE")`.
const SLOT_PROXIABLE: &str =
    "0xc5f16f0fcc639fa48a6947836d9850f504798523bf8c9a3a87d5876cf622bcf7";

/// Peta adopsi beserta seberapa jauh ia sudah dipindai.
///
/// KENAPA KELENGKAPANNYA IKUT DICATAT
/// Menghitung adopsi menuntut membaca seluruh sejarah `Initialize`, dan itu
/// puluhan permintaan yang memicu batas laju kalau dijalankan setiap kali.
/// Karena itu ia dipindai bertahap dan disimpan.
///
/// Peta yang belum lengkap tetap berguna, tapi hanya ke satu arah. Hitungannya
/// adalah BATAS BAWAH: kalau sudah melewati ambang, ia pasti melewatinya, dan
/// menambah pemindaian tidak akan mengubahnya. Kalau belum, kita belum tahu.
/// Memperlakukan hitungan sebagian sebagai jawaban akhir akan menolak hook yang
/// sebenarnya layak, dan penolakan keliru itu tidak akan pernah terlihat karena
/// hasilnya tampak masuk akal.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Adopsi {
    /// Alamat hook ke (jumlah pool, blok pool paling awal yang terlihat).
    pub peta: HashMap<Address, (usize, u64)>,
    /// Blok paling awal yang sudah dipindai.
    pub dari_blok: u64,
    /// Blok paling akhir yang sudah dipindai.
    pub sampai_blok: u64,
    /// Benar kalau pemindaiannya sudah menyentuh blok pemasangan protokol.
    pub lengkap: bool,
}

impl Adopsi {
    fn catat(&mut self, hooks: Address, blok: u64) {
        let e = self.peta.entry(hooks).or_insert((0, blok));
        e.0 += 1;
        e.1 = e.1.min(blok);
    }
}

/// Hasil satu kriteria.
///
/// Tiga keadaan, bukan dua. Membedakan "gagal dibaca" dari "tidak memenuhi"
/// adalah inti modul ini: yang pertama berarti kita belum tahu, yang kedua
/// berarti kita sudah tahu dan jawabannya tidak.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kriteria {
    Lolos,
    Gagal,
    BelumTerukur,
}


/// Penilaian lengkap sebuah hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PenilaianHook {
    pub hook: String,

    /// Berapa pool memakai hook ini. `None` berarti gagal dibaca.
    pub jumlah_pool: Option<usize>,
    pub umur_hari: Option<f64>,
    /// `None` berarti pemeriksaan proxy gagal dibaca, yang diperlakukan sebagai
    /// bisa diupgrade sampai terbukti sebaliknya.
    pub bisa_diupgrade: Option<bool>,

    pub kriteria_pool: Kriteria,
    pub kriteria_umur: Kriteria,
    pub kriteria_upgrade: Kriteria,
    pub kriteria_tvl: Kriteria,
    pub kriteria_lp: Kriteria,

    /// Benar hanya kalau SELURUH kriteria lolos.
    ///
    /// Selama TVL dan jumlah LP unik belum bisa diukur, nilai ini selalu salah.
    /// Itu disengaja, dan jalan keluarnya adalah persetujuan sadar user.
    pub lolos_otomatis: bool,

    /// Benar kalau hook ini tidak boleh dipakai dengan cara apa pun, termasuk
    /// lewat persetujuan user.
    pub terlarang: bool,

    /// Kalimat yang bisa dibaca user, satu per kriteria.
    pub catatan: Vec<String>,
}

impl PenilaianHook {
    /// Benar kalau user boleh menyetujuinya secara sadar per hook.
    ///
    /// PRD 5.4: hook yang belum lolos tetap bisa dipakai lewat pilihan sadar
    /// yang menampilkan peringatan dan mencatat persetujuannya. Yang otomatis
    /// hanya penolakan bawaannya. Satu satunya yang tidak bisa disetujui adalah
    /// hook yang masih bisa diupgrade.
    pub fn boleh_disetujui_manual(&self) -> bool {
        !self.terlarang
    }
}

/// Nilai sebuah hook terhadap kriteria adopsi.
///
/// `sampai_blok` dipakai sebagai patokan umur supaya seluruh penilaian dalam
/// satu pemindaian memakai titik acuan yang sama.
pub async fn nilai<P: Provider>(
    provider: &P,
    registry: &Registry,
    adopsi: &Adopsi,
    hook: &str,
    sampai_blok: u64,
) -> Result<PenilaianHook> {
    let alamat: Address = hook
        .parse()
        .with_context(|| format!("`{hook}` bukan alamat hook yang sah"))?;

    // Peta adopsi dihitung sekali di luar, bukan sekali per hook. Satu
    // pemindaian melayani seluruh hook: field `hooks` pada `Initialize` tidak
    // terindeks, jadi lognya harus dibaca semuanya, dan setelah dibaca semuanya
    // mengelompokkannya tidak menambah biaya apa pun.
    let (jumlah_pool, blok_pertama) = adopsi
        .peta
        .get(&alamat)
        .map(|(n, b)| (Some(*n), Some(*b)))
        .unwrap_or((Some(0), None));

    let umur_hari = blok_pertama.map(|b| {
        sampai_blok.saturating_sub(b) as f64 * registry.chain.block_time_ms as f64
            / 1_000.0
            / 86_400.0
    });

    let bisa_diupgrade = periksa_upgrade(provider, alamat).await.ok();

    susun(hook, jumlah_pool, umur_hari, bisa_diupgrade, adopsi.lengkap)
}

/// Susun penilaian dari angka yang sudah terkumpul.
///
/// Terpisah dari pembacaan rantai supaya seluruh cabang keputusannya bisa diuji
/// tanpa jaringan. Inilah bagian yang menentukan apakah modal boleh masuk.
pub fn susun(
    hook: &str,
    jumlah_pool: Option<usize>,
    umur_hari: Option<f64>,
    bisa_diupgrade: Option<bool>,
    adopsi_lengkap: bool,
) -> Result<PenilaianHook> {
    let mut catatan = Vec::new();

    // HITUNGAN SEBAGIAN ADALAH BATAS BAWAH, BUKAN JAWABAN
    //
    // Selama pemindaian sejarah belum selesai, angka yang terkumpul hanya
    // menyatakan paling sedikit sekian. Sudah melewati ambang berarti pasti
    // lolos, dan menambah pemindaian tidak akan mengubahnya. Belum melewati
    // ambang berarti kita belum tahu, bukan berarti tidak.
    //
    // Menyimpulkan `tidak` dari hitungan sebagian akan menolak hook yang
    // sebenarnya layak, dan penolakan keliru seperti itu tidak akan pernah
    // terlihat, karena hasilnya tampak masuk akal.
    let kriteria_pool = match jumlah_pool {
        Some(n) if n >= MINIMUM_POOL => {
            catatan.push(format!(
                "Dipakai {n} pool, syaratnya minimal {MINIMUM_POOL}."
            ));
            Kriteria::Lolos
        }
        Some(n) if adopsi_lengkap => {
            catatan.push(format!(
                "Dipakai {n} pool, syaratnya minimal {MINIMUM_POOL}."
            ));
            Kriteria::Gagal
        }
        Some(n) => {
            catatan.push(format!(
                "Baru {n} pool yang terhitung dari sebagian sejarah rantai, \
                 syaratnya minimal {MINIMUM_POOL}. Pemindaian sejarahnya belum \
                 selesai, jadi angka ini batas bawah, bukan jawaban akhir."
            ));
            Kriteria::BelumTerukur
        }
        None => {
            catatan.push("Jumlah pool gagal dibaca dari rantai.".to_string());
            Kriteria::BelumTerukur
        }
    };

    // Umur juga batas bawah: pool paling awal yang TERLIHAT bisa saja bukan
    // yang paling awal sungguhan, sehingga umur yang terhitung tidak pernah
    // melebihi umur sebenarnya.
    let kriteria_umur = match umur_hari {
        Some(h) if h >= MINIMUM_UMUR_HARI => {
            catatan.push(format!(
                "Pool pertamanya berumur {h:.1} hari, syaratnya minimal {MINIMUM_UMUR_HARI:.0}."
            ));
            Kriteria::Lolos
        }
        Some(h) if adopsi_lengkap => {
            catatan.push(format!(
                "Pool pertamanya berumur {h:.1} hari, syaratnya minimal {MINIMUM_UMUR_HARI:.0}."
            ));
            Kriteria::Gagal
        }
        Some(h) => {
            catatan.push(format!(
                "Pool paling awal yang terlihat berumur {h:.1} hari, syaratnya \
                 minimal {MINIMUM_UMUR_HARI:.0}. Sejarahnya belum selesai \
                 dipindai, jadi umur sebenarnya bisa lebih tua."
            ));
            Kriteria::BelumTerukur
        }
        None => {
            catatan.push("Umur hook gagal dibaca dari rantai.".to_string());
            Kriteria::BelumTerukur
        }
    };

    // TIDAK BISA DITAWAR
    // Hook yang bisa diganti isinya sewaktu waktu tidak boleh dipakai berapa pun
    // adopsinya, karena kode yang diperiksa hari ini bukan kode yang akan
    // berjalan besok. Gagal membaca diperlakukan sama dengan bisa diupgrade.
    let (kriteria_upgrade, terlarang) = match bisa_diupgrade {
        Some(false) => {
            catatan.push(
                "Tidak ditemukan slot proxy yang aktif, jadi kodenya tidak bisa \
                 diganti sewaktu waktu."
                    .to_string(),
            );
            (Kriteria::Lolos, false)
        }
        Some(true) => {
            catatan.push(
                "Alamatnya proxy yang masih bisa diupgrade. Ini penolakan mutlak, \
                 karena kode yang diperiksa hari ini bukan kode yang akan berjalan \
                 besok."
                    .to_string(),
            );
            (Kriteria::Gagal, true)
        }
        None => {
            catatan.push(
                "Pemeriksaan proxy gagal dibaca. Diperlakukan sebagai bisa \
                 diupgrade, karena yang belum terbukti aman bukan berarti aman."
                    .to_string(),
            );
            (Kriteria::BelumTerukur, true)
        }
    };

    catatan.push(format!(
        "TVL minimal {MINIMUM_TVL_USD:.0} USD belum bisa diukur alat ini."
    ));
    catatan.push(format!(
        "Jumlah alamat LP unik minimal {MINIMUM_LP_UNIK} belum bisa diukur alat ini."
    ));

    let semua = [
        kriteria_pool,
        kriteria_umur,
        kriteria_upgrade,
        Kriteria::BelumTerukur,
        Kriteria::BelumTerukur,
    ];
    let lolos_otomatis = semua.iter().all(|k| *k == Kriteria::Lolos);

    Ok(PenilaianHook {
        hook: hook.to_string(),
        jumlah_pool,
        umur_hari,
        bisa_diupgrade,
        kriteria_pool,
        kriteria_umur,
        kriteria_upgrade,
        kriteria_tvl: Kriteria::BelumTerukur,
        kriteria_lp: Kriteria::BelumTerukur,
        lolos_otomatis,
        terlarang,
        catatan,
    })
}

/// Hitung berapa pool memakai setiap hook, beserta blok pool pertamanya.
///
/// Satu pemindaian untuk seluruh hook sekaligus, bukan satu per hook. Field
/// `hooks` pada `Initialize` tidak terindeks, jadi menyaringnya di sisi RPC
/// tidak mungkin dan lognya harus dibaca semuanya. Karena harus dibaca semuanya,
/// mengelompokkannya sekalian tidak menambah biaya apa pun.
pub async fn hitung_adopsi<P: Provider>(
    provider: &P,
    registry: &Registry,
    store: Option<&crate::store::Store>,
    sampai_blok: u64,
) -> Result<Adopsi> {
    let pool_manager: Address = registry.contract("uniswap_v4", "pool_manager")?;
    let lantai = registry.chain.deploy_block;

    let mut adopsi: Adopsi = store
        .and_then(|st| st.get::<Adopsi>(KUNCI_ADOPSI).ok().flatten())
        .unwrap_or_default();

    // Cache yang tidak menyambung dengan rantai sekarang dibuang seluruhnya.
    // Melanjutkan dari cache yang bolong berarti melewatkan pool selamanya, dan
    // hitungan yang melewatkan pool tidak lagi menjadi batas bawah yang sah.
    if adopsi.sampai_blok > sampai_blok || (adopsi.sampai_blok > 0 && adopsi.dari_blok < lantai) {
        adopsi = Adopsi::default();
    }

    let mut potongan_terpakai = 0usize;

    // ARAH PERTAMA: blok baru sejak pemindaian terakhir.
    // Didahulukan karena inilah yang membuat hitungan tetap mutakhir, dan
    // jumlahnya biasanya kecil.
    if adopsi.sampai_blok > 0 && adopsi.sampai_blok < sampai_blok {
        let mut mulai = adopsi.sampai_blok + 1;
        while mulai <= sampai_blok && potongan_terpakai < MAKS_POTONGAN_PER_JALAN {
            let (baru, hingga) =
                baca_potongan(provider, pool_manager, sampai_blok, mulai).await?;
            potongan_terpakai += 1;
            for (h, b) in &baru {
                adopsi.catat(*h, *b);
            }
            adopsi.sampai_blok = hingga;
            if hingga >= sampai_blok {
                break;
            }
            mulai = hingga + 1;
        }
    }

    // ARAH KEDUA: mundur menuju blok pemasangan, sedikit demi sedikit.
    //
    // Dibatasi jumlah potongan per pemanggilan supaya satu pemindaian tidak
    // pernah menghabiskan jatah laju RPC. Percobaan pertama membaca seluruh
    // sejarah sekali jalan, dan tujuh puluh enam permintaan berturut turut
    // langsung dibalas 429. Sisanya diselesaikan pemanggilan berikutnya.
    if !adopsi.lengkap {
        let mut hingga = if adopsi.dari_blok > lantai {
            adopsi.dari_blok.saturating_sub(1)
        } else {
            sampai_blok
        };

        while hingga > lantai && potongan_terpakai < MAKS_POTONGAN_PER_JALAN {
            let potongan = crate::chain::logs::baca_mundur(
                provider,
                hingga,
                lantai,
                POTONGAN_ADOPSI,
                POTONGAN_ADOPSI_MINIMUM,
                |dari, sampai| {
                    Filter::new()
                        .address(pool_manager)
                        .event_signature(abi::Initialize::SIGNATURE_HASH)
                        .from_block(dari)
                        .to_block(sampai)
                },
            )
            .await
            .context("pembacaan Initialize untuk menghitung adopsi hook gagal")?;
            potongan_terpakai += 1;

            for log in &potongan.logs {
                if let Ok(d) = abi::Initialize::decode_log(&log.inner) {
                    adopsi.catat(d.hooks, log.block_number.unwrap_or(u64::MAX));
                }
            }

            adopsi.dari_blok = potongan.dari_blok;
            adopsi.sampai_blok = adopsi.sampai_blok.max(potongan.sampai_blok);
            if potongan.dari_blok <= lantai {
                adopsi.lengkap = true;
                break;
            }
            hingga = potongan.dari_blok.saturating_sub(1);
        }
    }

    if let Some(st) = store {
        if let Err(e) = st.put(KUNCI_ADOPSI, &adopsi) {
            tracing::warn!(error = %e, "peta adopsi hook gagal disimpan");
        }
    }

    Ok(adopsi)
}

/// Baca satu potongan maju, kembalikan pasangan hook beserta bloknya.
async fn baca_potongan<P: Provider>(
    provider: &P,
    pool_manager: Address,
    sampai_blok: u64,
    mulai: u64,
) -> Result<(Vec<(Address, u64)>, u64)> {
    let hingga = (mulai + POTONGAN_ADOPSI).min(sampai_blok);
    let logs = crate::chain::logs::baca_rentang(
        provider,
        &Filter::new()
            .address(pool_manager)
            .event_signature(abi::Initialize::SIGNATURE_HASH)
            .from_block(mulai)
            .to_block(hingga),
        "pembacaan Initialize blok baru gagal",
    )
    .await?;

    let mut out = Vec::with_capacity(logs.len());
    for log in &logs {
        if let Ok(d) = abi::Initialize::decode_log(&log.inner) {
            out.push((d.hooks, log.block_number.unwrap_or(u64::MAX)));
        }
    }
    Ok((out, hingga))
}
/// Benar kalau alamat itu proxy yang implementasinya masih bisa diganti.
///
/// Empat slot diperiksa sekaligus karena tidak ada satu pola proxy yang berlaku
/// umum. Salah satu saja terisi sudah cukup untuk menolak.
pub async fn periksa_upgrade<P: Provider>(provider: &P, alamat: Address) -> Result<bool> {
    for slot in [SLOT_IMPLEMENTASI, SLOT_ADMIN, SLOT_BEACON, SLOT_PROXIABLE] {
        let kunci: B256 = slot.parse().context("slot proxy tidak berbentuk bytes32")?;
        let isi = provider
            .get_storage_at(alamat, U256::from_be_bytes(kunci.0))
            .await
            .with_context(|| format!("pembacaan slot {slot} gagal"))?;
        if !isi.is_zero() {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOOK: &str = "0x4e3468951D49f2EEa976eD0D6e75fFCb44a9a544";

    #[test]
    fn bisa_diupgrade_adalah_penolakan_mutlak() {
        // Adopsi setinggi apa pun tidak boleh mengalahkan syarat ini. Kode yang
        // diperiksa hari ini bukan kode yang akan berjalan besok.
        let p = susun(HOOK, Some(10_000), Some(365.0), Some(true), true).unwrap();
        assert!(!p.lolos_otomatis);
        assert!(p.terlarang);
        assert!(
            !p.boleh_disetujui_manual(),
            "hook yang bisa diupgrade tidak boleh bisa disetujui user sekalipun"
        );
    }

    #[test]
    fn gagal_membaca_proxy_diperlakukan_sebagai_bisa_diupgrade() {
        // Yang belum terbukti aman bukan berarti aman. Kalau pemeriksaannya
        // gagal, sikapnya harus sama dengan kalau hasilnya buruk.
        let p = susun(HOOK, Some(10_000), Some(365.0), None, true).unwrap();
        assert!(p.terlarang);
        assert_eq!(p.kriteria_upgrade, Kriteria::BelumTerukur);
    }

    #[test]
    fn hook_bersih_tetap_belum_lolos_otomatis() {
        // TVL dan jumlah LP unik belum bisa diukur, jadi tidak ada hook yang
        // bisa lolos otomatis untuk sekarang. Ini disengaja: PRD meminta
        // SELURUH syarat terpenuhi, dan yang belum terukur belum terbukti.
        let p = susun(HOOK, Some(10_000), Some(365.0), Some(false), true).unwrap();
        assert!(!p.lolos_otomatis);
        assert_eq!(p.kriteria_pool, Kriteria::Lolos);
        assert_eq!(p.kriteria_umur, Kriteria::Lolos);
        assert_eq!(p.kriteria_upgrade, Kriteria::Lolos);
        assert_eq!(p.kriteria_tvl, Kriteria::BelumTerukur);
        assert_eq!(p.kriteria_lp, Kriteria::BelumTerukur);
        // Tapi ia BOLEH disetujui user secara sadar, dan itulah jalan keluarnya.
        assert!(p.boleh_disetujui_manual());
    }

    #[test]
    fn adopsi_kurang_ditolak_tapi_masih_bisa_disetujui() {
        let p = susun(HOOK, Some(3), Some(0.5), Some(false), true).unwrap();
        assert_eq!(p.kriteria_pool, Kriteria::Gagal);
        assert_eq!(p.kriteria_umur, Kriteria::Gagal);
        assert!(!p.lolos_otomatis);
        assert!(p.boleh_disetujui_manual());
    }

    #[test]
    fn hitungan_sebagian_tidak_pernah_menyimpulkan_tidak() {
        // Selama sejarahnya belum selesai dipindai, angka yang terkumpul hanya
        // batas bawah. Menyimpulkan `tidak` darinya akan menolak hook yang
        // sebenarnya layak, dan penolakan itu tidak akan pernah terlihat.
        let sebagian = susun(HOOK, Some(3), Some(0.5), Some(false), false).unwrap();
        assert_eq!(sebagian.kriteria_pool, Kriteria::BelumTerukur);
        assert_eq!(sebagian.kriteria_umur, Kriteria::BelumTerukur);

        // Tapi kalau sudah melewati ambang, ia pasti melewatinya. Menambah
        // pemindaian hanya bisa menambah, tidak pernah mengurangi.
        let cukup = susun(HOOK, Some(500), Some(90.0), Some(false), false).unwrap();
        assert_eq!(cukup.kriteria_pool, Kriteria::Lolos);
        assert_eq!(cukup.kriteria_umur, Kriteria::Lolos);
    }

    #[test]
    fn nol_pool_berbeda_dari_gagal_membaca() {
        // Nol berarti hooknya tidak dipakai siapa pun. Gagal membaca berarti
        // kita belum tahu. Menyamakan keduanya menyembunyikan masalah RPC
        // sebagai kesimpulan tentang hooknya.
        let nol = susun(HOOK, Some(0), Some(1.0), Some(false), true).unwrap();
        assert_eq!(nol.kriteria_pool, Kriteria::Gagal);

        let gagal = susun(HOOK, None, Some(1.0), Some(false), true).unwrap();
        assert_eq!(gagal.kriteria_pool, Kriteria::BelumTerukur);
    }

    #[test]
    fn setiap_kriteria_punya_kalimatnya_sendiri() {
        // Label tanpa alasan tidak mengajari user apa apa, dan inilah tempat
        // alasan itu dibuat.
        let p = susun(HOOK, Some(3), Some(0.5), Some(false), true).unwrap();
        assert_eq!(p.catatan.len(), 5, "lima kriteria, lima kalimat");
        assert!(p.catatan.iter().all(|c| !c.is_empty()));
    }

    #[test]
    fn slot_proxy_berbentuk_bytes32() {
        // Kekeliruan satu karakter di sini membuat pemeriksaannya membaca slot
        // yang salah, selalu menemukan nol, dan selalu menyimpulkan aman.
        for slot in [SLOT_IMPLEMENTASI, SLOT_ADMIN, SLOT_BEACON, SLOT_PROXIABLE] {
            assert!(slot.parse::<B256>().is_ok(), "slot {slot} tidak sah");
        }
        // EIP-1967 mendefinisikan slot implementasi sebagai
        // keccak256("eip1967.proxy.implementation") - 1. Nilainya dikunci di
        // standar, jadi ia bisa diperiksa apa adanya.
        assert_eq!(
            SLOT_IMPLEMENTASI,
            "0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc"
        );
    }
}
