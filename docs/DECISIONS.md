# Catatan keputusan arsitektural

Satu entri per keputusan. Keputusan yang dibatalkan ditandai, tidak dihapus,
supaya alasannya tetap bisa dibaca dan tidak diulang tanpa sadar.

Format: konteks, pilihan, alasan, konsekuensi, pemicu peninjauan ulang.

---

## 0001 Panggilan API gmgn lewat tab user, bukan dari engine

**Tanggal** 5 September 2026. **Status** berlaku.

**Konteks.** Engine butuh data token dari gmgn: peringkat, holder, harga,
metrik risiko. Ada dua cara: engine memanggil gmgn langsung lewat HTTP, atau
content script memanggilnya dari dalam tab gmgn milik user.

**Pilihan.** Semua panggilan gmgn dilakukan content script dari dalam tab user.
Engine tidak pernah memanggil gmgn sendiri.

**Alasan.** Pengamatan langsung menunjukkan request gmgn membawa parameter
fingerprint (`device_id`, `tab_id`, `fp_did`, `client_id`, `app_ver`, dan
lainnya) di samping cookie sesi. Meniru semua itu dari Rust rapuh, gampang kena
rate limit, dan akan rusak setiap kali gmgn memperbarui bundle. Memakai sesi
user yang memang sudah sah menghilangkan seluruh kelas masalah itu.

**Konsekuensi.** Fitur yang bergantung pada gmgn mati kalau tidak ada tab gmgn
terbuka. Itu diterima, dan panel wajib menyebutkan fitur mana yang mati. Posisi
yang sedang berjalan tetap terpantau karena hanya butuh RPC.

**Peninjauan ulang.** Kalau gmgn merilis API resmi berkunci yang mencakup data
yang kita butuhkan.

---

## 0002 Satu berkas registry kontrak, dipakai engine dan xtask

**Tanggal** 5 September 2026. **Status** berlaku.

**Konteks.** Alamat kontrak dibutuhkan engine saat runtime dan oleh perintah
verifikasi. Cara termudah adalah menulisnya sebagai konstanta Rust.

**Pilihan.** `chain.toml` di akar repo jadi sumber tunggal. Engine dan xtask
membacanya lewat modul `chain::registry` yang sama, karena xtask bergantung pada
pustaka engine.

**Alasan.** Dua daftar alamat pasti menyimpang cepat atau lambat, dan alamat
yang salah pada proyek yang mengirim transaksi berarti kehilangan dana. Berbagi
kode pembaca membuat penyimpangan mustahil, bukan sekadar tidak disarankan.

**Konsekuensi.** Engine wajib bisa menemukan akar repo saat runtime. Diselesaikan
lewat `config::repo_root` yang bisa ditimpa `RANGEFINDER_REPO_ROOT`.

**Peninjauan ulang.** Kalau engine dipaketkan sebagai biner lepas tanpa repo,
`chain.toml` harus ikut dibundel dan pencarian akarnya disesuaikan.

---

## 0003 Toolchain Windows dan penghalang raw-dylib

**Tanggal** 5 September 2026. **Status** SELESAI. Penghalangnya sudah hilang,
lihat bagian Penyelesaian di bawah.

**Konteks.** Mesin pengembangan tidak punya MSVC Build Tools dan tidak punya
gcc di PATH. Pemeriksaan menunjukkan direktori Visual Studio ada tapi kosong,
tanpa `cl.exe`, `link.exe`, maupun Windows SDK.

**Pilihan yang sudah diambil.**

1. `rust-toolchain.toml` dipin ke `stable-x86_64-pc-windows-gnu`. Menulis
   `channel = "stable"` saja teresolusi ke host default yaitu msvc, lalu build
   gagal dengan pesan `link: extra operand`, karena `link` yang ditemukan di
   PATH adalah perintah coreutils bawaan Git Bash, bukan linker MSVC. Pesan itu
   sangat menyesatkan dan memakan waktu, jadi jangan sampai terulang.
2. Semua dependensi dipilih Rust murni: `aes-gcm` dan `argon2` dari RustCrypto,
   `k256` bukan `secp256k1`, `redb` bukan SQLite, TLS lewat `native-tls` yang
   memakai SChannel bawaan Windows, bukan `rustls` dengan `ring`.
3. `.cargo/config.toml` memaksa `getrandom_backend="windows_legacy"` supaya
   getrandom memakai `RtlGenRandom` dari advapi32 dengan penautan biasa, bukan
   `ProcessPrng` lewat raw-dylib. Nilainya `windows_legacy`, bukan `windows7`.
   Nilai yang salah tidak menghasilkan error, cfg-nya hanya diabaikan diam diam.

**Penghalang yang belum selesai.** `windows-sys` memakai raw-dylib untuk
`ntdll.dll`, dan itu tidak bisa dimatikan lewat konfigurasi. rustup memang
membundel `dlltool.exe`, tapi tidak membundel `as.exe` yang dipanggil dlltool,
sehingga gagal dengan `dlltool could not create import library`. Karena
`windows-sys` dipakai tokio, dirs, keyring, dan banyak lainnya, penghalang ini
tidak bisa dihindari dengan mengganti dependensi.

