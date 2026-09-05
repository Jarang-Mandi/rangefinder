# 001 Toolchain Rust di mesin Windows ini

Ditulis 5 September 2026 saat fase 1, diperbarui pada hari yang sama setelah
penghalangnya selesai.

## SELESAI, dan ini pelajaran terbesarnya

**MSVC sebenarnya sudah terpasang sejak awal.** Saya menyimpulkan sebaliknya
karena hanya memeriksa `C:\Program Files\Microsoft Visual Studio`, yang memang
kosong. Instalasinya ada di **`C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools`**,
lengkap dengan `cl.exe`, `link.exe`, dan `lib.exe`. Yang benar benar kurang
hanya Windows SDK.

Kesimpulan yang salah itu membuat saya membangun tiga penyiasatan yang tidak
pernah dibutuhkan, dan hampir meminta pemasangan 3 sampai 5 GB padahal
selisihnya jauh lebih kecil.

**Aturan untuk lain kali: pakai `vswhere`, jangan menebak jalur.**

```
"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe" \
  -products '*' -all -format json
```

Perintah itu langsung memberi `installationPath`, `productId`, dan `channelId`,
dan ketiganya dibutuhkan kalau mau memodifikasi instalasi.

Keadaan sekarang: `channel = "stable"` yang berarti msvc, tanpa penyiasatan apa
pun. Sisa berkas ini disimpan karena jebakannya bisa muncul lagi di mesin lain.

## Keadaan mesin saat masalah terjadi

- Rust 1.97.1 terpasang lewat rustup.
- Toolchain default `stable-x86_64-pc-windows-gnu`, tapi **host default**
  `x86_64-pc-windows-msvc`. Dua hal berbeda, dan itu penting.
- MSVC ada, tapi **tanpa Windows SDK**, sehingga penautan msvc tetap gagal.
- **Tidak ada gcc di PATH.**
- Node 22 dan git ada dan normal.
- `winget` tersedia tapi tidak ada di PATH PowerShell, harus dipanggil lewat
  `$env:LOCALAPPDATA\Microsoft\WindowsApps\winget.exe`.

## Tiga kegagalan berurutan, dan pelajarannya

### Satu, `channel = "stable"` teresolusi ke msvc

Menulis `channel = "stable"` di `rust-toolchain.toml` memakai **host default**,
bukan toolchain default. Di mesin ini itu berarti msvc, yang linkernya tidak
ada.

Pesan errornya sangat menyesatkan:

```
error: linking with `link.exe` failed: exit code: 1
= note: link: extra operand '...'
        Try 'link --help' for more information.
```

`link` yang ditemukan di PATH ternyata perintah **coreutils bawaan Git Bash**,
yang gunanya membuat hard link, bukan linker MSVC. Butuh waktu untuk menyadari
ini karena pesannya sama sekali tidak menyebut toolchain.

**Pelajaran.** Kalau melihat `link: extra operand`, periksa
`rustup show active-toolchain` sebelum menduga apa pun tentang kode.

**Solusi.** Pin eksplisit ke `stable-x86_64-pc-windows-gnu`.

### Dua, getrandom gagal karena raw-dylib

```
error: dlltool could not create import library ... bcryptprimitives.dll
```

getrandom 0.3 dan 0.4 mengambil `ProcessPrng` dari `bcryptprimitives.dll` lewat
`raw-dylib`, karena DLL itu tidak punya import library. Pada target gnu,
raw-dylib butuh `dlltool.exe`.

**Solusi.** `.cargo/config.toml` memaksa
`--cfg getrandom_backend="windows_legacy"`, yang memakai `RtlGenRandom` dari
advapi32 dengan penautan biasa.

**Pelajaran penting.** Nilainya `windows_legacy`, bukan `windows7`. Saya sempat
menulis `windows7` dan build tetap gagal dengan pesan yang persis sama. Nilai
cfg yang salah **tidak menghasilkan error**, cfg-nya hanya diabaikan diam diam.
Kalau menyiasati backend crate lewat cfg, verifikasi dulu nama nilainya di
sumber crate-nya, jangan menebak dari dokumentasi yang mungkin sudah lama.

