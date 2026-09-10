# Rangefinder, manual teknik

Ekstensi Chrome side panel plus engine Rust lokal untuk mengelola concentrated
liquidity Uniswap V3 dan V4 di Robinhood Chain, dengan garis range yang bisa
digeser diinjeksi ke chart TradingView milik gmgn.ai.

> Dokumen ini untuk agent dan sesi berikutnya, bukan untuk pengguna akhir.
> `docs/PRD.md` menjelaskan apa dan kenapa. Berkas ini menjelaskan bagaimana,
> beserta hal hal yang gampang dirusak kalau tidak tahu.

## Keadaan sekarang, 10 September 2026

Audit integrasi dan pekerjaan tertinggal di `docs/PERBAIKAN.md` sudah ditutup
pada kode dan pengujian lokal. Protokol sekarang versi 3. Quote membawa aset
sumber, jumlah desimal, pilihan Zap, dan request ID. Persetujuan membawa plan
ID, approval membawa step ID, dan hasil positions membawa versi kalibrasi.
Engine serta ekstensi harus dibangun dan dimuat ulang bersama. Hasil async lama
dibatasi generasi permintaan; pergantian input membatalkan rencana lama.

Identitas posisi berbentuk chain:manager:token_id. Pemantauan engine berjalan
tiap 30 detik, termasuk ketika panel tertutup. Jurnal `tx:v1` ditulis setelah
signing dan sebelum broadcast, lalu receipt direkonsiliasi tanpa signer sampai
20 konfirmasi. Koneksi putus mempertahankan hash dan nonce. Klaim dan close
memakai event manager atau delta saldo aktual. Histori posisi tertutup tetap
ada setelah NFT dibakar. Posisi V3 eksternal merekonstruksi lot setoran, collect,
dan decrease dari log. V4 eksternal memakai batas jumlah dari calldata bertipe
dan tetap ditandai perkiraan karena eventnya tidak membawa jumlah aktual. Nilai
gas atau slippage yang tidak dapat dikonversi tidak diisi nol.

Tambah likuiditas dan rebalance tersedia untuk V3 serta V4 melalui alur
rencana, approval, simulasi, klik eksplisit, dan verifikasi. Rebalance V3
menjalankan decrease, collect, burn, lalu mint dalam satu multicall. V4
menjalankan burn, take, mint, lalu settle dalam satu `modifyLiquidities`.

Gate risiko R1 membuktikan transfer penuh pada calldata LP yang persis akan
dikirim. R2 melakukan `eth_call` jual kecil lewat SwapRouter02 V3 atau
UniversalRouter V4 dengan kutipan dan allowance nyata. Zap juga disimulasikan
dengan calldata, value, router, dan alamat pengirim yang persis sama sebelum
signing. Pengukuran hook menghitung pool, umur, TVL USD, serta pemilik LP aktif
unik. Lima kriteria yang lengkap membuat allowlist otomatis berumur 24 jam;
kode dan empat slot proxy tetap dibaca ulang sebelum modal masuk.

`learning.rs` mencatat vektor fitur pada open terverifikasi dan hasil PnL pada
close. Monitor mengakumulasi waktu di dalam range. Kalibrasi dipisahkan per
rezim, butuh 30 hasil lengkap, dan menjadi penantang selama 20 hasil baru.
Bobot, ambang sinyal, faktor APR, dan faktor waktu punya batas keras. Tab PnL
memfilter hasil berdasarkan versi. `cargo xtask calibration` menyediakan
status, kalibrasi manual, replay hasil, replay candle historis, dan rollback.

Bridge chart mengikat attach dan createShape ke generasi navigasi. Hasil shape
lama dibersihkan pada chart pemiliknya, antrean halaman baru tidak menunggu
shape iframe lama, dan dataReady dibatasi 15 detik. Perintah gambar membawa
token dan divalidasi lagi terhadap URL tujuan oleh relay serta bridge.
Pembaruan range dari panel memindahkan kedua shape yang sudah ada lewat
`setPoints`; membuat pasangan baru hanya jalur pemulihan kalau shape hilang.
Event dari perpindahan terprogram dibedakan dari drag user supaya tidak
memantul kembali ke engine.

Kartu eksekusi mengikuti hierarki MMT Finance: header ringkas, sakelar Zap,
satu input aset saat Zap aktif, dua input saat Zap mati, ticker dalam token pill,
nilai fiat, saldo, lalu satu tombol utama. Header tidak memakai ikon range.
Logo token tidak dipalsukan dari huruf ticker dan tidak diambil dari jaringan
selama protokol belum membawa aset terverifikasi. Fixture
`fixtures/execution.html` menguji alur input serta lebar 320, 360, dan 480
piksel. Rancangan pengganti dry run bernama `Simulasi | Tx Asli` sudah ada di
PRD 9.2.1, tetapi belum diimplementasikan.

Setelah fase 6 ditutup, seluruh pekerjaan berikutnya adalah perbaikan yang
datang dari user memakai panelnya sendiri. Itu tercatat di PRD 6.15, dan
bagian itu wajib dibaca sebelum menyentuh panel: banyak keputusan di sana
berlawanan dengan yang tampak masuk akal dari membaca kode saja.

