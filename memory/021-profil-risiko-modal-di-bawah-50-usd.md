# Profil risiko modal di bawah 50 USD

User mengubah keputusan pembatas nominal: Rangefinder sekarang ditujukan untuk
modal operasional di bawah 50 USD. Nilai bawaan dan konfigurasi lokal aktif
adalah 5 USD per posisi, 5 USD modal baru per hari, maksimal dua posisi aktif,
dan 2,50 USD untuk posisi pertama pada pool yang belum pernah dipakai.

Kombinasi ini membatasi eksposur posisi berukuran maksimum ke 10 USD dan membuat
pool baru dimulai dengan setengah ukuran biasa. Batas harian menghitung modal
yang sudah dikomit lewat jurnal, sedangkan jumlah posisi dibaca dari on-chain.
Posisi yang dibuat di luar Rangefinder karena itu tetap ikut dalam batas dua.
Pembacaan ini diuji langsung pada alur PAIREX/USDG. Dua probe memberi saldo
dalam 2,38 sampai 2,59 detik dan `open_plan` dalam 5,36 sampai 5,72 detik dari
subscribe, dengan G6 lulus. Jadi pemeriksaan posisi on-chain tidak mengembalikan
latensi panjang yang sudah diperbaiki sebelumnya.

Slippage Zap diturunkan dari 300 menjadi 200 basis poin. Cadangan gas tetap
0,002 ETH. Posisi yang keluar range tetap dinaikkan ke bagian teratas halaman
Positions, tetapi engine tidak menutup atau melakukan rebalance sendiri. Semua
aksi yang memindahkan dana tetap membutuhkan satu klik user dan dikerjakan oleh
flow engine di belakang layar.
