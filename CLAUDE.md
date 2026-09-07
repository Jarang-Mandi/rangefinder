# Rangefinder, manual teknik

Ekstensi Chrome side panel plus engine Rust lokal untuk mengelola concentrated
liquidity Uniswap V3 dan V4 di Robinhood Chain, dengan garis range yang bisa
digeser diinjeksi ke chart TradingView milik gmgn.ai.

> Dokumen ini untuk agent dan sesi berikutnya, bukan untuk pengguna akhir.
> `docs/PRD.md` menjelaskan apa dan kenapa. Berkas ini menjelaskan bagaimana,
> beserta hal hal yang gampang dirusak kalau tidak tahu.

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
  src/store/            redb, penyimpanan lokal
  src/wallet/           manajemen wallet
    keystore.rs         Argon2id plus AES-256-GCM
  src/chain/            akses RPC, kesehatan, gate G1
    registry.rs         pembaca chain.toml
  src/api/              server axum
    protocol.rs         kontrak pesan, wajib sinkron dengan PRD 7.3
    routes.rs           handler HTTP
    ws.rs               WebSocket ke panel

xtask/                  verify, docs-check, golden, dryrun, pairs, v4probe, zap, positions
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
| Nonce dilepas kalau pengiriman gagal, TIDAK dilepas kalau hanya receiptnya belum datang | `exec/nonce.rs` | Nonce yang bocor membuat transaksi berikutnya menggantung. Melepas nonce yang sudah terpakai membuat dua transaksi bertabrakan |
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
| Pendinginan per kode DAN subjek | `notify/mod.rs` | Dua posisi yang sama sama keluar range adalah dua kabar berbeda |
| Varian serde bertag tidak boleh membungkus Vec langsung | `api/protocol.rs` | `NotifyHistory(Box<Vec<..>>)` gagal saat diserialkan dan koneksinya diam tanpa penjelasan. Pakai varian struct |
| 429 ditunggu, rentang kelebaran dipecah | `pools::scan_initialize_logs` | Keduanya muncul sebagai kegagalan yang sama tapi obatnya berlawanan. Memecah rentang saat kena 429 menambah permintaan dan memperburuk |
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
| 5 | Positions, PnL, notifikasi | **selesai**. Posisi V3 lewat ERC721Enumerable dan V4 lewat pemindaian Transfer, karena V4 tidak punya enumerasi. Fee dibaca dari pertumbuhan fee, bukan `tokensOwed`. PnL memisahkan fee, impermanent loss, gas, dan slippage, dinyatakan dalam token kutipan. Toast diinjeksi ke halaman gmgn dengan pendinginan dan riwayat, dan content script menyambung sendiri ke engine supaya kabar tetap sampai saat panel tertutup. Terbukti terhadap wallet sungguhan lewat `cargo xtask positions`. Yang belum: aksi tambah, rebalance, klaim, dan tutup dari kartu posisi |
| 6 | Signals, Optimizer, doktrin penempatan | belum |
| 7 | Self improvement | belum |

### Bukti fase 1 selesai

Diverifikasi langsung, bukan diasumsikan:

```
cargo test               68 lolos, 0 gagal
cargo clippy             bersih
cargo xtask verify       HIJAU, registry 17 dari 17 cocok, dokumen lengkap
npm test  (extension)    17 lolos, uji terhadap snapshot gmgn
```

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
