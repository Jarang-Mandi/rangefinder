# Runbook

Cara menjalankan Rangefinder, dan apa yang dilakukan saat ada yang salah.

## Prasyarat

- Rust stable dengan target msvc. Sudah diatur `rust-toolchain.toml`, jadi
  tidak perlu disetel manual.
- MSVC Build Tools beserta Windows SDK. Sudah terpasang di mesin ini.
- Node 22 atau lebih baru untuk membangun ekstensi.
- Chrome dengan mode pengembang aktif.

## Lokasi instalasi Rust

Rust di mesin ini **tidak** dipasang di lokasi bawaan `%USERPROFILE%`, melainkan
di disk D, karena C hanya menyisakan sekitar 8 GB sedangkan toolchain plus cache
registry memakan hampir 3,5 GB.

| Variabel | Nilai |
| --- | --- |
| `RUSTUP_HOME` | `D:\Rust\rustup` |
| `CARGO_HOME` | `D:\Rust\cargo` |
| PATH | memuat `D:\Rust\cargo\bin`, bukan `%USERPROFILE%\.cargo\bin` |

Selain itu, `%USERPROFILE%\.cargo` dan `%USERPROFILE%\.rustup` dibuat sebagai
**junction** yang menunjuk ke D. Jadi ada dua lapis jaminan:

| Lapis | Melindungi dari |
| --- | --- |
| `CARGO_HOME` dan `RUSTUP_HOME` | pemakaian normal |
| Junction di `%USERPROFILE%` | proses yang tidak melihat variabel itu |

Lapis kedua bukan kemewahan. Variabel lingkungan hanya diwarisi proses yang
lahir sesudahnya, jadi VS Code yang sudah terbuka sebelum perubahan tetap
memakai lingkungan lama. Tanpa junction, rust-analyzer di dalamnya akan
mengunduh ulang toolchain ke C tanpa memberi tahu siapa pun. Itu benar benar
terjadi, 1,5 GB kembali ke C dalam hitungan menit.

Junction tidak memakan ruang di C. Ia hanya penunjuk, dan seluruh berkasnya
tetap di D.

Kalau variabelnya suatu saat hilang, misalnya setelah pasang ulang Windows atau
rustup, kembalikan dengan:

```powershell
[Environment]::SetEnvironmentVariable('CARGO_HOME',  'D:\Rust\cargo',  'User')
[Environment]::SetEnvironmentVariable('RUSTUP_HOME', 'D:\Rust\rustup', 'User')
$p = ([Environment]::GetEnvironmentVariable('Path','User') -split ';' |
      Where-Object { $_ -and $_ -notlike '*\.cargo\bin' })
if ($p -notcontains 'D:\Rust\cargo\bin') { $p += 'D:\Rust\cargo\bin' }
[Environment]::SetEnvironmentVariable('Path', ($p -join ';'), 'User')
```

Lalu tutup terminal dan buka yang baru. Periksa dengan `rustup show`, barisnya
harus berbunyi `rustup home: D:\Rust\rustup`.

Membuat ulang junction-nya kalau hilang:

```powershell
New-Item -ItemType Junction -Path "$env:USERPROFILE\.cargo"  -Target 'D:\Rust\cargo'
New-Item -ItemType Junction -Path "$env:USERPROFILE\.rustup" -Target 'D:\Rust\rustup'
```

**Hati hati saat menghapus junction.** Hapus dengan `Remove-Item` pada
junction-nya saja, jangan dengan alat yang menelusuri isi tautan, karena itu
bisa ikut menghapus berkas aslinya di D. Periksa dulu jenisnya:

```powershell
Get-Item "$env:USERPROFILE\.cargo" -Force | Select-Object Name, LinkType, Target
```

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

## Memakai RPC berbayar untuk menghindari 429

RPC publik `rpc.mainnet.chain.robinhood.com` membatasi laju, dan batasnya
tercapai pada pemakaian yang sepenuhnya wajar: memuat token dengan puluhan pool
sudah cukup. Engine sudah menunggu dan mengulang saat kena 429, tapi itu
menambah waktu tunggu, bukan menghilangkannya.

Alchemy mendukung Robinhood Chain mainnet, chain id 4663, jadi ia bisa dipakai
langsung tanpa mengubah kode apa pun.

### Cara memasangnya

Ambil API key di dashboard Alchemy, lalu buka
`%LOCALAPPDATA%\Rangefinder\config.toml` dan taruh endpointnya **di urutan
pertama**:

