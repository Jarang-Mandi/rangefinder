# Snapshot pool tujuh detik dan RPC berperan

## Banyak RPC hanya membantu jika pekerjaannya dibagi

Dua endpoint aktif diuji tanpa mencetak URL lengkap atau API key. RPC publik
Robinhood Chain rata rata memerlukan 1,301 detik untuk `eth_blockNumber` dan
1,451 detik untuk satu `eth_call`, tetapi menerima log 216.000 blok dalam 1,147
detik. Endpoint Alchemy menjawab keduanya dalam 0,091 dan 0,046 detik, tetapi
menolak rentang log itu karena paketnya membatasi sepuluh blok.

Mengirim permintaan yang sama ke keduanya bukan optimasi. Itu menggandakan
kuota dan endpoint yang kalah tetap mengerjakan sesuatu yang hasilnya dibuang.
Pembagian yang berguna adalah `rpc.http` untuk state, saldo, rencana, simulasi,
dan transaksi, lalu `rpc.logs_http` untuk discovery serta volume. Instalasi
lama tanpa `logs_http` tetap memakai endpoint aktif untuk semua pekerjaan.

## Snapshot pertama harus utuh

Sebelum perubahan ini, probe pada token `0xba527c...` menerima dua pool tanpa
statistik pada 3,151 detik, saldo pada 4,763 detik, lalu APR, TVL, dan volume
baru lengkap pada 21,659 detik. Menampilkan pool lebih dulu membuat panel
terlihat rusak meski pengukuran masih berjalan.

Panel sekarang menahan setiap `token_state` yang mempunyai pool tanpa volume
atau jendela. Saat cache kosong atau hanya mencakup sebagian pool, engine
mengukur 108.000 blok terlebih dulu, sekitar tiga jam pada rantai ini. TVL
dibaca bersamaan melalui RPC interaktif. Snapshot itu disimpan dan ditampilkan
sekaligus, lalu pengukuran 24 jam menggantikannya di belakang layar. Label
volume selalu berasal dari `jendela_menit`, jadi hasil tiga jam tampil sebagai
tiga jam dan tidak menyamar sebagai satu hari.

Snapshot tiga jam sengaja belum dianggap cache segar penuh. Kalau proses 24 jam
gagal, kunjungan berikutnya tetap mencoba menyegarkannya. Hasil pengukuran yang
kehilangan volume tidak boleh menimpa snapshot tiga jam yang sudah sah.

## Hasil release

Build release dipasang melalui Scheduled Task dan diverifikasi memakai token
`0xba527c23903eff094a987197ccd643f2da76e4ff`. Pada cache versi baru yang dingin,
keadaan pool pertama tersedia di kabel pada 0,242 detik, saldo pada 0,321 detik,
dan snapshot dua pool yang seluruhnya mempunyai APR, TVL, serta volume tampil
pada 4,703 detik. Panel menahan dua kiriman awal yang statistiknya belum siap.

Pengukuran internal snapshot dingin menunjukkan TVL selesai dalam 53 ms dan
volume tiga jam dalam 4.358 ms. Pembaruan 24 jam selesai di latar pada 24,454
detik. Setelah cache itu tersedia, pembukaan ulang menampilkan statistik lengkap
pada 0,228 detik dan saldo pada 0,349 detik. Hash
biner yang dipasang sama dengan hasil build release dan Scheduled Task berstatus
berjalan.

Sebelum tinggi blok dipindah ke RPC interaktif, volume tiga jam memerlukan 6.552
ms dan snapshot lengkap baru tiba pada 6,981 detik di debug. Setelah RPC log
tidak lagi menerima permintaan `eth_blockNumber` tambahan, volume release turun
menjadi 4.358 ms dan tersedia cukup jarak dari batas tujuh detik pada endpoint
yang sehat. Pembacaan state setelah discovery juga dipindahkan ke RPC interaktif;
kiriman discovery penuh turun dari sekitar 16,2 menjadi 10,9 detik.
