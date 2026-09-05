//! Eksekusi posisi Uniswap.
//!
//! INVARIAN
//! - Setiap transaksi wajib melewati seluruh gate di PRD 6.4, tanpa jalan
//!   pintas. Gate G2 ada di `calldata`, G3 di uji golden vector, sisanya
//!   menyusul di modul masing masing.
//! - Modul ini satu satunya tempat yang boleh menyusun jalur tulis. Modul
//!   `pools` sengaja hanya membaca.

pub mod abi_write;
pub mod calldata;
pub mod golden;
