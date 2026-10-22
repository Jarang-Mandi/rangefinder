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
//! APA YANG DIUKUR DI SINI
//!
//! | Kriteria | Keadaan |
//! | --- | --- |
//! | jumlah pool >= 50 | terukur, dari event `Initialize` |
//! | umur >= 14 hari | terukur, dari blok `Initialize` paling awal |
//! | tidak bisa diupgrade | terukur, dari slot proxy EIP-1967 dan EIP-1822 |
//! | TVL >= 250.000 USD | distribusi likuiditas pool dengan jalur harga USDG/WETH |
//! | alamat LP unik >= 200 | `ModifyLiquidity.salt` lalu `ownerOf(tokenId)` |
//!
//! TVL dan LP dipindai bertahap seperti jumlah pool. Selama cakupannya belum
//! lengkap, nilainya adalah batas bawah: melewati ambang sudah cukup untuk
//! membuktikan kelayakan, sedangkan belum melewati ambang tetap belum terukur.
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

use std::collections::{HashMap, HashSet};

use alloy::primitives::{Address, B256, U256};
use alloy::providers::Provider;
use alloy::rpc::types::Filter;
use alloy::sol_types::SolEvent;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::chain::registry::Registry;
use crate::exec::v4::abi::{IPositionManagerV4, ModifyLiquidity};
use crate::pools::abi;

/// Jumlah pool paling sedikit sebelum sebuah hook dianggap teradopsi.
pub const MINIMUM_POOL: usize = 50;

/// Umur paling muda sebelum sebuah hook dianggap teradopsi, dalam hari.
pub const MINIMUM_UMUR_HARI: f64 = 14.0;

/// TVL paling kecil, dalam USD.
pub const MINIMUM_TVL_USD: f64 = 250_000.0;

/// Jumlah alamat LP unik paling sedikit.
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
const SLOT_PROXIABLE: &str = "0xc5f16f0fcc639fa48a6947836d9850f504798523bf8c9a3a87d5876cf622bcf7";

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
    /// PoolKey lengkap menurut poolId. Bentuk string dipakai sebagai kunci
    /// supaya catatan JSON tetap bisa dibaca langsung.
    #[serde(default)]
    pub pools: HashMap<String, PoolAdopsi>,
    /// Token ID posisi aktif maupun terbakar ke poolId yang pernah diubahnya.
    #[serde(default)]
    pub token_pool: HashMap<String, String>,
    #[serde(default)]
    pub posisi_dari_blok: u64,
    #[serde(default)]
    pub posisi_sampai_blok: u64,
    #[serde(default)]
    pub posisi_lengkap: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolAdopsi {
    pub id: String,
    pub currency0: Address,
    pub currency1: Address,
    pub fee: u32,
    pub tick_spacing: i32,
    pub hooks: Address,
}

