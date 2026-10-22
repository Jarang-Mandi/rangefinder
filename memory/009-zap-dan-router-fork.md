# 009 Zap, dan bug yang hanya terlihat pada data sungguhan

Ditulis 7 September 2026, saat menyelesaikan fase 4.

## Yang dibangun

Zap satu sisi: user cukup memegang satu token, engine menghitung berapa bagian
yang harus ditukar supaya sisanya tepat memenuhi rasio range. Tiga bagian:
penyelesai iteratif, kutipan dari V4Quoter, dan encoder swap UniversalRouter.

## Bug yang paling penting: membandingkan dua token berdesimal berbeda

Penyelesai memakai bagi dua, dan arah pencariannya semula ditentukan dengan
membandingkan sisa token0 terhadap sisa token1. Uji satuan lolos semua, karena
di sana kedua token berskala sama.

Pada pool FIG/USDG sungguhan, bugnya langsung terlihat:

```
sisa0        77.380 satuan USDG, berdesimal 6
sisa1    12.325.754 satuan FIG,  berdesimal 18
```

Perbandingan angka mentah menyatakan sisa1 jauh lebih besar, padahal nilainya
justru jauh lebih kecil. Pencariannya lalu berjalan ke arah yang salah dan
berhenti di titik pertama, menyisakan lima belas persen modal menganggur.

Yang benar adalah membandingkan **likuiditas yang bisa dibentuk tiap sisi
seorang diri**. Sisi yang menghasilkan lebih sedikit adalah yang mengikat, dan
itu perbandingan tanpa satuan.

Hasilnya, dengan modal yang sama:

| | Likuiditas | Modal menganggur |
| --- | --- | --- |
| versi salah | 239.953.059.666.389 | 15 persen |
| versi benar | 259.595.705.243.535 | mendekati nol |

Selisihnya 8,2 persen likuiditas dari modal yang sama.

**Pelajarannya:** angka dari dua token berbeda tidak pernah boleh dibandingkan
langsung, bahkan untuk keputusan yang terasa sepele seperti arah pencarian. Uji
satuan tidak menangkapnya karena data buatan cenderung berskala sama. Yang
menangkapnya adalah alat yang menjalankan seluruh alur terhadap pool nyata,
`cargo xtask zap`, beserta sapuan yang menampilkan bentuk fungsinya.

## Kenapa bagi dua, bukan Newton

PRD 6.1 semula menyebut Newton dua sampai tiga putaran. Newton butuh turunan,
dan bentuk turunannya bergantung pada model dampak harga yang berbeda antara V3
dan V4 berhook. Bagi dua tidak butuh turunan sama sekali, dan pada fungsi
monoton ia pasti konvergen.

Jumlah putaran dibatasi bukan karena biaya di memori, melainkan karena setiap
putaran adalah satu permintaan RPC. Berhenti saat jangkauan sudah lebih kecil
dari satu per sepuluh ribu modal menurunkan putaran dari 18 ke 14, dengan
selisih likuiditas 0,003 persen.

## Field fork ditemukan lewat aritmetika offset

UniversalRouter di rantai ini adalah fork, dan kita tidak punya kode sumbernya.
Bentuk sebenarnya dibaca dari transaksi swap nyata
`0xf15c5f65d6f0372698805158351bf8c13de3578692a721a06094a365449fcd86`.

Caranya: offset `hookData` di transaksi itu bernilai `0x140`, yaitu sepuluh kata
dari awal struct. Struct Uniswap standar hanya sembilan kata, yaitu PoolKey lima
kata, lalu `zeroForOne`, `amountIn`, `amountOutMinimum`, dan offset `hookData`.
Selisih satu kata itulah `minHopPriceX36`, dan nilainya nol.

Kalau encoder memakai struct standar, `hookData` mendarat di posisi yang salah
dan kontrak membaca sampah. Uji golden vector membangun ulang seluruh 1124 byte
dan membandingkannya byte per byte.

**Pelajarannya:** struktur data yang tidak terdokumentasi bisa dipulihkan dari
offset di dalam transaksi sungguhan. Itu bukti yang jauh lebih kuat daripada
dokumentasi, karena ia berasal dari transaksi yang benar benar berhasil.

## Hal lain yang perlu diingat

1. **V4Quoter tidak bertanda `view`.** Ia menjalankan swap lalu revert untuk
   membatalkannya, dan hasilnya dibaca dari data revert. Hanya boleh lewat
   `eth_call`, tidak pernah dikirim sebagai transaksi.
2. **Kutipan nol adalah kegagalan, bukan jawaban.** Pool tanpa likuiditas di
   arah itu menjawab nol tanpa revert, dan menerimanya sebagai jawaban sah akan
   menghasilkan pembagian modal yang sepenuhnya salah.
3. **Zap tidak bisa atomik di rantai ini.** Swap lewat UniversalRouter, posisi
   lewat PositionManager, dan tidak ada kontrak yang bisa keduanya. Jadi dua
   transaksi, dengan `amountOutMinimum` sebagai penahan pergerakan harga di
   antaranya.
4. **Router butuh izin Permit2 sendiri.** Izin ke PositionManager tidak berlaku
   untuk UniversalRouter, dan melewatkannya membuat swap revert setelah gas
   terbakar.
5. **Ada ambang kelayakan.** Zap hanya ditawarkan kalau modal yang menganggur
   melebihi satu persen. Di bawah itu, gas dan slippage lebih mahal daripada
   perbaikannya.
