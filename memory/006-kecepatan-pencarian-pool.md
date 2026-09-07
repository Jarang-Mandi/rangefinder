# 006 Kecepatan pencarian pool, dan kenapa paralel justru lebih lambat

Ditulis 7 September 2026, saat fase 3.

## Kejadiannya

User melaporkan pool tidak muncul muncul di panel, sementara Zenith LP
menampilkan pool token yang sama tanpa jeda. Pengukuran memberi angka yang
memalukan: 38 detik untuk FIG dengan 39 pool.

Zenith tidak membaca sumber yang berbeda. Ia membaca PoolManager yang sama.
Selisihnya sepenuhnya pada jumlah perjalanan bolak balik ke RPC.

## Empat sebab, semuanya terukur

### 1. Panggilan satu per satu

Membaca `getSlot0` dan `getLiquidity` untuk 39 pool berarti 78 permintaan
berurutan. Latensi RPC publik sekitar 350 ms, jadi 25 detik habis di situ.

Multicall3 mengeksekusi seluruh daftar dalam satu `eth_call`. Hasilnya 380 ms.
Alamatnya sudah ada di `chain.toml` sejak awal dan tidak pernah dipakai.

Sekarang metadata token, pencarian pool V3, dan pembacaan pool V4 berada dalam
satu batch yang sama, 65 panggilan dalam satu permintaan.

### 2. Rentang log dipecah karena takut, bukan karena diukur

Nilai awal 500.000 blok per permintaan dipilih tanpa pengujian. Diukur langsung
ke RPC:

| Rentang satu permintaan | Hasil |
| --- | --- |
| 3 juta blok | berhasil, 1016 ms |
| 10 juta blok | `log query timed out` |
| 48 juta blok | `log query timed out` |

Seluruh jendela mundur muat dalam satu permintaan. Memecahnya menjadi enam
bagian hanya menambah lima perjalanan bolak balik.

### 3. Paralel membuatnya lebih lambat

Dua arah pemindaian, yaitu token sebagai `currency0` dan sebagai `currency1`,
sempat dijalankan berbarengan dengan `try_join!`. Diukur:

| Cara | Hasil |
| --- | --- |
| berurutan | 1102 ms, keduanya berhasil |
| berbarengan | yang lolos 658 ms, yang satunya kena 429 lalu diulang, total sekitar 5 detik |

Rantai ini memberi satu permintaan berat pada satu waktu. Lapisan percobaan
ulang menyembunyikan penolakannya, jadi yang terlihat hanya lambat, bukan
gagal, dan itu membuat sebabnya sulit dikenali.

Pelajarannya: pada sumber daya yang dibatasi laju, menumpuk permintaan bukan
optimasi melainkan pemborosan yang menyamar.

### 4. Sejarah dipindai ulang setiap kali

PoolKey tidak pernah berubah setelah pool dibuat. Yang berubah hanya harga dan
likuiditas. Jadi memindai ulang tiga juta blok setiap kali user membuka token
yang sama adalah pekerjaan yang jawabannya sudah diketahui.

`pools/cache.rs` menyimpan kunci pool di redb beserta blok terakhir yang sudah
dipindai. Pemuatan berikutnya cukup membaca keadaan, satu permintaan.

## Hasil

| Keadaan | Waktu sampai panel punya isi |
| --- | --- |
| versi pertama | 38 detik |
| setelah Multicall3 | 23,8 detik |
| setelah rentang log diperlebar | 15,1 detik |
| setelah pemindaian diserialkan | sekitar 3 detik |
| dengan kunci pool tersimpan | 400 sampai 700 ms |

## Aturan yang lahir dari sini

1. **Cache hanya menyimpan yang tidak pernah berubah.** Kunci pool boleh,
   harga dan likuiditas tidak pernah. Pada alat yang mengirim transaksi, angka
   basi yang terlihat sah lebih berbahaya daripada menunggu sebentar.
2. **`scanned_to` hanya maju kalau pemindaian benar benar selesai.**
   Memajukannya setelah kegagalan berarti blok yang terlewat tidak pernah
   dipindai lagi, dan poolnya hilang selamanya, tanpa satu pun pesan.
3. **Ukur batas sebelum menghormatinya.** Tiga dari empat sebab di atas berasal
   dari angka yang dipilih karena takut ditolak, bukan karena diuji.
4. **Paralel bukan otomatis lebih cepat.** Pada RPC berbatas laju, ia bisa
   lima kali lebih lambat.

## Temuan sampingan: desimal kutipan tidak boleh ditebak

Salinan FIG di `0x41F4267525a8AFf329540eF24fD83d9044758B33` berpasangan dengan
32 token berbeda sekaligus. Versi lama mengembalikan simbol `?` dan desimal 18
untuk lawan yang tidak ada di `chain.toml`.

Itu bug yang sama dengan bug USDG, hanya lebih halus, karena harganya tetap
tampil dan terlihat masuk akal. Sekarang lawan asing dibaca on chain dalam satu
batch, dan kalau pembacaannya gagal kelompoknya dibuang, bukan ditampilkan
dengan desimal tebakan.

Pemeringkatan kelompok juga berubah. Pengurutan murni likuiditas menaruh
pasangan berisi satu pool sampah di atas pasar USDG yang sebenarnya. Sekarang
pasangan terhadap kutipan yang dikenal rantai ini didahulukan, dan sisanya
tetap bisa dipilih user lewat `select_quote`.