### Yang sudah bisa dipakai

Memuat token dari halaman gmgn, memilih pool beserta APR, TVL, dan volume,
menggeser garis range di chart, melihat distribusi likuiditas pool, menyusun
rencana beserta seluruh gate, memberi izin token, menjalankan zap, membuka
posisi, melihat posisi dan PnL, mengklaim fee, menutup posisi, memindai pool
paling ramai di rantai, dan menilai hook terhadap kriteria adopsi.

**Belum pernah ada satu pun transaksi sungguhan.** `execution.dry_run` menyala
secara bawaan, dan mematikannya adalah keputusan sadar user. Jangan pernah
mematikannya atas inisiatif sendiri.

### Batas pembuktian saat ini

Transaksi dengan dana nyata belum pernah dijalankan. `execution.dry_run` tetap
menyala sampai user sendiri mematikannya. Deteksi upgrade hook membaca empat
slot proxy standar dan tidak dapat membuktikan pola proxy nonstandar yang tidak
menaruh alamat implementasi atau admin di slot tersebut. Data holder serta umur
token belum tersedia pada semua sumber; vektor self-improvement menyimpannya
sebagai `null`, bukan nol, sampai ada bukti.

Histogram sekarang memiliki handle batas bawah/atas, keyboard satu tick spacing
(Shift sepuluh spacing), serta zoom 1x sampai 8x dan reset. `Histogram.tsx`
memakai geometri harga logaritmis di `shared/histogram.ts`. Setelah drag,
posisi handle dipertahankan sampai batas hasil engine tiba agar tidak kembali
sesaat ke props lama. Pembulatan batas tetap dilakukan engine. Fixture
terisolasi dapat dijalankan dengan
`node extension/scripts/preview-histogram.mjs`; interaksi browser diperiksa
oleh `extension/fixtures/check-histogram.cjs` melalui Playwright CLI.

### Lima hal yang berulang kali menyesatkan

Semuanya sudah pernah memakan waktu satu sesi penuh. Baca sebelum menduga ada
bug baru.

1. **Engine harus dijalankan ulang setelah kode engine berubah.** Kalau
   `cargo run` menjawab `Access is denied` pada `rangefinder.exe`, prosesnya
   masih hidup: `Get-Process rangefinder | Stop-Process -Force` dulu. Panel
   yang berbicara dengan engine lama menampilkan kolom kosong tanpa satu pun
   pesan kesalahan, dan itu terlihat persis seperti bug perhitungan.

2. **RPC harus yang mengizinkan `eth_getLogs` rentang lebar.** Alchemy paket
   gratis membatasi sepuluh blok, sedangkan penemuan pool menuntut jutaan.
   RPC publik `https://rpc.mainnet.chain.robinhood.com` harus di urutan
   pertama pada `[rpc] http` di `%LOCALAPPDATA%\Rangefinder\config.toml`.
   `ChainClient` hanya berpindah endpoint saat pemeriksaan kesehatan gagal,
   bukan saat satu permintaan gagal, jadi menaruhnya di urutan kedua tidak
   menolong.

3. **Kalau user bilang sebuah angka tidak muncul, buktikan di kabel dulu.**
   Membaca kode tidak cukup, karena kegagalannya hampir selalu di jalur
   pengiriman, bukan di rumusnya. Caranya ada di bagian berikutnya.

4. **Fee dinamis tidak punya APR, dan itu benar.** Besarannya baru ditentukan
   hook saat swap terjadi. Kolom APR yang kosong pada pool berfee dinamis
   bukan bug.

5. **Nol berbeda dari belum terukur, di mana pun.** Seluruh struct di proyek
   ini memakai `Option` untuk membedakannya, dan panel menulis garis untuk
   yang kosong. Mengubahnya menjadi nol akan membuat pool yang ramai terbaca
   sebagai pool yang sepi.

### Cara membuktikan angka sampai ke panel

```
# 1. Hentikan engine lama, bangun ulang, jalankan.
Get-Process rangefinder -ErrorAction SilentlyContinue | Stop-Process -Force
cargo run -p rangefinder-engine

# 2. Di terminal lain, sambung seperti panel dan lihat isinya.
node scripts/probe-ws.mjs <alamat_token>
```

Skrip itu membaca `pairing.token` dari direktori data, menyambung ke
`ws://127.0.0.1:8787/ws` dengan subprotocol `["rangefinder", <token>]`,
mengirim `subscribe_token`, lalu mencetak setiap `token_state` beserta apakah
`stats` terisi. Node 22 sudah punya `WebSocket` bawaan, tidak perlu dependensi.
Skripnya ada di repo, jadi tidak perlu ditulis ulang.

## Baca ini dulu

- **Bahasa dokumen dan komentar: Indonesia. Istilah teknis tetap Inggris.**
  Ikuti gaya yang sudah ada, jangan campur aduk.
- **Tidak ada em dash di dokumen mana pun.** Pakai koma, titik dua, kurung,
  atau pecah kalimat.
