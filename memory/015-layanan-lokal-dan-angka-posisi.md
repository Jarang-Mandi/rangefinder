# 015 Layanan lokal dan angka posisi

## Masalah yang terlihat di UI berasal dari satuan data

`amount0`, `amount1`, `fee0`, `fee1`, dan liquidity berasal dari kontrak dalam
unit terkecil. Menaruh string itu langsung di kartu Positions menghasilkan
angka seperti `283694831448035` tanpa konteks. PnL punya masalah berbeda:
nilainya sudah dalam ETH, tetapi formatter empat desimal mengubah biaya kecil
menjadi `0,0000`. Keduanya terlihat seperti data salah meski hitungan engine
benar.

Protocol v4 memperkaya setiap posisi dengan simbol, desimal, orientasi
base/quote, dan harga current/lower/upper yang sudah manusiawi. Metadata seluruh
token diambil dalam satu multicall ketika tab Positions/PnL dimuat. Panel
memformat unit mentah dengan presisi adaptif: nilai kecil tidak menjadi nol,
sedangkan angka besar dipadatkan. Raw tick dan liquidity tetap ada di bagian
Detail teknis.

## PnL bukan jurnal transaksi

Jurnal `tx:v1` tetap menjadi sumber rekonsiliasi internal engine, tetapi tidak
ditampilkan di tab PnL. Tab itu sekarang memimpin dengan pair, status, dan satu
hasil bersih dominan, lalu menampilkan fee, impermanent loss, gas, dan slippage.
Kalau satu komponen belum terbukti, kartu mengatakan PnL belum lengkap daripada
menampilkan nol atau kemenangan palsu.

Cost basis yang direkonstruksi tidak lagi otomatis mengosongkan `net`. Bila
fee, IL, gas, dan slippage semuanya memiliki angka, engine menghitung hasil
bersih dan memberi label `perkiraan`. `net` tetap kosong bila salah satu
komponennya memang belum mempunyai nilai.

## Satu input harus menghasilkan pasangan yang benar

Menyamakan dua token dengan kurs sederhana di JavaScript salah untuk
concentrated liquidity: rasio berubah menurut harga sekarang dan kedua batas
range. Fungsi `api::deposit::budget` sudah mempunyai matematika integer yang
sama dengan kontrak. Jalur tambah likuiditas sekarang memakainya saat hanya
satu sisi diisi. Panel mengirim sisi aktif setelah debounce 250 ms dan mengisi
sisi lainnya dari `ActionPlanView.amounts`. Ini bekerja dari token0 ke token1
dan sebaliknya tanpa menggandakan rumus di UI.

## Proses lokal perlu dijaga di luar extension

Reconnect WebSocket tidak bisa menghidupkan proses engine yang sudah mati.
`scripts/install-local-engine.ps1` menaruh biner release di direktori data
lokal, mendaftarkan Scheduled Task saat logon, dan menjalankan launcher
tersembunyi yang memulai ulang engine tiga detik setelah proses berhenti. Panel
mempertahankan data terakhir dan menunjukkan “menyambung ulang” ketika koneksi
yang sebelumnya sehat putus. Timeout hello mencegah socket setengah terbuka
diam selamanya.

## Wallet lokal tanpa prompt berulang

Nilai `auto_lock_minutes = 0` kini berarti auto-lock mati. Dengan
`use_credential_manager = true`, unlock yang berhasil menyimpan passphrase di
Windows Credential Manager dan startup berikutnya membukanya otomatis. Wallet
lama tetap perlu satu unlock pertama karena passphrase tidak dapat diturunkan
dari keystore terenkripsi. Kunci privat tetap tidak keluar dari modul wallet.

Logo token tidak diambil dari layanan luar. Extension harus tetap ringan,
berfungsi offline, dan CSP MV3 membatasi sumber gambar; ticker on-chain menjadi
fallback yang konsisten sesuai pilihan user.
