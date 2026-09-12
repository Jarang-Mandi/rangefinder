# Close ke ETH, volume 24 jam, dan PnL perkiraan

## Close harus memakai jumlah yang baru diterima

Menutup NFT tidak sama dengan keluar dari risiko token. Close USDG/ZZZ,
ETH/ZZZ, atau WETH/ZZZ harus mempertahankan aset utama dan menjual ZZZ. Jika
kedua sisi token biasa, seperti PONS/ZZZ, keduanya dicoba ke ETH secara
terpisah.

Input swap tidak boleh berasal dari `balanceOf` wallet setelah close karena
angka itu juga memuat token lama. Sumber yang benar adalah `ReceivedAmounts`
yang sudah dibuktikan dari event V3 atau delta saldo wallet. Approval Permit2,
kutipan ulang, simulasi calldata yang persis, swap, dan delta ETH aktual tetap
masuk jurnal dengan flow dan position ID close. Kegagalan tahap ini adalah
close sukses dengan konversi belum selesai, bukan close gagal. Pemilihan rute
otomatis hanya memakai pool langsung V4 tanpa hook.

## Uniswap menampilkan satu hari, bukan satu jam

Bundle halaman token Uniswap meminta data `volume24H` dengan durasi `DAY` dan
menamai metriknya 24H. Statistik pool Rangefinder sebelumnya membaca 36.000
blok, kira kira 60 menit, lalu mengannualisasikannya. Lonjakan singkat karena
itu dapat membesarkan APR dan angka volumenya tidak dapat dibandingkan dengan
halaman Uniswap.

Jendela sekarang 864.000 blok, kira kira 24 jam pada cadence 100 ms. RPC tetap
ditanya per potongan 36.000 blok, tetapi helper harus meneruskan pembacaan sampai
batas bawah periode asli. Memperkecil potongan karena terlalu banyak log tidak
boleh memperkecil periode statistik. Sesudah pengukuran, pool diurutkan menurut
APR marginal dari volume, fee, dan TVL, bukan `fee x liquidity` kasar atau
modal acuan 1.000 unit yang tidak sebanding antara ETH dan stablecoin. Planner
baru memasukkan pengenceran setelah nominal setoran user diketahui.

APR pool nyaris kosong dapat membesar karena penyebut kecil. Pool dengan volume
atau TVL aktif di bawah satu persen nilai terbesar kelompok tetap ditampilkan,
tetapi tidak boleh menjadi pilihan otomatis.

Peringkat ini berguna untuk membandingkan pool pada token yang sama. Ia bukan
jaminan profit. Posisi kecil dapat tetap rugi ketika gas lebih besar daripada
fee, sedangkan pergerakan harga, slippage, dan lama posisi berada di dalam range
tetap menentukan hasil.

## Saldo harus mendahului statistik

Pengukuran 24 jam membutuhkan beberapa permintaan log dan tidak boleh menahan
input amount. Saat token atau pairing siap, engine mengirim keadaan pool dulu,
membaca saldo, lalu langsung mengirim saldo. Statistik berjalan sesudahnya dan
datang pada kiriman berikut. Saat statistik mengubah urutan pool, pemeriksaan
hasil async harus membandingkan himpunan pool, bukan urutan lamanya. Jika user
belum memilih pool lain selama pengukuran, pilihan aktif ikut pindah ke APR
marginal tertinggi yang lolos ambang kapasitas, lalu range lama dibuang sebelum
range pool baru dikirim.

## PnL perkiraan tetap mempunyai angka

Histori lama dapat memiliki seluruh komponen numerik tetapi cost basis-nya
ditandai rekonstruksi. Menyaring `net` dengan flag lengkap membuat fee, setoran,
nilai, gas, dan slippage terlihat sementara net dan persentasenya kosong.
Hitung net jika komponen angkanya tersedia, lalu gunakan tanda perkiraan untuk
menyatakan keterbatasan buktinya. Simbol histori yang kosong dapat diisi dari
metadata token on chain tanpa mengubah angka akuntansi.