- **Aturan sinkronisasi berlaku mutlak.** Setiap perubahan sistem wajib
  diikuti pembaruan dokumen terkait dalam commit yang sama, dan setiap
  pelajaran baru masuk ke `memory/` pada sesi yang sama. Lihat PRD 13.1.
  Penegaknya `cargo xtask docs-check`.
- **Empat keputusan sudah dikunci user dan tidak dibuka ulang**: hot wallet
  lokal satu wallet, Uniswap V3 dan V4 sekaligus dengan gate validasi berlapis,
  otomasi semi otomatis dengan klik user, dan Robinhood Chain saja.
- **Pembatas nominal bawaannya tidak terbatas.** User sudah diberi tahu
  risikonya dan tetap memilih itu. Jangan menawarkan ulang.

## Prasyarat membangun

Rust stable dengan target msvc, dan Node 22 untuk ekstensi. Tidak ada prasyarat
lain. `cargo build`, `cargo test`, dan `cargo xtask verify` jalan apa adanya.

**Rust tidak dipasang di lokasi bawaan.** `RUSTUP_HOME` menunjuk
`D:\Rust\rustup` dan `CARGO_HOME` menunjuk `D:\Rust\cargo`, karena disk C hanya
menyisakan sekitar 8 GB. Sebagai lapis kedua, `%USERPROFILE%\.cargo` dan
`.rustup` adalah junction ke D, sehingga proses yang tidak melihat variabel itu
pun tetap menulis ke D.

Lapis kedua itu perlu karena variabel lingkungan hanya diwarisi proses yang
lahir sesudahnya. VS Code yang sudah terbuka sempat mengunduh ulang 1,5 GB
toolchain ke C tanpa memberi tahu apa pun. Rinciannya di
`memory/004-rust-di-disk-d.md`, dan cara memulihkannya di `docs/RUNBOOK.md`
bagian Lokasi instalasi Rust.

Mesin ini sempat kelihatan tidak punya toolchain C, dan repo sempat berisi
penyiasatan raw-dylib untuk target GNU. Semua itu sudah dihapus setelah
ternyata MSVC memang sudah terpasang, hanya di `Program Files (x86)`, dan yang
kurang cuma Windows SDK. Riwayat lengkapnya beserta pesan error yang menyesatkan
ada di `docs/DECISIONS.md` entri 0003 dan `memory/001-toolchain-windows.md`.
Baca itu sebelum menduga apa pun kalau build tiba tiba gagal menaut.

Dependensi tetap sengaja dipilih Rust murni: RustCrypto untuk aes-gcm dan
argon2, `k256` bukan `secp256k1`, TLS lewat SChannel bawaan Windows. Jangan
menggantinya tanpa alasan tersendiri, meski MSVC sekarang ada.

## Peta repo

```
chain.toml              SUMBER TUNGGAL alamat kontrak. Jangan tulis alamat di Rust
.cargo/config.toml      alias `cargo xtask`
rust-toolchain.toml     pin toolchain stable, target msvc

engine/                 biner `rangefinder`, plus pustaka `rangefinder_engine`
  src/lib.rs            ekspos modul supaya xtask bisa pakai ulang tipe yang sama
  src/main.rs           urutan start, loop kesehatan, loop kunci otomatis
  src/config.rs         config.toml, lokasi berkas, invarian loopback
  src/auth.rs           token pairing, perbandingan waktu konstan
  src/error.rs          EngineError dan pemetaannya ke HTTP
  src/telemetry.rs      tracing
  src/math/             konversi mcap ke tick, snap grid, matematika likuiditas
  src/pools/            penemuan pool V3 dan V4, penyaringan, pemeringkatan
    abi.rs              ikatan ABI baca saja. Tidak ada alamat di sini
  src/range/            kalibrasi kurs dari chart, tiga satuan, snap tick
  src/learning.rs       fitur/hasil, kalibrasi per rezim, penantang, replay
  src/math/tick_math.rs        TickMath bilangan bulat, nilai untuk rantai
  src/math/liquidity_math.rs   LiquidityAmounts bilangan bulat
  src/exec/             jalur tulis. Satu satunya modul yang memindahkan dana
    abi_write.rs        ABI sisi tulis. Terpisah tegas dari ABI baca
    calldata.rs         gate G2, pembangun calldata bertipe
    golden.rs           gate G3, perbandingan terhadap transaksi sungguhan
    simulate.rs         gate G4 dan G5, eth_call plus asersi invarian
    limits.rs           gate G6, pembatas nilai dan slippage
    verify.rs           gate G7, baca ulang posisi lalu bandingkan
    pipeline.rs         perakitan seluruh gate jadi satu alur membuka posisi
  src/positions/        posisi V3/V4 dan rekonstruksi histori eksternal
  src/pnl/              akuntansi aktual dan histori posisi tertutup
  src/risk/             hook, allowlist bytecode, transfer tax, jual kecil
  src/store/            redb, penyimpanan lokal
  src/wallet/           manajemen wallet
    keystore.rs         Argon2id plus AES-256-GCM
  src/chain/            akses RPC, kesehatan, gate G1
    registry.rs         pembaca chain.toml
  src/api/              server axum
    protocol.rs         kontrak pesan, wajib sinkron dengan PRD 7.3
    routes.rs           handler HTTP
    ws.rs               WebSocket ke panel

xtask/                  verify, docs-check, golden, dryrun, pairs, v4probe, zap, positions, calibration
  src/golden.rs         pengambil golden vector dari transaksi sungguhan
  src/dryrun.rs         jalankan pipeline penuh terhadap pool sungguhan
extension/              ekstensi Chrome MV3
docs/                   PRD, keputusan, kontrak gmgn, alamat chain, runbook
memory/                 catatan per topik yang tumbuh seiring waktu
```

