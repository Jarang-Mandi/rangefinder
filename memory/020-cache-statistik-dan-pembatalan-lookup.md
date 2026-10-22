# Cache statistik dan pembatalan pekerjaan lama

## Statistik 24 jam tidak mungkin selalu menjadi pembacaan pertama yang instan

APR, TVL, dan volume bukan satu panggilan keadaan pool. Volume membutuhkan log
Swap selama 864.000 blok dan TVL membutuhkan distribusi tick. Pada
PAIREX/USDG `0xb8cf4ad...`, endpoint publik menerima satu query log besar lalu
membalas query yang terlalu rapat dengan 429. Potongan 216.000 blok yang lebih
lama juga kadang membalas `log query timed out` dan perlu dibagi dua.

Hasil pengukuran karena itu disimpan menurut token dan quote, lalu diterapkan
per ID pool. Pool yang masih sama tetap mendapat statistik ketika pool baru
muncul atau pool lama hilang; pool baru memicu penyegaran tanpa mengosongkan
yang sudah dikenal. Lima menit pertama dianggap segar. Sampai satu jam hasil
lama tetap ditampilkan sambil penyegaran berjalan. Cache yang lebih lama tidak
tampil. Harga dan likuiditas spot tidak ikut cache dan selalu dibaca ulang.
Snapshot tidak disimpan jika volume gagal, tetapi TVL satu pool yang gagal tidak
boleh membuang volume valid seluruh kelompok.

Pada probe release, pengukuran pertama tanpa cache tetap memerlukan 95,12 detik
karena batas endpoint. Setelah cache terbentuk, statistik tampil bersama hasil
discovery keadaan pool tanpa menunggu pemindaian 24 jam lagi.

## Generasi hasil saja tidak menghentikan beban RPC

`Intent` mencegah hasil subscribe lama menimpa token baru, tetapi itu tidak
membatalkan future HTTP yang sudah berjalan. Dua subscribe berdekatan pernah
membuat probe range tampil lalu saldo dan rencana tidak datang sampai timeout
120 detik. Task lookup sekarang disimpan sebagai `JoinHandle` dan di-abort saat
subscribe baru datang. Memilih pairing, memilih pool, atau meminta rencana juga
membatalkan discovery inkremental yang belum selesai.

Saldo dari snapshot cache harus selesai sebelum discovery penuh mulai. Statistik
lama memakai tiket generasi yang diperiksa sebelum setiap potongan, dan seluruh
pembaca log latar berbagi satu mutex. Jeda singkat antar-potongan mencegah burst.
Pada build akhir, dua probe memberi statistik, range, dan pairing pada 5,54
sampai 5,92 detik, saldo USDG pada 7,23 sampai 7,75 detik, dan `open_plan` pada
12,66 sampai 13,56 detik dari subscribe. Bagian perencanaan sesudah nominal
dikirim memerlukan 5,43 sampai 5,80 detik. Simulasi mint, pencarian Zap, dan
harga gas dimulai bersama; V4 memakai satu `eth_estimateGas` untuk membuktikan
G4 sekaligus memperoleh estimasi gas.

## Single sided menyatakan sumber modal, bukan bentuk likuiditas akhir

Saat pengujian awal, range $0,001203 sampai $0,003615 memuat harga aktif sehingga
posisi CLMM membutuhkan kedua aset dan satu input USDG memakai Zap sebagian.
Saat probe akhir, harga sudah turun ke sekitar $0,001064, di bawah range hasil
snap. Posisi akhirnya hanya menerima PAIREX. Karena sumbernya tetap USDG, engine
sekarang menyiapkan Zap 100 persen USDG ke PAIREX. Sebelumnya solver menganggap
semua range satu sisi tidak perlu Zap, sehingga pipeline berhenti pada
likuiditas nol jika user memegang sisi yang salah.

Pool ber-APR terpilih mempunyai tick spacing 1.000, sehingga grid on chain
membulatkan permintaan ke sekitar $0,0011508448 sampai $0,0038207101. Dua probe
akhir dengan sumber 1 USDG sama sama menghasilkan `zap=true`, jumlah swap
1.000.000 unit terkecil USDG, dan kutipan sekitar 891,61 PAIREX. Probe berhenti
pada `open_plan`; tidak ada approval atau transaksi yang dikirim.
