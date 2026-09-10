# Integrasi rencana, transaksi, dan akuntansi

9 September 2026. Analisis code menemukan bahwa unit test yang lulus belum
membuktikan alur lintas UI, protocol, engine, dan RPC. Pelaksanaan lengkap
dicatat di docs/PERBAIKAN.md.

Probe offline mereproduksi request Alloy tanpa fee fields, input 100 yang
menjadi setoran 200 atau 1000, full range terbalik yang keluar batas tick, dan
inventori pita yang dihitung sekitar dua kali. Fixture golden tidak diubah.

Keputusan implementasi: biaya dan dry run diperiksa di batas pengiriman
bersama; kebutuhan allowance harus dihitung sebelum simulasi mint. UI harus
membawa niat aset/jumlah dan identitas plan, bukan hanya nominal USD. Identitas
NFT harus mencakup manager karena token ID V3 dan V4 dapat sama.

Protokol v3 memakai deposit { side, amount, zap }. Jumlah
diurai sebagai integer desimal, bukan f64. Tanpa zap sisi lainnya dihitung
dari likuiditas. Dengan zap hanya aset sumber yang menjadi modal. Setelah
swap, quote berikutnya memakai sisa input dan kenaikan saldo hasil, dibatasi
kutipan, sehingga saldo tujuan yang sudah ada tidak ikut tersedot.

Plan ID mengikat persetujuan ke rencana yang ditampilkan. Revisi mematikan
rencana saat input berubah. Step ID menunjuk langkah tersimpan beserta spender
dan jumlah, termasuk UniversalRouter. Satu mutex mengurutkan pengiriman semua
aksi. Denyut tab notifikasi pasif tidak lagi memperpanjang waktu buka wallet.

G1 yang gagal tidak pulih hanya karena health menjadi hijau. Bytecode harus
diverifikasi ulang. G3 tidak boleh menyatakan test sudah berjalan hanya karena
binary berhasil dibangun: fixture disertakan dan dibandingkan pada runtime.

Posisi memakai position_id berbentuk chain:manager:token_id. Aksi dan PnL
menggunakan identitas itu, sedangkan token_id tetap ada untuk label NFT.
Catatan lama pnl:v1 tidak diimpor otomatis karena manager asalnya tidak ada.
Catatan tersebut tetap tersimpan; memilih V3 atau V4 tanpa bukti akan merusak
akuntansi. Cache V4 v2 mencakup chain dan manager, dipindai sejak deploy block,
dan daftar posisi tidak lagi dipotong diam diam pada 60 NFT.

Pemantauan posisi berjalan setiap 30 detik, tetap ketika panel tertutup.
Kegagalan pemuatan protokol tidak menghapus pembanding sebelumnya. Notifikasi
range membawa position_id sebagai subject agar dua NFT tidak saling menahan.
Ringkasan PnL dikelompokkan menurut alamat mata uang; total lintas mata uang
berbeda bernilai null. Biaya aktual, hasil klaim dan close, serta histori
posisi tertutup berasal dari jurnal transaksi yang sudah final.

Handle dan zoom histogram selesai di komponen Histogram.tsx. Logika geometri
terpisah dan diuji: total modal hasil penggabungan tetap, pita aktif terjaga,
sumbu harga serta zoom konsisten. Playwright menguji drag, keyboard, zoom/reset
dan tanpa overflow pada 320/360/480. Atribut SVG harus `tabindex` huruf kecil:
Preact mempertahankan tabIndex, sehingga slider tidak bisa menerima fokus.
Fixture ada di extension/fixtures, server pratinjau pada port lokal 5187.

SendOutcome menyimpan gas_cost_wei dari receipt, null untuk receipt menggantung
atau effectiveGasPrice hilang. Event IncreaseLiquidity V3 disaring alamat
manager dan wajib tunggal sebelum menjadi token ID/jumlah aktual. Catatan PnL
menyimpan bukti setoran/gas. Ledger menjumlahkan approval, zap, open, klaim,
close, tambah likuiditas, dan rebalance sesudah finalitas. Hasil
gas/slippage/net serta win rate tetap nullable bila sumbernya tidak terbukti.
Posisi dengan biaya tidak diketahui tidak boleh terlihat untung karena nol.