```toml
[rpc]
http = [
  "https://robinhood-mainnet.g.alchemy.com/v2/API_KEY_ANDA",
  "https://rpc.mainnet.chain.robinhood.com",
]
```

Yang pertama dipakai, sisanya cadangan. Menyisakan RPC publik di urutan kedua
disengaja: kalau kuota Alchemy habis atau layanannya sedang bermasalah, engine
berpindah sendiri alih alih berhenti. Perpindahan itu tercatat di log.

Jalankan ulang engine setelah mengubah berkas ini.

### API key tidak akan tercetak

URL Alchemy memuat API key di jalurnya, dan URL itu mengalir ke dua tempat yang
terlihat: log engine dan status yang dikirim ke panel. Keduanya mudah ikut
tersalin saat mengirim tangkapan layar.

Karena itu URL selalu disamarkan sebelum keluar dari engine:

```
https://robinhood-mainnet.g.alchemy.com/v2/[disamarkan]
```

Host tetap ditampilkan supaya masih terlihat RPC mana yang sedang dipakai.
Penyamarannya terjadi saat nilainya dibuat, bukan saat ditampilkan, karena
tempat penampilan bisa bertambah tanpa siapa pun ingat menyamarkannya.

Berkas `config.toml` sendiri tetap memuat key aslinya. Ia ada di direktori data
lokal user dan tidak pernah dikirim ke mana pun, tapi perlakukan seperti berkas
rahasia lain: jangan disalin ke repo, jangan dibagikan.

### WebSocket

Alchemy juga menyediakan WebSocket. Isi `ws` kalau mau, formatnya sama dan
penyamarannya berlaku juga:

```toml
ws = ["wss://robinhood-mainnet.g.alchemy.com/v2/API_KEY_ANDA"]
```

### Apakah 429 hilang sepenuhnya

Tidak dijamin. Paket gratis mana pun punya batas, dan yang berubah hanya di mana
batasnya berada. Karena itu penanganan 429 tetap ada di dua tempat, yaitu
pemindaian log dan multicall, dan keduanya menunggu lalu mengulang alih alih
menyerah. Lihat `memory/006-kecepatan-pencarian-pool.md`.

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

## Membuka posisi pertama

Tiga langkah, dan sengaja tidak digabung jadi satu tombol. Yang disetujui di
langkah ketiga memindahkan uang sungguhan.

### 1. Susun rencana

Muat token, atur garis range di chart gmgn, lalu isi nominal dolar di kartu
Eksekusi dan tekan **Susun rencana**. Engine menjalankan seluruh gate dan
mengembalikan hasilnya. Tidak ada apa pun yang dikirim ke rantai.

Yang perlu diperiksa di kartu itu:

| Baris | Artinya |
| --- | --- |
| G0 versi protokol | Menentukan jalur mana yang dipakai, V3 atau V4. Pool yang versinya tidak cocok dengan jalur ditolak di sini |
| G2 builder calldata | Tick sudah pas di grid pool |
| G4 simulasi | Kontrak menerima transaksinya, beserta likuiditas yang tercipta |
| G5 asersi invarian | Hasil simulasi sesuai yang diharapkan, bukan sekadar tidak revert |
| Likuiditas dan tick | Bandingkan dengan garis yang kamu gambar |

Rencana berlaku tiga menit. Sesudah itu ia ditolak dan harus disusun ulang,
karena jumlah tokennya dihitung dari harga saat rencana dibuat.

### 2. Beri izin

Kalau tombol izin muncul, artinya izinnya belum cukup. Ini transaksi
sungguhan, tapi ia hanya memberi izin, tidak memindahkan token.

Jumlah tombolnya berbeda menurut protokol, dan itu bukan kebetulan:

| Protokol | Langkah |
| --- | --- |
| V3 | Satu, yaitu allowance ERC20 ke NonfungiblePositionManager |
| V4 | Dua, yaitu token mengizinkan Permit2, lalu Permit2 mengizinkan PositionManager |

V4 menarik token lewat Permit2, jadi lapis kedua wajib ada. Melewatkannya
membuat mint revert setelah gas terbakar, dan lapis pertama terlihat seperti
sudah cukup. Tekan tombolnya berurutan sampai daftarnya kosong, satu tombol
satu transaksi. Teks pada tombol menyebutkan langkah keberapa.

Izin Permit2 berlaku tiga puluh menit. Itu disengaja: izin yang berlaku
setahun sama saja dengan izin tak terbatas dalam waktu.