impl Adopsi {
    fn catat(&mut self, pool: PoolAdopsi, blok: u64) {
        let id = pool.id.to_lowercase();
        if self.pools.insert(id, pool.clone()).is_none() {
            let e = self.peta.entry(pool.hooks).or_insert((0, blok));
            e.0 += 1;
            e.1 = e.1.min(blok);
        }
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
    /// Hash bytecode yang benar benar dibaca ketika penilaian dibuat.
    #[serde(default)]
    pub code_hash: Option<String>,

    /// Berapa pool memakai hook ini. `None` berarti gagal dibaca.
    pub jumlah_pool: Option<usize>,
    pub umur_hari: Option<f64>,
    /// `None` berarti pemeriksaan proxy gagal dibaca, yang diperlakukan sebagai
    /// bisa diupgrade sampai terbukti sebaliknya.
    pub bisa_diupgrade: Option<bool>,
    /// TVL yang berhasil dibuktikan dalam USD. Bisa berupa batas bawah saat
    /// sebagian pool belum bisa dihargai.
    #[serde(default)]
    pub tvl_usd: Option<f64>,
    /// Jumlah pemilik NFT posisi yang berbeda dan masih aktif.
    #[serde(default)]
    pub lp_unik: Option<usize>,

    pub kriteria_pool: Kriteria,
    pub kriteria_umur: Kriteria,
    pub kriteria_upgrade: Kriteria,
    pub kriteria_tvl: Kriteria,
    pub kriteria_lp: Kriteria,

    /// Benar hanya kalau SELURUH kriteria lolos.
    ///
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

/// Angka adopsi mahal beserta kelengkapan cakupannya.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MetrikAdopsi {
    pub tvl_usd: Option<f64>,
    pub tvl_lengkap: bool,
    pub lp_unik: Option<usize>,
    pub lp_lengkap: bool,
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
    let metrik = ukur_metrik(provider, registry, adopsi, alamat)
        .await
        .unwrap_or_default();
    let mut penilaian = susun_dengan_metrik(
        hook,
        jumlah_pool,
        umur_hari,
        bisa_diupgrade,
        adopsi.lengkap,
        metrik,
    )?;
    penilaian.code_hash = code_hash(provider, alamat).await.ok();
    if penilaian.code_hash.is_none() {
        penilaian.terlarang = true;
        penilaian.lolos_otomatis = false;
        penilaian.catatan.push(
            "Bytecode hook kosong atau belum terbaca; persetujuan belum dapat diberikan.".into(),
        );
    }
    Ok(penilaian)
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
    susun_dengan_metrik(
        hook,
        jumlah_pool,
        umur_hari,
        bisa_diupgrade,
        adopsi_lengkap,
        MetrikAdopsi::default(),
    )
}

pub fn susun_dengan_metrik(
    hook: &str,
    jumlah_pool: Option<usize>,
    umur_hari: Option<f64>,
    bisa_diupgrade: Option<bool>,
    adopsi_lengkap: bool,
    metrik: MetrikAdopsi,
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

    let kriteria_tvl = match metrik.tvl_usd {
        Some(v) if v >= MINIMUM_TVL_USD => {
            catatan.push(format!(
                "TVL yang terbukti {v:.0} USD, syaratnya minimal {MINIMUM_TVL_USD:.0} USD."
            ));
            Kriteria::Lolos
        }
        Some(v) if metrik.tvl_lengkap => {
            catatan.push(format!(
                "TVL {v:.0} USD, syaratnya minimal {MINIMUM_TVL_USD:.0} USD."
            ));
            Kriteria::Gagal
        }
        Some(v) => {
            catatan.push(format!(
                "TVL yang baru terbukti {v:.0} USD. Sebagian pool belum bisa dihargai, jadi ini batas bawah dari syarat {MINIMUM_TVL_USD:.0} USD."
            ));
            Kriteria::BelumTerukur
        }
        None => {
            catatan.push("TVL USD gagal dibaca dari rantai.".to_string());
            Kriteria::BelumTerukur
        }
    };

    let kriteria_lp = match metrik.lp_unik {
        Some(n) if n >= MINIMUM_LP_UNIK => {
            catatan.push(format!(
                "Ada {n} alamat LP aktif yang unik, syaratnya minimal {MINIMUM_LP_UNIK}."
            ));
            Kriteria::Lolos
        }
        Some(n) if metrik.lp_lengkap => {
            catatan.push(format!(
                "Ada {n} alamat LP aktif yang unik, syaratnya minimal {MINIMUM_LP_UNIK}."
            ));
            Kriteria::Gagal
        }
        Some(n) => {
            catatan.push(format!(
                "Baru {n} alamat LP aktif yang terbukti unik. Histori posisi belum lengkap, jadi ini batas bawah dari syarat {MINIMUM_LP_UNIK}."
            ));
            Kriteria::BelumTerukur
        }
        None => {
            catatan.push("Jumlah alamat LP unik gagal dibaca dari rantai.".to_string());
            Kriteria::BelumTerukur
        }
    };

    let semua = [
        kriteria_pool,
        kriteria_umur,
        kriteria_upgrade,
        kriteria_tvl,
        kriteria_lp,
    ];
    let lolos_otomatis = semua.iter().all(|k| *k == Kriteria::Lolos);

    Ok(PenilaianHook {
        hook: hook.to_string(),
        code_hash: None,
        jumlah_pool,
        umur_hari,
        bisa_diupgrade,
        tvl_usd: metrik.tvl_usd,
        lp_unik: metrik.lp_unik,
        kriteria_pool,
        kriteria_umur,
        kriteria_upgrade,
        kriteria_tvl,
        kriteria_lp,
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
    if adopsi.sampai_blok > sampai_blok
        || (adopsi.sampai_blok > 0 && adopsi.dari_blok < lantai)
        || (!adopsi.peta.is_empty() && adopsi.pools.is_empty())
    {
        adopsi = Adopsi::default();
    }

    let mut potongan_terpakai = 0usize;

    // ARAH PERTAMA: blok baru sejak pemindaian terakhir.
    // Didahulukan karena inilah yang membuat hitungan tetap mutakhir, dan
    // jumlahnya biasanya kecil.
    if adopsi.sampai_blok > 0 && adopsi.sampai_blok < sampai_blok {
        let mut mulai = adopsi.sampai_blok + 1;
        while mulai <= sampai_blok && potongan_terpakai < MAKS_POTONGAN_PER_JALAN {
            let (baru, hingga) = baca_potongan(provider, pool_manager, sampai_blok, mulai).await?;
            potongan_terpakai += 1;
            for (pool, blok) in baru {
                adopsi.catat(pool, blok);
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
                    adopsi.catat(pool_adopsi(&d), log.block_number.unwrap_or(u64::MAX));
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

    // Token ID baru dipindai setelah daftar pool lengkap. Kalau dimulai lebih
    // awal, event untuk pool yang belum ditemukan akan terbuang dan tidak akan
    // pernah terbaca ulang setelah pool tersebut masuk cache.
    if adopsi.lengkap {
        pindai_posisi(provider, registry, &mut adopsi, sampai_blok).await?;
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
) -> Result<(Vec<(PoolAdopsi, u64)>, u64)> {
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
            out.push((pool_adopsi(&d), log.block_number.unwrap_or(u64::MAX)));
        }
    }
    Ok((out, hingga))
}

fn pool_adopsi(d: &abi::Initialize) -> PoolAdopsi {
    PoolAdopsi {
        id: format!("{:#x}", d.id),
        currency0: d.currency0,
        currency1: d.currency1,
        fee: d.fee.to::<u32>(),
        tick_spacing: d.tickSpacing.as_i32(),
        hooks: d.hooks,
    }
}

/// Indeks pool setiap token ID posisi dari event ModifyLiquidity.
async fn pindai_posisi<P: Provider>(
    provider: &P,
    registry: &Registry,
    adopsi: &mut Adopsi,
    sampai_blok: u64,
) -> Result<()> {
    let pool_manager = registry.contract("uniswap_v4", "pool_manager")?;
    let position_manager = registry.contract("uniswap_v4", "position_manager")?;
    let lantai = registry.chain.deploy_block;

    if adopsi.posisi_sampai_blok > sampai_blok
        || (adopsi.posisi_sampai_blok > 0 && adopsi.posisi_dari_blok < lantai)
    {
        adopsi.token_pool.clear();
        adopsi.posisi_dari_blok = 0;
        adopsi.posisi_sampai_blok = 0;
        adopsi.posisi_lengkap = false;
    }

    let filter = |dari, sampai| {
        Filter::new()
            .address(pool_manager)
            .event_signature(ModifyLiquidity::SIGNATURE_HASH)
            .topic2(position_manager.into_word())
            .from_block(dari)
            .to_block(sampai)
    };
    let mut terpakai = 0usize;

    if adopsi.posisi_sampai_blok > 0 && adopsi.posisi_sampai_blok < sampai_blok {
        let mut mulai = adopsi.posisi_sampai_blok + 1;
        while mulai <= sampai_blok && terpakai < MAKS_POTONGAN_PER_JALAN {
            let hingga = (mulai + POTONGAN_ADOPSI).min(sampai_blok);
            let logs = crate::chain::logs::baca_rentang(
                provider,
                &filter(mulai, hingga),
                "pembacaan ModifyLiquidity blok baru gagal",
            )
            .await?;
            catat_token_pool(adopsi, &logs);
            adopsi.posisi_sampai_blok = hingga;
            terpakai += 1;
            mulai = hingga.saturating_add(1);
        }
    }

    if !adopsi.posisi_lengkap {
        let mut hingga = if adopsi.posisi_dari_blok > lantai {
            adopsi.posisi_dari_blok.saturating_sub(1)
        } else {
            sampai_blok
        };
        while hingga > lantai && terpakai < MAKS_POTONGAN_PER_JALAN {
            let potongan = crate::chain::logs::baca_mundur(
                provider,
                hingga,
                lantai,
                POTONGAN_ADOPSI,
                POTONGAN_ADOPSI_MINIMUM,
                filter,
            )
            .await
            .context("pembacaan ModifyLiquidity untuk LP unik gagal")?;
            catat_token_pool(adopsi, &potongan.logs);
            adopsi.posisi_dari_blok = potongan.dari_blok;
            adopsi.posisi_sampai_blok = adopsi.posisi_sampai_blok.max(potongan.sampai_blok);
            terpakai += 1;
            if potongan.dari_blok <= lantai {
                adopsi.posisi_lengkap = true;
                break;
            }
            hingga = potongan.dari_blok.saturating_sub(1);
        }
    }
    Ok(())
}

fn catat_token_pool(adopsi: &mut Adopsi, logs: &[alloy::rpc::types::Log]) {
    for log in logs {
        if log.removed {
            continue;
        }
        let Ok(d) = ModifyLiquidity::decode_log(&log.inner) else {
            continue;
        };
        let token_id = U256::from_be_bytes(d.salt.0);
        if token_id.is_zero() {
            continue;
        }
        adopsi
            .token_pool
            .insert(token_id.to_string(), format!("{:#x}", d.id).to_lowercase());
    }
}

async fn ukur_metrik<P: Provider>(
    provider: &P,
    registry: &Registry,
    adopsi: &Adopsi,
    hook: Address,
) -> Result<MetrikAdopsi> {
    let (tvl_usd, tvl_lengkap) = ukur_tvl(provider, registry, adopsi, hook)
        .await
        .unwrap_or((None, false));
    let (lp_unik, lp_lengkap) = ukur_lp_unik(provider, registry, adopsi, hook)
        .await
        .unwrap_or((None, false));
    Ok(MetrikAdopsi {
        tvl_usd,
        tvl_lengkap,
        lp_unik,
        lp_lengkap,
    })
}

async fn ukur_lp_unik<P: Provider>(
    provider: &P,
    registry: &Registry,
    adopsi: &Adopsi,
    hook: Address,
) -> Result<(Option<usize>, bool)> {
    let pool_ids: HashSet<&str> = adopsi
        .pools
        .values()
        .filter(|p| p.hooks == hook)
        .map(|p| p.id.as_str())
        .collect();
    let token_ids: Vec<U256> = adopsi
        .token_pool
        .iter()
        .filter(|(_, pool)| pool_ids.contains(pool.as_str()))
        .filter_map(|(token, _)| token.parse().ok())
        .collect();

    let pm = registry.contract("uniswap_v4", "position_manager")?;
    let multicall = registry.contract("common", "multicall3")?;
    let calls = token_ids
        .iter()
        .map(|token| {
            crate::chain::multicall::encode(
                pm,
                &IPositionManagerV4::ownerOfCall { tokenId: *token },
            )
        })
        .collect();
    let hasil = crate::chain::multicall::batch_call(provider, multicall, calls).await?;
    let pemilik: HashSet<Address> = hasil
        .iter()
        .filter_map(|raw| {
            crate::chain::multicall::decode::<IPositionManagerV4::ownerOfCall>(raw.as_ref())
        })
        .filter(|owner| !owner.is_zero())
        .collect();
    Ok((Some(pemilik.len()), adopsi.lengkap && adopsi.posisi_lengkap))
}

async fn harga_eth_usd<P: Provider>(provider: &P, registry: &Registry) -> Option<f64> {
    let weth: Address = registry.tokens.get("weth")?.address.parse().ok()?;
    let usdg: Address = registry.tokens.get("usdg")?.address.parse().ok()?;
    let pools = crate::pools::discover_v3(provider, registry, weth, usdg)
        .await
        .ok()?;
    pools
        .into_iter()
        .filter_map(|p| {
            let harga = if p.token0.eq_ignore_ascii_case(&weth.to_string()) {
                p.raw_price * 1e18 / 1e6
            } else {
                (1.0 / p.raw_price) * 1e18 / 1e6
            };
            let liquidity = p.liquidity.parse::<u128>().ok()?;
            (harga.is_finite() && (100.0..=100_000.0).contains(&harga))
                .then_some((liquidity, harga))
        })
        .max_by_key(|(liquidity, _)| *liquidity)
        .map(|(_, harga)| harga)
}

async fn ukur_tvl<P: Provider>(
    provider: &P,
    registry: &Registry,
    adopsi: &Adopsi,
    hook: Address,
) -> Result<(Option<f64>, bool)> {
    let target: Vec<&PoolAdopsi> = adopsi.pools.values().filter(|p| p.hooks == hook).collect();
    if target.is_empty() {
        return Ok((Some(0.0), adopsi.lengkap));
    }

    let weth: Address = registry
        .tokens
        .get("weth")
        .context("token weth tidak ada di registry")?
        .address
        .parse()?;
    let usdg: Address = registry
        .tokens
        .get("usdg")
        .context("token usdg tidak ada di registry")?
        .address
        .parse()?;
    let eth_usd = harga_eth_usd(provider, registry).await;
    let state_view = registry.contract("uniswap_v4", "state_view")?;
    let multicall = registry.contract("common", "multicall3")?;

    let mut token_meta = Vec::new();
    let mut terlihat = HashSet::new();
    for p in &target {
        for token in [p.currency0, p.currency1] {
            if !token.is_zero() && token != weth && token != usdg && terlihat.insert(token) {
                token_meta.push(token);
            }
        }
    }
    let metadata = crate::pools::token_meta_batch(provider, registry, &token_meta).await?;
    let decimals: HashMap<Address, u8> = token_meta
        .into_iter()
        .zip(metadata)
        .filter_map(|(alamat, meta)| meta.map(|m| (alamat, m.decimals)))
        .collect();

    let mut calls = Vec::with_capacity(target.len() * 2);
    for p in &target {
        let id: B256 = p.id.parse()?;
        calls.push(crate::chain::multicall::encode(
            state_view,
            &abi::IStateView::getSlot0Call { poolId: id },
        ));
        calls.push(crate::chain::multicall::encode(
            state_view,
            &abi::IStateView::getLiquidityCall { poolId: id },
        ));
    }
    let state = crate::chain::multicall::batch_call(provider, multicall, calls).await?;

    let mut daftar = Vec::new();
    let mut kurs = HashMap::new();
    for (i, p) in target.iter().enumerate() {
        let Some(slot) = crate::chain::multicall::decode::<abi::IStateView::getSlot0Call>(
            state.get(i * 2).and_then(|x| x.as_ref()),
        ) else {
            continue;
        };
        let Some(liquidity) = crate::chain::multicall::decode::<abi::IStateView::getLiquidityCall>(
            state.get(i * 2 + 1).and_then(|x| x.as_ref()),
        ) else {
            continue;
        };

        let quote = if p.currency0 == usdg || p.currency1 == usdg {
            Some((usdg, 6u8, 1.0))
        } else if p.currency0.is_zero() || p.currency1.is_zero() {
            eth_usd.map(|usd| (Address::ZERO, 18u8, usd))
        } else if p.currency0 == weth || p.currency1 == weth {
            eth_usd.map(|usd| (weth, 18u8, usd))
        } else {
            None
        };
        let Some((quote, quote_decimals, quote_usd)) = quote else {
            continue;
        };
        let meme = if p.currency0 == quote {
            p.currency1
        } else {
            p.currency0
        };
        let meme_decimals = if meme.is_zero() || meme == weth {
            18
        } else if meme == usdg {
            6
        } else if let Some(v) = decimals.get(&meme) {
            *v
        } else {
            continue;
        };
        daftar.push((
            p.id.clone(),
            crate::pools::distribusi::SumberTick::V4 { state_view },
            crate::pools::distribusi::KeadaanPool {
                tick_spacing: p.tick_spacing,
                tick_sekarang: slot.tick.as_i32(),
                likuiditas: liquidity,
                quote_adalah_token1: p.currency1 == quote,
                skala_meme: 10f64.powi(meme_decimals as i32),
                skala_quote: 10f64.powi(quote_decimals as i32),
            },
        ));
        kurs.insert(p.id.to_lowercase(), quote_usd);
    }

    let tvl = crate::pools::distribusi::tvl_beberapa(provider, multicall, &daftar).await;
    let total = tvl
        .iter()
        .filter_map(|(id, value)| kurs.get(id).map(|usd| value * usd))
        .sum::<f64>();
    let lengkap = adopsi.lengkap && daftar.len() == target.len() && tvl.len() == target.len();
    Ok((total.is_finite().then_some(total), lengkap))
}

/// Hash kode aktual. Alamat tanpa kode tidak menjadi bukti hook yang sah.
pub async fn code_hash<P: Provider>(provider: &P, alamat: Address) -> Result<String> {
    let code = provider
        .get_code_at(alamat)
        .await
        .context("bytecode hook tidak terbaca")?;
    anyhow::ensure!(!code.is_empty(), "alamat hook tidak mempunyai bytecode");
    Ok(format!("{:#x}", alloy::primitives::keccak256(code)))
}

/// Periksa empat slot proxy standar; satu slot terisi cukup untuk menolak.
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
    fn lima_kriteria_yang_terbukti_membolehkan_hook_otomatis() {
        let p = susun_dengan_metrik(
            HOOK,
            Some(50),
            Some(14.0),
            Some(false),
            true,
            MetrikAdopsi {
                tvl_usd: Some(250_000.0),
                tvl_lengkap: true,
                lp_unik: Some(200),
                lp_lengkap: true,
            },
        )
        .unwrap();
        assert!(p.lolos_otomatis);
        assert_eq!(p.kriteria_tvl, Kriteria::Lolos);
        assert_eq!(p.kriteria_lp, Kriteria::Lolos);
    }

    #[test]
    fn pool_initialize_yang_sama_tidak_dihitung_dua_kali() {
        let hook: Address = HOOK.parse().unwrap();
        let pool = PoolAdopsi {
            id: format!("{:#x}", B256::repeat_byte(1)),
            currency0: Address::repeat_byte(2),
            currency1: Address::repeat_byte(3),
            fee: 3_000,
            tick_spacing: 60,
            hooks: hook,
        };
        let mut adopsi = Adopsi::default();
        adopsi.catat(pool.clone(), 100);
        adopsi.catat(pool, 100);
        assert_eq!(adopsi.peta.get(&hook), Some(&(1, 100)));
        assert_eq!(adopsi.pools.len(), 1);
    }

    #[test]
    fn metrik_sebagian_hanya_bisa_membuktikan_lolos() {
        let kurang = susun_dengan_metrik(
            HOOK,
            Some(50),
            Some(14.0),
            Some(false),
            true,
            MetrikAdopsi {
                tvl_usd: Some(10_000.0),
                tvl_lengkap: false,
                lp_unik: Some(3),
                lp_lengkap: false,
            },
        )
        .unwrap();
        assert_eq!(kurang.kriteria_tvl, Kriteria::BelumTerukur);
        assert_eq!(kurang.kriteria_lp, Kriteria::BelumTerukur);

        let cukup = susun_dengan_metrik(
            HOOK,
            Some(50),
            Some(14.0),
            Some(false),
            false,
            MetrikAdopsi {
                tvl_usd: Some(300_000.0),
                tvl_lengkap: false,
                lp_unik: Some(250),
                lp_lengkap: false,
            },
        )
        .unwrap();
        assert!(cukup.lolos_otomatis);
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
