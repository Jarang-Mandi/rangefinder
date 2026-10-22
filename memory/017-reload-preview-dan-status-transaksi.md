# Reload chart, preview setoran, dan status transaksi

## State panel dan state halaman punya umur berbeda

Side panel Chrome dapat tetap hidup ketika halaman gmgn dimuat ulang. Token dan
`range_view` di panel masih sama, tetapi objek `createShape` TradingView sudah
hilang bersama dokumen lama. Mengabaikan pesan `token_page` untuk token yang
sama membuat garis tidak pernah pulih. Pesan itu datang sesudah bridge melihat
chart siap, sehingga untuk token yang sama ia harus diperlakukan sebagai sinyal
menggambar ulang range terakhir.

## Preview nominal tidak boleh menunggu planner transaksi

`quote_range` membaca ulang pool, saldo, allowance, kebijakan hook, simulasi,
gas, dan pembatas. Memakai jawabannya untuk mengisi kotak pasangan membuat input
terasa lambat walaupun engine berada di mesin yang sama. `preview_deposit`
sekarang hanya memakai snapshot pool, range, desimal, dan matematika likuiditas
yang sudah ada di memori. Preview membawa request ID supaya jawaban input lama
tidak menimpa angka baru. Ia tidak boleh dipakai untuk mengaktifkan transaksi;
otoritas eksekusi tetap hanya `open_plan`.

## Approval berurutan tidak membutuhkan replan per approval

Rencana awal sudah mengikat urutan, token, spender, dan jumlah setiap approval.
Setiap receipt approval juga diverifikasi sebelum flow maju. Menjalankan planner
penuh setelah tiap approval hanya mengulang RPC dan simulasi yang mahal. Hapus
langkah yang sudah sukses dari rencana tersimpan, selesaikan seluruh approval,
lalu susun ulang satu kali sebelum zap atau aksi akhir. Pemeriksaan akhir tetap
menggunakan state rantai terbaru.

## Pending adalah status, bukan kegagalan

Pada rantai berblok cepat, menahan tombol selama 60 detik membuat status sulit
dibaca. Jalur interaktif sekarang menunggu receipt 15 detik. Jika belum ada,
hasilnya `menggantung`, jurnal terus merekonsiliasi di latar, dan perubahan ke
sukses atau gagal memicu notifikasi dengan kode operasi yang sama. Jangan
mengirim ulang transaksi yang statusnya belum pasti.

## UI transaksi harus memakai satu tempat kabar

Open, tambah likuiditas, rebalance, klaim fee, dan close mempunyai pesan mulai,
berhasil, tertunda, dan gagal. Side panel menaruh pesan terakhir di baris kabar
yang sama, sedangkan content script menampilkan toast pada lokasi gmgn yang
sama dan membunyikan nada setelah browser mengizinkan audio. Rincian approval,
gate, dan simulasi tetap di engine serta jurnal.

Untuk rebalance, klaim fee, dan close, klik tombol aksinya sekaligus memberi
izin untuk menyusun dan menjalankan flow. Menampilkan lagi tombol konfirmasi
setelah planner selesai mengubah satu aksi menjadi dua klik dan membocorkan
langkah internal. Tambah likuiditas tetap punya tombol final karena jumlahnya
disunting dan rencananya disiapkan otomatis selama user mengetik.