## Invarian yang gampang dirusak

Setiap modul membuka dirinya dengan komentar kepala yang menyatakan invarian
miliknya. Baca komentar itu sebelum menyentuh isinya. Yang paling sering
tersandung:

| Invarian | Di mana | Kenapa penting |
| --- | --- | --- |
| Server hanya mengikat loopback | `config.rs::validate` | Mengikat ke luar berarti membuka wallet ke jaringan |
| Token dibandingkan waktu konstan | `auth.rs::verify` | Perbandingan biasa membocorkan prefiks lewat timing |
| Nonce AES tidak pernah dipakai ulang | `wallet/keystore.rs::encrypt` | Nonce berulang merusak GCM secara total |
| Passphrase salah bukan keystore rusak | `wallet/keystore.rs::decrypt` | Pesan yang salah membuat user menghapus keystore yang sebenarnya sehat |
| Alamat kontrak hanya dari chain.toml | `chain/registry.rs` | Dua daftar alamat akan menyimpang cepat atau lambat |
| Mismatch bytecode menghentikan engine | `chain/mod.rs::registry_gate` | Kontrak yang bukan milik kita bisa berarti dana hilang |
| Token tidak pernah lewat query string | `api/ws.rs` | Karena itu token dititipkan di subprotocol WebSocket |
| Setiap pesan protokol ada di PRD 7.3 | `api/protocol.rs` | Ditegakkan `cargo xtask docs-check` |
| Orientasi token membalik arah tick | `math/mod.rs` | Kalau memecoin adalah token1, harga naik berarti tick turun, dan batas atas bawah bertukar. Terlihat masuk akal saat salah |
| Pool tanpa likuiditas harus dibuang | `pools/mod.rs::usable_pools` | Pool kosong tetap menjawab `slot0` dengan harga dari saat ia dibuat. Pada PONS/WETH selisihnya 37 kali lipat dari pool yang benar benar likuid |
| Penyaringan pool hanya di engine | `pools` dan `App.tsx` | Panel menampilkan apa adanya. Dua tempat yang menyaring akan menyimpang |
| Token lawan tidak pernah ditebak | `pools/mod.rs::discover_for_token` | FIG punya 33 pool USDG, 5 pool ETH asli, dan nol pool WETH. Pencarian yang mengandaikan WETH melaporkan token ramai itu tidak punya pool |
| Desimal kutipan ikut kelompoknya | `pools::QuoteGroup` dan `api/mod.rs` | USDG berdesimal 6. Menganggapnya 18 menggeser seluruh harga 10^12 |
| Pool fee nol atau di atas 10 persen dibuang | `pools/mod.rs::implausible_fee` | Skor likuiditas dikali fee memenangkan pool fee 80 persen yang tidak pernah ditukar siapa pun, dan pool fee 0 persen memegang likuiditas terbesar sambil membayar LP nol |
| Cache hanya menyimpan kunci pool | `pools/cache.rs` | PoolKey tidak pernah berubah, harga dan likuiditas berubah tiap blok. Menyimpan harga berarti suatu saat mengirim transaksi berdasarkan angka basi |
| `scanned_to` hanya maju kalau pemindaian benar benar selesai | `pools::pindai_dan_gabung` | Memajukannya setelah kegagalan berarti blok yang terlewat tidak pernah dipindai lagi, dan poolnya hilang selamanya |
| Desimal kutipan tidak pernah ditebak | `pools::kenali_quote` | Lawan yang tidak dikenal dibaca on chain, dan kalau gagal kelompoknya dibuang. Menebak 18 pada USDG menggeser harga sepuluh triliun kali |
| Satu permintaan berat pada satu waktu | `pools::pindai_dan_gabung` | Dua getLogs berbarengan membuat yang kedua kena 429 lalu backoff, dan totalnya lima kali lebih lambat daripada berurutan |
| Chain id diperiksa sebelum menandatangani | `exec/send.rs::send_calldata` | Tanda tangan untuk chain id yang salah bisa diputar ulang di rantai lain, dan tetap sah selamanya |
| Transaksi yang sudah ditandatangani masuk jurnal sebelum broadcast dan nonce tetap ditahan saat hasil transport tidak pasti | `exec/journal.rs`, `exec/send.rs`, `exec/nonce.rs` | Melepas nonce setelah koneksi putus dapat membuat dua transaksi sah bertabrakan. Rekonsiliasi receipt atau bukti nonce pengganti yang menentukan akhirnya |
| Receipt sukses belum berarti berhasil | `exec/send.rs` dan `exec/verify.rs` | G7 membaca ulang posisinya dari rantai. Transaksi bisa sukses sambil menghasilkan posisi yang berbeda dari niat kita |
| Rencana disimpan di engine, tidak bolak balik lewat panel | `api/mod.rs::pending_plan` | Rencana yang kembali dari panel bisa saja sudah diubah, dan bagian yang paling menarik untuk diubah justru calldata-nya |
| Rencana dikeluarkan dari slot sebelum dikirim | `api/mod.rs::kirim_open` | Rencana yang masih tersimpan setelah dikirim bisa disetujui dua kali, dan dua persetujuan berarti dua posisi |
| Izin ERC20 sebesar yang dibutuhkan, bukan tak terbatas | `exec/approve.rs` | Wallet panas ditambah batas nominal yang sengaja tidak dibatasi berarti izin tak terbatas menaruh seluruh saldo pada satu kontrak, selamanya |
| Pool V4 ditolak jalur eksekusi V3 | `exec/pipeline.rs` gate G0 | Tanpa gate ini kegagalannya muncul sebagai revert tanpa data, dan tebakan terdekatnya menyalahkan tick yang sebenarnya benar |
| RangeView memakai orientasi harga, MintParams memakai orientasi tick | `api/execute.rs::tick_menaik` | Saat memecoin token1, `lower.tick` justru yang lebih besar. Memakainya apa adanya menolak setiap posisi pada separuh pool yang ada |
| ABI V4 wajib dibuktikan ada di bytecode | `cargo xtask v4probe` | Rantai ini sudah terbukti memakai fork pada UniversalRouter. ABI dari ingatan bukan bukti, dan kekeliruannya baru terlihat saat transaksi revert |
| V4 butuh izin dua lapis lewat Permit2 | `exec/v4/permit.rs` | Lapis pertama terlihat seperti sudah cukup, dan melewatkan yang kedua membuat mint revert setelah gas terbakar |
| V4 memakai batas ATAS jumlah, bukan batas bawah | `exec/v4/pipeline.rs::batas_atas` | Itu satu satunya pengaman slippage di V4. Menyetelnya sebesar saldo berarti mengizinkan pool mengambil seluruh saldo |
| Hook ikut dibandingkan saat verifikasi V4 | `exec/v4/verify.rs` | poolId adalah hash dari kelima field PoolKey. Hook yang berbeda berarti pool yang berbeda, betapapun mirip sisanya |
| ETH asli tidak punya izin, ia ikut sebagai msg.value | `exec/v4/permit.rs` dan `calldata.rs::value` | Currency nol berarti ETH asli. Mengirim ETH ke pool yang tidak memakainya membuatnya tersangkut di kontrak |
| URL RPC disamarkan sebelum keluar dari engine | `chain/endpoint.rs` | URL penyedia berbayar memuat API key di jalurnya, dan URL itu masuk ke log serta ke panel. Penyamarannya di tempat pembuatan, bukan di tempat penampilan, karena tempat penampilan bisa bertambah |
| Angka dua token berbeda tidak pernah dibandingkan langsung | `exec/zap.rs` | Sisa 77.380 USDG berdesimal 6 lebih bernilai daripada 12.325.754 FIG berdesimal 18, tapi perbandingan mentah menyatakan sebaliknya. Bandingkan likuiditas per sisi, yang tanpa satuan |
| Kutipan quoter yang nol adalah kegagalan | `exec/v4/quote.rs` | Pool tanpa likuiditas di arah itu menjawab nol tanpa revert, dan menerimanya sebagai jawaban sah membuat pembagian modal sepenuhnya salah |
| Struct UniversalRouter punya `minHopPriceX36` sebelum `hookData` | `exec/router_rbh.rs` | Terbukti dari offset `hookData` bernilai 0x140, sepuluh kata, sedangkan struct standar sembilan kata. Memakai struct standar membuat kontrak membaca sampah |
| UniversalRouter butuh izin Permit2 sendiri | `exec/plan.rs::izin_permit2_untuk` | Izin ke PositionManager tidak berlaku untuknya, dan melewatkannya membuat swap revert setelah gas terbakar |
| Kepemilikan NFT posisi tidak pernah di-cache | `positions/mod.rs::kandidat_v4` | NFT bisa berpindah kapan saja, dan cache kepemilikan akan menampilkan posisi milik orang lain |
| Fee dibaca dari pertumbuhan, bukan `tokensOwed` | `positions/fees.rs` dan `muat.rs` | `tokensOwed` hanya diperbarui saat posisi disentuh, jadi posisi yang didiamkan melaporkan nol selamanya |
| Pengurangan pertumbuhan fee membungkus, bukan jenuh | `positions/fees.rs` | Kontrak Uniswap memakai unchecked dan nilainya memang berputar. Jenuh membuat fee terbaca nol pada posisi yang paling lama berjalan |
| Setiap angka di layar punya satuan yang bisa disebutkan | `positions/muat.rs::kutipan_pool` | PnL dalam satuan terkecil memecoin terbaca seperti dolar. Kalau satuannya tidak bisa disebutkan, angkanya tidak ditampilkan |
| Impermanent loss diukur terhadap memegang | `pnl/mod.rs` | Terhadap modal awal akan mencampur perubahan harga dengan kerugian menjadi LP |
| Notifikasi memberitakan perubahan, bukan keadaan | `api/mod.rs::periksa_perubahan_range` | Memberitakan keadaan membanjiri layar setiap kali daftar dimuat ulang |
| Data belum cukup mengembalikan kosong, bukan skor rendah | seluruh `signals` | Skor rendah berarti sudah diperiksa dan tidak menarik. Menyamakannya dengan belum bisa diperiksa menyembunyikan token yang chartnya baru termuat |
| Bar terakhir tidak ikut dalam pembandingnya sendiri | `signals/metrik.rs::volume_z` | Kalau ikut, lonjakan besar menaikkan rata ratanya sendiri sehingga terbaca lebih kecil, persis pada kejadian yang paling ingin ditangkap |
| Bar tanpa arah tidak dihitung keseimbangan tape | `signals/metrik.rs::keseimbangan_tape` | Menganggapnya beli membuat deret yang paling seimbang justru terbaca berat sebelah |
| Basis yang miring turun selalu ditolak | `signals/dump_basis.rs` | Tanpa penjaga ini, detektor terus menyarankan LP di tengah tren turun, dan itu cara tercepat kehilangan modal |
| Setiap penolakan menyebut angkanya | `signals` | Ada ujinya. Detektor yang hanya bilang tidak cocok tidak bisa dipercaya maupun diperbaiki |
| Setiap saran penempatan menjelaskan beli dan jual | `signals/penempatan.rs` | Ada ujinya. Angka tanpa alasan membuat user bergantung pada alat ini bahkan saat alatnya salah |
| Bobot skor berjumlah satu | `signals/sideways.rs` | Ada ujinya. Menambah suku baru memaksa mengurangi yang lain, alih alih menggeser skala skor diam diam |
| Pendinginan per kode DAN subjek | `notify/mod.rs` | Dua posisi yang sama sama keluar range adalah dua kabar berbeda |
| Varian serde bertag tidak boleh membungkus Vec langsung | `api/protocol.rs` | `NotifyHistory(Box<Vec<..>>)` gagal saat diserialkan dan koneksinya diam tanpa penjelasan. Pakai varian struct |
| 429 ditunggu, rentang kelebaran dipecah | `chain::logs` | Keduanya muncul sebagai kegagalan yang sama tapi obatnya berlawanan. Memecah rentang saat kena 429 menambah permintaan dan memperburuk. Satu helper bersama, karena tiga salinannya sempat berbeda perilaku |
| Jendela pengukuran selalu ikut hasilnya | `pools::volume`, `pools::kandidat` | Volume tanpa jendela tidak bisa dibandingkan, dan proyeksi setahun dari jendela dua menit mengalikan kesalahannya ratusan ribu kali |
| Proyeksi hasil memuat modal kita di penyebut | `pools::kandidat::hasil_per_jendela` | `modal / kedalaman` meledak pada pool tipis. Bagian LP sebenarnya `modal / (kedalaman + modal)`, dan tidak pernah melewati satu |
| Modal posisi menghitung kedua kaki | `pools::kandidat::kedalaman_rentang` | Satu kaki saja terbaca nol setiap kali harga berada di ujung rentangnya, dan pembagi nol membuat pool tipis terlihat paling menguntungkan |
| Kutipan dikenali dari alamat, bukan simbol | `pools::kandidat` | Simbol ditentukan penerbit token. Tiruan USDG memang ada di rantai ini, dan simbolnya sama persis |
| Arah baru disebut kalau perpindahannya berarti | `pools::kandidat::tentukan_label` | Rasio efisiensi mengukur kelurusan jalan, bukan panjangnya. Dua tick selalu lurus, dan pernah diberi label tren |
| Hook yang bisa diupgrade ditolak mutlak | `risk::hook`, `risk::persetujuan` | Kode yang diperiksa hari ini bukan kode yang akan berjalan besok. Penolakannya ditegakkan di engine, bukan hanya di panel |
| Gagal membaca slot proxy sama dengan bisa diupgrade | `risk::hook::susun` | Yang belum terbukti aman bukan berarti aman |
| Hitungan adopsi sebagian adalah batas bawah | `risk::hook::susun` | Sudah melewati ambang berarti pasti lolos. Belum melewati berarti belum tahu, bukan tidak. Menyimpulkan `tidak` akan menolak hook yang layak tanpa pernah terlihat |
| LP unik adalah pemilik NFT aktif saat ini | `risk::hook::ukur_adopsi` | Salt event dipetakan ke token ID lalu `ownerOf` dibaca ulang. NFT terbakar tidak boleh tetap dihitung sebagai LP |
| R1 dan R2 memakai jalur kontrak nyata | `risk/token.rs` | Metadata pajak pihak ketiga tidak membuktikan transfer wallet ke manager atau kemampuan menjual lewat router yang dipakai |
| Zap disimulasikan utuh sebelum signing | `api/mod.rs::kirim_zap` | Kutipan saja tidak membuktikan allowance, hook, transfer, `msg.value`, serta minimum keluaran pada calldata gabungan |
| Kalibrasi dipisah per rezim dan tidak langsung aktif | `learning.rs` | Tiga puluh hasil membentuk penantang. Dua puluh hasil berikutnya membandingkan penantang dengan juara sebelum promosi |
| Rollback kalibrasi tidak menghapus histori | `learning.rs::rollback` | Menghapus versi lama membuat hasil sebelum dan sesudah perubahan tidak dapat dibandingkan lagi |
| Batas pita dinamai menurut harga, bukan tick | `pools::distribusi` | Pada pool yang memecoinnya token1, tick lebih besar berarti harga lebih rendah. Histogram yang terbalik tetap terlihat masuk akal |
| Harga sekarang selalu tercakup satu pita | `pools::distribusi::rangkai` | Pool yang ticknya hanya di satu sisi pernah menghasilkan histogram tanpa pita aktif sama sekali |
| Pita penutup dipotong di batas tampilan | `pools::distribusi` | Satu kata bitmap bisa mencakup ratusan persen. Batang terpanjang karena lebar, bukan karena padat, menyesatkan justru di bagian yang paling dibaca |
| Konversi satuan hanya di engine | `range` dan `math` | Panel tidak menghitung apa pun. Angka di layar harus sama persis dengan yang tercatat on chain |
| Kurs ETH diturunkan dari chart | `math::calibrate_eth_usd` | Sumber luar selalu sedikit meleset dari gmgn, dan garis ikut bergeser |
| Sisi tampilan bertukar saat memecoin token1 | `range::build_view` | Tick yang lebih besar berarti harga memecoin lebih rendah. Batas atas dan bawah harus ditukar sebelum ditampilkan |
| f64 untuk tampilan, bilangan bulat untuk rantai | `math` versus `math::tick_math` | Menukar keduanya berarti mengirim harga yang meleset ke kontrak |
| Perkalian 256 bit meluap | `liquidity_math::mul_div` | Semua perkalian lewat 512 bit. Luapan di sini baru terlihat saat dana sudah bergerak |
| Golden vector tidak boleh diperbarui supaya lulus | `exec/golden.rs` | Kalau perbandingan gagal, yang salah builder-nya. Memperbarui fixture menyembunyikan kesalahan |
| Fee V4 bisa berupa penanda, bukan besaran | `pools::ranking_fee` | `0x800000` berarti dynamic fee. Dibaca apa adanya akan tertulis 838 persen |
| Dry run menyala secara bawaan | `config::ExecutionConfig` | Engine harus bisa dipasang di mesin bersih tanpa memindahkan dana pada percobaan pertama |
| Simulasi berhasil belum cukup | `exec/simulate.rs` | Transaksi bisa lolos sambil melakukan hal yang berbeda dari niat kita. G5 yang menangkapnya |
| Receipt sukses belum cukup | `exec/verify.rs` | Token yang tertukar tetap menghasilkan posisi dan transaksi yang sukses. Hanya pembacaan ulang yang membuktikannya benar |

