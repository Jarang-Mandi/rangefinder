# Prioritas RPC dan kutipan Zap paralel

## Pastikan proses yang melayani port adalah build release

Keluhan membuka posisi yang lambat pada 12 September 2026 ternyata terjadi
ketika port 8787 dilayani `target/debug/rangefinder.exe` dari `cargo run`,
sementara scheduled task build release hanya berstatus siap. Nama proses tidak
cukup untuk membedakannya. Periksa path executable pemilik listener 8787.

Pengukuran awal pada token `0xba527c23903eff094a987197ccd643f2da76e4ff`
dengan build release lama memberi daftar pool sekitar 1,8 detik, saldo sekitar
5,7 detik, dan `open_plan` sekitar 30,5 detik. Probe hanya memakai subscribe,
harga chart, dan `quote_range`; tidak ada transaksi yang dikirim.

## Statistik latar tidak boleh berebut jalur rencana

Tahap cache dan discovery penuh dapat memanggil pengiriman saldo hampir
bersamaan. Jika himpunan poolnya identik, hanya satu pengukuran statistik 24 jam
yang boleh berjalan. Kuncinya mencakup token, pasangan, dan himpunan pool agar
hasil discovery yang benar benar berubah tetap dapat diukur.

Pemindaian volume memeriksa penjaga prioritas sebelum setiap potongan log.
Potongan yang sudah dikirim dibiarkan selesai, lalu potongan berikutnya menunggu
sampai penyusunan rencana atau transaksi interaktif selesai. Membatalkan
request RPC yang sudah berjalan lebih berisiko daripada memberi jalan pada
batas potongan yang jelas.

## Titik Zap tidak harus menunggu berurutan

Penyelesai Zap biasanya mengunjungi sekitar 14 titik untuk ketelitian satu per
sepuluh ribu modal. Semua titik awal itu sudah diketahui dan tidak saling
bergantung. Menanyakan V4Quoter satu per satu mengalikan latensi RPC sampai
rencana menunggu sekitar 30 detik.

Engine menanyakan maksimal enam belas titik sekaligus, lalu tetap meminta satu
kutipan langsung untuk jumlah final. Titik yang gagal sekali dicoba ulang
berurutan. Hasil final, batas slippage, dan calldata tetap berasal dari rantai.
Paralelisme tidak boleh diperluas ke pemindaian log berat karena endpoint publik
pernah membatasi pekerjaan log yang bersamaan.

Rencana sebelum Zap tidak perlu membaca izin PositionManager. Saldo dua sisi
belum terbentuk, dan flow selalu menyusun ulang rencana sesudah swap. Membaca
izin itu lalu membuang hasilnya hanya menambah satu perjalanan RPC.

Saldo native dan batch ERC20 juga tidak saling bergantung sehingga boleh dibaca
bersamaan. Jika pasangan memakai ETH asli, angka yang dibaca untuk sisi native
adalah saldo gas yang sama dan harus dipakai ulang.

Setelah build final dipasang, tiga probe berurutan pada token yang sama memberi
daftar pool dalam 1,46 sampai 1,70 detik, saldo dalam 3,13 sampai 3,36 detik,
dan `open_plan` Zap dalam 9,88 sampai 10,73 detik. Ketiganya tetap menghasilkan
Zap. Dibanding pengukuran awal, waktu rencana turun sekitar dua pertiga tanpa
mengirim transaksi apa pun.