Izin diberikan **sebesar yang dibutuhkan rencana itu**, bukan tak terbatas. Ini
disengaja: wallet ini panas dan batas nominal posisi tidak dibatasi, jadi izin
tak terbatas menaruh seluruh saldo pada satu kontrak selamanya. Konsekuensinya
langkah ini berulang tiap posisi.

Sesudah terkirim, engine membaca ulang allowance dari rantai untuk
membuktikannya benar benar naik. Sebagian token mengembalikan `false` alih alih
revert saat approve gagal, dan tanpa pembacaan ulang kegagalan itu baru
ketahuan saat mint ikut gagal.

### 3. Buka posisi

Tombol **Buka posisi** hanya hidup kalau seluruh gate lolos, izin sudah cukup,
dan `dry_run` sudah dimatikan. Sesudah receipt datang, gate G7 membaca ulang
posisinya dari rantai dan membandingkannya dengan yang diminta.

Tiga kemungkinan status:

| Status | Artinya | Yang harus dilakukan |
| --- | --- | --- |
| `sukses` | Masuk blok, tidak revert | Periksa baris G7. Sukses tanpa G7 lolos berarti posisinya berbeda dari yang diminta |
| `revert` | Masuk blok tapi ditolak kontrak. Gas tetap terbakar | Jangan ulangi sebelum tahu sebabnya |
| `menggantung` | Receipt belum datang dalam 60 detik | Transaksinya SUDAH ada di rantai. Periksa di explorer. **Jangan kirim ulang**, karena dua transaksi berarti dua posisi |

Engine tidak pernah mengirim ulang sendiri. Transaksi yang gagal karena alasan
yang tidak dimengerti tidak boleh diulang oleh mesin.

### Sebelum yang pertama

Jalankan dry run terhadap tokennya lebih dulu, dari terminal:

```
cargo xtask dryrun <alamat_token>
```

Perintah itu menjalankan pipeline yang sama persis terhadap saldo sungguhan,
menampilkan tiap gate, dan menyebutkan izin mana yang kurang. Kalau di sana
merah, di panel juga akan merah.

## Verifikasi

```
cargo xtask verify        periksa registry terhadap chain, lalu dokumen
cargo xtask verify --pin  sekaligus mengunci code_hash yang belum dipin
cargo xtask docs-check    hanya periksa kesesuaian kode dengan dokumen
cargo xtask golden [n]    ambil n golden vector dari transaksi mint sungguhan
cargo xtask dryrun <token> [pengirim]   jalankan seluruh pipeline tanpa mengirim
cargo xtask pairs <token>  lihat token itu berpasangan dengan apa saja di V4
cargo xtask v4probe       buktikan ABI V4 ada di bytecode yang terpasang
cargo xtask zap <token> [nominal]   jalankan penyelesai zap terhadap pool sungguhan
cargo xtask positions <wallet>     muat posisi LP sebuah wallet dari rantai
cargo xtask optimizer <token>      peringkatkan pool token itu menurut hasil per modal
cargo xtask scan                   pindai pool paling ramai dan nilai kelayakannya
cargo xtask depth <token>          gambar distribusi likuiditas pool teratasnya
node scripts/probe-ws.mjs <token>  lihat token_state persis seperti panel menerimanya
```

`cargo xtask scan` juga mencetak penilaian setiap hook yang muncul, sesuai
kriteria adopsi PRD 5.4. Sejarah rantai dipindai bertahap dan disimpan di store
yang sama dengan engine, sekitar sepuluh detik per pemanggilan, jadi jumlah pool
dan umur hook akan mengendap setelah dijalankan belasan kali. Selama belum
selesai, kedua angka itu adalah batas bawah, dan kriterianya tertulis
`belum diukur` alih alih `tidak`.

```text
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

### PowerShell bilang "cargo is not recognized"

Perubahan PATH tidak berlaku pada jendela yang sudah terbuka, jadi kalau baru
saja diatur, **tutup terminalnya lalu buka yang baru**. Untuk jendela yang
sedang jalan, tambahkan sementara:

```powershell
$env:Path += ';D:\Rust\cargo\bin'
```

Kalau memang belum pernah diatur, periksa ketiganya sekaligus:

```powershell
[Environment]::GetEnvironmentVariable('CARGO_HOME','User')   # D:\Rust\cargo
[Environment]::GetEnvironmentVariable('RUSTUP_HOME','User')  # D:\Rust\rustup
([Environment]::GetEnvironmentVariable('Path','User')) -split ';'
```

Cara memasang ulang ketiganya ada di bagian Lokasi instalasi Rust di atas.

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
