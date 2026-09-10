# 002 Jebakan saat menyentuh chart gmgn

Ditulis 5 September 2026, dari riset kelayakan sebelum fase 1.

Kontrak lengkapnya ada di `docs/GMGN.md`. Berkas ini hanya memuat hal hal yang
sudah terbukti membuang waktu.

## `createShape` mengembalikan Promise

Ini yang pertama kali menjebak. Kode ini terlihat benar dan gagal:

```js
const id = chart.createShape({ time, price }, { shape: 'horizontal_line' });
chart.getShapeById(id);   // Error: There is no such shape
```

Yang benar:

```js
const id = await chart.createShape({ time, price }, { shape: 'horizontal_line' });
```

Pesan errornya menyesatkan, karena terdengar seperti shape-nya gagal dibuat.
Padahal shape-nya berhasil dibuat, hanya saja `id` masih berupa Promise pada
saat dibaca. Gejala sampingannya: `getAllShapes().length` tetap 0 kalau dicek
segera setelah pemanggilan, lalu tiba tiba berisi setelah Promise selesai.

`createMultipointShape` berperilaku sama.

## Entity id acak, jangan andalkan urutan

Entity id berupa string pendek acak seperti `jnm56k` atau `OOyO39`. Simpan
sendiri di state. Jangan pernah menyimpulkan mana garis atas dan mana garis
bawah dari urutan `getAllShapes()`.

## `getAllShapes` memuat shape milik user

Menghapus massal akan menghapus gambar user, dan itu tidak bisa dibatalkan dari
sisi kita. Hapus hanya id yang tercatat sebagai milik Rangefinder.

Saat riset saya sempat menghapus semua shape di halaman untuk membersihkan
percobaan. Di halaman uji itu aman karena tidak ada gambar user, tapi kalau
pola itu masuk ke kode produksi akibatnya buruk.

## Satuan chart bukan harga

`activeChart().symbol()` mengembalikan `robinhood/<address>/USD/MCAP`. Jadi
sumbu vertikalnya **market cap dalam USD**, sedangkan tick Uniswap berbasis
harga token1 per token0. Konversinya empat langkah dan urutan token perlu
diperiksa, karena `token0` adalah alamat yang lebih kecil secara numerik dan
kalau memecoin-nya ternyata token1 semua nilai harus diinversi.

Rumus lengkapnya di PRD 4.3. Jangan menulis ulang dari ingatan.

Chart juga punya toggle `Price / MC`, `USD / ETH`, dan skala `log`. Ketiganya
mengubah arti angka pada garis, jadi mode aktif wajib dibaca ulang setiap kali
user mengubahnya.

## gmgn adalah Next.js satu halaman

Berpindah token tidak memuat ulang dokumen. Mendengarkan `load` tidak akan
pernah terpicu untuk token kedua dan seterusnya. Pakai History API ditambah
`MutationObserver`.

## Konteks eksekusi harus MAIN world

`tradingViewApi` adalah variabel milik halaman. Content script di isolated
world bisa melihat DOM iframe yang same origin, tapi **tidak** bisa melihat
variabel JavaScript halaman. Manifest wajib memakai `world: "MAIN"`.

## Harga terakhir hanya ada di struktur internal

Tidak ada API publik untuk membaca lilin terakhir. Yang berhasil:

```js
chart.getSeries().data().m_bars.last().value
// [waktu, open, high, low, close, volume, ...]
```

`getSeries()` sendiri mengembalikan objek yang `Object.keys` dan prototipenya
tidak menampilkan method apa pun, jadi menebak dari introspeksi tidak membantu.
Yang membuka jalan adalah memeriksa hasil `data()`, yang memperlihatkan
`m_bars`.

Nilainya array, bukan objek, jadi urutan indeksnya yang menentukan arti. Karena
itu pembacanya wajib memeriksa `high >= max(open, close)` dan
`low <= min(open, close)`. Kalau urutan indeksnya berubah di versi Charting
Library berikutnya, pemeriksaan itu yang akan menangkapnya, bukan pengguna yang
menemukan garisnya salah tempat.

## URL baru bisa masih memakai data token lama

Pengujian browser 10 September menemukan bahwa navigasi SPA mengubah URL lebih
dulu. Selama sekitar satu detik, `tradingViewApi.activeChart().symbol()` dan
lilin terakhir masih milik token sebelumnya. Memeriksa URL saja justru membuat
harga lama diberi label token baru. Attach harus menunggu sampai segmen alamat
pada simbol sama dengan token URL.

Pada profil Chromium baru, API chart juga pernah belum siap setelah 15 detik.
Bridge harus mencoba attach lagi dengan satu timer, selama token dan generasi
belum berubah. Jangan memperpanjang satu Promise tanpa batas karena iframe yang
dibuang memang tidak akan memanggil callback `dataReady`.

Bundle hasil build diuji sebagai ekstensi unpacked pada dua tab token GMGN.
Keduanya mencapai capability tier 1 dengan simbol yang cocok. Pada chart berisi
candle, perintah bridge menambah tepat dua shape lalu `clear_lines`
mengembalikan jumlah shape ke keadaan awal.
