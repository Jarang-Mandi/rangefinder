//! Simpanan kunci pool V4 supaya pemuatan kedua tidak memindai ulang sejarah.
//!
//! KENAPA MODUL INI ADA
//! Menemukan pool V4 berarti memindai event `Initialize`, dan itu satu satunya
//! cara karena V4 tidak punya factory yang bisa dienumerasi. Pemindaian tiga
//! juta blok memakan sekitar satu detik per arah, dan itu terjadi setiap kali
//! user membuka token yang sama. Zenith LP terasa seketika karena ia membaca
//! basis data indexer, bukan karena RPC-nya lebih cepat.
//!
//! Yang membuat cache ini aman adalah sifat datanya: PoolKey bersifat tetap.
//! Sebuah pool yang sudah ada tidak pernah berubah currency, fee, tick spacing,
//! atau hook-nya. Yang berubah hanya harga dan likuiditas, dan keduanya memang
//! selalu dibaca ulang dari rantai.
//!
//! INVARIAN
//! - Cache hanya menyimpan KUNCI pool, tidak pernah harga atau likuiditas.
//!   Menyimpan harga berarti suatu saat menampilkan angka basi sebagai angka
//!   sekarang, dan pada alat yang mengirim transaksi itu tidak bisa diterima.
//! - `scanned_to` adalah blok terakhir yang benar benar sudah dipindai. Kalau
//!   pemindaian gagal, nilainya TIDAK dimajukan, supaya blok yang terlewat
//!   dipindai lagi lain kali.
//! - Cache yang rusak atau berbeda versi diperlakukan seperti cache kosong,
//!   bukan sebagai kesalahan fatal. Kehilangan cache hanya membuat lambat,
//!   sedangkan menolak jalan membuat alatnya tidak berguna.

use serde::{Deserialize, Serialize};

use crate::store::Store;

/// Dinaikkan kalau bentuk `PoolKeyRecord` berubah, supaya entri lama diabaikan
/// alih alih dibaca dengan skema yang salah.
const VERSI: u32 = 1;

/// Kunci pool V4 yang tidak pernah berubah setelah pool dibuat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolKeyRecord {
    pub id: String,
    pub currency0: String,
    pub currency1: String,
    pub fee: u32,
    pub tick_spacing: i32,
    pub hooks: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CachedKeys {
    pub scanned_to: u64,
    pub keys: Vec<PoolKeyRecord>,
}

fn kunci_simpanan(token: &str) -> String {
    format!("poolkeys:v{VERSI}:{}", token.to_lowercase())
}

/// Baca kunci pool yang sudah pernah ditemukan untuk sebuah token.
///
/// Kegagalan membaca dikembalikan sebagai cache kosong beserta peringatan.
/// Basis data yang bermasalah tidak boleh menghentikan pencarian pool.
pub fn baca(store: &Store, token: &str) -> CachedKeys {
    match store.get::<CachedKeys>(&kunci_simpanan(token)) {
        Ok(Some(c)) => c,
        Ok(None) => CachedKeys::default(),
        Err(e) => {
            tracing::warn!(token, error = %e, "cache kunci pool tidak terbaca, dipindai ulang");
            CachedKeys::default()
        }
    }
}

/// Gabungkan kunci baru ke cache lalu simpan.
///
/// Mengembalikan seluruh kunci yang diketahui setelah penggabungan, supaya
/// pemanggil tidak perlu membaca ulang.
pub fn gabung(
    store: &Store,
    token: &str,
    mut lama: CachedKeys,
    baru: Vec<PoolKeyRecord>,
    scanned_to: u64,
) -> CachedKeys {
    let mut ada: std::collections::HashSet<String> =
        lama.keys.iter().map(|k| k.id.to_lowercase()).collect();

    let mut ditambah = 0usize;
    for k in baru {
        if ada.insert(k.id.to_lowercase()) {
            lama.keys.push(k);
            ditambah += 1;
        }
    }
    lama.scanned_to = scanned_to;

    if let Err(e) = store.put(&kunci_simpanan(token), &lama) {
        // Gagal menyimpan hanya berarti pemuatan berikutnya lambat lagi.
        tracing::warn!(token, error = %e, "cache kunci pool gagal disimpan");
    } else if ditambah > 0 {
        tracing::debug!(
            token,
            ditambah,
            total = lama.keys.len(),
            "cache kunci pool diperbarui"
        );
    }

    lama
}

