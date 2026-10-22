# UI utama dan alur engine

## Permintaan yang menetap

Tampilan utama hanya memuat informasi yang membantu user mengambil tindakan.
Jangan memakai eyebrow. Jangan menampilkan liquidity mentah, tick mentah, gate,
simulasi, Permit2, approval, hash transaksi, gas, versi protokol, atau rincian
teknis lain di kartu setoran, Positions, dan PnL.

Logo token hanya boleh muncul jika ada aset logo yang nyata dan terverifikasi.
Jangan membuat logo pengganti dari huruf ticker dan jangan menambah permintaan
jaringan hanya demi logo. Sampai protokol membawa aset tersebut, tampilkan
ticker saja.

## Satu tombol berarti satu alur engine

Satu tombol di panel tidak boleh dicapai dengan melewati pemeriksaan. Setelah
user menekan tombol, engine menjalankan approval satu per satu, menunggu receipt,
menyusun ulang rencana dari state rantai, menjalankan zap bila perlu, menyusun
ulang lagi, lalu mengirim aksi akhir. Calldata, spender, dan jumlah izin tetap
diambil dari rencana yang tersimpan di engine.

Seluruh alur dikunci. Tanpa kunci tingkat alur, dua pesan WebSocket yang datang
hampir bersamaan dapat sama sama membaca approval pertama sebelum salah satunya
selesai, lalu mengirim transaksi yang sama dua kali. Kunci nonce saja tidak
mencegah transaksi ganda; ia hanya mencegah nonce ganda.

## Saldo harus mendahului statistik

Saat token atau pasangan dipilih, saldo adalah data pertama yang dibutuhkan
input amount. Kirim `token_state` begitu pembacaan saldo selesai. Pengukuran APR,
TVL, dan volume berjalan sesudahnya dan boleh menghasilkan pembaruan kedua.

Permintaan subscribe dapat terjadi sebelum pairing WebSocket siap. Panel harus
mengirim ulang subscribe tepat setelah menerima keadaan `connected`; kalau tidak,
saldo terlihat lambat atau tidak muncul sampai ada interaksi berikutnya.