## Alur start engine

```
config     gagal berarti berhenti, tidak ada tebak tebakan
token      dibuat sekali, ditampilkan sekali
registry   dibaca dari chain.toml
gate G1    verifikasi bytecode. Mismatch berhenti, RPC mati hanya peringatan
wallet     dibuka dalam keadaan terkunci
server     mengikat loopback
```

Alasan RPC mati tidak menghentikan engine: chain ini memang sering bermasalah,
dan panel tetap berguna untuk melihat status. Yang menghentikan hanyalah
bytecode yang tidak cocok dengan yang dipin.

## Pola yang harus ditiru

- **Error spesifik.** Setiap `EngineError` punya kode mesin yang stabil dan
  pesan yang bisa ditindaklanjuti. Tidak ada "terjadi kesalahan".
- **Rahasia dibungkus tipe.** `Secret` dan `PairingToken` punya `Debug` manual
  yang mencetak `[redacted]`. Kalau menambah tipe pembawa rahasia, lakukan hal
  yang sama, jangan andalkan kedisiplinan pemanggil.
- **Keputusan terpusat.** `evaluate_execution_readiness` adalah satu satunya
  tempat yang memutuskan boleh eksekusi atau tidak, supaya HTTP, WebSocket, dan
  panel tidak mungkin memberi jawaban berbeda.