**Dua jalan keluar, keduanya butuh pemasangan.**

| Jalan | Perintah | Ukuran | Akibat |
| --- | --- | --- | --- |
| MSVC Build Tools | `winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"` | sekitar 3 sampai 5 GB | Setelah itu `rust-toolchain.toml` boleh dikembalikan ke `channel = "stable"` dan `.cargo/config.toml` boleh dihapus. Ini jalur baku Rust di Windows dan menutup seluruh kelas masalah ini |
| MinGW-w64 lengkap | misalnya `winget install -e --id MSYS2.MSYS2` lalu pasang binutils | sekitar 300 MB sampai 1 GB | Menyediakan `as.exe` sehingga dlltool jalan. Toolchain tetap gnu, penyiasatan di repo tetap dibutuhkan |

### Penyelesaian

Ternyata **MSVC sudah terpasang sejak awal**, hanya di
`C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools`. Pencarian
pertama gagal karena hanya melihat `C:\Program Files\Microsoft Visual Studio`
yang memang kosong. `cl.exe`, `link.exe`, dan `lib.exe` semuanya ada. Yang
benar benar kurang cuma **Windows SDK**, jadi selisih pemasangannya jauh lebih
kecil daripada perkiraan awal.

Menambahkan workload lewat VS Installer sempat gagal tiga kali dengan pesan yang
membingungkan, dan penyebabnya berbeda beda:

| Percobaan | Kode keluar | Sebab sebenarnya |
| --- | --- | --- |
| `winget install` dengan `--override` yang menyebut versi SDK eksplisit | 1 | Id komponen SDK yang disebut tidak ada di channel ini |
| `setup.exe modify --installPath ... --quiet` | 5007 | Proses admin tapi **tidak elevated**. Log menyebutnya terang terangan: `Commands with --quiet or --passive should be run elevated from the beginning` |
| `setup.exe modify --installPath ...` dengan elevasi | 1 | `--installPath` saja tidak cocok dengan produk terdaftar. Log berkata `An installed product matching the following parameters cannot be found` |

Yang berhasil adalah menyebut identitas produk secara eksplisit, diambil dari
`vswhere`, dan dijalankan elevated:

```
setup.exe modify
  --productId Microsoft.VisualStudio.Product.BuildTools
  --channelId VisualStudio.17.Release
  --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended
  --quiet --norestart
```

Setelah itu Windows SDK 10.0.22621.0 dan 10.0.26100.0 terpasang.

**Keadaan sekarang.** `rust-toolchain.toml` memakai `channel = "stable"`, yang
berarti target msvc, jalur baku Rust di Windows. Seluruh penyiasatan raw-dylib
dihapus, dan `.cargo/config.toml` sekarang hanya berisi alias `xtask`.

**Yang tetap dipertahankan.** Pemilihan dependensi Rust murni tetap berlaku
dan jangan dibongkar tanpa alasan tersendiri: RustCrypto untuk aes-gcm dan
argon2, `k256` bukan `secp256k1`, TLS lewat `native-tls` yang memakai SChannel
bawaan Windows. Pilihan itu memperkecil permukaan build dan tetap benar
sekarang meski MSVC sudah ada.

**Pelajaran untuk lain kali.** Sebelum menyimpulkan sebuah toolchain tidak ada,
periksa `Program Files (x86)`, bukan hanya `Program Files`, dan pakai `vswhere`
alih alih menebak jalur.

---

## 0004 redb menggantikan SQLite

**Tanggal** 5 September 2026. **Status** berlaku, ditinjau ulang di fase 5.

**Konteks.** PRD 7.2 semula menyebut SQLite lewat sqlx untuk menyimpan posisi,
kutipan, dan sinyal.

**Pilihan.** redb, penyimpanan kunci nilai Rust murni.

**Alasan awal.** SQLite butuh kompilasi C, baik lewat `libsqlite3-sys` bundled
maupun lewat pustaka sistem, dan saat keputusan ini diambil mesin ini dikira
tidak punya toolchain C sama sekali.

**Alasan itu sudah gugur.** Entri 0003 selesai, MSVC lengkap terpasang, jadi
SQLite sekarang bisa dibangun tanpa masalah. Keputusan memakai redb **tetap
dipertahankan untuk sementara**, tapi sekarang atas alasan yang berbeda dan
lebih lemah: penyimpanan fase 1 hanya menyimpan pengaturan dan penghitung
ringan, jadi belum ada yang menuntut SQL, dan menukarnya sekarang berarti
mengerjakan migrasi tanpa manfaat yang bisa diukur.

Karena alasannya melemah, peninjauan ulang di fase 5 jadi lebih penting, bukan
sekadar formalitas.

**Konsekuensi.** Tidak ada SQL. Kueri analitik untuk tab PnL di fase 5 dan
kalibrasi di fase 7 harus ditulis manual sebagai pemindaian dan agregasi di
Rust. Untuk skala satu pengguna dengan ribuan baris, itu masih murah.