Cara memverifikasi:

```
grep -rn "getrandom_backend" ~/.cargo/registry/src/*/getrandom-*/build.rs
ls ~/.cargo/registry/src/*/getrandom-*/src/backends/
```

### Tiga, windows-sys juga memakai raw-dylib, dan ini yang belum selesai

```
error: dlltool could not create import library ... ntdll.dll
error: could not compile `windows-sys`
```

`windows-sys` memakai raw-dylib untuk `ntdll.dll`, dan itu **tidak bisa
dimatikan lewat konfigurasi**. Karena `windows-sys` dipakai tokio, dirs,
keyring, dan banyak crate lain, mengganti dependensi tidak menyelesaikan apa
apa.

Akar masalahnya: rustup membundel `dlltool.exe` di

```
~/.rustup/toolchains/stable-x86_64-pc-windows-gnu/lib/rustlib/x86_64-pc-windows-gnu/bin/self-contained/
```

tapi **tidak membundel `as.exe`** yang dipanggil dlltool. Menjalankan dlltool
manual mengonfirmasinya:

```
dlltool.exe: CreateProcess
```

Direktori itu juga tidak ada di PATH secara bawaan, sehingga kalau belum
ditambahkan, pesannya malah `dlltool.exe: program not found`, yang mengarahkan
ke dugaan yang salah.

**Penyelesaiannya bukan menyiasati, melainkan pindah ke msvc**, yang ternyata
sudah hampir lengkap sejak awal. Lihat bagian teratas berkas ini.

### Empat, VS Installer punya tiga cara gagal yang pesannya menyesatkan

Menambahkan workload ke instalasi yang sudah ada gagal tiga kali:

| Perintah | Kode | Sebab sebenarnya |
| --- | --- | --- |
| `winget install` dengan `--override` menyebut versi SDK eksplisit | 1 | Id komponen SDK itu tidak ada di channel ini |
| `setup.exe modify --installPath ... --quiet` | 5007 | Proses admin tapi **tidak elevated**. Log menyebutnya: `Commands with --quiet or --passive should be run elevated from the beginning` |
| Sama, tapi sudah elevated | 1 | `--installPath` saja tidak cukup. Log: `An installed product matching the following parameters cannot be found` |

Yang berhasil menyebut identitas produk secara eksplisit dari `vswhere`:

```
setup.exe modify
  --productId Microsoft.VisualStudio.Product.BuildTools
  --channelId VisualStudio.17.Release
  --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended
  --quiet --norestart
```

**Pelajaran.** Kode keluar VS Installer nyaris tidak berguna. Log di
`%TEMP%\dd_installer_*.log` yang memuat sebab sebenarnya, dan `iselevated`
adalah baris pertama yang harus dicari.

## Yang sudah dicoba dan terbukti tidak menyelesaikan

| Percobaan | Hasil |
| --- | --- |
| Menambahkan direktori `self-contained` ke PATH | dlltool ditemukan, tapi gagal karena `as.exe` tidak ada |
| `getrandom_backend="windows7"` | nilai cfg tidak dikenal, diabaikan diam diam, gagal sama |
| `getrandom_backend="windows_legacy"` | getrandom lolos, lalu tersandung `windows-sys` |
| Memilih dependensi Rust murni | tetap berguna dan dipertahankan, tapi tidak menyelesaikan raw-dylib |
| Memeriksa `C:\Program Files\Microsoft Visual Studio` saja | menyesatkan. Instalasinya ada di `Program Files (x86)` |

## Yang tetap berlaku dan jangan dibongkar

Semua dependensi sudah sengaja dipilih Rust murni: RustCrypto untuk aes-gcm dan
argon2, `k256` bukan `secp256k1`, `redb` bukan SQLite, TLS lewat `native-tls`
yang memakai SChannel bawaan Windows bukan `rustls` dengan `ring`. Pilihan itu
tetap benar dan menghemat masalah lain, jadi jangan diganti begitu MSVC
terpasang tanpa alasan tersendiri.
