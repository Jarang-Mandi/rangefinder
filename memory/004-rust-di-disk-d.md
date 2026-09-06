# 004 Rust dipindahkan ke disk D

Ditulis 6 September 2026.

## Kenapa

Disk C hanya menyisakan sekitar 8,6 GB dari 124 GB, sedangkan instalasi Rust
memakan 3,47 GB: toolchain 2,94 GB ditambah cache registry dan biner cargo
0,53 GB. Disk D punya 74 GB kosong.

## Keadaan sekarang

| Variabel | Nilai |
| --- | --- |
| `RUSTUP_HOME` | `D:\Rust\rustup` |
| `CARGO_HOME` | `D:\Rust\cargo` |
| PATH user | memuat `D:\Rust\cargo\bin` |

Ketiganya variabel lingkungan tingkat user. `%USERPROFILE%\.cargo` dan
`%USERPROFILE%\.rustup` sudah dihapus.

## Cara memindahkannya, kalau perlu diulang

Rustup memang mendukung ini lewat dua variabel di atas, jadi tidak perlu pasang
ulang. Urutannya penting:

1. Salin dulu, jangan pindahkan. `robocopy <sumber> <tujuan> /E /MT:8`.
2. Set `CARGO_HOME` dan `RUSTUP_HOME`, lalu ganti entri PATH.
3. Buka terminal baru, jalankan `rustup show`. Barisnya harus berbunyi
   `rustup home: D:\Rust\rustup`.
4. Baru setelah itu hapus direktori yang lama.

Menghapus lebih dulu berarti kalau ada yang meleset, Rust-nya hilang dan harus
diunduh ulang tiga giga.

## Dua jebakan

**Robocopy exit code 1 itu BERHASIL, bukan gagal.** Nol berarti tidak ada
berkas yang disalin, satu berarti berkas tersalin. Skrip yang memperlakukan
nilai bukan nol sebagai kegagalan akan salah melapor. Yang benar benar
kegagalan adalah delapan ke atas.

**`target/` menyimpan jalur absolut ke registry.** Setelah registry pindah,
sebagian besar crate perlu dikompilasi ulang sekali. Itu normal dan hanya
terjadi sekali, bukan tanda ada yang rusak.

## Yang harus dijaga

Jangan menghapus `RUSTUP_HOME` atau `CARGO_HOME` tanpa memindahkan isinya
kembali. Tanpa keduanya, rustup mencari di `%USERPROFILE%` yang sudah kosong,
lalu menawarkan pemasangan ulang dari nol seolah Rust belum pernah ada.
