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

---

## 0009 Self-improvement tidak boleh melonggarkan gate keamanan

**Tanggal** 10 September 2026. **Status** berlaku.

**Konteks.** Draft PRD 12 semula memasukkan ambang gate risiko 5.4 ke parameter
yang dapat dikalibrasi dari hasil trading. Hasil untung atau rugi sebuah posisi
tidak membuktikan bahwa token berpajak, honeypot, atau hook yang dapat diupgrade
menjadi aman. Sampel yang selamat juga mengandung survivor bias paling kuat.

**Pilihan.** Loop self-improvement boleh mengubah faktor koreksi APR dan waktu,
bobot klasifikasi, serta ambang detektor sideways dalam batas keras. Ia tidak
boleh menurunkan syarat R1, R2, jumlah pool, TVL, LP unik, umur hook, atau
larangan upgrade. Perubahan gate keamanan tetap membutuhkan perubahan kode,
pengujian, dan review sadar.

**Alasan.** Kalibrasi strategi menjawab seberapa baik sebuah setup menghasilkan
fee. Gate keamanan menjawab apakah modal dapat masuk dan keluar sesuai kontrak.
Keduanya memakai bukti yang berbeda dan tidak boleh saling menggantikan.

**Konsekuensi.** Versi kalibrasi dapat meningkatkan atau menurunkan skor dan
prediksi, tetapi tidak dapat membuat token atau hook yang gagal pemeriksaan
kontrak menerima modal.

---

## 0010 Close mengubah token biasa hasil posisi menjadi ETH

**Tanggal** 12 September 2026. **Status** berlaku.

**Konteks.** Close sebelumnya hanya menarik kedua aset dari NFT ke wallet.
Pada pasangan USDG/ZZZ atau ETH/ZZZ, user masih memegang ZZZ sesudah menekan
Tutup posisi. Pada pasangan PONS/ZZZ, kedua aset spekulatif tetap tertinggal.
Hasil itu tidak sesuai arti keluar dari posisi bagi user.

**Pilihan.** Sesudah receipt close sukses dan jumlah penerimaan terbukti,
engine menjual setiap token biasa melalui pool V4 langsung ke ETH. ETH asli,
WETH, dan USDG dipertahankan. Jumlah input berasal dari delta close saja.
Approval Permit2, kutipan ulang, simulasi calldata, pengiriman, gas, dan ETH
aktual dicatat dalam jurnal posisi yang sama.

**Alasan.** Satu klik Tutup posisi harus menyelesaikan niat user tanpa membuka
rincian approval dan router di panel. Memakai delta close mencegah saldo token
lama ikut terjual. Menjalankan kedua sisi secara terpisah membuat satu rute
yang gagal tidak menahan sisi lainnya.

**Konsekuensi.** Close yang sudah sukses tidak dibatalkan oleh kegagalan swap
lanjutan. Token yang belum dapat dikonversi tetap berada di wallet dan panel
memberi peringatan. Saat tidak ada pool langsung ke ETH, engine tidak menebak
rute multi hop.

---

## 0011 RPC dibagi menurut jenis pekerjaan dan statistik awal memakai tiga jam

**Tanggal** 13 September 2026. **Status** digantikan sebagian oleh 0014.

**Konteks.** Satu endpoint tidak unggul pada semua metode. Pengukuran lokal
menunjukkan endpoint Alchemy menjawab `eth_call` jauh lebih cepat, tetapi paket
yang tersedia menolak `eth_getLogs` di atas sepuluh blok. RPC publik Robinhood
Chain menerima 216.000 blok dalam satu permintaan, tetapi pembacaan state jauh
lebih lambat. Mengukur volume 24 jam penuh sebelum menampilkan daftar pool
memerlukan lebih dari tujuh detik saat cache kosong.

**Pilihan.** `rpc.http` melayani state, saldo, rencana, simulasi, dan transaksi.
`rpc.logs_http` melayani discovery serta volume. Ketika cache statistik belum
lengkap, engine lebih dulu mengukur 108.000 blok, sekitar tiga
jam, dan panel menampilkan daftar hanya setelah seluruh pool mempunyai volume
beserta jendelanya. Pengukuran 24 jam meneruskan pekerjaan di belakang layar
dan mengganti snapshot awal.

