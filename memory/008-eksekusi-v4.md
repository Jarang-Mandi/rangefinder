# 008 Eksekusi V4, dan kenapa ABI dari ingatan bukan bukti

Ditulis 7 September 2026, saat fase 4.

## Yang dibangun

Jalur eksekusi V4 lengkap untuk membuka posisi: encoder aksi, builder calldata,
izin dua lapis lewat Permit2, gate G0 sampai G7, dan perutean otomatis menurut
versi pool. Panel tidak tahu protokol mana yang dipakai.

Dibuktikan terhadap pool V4 sungguhan:

```
pasangan  USDG (6 desimal), 3 pool layak
pool      0xc8e37d137d V4 spacing 200 tick 371247
lolos G0   versi protokol     pool V4, cocok dengan jalur eksekusi ini
lolos G2   builder calldata   868 byte, dua aksi mint lalu settle
lolos G4   simulasi           kontrak menerima, likuiditas 1411538296444878206
lolos G5   asersi invarian    jumlah, batas atas, dan nilai ETH konsisten
```

## Cara kerja V4, dibaca dari rantai bukan dari dokumentasi

Tidak ada fungsi `mint`. Satu satunya pintu masuk adalah
`modifyLiquidities(bytes unlockData, uint256 deadline)`, dan `unlockData` adalah
`abi.encode(bytes actions, bytes[] params)`.

Membuka posisi berarti dua aksi berurutan:

| Kode | Aksi |
| --- | --- |
| `0x02` | MINT_POSITION |
| `0x0d` | SETTLE_PAIR |

Parameter MINT_POSITION adalah PoolKey yang diratakan jadi lima kata, lalu
tickLower, tickUpper, liquidity, amount0Max, amount1Max, owner, dan hookData.

Seluruh bentuk ini dibaca dari transaksi
`0x92acbe0bc669fbdff06263646d45399825c1a398090fbe42e8530bde610f1ccd`, bukan
disalin dari repo Uniswap. Encoder kita diuji dengan membangun ulang calldata
transaksi itu dan membandingkannya byte per byte, seluruhnya 868 byte.

## Empat hal yang mudah salah

### 1. ABI dari ingatan bukan bukti

Rantai ini sudah terbukti memakai fork pada UniversalRouter, yang membawa field
tambahan `minHopPriceX36`. Kalau hal serupa terjadi pada PositionManager,
kekeliruannya baru terlihat saat transaksi revert setelah gas terbakar.

`cargo xtask v4probe` membuktikan dengan dua cara sekaligus: mencari setiap
selector di dalam bytecode yang terpasang, lalu memanggil fungsi bacanya
sungguhan. Hasilnya semua cocok, dan `PositionManager.permit2()` beserta
`poolManager()` sama dengan yang tertulis di `chain.toml`.

Perintah itu juga menemukan bahwa rantai ini punya 2,1 juta posisi V4, jadi V4
memang jalur yang sebenarnya dipakai orang di sini, bukan pelengkap.

### 2. Izinnya dua lapis, dan lapis pertama terlihat sudah cukup

V3 menarik token lewat `transferFrom` biasa. V4 menarik lewat Permit2, jadi:

1. Token mengizinkan Permit2 lewat `approve` ERC20 biasa.
2. Permit2 mengizinkan PositionManager lewat `Permit2.approve`, yang punya masa
   berlaku.

Melewatkan lapis kedua adalah kesalahan yang paling mudah terjadi. Masa berlaku
disetel tiga puluh menit, karena izin Permit2 yang berlaku setahun sama saja
dengan izin tak terbatas dalam waktu.

### 3. Slippage dijaga dari arah yang berlawanan

V3 memakai `amountMin`, yaitu batas bawah yang harus tercapai. V4 memakai
`amountMax`, yaitu batas atas yang boleh terpakai. Kebiasaan dari V3 akan
membuat orang menyetelnya sebesar saldo, dan itu berarti mengizinkan pool
mengambil seluruh saldo saat harga bergerak.

Engine memakai jumlah yang dibutuhkan ditambah margin slippage, dan tidak
pernah melampaui saldo.

### 4. Hook adalah bagian dari identitas pool

poolId adalah hash dari kelima field PoolKey, termasuk alamat hook. Dua pool
bisa punya currency, fee, dan tick spacing yang sama tapi hook berbeda, dan
keduanya pool yang sama sekali berbeda.

Karena itu verifikasi pasca kirim membandingkan PoolKey lengkap, bukan hanya
tick dan likuiditas.

## PositionInfo mengemas dua tick dalam satu uint256

Tata letaknya, dari bit terendah: delapan bit flag, dua puluh empat bit
`tickLower`, dua puluh empat bit `tickUpper`, sisanya poolId terpotong.
Keduanya int24, jadi bit ke-23 adalah tandanya.

Salah membacanya menghasilkan angka delapan juta untuk tick negatif, dan range
di tempat yang sama sekali berbeda. Pembacaannya diuji terhadap posisi sungguhan
tokenId 2100029, yang `info`-nya
`0x0f841897ebec50b6ea31047df4269dcb675f63d941b9014f8805822805728800` dan
menghasilkan tick 357000 sampai 361000.

## Aturan yang lahir dari sini

1. **Buktikan ABI ada di bytecode sebelum memakainya.** Dua cara sekaligus:
   cari selectornya, lalu panggil sungguhan.
2. **Bangun encoder dari transaksi nyata, bukan dari dokumentasi.** Golden
   vector yang berasal dari transaksi berhasil adalah satu satunya bukti yang
   tidak berputar.
3. **Jangan bawa kebiasaan V3 ke V4.** Nama yang mirip bisa berarti arah yang
   berlawanan, seperti `amountMin` melawan `amountMax`.
4. **Satu tempat yang memutuskan versi.** `exec::plan::plan_open_auto`. Panel
   tidak boleh tahu protokolnya, karena kalau tahu, ia akan menyimpulkan
   sendiri.