Scan sebelumnya mengenali native hanya di sisi_memecoin, tetapi susun_baris
menolaknya karena alamat nol tidak ada di registry.tokens. Definisi kutipan
sekarang bersama. token_meta_batch memberi metadata ETH tanpa RPC ke nol,
menjaga urutan respons token lain. Uji integrasi metadata tiruan sampai baris
scan membuktikan simbol ETH, harga 1, dan volume 1 ETH. saldo_dua_token kini
menggunakan helper saldo yang sama dengan UI dan menolak sisi yang gagal.

Kebijakan hook kini terhubung ke quote, approve token, zap, dan open melalui
risk::policy. Persetujuan menyimpan hash bytecode dari pembacaan nyata;
persetujuan tanpa hash tidak lagi dianggap memenuhi kebijakan. Kode dan slot
proxy diperiksa ulang sebelum plan dipakai. Approve/forget hook memakai lock
eksekusi yang sama dan menaikkan revisi. Uji menutup jalur hook asing, pencabutan,
perubahan bytecode, slot proxy terisi, RPC gagal, dan alamat tanpa kode.
Penarikan tetap tersedia. Pengukuran adopsi menghitung lima kriteria on chain:
jumlah pool, umur hook, volume 30 hari, TVL, dan LP unik aktif. TVL berasal
dari distribusi pool dan konversi USDG atau rute V3 WETH/USDG terkuat. LP unik
berasal dari owner token ID aktif. Data sejarah yang belum penuh ditandai
sebagai batas bawah. Allowlist otomatis berlaku 24 jam, lalu seluruh kriteria,
code hash, dan empat slot proxy diperiksa lagi. Pola upgrade nonstandar di luar
slot tersebut tetap menjadi batas pembuktian.
Generasi approve/forget terpisah per alamat hook. Revoke saat approve masih
menunggu RPC membatalkan hasil approve itu, termasuk bila belum ada persetujuan
tersimpan yang bisa dihapus. Mutasi persetujuan serta transaksi memakai lock
eksekusi yang sama.

ChartSession mengatasi waitDataReady yang tidak pernah kembali serta shape
yang selesai setelah navigasi. Bridge memakai generasi, membersihkan shape
dari chart pemiliknya, melepas listener lama, dan memulai antrean baru saat
halaman berganti. Perintah gambar membawa token; relay dan MAIN world
menolaknya jika URL tujuan sudah berubah. Dua tes bundle produksi berjalan
di VM dengan DOM/API tiruan, melengkapi tes helper. Token API yang digunakan
ulang GMGN memang sempat membawa data token lama sesudah URL berpindah. Bridge
kini menunggu sampai alamat pada simbol chart cocok. Bundle akhir diuji pada
dua tab GMGN nyata; keduanya tetap terisolasi dan mencapai capability tier 1.

Jurnal transaksi `tx:v1` sekarang menjadi sumber keadaan persisten setelah
signing. Hash, request lengkap, plan/flow/position ID, receipt, jumlah diterima,
dan finalitas disimpan tanpa signature atau kunci. Broadcast yang kehilangan
koneksi tidak dianggap gagal pasti dan nonce tidak dilepas. Rekonsiliasi tidak
memerlukan signer, memeriksa receipt sampai 20 konfirmasi, memulihkan reorg
sebelum finalitas, dan menandai `replaced` jika nonce sudah mined oleh hash lain.
Error transport yang dikirim ke panel disunting karena teks provider dapat
memuat URL RPC berkunci.

