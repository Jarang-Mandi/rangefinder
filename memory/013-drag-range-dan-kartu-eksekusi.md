# Drag range dan kartu eksekusi

**Tanggal:** 10 September 2026.

## Gejala yang terlihat sama punya dua penyebab

Setelah handle histogram dilepas, panel sempat menggambar batas lama sebelum
jawaban `set_range` kembali dari engine. Penyebabnya `Histogram.commit`
langsung membuang draft lokal, padahal props masih berisi range sebelum drag.
Solusinya mempertahankan draft sebagai posisi optimistis dan baru membuangnya
saat props hasil engine berubah.

Chart GMGN berkedip ketika perubahan berasal dari panel karena bridge menghapus
dua shape lama sebelum dua `createShape` baru selesai. Drag langsung pada chart
tidak melewati jalur gambar ulang itu, sehingga tidak menunjukkan gejala yang
sama. Jalur normal sekarang memakai `getShapeById(...).setPoints(...)` untuk
memindahkan kedua garis yang sudah ada. Kalau shape hilang, bridge membuat
pasangan pengganti sampai lengkap sebelum menghapus pasangan lama.

Pemindahan lewat API dapat memicu `drawing_event`. Harga yang baru dipasang
disimpan sementara per shape agar event itu tidak dianggap sebagai drag user
dan dikirim kembali ke engine. Event pada harga lain tetap diteruskan seperti
biasa.

Fixture histogram sengaja menunda pengakuan props 300 milidetik. Pemeriksaan
Playwright membandingkan koordinat handle tepat setelah pointer dilepas dan
setelah pengakuan tiba. Keduanya harus sama, bukan hanya memastikan nilai akhir
akhirnya benar.

## Pola kartu eksekusi dari acuan MMT

Saat Zap aktif, hanya aset sumber yang perlu ditampilkan. Pilihan aset berada
di token pill dalam kotak jumlah, sehingga tidak perlu baris tombol pemilih
terpisah. Saat Zap mati, kedua kotak tampil karena kedua sisi perlu diisi.
Urutan visualnya: judul dan sakelar, label jumlah, angka besar dan token pill,
nilai fiat dan saldo, tombol utama, lalu estimasi gas dan APR.

Header tidak membutuhkan ikon range karena konteks range sudah jelas dari tab
dan kartu di atasnya. Token pill tidak memakai lingkaran berisi huruf sebagai
logo semu. Protokol belum membawa URL atau aset logo yang terverifikasi, dan
mencarinya dari layanan luar akan menambah request, cache, CSP, serta risiko
gambar salah. Sampai aset itu tersedia dari sumber tepercaya, ticker saja lebih
ringan dan jujur.

`fixtures/execution.html` memakai komponen produksi tanpa engine atau wallet.
`check-execution.cjs` membuktikan jumlah input pada kedua keadaan Zap,
pergantian aset, payload setoran, dan tidak adanya scroll horizontal pada lebar
320, 360, serta 480 piksel.

## Mode simulasi masih rancangan

Sakelar `Simulasi | Tx Asli` belum boleh diimplementasikan pada perubahan ini.
Kontrak produknya dicatat lebih dulu di PRD 9.2.1. Dry run yang sekarang tetap
berlaku sampai migrasi engine, protokol, penyimpanan posisi virtual, dan UI
dikerjakan sebagai satu perubahan tersendiri.