**Alasan.** Membagi endpoint menurut kemampuan mengurangi perebutan antrean dan
tidak menggandakan permintaan yang sama ke beberapa penyedia. Snapshot tiga jam
memerlukan satu panggilan log dan tetap menyatakan jendela sebenarnya, sehingga
lebih cepat tanpa menyebut data pendek sebagai volume 24 jam.
Tinggi blok dibaca lewat RPC interaktif lalu dikurangi sepuluh blok sebelum
diberikan ke RPC log. Dengan begitu endpoint publik tidak menerima permintaan
`eth_blockNumber` tambahan dan perbedaan sinkronisasi singkat antarpenyedia
tidak membuat pemindaian melewati ujung rantai yang tersedia.

**Konsekuensi.** Instalasi satu RPC tetap bekerja karena `logs_http` bersifat
opsional. Target tujuh detik berlaku pada jalur normal dengan endpoint sehat.
Jika RPC log berhenti menjawab, engine tidak mengarang nol dan panel tetap
menunggu data yang sah. Snapshot tiga jam tidak pernah dianggap cache penuh,
jadi pengukuran 24 jam tetap dijalankan.

---

## 0012 Pembacaan RPC memakai failover per permintaan

**Tanggal** 14 September 2026. **Status** berlaku.

**Konteks.** Endpoint gratis Alchemy kadang gagal dan rotasi lima detik pada
pemeriksaan kesehatan tidak membantu permintaan yang sudah terlanjur gagal.
Uji langsung menunjukkan PublicNode dan OrdoFi menjawab `eth_call` sekitar
0,15 sampai 0,32 detik. Endpoint resmi memerlukan sekitar 1,4 detik. Namun
PublicNode menolak log lebar dengan HTTP 403, OrdoFi menjawab jaringan sibuk,
BlockReq publik hanya melayani 32.768 blok terbaru, dan hanya endpoint resmi
yang menyelesaikan rentang 108.000 blok pada uji ini.

**Pilihan.** `rpc.http` khusus kesehatan dan jalur transaksi. `rpc.read_http`
memakai PublicNode, OrdoFi, lalu endpoint resmi. `rpc.logs_http` tetap berisi
endpoint yang lolos uji log lebar. Setiap pembacaan mencoba endpoint secara
berurutan, memperlakukan respons JSON-RPC error sebagai kegagalan, membatasi
waktu tunggu per endpoint, dan mengingat endpoint terakhir yang berhasil.
Saldo dimulai bersama pengukuran statistik karena keduanya memakai jalur RPC
yang berbeda.

**Alasan.** Mengirim semua permintaan ke semua penyedia sekaligus memboroskan
kuota. Failover berurutan membayar endpoint kedua hanya ketika yang pertama
gagal. Pemisahan transaksi mencegah satu operasi kirim direplikasi oleh
transport balapan dan mempertahankan nonce serta simulasi pada jalur sehat yang
sama. Pemisahan log menjaga provider state cepat dari permintaan yang memang
tidak didukungnya.

**Konsekuensi.** Token yang sudah terindeks lokal dapat memuat state dan saldo
tanpa bergantung pada Alchemy. Token baru tetap perlu satu pemindaian log.
Kecepatan Zenith untuk token baru memerlukan indexer yang hidup terus, bukan
sekadar menambah daftar RPC. Jika endpoint publik mengubah kebijakannya, uji
`eth_call` dan rentang log harus diulang sebelum urutannya diubah.

Probe PONS `0x39dbed3a2bd333467115de45665cc57f813c4571` pada data dir
baru dengan `dry_run` menghasilkan tiga pool beserta statistik awal lengkap
dalam 4,137 detik. Probe engine terpasang yang berbagi proses dengan ekstensi
aktif menyelesaikannya dalam 6,838 detik dan mengirim saldo pada 1,998 detik.

Pada 15 September 2026, jalur `plan_action` diperpendek: quote untuk satu NFT
langsung membaca `ownerOf`, kunci pool, likuiditas, harga, dan fee NFT itu dari
RPC baca cepat. Sebelumnya ia memuat seluruh posisi V3 dan V4, termasuk
pemindaian log Transfer V4, sebelum memilih satu posisi. Validasi sebelum
pengiriman tetap memakai RPC transaksi dan gate eksekusi yang sama. API
`/lp/pool_info` Uniswap dapat membatch state pool yang sudah diketahui, tetapi
tidak menggantikan indeks pasangan token atau pengukuran volume 24 jam; untuk
token yang baru terlihat, sumber indeks yang terus berjalan tetap diperlukan.

---

