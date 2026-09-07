# 007 Pengiriman transaksi, dan tiga hal yang hampir terlewat

Ditulis 7 September 2026, saat menyelesaikan fase 3.

## Yang dibangun

Fase 3 sebelumnya punya ketujuh gate sebagai fungsi terpisah yang lolos uji
satuan, tapi tidak ada yang benar benar mengirim transaksi. Sekarang jalurnya
lengkap: `quote_range` menyusun rencana, `approve_token` memberi izin, dan
`open_position` mengirim mint-nya.

Dibuktikan terhadap pool sungguhan lewat `cargo xtask dryrun`:

```
pasangan  WETH (18 desimal), 1 pool layak
pool      0x714442e9A6 V3 spacing 200 tick 110936
lolos G0   versi protokol     pool V3, cocok dengan jalur eksekusi ini
lolos G6   pembatas nilai     dalam batas
lolos G2   builder calldata   356 byte, tick pas di grid 200
lolos G3   golden vector      terpenuhi saat build
lolos G4   simulasi           likuiditas 400457213422599480459 tercipta
lolos G5   asersi invarian    hasil simulasi sesuai harapan
```

## Tiga hal yang hampir terlewat

### 1. Permit2 tidak berlaku di jalur yang kita pakai

PRD 6.2 menyebut approval memakai Permit2 supaya tidak perlu approve terpisah
tiap token. Itu keliru untuk NonfungiblePositionManager v3, yang menarik token
lewat `transferFrom` biasa dan butuh allowance ERC20 langsung ke alamatnya.
Permit2 dipakai UniversalRouter, bukan NPM.

Kalau ini tidak ketahuan, `mint` akan selalu revert dengan
`TRANSFER_FROM_FAILED`, dan revert itu terjadi **setelah gas terbakar**.

Pelajarannya: rencana di dokumen bukan bukti. Jalur yang ditulis di PRD harus
dicocokkan dengan cara kontraknya benar benar bekerja sebelum dianggap ada.

### 2. Pool V4 lewat jalur V3 menghasilkan pesan yang menyesatkan

Percobaan pertama terhadap FIG gagal di G4 dengan `execution reverted` tanpa
data, dan penjelasan terdekat yang bisa ditebak dari revert kosong adalah
"tick di luar batas yang diizinkan pool". Itu menyesatkan: ticknya benar,
protokolnya yang salah. Pool itu V4, sedangkan seluruh jalur eksekusi memakai
NonfungiblePositionManager v3.

Ditambahkan gate G0 yang memeriksa versi protokol paling awal, karena ia paling
murah dan paling pasti. Sekarang pesannya menyebut sebab sebenarnya.

Pelajarannya: revert tanpa data tidak boleh diterjemahkan menjadi tebakan yang
terdengar spesifik. Tebakan yang terdengar yakin mengirim orang mengejar
masalah yang tidak ada.

### 3. Memecah rentang saat kena 429 justru memperburuk

Pemindaian log mengecilkan rentang setiap kali gagal, dengan anggapan
kegagalan berarti rentangnya kelebaran. Saat sebabnya ternyata rate limit,
mengecilkan rentang berarti menambah jumlah permintaan, dan pemindaian akhirnya
menyerah pada rentang 2929 blok yang sebenarnya sanggup dilayani.

Sekarang 429 dibedakan dari rentang kelebaran. Untuk 429 obatnya menunggu,
untuk rentang kelebaran obatnya memecah. Keduanya muncul sebagai kegagalan yang
sama, tapi obatnya berlawanan.

### 4. RangeView memakai orientasi harga, MintParams memakai orientasi tick

Percobaan pertama lewat panel ditolak dengan `batas bawah tidak boleh melewati
batas atas`, padahal rangenya jelas benar. Sebabnya `RangeView` menamai
batasnya menurut HARGA bagi user, sedangkan MintParams menuntut `tickLower <
tickUpper` dalam ruang TICK.

Kalau memecoin menempati token1, tick yang lebih besar justru berarti harga
memecoin lebih rendah, sehingga `lower.tick` menjadi yang lebih besar. Memakai
namanya apa adanya menolak setiap posisi pada pool seperti itu, yaitu kira kira
separuh dari semua pool, dengan pesan yang terdengar seperti kesalahan user.

Sekarang lewat `execute::tick_menaik`, beserta ujinya.

Pelajarannya: nama field yang sama bisa berarti hal berbeda di dua ruang yang
berbeda. Invarian pembalikan sumbu tick sudah tercatat sejak fase 2, dan tetap
terlewat saat menyeberang ke modul lain, karena di sana namanya terlihat wajar.

## Invarian pengiriman yang wajib dijaga

1. **Chain id diperiksa sebelum menandatangani.** Tanda tangan untuk chain id
   yang salah bisa diputar ulang di rantai lain, dan tetap sah selamanya.
2. **Nonce dipesan lewat satu penjaga, dan dilepas kalau pengiriman gagal.**
   Nonce yang bocor membuat seluruh transaksi berikutnya menggantung menunggu
   nonce yang tidak akan pernah datang. Tapi kalau transaksinya sudah sampai
   rantai dan hanya receiptnya yang belum datang, nonce TIDAK boleh dilepas.
3. **Receipt sukses belum berarti berhasil.** G7 membaca ulang posisinya dari
   rantai dan membandingkannya dengan yang diminta.
4. **Tidak ada percobaan ulang otomatis.** Dua transaksi yang sama berarti dua
   posisi beserta dua kali biayanya.
5. **Rencana disimpan di engine, tidak bolak balik lewat panel.** Rencana yang
   kembali dari panel bisa saja sudah diubah, dan bagian yang paling menarik
   untuk diubah justru calldata-nya.
6. **Rencana dikeluarkan dari slot sebelum dikirim.** Rencana yang masih
   tersimpan setelah dikirim bisa disetujui dua kali.
7. **Izin sebesar yang dibutuhkan, bukan tak terbatas.** Wallet ini panas dan
   batas nominal posisi sengaja tidak dibatasi atas permintaan user. Keduanya
   bersama berarti izin tak terbatas menaruh seluruh saldo pada satu kontrak,
   selamanya.

## Yang belum pernah terjadi

Belum ada satu pun transaksi sungguhan yang dikirim proyek ini.
`execution.dry_run` menyala secara bawaan, dan mematikannya adalah keputusan
sadar user. Jangan menyalakannya atas inisiatif sendiri.