- **Uji menempel di modulnya.** Setiap modul membawa `mod tests` sendiri, dan
  yang diuji adalah invariannya, bukan getter setter.
- **Komentar menjelaskan kenapa, bukan apa.** Kode sudah mengatakan apa.

## Status fase

| Fase | Isi | Status |
| --- | --- | --- |
| 0 | Spesifikasi, repo, dokumen | selesai |
| 1 | Engine: config, pairing, keystore, chain, api, xtask | **selesai dan terverifikasi jalan** |
| 1 | Ekstensi: manifest, side panel, jembatan chart | selesai, typecheck bersih, build menghasilkan dist yang bisa dimuat |
| 2 | Injeksi chart dan matematika satuan | **selesai**. Penemuan pool, kalibrasi kurs dari chart, konversi tiga satuan, snap tick beserta laporan drift, lingkaran penuh panel ke chart ke engine, dan uji integrasi terhadap snapshot gmgn |
| 3 | Eksekusi V3 | **selesai**. Gate G0 sampai G7 terangkai dan terbukti jalan terhadap pool sungguhan lewat `cargo xtask dryrun`: versi protokol, pembatas nilai, builder calldata, golden vector, simulasi, asersi invarian, izin ERC20, pengiriman beserta penjaga chain id dan nonce, serta verifikasi pasca kirim. Jalur panel lengkap: `quote_range`, `approve_token`, `open_position`. Yang belum pernah terjadi: satu pengiriman sungguhan, karena `dry_run` menyala secara bawaan dan mematikannya adalah keputusan user |
| 4 | Eksekusi V4 dan zap | **selesai**. ABI V4 dibuktikan ada di bytecode terpasang lewat `cargo xtask v4probe`, encoder aksi menghasilkan byte identik dengan transaksi V4 sungguhan sepanjang 868 byte, izin dua lapis lewat Permit2, gate G0 sampai G7 terangkai. Zap satu sisi memakai kutipan V4Quoter dan penyelesai bagi dua, dan encoder UniversalRouter fork membangun ulang transaksi swap sungguhan 1124 byte byte per byte termasuk field `minHopPriceX36`. `cargo xtask dryrun` dan `cargo xtask zap` hijau terhadap pool nyata |
| 5 | Positions, PnL, notifikasi | **selesai**. Posisi V3 lewat ERC721Enumerable dan V4 lewat pemindaian Transfer. Fee dibaca dari pertumbuhan fee. PnL memisahkan fee, IL, gas, dan slippage, serta mempertahankan `null` untuk komponen yang belum terbukti. Histori eksternal V3 direkonstruksi dari event; V4 memakai batas calldata sebagai perkiraan. Klaim, close, tambah likuiditas, dan rebalance tersedia lewat rencana dan klik eksplisit |
| 6 | Signals dan doktrin penempatan | **selesai**. Detektor sideways serta dump lalu basis, doktrin penempatan, optimizer fee tier, dan scan berlabel tersedia. Gate hook mengukur kelima kriteria on chain dan dapat membuat allowlist otomatis 24 jam. Transfer tax jalur LP dan jual kecil dibuktikan melalui simulasi kontrak nyata |
| 6b | Perbaikan dari pemakaian | **selesai**. Distribusi likuiditas V3/V4, histogram interaktif tanpa lompatan setelah drag, statistik pool, kartu setoran bergaya MMT dengan satu aset saat Zap aktif, saldo wallet, perkiraan gas, Settings, snapshot konsisten, isolasi tab, dan Zap satu sisi terhubung dari panel sampai engine |
| 7 | Self improvement | **selesai**. Vektor fitur dan hasil tersimpan per posisi, kalibrasi per rezim memakai minimum 30 hasil, penantang 20 hasil, batas keras, filter PnL per versi, replay hasil/candle, serta rollback satu perintah |

