# 010 Posisi, PnL, dan notifikasi

Ditulis 8 September 2026, saat fase 5.

## Yang dibangun

Tab Positions dan PnL beserta notifikasi toast di halaman gmgn. Dibuktikan
terhadap wallet sungguhan `0x84CFf4bC...`, yang punya enam posisi V4:

```
KELUAR V4 #2142514  fee 4.00%  tick 346000 sampai 350000
        fee belum diklaim 1925159 dan 7903951315686111084493
        hasil fee 6.0393  IL 0.0000  net 6.0393  (perkiraan)
```

## Enumerasi posisi berbeda per protokol

Diperiksa langsung di bytecode, bukan diasumsikan:

| Kontrak | `tokenOfOwnerByIndex` | `totalSupply` | `tokenByIndex` |
| --- | --- | --- | --- |
| NonfungiblePositionManager v3 | ada | ada | ada |
| PositionManager V4 | tidak ada | tidak ada | tidak ada |

V3 mengimplementasikan ERC721Enumerable, V4 tidak. Untuk V4 satu satunya cara
adalah memindai event `Transfer` yang menuju wallet, lalu membaca ulang
kepemilikannya.

**Yang di-cache hanya daftar calon, tidak pernah kepemilikannya.** NFT posisi
bisa berpindah kapan saja, dan cache yang menyimpan kepemilikan akan
menampilkan posisi milik orang lain.

## `tokensOwed` bukan fee yang sebenarnya

Field itu hanya diperbarui saat posisinya disentuh. Posisi yang dibuka lalu
didiamkan melaporkan nol selamanya, padahal fee-nya terus bertambah.

| Protokol | Cara membaca fee |
| --- | --- |
| V3 | Simulasikan `collect` lewat `eth_call` dari pemiliknya |
| V4 | `getFeeGrowthInside` dikurangi `getPositionInfo`, dikali likuiditas |

Simulasi `collect` tidak bisa lewat Multicall3, karena fungsi itu menuntut
pemanggilnya adalah pemilik NFT.

**Pengurangan pertumbuhan fee wajib membungkus, bukan jenuh.** Kontrak Uniswap
memakai aritmetika `unchecked`, dan nilainya memang dirancang melampaui batas
lalu berputar. Dengan `saturating_sub`, fee akan terbaca nol persis pada posisi
yang paling lama berjalan, yaitu yang fee-nya paling besar.

Pada V4, pemilik posisi di dalam PoolManager adalah PositionManager, bukan
wallet user, dan `salt` adalah token id. Salah di sini menampilkan fee milik
orang lain.

## Bug satuan yang tertangkap saat pengujian

Versi pertama menghitung PnL dalam satuan terkecil token1. Secara aritmetika
konsisten, tapi hasilnya:

```
hasil fee 1392536216340532297728.0000
```

Angka itu benar sebagai satuan terkecil FIG, tapi terbaca seperti dolar. Angka
yang salah satuannya lebih berbahaya daripada tidak ada angka, karena ia
terlihat seperti fakta.

Sekarang nilai dinyatakan dalam sisi yang merupakan kutipan yang dikenal rantai
ini, yaitu ETH asli atau entri `[tokens]` di chain.toml, dan dibagi desimalnya.
Hasilnya `6.0393` USDG, yang bisa dibaca. Kalau kedua sisinya memecoin, PnL
tidak ditampilkan sama sekali.

**Pelajarannya:** setiap angka yang sampai ke layar harus punya satuan yang
bisa disebutkan. Kalau satuannya tidak bisa disebutkan, angkanya tidak boleh
ditampilkan.

## Impermanent loss diukur terhadap memegang

Bukan terhadap modal awal. Keduanya dinilai pada harga sekarang, sehingga kalau
posisinya masih memegang persis yang disetor, hasilnya nol berapa pun harganya.

Mengukurnya terhadap modal awal mencampur perubahan harga dengan kerugian
menjadi LP, dan membuat strategi yang bagus terlihat buruk saat pasar turun.

## Notifikasi

**Content script menyambung sendiri ke engine, tidak lewat panel.** Kalau
jalurnya lewat panel, notifikasi berhenti persis saat ia paling dibutuhkan,
karena panel yang tertutup adalah keadaan paling umum.

**Pendinginan per pasangan kode dan subjek, bukan per kode saja.** Dua posisi
berbeda yang sama sama keluar range adalah dua kabar berbeda, dan pendinginan
per kode akan menyembunyikan yang kedua.

**Riwayat menyimpan yang tertahan juga**, ditandai. Toast boleh ditahan,
catatan tidak.

**Yang diberitahukan adalah perubahan, bukan keadaan.** Pemuatan pertama tidak
pernah memicu notifikasi, kalau tidak layar dibanjiri setiap kali engine baru
dijalankan.

**Nada dibangkitkan oscillator, bukan berkas audio.** PRD menyebut berkas
pendek yang dibundel, tapi oscillator lebih baik: tidak ada berkas yang dimuat,
tidak ada jeda pada bunyi pertama, dan tingkatnya dibedakan hanya dengan jumlah
ketukan.

## Jebakan serde yang menghabiskan waktu

`ServerMessage::NotifyHistory(Box<Vec<Kejadian>>)` gagal saat diserialkan
dengan pesan `cannot serialize tagged newtype variant containing a sequence`.
Serde dengan tag internal tidak bisa membungkus urutan di dalam varian newtype.

Obatnya varian struct: `NotifyHistory { items: Vec<Kejadian> }`.

Yang membuat ini mahal adalah kegagalannya tidak terlihat sampai pesan itu
benar benar dikirim, dan saat itu koneksinya diam tanpa penjelasan di sisi
panel. Kalau ada varian baru yang membungkus `Vec`, ingat pola ini.
