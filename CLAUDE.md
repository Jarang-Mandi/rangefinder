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
menyisakan sekitar 8 GB. Kalau `cargo` tiba tiba tidak dikenali atau rustup
menawarkan pemasangan ulang dari nol, kedua variabel itu yang hilang, bukan
Rust-nya. Cara mengembalikannya ada di `docs/RUNBOOK.md` bagian Lokasi
instalasi Rust.

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

xtask/                  verify, docs-check, golden, dryrun
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
| 3 | Eksekusi V3 | **berjalan**. Keenam gate G1 sampai G7 kecuali pengiriman sudah ada dan teruji: registry, builder calldata, golden vector terhadap transaksi sungguhan, simulasi, asersi invarian, pembatas nilai, dan verifikasi pasca kirim. Yang belum: perakitan pipeline dan pengiriman transaksinya sendiri |
| 4 | Eksekusi V4 dan zap | belum |
| 5 | Positions, PnL, notifikasi | belum |
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
