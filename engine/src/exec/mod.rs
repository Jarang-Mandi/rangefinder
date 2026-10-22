//! Eksekusi posisi Uniswap.
//!
//! INVARIAN
//! - Setiap transaksi wajib melewati seluruh gate di PRD 6.4, tanpa jalan
//!   pintas. Gate G2 ada di `calldata`, G3 di uji golden vector, sisanya
//!   menyusul di modul masing masing.
//! - Modul ini satu satunya tempat yang boleh menyusun jalur tulis. Modul
//!   `pools` sengaja hanya membaca.

pub mod abi_write;
pub mod aksi;
pub mod approve;
pub mod calldata;
pub mod exit;
pub mod golden;
pub mod journal;
pub mod limits;
pub mod nonce;
pub mod pipeline;
pub mod plan;
pub mod router_rbh;
pub mod send;
pub mod simulate;
pub mod v4;
pub mod verify;
pub mod zap;