/// Buang cache sebuah token. Dipakai kalau isinya terbukti tidak masuk akal.
pub fn lupakan(store: &Store, token: &str) {
    let _ = store.delete(&kunci_simpanan(token));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_sementara(nama: &str) -> (Store, std::path::PathBuf) {
        let d =
            std::env::temp_dir().join(format!("rangefinder-cache-{nama}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        (Store::open(&d).unwrap(), d)
    }

    fn rekaman(id: &str) -> PoolKeyRecord {
        PoolKeyRecord {
            id: id.into(),
            currency0: "0xaaa".into(),
            currency1: "0xbbb".into(),
            fee: 3000,
            tick_spacing: 60,
            hooks: "0x0".into(),
        }
    }

    #[test]
    fn cache_kosong_saat_belum_pernah_ditulis() {
        let (store, dir) = store_sementara("kosong");
        let c = baca(&store, "0xTOKEN");
        assert_eq!(c.scanned_to, 0);
        assert!(c.keys.is_empty());
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn kunci_bertahan_dan_tidak_terduplikasi() {
        let (store, dir) = store_sementara("gabung");
        let awal = baca(&store, "0xTOKEN");
        let setelah = gabung(
            &store,
            "0xTOKEN",
            awal,
            vec![rekaman("0x1"), rekaman("0x2")],
            100,
        );
        assert_eq!(setelah.keys.len(), 2);
        assert_eq!(setelah.scanned_to, 100);

        // Pemindaian berikutnya menemukan satu pool lama dan satu pool baru.
        let lagi = baca(&store, "0xTOKEN");
        let setelah = gabung(
            &store,
            "0xTOKEN",
            lagi,
            vec![rekaman("0x2"), rekaman("0x3")],
            200,
        );
        assert_eq!(
            setelah.keys.len(),
            3,
            "pool yang sudah ada tidak boleh dobel"
        );
        assert_eq!(setelah.scanned_to, 200);

        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn perbandingan_id_tidak_peduli_besar_kecil_huruf() {
        // poolId datang dari log sebagai heksadesimal huruf kecil, tapi bisa
        // saja tersimpan dengan gaya lain. Kalau perbandingannya sensitif
        // huruf, pool yang sama akan menumpuk setiap kali dipindai.
        let (store, dir) = store_sementara("huruf");
        let awal = baca(&store, "0xTOKEN");
        let setelah = gabung(&store, "0xTOKEN", awal, vec![rekaman("0xAbC")], 10);
        let lagi = baca(&store, "0xTOKEN");
        let setelah2 = gabung(&store, "0xTOKEN", lagi, vec![rekaman("0xabc")], 20);
        assert_eq!(setelah.keys.len(), 1);
        assert_eq!(setelah2.keys.len(), 1);
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn token_berbeda_punya_ruang_sendiri() {
        let (store, dir) = store_sementara("terpisah");
        gabung(&store, "0xA", baca(&store, "0xA"), vec![rekaman("0x1")], 10);
        gabung(&store, "0xB", baca(&store, "0xB"), vec![rekaman("0x9")], 10);
        assert_eq!(baca(&store, "0xA").keys.len(), 1);
        assert_eq!(baca(&store, "0xB").keys[0].id, "0x9");
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn alamat_token_tidak_peduli_besar_kecil_huruf() {
        let (store, dir) = store_sementara("token-huruf");
        gabung(
            &store,
            "0xAbCd",
            baca(&store, "0xAbCd"),
            vec![rekaman("0x1")],
            10,
        );
        assert_eq!(
            baca(&store, "0xABCD").keys.len(),
            1,
            "alamat yang sama harus menunjuk entri cache yang sama"
        );
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }
}