### Bukti fase 1 selesai

Diverifikasi langsung, bukan diasumsikan:

```
cargo test --workspace   511 lolos, 1 uji jaringan diabaikan, 3 uji xtask lolos
cargo clippy             bersih dengan warnings sebagai error
cargo xtask docs-check   dokumen dan kontrak pesan sinkron
npm test  (extension)    41 lolos, typecheck dan build bersih
```

Bundle ekstensi juga dimuat sebagai ekstensi unpacked pada Chromium dan diuji
di dua tab token GMGN nyata. Masing masing tab mencapai capability tier 1.
Navigasi SPA menunggu sampai `activeChart().symbol()` cocok dengan token baru,
sehingga harga token lama tidak pernah dikirim memakai alamat baru. Dua garis
dibuat pada chart berisi candle lalu dibersihkan kembali tanpa menghapus shape
lain.

Engine dijalankan sungguhan terhadap Robinhood Chain: RPC sehat, gate G1 lolos
17 dari 17, server terikat 127.0.0.1:8787. Endpoint diuji lewat HTTP: `/v1/ping`
terbuka, `/v1/status` menolak tanpa token dan dengan token salah, siklus wallet
buat, buka, kunci berjalan, passphrase salah menghasilkan `bad_passphrase`
bukan `keystore_corrupt`, dan membuat wallet dua kali menghasilkan
`wallet_exists` dengan HTTP 409.

## Apa yang dibaca berikutnya

1. `docs/PRD.md` bagian 4 kalau menyentuh injeksi chart, bagian 5.7 kalau
   menyentuh penempatan range, bagian 6.4 kalau menyentuh eksekusi.
2. `docs/DECISIONS.md` sebelum mengubah pilihan teknologi apa pun.
3. `docs/GMGN.md` sebelum menyentuh apa pun yang berhubungan dengan gmgn.
4. `memory/` sebelum mengulangi pendekatan yang mungkin sudah pernah gagal.
