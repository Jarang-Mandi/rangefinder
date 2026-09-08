//! Gate risiko sebelum modal masuk, PRD 5.4.
//!
//! Dua kelompok. Kelompok pertama murah dan datang dari data gmgn, kelompok
//! kedua mahal dan datang dari simulasi on chain. Yang ada di sini adalah
//! kelompok kedua, karena kelompok pertama belum dipakai di jalur mana pun.
//!
//! SIKAP BAWAAN ADALAH MENOLAK
//! Setiap gate di sini menjawab pertanyaan "sudah terbukti aman?", bukan
//! "sudah terbukti berbahaya?". Perbedaannya menentukan apa yang terjadi saat
//! pembacaan gagal: yang pertama menolak, yang kedua meloloskan. Modal yang
//! tidak bisa ditarik kembali bukan risiko harga yang bisa ditanggung.

pub mod hook;
pub mod persetujuan;
