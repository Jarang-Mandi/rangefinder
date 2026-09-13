# Failover RPC dan pola indeks Zenith

## Satu endpoint cepat tidak cukup

Pada 14 September 2026 empat endpoint publik diuji untuk Robinhood Chain.
PublicNode menjawab pembacaan state sekitar 0,15 sampai 0,23 detik dan OrdoFi
sekitar 0,25 sampai 0,32 detik. Endpoint resmi memerlukan sekitar 1,4 detik.
ArrowRPC menjawab HTTP 530. Kemampuan log berbeda: PublicNode menjawab HTTP 403,
OrdoFi mengembalikan jaringan sibuk, BlockReq publik hanya melayani 32.768 blok
terbaru, sedangkan endpoint resmi menerima rentang 108.000 blok.

Pelajarannya adalah daftar endpoint saja belum memberi ketahanan. Provider
Alloy dapat mengembalikan JSON-RPC error di dalam respons HTTP yang berhasil,
jadi fallback yang hanya melihat error transport akan berhenti pada jawaban
yang sebenarnya gagal. Transport Rangefinder sekarang menolak kedua jenis
kegagalan, mencoba endpoint berikutnya secara berurutan, dan mengingat pilihan
terakhir yang sehat. Batas waktu state dibuat pendek agar endpoint yang macet
tidak menghabiskan sasaran tujuh detik.

## Provider harus mempunyai pekerjaan berbeda

`rpc.http` menjaga kesehatan dan mengirim transaksi. `rpc.read_http` membaca
metadata, saldo, state pool, serta TVL. `rpc.logs_http` membaca discovery dan
volume. Transaksi tidak boleh ikut transport baca berantai karena respons yang
hilang dapat membuat pengiriman dicoba melalui provider lain. Saldo dan
statistik boleh berjalan bersamaan setelah jalurnya terpisah.

## Kenapa Zenith tampak seketika

Pada Telegram Web, alamat PONS dan jawaban berisi 21 pool lengkap mempunyai cap
waktu menit yang sama. Token lain menghasilkan 26 sampai 30 pool dengan TVL,
APR, dan volume 24 jam langsung, sementara beberapa baris membawa label belum
terindeks. Bot juga pernah gagal ketika membaca data on-chain setelah sebuah
pool dipilih.

Pola itu menunjukkan dua lapis. Daftar dan statistik berasal dari indexer yang
terus hidup, sedangkan validasi state dilakukan saat user masuk ke alur posisi.
Ini inferensi dari perilaku bot, bukan pengetahuan atas kode Zenith. Cache redb
Rangefinder sudah memberi jalur cepat setelah token pernah dipindai. Menyamai
token baru secara penuh memerlukan indexer global yang mengikuti setiap blok;
RPC tambahan hanya membuat validasi lebih tahan gagal.

Probe PONS pada data dir baru dengan `dry_run` menyiapkan tiga pool dan seluruh
statistik awal dalam 4,137 detik. Pada proses engine terpasang yang juga sedang
melayani ekstensi, saldo tiba dalam 1,998 detik dan snapshot lengkap dalam
6,838 detik. Keduanya berada di bawah sasaran tujuh detik.

## Pengukuran transaksi Zenith secara langsung

Uji PONS/USDG V4 0,30% pada 14 September 2026 mengukur waktu dari aksi sampai
hasil terlihat di Telegram Web. Daftar 21 pool lengkap terlihat paling lambat
14,237 detik setelah alamat dikirim. Quote open terlihat paling lambat 17,919
detik setelah nominal dikirim. Angka ini adalah batas atas karena mencakup waktu
polling UI, bukan hanya waktu server bot.

Setelah konfirmasi open, transaksi masuk blok sekitar 12,5 detik kemudian dan
pesan sukses terlihat paling lambat 29,431 detik. Portfolio terlihat paling
lambat 8,910 detik setelah diminta. Quote close terlihat paling lambat 12,510
detik setelah tombol close dipilih. Setelah konfirmasi close yang benar-benar
memulai proses, transaksi masuk blok sekitar 6,7 detik kemudian dan pesan sukses
terlihat paling lambat 20,007 detik. Jadi kesan instan Zenith terutama berasal
dari satu jawaban lengkap dan indeks yang sudah hangat; cold scan dan alur
transaksinya sendiri tidak konsisten di bawah tujuh detik.

Uji nominal 1 USDG juga menegaskan bahwa biaya tetap harus terlihat sebelum
aksi: dua biaya layanan berjumlah $0,60 dan bot melaporkan gas $0,13, sehingga
hasil bersih siklus menjadi sekitar -$0,74 walaupun PnL posisi sebelum biaya
hanya -0,41%. Untuk modal kecil, waktu cepat tidak boleh menyembunyikan biaya
round-trip yang dominan.
