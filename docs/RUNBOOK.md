# Runbook

Cara menjalankan Rangefinder, dan apa yang dilakukan saat ada yang salah.

## Prasyarat

- Rust stable dengan target msvc. Sudah diatur `rust-toolchain.toml`, jadi
  tidak perlu disetel manual.
- MSVC Build Tools beserta Windows SDK. Sudah terpasang di mesin ini.
- Node 22 atau lebih baru untuk membangun ekstensi.
- Chrome dengan mode pengembang aktif.

## Menjalankan engine

```
cargo run -p rangefinder-engine
```

Saat pertama kali jalan, engine akan:

1. Membuat `config.toml` bawaan di direktori data.
2. Membuat token pairing dan menampilkannya sekali.
3. Membaca `chain.toml` lalu menjalankan gate G1.
4. Mengikat `127.0.0.1:8787`.

Direktori data ada di `%LOCALAPPDATA%\Rangefinder`. Bisa dipindahkan lewat
`RANGEFINDER_DATA_DIR` untuk pengujian.

Melihat token lagi tanpa menjalankan server:

```
cargo run -p rangefinder-engine -- --print-token
```

Log lebih rinci:

```
RANGEFINDER_LOG=rangefinder=debug cargo run -p rangefinder-engine
```

## Memasang ekstensi

```
cd extension
npm install
npm run build
```

Lalu di Chrome: buka `chrome://extensions`, nyalakan Developer mode, klik
Load unpacked, pilih `extension/dist`.

## Pairing

1. Jalankan engine, salin token yang tercetak.
2. Buka side panel Rangefinder.
3. Tempel token, tekan Sambungkan.

Token tersimpan di penyimpanan ekstensi dan tidak diminta lagi. Kalau token
hilang atau ingin dicabut, hapus `pairing.token` di direktori data lalu
jalankan ulang engine, dan lakukan pairing ulang.

## Membuat wallet

Wallet dibuat dari side panel, bukan dari baris perintah, supaya passphrase
tidak pernah masuk riwayat shell.

Setelah dibuat, wallet **tetap terkunci**. Itu disengaja: jalur buka kunci
diuji sejak menit pertama, bukan nanti saat sudah ada dana di dalamnya.

Wallet mengunci diri setelah 30 menit tanpa aktivitas. Ubah lewat
`wallet.auto_lock_minutes` di `config.toml`.

**Peringatan.** Pembatas nominal bawaannya tidak terbatas, atas keputusan yang
sudah diambil sadar. Artinya pengaman satu satunya adalah isi wallet itu
sendiri. Pakai wallet terpisah khusus operasi ini, isi hanya sebanyak yang siap
dipertaruhkan.

## Mode dry run

`execution.dry_run` di `config.toml` menyala secara bawaan. Dalam mode ini
engine menyusun transaksi, menjalankan seluruh gate, dan menyimulasikannya,
tapi tidak pernah mengirim apa pun ke rantai.

Matikan hanya setelah kamu memeriksa hasil simulasinya dan siap memindahkan
dana:

```toml
[execution]
dry_run = false
```

## Verifikasi

```
cargo xtask verify        periksa registry terhadap chain, lalu dokumen
cargo xtask verify --pin  sekaligus mengunci code_hash yang belum dipin
cargo xtask docs-check    hanya periksa kesesuaian kode dengan dokumen
cargo xtask golden [n]    ambil n golden vector dari transaksi mint sungguhan
cargo xtask dryrun <token> [pengirim]   jalankan seluruh pipeline tanpa mengirim
cargo test                seluruh uji unit engine
cargo clippy --all-targets
```

Sisi ekstensi:

```
cd extension
npm test                  uji jembatan chart terhadap snapshot gmgn
npm run typecheck
npm run build
```

`npm test` memakai test runner bawaan Node, tanpa dependensi tambahan. Yang
dijaga adalah hal hal yang paling mungkin rusak diam diam saat gmgn
memperbarui bundle: bentuk URL token, pola id iframe chart, jalur akses lilin
terakhir, dan penentuan tingkat degradasi. Snapshotnya ada di
`extension/fixtures/gmgn-token-page.html`, beserta cara memperbaruinya.

Jalankan `cargo xtask verify` sebelum setiap commit yang menyentuh
`chain.toml`, `engine/src/api/protocol.rs`, atau dokumen di `docs/`.

## Kalau ada yang salah

### Engine menolak start dengan "gate G1 gagal"

Bytecode di salah satu alamat tidak cocok dengan yang dipin. Ini disengaja
menghentikan engine. Jalankan `cargo xtask verify` untuk melihat kontrak mana.
Lalu putuskan: apakah kontraknya memang diperbarui secara sah, atau alamatnya
salah. Jangan pernah menghapus `code_hash` hanya supaya engine mau jalan.

### Engine jalan tapi eksekusi diblokir

Lihat `blocked_reasons` pada status di panel. Penyebab yang mungkin: RPC tidak
sehat, lag blok melebihi ambang, wallet belum dibuat, atau wallet terkunci.

### RPC tidak sehat terus menerus

RPC publik dibatasi laju dan chain-nya memang sering bermasalah. Tambahkan
endpoint cadangan di `rpc.http` pada `config.toml`. Engine berpindah otomatis
setelah kegagalan.

### Panel tidak bisa menyambung

1. Pastikan engine jalan, cek `http://127.0.0.1:8787/v1/ping` di browser.
   Endpoint itu terbuka dan hanya mengembalikan nama serta versi.
2. Kalau `ping` menjawab tapi WebSocket tidak, biasanya token pairing salah.
   Ambil ulang dengan `--print-token`.

### Build gagal dengan "link: extra operand"

Artinya toolchain teresolusi ke msvc tapi linkernya tidak ditemukan, sehingga
`link` yang dipakai justru perintah coreutils bawaan Git Bash. Periksa MSVC
Build Tools dan Windows SDK masih terpasang:

```
"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe" -products * -all -format value -property installationPath
ls "C:\Program Files (x86)\Windows Kits\10\Lib"
```

Kalau `Lib` kosong, Windows SDK-nya hilang. Cara memasangnya ada di
`docs/DECISIONS.md` entri 0003 bagian Penyelesaian.

### Build gagal dengan "dlltool could not create import library"

Artinya toolchain sedang memakai target GNU, bukan msvc. Periksa
`rust-toolchain.toml` masih berisi `channel = "stable"`. Riwayat lengkap
kenapa target GNU bermasalah di mesin ini ada di
`memory/001-toolchain-windows.md`.

## Yang tidak boleh dilakukan

- Jangan meng-commit `pairing.token`, `wallet.keystore.json`, atau isi
  direktori data. `.gitignore` sudah menutupnya, tapi periksa lagi kalau
  menambah berkas baru.
- Jangan mengubah `server.host` ke selain loopback. Engine akan menolak jalan,
  dan itu memang tujuannya.
- Jangan menulis alamat kontrak langsung di kode Rust. Semuanya lewat
  `chain.toml`.