## 0013 Uniswap API menjadi jalur utama state dan transaksi LP

**Tanggal** 17 September 2026. **Status** berlaku.

**Konteks.** Pembacaan langsung dari beberapa RPC cukup cepat untuk saldo dan
state kontrak tunggal, tetapi discovery, statistik, dan perakitan beberapa
langkah masih kalah responsif dari aplikasi yang memakai indexer serta layanan
transaksi Uniswap. Liquidity API resmi mendukung Robinhood Chain 4663, dapat
membaca sampai dua puluh pool yang sudah dikenal dalam satu permintaan, dan
mengembalikan transaksi siap tanda tangan untuk create, increase, decrease,
serta claim. API tetap membutuhkan RPC aplikasi untuk simulasi, tanda tangan,
broadcast, dan receipt.

**Pilihan.** Engine memakai `https://liquidity.api.uniswap.org` sebagai jalur
utama ketika API key tersedia. `/lp/pool_info` menimpa harga, tick, dan
likuiditas snapshot pool yang sudah ditemukan. `/lp/create`, `/lp/increase`,
`/lp/decrease`, dan `/lp/claim_fees` merakit transaksi LP. Setiap payload wajib
cocok dengan chain 4663, wallet aktif, PositionManager yang dipin, pasangan
token, range, dan batas nominal, lalu disimulasikan lewat RPC sebelum disimpan.
Swap zap tetap memakai jalur Robinhood yang sudah diverifikasi. Rebalance tetap
memakai builder lokal karena API tidak menyediakan close dan mint atomik.

API key disimpan di Windows Credential Manager dan tidak pernah masuk
`config.toml`, log, status, atau bundle extension. Bila key belum ada, API
melewati batas waktu, respons tidak cocok, atau simulasi gagal, engine memakai
snapshot dan builder lokal yang selesai secara paralel.

**Alasan.** Ini mengikuti arsitektur resmi Uniswap tanpa menjadikan layanan
HTTP sebagai sumber kepercayaan transaksi. Jalur resmi mengurangi pekerjaan
yang berulang, sedangkan pemeriksaan on-chain mempertahankan batas risiko dan
kemampuan berjalan saat API gagal.

**Konsekuensi.** API mempercepat state pool dan transaksi pool yang sudah
diketahui. Cadangan V4 dari `/lp/pool_info` langsung menjadi TVL dan membuat
pembacaan distribusi tick lama dilewati. Endpoint itu tidak menyediakan volume
atau APR; volume tetap berasal dari index event persisten Rangefinder, lalu APR
dihitung dari volume tersebut dan TVL Uniswap. Token baru tetap memerlukan
discovery pertama; sesudah kuncinya masuk cache, pemuatan berikutnya memakai
batch API dengan fallback on-chain.

---

## 0014 Data API v2 Uniswap menjadi jalur utama discovery dan statistik

**Tanggal** 17 September 2026. **Status** berlaku.

**Konteks.** Liquidity API mempercepat state pool yang sudah diketahui, tetapi
tidak menyediakan discovery, volume, atau APR. GraphQL gateway Uniswap dapat
menemukan pool dan TVL, tetapi uji Robinhood Chain mengembalikan volume 1D kosong
dan membutuhkan sekitar empat detik. Data API v2 yang dipakai tabel Explore
Uniswap mengembalikan ketujuh pool SWARM beserta state, TVL, volume 1D/30D, dan
APR dalam sekitar satu detik.

**Pilihan.** Lookup token lebih dulu memanggil
`data.v2.DataApiService/ListPools` dengan filter token dan chain 4663. Metadata
on-chain dibaca bersamaan. Respons indeks langsung menjadi `token_state`; saldo
dimulai segera sesudahnya. Ketika respons lengkap ini tersedia, engine tidak
menjalankan discovery atau pemindaian volume log. Snapshot tiga jam dihapus.
Fallback RPC lama tetap tersedia bila indeks gagal, tetapi langsung mengukur
24 jam dan tidak pernah menampilkan periode sementara.

**Konsekuensi.** Jalur normal membutuhkan satu request indeks dan satu batch
metadata, serta tidak bergantung pada kemampuan `eth_getLogs` RPC publik.
Angka indeks ditandai sebagai USD agar tidak dicampur dengan fallback yang
berdenominasi token kutipan. Perubahan skema atau kegagalan endpoint jatuh ke
jalur on-chain yang lebih lambat tanpa mengarang angka nol.
