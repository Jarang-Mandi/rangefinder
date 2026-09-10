# Rencana otomatis dan uji transaksi pertama

## Rencana tidak perlu menjadi langkah UI

`quote_range` sendiri tidak mengirim transaksi. Karena itu input setoran dapat
memanggilnya otomatis setelah stabil 400 ms tanpa mengurangi persetujuan user
atas aksi on-chain. Panel menyimpan sidik jari sisi, nominal, mode zap, pool,
dan kedua tick supaya jawaban rencana tidak memicu permintaan identik tanpa
akhir. Mengubah salah satunya tetap membatalkan rencana lama lebih dulu.

Tombol **Susun rencana** dihapus. Tombol yang muncul sesudah rencana kini
langsung mewakili tindakan berikutnya: approval, zap, atau buka posisi. Ketiga
jenis transaksi tetap membutuhkan klik karena dapat memindahkan dana atau
memberi izin kontrak.

Fixture `check-execution.cjs` membuktikan rencana muncul hanya dengan mengisi
nominal, tanpa Enter atau klik tambahan, serta tetap menguji lebar 320, 360,
dan 480 piksel.

## Batas uji live pertama

User mengizinkan transaksi sungguhan dengan total modal di bawah US$5. Config
lokal memakai `max_position_usd = 5`, `max_daily_deploy_usd = 5`, dan
`new_pool_first_size_usd = 5`, sementara nominal uji dipilih 0,0005 ETH.
Kuotasi dry run PONS/ETH membagi 0,000214462280273437 ETH untuk zap dan
menahan sisanya bagi posisi. Uji live belum boleh disebut berhasil sebelum
receipt zap, seluruh approval yang diperlukan, receipt mint, dan gate G7
tercatat dari rantai.

Zap pertama kemudian sukses pada transaksi
`0x94f3dcd76df96b60450515e32d15a6a0fbaff62cf56aad3a0cfb227c7c1dce01`
dan menghasilkan sekitar 0,9482 PONS. Dua approval Permit2 berikutnya juga
sukses, tetapi quote ulang menaikkan kebutuhan mint sedikit demi sedikit dan
izin yang persis sebesar snapshot sebelumnya langsung kurang. Jalur posisi
baru diperbaiki supaya approval memakai seluruh anggaran flow yang sudah
dibatasi input user. Ini tetap terbatas, sekaligus stabil terhadap perubahan
rasio selama approval masuk blok.

Uji ini juga menemukan penerjemah revert yang mencari substring `ti`. Kata
`execution` sendiri memuat substring itu, sehingga selector Permit2
`0xd81b2f2e` (`AllowanceExpired`) dan `0xf96fb071`
(`InsufficientAllowance`) ditampilkan sebagai error tick. Selector Permit2
kini dikenali lebih dulu dan pola tick dipersempit menjadi `reverted: TI`.
