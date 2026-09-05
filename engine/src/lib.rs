//! Pustaka engine Rangefinder.
//!
//! Modul modulnya diekspos sebagai pustaka supaya `xtask` bisa memakai ulang
//! tipe yang sama, terutama `chain::registry`. Ini menegakkan invarian penting:
//! engine dan perintah verifikasi membaca `chain.toml` lewat kode yang sama,
//! sehingga tidak mungkin ada dua tafsir yang berbeda atas satu berkas.
//!
//! Biner `rangefinder` di `main.rs` hanya merangkai modul modul ini.

pub mod api;
pub mod auth;
pub mod chain;
pub mod config;
pub mod error;
pub mod math;
pub mod pools;
pub mod store;
pub mod telemetry;
pub mod wallet;
