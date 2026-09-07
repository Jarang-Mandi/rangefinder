# 005 Token lawan tidak boleh ditebak, dan pool berfee ekstrem itu jebakan

Ditulis 7 September 2026, saat fase 3.

## Kejadiannya

User menempel alamat `0xdc4Dd8839F02F2242E9552fe4652635fB7F95cC6` (FIG) ke
panel. Engine menjawab tidak ada pool. Padahal di Zenith LP, token yang sama
menampilkan puluhan pool V4 yang jelas hidup dan diperdagangkan.

Dua bug berbeda tertumpuk di situ, dan yang kedua jauh lebih berbahaya.

## Bug pertama: pasangan token diandaikan WETH

`discover_pools` mencari pasangan token terhadap WETH saja. Itu asumsi yang
saya bawa dari kebiasaan di Ethereum mainnet, dan tidak pernah diuji terhadap
Robinhood Chain.

Setelah pencarian dibuat tanpa asumsi, hasilnya:

```
39 pool V4 ditemukan
Berpasangan dengan:
  0x5fc5360D0400a0Fd4f2af552ADD042D716F1d168  USDG   33 pool
  0x0000000000000000000000000000000000000000  ETH     5 pool
  0x41F4267525a8AFf329540eF24fD83d9044758B33  FIG     1 pool
```

Nol pool berpasangan dengan WETH. Pencarian lama tidak menemukan apa apa bukan
karena tokennya sepi, tapi karena ia mencari pasangan yang memang tidak ada.
Di rantai ini USDG adalah token kutipan yang sebenarnya, dan V4 juga memakai
ETH asli di alamat nol, yang tidak akan pernah cocok dengan pencarian WETH.

Perhatikan juga baris terakhir: ada token lain yang juga bernama FIG di
`0x41F4267525a8AFf329540eF24fD83d9044758B33`. Simbol bukan identitas. Yang
mengikat sebuah posisi hanyalah alamat.

### Yang berubah

- `discover_v4_any` memindai semua `Initialize` yang menyentuh token itu,
  tanpa mengandaikan lawannya.
- `discover_for_token` mengelompokkan hasilnya per token lawan menjadi
  `QuoteGroup`, dan `TokenState` sekarang membawa `quotes` beserta
  `selected_quote`.
- Desimal token kutipan ikut dibawa dalam kelompoknya. USDG berdesimal 6,
  bukan 18. Menganggapnya 18 menggeser setiap harga sebesar 10^12.
- `cargo xtask pairs <alamat>` dipakai untuk melihat sebuah token berpasangan
  dengan apa saja, tanpa lewat panel.

## Bug kedua: peringkat pool memenangkan pool jebakan

Pemeringkatan lama menskor pool dengan likuiditas dikali fee, dengan alasan
yang terdengar masuk akal, yaitu fee besar berarti pendapatan besar. Data
sebenarnya membantah itu:

```
6 dari 39 layak, 26 tanpa likuiditas, 7 berfee di luar akal sehat
  dibuang  fee 99.08%  spacing 200    likuiditas 10126379563708
  dibuang  fee 87.00%  spacing 8700   likuiditas 127714945147817
  dibuang  fee 81.00%  spacing 19988  likuiditas 11335223470442529259
  dibuang  fee 80.27%  spacing 9303   likuiditas 12581485599219836851
  dibuang  fee 79.06%  spacing 200    likuiditas 3802979883995539066
  dibuang  fee 20.00%  spacing 2000   likuiditas 40276988993116175800
  dibuang  fee  0.00%  spacing 200    likuiditas 282361371356284006478705
  layak    fee  8.00%  spacing 200    likuiditas 219498049902998267
```

Pool fee 80 persen tidak menghasilkan 80 persen untuk siapa pun. Tidak ada yang
mau menukar di sana, jadi pendapatannya nol sambil terlihat paling menarik di
daftar. Menurunkan peringkatnya tidak cukup, karena likuiditasnya lima puluh
kali lipat pool asli sehingga tetap menang.

Sebaliknya pool fee 0 persen memegang likuiditas 1.286 kali lipat pool teratas
yang layak. Ia akan menjadi pilihan pertama pada pemeringkatan berbasis
likuiditas mana pun, dan menaruh modal di sana berarti menanggung impermanent
loss penuh dengan pendapatan fee nol.

### Yang berubah

`MAX_PLAUSIBLE_FEE` disetel 100_000, yaitu 10 persen. `implausible_fee` menolak
fee nol dan fee di atas ambang itu, dan `usable_pools` **membuang** pool
seperti itu, bukan menurunkan peringkatnya. Pool dynamic fee dikecualikan,
karena angka feenya adalah penanda `0x800000`, bukan besaran.

## Pelajaran

1. **Jangan mengandaikan bentuk pasar sebuah rantai dari kebiasaan rantai
   lain.** Token kutipan, desimal, dan penggunaan ETH asli berbeda beda.
2. **Simbol token bukan identitas.** Dua token bisa memakai simbol yang sama
   di rantai yang sama.
3. **Angka besar pada satu dimensi bukan tanda kualitas.** Likuiditas
   terbesar dan fee terbesar keduanya menunjuk ke pool yang tidak layak di
   sini.
4. **Penyaringan harus terlihat.** Perintah `pairs` sekarang menyebutkan apa
   yang dibuang beserta alasannya. Penyaringan yang diam membuat orang
   mengira pool itu tidak pernah ada, yang persis bagaimana bug pertama
   sanggup bertahan.
5. Pemindaian log dilakukan mundur dari blok terbaru dengan potongan adaptif,
   dan kegagalan dikembalikan sebagai error. Memindai maju dari blok deploy
   berarti 55 juta blok dan langsung kena 429. Ini pengulangan pelajaran di
   `003`, kesalahan yang sama muncul lagi di tempat baru.
