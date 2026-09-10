# Pelaksanaan perbaikan dan pekerjaan tertinggal

Mulai 9 September 2026 dari commit 5e57bfe. Permintaan user mencakup seluruh
temuan analisis serta pekerjaan yang belum selesai pada CLAUDE.md. Kotak hanya
ditandai setelah implementasi beserta pengujian yang sesuai selesai.

## Temuan analisis

- [x] 1. Parameter biaya transaksi sebelum signing. Uji menandatangani request sintetis offline.
- [x] 2. Budget, aset sumber, rasio dua sisi, dan pilihan zap sampai ke engine.
- [x] 3. Kebutuhan approval V3 tersedia sebelum simulasi mint. Uji RPC mengembalikan revert allowance pertama.
- [x] 4. Approval terikat ke spender, jumlah, dan ID langkah yang sebenarnya.
- [x] 5. Dry run pada seluruh jalur tulis. Penolakan di send_calldata sebelum RPC, nonce, atau signing.
- [x] 6. ID/revisi rencana dan pembatalan saat input berubah.
- [x] 7. Isolasi tab, chain, token, dan navigasi GMGN.
- [x] 8. Snapshot pool segar dan kalibrasi harga konsisten.
- [x] 9. Identitas posisi mencakup chain, manager, dan token ID.
- [x] 10. G1, health, golden, serta kebijakan hook mengikat eksekusi.
- [x] 11. Inventori pita, cakupan bitmap, dan jendela APR benar.
- [x] 12. PnL bermata uang, biaya dan fee aktual, histori posisi tertutup.
- [x] 13. Pemantauan posisi terus berjalan dan cooldown per posisi.
- [x] 14. Full range pada kedua orientasi. Uji tick -100000, 0, 100000 untuk token0 dan token1.
- [x] 15. Saldo native dan cadangan gas berdasarkan kebutuhan aktual.
- [x] 16. Hasil async lama tidak mengganti state baru.
- [x] 17. Penghitung limits opsional memakai keadaan nyata.

## Pekerjaan tertinggal CLAUDE.md

- [x] Self improvement: rekam fitur/hasil, kalibrasi per rezim, penantang 20
  posisi, batas parameter, rollback, filter PnL, dan replay (PRD 12 dan 14).
- [x] Tambah likuiditas serta rebalance atomik V3/V4, termasuk posisi eksternal.
- [x] Uji transfer tax jalur LP dan simulasi jual kecil (PRD 5.4).
- [x] Pengukuran TVL serta LP unik untuk adopsi hook.
- [x] Handle range pada histogram dan zoom. Drag, keyboard, reset, batas zoom
  8x, serta lebar 320/360/480 diuji pada browser Chrome dengan fixture komponen
  produksi. Uji keyboard menemukan dan memperbaiki atribut SVG tabindex.
- [x] Switch Zap in mengendalikan perilaku engine.

## Bukti penyelesaian

Per 10 September, 511 unit test engine lulus, satu uji RPC nyata diabaikan,
dan tiga test xtask lulus. Seluruh 39 test ekstensi, typecheck, serta build
ekstensi lulus. Clippy seluruh workspace dan seluruh target lulus dengan
warnings sebagai error. Protokol engine dan ekstensi sama sama versi 3.

Cakupan uji integrasi meliputi anggaran setoran desimal, langkah approval
dengan spender, plan ID/revisi, saldo ETH asli, jendela lengkap TVL/histogram,
umur volume per pool, isolasi chart/tab, penolakan hasil lookup lama, serta
golden vector di runtime.

Histogram selesai dengan uji interaksi browser. PnL menyimpan gas receipt,
jumlah mint V3, klaim, dan close aktual. Ledger menjumlahkan approval, zap,
open, tambah likuiditas, rebalance, klaim, dan close setelah finalitas 20 blok.
Histori posisi tertutup tetap ada setelah NFT dibakar dan menandai komponen
yang belum diketahui sebagai null. Rekonstruksi V3 eksternal membaca IncreaseLiquidity,
DecreaseLiquidity, serta Collect dan mempertahankan lot basis saat likuiditas
berkurang. V4 membaca batas jumlah dari calldata PositionManager bertipe dan
menandainya perkiraan karena ModifyLiquidity tidak membawa jumlah aktual.

Scan ETH native telah diuji dari respons metadata batch sampai harga dan
volume pada baris hasil. Deteksi dan pengayaan memakai definisi kutipan sama,
dan alamat nol tidak lagi dipanggil sebagai ERC20.

Kebijakan hook kini mengikat quote dan seluruh jalur memasukkan modal yang
sudah ada (approval token, zap, mint). Uji RPC tiruan mencakup hash berubah,
slot proxy terisi, kode kosong, kegagalan RPC, serta pencabutan. Generasi per
hook mencegah penilaian terlambat menghidupkan lagi persetujuan yang dicabut.
Klaim dan penutupan tidak tertahan oleh kebijakan modal masuk. TVL hook
dijumlahkan dari distribusi pool dan dikonversi lewat pasangan WETH/USDG
terkuat. LP unik berasal dari `ownerOf` token ID aktif. Riwayat parsial menjadi
batas bawah; allowlist otomatis hanya muncul setelah lima kriteria terbukti dan
kedaluwarsa setelah 24 jam. Pola upgrade nonstandar di luar empat slot proxy
yang diperiksa tetap menjadi batas pembuktian, bukan nilai aman.

Bridge chart memakai generasi attach dan kepemilikan shape. Uji menjalankan
bundle produksi di lingkungan DOM tiruan: callback dataReady lama tidak
mengganti API token baru; shape lama tidak menghalangi antrean token baru dan
dibersihkan hanya dari chart pemiliknya. Pesan draw_lines yang sudah berbeda
token ditolak.

Bundle akhir dimuat sebagai ekstensi unpacked pada Chromium. Dua tab GMGN nyata
masing masing mencapai capability tier 1 dengan token dan simbol sendiri.
Navigasi SPA mereproduksi jeda saat URL baru masih memakai API token lama;
bridge kini menunggu simbol yang cocok sebelum mengirim harga. Pada chart
berisi candle, dua shape Rangefinder dibuat lalu dibersihkan kembali tanpa
menghapus shape lain.

Self improvement menyimpan fitur dan hasil per posisi, mengelompokkan kalibrasi
per rezim, serta menahan penantang sampai 20 hasil bayangan. Delapan unit test
mencakup ambang 30 hasil, promosi, batas parameter, rollback, sinkronisasi hasil,
dan replay tanpa mutasi. CLI kalibrasi juga diuji pada database terisolasi.

- [x] Test unit dan integrasi request, state, allowance, transaksi, serta akuntansi.
- [x] Typecheck dan test ekstensi, termasuk routing beberapa tab dan input setoran.
- [x] Build engine dan ekstensi, clippy, docs-check, verifikasi registry baca saja.
- [x] Probe HTTP/WS dengan engine baru dan data pengujian terisolasi.
- [x] Dokumen, kontrak pesan, manual teknik, dan memori sesuai implementasi akhir.

Mode transaksi nyata tidak dinyalakan otomatis. Pengujian yang memerlukan
signer memakai kunci sintetis dan jaringan lokal/tiruan, tidak wallet pengguna.
