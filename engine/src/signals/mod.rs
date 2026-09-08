//! Mesin sinyal: mencari kondisi pasar yang layak dimasuki sebagai LP.
//!
//! KENAPA SELURUHNYA MURNI
//! Setiap detektor di sini menerima deret candle dan mengembalikan angka. Tidak
//! ada yang menyentuh jaringan, dan tidak ada yang menyimpan keadaan. Itu
//! disengaja: bagian ini yang menentukan di mana uang ditaruh, dan bagian
//! seperti itu harus bisa diuji ribuan kali tanpa rantai.
//!
//! INVARIAN
//! - Bar datang dari chart gmgn, bukan dari sumber luar. Angka yang dipakai
//!   untuk keputusan harus sama dengan yang dilihat user di layar.
//! - Detektor yang datanya kurang mengembalikan `None`, bukan skor rendah.
//!   Skor rendah berarti "sudah diperiksa dan tidak menarik", sedangkan data
//!   yang kurang berarti "belum bisa diperiksa". Menyamakan keduanya membuat
//!   token yang belum termuat penuh terlihat seperti token yang buruk.

pub mod bar;
pub mod dump_basis;
pub mod metrik;
pub mod penempatan;
pub mod sideways;