**Peninjauan ulang.** Di awal fase 5, saat bentuk kueri yang sebenarnya sudah
kelihatan. Dengan MSVC terpasang, `sqlx` dengan fitur `sqlite` sekarang jadi
pilihan yang wajar dan tidak lagi perlu siasat apa pun. Kalau tab PnL dan
kalibrasi ternyata menuntut agregasi yang rumit, pindah ke sana lebih baik
daripada menulis pemindaian manual di atas redb.

---

## 0005 Token pairing lewat subprotocol WebSocket

**Tanggal** 5 September 2026. **Status** berlaku.

**Konteks.** Endpoint HTTP bisa memakai header `Authorization`, tapi browser
tidak mengizinkan memasang header pada koneksi WebSocket. Pilihan yang tersisa
adalah query string atau subprotocol.

**Pilihan.** Token dititipkan di `Sec-WebSocket-Protocol`, yang bisa diisi lewat
argumen kedua `new WebSocket(url, protocols)`. Server menjawab hanya dengan
`rangefinder`, tidak pernah menggemakan token.

**Alasan.** Rahasia tidak boleh masuk URL. URL tercatat di riwayat, di log
server, dan di alat pengembang, dan itu berlaku juga untuk server lokal.

**Konsekuensi.** Sisi klien harus memakai bentuk `new WebSocket(url,
["rangefinder", token])`. Ini tidak lazim, jadi wajib dijelaskan di kode panel.

---

## 0006 Profil rilis dioptimalkan untuk latensi

**Tanggal** 5 September 2026. **Status** berlaku.

**Konteks.** Robinhood Chain memakai sequencer first come first served tanpa
lelang priority fee, jadi yang menang adalah yang paling cepat sampai.

**Pilihan.** `opt-level = 3`, `lto = "thin"`, `codegen-units = 1`,
`panic = "abort"`.

**Alasan.** Target dari klik user sampai byte terkirim ada di bawah 20 ms, dan
itu ditulis sebagai KPI di PRD. Ukuran biner tidak penting untuk aplikasi lokal.

**Konsekuensi.** Build rilis lebih lambat. `panic = "abort"` berarti panic tidak
bisa ditangkap, jadi jalur yang boleh gagal wajib memakai `Result`, bukan
mengandalkan `catch_unwind`.

---

## 0007 Pemindaian kandidat dibaca dari rantai, bukan dari API gmgn

**Tanggal** 8 September 2026. **Status** berlaku.

**Konteks.** PRD 3.4 semula membayangkan tab Signals diisi dengan memindai
daftar peringkat gmgn lewat content script, dengan batas satu permintaan per dua
detik supaya sesi user tidak kena rate limit.

**Pilihan.** Sumbernya adalah event `Swap` pada PoolManager V4, dibaca langsung
dari RPC.

**Alasan.** Yang dicari alat ini adalah pool yang membayar fee, dan yang membayar
fee adalah swap yang benar benar terjadi di pool itu. Peringkat gmgn mengukur
perhatian pasar, yang berkorelasi tapi bukan hal yang sama. Endpoint gmgn juga
privat, membawa parameter fingerprint, dan bisa berubah kapan saja tanpa
pemberitahuan.

**Konsekuensi.** Batas laju gmgn menjadi tidak relevan, karena sesi user tidak
disentuh sama sekali. Sebagai gantinya, batas log RPC menjadi kendala baru, dan
jendela pemindaian harus bisa menyempit sendiri. Data yang hanya ada di gmgn,
seperti flag risiko holder dan status kreator pada 5.4 kelompok pertama, tetap
harus datang dari sana kalau nanti dibutuhkan.

---

## 0008 Angka hasil dinyatakan terhadap modal acuan yang disebutkan

**Tanggal** 8 September 2026. **Status** berlaku.

**Konteks.** PRD 11.5 memakai `volume x fee / likuiditas` untuk memeringkat pool.
Rumus itu benar sebagai alat perbandingan antar pool pada token yang sama, tapi
begitu ditampilkan sebagai persentase ia meledak pada pool tipis. Pemindaian
sungguhan menghasilkan 1.890.130 persen dan 7.645.797 persen.

**Pilihan.** Bagian LP dihitung sebagai `modal / (kedalaman + modal)` dengan
modal acuan tetap yang ikut ditampilkan, dan hasil per jendela disajikan
berdampingan dengan proyeksi setahunnya.

**Alasan.** Bentuk `modal / kedalaman` diam diam mengandaikan modal kita jauh
lebih kecil daripada poolnya. Pada pool tipis andaian itu terbalik: modal kita
menjadi hampir seluruh pool, dan bagian kita tidak pernah melewati satu. Angka
yang tidak terbatas bukan sekadar tidak berguna, ia terlihat seperti temuan.

**Konsekuensi.** Setiap angka hasil hanya berarti bersama modal acuannya, jadi
modal itu wajib ikut di pesan protokol dan wajib tampil di panel. Kalau nanti
modal acuan diambil dari ukuran posisi user, peringkat daftarnya bisa berubah,
dan itu memang benar: pool tipis memang lebih menarik bagi modal kecil.
