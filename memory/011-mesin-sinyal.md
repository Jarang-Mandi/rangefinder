# 011 Mesin sinyal, dan pembedaan yang berulang di seluruh modul

Ditulis 8 September 2026, saat fase 6.

## Yang dibangun

Detektor sideways bervolume tinggi, detektor dump lalu basis, dan doktrin
penempatan yang menjawab pertanyaan yang user sebut sebagai kesulitannya
sendiri: di mana garis harus ditaruh.

Seluruhnya murni. Masuk deret candle, keluar angka. Tidak menyentuh jaringan,
tidak menyimpan keadaan, 52 uji berjalan tanpa rantai. Itu disengaja: bagian
inilah yang menentukan di mana uang ditaruh.

Diuji lewat WebSocket terhadap engine yang jalan, dengan 300 bar sintetis:

```
sideways skor 99.7  er 0.00  containment 100%  kontraksi 0.41  LOLOS
saran dua_sisi: 99.7200 sampai 100.680, lebar 1.0%
konsentrasi 5.0x range lima kali lebih lebar
```

## Pembedaan yang berulang: kosong bukan skor rendah

Ini muncul di setiap modul dan layak diingat sebagai aturan tersendiri.

**Skor rendah** berarti sudah diperiksa dan tidak menarik.
**Kosong** berarti belum bisa diperiksa.

Menyamakan keduanya membuat token yang chartnya baru termuat terlihat seperti
token yang buruk, dan itu menyembunyikan kandidat yang justru sedang dicari.

Karena itu setiap metrik mengembalikan `None` kalau jendelanya lebih panjang
daripada data yang tersedia, bukan menghitung dari data yang dipendekkan diam
diam. Menghitung efficiency ratio atas sepuluh bar lalu menyebutnya empat jam
adalah kebohongan yang tidak akan pernah terlihat di layar.

Pengecualiannya satu, dan disengaja: detektor dump lalu basis tetap
mengembalikan hasil meski basisnya belum terbentuk. Token yang masih mencetak
low baru BISA dinilai, dan jawabannya adalah belum layak. User perlu melihat
itu supaya tidak masuk terlalu cepat.

## Lima koreksi terhadap rumus di PRD

Semuanya ditemukan saat menulis ujinya, bukan saat memakainya.

1. **Volume z score harus dijepit sebelum masuk skor.** Tanpa penjepitan, satu
   lonjakan ekstrem mendominasi seluruh skor sehingga suku lain tidak berarti,
   dan token yang sedang tren bisa lolos hanya karena volumenya meledak. Ada
   ujinya: tren dengan volume sejuta tetap harus ditolak.

2. **Bar terakhir tidak boleh ikut dalam rata rata pembandingnya sendiri.**
   Kalau ikut, lonjakan besar menaikkan rata ratanya sendiri, sehingga skornya
   lebih kecil daripada yang seharusnya, persis pada kejadian yang paling ingin
   ditangkap.

3. **Bar yang tutup sama dengan bukanya tidak membawa arah.** Versi pertama
   menganggapnya beli, dan akibatnya deret yang paling seimbang justru terbaca
   berat sebelah. Ini ketahuan karena uji keseimbangan tape gagal pada data
   yang jelas jelas seimbang.

4. **Efficiency ratio nol berarti chop sempurna, dan harga yang diam bukan
   itu.** Harga yang sama sekali tidak bergerak berarti datanya tidak berguna,
   jadi hasilnya kosong.

5. **ATR harus memakai close bar sebelumnya.** Mengabaikan celah antar bar
   meremehkan volatilitas persis saat harga melompat, yaitu saat ukuran itu
   paling dibutuhkan.

## Penjaga dead cat adalah bagian terpenting

Tanpa syarat kemiringan basis yang tidak menurun, detektor dump lalu basis akan
terus menyarankan LP di tengah tren turun. Itu cara tercepat kehilangan modal,
dan pola yang menyebabkannya terlihat sangat mirip dengan pola yang dicari.

Ujinya memakai dua deret yang sengaja mirip: dump lalu basis mendatar yang
ramai, dan pendarahan lambat yang terus mencetak low baru. Yang pertama wajib
lolos, yang kedua wajib ditolak.

## Setiap penolakan menyebut angkanya

Ada uji yang memastikan setiap alasan penolakan memuat digit. Detektor yang
hanya bilang "tidak cocok" tidak bisa dipercaya maupun diperbaiki, dan user
tidak belajar apa apa darinya.

Hal yang sama berlaku untuk saran penempatan: setiap saran wajib menjelaskan
dirinya dalam bentuk keputusan beli dan jual, dan ada uji yang memeriksa kata
"membeli" dan "menjual" benar benar muncul.

## Bobot berjumlah satu, dan ada ujinya

Kalau ada yang menambah suku baru ke skor, ia harus mengurangi yang lain. Uji
itu memaksa keputusan sadar alih alih penambahan diam diam yang menggeser
seluruh skala skor tanpa siapa pun sadar.

## Jangkar basis menang atas kanal

Kalau kedua detektor lolos, jangkar dari basis yang dipakai. Alasannya, ia
terbentuk dari perilaku sungguhan: penjual habis di satu harga dan pembeli
mempertahankannya. Kanal hanya ringkasan sebaran, dan sebaran tidak punya
ingatan tentang siapa yang membela harga berapa.

## Batas ditarik masuk, tidak ditaruh persis di struktur

Batas kanal adalah tempat harga berbalik. Dasar basis adalah tempat penjual
habis. Menaruh batas range persis di situ membuat posisi keluar range tepat
saat harga mengujinya, dan pengujian ulang adalah hal yang normal terjadi.

Kanal ditarik masuk sepersepuluh lebarnya, basis lima belas persen dari sisi
bawah, dan batas bawah tidak pernah melewati level pembatalan.
