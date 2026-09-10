//! Penyimpanan lokal berbasis redb.
//!
//! INVARIAN
//! - Penyimpanan ini lokal dan milik satu pengguna. Tidak ada sinkronisasi,
//!   tidak ada server. Lihat PRD 9.4.
//! - Nilai disimpan sebagai JSON supaya berkasnya bisa diperiksa dan skemanya
//!   bisa berkembang tanpa migrasi biner.
//!
//! CATATAN KEPUTUSAN
//! PRD 7.2 semula menyebut SQLite lewat sqlx. redb dipilih karena Rust murni,
//! saat itu karena mesin ini dikira tidak punya toolchain C. Alasan itu sudah
//! gugur, MSVC lengkap sekarang terpasang, jadi SQLite bisa dipakai kapan saja.
//!
//! redb tetap dipertahankan untuk sementara karena fase 1 hanya menyimpan
//! pengaturan dan penghitung ringan, sehingga belum ada yang menuntut SQL.
//! Konsekuensinya: tidak ada kueri relasional, jadi agregasi untuk tab PnL dan
//! kalibrasi harus ditulis manual. Kalau itu mulai terasa berat, pindah ke
//! sqlx. Lihat docs/DECISIONS.md entri 0004.

use std::path::Path;

use anyhow::{Context, Result};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{de::DeserializeOwned, Serialize};

/// Pengaturan dan penghitung ringan. Kunci bebas, nilai JSON.
const SETTINGS: TableDefinition<&str, &str> = TableDefinition::new("settings");

pub struct Store {
    db: Database,
}

impl Store {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("rangefinder.redb");
        let db = Database::create(&path)
            .with_context(|| format!("gagal membuka basis data di {}", path.display()))?;

        // Buat tabel sekali di awal supaya pembacaan pertama tidak gagal
        // hanya karena tabelnya belum pernah ditulis.
        let tx = db.begin_write()?;
        {
            let _ = tx.open_table(SETTINGS)?;
        }
        tx.commit()?;

        Ok(Self { db })
    }

    pub fn put<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let json = serde_json::to_string(value)?;
        let tx = self.db.begin_write()?;
        {
            let mut table = tx.open_table(SETTINGS)?;
            table.insert(key, json.as_str())?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(SETTINGS)?;
        let Some(raw) = table.get(key)? else {
            return Ok(None);
        };
        let value = serde_json::from_str(raw.value())
            .with_context(|| format!("nilai untuk kunci `{key}` bukan JSON yang cocok"))?;
        Ok(Some(value))
    }

    pub fn delete(&self, key: &str) -> Result<bool> {
        let tx = self.db.begin_write()?;
        let existed;
        {
            let mut table = tx.open_table(SETTINGS)?;
            existed = table.remove(key)?.is_some();
        }
        tx.commit()?;
        Ok(existed)
    }

    /// Baca satu namespace dari satu snapshot redb, tanpa menyembunyikan JSON rusak.
    pub fn prefix<T: DeserializeOwned>(&self, prefix: &str) -> Result<Vec<(String, T)>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(SETTINGS)?;
        let mut out = Vec::new();
        for item in table.range(prefix..)? {
            let (key, value) = item?;
            if !key.value().starts_with(prefix) {
                break;
            }
            out.push((
                key.value().to_owned(),
                serde_json::from_str(value.value())
                    .with_context(|| format!("catatan {} tidak valid", key.value()))?,
            ));
        }
        Ok(out)
    }

    /// Read-modify-write dalam satu transaksi, termasuk validasi sebelum commit.
    pub fn update<T: DeserializeOwned + Serialize>(
        &self,
        key: &str,
        change: impl FnOnce(Option<T>) -> Result<T>,
    ) -> Result<T> {
        let tx = self.db.begin_write()?;
        let updated;
        {
            let mut table = tx.open_table(SETTINGS)?;
            let previous = table
                .get(key)?
                .map(|raw| serde_json::from_str(raw.value()))
                .transpose()?;
            updated = change(previous)?;
            let json = serde_json::to_string(&updated)?;
            table.insert(key, json.as_str())?;
        }
        tx.commit()?;
        Ok(updated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Contoh {
        a: u32,
        b: String,
    }

    fn dir_sementara(nama: &str) -> std::path::PathBuf {
        let d =
            std::env::temp_dir().join(format!("rangefinder-store-{nama}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn simpan_baca_hapus() {
        let dir = dir_sementara("dasar");
        let store = Store::open(&dir).unwrap();

        assert!(store.get::<Contoh>("belum_ada").unwrap().is_none());

        let nilai = Contoh {
            a: 42,
            b: "halo".into(),
        };
        store.put("kunci", &nilai).unwrap();
        assert_eq!(store.get::<Contoh>("kunci").unwrap(), Some(nilai));

        assert!(store.delete("kunci").unwrap());
        assert!(!store.delete("kunci").unwrap());
        assert!(store.get::<Contoh>("kunci").unwrap().is_none());

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nilai_bertahan_setelah_dibuka_ulang() {
        let dir = dir_sementara("persist");
        {
            let store = Store::open(&dir).unwrap();
            store.put("angka", &7u32).unwrap();
        }
        {
            let store = Store::open(&dir).unwrap();
            assert_eq!(store.get::<u32>("angka").unwrap(), Some(7));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