Fee klaim dan close V3 diambil dari event `Collect` manager yang tepat. V4 dan
fallback V3 memakai delta saldo sebelum/sesudah; biaya receipt ditambahkan
kembali pada sisi ETH asli supaya jumlah bruto tidak berkurang oleh gas.
Ledger posisi menjumlahkan klaim dan seluruh gas aksi hanya setelah final.
Sebelum close, konteks jurnal membekukan sqrtPrice, fee belum diklaim, arah
kutipan, desimal, dan mata uang. Setelah NFT hilang, tab PnL masih dapat merakit
principal, fee, IL, gas, dan net dari bukti tersebut. Posisi eksternal yang
tidak punya catatan setoran direkonstruksi dari log IncreaseLiquidity,
DecreaseLiquidity, dan Collect untuk V3. Untuk V4, batas jumlah pada calldata
PositionManager bertipe dipakai sebagai estimasi karena event ModifyLiquidity
tidak membawa jumlah token aktual. Gas dan slippage yang tidak bisa dibuktikan
tetap null.

Pesan `token_page` tidak langsung dipercaya panel. Panel membaca ulang tab
aktif melalui Chrome tabs API dan membandingkan token pada URL. Setiap query
memiliki revisi; hasil navigasi lama yang selesai belakangan dibuang. Hasil
range, depth, sinyal, quote, dan action juga diikat ke token, pool, atau request
yang sedang aktif.

Tambah likuiditas dan rebalance atomik sudah terhubung dari panel sampai
verifikasi hasil, termasuk untuk posisi eksternal. Setiap aksi memakai plan ID,
expiry, pembacaan ulang kepemilikan dan posisi, allowance yang tepat, R1/R2
ketika ada modal baru, simulasi, serta klik konfirmasi eksplisit. Snapshot pool
dan bar chart dipasangkan setelah refresh agar range, harga, dan kalibrasi tidak
berasal dari dua waktu berbeda.

V3 membungkus decrease, collect, burn, dan mint dalam satu multicall. V4
membungkus burn, take, mint, dan settle dalam satu modifyLiquidities. Zap tetap
dua transaksi karena router swap dan PositionManager terpisah, tetapi calldata
Zap yang persis sama disimulasikan dari alamat signer sebelum signing. Mint
sesudah Zap memakai delta saldo hasil, bukan seluruh saldo wallet.

Gate token R1 membuktikan jalur LP sebenarnya lewat simulasi G4/G5. R2 menjual
sekitar 0,10 USD lewat QuoterV2 dan SwapRouter02 V3 atau router V4 yang benar,
dengan allowance serta alamat pengirim sebenarnya. Bukti terikat ke chain,
pool, token, dan masa berlaku. Revert atau keluaran nol menutup semua jalur
modal masuk.

Loop self improvement tersimpan di `learning:v1`. Setiap posisi Rangefinder
yang benar benar terbuka menyimpan vektor fitur, prediksi mentah dan
terkalibrasi, rezim, serta versi kalibrasi. Monitor mengakumulasi waktu
in-range dan memasangkan catatan itu dengan hasil PnL tertutup. Umur token dan
jumlah holder dibiarkan kosong sampai ada sumber yang dapat dibuktikan.

Tiga puluh hasil lengkap dalam rezim yang sama membuat versi penantang.
Penantang berjalan sebagai bayangan untuk 20 hasil berikutnya dan hanya
dipromosikan bila menang sedikitnya 12 kali serta total error lebih rendah.
Faktor, bobot, dan ambang sinyal memiliki batas keras. Gate keamanan R1/R2 dan
ambang hook tidak ikut belajar. Semua versi disimpan; rollback hanya mengubah
pointer aktif. CLI `cargo xtask calibration` menyediakan status, train, replay,
replay-bars, dan rollback. Tab PnL memfilter ringkasan yang dihitung engine
berdasarkan versi kalibrasi.

Verifikasi 10 September 2026: 511 test engine lulus, satu uji jaringan
diabaikan, tiga test xtask lulus, 39 test ekstensi lulus, typecheck dan build
ekstensi bersih, serta clippy seluruh workspace dan target bersih dengan
warnings sebagai error. Seluruh pengujian transaksi memakai kunci sintetis,
RPC tiruan, atau data terisolasi; `dry_run` pengguna tetap menyala.
