# Rangefinder

Ekstensi Chrome side panel dengan engine Rust lokal untuk mengelola
concentrated liquidity Uniswap V3 dan V4 di Robinhood Chain. Batas atas dan
batas bawah range digambar langsung di atas chart gmgn.ai dan bisa digeser.

Semuanya berjalan di mesin sendiri. Tidak ada akun, tidak ada server, tidak ada
telemetri.

## Kenapa ini ada

Menempatkan LP di memecoin biasanya menebak. Trader melihat chart di gmgn, lalu
pindah ke antarmuka lain untuk menentukan angka batas, tanpa tahu berapa
likuiditas yang sudah antre di range itu, berapa lama harga realistis bertahan
di dalamnya, dan berapa jam fee yang dibutuhkan untuk menutup impermanent loss.

Rangefinder menjawab ketiganya di tempat keputusannya diambil, yaitu di atas
chart itu sendiri.

## Yang membedakan

- **Garis range langsung di chart gmgn**, bukan di antarmuka terpisah.
- **Peta kepadatan likuiditas pesaing**, karena fee yang diterima sebanding
  dengan porsi likuiditas kita di range aktif, bukan dengan lebar range.
- **Jam impas impermanent loss**, satu angka yang menentukan tapi hampir tidak
  pernah ditampilkan alat mana pun.
- **Deteksi sideways bervolume tinggi**, kondisi terbaik untuk LP dan justru
  kondisi yang paling dihindari trader spot.
- **LP satu sisi sebagai alat eksekusi**, range di bawah harga berperan sebagai
  tangga limit buy yang dibayar fee.

## Status

Fase 0 dan 1 selesai dan terverifikasi. Engine berjalan terhadap Robinhood
Chain, gate verifikasi kontrak lolos 17 dari 17, siklus wallet berfungsi, dan
ekstensi sudah bisa dimuat. Eksekusi on chain, yaitu membuka dan menutup posisi,
belum ada dan dijadwalkan di fase 3.

Rincian per fase beserta buktinya ada di `CLAUDE.md`.

## Menjalankan

```
cargo run -p rangefinder-engine    # engine lokal di 127.0.0.1:8787
cd extension && npm install && npm run build
```

Lalu muat `extension/dist` lewat `chrome://extensions` dengan Load unpacked,
dan tempel token pairing yang ditampilkan engine.

Langkah lengkap beserta pemecahan masalah ada di `docs/RUNBOOK.md`.

## Dokumen

| Berkas | Isi |
| --- | --- |
| `docs/PRD.md` | Spesifikasi lengkap, termasuk doktrin penempatan range |
| `CLAUDE.md` | Manual teknik untuk yang mengubah kodenya |
| `docs/DECISIONS.md` | Keputusan arsitektural beserta alasannya |
| `docs/GMGN.md` | Kontrak terhadap gmgn dan cara memverifikasinya ulang |
| `docs/CHAIN.md` | Robinhood Chain dan cara memverifikasi alamat kontrak |
| `docs/RUNBOOK.md` | Menjalankan, dan apa yang dilakukan saat ada yang salah |

## Peringatan

Engine menyimpan kunci privat terenkripsi di mesin ini dan menandatangani
transaksi sendiri. Pembatas nominal bawaannya tidak terbatas. Artinya pengaman
satu satunya adalah isi wallet itu sendiri.

Pakai wallet terpisah khusus operasi ini, dan isi hanya sebanyak yang siap
dipertaruhkan.
