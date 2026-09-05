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

## Chart tanpa data saat riset

Saat pengujian, chart tidak menampilkan lilin sama sekali, kemungkinan karena
data chain sedang tertunda, dan gmgn memang menampilkan banner bahwa Robinhood
Chain tidak stabil. Seluruh jalur API terbukti berfungsi dan garis berhasil
dibuat lalu dibaca kembali, tapi **penggambaran garis di atas chart yang berisi
lilin belum pernah dilihat langsung**. Verifikasi ulang hal itu di fase 2
sebelum menganggap tampilannya benar.
