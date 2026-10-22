# Robinhood Chain

Alamat kontrak TIDAK ditulis di berkas ini. Sumber tunggalnya `chain.toml` di
akar repo. Berkas ini menjelaskan chain-nya dan cara memverifikasi isi
`chain.toml` dari nol.

## Jaringan

| Item | Nilai |
| --- | --- |
| Chain id | 4663 (`0x1237`) |
| Jenis | Arbitrum Orbit L2 |
| Gas token | ETH, 18 desimal |
| Block time | sekitar 100 ms |
| Sequencer | first come first served, tanpa lelang priority fee |
| RPC publik | `https://rpc.mainnet.chain.robinhood.com`, dibatasi laju |
| Explorer | `https://robinhoodchain.blockscout.com` |
| Mainnet sejak | 1 Juli 2026 |

Konsekuensi sequencer tanpa lelang: yang menang adalah yang paling cepat
sampai, bukan yang membayar paling mahal. Itu alasan utama engine ditulis dalam
Rust dan memakai koneksi RPC persisten dengan nonce yang dikelola lokal.
Lihat PRD 6.5 dan 11.11.

## Tiga peringatan yang harus dibaca sebelum menulis kode eksekusi

**Satu, UniversalRouter di chain ini adalah fork.** Struct swap V4 membawa
field tambahan `minHopPriceX36`. Calldata hasil SDK Uniswap standar akan gagal.
Encoder wajib dibangun manual di modul `exec::router_rbh` dan diverifikasi
dengan golden vector. Entri di `chain.toml` sudah ditandai `forked = true`.

**Dua, banyak pool V4 memakai hook.** Hook bisa mengubah fee, membatasi siapa
yang boleh menambah likuiditas, atau memungut potongan. Alamat hook wajib
dibaca dari PoolKey dan dicek terhadap allowlist. Kriteria allowlist berbasis
adopsi ada di PRD 5.4, dengan satu syarat mutlak: hook yang masih bisa
diupgrade selalu ditolak berapa pun adopsinya.

**Tiga, chain ini secara resmi dinyatakan tidak stabil.** gmgn menampilkan
banner "Robinhood Chain is unstable, data and trades may be delayed". Engine
tidak boleh mempercayai satu sumber harga, dan wajib memantau lag blok.
Lihat PRD 7.5.

## Cara memverifikasi chain.toml dari nol

### Alamat

Sumber utama:

- Uniswap V3: halaman deployment resmi Uniswap untuk Robinhood Chain.
- Uniswap V4: dokumentasi protokol Robinhood Chain.
- Semua alamat: cocokkan di `https://robinhoodchain.blockscout.com`, pastikan
  ada bytecode dan kontraknya terverifikasi.

Jangan pernah menyalin alamat dari hasil pencarian, forum, atau tangkapan layar.
Hanya dari dokumentasi resmi protokol atau dari explorer.

### Bytecode

```
cargo xtask verify
```

Perintah ini membaca `chain.toml`, memanggil `eth_getCode` untuk setiap alamat,
menghitung keccak256 dari bytecode, lalu membandingkan dengan `code_hash` yang
tercatat. Hasilnya:

| Tanda | Arti | Akibat |
| --- | --- | --- |
| `cocok` | terpin dan sama | lolos |
| `belum` | ada kode, hash belum pernah dipin | peringatan |
| `KOSONG` | tidak ada kode di alamat itu | gagal, alamatnya kemungkinan salah |
| `BEDA` | terpin tapi tidak sama | gagal keras, engine menolak start |

Untuk mengunci hash setelah yakin alamatnya benar:

```
cargo xtask verify --pin
```

Penulisan dilakukan per baris supaya seluruh komentar di `chain.toml` tetap
utuh. Komentar itu memuat peringatan tentang UniversalRouter, jadi jangan
sekali kali menulis ulang berkasnya dengan serialisasi TOML biasa.

### Kenapa `BEDA` menghentikan engine

Bytecode yang berubah di alamat yang sama berarti salah satu dari dua hal:
kontraknya proxy yang implementasinya diganti, atau kita sedang bicara dengan
chain yang salah. Keduanya membuat pengiriman transaksi berbahaya. Karena itu
gate G1 di PRD 6.4 memperlakukannya sebagai penghentian, bukan peringatan.

## Token

WETH di chain ini adalah proxy aeWETH yang kompatibel WETH9. Alamatnya ada di
`chain.toml` bagian `[tokens]`. V4 juga mendukung ETH asli sebagai currency,
bukan hanya WETH, jadi engine memilih jalur yang biayanya paling rendah dan
mencatat pilihannya di log posisi.

WETH bukan token kutipan yang dominan di sini. USDG di
`0x5fc5360D0400a0Fd4f2af552ADD042D716F1d168` jauh lebih banyak dipakai, dan
desimalnya **6**, bukan 18. Karena itu engine tidak pernah menebak dengan token
apa sebuah memecoin berpasangan. Ia memindai semua pool yang menyentuh token
itu lalu mengelompokkannya per token lawan. Lihat PRD 4.3b dan
`memory/005-pasangan-token-dan-fee-jebakan.md`.

Untuk memeriksa sebuah token sendiri:

```
cargo xtask pairs 0xdc4dd8839f02f2242e9552fe4652635fb7f95cc6
```

Perintah itu menyebut setiap token lawan beserta jumlah poolnya, dan merinci
pool yang dibuang karena kosong atau karena feenya di luar akal sehat.
