# Rangefinder

Product Requirements Document

| Field | Value |
| --- | --- |
| Versi | 0.1 (draft untuk review) |
| Tanggal | 5 September 2026 |
| Status | Implementasi aktif. Audit integrasi dan pekerjaan tertinggal dilacak di `docs/PERBAIKAN.md`. |
| Repo | `D:\Rangefinder` |

## Keputusan yang sudah terkunci

| Keputusan | Pilihan | Konsekuensi |
| --- | --- | --- |
| Custody | Hot wallet lokal terenkripsi di dalam engine Rust, dengan hard cap per transaksi dan per hari | Engine bisa mengeksekusi tanpa popup wallet. Semua pengaman risiko ada di sisi engine. |
| Protokol | Uniswap V3 dan V4 sekaligus | Cakupan pool paling luas. Wajib lapisan validasi berlapis karena permukaan bug dua kali lipat. |
| Otomasi | Semi otomatis: engine memantau, side panel menampilkan kartu aksi, user klik sekali | Tidak ada transaksi yang terjadi tanpa persetujuan eksplisit per aksi. |
| Chain | Robinhood Chain saja (chain id 4663) | Kode tetap memakai chain abstraction supaya penambahan chain lain nanti murah. |
| Wallet | Satu wallet saja | Tidak ada pemilih wallet di UI. Struktur data tetap menyimpan `wallet_address` supaya penambahan nanti tidak butuh migrasi. |
| Batas nominal | Profil modal di bawah 50 USD | Maksimal 5 USD per posisi dan per hari, dua posisi aktif, serta 2,50 USD untuk pool baru. Rincian di 9.2. |
| Posisi dari luar | Dikelola penuh | Posisi V3 dan V4 milik wallet yang dibuka di platform lain bisa direbalance, diklaim, dan ditutup dari Rangefinder. |
| Allowlist hook V4 | Berbasis adopsi nyata | Hook masuk allowlist otomatis jika lolos ambang pemakaian on chain. Kriteria di 5.4. |
| Notifikasi | Toast diinjeksi di pojok kanan atas halaman gmgn, dengan suara | Tidak bergantung pada side panel yang sedang terbuka. Spesifikasi di 3.7. |
| Penggambaran garis | Otomatis di setiap halaman token gmgn | Tidak perlu paste contract address dulu. Spesifikasi di 3.3. |

---

## 1. Executive Summary

### Problem Statement

Menempatkan concentrated liquidity di memecoin Robinhood Chain saat ini adalah proses buta. Trader melihat chart di gmgn.ai, lalu pindah ke antarmuka lain untuk menebak angka batas atas dan batas bawah, tanpa tahu berapa likuiditas yang sudah antre di range itu, berapa lama harga realistis bertahan di dalamnya, dan berapa jam fee yang dibutuhkan untuk menutup impermanent loss. Akibatnya posisi sering keluar range dalam hitungan menit dan LP kalah dari sekadar hold.

### Proposed Solution

Rangefinder adalah ekstensi Chrome side panel yang menggambar dua garis batas yang bisa digeser langsung di atas chart gmgn.ai, ditopang engine Rust lokal yang menghitung proyeksi fee, kepadatan likuiditas pesaing, dan probabilitas bertahan di range secara realtime, lalu mengeksekusi open, rebalance, claim, dan close di Uniswap V3 dan V4.

### Success Criteria

Diukur setelah 30 hari pemakaian nyata.

| KPI | Target | Cara ukur |
| --- | --- | --- |
| Latensi garis chart ke angka di panel | p95 di bawah 50 ms | Timestamp `drawing_event` sampai render angka di side panel |
| Latensi engine menghitung ulang proyeksi | p95 di bawah 150 ms | Timestamp request masuk sampai response WebSocket keluar |
| Akurasi proyeksi fee 1 jam | Median absolute percentage error di bawah 25 persen | Bandingkan proyeksi saat open dengan fee aktual pada jam pertama |
| Transaksi gagal atau revert | Di bawah 1 persen dari total transaksi terkirim | Counter di engine dibagi total submit |
| Median time in range | Naik minimal 40 persen dibanding baseline range manual | Log posisi sebelum dan sesudah pakai Range Optimizer |
| Posisi net positif setelah gas dan IL | Di atas 55 persen dari posisi tertutup | Tabel `positions`, kolom `net_pnl_usd` |

---

## 2. Temuan Riset yang Sudah Diverifikasi

Bagian ini bukan asumsi. Semua poin di bawah sudah dites langsung di browser terhadap gmgn.ai pada 5 September 2026, atau diambil dari dokumentasi resmi. Agent yang melanjutkan proyek ini boleh memakainya sebagai titik awal, tapi wajib verifikasi ulang jika gmgn mengganti bundle.

### 2.1 Chart gmgn.ai bisa diinjeksi

| Temuan | Hasil |
| --- | --- |
| Format URL token | `https://gmgn.ai/robinhood/token/<contract_address>` |
| Slug chain gmgn | `robinhood`. Slug lain di bundle: `sol`, `eth`, `bsc`, `base`, `tron`, `blast`, `monad`, `hyperevm`, `xlayer`, `arc` |
| Engine chart | TradingView Charting Library dalam iframe dengan id berpola `tradingview_<hash>` |
| Akses cross frame | Same origin, `iframe.contentWindow.document` bisa diakses penuh |
| Objek API | `iframe.contentWindow.tradingViewApi`, plus `TradingViewApi`, `chartWidget`, `chartWidgetCollection` |
| Method chart tersedia | `createShape`, `createMultipointShape`, `createPositionLine`, `createOrderLine`, `getAllShapes`, `getShapeById`, `removeEntity`, `getVisiblePriceRange`, `getVisibleRange`, `onDataLoaded`, `crossHairMoved`, `dataReady`, `priceFormatter`, `getSeries`, `executeActionById`, `getPanes` |
| Method level API | `subscribe`, `unsubscribe`, `chart`, `activeChart` |
| Nilai balik `createShape` | Promise yang resolve ke entity id berupa string pendek, contoh `jnm56k` |
| Properti shape | `linecolor`, `textcolor`, `linewidth`, `linestyle`, `showPrice`, `fontsize`, `bold`, `italic`, `horzLabelsAlign`, `vertLabelsAlign`, `text`, `frozen`, `visible` |
| Baca harga garis | `getShapeById(id).getPoints()[0].price` |
| Format simbol | `robinhood/<address>/USD/MCAP`, resolution `"1"` untuk 1 menit |

Konsekuensi penting: simbol default memakai satuan **market cap dalam USD**, sedangkan range LP di Uniswap adalah **harga token1 per token0**, yaitu WETH per token. Setiap nilai garis wajib melewati dua konversi sebelum jadi tick, dan sebaliknya saat menggambar. Dijabarkan di bagian 4.3.

### 2.2 API internal gmgn bisa dipakai dari konteks halaman

Request dari dalam halaman gmgn.ai berhasil dengan status 200 dan `code: 0`. Endpoint terkonfirmasi:

```
/defi/quotation/v1/rank/robinhood/swaps/{period}    orderby, direction, limit
/api/v1/mutil_window_token_info                     POST, dipakai halaman token
/api/v1/token_info_brief_multi_chain
/api/v1/token_holder_counts
/api/v1/token_prices
/api/v1/token_fee_info_polling
/api/v1/dex_trades_polling
/api/v1/major_coin_prices
/api/v1/gas_price_list
/api/v1/follow/follow_wallet_list
/api/v1/person_wallets_pnl_info
wss://ws.gmgn.ai                                    stream realtime
```

Field yang dikembalikan endpoint rank, semuanya relevan untuk screening:

```
address, name, symbol, price, price_change_percent1m/5m/1h, volume, liquidity,
market_cap, total_supply, initial_liquidity, history_highest_market_cap,
swaps, buys, sells, holder_count, top_10_holder_rate, open_timestamp,
creation_timestamp, pool_type, pool_type_str, launchpad, launchpad_platform,
launchpad_status, exchange, twitter_username, website, telegram,
renounced_mint, renounced_freeze_account, burn_ratio, burn_status,
dev_token_burn_amount, dev_token_burn_ratio, creator, creator_token_status,
creator_close, fund_from_address, hot_level, is_show_alert
```

Request gmgn membawa parameter fingerprint: `device_id`, `tab_id`, `fp_did`, `client_id`, `from_app`, `app_ver`, `tz_name`, `tz_offset`, `app_lang`, `os`. Panggilan dari engine Rust langsung ke gmgn akan rapuh dan gampang kena rate limit atau blokir.

**Keputusan desain:** semua panggilan API gmgn dilakukan dari content script di dalam tab gmgn milik user, lalu hasilnya diteruskan ke engine. Engine tidak pernah memanggil gmgn sendiri.

### 2.3 Infrastruktur Robinhood Chain

| Item | Nilai |
| --- | --- |
| Chain id | 4663 (`0x1237`) |
| Jenis | Arbitrum Orbit L2 |
| Gas token | ETH, 18 desimal |
| Block time | Sekitar 100 ms, sequencer first come first served, tanpa lelang priority fee |
| RPC publik | `https://rpc.mainnet.chain.robinhood.com` (rate limited) |
| Explorer | `https://robinhoodchain.blockscout.com` |
| WETH | `0x0Bd7D308f8E1639FAb988df18A8011f41EAcAD73` (aeWETH proxy, kompatibel WETH9) |
| USDG | `0x5fc5360D0400a0Fd4f2af552ADD042D716F1d168`, **6 desimal**. Token kutipan yang paling banyak dipakai di rantai ini |
| Permit2 | `0x000000000022D473030F116dDEE9F6B43aC78BA3` |
| Multicall3 | `0xcA11bde05977b3631167028862bE2a173976CA11` |

Uniswap V3:

| Kontrak | Alamat |
| --- | --- |
| UniswapV3Factory | `0x1f7d7550b1b028f7571e69a784071f0205fd2efa` |
| NonfungiblePositionManager | `0x73991a25c818bf1f1128deaab1492d45638de0d3` |
| SwapRouter02 | `0xcaf681a66d020601342297493863e78c959e5cb2` |
| QuoterV2 | `0x33e885ed0ec9bf04ecfb19341582aadcb4c8a9e7` |
| TickLens | `0x7dfd4f31be6814d2906bde155c3e1b146eac1468` |
| UniswapInterfaceMulticall | `0x282a3c4d320cc7f0d5eaf56b8029e4b88338f0a3` |

Uniswap V4:

| Kontrak | Alamat |
| --- | --- |
| PoolManager | `0x8366a39CC670B4001A1121B8F6A443A643e40951` |
| PositionManager | `0x58daec3116aae6D93017bAAea7749052E8a04fA7` |
| V4Quoter | `0x8Dc178eFB8111BB0973Dd9d722ebeFF267c98F94` |
| StateView | `0xF3334192D15450CdD385c8B70e03f9A6bD9E673b` |
| UniversalRouter | `0x8876789976dEcBfCbBbe364623C63652db8C0904` |

Ekosistem pihak ketiga yang sering muncul di chain ini:

| Kontrak | Alamat |
| --- | --- |
| BagsFactory | `0xe8Cc4431adF8b5A847C113EF0c6af9043219Cb37` |
| BagsLens | `0xC82Db941dAf90B754aecb5F7D14c683dc608d595` |
| BagsV4Hook | `0x2380aBf72C17aABAb76480244759AC7E2932EEcC` |
| BagsVault | `0x4861446aa7fFd9e67a83cBbAcb1A4B70540B83Aa` |

**Peringatan kritis satu.** UniversalRouter di Robinhood Chain adalah fork yang dimodifikasi. Struct swap V4 membawa field tambahan `minHopPriceX36`. Calldata hasil SDK Uniswap standar akan gagal. Semua encoding router wajib dibangun manual dan diverifikasi dengan golden vector.

**Peringatan kritis dua.** Banyak pool V4 di chain ini memakai hook, termasuk BagsV4Hook. Hook bisa mengubah fee, membatasi siapa yang boleh menambah likuiditas, atau memungut potongan. Engine wajib membaca alamat hook dari PoolKey dan menolak pool dengan hook yang belum masuk allowlist.

**Peringatan tiga.** gmgn menampilkan banner "Robinhood Chain is unstable, data and trades may be delayed". Engine tidak boleh mempercayai satu sumber harga. Lihat bagian 7.5.

### 2.4 Referensi mekanisme LPZenith

LPZenith adalah bot Telegram, bukan ekstensi. Alur inti yang kita adopsi: paste contract address, pilih pool dan price range, deposit satu token dengan zap otomatis, terima update PnL berkala. Detail yang layak ditiru:

- Zap satu sisi. User cukup punya satu token dan sistem menyeimbangkan sendiri.
- Menolak posisi jika grid tick tidak bisa menempatkan range dalam 1 persen dari harga live, lalu menawarkan pool dengan fee tier lebih halus.
- Zap bersifat atomik. Seluruh dana dikembalikan jika ada satu langkah gagal.
- Range ditampilkan dalam satuan harga dan market cap sekaligus.
- Mendeteksi posisi V3 dan V4 yang sudah ada dari platform lain pada wallet yang sama.
- Notifikasi otomatis saat posisi keluar range.

Yang tidak ditiru: model Telegram, biaya per open dan close, dan custody terpusat.

### 2.5 Referensi strategi sinyal

Playbook dari utas rektfencer, diterjemahkan jadi filter yang bisa dieksekusi:

1. Cari runner di GMGN dengan chain Robinhood, pantau New Pairs, Trending, dan Hot Searches. Untuk nama mapan: market cap di bawah 25 juta dolar, umur 7 hari ke atas, urutkan berdasarkan volume, incar sekunder di 5 sampai 10 juta. Untuk yang baru: market cap 500 ribu ke atas, umur di bawah 7 hari, incar entry 1 sampai 2 juta. Selalu cek bundle, sniper, dan flag rug.
2. Cek holder. Ambil runner 2 sampai 3 hari terakhir, buka Holders, cari wallet yang masuk sangat awal, lalu saring. Wallet mati dilewati. Beli lalu buang dalam 2 menit dilewati. Menyemprot semua launch dilewati. Yang selektif dan awal diikuti.
3. Berhenti membeli lilin hijau. Buru nama berumur 1 sampai 4 hari yang sudah lari lalu jatuh tapi komunitasnya masih hidup di X. Beli penurunan, bukan pump yang sudah dilihat semua orang.

Tambahan wajib, dan ini justru inti nilai produk: **deteksi koin bervolume besar yang akan bergerak menyamping**. Untuk trader spot, sideways adalah kondisi terburuk. Untuk LP, sideways bervolume tinggi adalah kondisi terbaik yang ada, karena fee terus mengalir sementara impermanent loss mendekati nol. Detektor ini ada di bagian 5.2 dan menjadi pembeda utama Rangefinder.

---

## 3. User Experience

### 3.1 Persona

**Operator LP memecoin.** Menjalankan modal sendiri, sudah terbiasa dengan gmgn.ai sebagai terminal utama, membuka 5 sampai 20 token per hari. Bukan developer. Ingin keputusan range yang berdasar angka, bukan feeling, dan ingin tahu kapan harus keluar sebelum posisinya membusuk. Tidak mau pindah tab.

### 3.2 Struktur navigasi

Side panel punya empat tab. Pemisahan ini diminta eksplisit dan tidak digabung.

```
+---------------------------------+
|  Range  Signals  Positions  PnL |
+---------------------------------+
|                                 |
|         konten tab aktif        |
|                                 |
+---------------------------------+
|  status engine, wallet, gas     |
+---------------------------------+
```

### 3.3 Tab Range

**Story.** Sebagai operator LP, saya ingin paste contract address lalu langsung melihat batas atas dan batas bawah di chart yang bisa saya geser, supaya saya menentukan range sambil melihat struktur harga yang sebenarnya.

**Dua jalan masuk, keduanya wajib.**

Jalan pertama, user paste contract address ke input di puncak tab Range.

Jalan kedua, dan ini yang lebih sering terjadi dalam pemakaian nyata: user sendiri yang membuka halaman token mana pun di gmgn, entah dari Trending, dari hasil pencarian, atau dari tautan orang lain. Begitu content script mendeteksi halaman token, Rangefinder **langsung menggambar garis range yang disarankan dan menampilkan kartu aksi tanpa diminta**. User tidak perlu menyalin alamat lalu menempelkannya kembali. Ini berlaku untuk setiap pair yang dibuka, termasuk pair yang belum pernah dilihat sebelumnya.

**Alur.**

1. Content script mendeteksi navigasi ke pola `https://gmgn.ai/robinhood/token/<CA>`, baik karena user membuka sendiri maupun karena hasil paste di panel. Deteksi memakai History API dan `MutationObserver`, karena gmgn adalah aplikasi Next.js satu halaman yang tidak memuat ulang dokumen saat berpindah token.
2. Jika alurnya berasal dari paste, ekstensi menavigasi tab gmgn yang sudah terbuka ke alamat itu. Jika tidak ada tab gmgn sama sekali, ekstensi membuka satu tab baru.
3. Content script menunggu chart siap, lalu menggambar dua garis horizontal: batas atas hijau, batas bawah merah, sesuai gambar referensi. Posisi awal kedua garis berasal dari doktrin penempatan di 5.7, bukan dari lebar tetap.
4. Engine menarik daftar pool untuk pair itu di V3 dan V4, mengurutkan berdasarkan proyeksi fee, lalu mengisi pool picker.
5. Engine menjalankan Range Optimizer dan menempatkan kedua garis di lebar yang disarankan.
6. User menggeser garis. Setiap `drawing_event` memicu perhitungan ulang.
7. User mengisi ukuran modal, melihat kartu proyeksi, lalu klik satu tombol untuk membuka posisi.

Saat dokumen gmgn dimuat ulang, side panel dapat tetap hidup sementara seluruh
objek garis di TradingView hilang. Pesan kesiapan chart untuk token yang sama
karena itu selalu memicu penggambaran ulang dari `range_view` terakhir.

**Acceptance criteria.**

- Paste alamat valid menavigasi tab gmgn dalam waktu di bawah 1 detik.
- Alamat tidak valid, bukan checksum EVM, atau token tanpa pool ditolak dengan pesan spesifik, bukan pesan umum.
- Dua garis muncul dengan warna hijau untuk atas dan merah untuk bawah, ketebalan 2, label harga aktif.
- Memuat ulang halaman token yang sama menggambar kembali kedua garis tanpa perlu mengganti token atau range.
- Menggeser garis memperbarui angka di panel dalam p95 di bawah 50 ms tanpa flicker.
- Garis atas tidak bisa turun melewati garis bawah dan sebaliknya. Jika user memaksa, keduanya bertukar peran dan warnanya ikut bertukar.
- Panel menampilkan nilai range dalam tiga satuan sekaligus: harga USD, market cap, dan tick index Uniswap.
- Tick yang ditampilkan sudah dibulatkan ke `tickSpacing` pool terpilih, dan panel menunjukkan selisih akibat pembulatan itu.
- Kartu proyeksi menampilkan estimasi fee APR, estimasi waktu bertahan di range, jam yang dibutuhkan untuk menutup IL, dan kepadatan likuiditas pesaing di dalam range.
- Mengganti pool di picker menggambar ulang garis ke tick grid pool baru.
- Menutup tab gmgn atau berpindah token membersihkan semua shape milik Rangefinder dan tidak menyentuh shape milik user.
- Membuka halaman token mana pun di gmgn secara langsung, tanpa melalui input paste, tetap menghasilkan garis dan kartu aksi dalam waktu di bawah 2 detik setelah chart siap.
- Kartu aksi selalu menyatakan satu rekomendasi tegas beserta alasannya dalam satu kalimat, termasuk saat rekomendasinya adalah tidak membuka posisi.

### 3.4 Tab Signals

**Story.** Sebagai operator LP, saya ingin daftar koin bervolume besar yang kemungkinan besar akan menyamping, supaya saya menempatkan LP di kondisi yang paling menguntungkan.

**Alur.** Engine memindai secara berkala melalui content script gmgn, memberi skor, lalu menampilkan daftar terurut. Klik satu baris menavigasi tab gmgn ke token itu, memuat chart, dan menampilkan kartu aksi yang menyarankan tipe posisi beserta alasannya.

**Acceptance criteria.**

- Daftar menampilkan minimal simbol, market cap, volume 1 jam, likuiditas, umur, skor Sideways, proyeksi fee APR, dan flag risiko.
- Setiap baris punya satu label aksi yang tegas: `LP Sideways`, `LP Beli Turun`, `LP Jual Naik`, `Pantau`, atau `Hindari`.
- Klik baris menavigasi gmgn dan mengaktifkan tab Range dengan pool dan range terisi otomatis.
- Filter mengikuti playbook di 2.5, semuanya bisa diubah user dan tersimpan antar sesi.
- Token yang gagal uji jalur transfer di 5.4 ditandai `Hindari` dan tidak bisa diklik untuk LP, dengan alasan tertulis. Token yang berpajak bagi pembeli biasa tapi mengecualikan jalur LP tetap boleh muncul, dengan penanda bahwa pajaknya sudah diuji dan tidak mengenai jalur yang kita pakai.
- Pemindaian tidak melebihi 1 request gmgn per 2 detik agar sesi user tidak kena rate limit.

### 3.5 Tab Positions

**Story.** Sebagai operator LP, saya ingin melihat semua posisi yang sedang berjalan dan tahu persis aksi apa yang harus saya ambil sekarang.

**Acceptance criteria.**

- Menampilkan semua posisi V3 dan V4 milik wallet, termasuk posisi yang dibuka di luar Rangefinder, dan **mengelolanya penuh**. Posisi dari platform lain punya set aksi yang sama persis: tambah, rebalance, klaim, tutup. Satu satunya pembatas adalah posisi pada pool dengan hook di luar allowlist, yang hanya boleh ditutup, tidak boleh ditambah.
- Posisi dari luar yang tidak punya catatan cost basis di basis data lokal direkonstruksi dari log on chain, yaitu event `IncreaseLiquidity`, `DecreaseLiquidity`, dan `Collect`, sehingga PnL-nya tetap terhitung. Jika rekonstruksi tidak lengkap, kartu menandainya sebagai perkiraan, bukan menyembunyikannya.
- Setiap kartu menampilkan pair, range, harga sekarang relatif range, status in range atau out of range, jumlah token, fee terkumpul yang belum diklaim, dan aksi utama. Versi protokol, fee tier, tick mentah, liquidity mentah, serta rincian implementasi tidak tampil di kartu.
- Indikator posisi harga di dalam range berupa bar horizontal solid, bukan gradient.
- Klik kartu membuka aksi Tambah likuiditas, Rebalance ke range baru, Klaim fee, dan Tutup posisi.
- Engine menjalankan simulasi dan seluruh gate sebelum transaksi, tetapi panel hanya menampilkan satu tombol aksi. Rebalance, klaim fee, dan tutup posisi mulai dari satu klik pada tombolnya; planner meneruskan hasil yang siap tanpa meminta klik kedua. Tambah likuiditas menyiapkan rencana selama user mengisi jumlah, lalu memakai satu tombol final. Approval dan penyusunan ulang berjalan berurutan di belakang layar setelah tombol ditekan.
- Setelah close sukses, engine otomatis menjual token biasa yang baru diterima ke ETH. ETH asli, WETH, dan USDG dipertahankan. Pada pasangan dua token biasa, kedua sisi dicoba secara terpisah. Jumlah swap hanya delta hasil close, bukan saldo lama di wallet. Kegagalan konversi dilaporkan sebagai peringatan tanpa mengubah close yang sudah sukses menjadi gagal.
- Rute konversi otomatis hanya memakai pool V4 langsung ke ETH tanpa hook, dipilih dari kutipan keluaran terbaik untuk jumlah aktual dan tetap melalui batas slippage serta simulasi calldata.
- Tombol aksi selalu berada di tempatnya sejak kartu tampil. Selama rencana disusun tombol tetap terlihat dalam keadaan nonaktif, sehingga tata letak tidak melompat.
- Posisi yang keluar range lebih dari ambang waktu yang diset user naik ke puncak daftar dan ditandai.
- Data posisi diperbarui melalui WebSocket dari engine, tanpa polling dari sisi panel.

### 3.6 Tab PnL

**Story.** Sebagai operator LP, saya ingin kartu hasil untuk posisi yang selesai maupun yang masih jalan, supaya saya tahu strategi mana yang benar benar menghasilkan.

**Acceptance criteria.**

- Engine tetap menghitung fee, impermanent loss, gas, slippage zap, dan net akhir secara terpisah. Panel mengutamakan net dan fee yang diperoleh; rincian akuntansi teknis tetap berada di engine dan jurnal.
- Ada ringkasan agregat: total net, win rate, median time in range, dan fee APR terealisasi.
- Ringkasan memuat seluruh posisi aktif dan tertutup tanpa filter versi teknis di tampilan utama.
- Setiap kartu memuat pair, waktu buka, durasi atau waktu tutup, nilai yang disetor, nilai sekarang atau diterima, fee, PnL bersih, dan persentase PnL. Tidak ada eyebrow, versi protokol, tick, atau keterangan implementasi.
- Net yang komponennya tersedia tetap ditampilkan dengan tanda perkiraan ketika cost basis historis belum lengkap. Angka tidak disembunyikan hanya karena statusnya perkiraan.
- Kartu bisa diekspor jadi PNG.
- Angka net PnL memakai basis yang bisa dipilih user antara USD atau ETH.

### 3.7 Notifikasi

**Story.** Sebagai operator LP, saya ingin diberi tahu setiap ada kejadian penting pada posisi saya, tanpa harus menatap side panel terus menerus.

**Tempat munculnya.** Notifikasi utama adalah toast yang diinjeksi content script ke **pojok kanan atas halaman gmgn**. Ini pilihan yang disengaja: side panel hanya hidup saat dibuka, sedangkan halaman gmgn hampir selalu terbuka saat user bekerja. Toast menumpuk ke bawah, maksimal empat sekaligus, sisanya masuk antrean.

Jika tidak ada tab gmgn yang terbuka, kejadian tetap tersimpan dalam riwayat
engine dan muncul di baris kabar side panel ketika panel tersambung.

**Tingkat dan perilaku.**

| Tingkat | Contoh kejadian | Perilaku toast | Suara |
| --- | --- | --- | --- |
| Info | Fee terkumpul melewati ambang klaim, sinyal baru berskor tinggi, posisi kembali masuk range | Hilang sendiri setelah 6 detik | Nada pendek satu ketukan, pelan |
| Peringatan | Harga mendekati batas range, dilusi likuiditas melewati ambang, aturan keluar berbasis imbal hasil terpicu | Hilang sendiri setelah 12 detik | Nada dua ketukan |
| Kritis | Posisi keluar range, transaksi gagal atau revert, data harga tidak sehat, RPC mati, verifikasi pasca kirim tidak cocok | Menetap sampai diklik | Nada tiga ketukan, lebih tegas |

**Daftar kejadian yang wajib memicu notifikasi.**

```
posisi dibuka berhasil            posisi dibuka gagal
rebalance mulai/berhasil/gagal     klaim fee mulai/berhasil/gagal
tambah likuiditas mulai/selesai    penutupan mulai/berhasil/gagal
transaksi terkirim                transaksi terkonfirmasi
transaksi gagal atau revert       verifikasi pasca kirim tidak cocok
harga mendekati batas range       posisi keluar range
posisi kembali masuk range        fee melewati ambang klaim
dilusi likuiditas melewati ambang aturan keluar imbal hasil terpicu
sinyal baru berskor tinggi        wallet terkunci otomatis
data harga tidak sehat            RPC gagal dan gagal failover
```

**Catatan teknis yang tidak boleh dilewatkan.**

1. Service worker MV3 tidak bisa memutar audio. Pemutaran suara wajib lewat `chrome.offscreen` dengan alasan `AUDIO_PLAYBACK`, atau dari content script di halaman gmgn.
2. Chrome memblokir autoplay audio sampai ada interaksi user pada halaman itu. Content script mengaktifkan `AudioContext` pada interaksi pertama. Sebelum itu notifikasi tetap muncul tanpa suara.
3. Nada dibangkitkan dengan oscillator lokal, bukan dimuat dari jaringan, sehingga bunyi pertama tidak menunggu berkas.

**Acceptance criteria.**

- Toast muncul di pojok kanan atas halaman gmgn dalam waktu di bawah 300 ms setelah engine memancarkan kejadian.
- Toast tidak pernah menutupi harga terakhir atau tombol beli jual milik gmgn.
- Setiap toast punya judul, satu baris keterangan, dan aksi lanjutan jika relevan, misalnya Buka posisi atau Rebalance sekarang.
- Suara bisa dimatikan seluruhnya, atau dimatikan per tingkat.
- Kejadian yang sama untuk posisi yang sama tidak diulang dalam periode pendinginan yang bisa diatur, bawaan 5 menit, supaya harga yang bergetar di sekitar batas tidak membanjiri layar.
- Riwayat 50 notifikasi terakhir tetap tersedia dari engine untuk audit, sedangkan tampilan utama hanya menunjukkan kabar terbaru.

### 3.8 Non Goals

- Bukan pengganti terminal trading. Tidak ada beli jual spot di luar swap yang dibutuhkan zap.
- Tidak ada chart sendiri di dalam side panel pada fase awal. Chart tetap milik gmgn.
- Tidak mendukung chain selain Robinhood Chain di v1.
- Tidak ada akun, server jarak jauh, atau sinkronisasi cloud. Semuanya lokal.
- Tidak ada eksekusi otomatis penuh tanpa klik user di v1.
- Tidak ada dukungan Uniswap V2 atau Rialto di v1.
- Tidak ada fitur sosial, leaderboard, atau referral.

---

## 4. Spesifikasi Injeksi Chart

Ini komponen dengan risiko teknis tertinggi sekaligus pembeda utama produk. Semua yang ada di sini sudah diuji langsung, bukan rancangan di atas kertas.

### 4.1 Siklus hidup content script

Content script didaftarkan pada `https://gmgn.ai/*` dengan `world: "MAIN"` di manifest MV3. Dunia MAIN wajib, karena `tradingViewApi` adalah variabel milik halaman dan tidak terlihat dari isolated world.

```
document_idle
  -> tunggu iframe dengan id berpola /^tradingview_/ muncul (MutationObserver, timeout 15 detik)
  -> tunggu iframe.contentWindow.tradingViewApi terisi (poll 50 ms, timeout 15 detik)
  -> tunggu activeChart().dataReady(cb) memanggil balik
  -> cocokkan alamat pada activeChart().symbol() dengan token URL
  -> probe kapabilitas, kirim laporan ke side panel
  -> gambar garis
  -> subscribe('drawing_event')
```

Probe kapabilitas memeriksa keberadaan setiap method yang dipakai, lalu mengirim hasilnya ke panel sebagai status chip. Jika satu method hilang karena gmgn mengganti versi Charting Library, panel menampilkan mode degradasi yang aktif, bukan gagal diam diam.

Jika batas tunggu habis, bridge melaporkan tingkat 3 lalu mencoba attach lagi
dengan satu timer selama token dan generasinya belum berubah. Ini diperlukan
karena profil browser baru pernah menerima `tradingViewApi` lebih dari 15 detik
sesudah `document_idle`. Pada navigasi SPA, GMGN juga mengganti URL sebelum API
chart berpindah. Bridge tidak mengirim `token_page`, harga, atau candle sampai
alamat pada simbol chart cocok dengan URL, sehingga data token lama tidak bisa
diberi label token baru.

### 4.2 Menggambar dan membaca garis

Pembuatan garis, warna mengikuti gambar referensi:

```js
const chart = iframe.contentWindow.tradingViewApi.activeChart();
const t = chart.getVisibleRange().to;

const upperId = await chart.createShape(
  { time: t, price: upperPrice },
  { shape: 'horizontal_line',
    overrides: { linecolor: '#2E9E63', linewidth: 2, showPrice: true, textcolor: '#2E9E63' } }
);

const lowerId = await chart.createShape(
  { time: t, price: lowerPrice },
  { shape: 'horizontal_line',
    overrides: { linecolor: '#D8484D', linewidth: 2, showPrice: true, textcolor: '#D8484D' } }
);
```

Catatan implementasi yang menghemat waktu debugging:

- `createShape` mengembalikan Promise. Menyimpan hasilnya langsung tanpa `await` akan membuat `getShapeById` melempar `There is no such shape`. Ini sudah terjadi saat riset.
- Entity id berupa string pendek acak, contoh `jnm56k`. Simpan di state, jangan mengandalkan urutan `getAllShapes()`.
- `getAllShapes()` mengembalikan seluruh shape, termasuk milik user. Jangan pernah menghapus massal. Hanya hapus id yang tercatat milik Rangefinder.

Membaca posisi garis saat digeser:

```js
tradingViewApi.subscribe('drawing_event', (id, type) => {
  if (id !== upperId && id !== lowerId) return;
  if (type === 'remove') { handleUserDeletedLine(id); return; }
  const price = chart.getShapeById(id).getPoints()[0].price;
  postToPanel({ id, price });
});
```

Event drag terbit dengan frekuensi tinggi. Panel melakukan throttle pada 30 fps untuk render, tapi perhitungan berat di engine hanya dipicu setelah 120 ms tanpa event baru, supaya engine tidak dibanjiri.

### 4.3 Konversi satuan

Ini sumber bug paling halus di seluruh proyek. Chart bekerja dalam market cap USD, Uniswap bekerja dalam tick. Rantai konversinya:

```
mcap_usd  ->  price_usd  ->  price_quote  ->  raw_price  ->  tick
```

`price_quote` adalah harga dalam token kutipan pool yang sedang dipilih, dan token itu **tidak selalu WETH**. Lihat 4.3b.

1. `price_usd = mcap_usd / total_supply`. Ambil `total_supply` dari endpoint rank gmgn, verifikasi silang dengan `totalSupply()` on chain. Jika beda lebih dari 0,5 persen, pakai nilai on chain dan tandai.
2. `price_quote = price_usd / quote_usd`. Untuk kutipan WETH, `quote_usd` adalah kurs ETH yang dikalibrasi dari chart. Untuk kutipan stablecoin seperti USDG, `quote_usd` bernilai 1 dan langkah ini tidak menambah error sama sekali, yang membuat pool berkutipan stablecoin lebih akurat sekaligus lebih murah untuk dihitung.
3. Tentukan urutan token. `token0` adalah alamat yang lebih kecil secara numerik. Jika memecoin adalah token0, maka harga pool adalah kutipan per memecoin. Jika terbalik, semua nilai harus diinversi sebelum masuk rumus tick.
4. `raw_price = price_quote * 10^(dec1 - dec0)`. Desimal kutipan wajib diambil dari token kutipan yang sebenarnya. USDG berdesimal 6, dan menganggapnya 18 menggeser seluruh harga sebesar 10^12.
5. `tick = round( ln(raw_price) / ln(1.0001) )`.
6. Snap ke grid: `tickLower = floor(tick / tickSpacing) * tickSpacing`, `tickUpper = ceil(tick / tickSpacing) * tickSpacing`.

Tick spacing V3 per fee tier: 100 memakai 1, 500 memakai 10, 3000 memakai 60, 10000 memakai 200. Di V4, `tickSpacing` adalah bagian dari PoolKey dan bisa bernilai apa saja, jadi wajib dibaca dari pool, bukan diasumsikan.

Aturan tampilan: panel selalu menunjukkan harga hasil pembulatan tick, bukan harga mentah yang digeser user, supaya angka di panel sama persis dengan yang akan tercatat on chain. Selisih pembulatan ditampilkan dalam basis poin.

Chart gmgn punya toggle `Price / MC` dan `USD / ETH`, dan toggle skala `log`. Content script membaca simbol aktif melalui `activeChart().symbol()` untuk mendeteksi mode, lalu menyesuaikan konversi. Perubahan mode oleh user memicu penggambaran ulang garis.

### 4.3b Pencarian pool tanpa asumsi pasangan

Engine tidak menebak dengan token apa sebuah memecoin berpasangan. Ia memindai seluruh event `Initialize` PoolManager yang menyentuh alamat token itu, lalu mengelompokkan hasilnya per token lawan menjadi `QuoteGroup`. Kelompok dengan likuiditas layak terbesar dipilih otomatis, dan panel menampilkan pilihan itu beserta kelompok lain yang tersedia.

Aturan ini lahir dari kasus nyata. Pada FIG di `0xdc4Dd8839F02F2242E9552fe4652635fB7F95cC6`, dari 39 pool V4 yang ada, 33 berpasangan dengan USDG, 5 dengan ETH asli, dan **nol** dengan WETH. Pencarian yang mengandaikan WETH melaporkan token itu tidak punya pool sama sekali, padahal pasarnya ramai.

Tiga hal yang mengikuti dari situ:

1. **ETH asli ikut dicari.** Di V4, currency `0x0000...0000` berarti ETH asli, bukan alamat kosong, dan itu tidak akan pernah cocok dengan pencarian berbasis WETH.
2. **Simbol bukan identitas.** Token lain yang juga bernama FIG ada di `0x41F4267525a8AFf329540eF24fD83d9044758B33` dan punya pool terhadap FIG yang asli. Yang mengikat posisi hanya alamat.
3. **Pemindaian log berjalan mundur** dari blok terbaru dengan potongan adaptif, dan kegagalan RPC dikembalikan sebagai error, tidak pernah dilewati diam diam. Memindai maju dari blok deploy berarti puluhan juta blok dan langsung kena rate limit.

**Penyaringan fee di luar akal sehat.** Pool dengan fee statis nol atau di atas `MAX_PLAUSIBLE_FEE` yaitu 10 persen **dibuang**, bukan diturunkan peringkatnya. Alasannya terlihat pada token yang sama:

| Fee | Likuiditas | Kenapa berbahaya |
| --- | --- | --- |
| 99,08 sampai 79,06 persen | sampai 12,58e18 | Tidak ada yang menukar di sana, jadi pendapatan nol sambil skor `likuiditas x fee` memenangkannya |
| 20,00 persen | 4,03e19 | Sama, hanya lebih halus |
| 0,00 persen | 2,82e23 | Likuiditas terbesar di seluruh token, 1.286 kali pool teratas yang layak. Menang pada pemeringkatan berbasis likuiditas mana pun, dan membayar LP nol sambil tetap menanggung impermanent loss penuh |

Pool dynamic fee dikecualikan dari aturan ini, karena angka feenya adalah penanda `0x800000`, bukan besaran.

Perintah `cargo xtask pairs <alamat>` menampilkan seluruh rincian ini, termasuk apa yang dibuang beserta alasannya. Penyaringan yang tidak terlihat adalah persis bagaimana bug pasangan WETH bisa bertahan tanpa ketahuan.

### 4.3c Kenapa pool muncul seketika

Zenith LP menampilkan puluhan pool sebuah token tanpa jeda, sedangkan versi pertama Rangefinder butuh 38 detik untuk token yang sama. Selisihnya bukan pada kecerdasan melainkan pada jumlah perjalanan bolak balik ke RPC. Perubahan berikut menutup jarak itu, dan semuanya diukur, bukan ditebak.

**Satu, gabungkan panggilan lewat Multicall3.** Membaca `getSlot0` dan `getLiquidity` untuk 39 pool berarti 78 permintaan berurutan. Pada RPC publik dengan latensi sekitar 350 ms itu 25 detik. Multicall3 mengeksekusi seluruhnya dalam satu `eth_call`, dan hasilnya 380 ms. Metadata token, pencarian pool V3, dan pembacaan pool V4 sekarang berada dalam satu batch yang sama.

**Dua, jangan memecah pemindaian log tanpa alasan.** Nilai awal 500.000 blok per permintaan dipilih karena takut ditolak. Pengukuran langsung menunjukkan batas sebenarnya jauh lebih longgar:

| Rentang satu permintaan | Hasil |
| --- | --- |
| 3 juta blok | berhasil, 1016 ms |
| 10 juta blok | `log query timed out` |
| 48 juta blok | `log query timed out` |

Seluruh jendela mundur muat dalam satu permintaan, jadi memecahnya hanya menambah latensi sekaligus memancing rate limit.

**Tiga, hormati batas laju alih alih melawannya.** Dua arah pemindaian sempat dijalankan berbarengan supaya lebih cepat, dan hasilnya justru lebih lambat:

| Cara | Hasil |
| --- | --- |
| berurutan | 1102 ms, keduanya berhasil |
| berbarengan | yang lolos 658 ms, yang satunya kena 429 lalu diulang, total sekitar 5 detik |

Rantai ini memberi satu permintaan berat pada satu waktu.

**Empat, ingat kunci pool.** PoolKey tidak pernah berubah setelah pool dibuat. Yang berubah hanya harga dan likuiditas. Jadi kunci pool disimpan di redb beserta blok terakhir yang sudah dipindai, dan pemuatan berikutnya cukup membaca keadaan, satu permintaan.

Konsekuensinya panel memuat dua tahap, dan `token_state` bisa datang dua kali:

| Tahap | Yang dilakukan | Biaya |
| --- | --- | --- |
| 1 | Baca keadaan pool yang kuncinya sudah tersimpan. Tidak menyentuh log sama sekali | satu batch state dari RPC interaktif |
| 2 | Pindai blok yang belum pernah dilihat, gabungkan, baca ulang | dua sampai tiga permintaan |

Kiriman kedua hanya terjadi kalau kumpulan poolnya benar benar berubah. Panel yang menerima dua kiriman identik akan menggambar ulang garis chart tanpa alasan, dan itu terlihat seperti kedipan.

Token yang belum pernah dibuka tidak punya tahap satu, dan itu memang tidak bisa dihindari: pool V4 hanya bisa ditemukan lewat log, dan log harus dipindai sekali. Panel tetap menahan daftar parsial sampai statistik awal seluruh pool siap.

**Permintaan dibagi menjadi tiga jalur RPC.** `rpc.http` melayani pemeriksaan
kesehatan, simulasi final, dan transaksi. `rpc.read_http` melayani metadata,
state pool, TVL, saldo, serta pembacaan rencana. `rpc.logs_http` melayani
discovery dan volume. Pemisahan ini diperlukan karena endpoint yang menjawab
`eth_call` paling cepat dapat membatasi `eth_getLogs` sampai rentangnya tidak
berguna. Jalur baca mencoba endpoint secara berurutan dalam permintaan yang
sama. Timeout dan respons JSON-RPC error memindahkannya langsung ke cadangan,
dan endpoint sehat terakhir didahulukan pada pembacaan berikutnya. Transaksi
tidak memakai failover baca agar satu pengiriman hanya melewati endpoint yang
sudah dinilai sehat. Statistik volume 24 jam
berjalan di latar dan memeriksa penanda prioritas sebelum membaca setiap
potongan log. Saat user meminta rencana posisi atau menjalankan transaksi,
potongan yang sedang berjalan diselesaikan, lalu pemindaian menunggu sampai
permintaan interaktif selesai. Tahap cache dan discovery penuh juga tidak boleh
memulai dua pengukuran statistik untuk token, pasangan, dan himpunan pool yang
sama. Pekerjaan statistik pairing lama dihentikan ketika user memilih pairing
lain. Saat pool dipilih atau rencana diminta, discovery lama dihentikan dan
statistik pairing aktif memberi giliran sampai pekerjaan interaktif selesai.
Seluruh pembaca log latar memakai satu antrean karena
endpoint publik terbukti membalas 429 ketika dua pemindaian berat berjalan
bersamaan. Subscribe baru juga membatalkan task discovery token lama, bukan
hanya membuang hasil akhirnya.

**APR, TVL, dan volume memakai dua jendela serta cache singkat.** Jika cache
belum lengkap, 108.000 blok menghasilkan snapshot awal sekitar tiga jam.
Daftar pool baru ditampilkan setelah volume dan jendela seluruh pool
siap, sehingga kolom tidak terisi satu per satu. Target jalur normalnya paling
lama tujuh detik. Pengukuran 24 jam meneruskan pekerjaan di belakang layar lalu
mengganti snapshot awal. Keduanya disimpan di redb menurut token dan pairing,
tetapi snapshot tiga jam tetap berstatus perlu penyegaran sampai hasil 24 jam
tersedia. Cache berumur paling banyak lima menit langsung dipakai. Cache berumur
lima menit sampai satu jam tetap ditampilkan sambil disegarkan di latar. Hasil
yang lebih lama atau hasil volume yang gagal tidak dipakai. Kegagalan TVL satu
pool tidak membuang volume valid milik pool lain.

Saldo dan statistik dimulai bersamaan. Keduanya tidak lagi diurutkan karena
saldo memakai `read_http` sedangkan volume memakai `logs_http`. Snapshot
statistik mengambil saldo yang sudah selesai, dan kiriman saldo yang datang
sesudahnya mengambil daftar pool terbaru. Daftar pool tetap tidak tampil
sebagian: panel menunggu volume dan jendela seluruh pool pada pairing aktif.

Pengamatan pada bot Zenith menunjukkan alamat token dan daftar puluhan pool
lengkap memiliki cap waktu pada menit yang sama. Bot juga menandai sebagian
pool sebagai belum terindeks. Ini menjadi bukti perilaku, bukan akses ke
implementasinya. Kesimpulan arsitekturalnya adalah Zenith membaca indeks yang
terus diperbarui lalu memvalidasi state on-chain saat pool dipilih. Cache redb
Rangefinder memberi hasil serupa untuk token yang pernah dibaca, sedangkan
token baru masih memerlukan satu discovery log sebelum dapat masuk cache.

Probe PONS `0x39dbed3a2bd333467115de45665cc57f813c4571` pada 14 September
2026 menghasilkan snapshot tiga pool lengkap dalam 4,137 detik pada data dir
baru dengan `dry_run`. Pada engine terpasang yang juga sedang melayani ekstensi,
saldo tiba dalam 1,998 detik dan snapshot lengkap tiba dalam 6,838 detik.

Probe build release pada 13 September 2026 untuk token
`0xba527c23903eff094a987197ccd643f2da76e4ff` menghasilkan snapshot cache dingin
yang utuh pada 4,703 detik. Saldo sudah tiba pada 0,321 detik. Setelah cache 24
jam tersedia, pembukaan ulang menampilkan seluruh statistik pada 0,228 detik
dan saldo pada 0,349 detik. Pengukuran ini memakai endpoint sehat dan menjadi
bukti untuk sasaran tujuh detik, bukan jaminan saat RPC log tidak tersedia.

**Yang tidak pernah di-cache adalah harga dan likuiditas.** Keduanya selalu dibaca ulang dari rantai. Pada alat yang mengirim transaksi, angka basi yang terlihat sah jauh lebih berbahaya daripada menunggu sebentar.

**Hasil terukur** pada FIG di `0xdc4Dd883...`, 39 pool:

| Keadaan | Waktu sampai panel punya isi |
| --- | --- |
| versi pertama | 38 detik |
| setelah Multicall3 | 23,8 detik |
| setelah rentang log diperlebar | 15,1 detik |
| setelah pemindaian diserialkan | sekitar 3 detik |
| dengan kunci pool tersimpan | 400 sampai 700 ms |

Probe release pada PAIREX/USDG `0xb8cf4ad...` dengan range yang diminta user
$0,001203 sampai $0,003615 memberi statistik cache, range, dan pilihan pairing
dalam 5,54 sampai 5,92 detik, saldo USDG dalam 7,23 sampai 7,75 detik, lalu
`open_plan` sumber 1 USDG dalam 12,66 sampai 13,56 detik dari awal. Bagian
perencanaan setelah nominal dikirim memerlukan 5,43 sampai 5,80 detik. Probe
berhenti pada rencana dan tidak mengirim approval, Zap, atau transaksi mint.

### 4.3d Berpindah pasangan tanpa memindai ulang

Seluruh kelompok pasangan disimpan di memori sejak token dimuat, bukan hanya yang terpilih. Menekan pasangan lain di panel mengirim `select_quote`, dan engine memilih ulang dari data yang sama tanpa satu pun permintaan jaringan.

Yang ikut berpindah bukan hanya daftar pool, tapi juga desimal kutipan, dan itu alasan kenapa perpindahan ini tidak boleh diselesaikan di sisi panel. ETH berdesimal 18 sedangkan USDG berdesimal 6, jadi salah membawa desimal menggeser seluruh harga sebesar sepuluh pangkat dua belas.

Token lawan yang tidak ada di `chain.toml` dibaca desimalnya on chain, tidak pernah ditebak 18. Kalau pembacaannya gagal, kelompok itu dibuang alih alih ditampilkan dengan harga yang salah.

### 4.4 Degradasi bertingkat

Rangefinder bergantung pada internal pihak ketiga yang bisa berubah kapan saja. Empat tingkat, turun otomatis:

| Tingkat | Mekanisme | Kapan dipakai |
| --- | --- | --- |
| 1 | `tradingViewApi.createShape` dan `drawing_event` | Normal, sudah terverifikasi |
| 2 | Overlay canvas di atas iframe, kalibrasi lewat `getVisiblePriceRange()` dan tinggi pane | `createShape` hilang tapi API baca masih ada |
| 3 | Overlay canvas dengan kalibrasi dari label sumbu harga di DOM iframe, dua label untuk membentuk pemetaan linear atau logaritmik | Seluruh objek `tradingViewApi` hilang |
| 4 | Chart mini di dalam side panel dari data candle gmgn, range diisi numerik | Halaman gmgn berubah total |

Tingkat 3 wajib mendeteksi skala logaritmik. Jika chart dalam mode log, pemetaan pixel ke harga memakai interpolasi pada `ln(price)`, bukan pada `price`.

Setiap tingkat punya tes integrasi tersendiri yang dijalankan terhadap snapshot HTML gmgn yang disimpan di repo, supaya regresi ketahuan tanpa harus membuka browser.

### 4.5 Aturan kebersihan

- Rangefinder hanya menyentuh shape yang id-nya tercatat di state miliknya.
- Saat tab berpindah token, semua shape milik Rangefinder dihapus sebelum yang baru digambar.
- Saat side panel ditutup, garis tetap ada tapi tidak lagi dipantau. Saat dibuka kembali, garis diadopsi ulang berdasarkan id yang tersimpan.
- Jika user menghapus salah satu garis lewat UI TradingView, panel mendeteksi lewat event `remove` dan menawarkan menggambar ulang, bukan menggambar ulang sendiri tanpa izin.
- `disableSave: true` supaya garis Rangefinder tidak masuk ke layout tersimpan milik user di gmgn.

---

## 5. Signal Engine

### 5.1 Pipeline

```
sumber data (content script gmgn + RPC)
  -> tahap 1  saringan murah pada daftar rank, tanpa panggilan tambahan
  -> tahap 2  ambil candle 1 menit dan metrik holder untuk kandidat lolos
  -> tahap 3  hitung skor Sideways dan proyeksi fee
  -> tahap 4  gate risiko dan pemeriksaan on chain
  -> tahap 5  klasifikasi aksi dan pemeringkatan
```

Tahap 1 dan 2 dipisah karena request gmgn dibatasi 1 per 2 detik. Saringan murah memangkas kandidat sampai sekitar 20 token sebelum data mahal ditarik.

### 5.2 Detektor sideways bervolume tinggi

Ini fitur inti. Semua rumus dihitung di Rust, atas candle 1 menit dengan jendela `W` yang bisa diatur, default 240 bar atau 4 jam.

**Efficiency Ratio Kaufman.** Ukuran paling bersih untuk membedakan tren dari chop.

```
ER = |P_t - P_{t-W}| / sum( |P_i - P_{i-1}| , i = t-W+1 .. t )
```

Nilai mendekati 0 berarti harga banyak bergerak tapi tidak ke mana mana, persis kondisi yang dicari. Nilai mendekati 1 berarti tren bersih.

**Volatilitas realisasi Parkinson.** Memakai high dan low sehingga lebih efisien daripada standar deviasi close.

```
sigma_P = sqrt( ( 1 / (4 * W * ln 2) ) * sum( ln(H_i / L_i)^2 ) )
```

**Kontraksi volatilitas.** Chop yang akan bertahan biasanya didahului penyempitan.

```
VC = ATR(15m) / ATR(4h)      target di bawah 0,80
```

**Kontainmen kanal.** Bentuk kanal dari persentil 10 pada low dan persentil 90 pada high sepanjang W. `C` adalah proporsi bar yang close-nya berada di dalam kanal. `BW` adalah lebar kanal relatif terhadap titik tengah.

**Volume z score.** `z_V = (V_1h - mean(V_1h, 24h)) / std(V_1h, 24h)`.

**Keseimbangan tape.** `B = 1 - |buys - sells| / (buys + sells)`. Tape yang seimbang adalah ciri chop. Tape berat sebelah adalah ciri tren yang sedang berjalan.

**Proksi imbal hasil fee.** `Y = V_1h * fee_tier / liquidity_usd`. Ini yang benar benar dibayar ke LP, dan bukan volume mentah.

**Skor akhir.** Semua suku dinormalisasi ke rentang 0 sampai 1 memakai persentil historis token itu sendiri, bukan konstanta global, karena skala tiap token berbeda jauh.

```
S = 100 * ( 0.20*norm(z_V) + 0.30*(1 - ER) + 0.20*C + 0.10*B + 0.20*norm(Y) )
```

**Gate keras.** Kandidat dibuang jika salah satu tidak terpenuhi.

```
ER  < 0.35
z_V > 0.50
BW  < 0.35
VC  < 0.80
liquidity_usd > ambang user
uji jalur transfer di 5.4 lolos
```

**Persistensi rezim.** Chop mengelompok. Engine menghitung skor untuk 6 jendela sebelumnya. Jika minimal 4 dari 6 juga di atas ambang, skor mendapat pengali 1,15. Ini membuat sinyal lebih ke arah prediksi ketimbang deskripsi masa lalu.

**Pola pasca run.** Setup klasik dari playbook, dideteksi terpisah dan diberi bendera sendiri: token mencetak all time high antara 1 sampai 4 hari lalu, sudah retrace 40 sampai 70 persen dari puncak, dan 6 jam terakhir menunjukkan `ER` rendah dengan volume yang masih di atas median. Ini kandidat LP terbaik yang ada, karena penjual panik sudah habis dan harga membangun basis sambil tetap ramai.

**Bobot bukan angka suci.** Semua bobot dan ambang di atas adalah titik awal, disimpan di file konfigurasi, dan dikalibrasi ulang oleh loop self improvement di bagian 12.

### 5.2b Detektor dump lalu basis

Diminta secara khusus: kondisi di mana harga jatuh seperti dump, tapi volume masih besar dan aktif, dan setelah itu bergerak menyamping. Ini pola yang paling menguntungkan untuk LP dan sekaligus paling sering ditinggalkan trader spot, karena secara tampilan terlihat seperti koin yang sudah selesai.

Detektor ini terpisah dari 5.2 dan punya bendera sendiri, karena 5.2 mengukur kondisi menyamping yang sudah berjalan, sedangkan detektor ini menangkap saat menyamping baru saja terbentuk, yaitu titik masuk yang paling bernilai.

**Bentuk yang dicari.** Sebuah kaki turun yang tajam dan impulsif dari puncak lokal, lalu berhenti, lalu membentuk basis mendatar yang volumenya masih ramai.

**Syarat, dihitung atas candle 1 menit.**

```
1  kedalaman        drawdown = (P_puncak - P_dasar) / P_puncak        >= 0.40
2  ketajaman        jumlah bar dari puncak ke dasar                   <= 60
3  usia dasar       jumlah bar sejak dasar terbentuk                  >= 30
4  tidak ada low baru   min(low, 30 bar terakhir) >= P_dasar * 0.98
5  dasar tidak miring turun   slope regresi linear atas low basis     >= 0
6  volume masih hidup   median V basis / median V sebelum pump        >= 2.0
7  volume sudah mereda  median V basis / V puncak dump          dalam [0.05, 0.60]
8  volatilitas mengendap   ATR(basis) / ATR(jendela dump)             <= 0.50
9  sudah menyamping     ER pada jendela basis                         <  0.35
10 tape seimbang        B pada jendela basis                          >= 0.60
```

**Kenapa setiap syarat ada.** Syarat 1 dan 2 memastikan yang terjadi memang dump, bukan penurunan pelan. Syarat 3 sampai 5 adalah **penjaga dead cat**, yang memisahkan basis sejati dari pendarahan lambat yang terus mencetak low baru. Ini pembeda terpenting, karena tanpa syarat 5 detektor akan terus menyarankan LP di tengah tren turun, dan itu cara tercepat kehilangan modal. Syarat 6 memastikan koin belum mati, karena basis tanpa volume tidak membayar fee. Syarat 7 memastikan kepanikan sudah lewat, karena volume yang masih sekencang puncak dump berarti penjualan belum selesai. Syarat 8 sampai 10 memastikan menyamping sudah benar benar terbentuk.

**Sinyal penguat, menaikkan peringkat tapi tidak wajib.**

- **Sumbu kapitulasi.** Bar dasar punya `z_V > 3` dan sumbu bawah lebih panjang dari dua kali badannya. Ini penanda dasar yang jauh lebih dapat dipercaya daripada low biasa, dan menjadi jangkar batas bawah terbaik.
- **Breakdown gagal.** Harga sempat menembus di bawah dasar basis lalu direbut kembali dalam kurang dari 10 bar dengan volume naik. Konfirmasi terkuat bahwa dasar itu dipertahankan.
- **Rasio jual mereda.** Porsi volume jual pada jendela basis lebih kecil daripada pada jendela dump. Penjual habis.

**Keluaran detektor.** Selain skor, detektor mengembalikan jangkar harga yang langsung dipakai untuk menempatkan garis di 5.7:

```
L_basis   low terendah jendela basis, atau low sumbu kapitulasi jika ada
H_basis   high tertinggi jendela basis
POC       harga dengan volume terbanyak pada jendela basis
VAL, VAH  batas area nilai 70 persen pada jendela basis
P_invalid level pembatalan, yaitu L_basis dikurangi satu ATR basis
```

**Risiko yang harus dinyatakan di kartu aksi.** Pola ini bisa juga mendahului kaki turun kedua. Karena itu setiap posisi yang dibuka atas dasar sinyal ini **wajib** membawa level pembatalan `P_invalid`. Jika harga menutup di bawahnya dengan `z_V > 1`, engine mengirim notifikasi kritis dan menyarankan tutup. Sinyal ini tidak pernah disajikan sebagai kepastian.

### 5.3 Filter playbook

Diturunkan langsung dari 2.5, semuanya bisa diubah user.

| Profil | Market cap | Umur | Urutan | Target |
| --- | --- | --- | --- | --- |
| Nama mapan | Di bawah 25 juta USD | 7 hari ke atas | Volume | Sekunder 5 sampai 10 juta |
| Nama baru | 500 ribu USD ke atas | Di bawah 7 hari | Volume | Entry 1 sampai 2 juta |

Filter holder, memakai data wallet dari gmgn:

- Wallet mati, tidak ada transaksi dalam 72 jam terakhir, tidak dihitung sebagai sinyal.
- Wallet yang beli lalu jual dalam 2 menit tidak dihitung.
- Wallet yang masuk ke lebih dari 30 launch dalam 7 hari terakhir tidak dihitung.
- Wallet yang selektif dan masuk awal diberi bobot, dan pembeliannya menjadi sinyal.

Filter komunitas: jika `twitter_username` ada, engine memantau kecepatan posting akun dan penyebutan ticker. Aktivitas yang mati adalah alasan menurunkan peringkat, sesuai poin ketiga playbook.

### 5.4 Gate risiko

Dua kelompok. Kelompok pertama dari data gmgn, murah:

```
renounced_mint, renounced_freeze_account, burn_ratio, burn_status,
top_10_holder_rate, dev_token_burn_ratio, creator_token_status,
creator_close, fund_from_address, is_show_alert, hot_level
```

Kelompok kedua dari simulasi on chain, mahal tapi wajib sebelum modal masuk:

1. **Uji transfer tax pada jalur yang sebenarnya dipakai.** Angka `Total Tax` di halaman gmgn hanya saringan awal, bukan keputusan. Banyak memecoin memang berpajak untuk pembeli biasa, tapi mengecualikan alamat pool, router, dan PositionManager, sehingga LP tetap berjalan normal. Menolak semua token berpajak secara membabi buta akan membuang banyak setup bagus, termasuk token pada gambar contoh yang menampilkan pajak 2 persen beli dan 2 persen jual.

   Karena itu ujinya adalah simulasi terhadap **jalur persis yang akan dipakai saat mint**, yaitu dari wallet ke PositionManager lalu ke pool. Ukur selisih antara jumlah yang dikirim dan jumlah yang benar benar diterima pool. Selisih nol berarti jalur itu dikecualikan dari pajak dan token boleh di-LP. Selisih bukan nol adalah blokir keras, karena matematika concentrated liquidity mengasumsikan transfer tepat, dan posisi bisa berakhir tidak bisa ditutup dengan benar.

   Implementasi menyimpan bukti pada namespace
   `risk/token:v1:<chain>:<pool>:<token>`. Gate R1 tidak memakai angka metadata:
   ia menerima hanya jika G4 dan G5 pada calldata LP yang persis akan dikirim
   sama sama lolos. Uniswap core menolak penyelesaian ketika jumlah yang diterima
   kurang dari delta yang diminta. Bukti diuji ulang pada setiap penyusunan
   rencana, sehingga tidak pernah mengandalkan cache berumur 24 jam.
2. **Uji honeypot.** R2 mengutip dan mensimulasikan jual sekitar 0,10 USD lewat
   router sebenarnya dari alamat wallet. V3 memakai QuoterV2 dan SwapRouter02;
   V4 memakai V4Quoter dan UniversalRouter fork Robinhood. Allowance aktual,
   `msg.value`, batas slippage, dan seluruh transfer dijalankan dalam `eth_call`.
   Revert, keluaran nol, allowance kurang, atau kegagalan RPC adalah blokir.
3. **Uji hook V4 dengan allowlist berbasis adopsi.** Baca alamat hook dari PoolKey. Hook nol berarti aman.

   Keputusan user: hook yang sudah ramai dipakai orang boleh langsung masuk allowlist. Supaya itu bisa dijalankan mesin dan bukan tebakan, adopsi diukur, bukan dikira kira. Sebuah hook masuk allowlist otomatis jika memenuhi seluruh syarat berikut, dihitung dari data on chain:

   ```
   jumlah pool yang memakai hook itu      >= 50
   total TVL seluruh pool tersebut        >= 250.000 USD
   jumlah alamat LP unik                  >= 200
   umur sejak deploy                      >= 14 hari
   tidak ada fungsi upgrade atau proxy admin yang masih aktif
   ```

   Syarat terakhir tidak bisa ditawar. Hook yang bisa diganti isinya sewaktu waktu tidak boleh masuk allowlist berapa pun adopsinya, karena kode yang diperiksa hari ini bukan kode yang akan berjalan besok. BagsV4Hook dievaluasi memakai kriteria yang sama seperti hook lain, tanpa perlakuan khusus.

   Hook yang belum lolos tetap bisa dipakai user secara manual lewat pilihan sadar per pool, yang menampilkan peringatan dan mencatat persetujuannya. Yang otomatis hanya penolakan bawaannya.
4. **Uji likuiditas terkunci.** Verifikasi bahwa pool bukan pool palsu yang bisa dikuras kreator.

**Keadaan pengukuran butir 3 per 10 September 2026.** Kelima syarat sudah
terukur di `risk::hook`. Pool dan umur berasal dari event `Initialize` yang
di-dedup menurut poolId. Setelah sejarah Initialize lengkap, event
`ModifyLiquidity` PositionManager memetakan salt ke token ID; `ownerOf` dibaca
ulang lewat Multicall agar posisi terbakar tidak dihitung dan alamat pemilik
aktif dapat didedup. TVL dibentuk dari distribusi likuiditas tiap pool pada
jendela harga yang sama, USDG dinilai langsung, dan ETH/WETH dikonversi dengan
pool V3 WETH/USDG berlikuiditas aktif terkuat.

Pemindaian yang belum sampai deploy block menghasilkan batas bawah, bukan
kesimpulan palsu. Batas bawah yang sudah melewati ambang boleh membuktikan
lolos; angka di bawah ambang tetap `belum_terukur` sampai sejarah lengkap.
Hook yang memenuhi kelima syarat dicatat otomatis selama 24 jam. Setiap modal
masuk tetap membaca ulang code hash dan empat slot proxy. Hook lain memakai
`approve_hook`, yang mencatat penilaian dan hash bytecode yang dilihat user.

Satu satunya syarat yang tidak bisa dilewati persetujuan adalah kemampuan
upgrade, dan penolakannya ditegakkan di engine, bukan hanya di panel. Panel
bisa keliru atau diakali, dan satu bug di sana tidak boleh cukup untuk menaruh
modal di hook yang isinya bisa diganti sewaktu waktu. Gagal membaca slot proxy
diperlakukan sama dengan bisa diupgrade, karena yang belum terbukti aman bukan
berarti aman.
### 5.5 Klasifikasi aksi

Satu token menghasilkan tepat satu label.

| Label | Kondisi | Bentuk posisi |
| --- | --- | --- |
| `LP Sideways` | Skor `S` tinggi, semua gate lolos | Range simetris mengelilingi harga, dua sisi |
| `LP Beli Turun` | Pola pasca run, harga masih di atas zona basis | Range seluruhnya di bawah harga, satu sisi dalam WETH. Berfungsi sebagai tangga limit buy yang sekaligus memungut fee |
| `LP Jual Naik` | Sudah memegang token dan ingin keluar bertahap | Range seluruhnya di atas harga, satu sisi dalam token. Tangga limit sell yang memungut fee |
| `Pantau` | Sebagian gate lolos, sinyal belum matang | Tidak ada aksi, masuk daftar pantau |
| `Hindari` | Ada gate risiko yang gagal | Diblokir, alasan ditampilkan |

Dua label satu sisi itu penting dan sering dilupakan. Range yang seluruhnya berada di satu sisi harga tidak butuh dua token, tidak menanggung impermanent loss sampai harga menyentuhnya, dan mengubah LP menjadi alat eksekusi order yang dibayar fee. Ini yang membuat produk ini berguna bahkan saat pasar sedang tidak menyamping.

### 5.6 Keluarga sinyal tambahan

Selain 5.2 dan 5.2b, sembilan sinyal berikut dihitung dan bisa dinyalakan sendiri sendiri. Semuanya dipilih karena bisa dihitung dari data yang sudah kita punya, dan karena masing masing menjawab pertanyaan yang berbeda.

| Sinyal | Yang diukur | Rumus atau sumber | Arti untuk LP |
| --- | --- | --- | --- |
| Kecepatan fee | Fee nyata per dolar likuiditas, satu jam terakhir | `fee_1h / liquidity_usd` dari event `Swap` on chain | Pengukur paling langsung tentang di mana uang LP sedang dihasilkan. Ini yang seharusnya jadi urutan utama, bukan volume |
| Vakum likuiditas | Volume besar bertemu likuiditas tipis | `volume_1h / liquidity_usd` | Laju fee ekstrem. Risiko juga tinggi karena dampak harga saat masuk dan keluar besar |
| Likuiditas menganggur | Selisih TVL nominal dengan likuiditas yang benar benar aktif | Peta tick: `L_dalam_range_aktif / L_total` | Ini keunggulan yang jarang dipakai orang. Pool dengan TVL besar tapi sebagian besarnya terparkir jauh di luar range berarti porsi kita jauh lebih besar daripada yang terlihat dari TVL |
| Sumbu kapitulasi | Bar dasar dengan volume ekstrem dan sumbu bawah panjang | `z_V > 3` dan sumbu bawah lebih dari dua kali badan | Jangkar batas bawah paling dapat dipercaya |
| Breakdown gagal | Tembus ke bawah lalu direbut kembali | Tembus di bawah `L_basis` lalu close di atasnya dalam 10 bar dengan volume naik | Konfirmasi dasar. Setelah ini, batas bawah boleh dinaikkan lebih rapat |
| Squeeze rentang | Kelompok bar dengan rentang tersempit | k bar tersempit dalam N terakhir muncul berdekatan | Volatilitas rendah cenderung berlanjut dalam jangka pendek, cocok untuk range rapat |
| Churn dua arah | Banyak transaksi kecil, arah seimbang | Jumlah swap tinggi, `B >= 0.7`, ukuran rata rata trade kecil | Mesin fee paling murni. Banyak transaksi kecil membayar lebih baik daripada sedikit transaksi besar |
| Musim volume harian | Jam aktif token itu sendiri | Rata rata volume per jam dalam sehari, dari 7 hari terakhir | Buka posisi sebelum jendela ramai dimulai, bukan setelahnya |
| Delta kepadatan LP | Perubahan mendadak likuiditas pesaing di range sasaran | Event `Mint` dan `Burn` pada pita tick sasaran | Penarikan besar oleh pihak lain berarti porsi kita melonjak. Ini jendela masuk yang bagus dan berumur pendek |

Tiga di antaranya layak disebut lagi karena paling jarang dipakai orang dan paling mudah diukur: kecepatan fee, likuiditas menganggur, dan delta kepadatan LP. Ketiganya berbicara tentang **penerimaan kita**, sementara hampir semua alat lain hanya berbicara tentang **pergerakan harga**. Untuk LP, yang pertama jauh lebih menentukan.

### 5.7 Doktrin penempatan range

Bagian ini menjawab pertanyaan yang paling sulit: di mana persisnya garis harus ditaruh. Aturannya dibuat supaya bisa dijalankan mesin, dan supaya user bisa memeriksa alasannya, bukan sekadar percaya.

**Prinsip dasar yang mendasari semuanya.** Di concentrated liquidity, harga yang naik menembus range membuat kita berakhir memegang seluruhnya WETH, artinya kita telah menjual. Harga yang turun menembus range membuat kita berakhir memegang seluruhnya token, artinya kita telah membeli. Jadi menempatkan range sama dengan menyatakan dua hal sekaligus: pada harga berapa saya bersedia membeli, dan pada harga berapa saya bersedia menjual. Setiap kali ragu, kembalikan pertanyaannya ke bentuk itu.

**Langkah 1, tentukan rezim.** Dari 5.2, 5.2b, dan `ER`.

**Langkah 2, temukan jangkar struktur.** Semua dihitung dari candle 1 menit pada jendela yang relevan, bukan dari angka bulat.

```
L_basis   low terendah, atau low sumbu kapitulasi jika ada
H_basis   high tertinggi
POC       harga dengan volume terbanyak
VAL, VAH  batas bawah dan atas area nilai 70 persen
ATR       rentang sebenarnya rata rata pada jendela basis
```

**Langkah 3, petakan rezim ke batas.**

| Rezim | Batas bawah | Batas atas | Alasan |
| --- | --- | --- | --- |
| Basis pasca dump | Tepat di bawah `L_basis`, sekitar `L_basis` dikurangi `0.25 x ATR` | `VAH`, atau retracement 0.382 dari kaki dump, ambil yang lebih rendah | Bawah adalah harga di mana kita rela memegang token. Atas adalah tempat pantulan pertama biasanya tersendat |
| Menyamping murni | `VAL` | `VAH` | Area nilai menampung sekitar 70 persen volume. Di sinilah fee sesungguhnya dibayar |
| Tren naik | `POC`, yang berada di bawah harga | Jauh di atas harga, minimal `2 x ATR` | Batas atas yang terlalu rapat akan menjual seluruh posisi terlalu cepat, tepat sebelum bagian terbaiknya |
| Tren turun | Jangan pasang range dua sisi | | Pakai `LP Jual Naik` satu sisi, atau tidak sama sekali. Range dua sisi di tren turun hanya membeli terus menerus sambil harga jatuh |
| Launch baru | Tidak ada penempatan | | Belum ada struktur. Tidak ada `L_basis` yang bisa dipercaya, jadi tidak ada batas bawah yang bisa dibenarkan |

**Langkah 4, pisahkan inti dan luar.** Jangan menaruh seluruh modal pada satu lebar. Bagi dua:

```
range inti   VAL sampai VAH             sekitar 70 persen modal   fee maksimum
range luar   L_basis*0.95 sampai H_basis*1.05   sekitar 30 persen modal   penangkap sumbu dan penyangga
```

Range inti yang rapat menghasilkan fee jauh lebih besar per dolar, sementara range luar menahan sumbu harga tanpa menyeret seluruh posisi keluar. Keduanya digambar sekaligus di chart dengan ketebalan garis berbeda.

**Langkah 5, tiga pemeriksaan sebelum konfirmasi.** Jika salah satu gagal, penempatan ditolak dan panel menjelaskan mana yang gagal.

1. Jam impas IL dari 11.4 harus lebih pendek daripada ekspektasi jam bertahan dari 11.2. Kalau tidak, posisi kalah secara matematis sebelum dibuka.
2. Porsi kita terhadap likuiditas aktif di dalam range harus di atas ambang minimum. Kalau terlalu kecil, fee-nya tidak sepadan dengan risikonya.
3. Level pembatalan wajib terdefinisi dan wajib berada di bawah batas bawah, bukan di dalam range.

**Langkah 6, aturan darurat saat struktur tidak jelas.** Kalau chart tidak menunjukkan basis yang bisa dibaca dan user tetap ingin masuk, jangan menebak lebarnya. Pakai `harga sekarang dikurangi 1.5 x ATR` sampai `harga sekarang ditambah 1.5 x ATR`, ukuran modal setengah dari biasa, dan tinjau ulang dalam 30 menit. Ini bukan penempatan optimal, ini penempatan yang tidak memalukan saat salah.

**Contoh terpakai pada gambar referensi.** Angka di bawah dibaca dari chart contoh dan sifatnya perkiraan, tapi cara berpikirnya yang penting.

Bacaan struktur: all time high di 527,61K, dump ke dasar sekitar 200K, harga sekarang 239,22K, pantulan tertinggi setelah dump sekitar 300K. Jadi basisnya kira kira 200K sampai 300K, dan POC kemungkinan berada di 230K sampai 250K karena di situ lilin konsolidasi menumpuk.

Penempatan yang dihasilkan doktrin ini:

```
range inti   sekitar 215K  sampai 280K
range luar   sekitar 195K  sampai 320K
pembatalan   penutupan di bawah 195K dengan z_V > 1
```

Bandingkan dengan garis pada gambar, yaitu 300K di atas dan 150K di bawah. Garis atas di 300K sudah tepat, karena persis berada di puncak basis dan di resistensi pertama. Garis bawah di 150K yang bermasalah, karena berada 25 persen di bawah dasar basis. Seluruh modal yang duduk di tick antara 150K dan 200K kemungkinan besar tidak akan pernah tersentuh perdagangan, jadi tidak menghasilkan apa apa sambil tetap mengunci modal.

Besarnya kerugian itu bisa dihitung. Kepadatan likuiditas untuk modal yang sama kira kira berbanding terbalik dengan `sqrt(Pb) - sqrt(Pa)`:

```
150K sampai 300K   ->  sqrt(300) - sqrt(150) = 17,32 - 12,25 = 5,07
200K sampai 300K   ->  sqrt(300) - sqrt(200) = 17,32 - 14,14 = 3,18
215K sampai 280K   ->  sqrt(280) - sqrt(215) = 16,73 - 14,66 = 2,07
```

Artinya, dengan modal yang sama, menaikkan batas bawah dari 150K ke 200K memberi sekitar 1,6 kali lebih banyak fee selama harga berada di dalam range. Memakai range inti 215K sampai 280K memberi sekitar 2,45 kali lipat dibanding penempatan awal. Angka ini pendekatan untuk ilustrasi, sedangkan engine menghitungnya persis dari likuiditas dan komposisi token yang sebenarnya.

Itulah nilai yang diberikan alat ini. Bukan menebak arah, melainkan memastikan modal ditempatkan di tick yang benar benar dilewati perdagangan.

---

### 5.9 Catatan implementasi mesin sinyal

Seluruh detektor murni: masuk deret candle, keluar angka. Tidak ada yang menyentuh jaringan, dan tidak ada yang menyimpan keadaan. Itu disengaja, karena bagian inilah yang menentukan di mana uang ditaruh, dan bagian seperti itu harus bisa diuji ribuan kali tanpa rantai.

**Bar datang dari chart gmgn, bukan dari sumber luar.** Alasannya sama dengan kalibrasi kurs di 4.3: angka yang dipakai untuk keputusan harus sama dengan yang dilihat user di layar. Sumber lain akan selalu sedikit berbeda, dan selisihnya muncul persis saat harga bergerak cepat, yaitu saat keputusannya paling penting. Deret penuh dikirim tiap tiga puluh detik lewat `chart_bars`, jauh lebih jarang daripada `chart_price`, karena isinya ratusan bar dan jawaban detektor tidak berubah dalam hitungan detik.

**Data yang belum cukup mengembalikan kosong, bukan skor rendah.** Ini pembedaan yang berulang di seluruh modul. Skor rendah berarti sudah diperiksa dan tidak menarik. Kosong berarti belum bisa diperiksa. Menyamakan keduanya membuat token yang chartnya baru termuat terlihat seperti token yang buruk, dan itu menyembunyikan kandidat yang justru sedang dicari.

**Beberapa penyesuaian terhadap rumus di 5.2, dan alasannya.**

| Yang berubah | Alasan |
| --- | --- |
| Volume z score menjepit sumbangannya ke skor pada tiga simpangan | Tanpa penjepitan, satu lonjakan ekstrem mendominasi seluruh skor sehingga suku lain tidak berarti, dan token yang sedang tren bisa lolos hanya karena volumenya meledak |
| Bar terakhir tidak ikut dalam rata rata pembanding volume | Kalau ikut, lonjakan besar menaikkan rata ratanya sendiri, sehingga skornya lebih kecil daripada yang seharusnya, persis pada kejadian yang paling ingin ditangkap |
| Bar yang tutup sama dengan bukanya tidak dihitung dalam keseimbangan tape | Bar seperti itu tidak membawa arah sama sekali. Menganggapnya beli membuat deret yang paling seimbang justru terbaca berat sebelah |
| Efficiency ratio mengembalikan kosong saat harga sama sekali tidak bergerak | Nilai nol berarti chop sempurna, dan harga yang diam bukan itu. Ia berarti datanya tidak berguna |
| ATR memakai close bar sebelumnya, bukan hanya rentang bar | Mengabaikan celah antar bar meremehkan volatilitas persis saat harga melompat, yaitu saat ukuran itu paling dibutuhkan |

**Detektor dump lalu basis mengembalikan hasil meski basisnya belum terbentuk.** Token yang masih mencetak low baru bisa dinilai, dan jawabannya adalah belum layak. Mengembalikannya sebagai kosong akan menyembunyikan token yang sedang jatuh, padahal itu justru yang perlu dilihat user supaya tidak masuk terlalu cepat.

**Bobot dijaga berjumlah satu, dan ada ujinya.** Kalau ada yang menambah suku baru, ia harus mengurangi yang lain. Itu memaksa keputusan sadar alih alih penambahan diam diam yang menggeser seluruh skala skor.

### 5.10 Penempatan sebagai keputusan beli dan jual

Setiap saran penempatan wajib menyertakan alasannya, dan alasan itu selalu dalam bentuk yang sama: pada harga berapa saya bersedia membeli, dan pada harga berapa saya bersedia menjual. Angka tanpa alasan tidak membantu user belajar, dan user yang tidak belajar akan terus bergantung pada alat ini bahkan saat alatnya salah.

**Batas ditarik masuk dari batas struktur, bukan ditaruh persis di atasnya.** Batas kanal adalah tempat harga berbalik, dan dasar basis adalah tempat penjual habis. Menaruh batas range persis di situ membuat posisi keluar range tepat saat harga mengujinya, dan pengujian ulang adalah hal yang normal terjadi. Kanal ditarik masuk sepersepuluh lebarnya, basis lima belas persen dari sisi bawah.

**Batas bawah tidak pernah melewati level pembatalan.** Range yang melewatinya berarti bersedia terus membeli sesuatu yang anggapannya sudah terbukti salah.

**Jangkar basis menang atas kanal.** Kalau detektor dump lalu basis lolos, jangkarnya yang dipakai, bukan kanal statistik dari detektor sideways. Alasannya, jangkar basis terbentuk dari perilaku sungguhan: penjual habis di satu harga, dan pembeli mempertahankannya. Kanal hanya ringkasan sebaran.

**Konsentrasi ditampilkan sebagai perbandingan, bukan angka mutlak.** Yang berarti bagi user bukan nilai konsentrasinya, melainkan berapa kali lipat ia dibanding range yang lebih lebar. Itu yang menjelaskan kenapa range sempit menghasilkan lebih banyak fee dari volume yang sama.

**Range satu sisi punya tempatnya sendiri.** Range yang seluruhnya di bawah harga hanya berisi token kutipan dan berperan sebagai tangga limit buy yang dibayar fee. Kalau harga tidak pernah turun ke sana, tidak ada yang terbeli dan tidak ada yang rugi. Engine menolak menyusunnya kalau batasnya menyentuh harga sekarang, karena saat itu ia bukan lagi satu sisi dan sebagian modal langsung berubah bentuk.

## 6. LP Execution Engine

### 6.1 Alur membuka posisi

```
input: pool, tickLower, tickUpper, modal, token sumber
  1  hitung rasio token yang dibutuhkan untuk range tersebut pada harga sekarang
  2  jika token sumber tidak sesuai rasio, susun swap zap
  3  bangun calldata mint
  4  gabungkan menjadi satu transaksi multicall yang atomik
  5  jalankan seluruh gate validasi bagian 6.4
  6  kirim, pantau, verifikasi ulang on chain
```

Rasio token untuk range `[Pa, Pb]` pada harga `P`, dengan `L` sebagai likuiditas:

```
Pa <= P <= Pb:  amount0 = L * (1/sqrt(P) - 1/sqrt(Pb))
                amount1 = L * (sqrt(P) - sqrt(Pa))
P < Pa:         amount0 = L * (1/sqrt(Pa) - 1/sqrt(Pb)),  amount1 = 0
P > Pb:         amount0 = 0,  amount1 = L * (sqrt(Pb) - sqrt(Pa))
```

Zap dihitung dengan menyelesaikan berapa bagian dari modal yang harus ditukar supaya sisa dua token tepat memenuhi rasio di atas, dengan memperhitungkan dampak harga dari swap itu sendiri. Penyelesaiannya memakai bagi dua, bukan Newton seperti yang semula direncanakan, dan alasannya beserta angka hasilnya ada di 6.7. Setiap kutipan diverifikasi dengan V4Quoter sebelum dikirim.

Mengikuti LPZenith: jika grid tick tidak memungkinkan penempatan range dalam 1 persen dari harga live, posisi ditolak dan engine menyarankan pool dengan fee tier lebih halus.

### 6.2 Jalur V3

Mint lewat `NonfungiblePositionManager.mint(MintParams)`. Posisi berupa NFT ERC721. Seluruh langkah dibungkus `multicall` milik NonfungiblePositionManager agar atomik.

**Koreksi terhadap rencana awal soal Permit2.** Bagian ini semula menyebut approval memakai Permit2 supaya tidak perlu approve terpisah tiap token. Itu keliru untuk jalur yang kita pakai. Permit2 dipakai UniversalRouter, sedangkan NonfungiblePositionManager v3 menarik token lewat `transferFrom` biasa, jadi ia butuh allowance ERC20 langsung ke alamatnya. Tanpa itu `mint` selalu revert dengan `TRANSFER_FROM_FAILED`, dan revert itu terjadi setelah gas terbakar.

Karena itu membuka posisi berjalan dua transaksi: `approve` lebih dulu, lalu `mint`. Untuk posisi baru, izin diberikan **sebesar anggaran flow yang sudah dibatasi input**, bukan seluruh saldo dan bukan tak terbatas. Jumlah mint pada satu snapshot dapat berubah selama approval masuk blok; memakai angka itu persis dapat membuat quote berikutnya meminta approval lagi. Biaya tambahannya satu transaksi per posisi, dan pada rantai berblok 100 ms itu hampir tidak terasa.

Aksi lain: `increaseLiquidity`, `decreaseLiquidity`, `collect`, `burn`. Rebalance adalah `decreaseLiquidity` penuh, lalu `collect`, lalu `mint` baru, semuanya dalam satu multicall.

### 6.3 Eksekusi V4

Bentuk transaksinya berbeda secara mendasar dari V3, dan perbedaannya bukan detail yang bisa disamarkan di balik satu builder.

**Tidak ada fungsi `mint`.** Satu satunya pintu masuk adalah `modifyLiquidities(bytes unlockData, uint256 deadline)`, dan `unlockData` adalah `abi.encode(bytes actions, bytes[] params)`. Setiap byte di `actions` adalah satu aksi, dan `params[i]` parameternya.

Membuka posisi berarti dua aksi: `MINT_POSITION` bernilai `0x02`, lalu `SETTLE_PAIR` bernilai `0x0d`. Tanpa penyelesaian, token tidak pernah berpindah dan seluruh transaksi revert dengan `CurrencyNotSettled`. Menutup posisi memakai `BURN_POSITION` bernilai `0x03` lalu `TAKE_PAIR` bernilai `0x11`.

Parameter `MINT_POSITION` adalah PoolKey yang diratakan menjadi lima kata, lalu `tickLower`, `tickUpper`, `liquidity`, `amount0Max`, `amount1Max`, `owner`, dan `hookData`.

**Bentuk ini tidak disalin dari repo Uniswap.** Ia dibaca langsung dari transaksi sungguhan di Robinhood Chain, `0x92acbe0bc669fbdff06263646d45399825c1a398090fbe42e8530bde610f1ccd`, dan builder kita diuji dengan membangun ulang calldata transaksi itu lalu membandingkannya byte per byte, seluruhnya 868 byte. Alasannya sama dengan kenapa UniversalRouter tidak dipakai apa adanya: rantai ini sudah terbukti memakai fork di satu tempat, jadi ABI dari ingatan bukan bukti. `cargo xtask v4probe` memeriksa setiap selector ada di bytecode yang terpasang, lalu memanggil fungsi bacanya sungguhan.

**Izinnya dua lapis.** V3 menarik token lewat `transferFrom` biasa, jadi cukup satu allowance ERC20. PositionManager V4 menarik lewat Permit2, sehingga:

| Lapis | Apa yang terjadi |
| --- | --- |
| 1 | Token mengizinkan Permit2 lewat `approve` ERC20 biasa |
| 2 | Permit2 mengizinkan PositionManager lewat `Permit2.approve`, yang punya masa berlaku |

Melewatkan lapis kedua adalah kesalahan yang paling mudah terjadi, karena lapis pertama terlihat seperti sudah cukup. Akibatnya `mint` revert setelah gas terbakar. Masa berlaku disetel tiga puluh menit: izin Permit2 yang berlaku setahun sama saja dengan izin tak terbatas dalam waktu.

Error Permit2 `AllowanceExpired(uint256)` dan `InsufficientAllowance(uint256)`
datang sebagai selector `0xd81b2f2e` dan `0xf96fb071`. Penerjemah mengenali
selector itu sebelum reason pendek V3 seperti `TI`; pencarian substring `ti`
akan keliru mencocokkan kata `execution` pada hampir setiap revert.

**Slippage dijaga dari arah yang berlawanan.** V3 memakai `amount0Min` dan `amount1Min`, yaitu batas bawah yang harus tercapai. V4 memakai `amount0Max` dan `amount1Max`, yaitu batas atas yang boleh terpakai. Itu satu satunya pengaman di jalur ini, jadi menyetelnya sebesar saldo berarti mengizinkan pool mengambil seluruh saldo saat harga bergerak. Engine memakai jumlah yang dibutuhkan ditambah margin slippage, dan tidak pernah melampaui saldo.

**ETH asli didukung.** Currency `0x0000...0000` di V4 berarti ETH asli, bukan alamat kosong. Nilainya ikut sebagai `msg.value`, bukan ditarik lewat Permit2, dan ia tidak punya izin sama sekali. Sebaliknya, ETH yang ikut untuk pool yang kedua sisinya token akan tersangkut di kontrak, jadi gate G5 menolaknya.

**Verifikasi pasca kirim membaca lebih banyak.** V4 tidak memancarkan `IncreaseLiquidity` di PositionManager, jadi penanda posisi baru adalah `Transfer` ERC721 dari alamat nol. Yang dibandingkan bukan hanya tick dan likuiditas, tapi PoolKey lengkap termasuk hook, karena poolId adalah hash dari kelimanya. Hook yang berbeda berarti pool yang berbeda, betapapun mirip sisanya.

**Bukti live pertama, 10 September 2026.** Dengan batas posisi, harian, dan
ukuran pertama pool masing masing US$5, setoran awal 0,0005 ETH menjalankan
zap `0x94f3...ce01` lalu mint `0xfd2a...2170` pada pool PONS/ETH
`0x5b940d...3780`. Receipt mint membuat posisi V4 `2339246`; pembacaan ulang
menemukan tick 80400–88038, likuiditas `95279522151851660`, status in-range,
dan G7 cocok. Seluruh arus keluar native termasuk gas adalah
0,000589301266414847 ETH atau sekitar US$1,46 pada kurs saat uji.

`PositionInfo` mengemas kedua tick sebagai int24 di dalam satu uint256: delapan bit terbawah flag, lalu `tickLower`, lalu `tickUpper`, lalu poolId terpotong. Pembacaannya diuji terhadap posisi V4 sungguhan, tokenId 2100029.

### 6.3b Pemilihan jalur

Versi pool menentukan segalanya: kontrak tujuan, bentuk calldata, bentuk izin, dan cara verifikasi. Gate G0 menolak pool yang versinya tidak cocok dengan jalur, dan pemilihannya terjadi di satu tempat saja, `exec::plan::plan_open_auto`.

Panel tidak pernah tahu protokol mana yang dipakai. Ia menerima daftar langkah izin yang bentuknya seragam, dan menekan tombolnya sampai daftar itu kosong. Kalau panel tahu bedanya, ia akan menyimpulkan sendiri, dan dua tempat yang menyimpulkan hal yang sama pasti menyimpang.

**Yang tetap berlaku dari rencana awal.** UniversalRouter di rantai ini adalah fork yang membawa field tambahan `minHopPriceX36`, jadi encoder swap dibuat manual dan tidak memakai encoder SDK. Alamat hook wajib diperiksa terhadap allowlist sebelum transaksi apa pun disusun. Matematika likuiditas V4 identik dengan V3, jadi modul `math` dipakai bersama, dan yang berbeda hanya pembungkus transaksi beserta akuntansi currency-nya.

### 6.4 Gate validasi berlapis

Ini permintaan eksplisit: V3 dan V4 sekaligus, tapi bug tidak boleh terjadi.
Gate utama G0 sampai G7 ditambah dua gate token R1 dan R2. Setiap transaksi
modal masuk wajib melewati gate yang berlaku, tanpa jalan pintas.

| Lapis | Nama | Isi |
| --- | --- | --- |
| G0 | Versi protokol | Pool V3 hanya masuk builder V3 dan pool V4 hanya masuk builder V4. Kontrak tujuan, izin, simulasi, serta verifikasi dipilih bersama. |
| G1 | Verifikasi registry saat start | Setiap alamat kontrak di konfigurasi dicek `eth_getCode`, hash bytecode dibandingkan dengan nilai yang dipin di repo. Beda berarti engine menolak jalan. |
| G2 | Builder calldata bertipe | Semua struct dibangun lewat tipe Rust dari ABI, bukan string. Unit test per fungsi. |
| G3 | Golden vector | Calldata hasil builder dibandingkan byte per byte dengan calldata dari transaksi yang benar benar berhasil di rantai. Wajib untuk V3 mint, V4 mint, dan setiap jalur router. Perbedaan sekecil apa pun menggagalkan build. |

Catatan tentang G3. Rencana awal merekam calldata dari UI Uniswap resmi.
Sumbernya diganti menjadi **transaksi sungguhan di rantai**, yang lebih kuat
karena tiga alasan. Pertama, transaksi itu terbukti diterima kontrak, bukan
sekadar dihasilkan antarmuka. Kedua, pengambilannya bisa diotomatiskan lewat
`cargo xtask golden` sehingga vektornya mudah diperbarui dan diperbanyak.
Ketiga, ia menangkap pemakaian nyata di chain ini, termasuk pola yang tidak
muncul di UI, misalnya posisi satu sisi.

Pengambilan dan pengujian dipisah dengan sengaja. `cargo xtask golden`
menyentuh jaringan dan dijalankan manual. Ujinya berjalan offline terhadap
berkas di `engine/fixtures/golden/`, sehingga deterministik. Kalau
perbandingannya gagal, yang salah adalah builder, bukan fixture. Memperbarui
fixture supaya uji lewat berarti menyembunyikan kesalahan yang akan muncul lagi
saat dana bergerak.
| G4 | Simulasi | Jalankan transaksi terhadap state pending sebelum kirim. V3 memakai `eth_call`; V4 memakai `eth_estimateGas` karena `modifyLiquidities` tidak membawa hasil, sehingga satu eksekusi membuktikan tidak ada revert sekaligus memberi estimasi gas. Pesan revert didekode ke bahasa manusia. |
| G5 | Asersi invarian | Hasil simulasi diperiksa: tokenId tercipta, delta likuiditas sesuai, jumlah token dalam batas slippage, tidak ada token tersisa di kontrak, tick akhir sama dengan yang diminta. |
| G6 | Pembatas nilai | Mode dry run untuk semua fitur baru. Batas nominal per transaksi dan per hari. Posisi pertama pada pool baru dibatasi ke ukuran kecil yang bisa diatur. |
| G7 | Verifikasi pasca kirim | Setelah receipt, engine membaca ulang posisi on chain dan membandingkan dengan yang diharapkan. Selisih memicu alarm dan menghentikan otomasi. |
| R1 | Transfer penuh jalur LP | Keberhasilan G4 dan G5 pada calldata LP membuktikan manager dan pool menerima jumlah penuh. Transfer tax yang memotong jalur ini memblokir modal masuk. |
| R2 | Jual kecil | Quoter menentukan minimum keluaran, lalu router V3/V4 menjalankan jual kecil lewat `eth_call` dari wallet dengan allowance sebenarnya. Revert dan hasil nol memblokir modal masuk. |

Tambahan uji diferensial: jalur V3 dan V4 diberi input range yang sama, lalu jumlah token yang mereka hitung dibandingkan satu sama lain dan dengan quoter resmi. Ketiganya harus cocok dalam toleransi satu wei per basis. Ini menangkap kesalahan konversi tick dan pembulatan yang lolos dari unit test biasa.

### 6.5 Nonce, gas, dan kecepatan

Robinhood Chain memakai sequencer first come first served tanpa lelang priority fee. Artinya yang menang adalah yang paling cepat sampai, bukan yang membayar paling mahal. Ini justru alasan utama engine ditulis dalam Rust.

- Koneksi RPC persisten lewat WebSocket, tanpa handshake ulang per transaksi.
- Nonce dikelola lokal dengan pelacakan in flight, bukan menanyakan `eth_getTransactionCount` tiap kali.
- Transaksi disusun dan ditandatangani lebih awal saat sinyal mendekati ambang, lalu tinggal dikirim saat user klik. Target dari klik sampai byte terkirim di bawah 20 ms.
- Estimasi gas dicache per bentuk transaksi, dengan margin, dan hanya diestimasi ulang saat bentuknya berubah.
- Backoff dan failover ke RPC cadangan jika RPC publik menolak.

### 6.6 Aksi lain

| Aksi | Isi |
| --- | --- |
| Tambah likuiditas | `increaseLiquidity` pada range yang sama |
| Rebalance | Tutup, kumpulkan fee, buka di range baru, dalam satu transaksi atomik |
| Klaim fee | `collect` di V3, `TAKE_PAIR` di V4. Opsi swap otomatis ke ETH setelah klaim |
| Tutup | Tarik seluruh likuiditas, kumpulkan fee, burn posisi |
| Tutup dan keluar | Tutup lalu swap sisa token ke ETH dalam satu transaksi |

---

### 6.7 Zap satu sisi

User cukup memegang satu token. Engine menghitung berapa bagian yang harus ditukar supaya sisanya tepat memenuhi rasio range.

**Penyelesaiannya iteratif, dan memakai bagi dua bukan Newton.** Bagian 6.1 semula menyebut Newton dua sampai tiga putaran. Newton butuh turunan yang bentuknya bergantung pada model dampak harga, dan model itu berbeda antara V3 dan V4 berhook. Bagi dua tidak butuh turunan sama sekali, dan pada fungsi yang monoton ia pasti konvergen. Empat belas putaran sudah cukup pada pool sungguhan.

**Arah pencarian ditentukan sisi mana yang mengikat, bukan sisa mana yang lebih besar.** Ini kesalahan yang sempat terjadi dan berhasil ditangkap saat pengujian terhadap pool nyata. Membandingkan sisa token0 dengan sisa token1 secara langsung tidak sah, karena keduanya token berbeda dengan desimal berbeda. Pada FIG/USDG, sisa 77.380 satuan USDG berdesimal enam ternyata jauh lebih bernilai daripada 12.325.754 satuan FIG berdesimal delapan belas, tapi perbandingan angka mentah menyatakan sebaliknya.

Yang benar adalah membandingkan likuiditas yang bisa dibentuk tiap sisi seorang diri. Sisi yang menghasilkan lebih sedikit adalah yang mengikat, dan itu perbandingan tanpa satuan sehingga aman terhadap selisih desimal.

Selisihnya bukan hal kecil. Pada pool FIG/USDG dengan modal yang sama, versi yang benar menghasilkan **8,2 persen likuiditas lebih banyak**, dan menyisakan modal menganggur mendekati nol alih alih lima belas persen.

**Kutipan selalu datang dari rantai.** V4Quoter dipanggil lewat `eth_call`, dan hasilnya tidak pernah ditebak. Perlu dicatat bahwa `quoteExactInputSingle` tidak bertanda `view`: ia benar benar menjalankan swap lalu revert untuk membatalkannya, dan hasilnya dibaca dari data revert. Karena itu ia tidak boleh dikirim sebagai transaksi.

Titik pencarian Zap yang tidak saling bergantung dikutip maksimal enam belas
sekaligus. Semua jawaban tetap berasal dari V4Quoter, dan jumlah final yang
akan ditukar memakai kutipan langsung yang sudah ada atau dikutip sekali lagi
jika titiknya belum pernah diminta. Titik yang gagal sekali
dicoba ulang berurutan. Paralelisme ini hanya mengubah waktu tunggu, bukan
sumber harga atau batas slippage. Izin PositionManager belum dibaca sebelum
Zap karena saldo akhirnya belum ada; izin itu dihitung sesudah swap ketika
rencana disusun ulang.

**Range satu sisi tetap memperhatikan aset sumber.** Di bawah range, posisi
hanya menerima token0; di atas range, posisi hanya menerima token1. Jika user
memegang sisi yang benar, Zap tidak diperlukan. Jika user memilih aset di sisi
sebaliknya, seluruh jumlah sumber dikutip dan ditukar ke aset yang diterima
range. Tanpa cabang ini, perencana berhenti pada likuiditas nol sebelum Zap
sempat memperbaiki komposisinya.

Kutipan yang menjawab nol diperlakukan sebagai kegagalan, bukan sebagai jawaban. Nol yang diterima sebagai jawaban sah akan menghasilkan pembagian modal yang sepenuhnya salah.

**Zap dijalankan sebagai transaksi terpisah, bukan atomik.** Bagian 5.1 menyebut zap LPZenith bersifat atomik, dan itu memang lebih baik. Di rantai ini tidak bisa ditiru untuk V4: swap berjalan lewat UniversalRouter sedangkan posisi dibuat lewat PositionManager, dan tidak ada kontrak yang bisa melakukan keduanya dalam satu panggilan.

Konsekuensinya harga bisa bergerak di antara kedua transaksi. Yang menahan itu adalah `amountOutMinimum`, dan kalau pergerakannya melewati batas slippage, swapnya gagal tanpa memindahkan apa pun alih alih menghasilkan posisi timpang.

Setelah swap berhasil, panel membuang rencana lama supaya jumlahnya dihitung ulang dari saldo yang baru. Menyusun posisi dari angka sebelum swap akan meleset persis sebesar hasil swapnya.

Sebelum signing, engine juga menjalankan calldata Zap yang persis sama lewat
`eth_call` dengan router, pengirim, calldata, dan `msg.value` yang akan dipakai.
Ini membuktikan allowance, transfer, hook, serta `amountOutMinimum` bersama.
Estimasi gas pada pintu kirim mengulang simulasi karena state dapat berubah.

**Ambang kelayakan.** Zap hanya ditawarkan kalau modal yang akan menganggur melebihi satu persen. Di bawah itu, biaya gas dan slippage sebuah swap lebih besar daripada perbaikannya.

### 6.8 Encoder swap UniversalRouter

UniversalRouter di rantai ini adalah fork, dan bentuk sebenarnya dibaca dari transaksi swap nyata `0xf15c5f65d6f0372698805158351bf8c13de3578692a721a06094a365449fcd86`, bukan dari kode sumber yang tidak kita punya.

Struktur luarnya `execute(bytes commands, bytes[] inputs, uint256 deadline)`, dengan satu perintah `V4_SWAP` bernilai `0x10`. Isinya rangkaian tiga aksi:

| Kode | Aksi |
| --- | --- |
| `0x06` | SWAP_EXACT_IN_SINGLE |
| `0x0c` | SETTLE_ALL |
| `0x0f` | TAKE_ALL |

**Field tambahan fork ditemukan lewat aritmetika offset.** Pada transaksi itu, offset `hookData` bernilai `0x140`, yaitu sepuluh kata dari awal struct. Struct Uniswap standar hanya sembilan kata: PoolKey lima kata, lalu `zeroForOne`, `amountIn`, `amountOutMinimum`, dan offset `hookData`. Selisih satu kata itulah `minHopPriceX36`, dan nilainya nol pada transaksi tersebut.

Kalau encoder memakai struct standar, `hookData` akan mendarat di posisi yang salah dan kontrak membaca sampah. Uji golden vector membangun ulang seluruh calldata transaksi itu, 1124 byte, dan membandingkannya byte per byte, sehingga kekeliruan seperti itu tertangkap saat build bukan saat dana bergerak.

`minHopPriceX36` diisi nol kecuali ada alasan yang dipahami. Menebak nilai lain untuk field yang perilakunya belum diketahui lebih berbahaya daripada tidak memakainya.

**Izin swap terpisah dari izin posisi.** UniversalRouter menarik token lewat Permit2 juga, dan izin ke PositionManager tidak berlaku untuknya. Panel menampilkan langkah izin router sebagai bagian dari daftar yang sama.

### 6.9 Menemukan posisi milik wallet

Kedua protokol butuh cara yang sama sekali berbeda, dan perbedaannya ditemukan lewat pemeriksaan bytecode, bukan dari dokumentasi.

| Kontrak | `balanceOf` | `tokenOfOwnerByIndex` | `totalSupply` | `tokenByIndex` |
| --- | --- | --- | --- | --- |
| NonfungiblePositionManager v3 | ada | ada | ada | ada |
| PositionManager V4 | ada | tidak ada | tidak ada | tidak ada |

V3 mengimplementasikan ERC721Enumerable sepenuhnya, jadi posisinya dihitung lewat `balanceOf` lalu diambil satu per satu dengan `tokenOfOwnerByIndex`. Semuanya muat dalam dua permintaan.

V4 tidak. Satu satunya cara adalah memindai event `Transfer` yang menuju wallet itu, lalu membaca ulang kepemilikannya. Hasil pemindaian disimpan supaya pemuatan berikutnya hanya melihat blok baru.

**Yang disimpan hanya daftar calon, bukan kepemilikannya.** NFT posisi bisa berpindah kapan saja, dan cache yang menyimpan kepemilikan akan menampilkan posisi milik orang lain. Itu kesalahan yang paling membingungkan yang bisa dilakukan alat seperti ini.

### 6.10 Fee yang belum diklaim

Field `tokensOwed0` dan `tokensOwed1` pada posisi V3 hanya diperbarui saat posisinya disentuh, yaitu saat likuiditas berubah atau fee diklaim. Posisi yang dibuka lalu didiamkan akan melaporkan nol selamanya, padahal fee-nya terus bertambah. Menampilkan nol berarti user mengira posisinya tidak menghasilkan apa apa.

Karena itu keduanya dibaca dengan cara lain, dan caranya berbeda per protokol:

| Protokol | Cara | Kenapa |
| --- | --- | --- |
| V3 | Simulasikan `collect` lewat `eth_call` dari pemiliknya | Menjalankan perhitungan yang sama persis dengan yang akan terjadi sungguhan |
| V4 | `StateView.getFeeGrowthInside` dikurangi `getPositionInfo` lalu dikali likuiditas | V4 tidak punya `collect` yang bisa disimulasikan |

Simulasi `collect` tidak bisa lewat Multicall3, karena fungsi itu menuntut pemanggilnya adalah pemilik NFT, dan lewat multicall pemanggilnya menjadi multicall.

**Pengurangan pertumbuhan fee wajib membungkus, bukan jenuh.** Kontrak Uniswap memakai aritmetika `unchecked`, dan nilainya memang dirancang melampaui batas lalu berputar. Memakai pengurangan yang jenuh akan melaporkan fee nol persis pada posisi yang paling lama berjalan, yaitu yang fee-nya paling besar.

Pada V4, pemilik posisi di dalam PoolManager adalah PositionManager, bukan wallet user, dan `salt` adalah token id. Itu yang membedakan posisi dua wallet pada range yang sama persis, dan salah di sini menampilkan fee milik orang lain.

### 6.11 Akuntansi hasil

Fee kotor menipu. Posisi yang mengumpulkan fee besar bisa tetap rugi setelah impermanent loss, gas, dan slippage. Karena itu komponennya selalu dipisah, dan `net` selalu dihitung dari selisihnya, tidak pernah disimpan terpisah.

**Impermanent loss diukur terhadap memegang, bukan terhadap modal awal.** Keduanya dinilai pada harga sekarang. Itu yang membuatnya benar benar mengukur akibat menjadi LP: kalau posisinya masih memegang persis yang disetor, hasilnya nol berapa pun harganya. Mengukurnya terhadap modal awal akan mencampur perubahan harga dengan kerugian menjadi LP, dan membuat strategi yang bagus terlihat buruk saat pasar turun.

**Satuannya token kutipan pool, bukan dolar.** Nilai dinyatakan dalam sisi yang merupakan kutipan yang dikenal rantai ini, yaitu ETH asli atau entri `[tokens]` di chain.toml. Kalau kedua sisinya memecoin, PnL-nya tidak ditampilkan sama sekali.

Itu keputusan yang disengaja. Menghitungnya dalam satuan terkecil token1 memang konsisten secara aritmetika, tapi kalau token1 ternyata memecoin, hasilnya adalah angka raksasa dalam satuan memecoin yang terbaca seperti dolar. Angka yang salah satuannya lebih berbahaya daripada tidak ada angka.

**Posisi tanpa catatan tetap dihitung dan ditandai.** Posisi yang dibuka di luar Rangefinder tidak punya catatan berapa yang dulu disetor. Untuk posisi seperti itu, yang disetor dianggap sama dengan yang ada sekarang, sehingga impermanent loss-nya nol, dan hasilnya ditandai perkiraan. Menyembunyikannya justru melanggar permintaan user bahwa posisi dari luar dikelola penuh.

### 6.12 Notifikasi

Toast muncul di pojok kanan atas halaman gmgn, sesuai PRD 3.7. Yang perlu ditegaskan adalah bagaimana ia tetap sampai saat panel tertutup.

**Content script menyambung sendiri ke engine.** Kalau jalurnya lewat side panel, notifikasi berhenti persis saat ia paling dibutuhkan, karena panel yang tertutup adalah keadaan paling umum: user menatap chart, bukan panel. Koneksi ini hanya mendengarkan, dan tidak pernah mengirim perintah yang mengubah keadaan. Halaman gmgn adalah lingkungan pihak ketiga.

**Pendinginan berlaku per pasangan kode dan subjek, bukan per kode saja.** Dua posisi berbeda yang sama sama keluar range adalah dua kabar berbeda. Pendinginan per kode akan menyembunyikan yang kedua, dan posisi itu bisa jadi justru yang paling besar.

**Riwayat menyimpan yang tertahan juga.** Toast boleh ditahan, catatan tidak. Catatan itulah yang menjelaskan apa yang terjadi saat user tidak menatap layar, dan kejadian yang tertahan ditandai supaya jelas kenapa ia tidak muncul.

**Nada dibangkitkan, bukan berkas.** PRD 3.7 menyebut berkas audio pendek yang dibundel. Oscillator lebih baik untuk keperluan ini: tidak ada berkas yang dimuat, tidak ada jeda pada bunyi pertama, ukurannya nol, dan bunyinya dibedakan per tingkat hanya dengan mengubah jumlah ketukan. Audio dinyalakan pada interaksi pertama user di halaman gmgn, karena Chrome memblokirnya sampai saat itu.

**Yang diberitahukan adalah perubahan, bukan keadaan.** Memberi tahu setiap kali posisi berada di luar range akan membanjiri layar setiap kali daftar dimuat ulang. Yang layak diberitahukan adalah baru saja keluar, atau baru saja kembali masuk. Pemuatan pertama tidak pernah memicu notifikasi.

### 6.13 Pemindaian pool ramai

Ini sumber daftar kandidat tab Signals. Enam keputusan di sini menyimpang dari
3.4 atau memperjelasnya, dan semuanya datang dari menjalankan alatnya terhadap
rantai sungguhan.

**Sumbernya rantai, bukan API gmgn.** 3.4 semula membayangkan pemindaian lewat
content script gmgn dengan batas satu permintaan per dua detik. Volume swap on
chain lebih baik untuk keperluan ini, dan alasannya bukan sekadar keandalan:
yang kita cari adalah pool yang membayar fee, dan yang membayar fee adalah swap
yang benar benar terjadi di pool itu. Peringkat gmgn mengukur perhatian pasar,
yang berkorelasi tapi bukan hal yang sama. Batas laju gmgn juga menjadi tidak
relevan, karena sesi user tidak disentuh sama sekali.

**Dua penolakan RPC yang obatnya berlawanan.** `logs matched by query exceeds
limit` berarti jawabannya kebanyakan dan obatnya mempersempit jendela.
`429 Too Many Requests` berarti permintaannya kerapatan dan obatnya menunggu.
Memperlakukan keduanya sama memperburuk salah satunya: mempersempit saat
dibatasi laju justru menambah jumlah permintaan. Keduanya pernah terjadi
berurutan pada dua percobaan yang berdekatan, dan itulah kenapa keduanya
ditangani satu helper bersama di `chain::logs`.

**Jendela pemindaian pendek, jendela volume panjang.** Pemindaian harus membaca
setiap swap di rantai untuk menemukan pool mana yang ramai, jadi jendelanya
beberapa menit. Jendela sependek itu tidak boleh dipakai memproyeksikan hasil
setahun: pengalinya ratusan ribu, dan satu ledakan volume dua menit pernah
menghasilkan proyeksi 7.645.797 persen. Setelah poolnya diketahui, lognya bisa
disaring menurut poolId, sehingga jendela bergulir 24 jam dibaca mulai dari
potongan 216.000 blok, kira kira enam jam. Pesan `log query timed out` atau batas
jumlah hasil memperkecil potongan secara adaptif, sedangkan 429 menunggu tanpa
memperbanyak permintaan. Semua potongan diteruskan sampai batas bawah periode
semula, jadi batas RPC tidak diam diam mengubah 24 jam menjadi jendela pendek.
Jendela yang benar benar terpakai selalu ikut hasilnya, karena volume tanpa
jendela tidak bisa dibandingkan.

**Rasio efisiensi mengukur kelurusan jalan, bukan panjangnya.** Event `Swap`
membawa tick, sehingga deret harga tersedia dari sumber paling berwenang tanpa
satu pun permintaan tambahan, dan rasio efisiensi Kaufman bisa dihitung darinya.
Tapi rasio itu saja tidak cukup: pool yang hanya berpindah dua tick menghasilkan
rasio 1,00 karena dua titik selalu lurus, dan hasil sungguhan pernah memberi
label `LP Jual Naik` pada perpindahan 0,0 persen. Arah baru disebut kalau
perpindahan bersihnya melewati satu persen. Di atas 60 persen, arahnya juga
tidak disebut, karena range yang dipasang setelah gerakan sebesar itu akan
langsung tertinggal.

**Proyeksi hasil memakai modal acuan, bukan pembagian lurus.** Bagian LP atas
fee adalah `modal / (kedalaman + modal)`. Bentuk `modal / kedalaman` hanya sah
kalau modalnya jauh lebih kecil daripada poolnya, dan pada pool tipis ia
meledak: kedalaman 296 dengan volume 35.500 menghasilkan 1.890.130 persen,
karena rumusnya diam diam mengandaikan kita menguasai seluruh pool tanpa
mengencerkannya. Modal acuan disebutkan bersama angkanya, sehingga hasilnya
menjadi pernyataan yang bisa diperiksa: sekian persen kalau menaruh sekian.

Modal itu sendiri diukur pada rentang yang harga benar benar lewati, bukan pita
tetap, dan **kedua kaki posisi ikut dihitung**. Menghitung satu kaki saja
membuat modalnya terbaca nol setiap kali harga kebetulan berada di ujung
rentangnya, karena kaki di sisi itu memang menyusut sampai nol.

**Kutipan dinilai dari alamat, tidak pernah dari simbol.** Simbol ditentukan
penerbit token, jadi siapa pun bisa menerbitkan token bersimbol USDG, dan
pemindaian sungguhan memang menemukannya. Baris yang alamat kutipannya tidak
ada di registry tidak diberi angka uang sama sekali, dan panel menandainya
dengan tanda tanya di depan simbol. Masalah yang sama pernah muncul pada FIG,
yang punya pool terhadap 32 token berbeda dan sebagian besar tiruan.

### 6.14 Distribusi likuiditas

Ditambahkan setelah user menunjukkan antarmuka MMT Finance sebagai acuan.
Bagian yang diambil bukan tampilannya, melainkan pertanyaan yang dijawabnya.

**Kenapa ini yang paling bernilai dari acuan itu.** Porsi fee yang kita terima
bukan ditentukan oleh besarnya modal kita, melainkan oleh perbandingan modal
kita terhadap likuiditas lain YANG AKTIF di pita tempat harga bergerak. Dua
range selebar sama bisa berbeda hasilnya berkali lipat hanya karena yang satu
ditaruh di pita yang sudah padat dan yang lain di pita yang masih lengang.
Tanpa gambar ini, satu satunya cara mengetahuinya adalah menaruh dana lalu
melihat hasilnya, dan itu cara belajar yang paling mahal.

**Sumbernya `liquidityNet`, bukan angka jadi.** Likuiditas tidak disimpan per
pita. Yang tersimpan adalah berapa banyak likuiditas yang mulai atau berhenti
aktif tepat di setiap tick yang terinisialisasi. Likuiditas sebuah pita didapat
dengan berjalan dari tick sekarang ke luar sambil menjumlahkannya. Tick mana
saja yang terinisialisasi diketahui dari bitmap, satu kata untuk 256 tick
terkompresi. `getTickBitmap` dan `getTickLiquidity` sudah dibuktikan ada di
bytecode StateView yang terpasang lewat `cargo xtask v4probe`, bukan disalin
dari repo Uniswap lalu diandaikan sama.

**Tinggi batang dinyatakan dalam token kutipan, bukan dalam satuan `L`.** Angka
`L` tidak punya satuan dan tidak bisa dibandingkan antar pool, sedangkan modal
yang tertanam di sebuah pita bisa. Kedua kaki posisi ikut dihitung, dengan
alasan yang sama seperti di 6.13.

**Pita penutup dipotong di batas tampilan, bukan di tepi kata bitmap.** Satu
kata bisa mencakup ratusan persen gerakan harga, dan pita sepanjang itu akan
menghasilkan batang tertinggi di seluruh histogram semata mata karena lebarnya.
Percobaan pertama memang begitu, dan batang terpanjangnya tidak berarti apa apa.

**Harga sekarang selalu tercakup tepat satu pita.** Pool yang seluruh tick
terinisialisasinya berada di satu sisi harga pernah menghasilkan empat pita yang
tidak satu pun memuat harga sekarang. Likuiditas di sisa rentang bukan nol, ia
justru yang sedang aktif, dan pita penutup di kedua ujung yang menutupinya.

**Batas pita dinamai menurut harga, bukan menurut tick.** Pada pool yang
memecoinnya token1, tick yang lebih besar berarti harga yang lebih rendah.
Menamainya menurut tick membuat histogram tergambar terbalik tanpa satu pun
error muncul, kekeliruan yang sama seperti yang pernah terjadi pada batas range.

**Panel menggambarnya mendatar, bukan tegak seperti acuannya.** Side panel
lebarnya 320 sampai 480 piksel. Sumbu harga yang tegak memberi setiap pita satu
baris penuh sehingga label harganya terbaca, sedangkan sumbu mendatar akan
menyisakan beberapa piksel per pita dan labelnya hilang.

**Distribusi tidak ikut `range_view`.** Ia butuh dua perjalanan ke rantai,
sekitar setengah detik, sedangkan `range_view` dikirim ulang setiap kali user
menggeser garis. Bentuk pool tidak berubah secepat harganya, jadi satu
pembacaan per pool menemani banyak geseran.

**Preset lebar range mengikuti acuan**, yaitu kurang lebih 0,2 sampai 20 persen
ke tiap arah. Gerakan satu persen pada harga adalah gerakan satu persen pada
market cap, jadi preset dinyatakan sebagai kelipatan terhadap market cap
sekarang dan tidak ada konversi satuan yang bisa meleset.

### 6.15 Penyesuaian antarmuka setelah dipakai

Perubahan berikut datang dari user setelah memakai panelnya, dan sebagian
menyimpang dari bagian 3 yang ditulis sebelum apa pun berjalan.

**Garis range tidak digambar ulang selagi digeser.** Menggeser garis memicu
peristiwa pada setiap gerakan tetikus. Tiap gerakan dikirim ke engine, dibulatkan
ke grid tick, lalu dikirim balik, dan panel menggambar ulang garis di posisi
hasil pembulatan itu. Akibatnya garis tersentak ke belakang di bawah kursor yang
masih bergerak. Sekarang chart yang memegang posisi selama geseran berlangsung,
dan panel menggambar ulang sekali setelah geserannya berhenti, sehingga
pembulatan tetap terlihat tapi hanya di akhir.

**Batas range dijepit ke tick yang masih kelipatan spacing.** `MIN_TICK` dan
`MAX_TICK` belum tentu kelipatan spacing, dan menjepit ke angka itu apa adanya
menghasilkan tick di luar grid yang ditolak Uniswap. Kekeliruan ini tidak pernah
muncul selama range yang diminta sempit, dan langsung muncul begitu preset
range penuh ditambahkan.

**Tick spacing tidak lagi ditampilkan di daftar pool.** Ia menentukan grid
tempat batas range dibulatkan, tapi akibatnya sudah terlihat langsung sebagai
selisih harga di kartu Range, dan angkanya sendiri tidak mengubah keputusan
memilih pool. Yang menggantikannya adalah APR, TVL, dan volume, yang memang
menentukan pilihan.

**TVL sebuah pool hanya mencakup pita di sekitar harga**, selebar sepuluh persen
ke tiap arah. Likuiditas yang jauh dari harga tidak menerima fee apa pun, dan
memasukkannya akan membuat pool tebal terlihat lebih baik daripada yang
sebenarnya.

**TVL sebuah pool dijumlahkan dari distribusinya, bukan ditaksir.** Taksiran
memakai likuiditas di satu tick saja pernah melaporkan 0,82 untuk pool yang
isinya kira kira 360, karena likuiditas berubah di setiap tick yang
terinisialisasi dan satu titik tidak mewakili seluruhnya. Seluruh pool dalam
satu kelompok kutipan dibaca bersama, sehingga biayanya tetap dua perjalanan ke
rantai berapa pun jumlah poolnya.

**Garis yang digeser user tidak pernah digambar ulang.** Percobaan pertama
menunda penggambaran sampai geserannya berhenti, dan hasilnya garis tetap
meloncat, hanya sekali di akhir alih alih terus menerus. Chart sudah menaruh
garis persis di tempat kursor dilepas, dan hasil pembulatan ke grid tick tetap
terbaca sebagai angka di kartu Range. Penggambaran ulang hanya berlaku untuk
perubahan yang bukan dari chart: preset, custom, dan ganti pool.

**Penggambaran garis berantre, satu per satu.** `createShape` mengembalikan
Promise. Kalau permintaan kedua datang selagi permintaan pertama masih menunggu
shape pertamanya dibuat, penghapusan miliknya berjalan sebelum id shape itu
sempat dicatat. Shape yang tidak tercatat tidak pernah dihapus, dan garisnya
tertinggal di chart. Gejalanya tiga garis: satu batas atas menjadi yatim,
sehingga ada dua garis berwarna sama. Permintaan yang sudah disusul permintaan
lain dilewati, karena menggambarnya hanya untuk langsung ditimpa berarti dua
kali kerja dan satu kedipan.

Shape dibuat dengan `disableSave`, jadi yang terlanjur yatim hilang sendiri
begitu halaman gmgn dimuat ulang.

**Panel menggambar ke tab gmgn yang sedang aktif.** Kalau user membuka beberapa
halaman gmgn sekaligus, menggambar di tab yang tidak dilihat terlihat persis
seperti garisnya tidak pernah muncul.

**Pool tanpa tick terinisialisasi di jendela tetap punya modal.** Likuiditas
yang sedang aktif membentang dari tick di luar jendela, jadi modalnya nyata dan
tetap menerima fee. Percobaan sebelumnya berhenti lebih awal saat daftar
panggilannya kosong, dan setiap pool seperti itu melaporkan TVL nol, yang
terbaca sebagai pool kosong.

**Statistik diukur untuk kelompok kutipan yang sedang dipilih**, bukan hanya
yang teratas. Berpindah pasangan tetap tidak menyentuh jaringan, dan
pengukurannya menyusul lewat kiriman kedua bersama saldo. Hasilnya disimpan ke
keadaan aktif supaya tidak diukur ulang setiap kali user bolak balik.

**Kiriman `range_view` diabaikan menurut waktu, bukan menurut penanda.** Satu
geseran garis memicu puluhan peristiwa, dan setiap peristiwa menghasilkan satu
kiriman. Penanda benar salah hanya menahan kiriman pertama, sehingga kiriman
kedua dan seterusnya tetap menggambar ulang garis di posisi hasil pembulatan.
Jendela waktu menahan seluruhnya, berapa pun jumlahnya, dan sengaja melebihi
lama satu perjalanan ke engine supaya kiriman yang masih di jalan saat kursor
dilepas ikut tertahan.

**Peringatan kurs hanya berlaku untuk kutipan ETH.** Field `eth_usd` sebenarnya
harga satu token KUTIPAN dalam USD, dan namanya menyesatkan. Pada pool
berpasangan stablecoin angkanya kira kira satu, dan itu benar. Memeriksanya
terhadap rentang harga ETH memunculkan peringatan pada setiap pool USDG yang
sebenarnya sehat. Nilainya sendiri tidak lagi ditampilkan di panel, karena ia
tidak mengubah keputusan apa pun: yang menentukan posisi adalah batas range,
dan batas itu sudah tampil dalam tiga satuan.

**Histogram diringkas di panel, bukan di engine.** Pool V3 yang ramai bisa punya
ratusan tick terinisialisasi, dan satu baris per pita menghasilkan dinding
batang yang tidak terbaca. Panel menggabungkan pita yang berdekatan sampai
paling banyak dua puluh empat baris, menjumlahkan nilainya sehingga total yang
tergambar tetap sama, dan tidak pernah menggabung pita yang memuat harga
sekarang. Engine tetap mengirim data lengkap, karena meringkas di sana berarti
membuang informasi yang tidak bisa dikembalikan.

Histogram memiliki sumbu harga logaritmis, handle bawah merah dan atas hijau.
Drag mengirim perubahan melalui `set_range` saat dilepas, lalu engine melakukan
snap tick. Tombol panah mengubah satu tick spacing, Shift sepuluh spacing.
Zoom 1x, 2x, 4x, 8x beserta reset hanya mengubah jendela gambar, tidak mengubah
range atau mengarang likuiditas di luar data yang sudah dibaca. Setiap commit
range membatalkan rencana setoran lama. Pengujian browser fixture memakai
komponen produksi pada lebar 320, 360, dan 480 piksel.

**Input setoran menyusun rencana secara otomatis setelah stabil 250 ms.**
Mengubah nominal, aset setoran, zap, pool, atau tick membatalkan rencana lama,
lalu panel meminta rencana baru tanpa tombol antara. Sidik jari input menahan
permintaan identik agar jawaban engine tidak membentuk loop. Approval, zap,
dan pembukaan posisi tetap berupa klik terpisah karena masing masing adalah
aksi on-chain.

Pada setoran dua aset dan penambahan likuiditas, user cukup mengisi salah satu
token. Engine menurunkan sisi pasangan dari rasio likuiditas range yang sama
dan panel mengisi kotak lain setelah quote kembali. Hitungannya memakai integer
yang sama dengan calon calldata, bukan perkiraan harga di panel.

**Preset range menandai dirinya dari lebar yang sebenarnya berlaku**, bukan dari
tombol mana yang terakhir ditekan. Engine membulatkan batas ke grid tick,
sehingga lebarnya bisa bergeser sedikit, dan menandai tombol menurut ingatan
panel akan berbohong setelah pembulatan.

**Perkiraan biaya gas ditampilkan dalam token gas, bukan jumlah unit.** Jumlah
unit tidak bisa dinilai tanpa harga gas, dan harga gas berubah dari blok ke
blok, jadi ia dibaca saat rencana disusun dan ikut dikirim supaya angkanya bisa
diperiksa ulang. Simulasi mint, pencarian jumlah Zap, dan pembacaan harga gas
berjalan bersamaan karena ketiganya tidak saling bergantung. Jumlah yang dipakai
Zap berasal dari hitungan likuiditas murni yang sama dengan pipeline mint.

**Wallet dibuat dari satu isian.** User menempelkan passphrase atau kunci privat
ke kotak yang sama, dan panel mengenalinya dari bentuknya. Kotak kedua hanya
muncul kalau yang ditempel ternyata kunci privat, karena keystore tetap harus
dienkripsi dan kunci privat bukan passphrase.

**Nama event menentukan topicnya, dan salah nama tidak memunculkan error.**
Event `Swap` V3 sempat dideklarasikan sebagai `SwapV3` supaya tidak bentrok
dengan padanan V4. Topic dihitung dari nama beserta tipe argumennya, sehingga
penyaringnya tidak pernah cocok dengan apa pun di rantai, dan akibatnya nol log
yang terbaca sebagai pool yang sepi. Bentrok nama diselesaikan dengan menaruhnya
di dalam interface tersendiri.

**Volume diukur per protokol.** V4 memancarkan `Swap` dari PoolManager dengan
poolId sebagai topic, V3 dari kontrak pool masing masing tanpa pengenal apa pun
selain alamat log. Keduanya tetap satu permintaan untuk semua pool, karena baik
`topic1` maupun `address` menerima daftar nilai.

**TVL dijumlahkan pada jendela yang sama dengan histogram.** Percobaan pertama
memakai delapan kata bitmap, yang pada tick spacing 200 mencakup empat ratus
ribu tick. Likuiditas yang dijumlahkan sejauh itu berhenti menggambarkan apa
pun: tick penutupnya berada di luar jendela sehingga tidak pernah berkurang, dan
satu pool V3 dilaporkan berisi 50 juta WETH, jauh melebihi seluruh market cap
tokennya.

**Sisi memecoin dinilai pada harga sekarang, bukan harga tengah pita.** TVL
berarti nilai pasar sekarang, bukan nilai pada harga hipotetis di masa depan.
Menilainya pada harga tengah pita ikut menyumbang ledakan di atas.

**Volume yang belum bisa diukur dilaporkan kosong, bukan nol.** Nol berarti tidak
ada perdagangan, dan itu kesimpulan yang sangat berbeda dari belum terukur.
Keduanya terlihat sama persis di layar tanpa pembedaan ini, dan user membaca nol
sebagai data yang tidak muncul.

**Endpoint RPC harus mengizinkan pemindaian log yang lebar.** Alchemy paket
gratis membatasi `eth_getLogs` pada sepuluh blok, sedangkan penemuan pool V4
menuntut pemindaian jutaan blok. Batas LEBAR rentang berbeda dari batas JUMLAH
hasil: yang kedua bisa diatasi dengan menyempitkan jendela, yang pertama tidak,
karena menyempitkannya sampai sepuluh blok berarti ratusan ribu permintaan.
Keduanya kini dibedakan, dan batas lebar dilaporkan sebagai kesalahan pemilihan
endpoint, bukan sebagai kegagalan pemindaian yang tidak menjelaskan apa apa.

**Statistik pool ikut menentukan apakah keadaan token dikirim ulang.** Tahap
cepat memakai kunci pool dari cache dan tidak mengukur apa apa, sehingga daftar
poolnya sering sama persis dengan tahap penuh. Sebelum ini keduanya dianggap
identik, kiriman kedua ditahan, dan APR, TVL, serta volume tidak pernah sampai
ke panel meski sudah terhitung.

**Pool token akhirnya dibandingkan dengan periode yang sama seperti halaman
token Uniswap.** Snapshot awal memakai sekitar tiga jam agar daftar siap dalam
tujuh detik, lalu volume dan APR pool diganti dengan 24 jam bergulir. Label
volume selalu membawa jendela yang benar. Setelah statistik tersedia, urutan
pool memakai APR marginal yang dihitung dari
volume, fee tier, dan TVL. Ukuran setoran aktual beserta pengencerannya dihitung
lagi oleh planner setelah user mengisi amount. Peringkat ini membantu memilih
pool relatif terhadap pool lain dan tidak menjamin posisi akan untung, karena
gas, pergerakan harga, waktu di dalam range, serta slippage tetap menentukan
hasil akhirnya.

**Distribusi likuiditas berlaku untuk V3 maupun V4.** Keduanya menyimpan bitmap
dan likuiditas tick dengan bentuk yang sama, hanya tempatnya berbeda: V3 di
kontrak poolnya sendiri, V4 di PoolManager lewat StateView. Sebelum ini seluruh
jalur mengandaikan V4, dan setiap pool V3 selalu melaporkan distribusi gagal
dibaca.

**Settings menjadi tab tersendiri**, bukan bagian tab Range. Isinya diatur sekali
lalu jarang disentuh, sedangkan tab Range dipakai setiap kali menaruh posisi.

**Kabar dari engine tampil di panel**, bukan hanya di konsol. Aksi seperti
mengganti endpoint RPC sebelumnya tidak memberi tanda apa pun di layar, sehingga
user tidak tahu apakah permintaannya sampai.

**Setoran mengikuti tata letak acuan MMT Finance.** Saat Zap aktif, satu kotak
menampilkan aset sumber, jumlah, nilai USD, saldo wallet, dan ticker yang dapat
ditekan untuk mengganti sisi. Saat Zap mati, kedua kotak tampil karena user
perlu mengisi kedua aset. Header hanya berisi judul dan sakelar Zap, tanpa ikon
range. Logo token tidak direka dari huruf ticker dan tidak dicari dari jaringan;
selama protokol belum membawa aset logo terverifikasi, token pill hanya memakai
ticker. Di bawah input hanya ada satu tombol utama. Approval, zap, simulasi,
gas, gate, dan penyusunan ulang rencana dikerjakan engine di belakang layar.

Saldo wallet ikut di `token_state`. `None` berarti wallet belum dibuka, bukan
saldo nol: yang pertama menuntun ke membuka wallet, yang kedua ke membeli
tokennya dulu. Saat user berpindah pasangan, saldo dan statistik awal dimulai
bersamaan pada RPC yang berbeda. Panel menampilkan keduanya sebagai snapshot
pool utuh setelah statistik siap.
Saldo native dan batch ERC20 dibaca bersamaan. Pada pasangan ETH asli, saldo
yang sama dipakai juga sebagai saldo gas supaya tidak ada pembacaan RPC kedua.
Discovery penuh menunggu pembacaan saldo cache selesai, lalu dibatalkan jika
user sudah memilih pairing, pool, atau meminta rencana. Dengan begitu saldo dan
tombol utama tidak berebut RPC dengan hasil discovery yang sudah tidak relevan.
Panel mengulang subscribe saat koneksi pairing siap agar permintaan awal tidak hilang.

**Riwayat notifikasi dihapus dari panel**, menyimpang dari 3.7. Toast di halaman
gmgn tetap berjalan lewat koneksi content script sendiri, dan itulah jalur yang
benar benar dipakai. Daftar riwayat di panel jarang dibuka dan hanya menambah
panjang halaman.

**Endpoint RPC bisa diganti dari panel.** Engine hanya menulis config.toml dan
mengatakan bahwa ia perlu dijalankan ulang. Menukar endpoint saat engine
berjalan berarti ada permintaan yang berangkat ke satu RPC dan jawabannya
ditunggu dari yang lain, dan itu bukan jenis kekeliruan yang layak ditanggung
demi menghindari satu kali start ulang.

**Keterangan panjang di bawah setiap bagian dipendekkan.** Yang tersisa hanya
kalimat yang mengubah keputusan user, bukan yang menjelaskan cara kerja bagian
dalam. Alasan teknisnya tetap ada, tapi tempatnya di komentar kode dan di
dokumen ini.

## 7. Technical Specifications

### 7.1 Arsitektur

```
Chrome
  side panel            Preact + TypeScript + Vite, empat tab
  service worker MV3    routing pesan, manajemen tab, storage, pairing
  content script MAIN   di gmgn.ai: jembatan chart + proxy API gmgn
        |
        |  chrome.runtime messaging, lalu WebSocket
        v
Engine Rust  127.0.0.1:8787
  api         axum HTTP dan WebSocket, broadcast ke panel
  chain       alloy provider, langganan WebSocket RPC, manajer nonce
  pools       penemuan pool V3 dan V4, peta tick lewat TickLens dan StateView
  math        tick, sqrtPrice, likuiditas, impermanent loss
  optimizer   Range Optimizer Monte Carlo
  signals     mesin skor sideways dan playbook
  exec        builder calldata V3, V4, router_rbh, gate validasi, simulasi, kirim
  pnl         cost basis, fee, IL, gas, slippage
  wallet      keystore terenkripsi, Windows Credential Manager
  store       redb lokal dengan pembaruan atomik
```

Alasan pembagian: browser memegang sesi gmgn dan chart, sehingga semua yang butuh identitas gmgn tinggal di browser. Engine memegang kunci, matematika, dan kecepatan, sehingga tidak pernah bergantung pada tab yang bisa ditutup user.

### 7.2 Tumpukan teknologi

| Lapisan | Pilihan | Alasan |
| --- | --- | --- |
| Runtime engine | Rust stable, tokio | Diminta eksplisit. Latensi dan determinisme cocok untuk sequencer 100 ms. |
| Server lokal | axum plus tower-http | Ringan, WebSocket bawaan, ekosistem tokio. |
| Akses chain | alloy | Penerus ethers-rs, tipe kuat, `sol!` macro untuk ABI. |
| Matematika Uniswap | `uniswap-v3-math` sebagai basis, plus modul sendiri | Matematika likuiditas V4 identik dengan V3, satu modul melayani keduanya. |
| Penyimpanan | SQLite lewat sqlx | Satu file, mudah diperiksa, cukup untuk skala satu pengguna. |
| Kripto | aes-gcm, argon2, keyring | Enkripsi keystore dan integrasi Credential Manager. |
| UI | Preact plus TypeScript, Vite | Bundle kecil, penting untuk side panel sempit dan MV3. |
| Observability | tracing plus tracing-subscriber | Log terstruktur, wajib untuk audit transaksi. |

### 7.3 Kontrak pesan

Daftar ini mencakup fase yang belum ditulis kodenya. Kolom Fase menyatakan kapan
pesan mulai ada. `cargo xtask docs-check` memeriksa satu arah: setiap pesan yang
sudah ada di kode wajib tercantum di sini. Sebaliknya tidak diwajibkan, karena
rencana boleh mendahului implementasi.

Versi protokol saat ini 7. Nama field di kabel memakai snake_case. Engine dan
ekstensi harus dimuat ulang bersama ketika kontrak ini berubah.

Panel ke engine:

| Pesan | Isi | Fase |
| --- | --- | --- |
| `get_status` | tanpa isi, minta kiriman status terbaru sekarang juga | 1 |
| `ping` | denyut koneksi saja. Tidak memperpanjang waktu buka wallet karena tab notifikasi pasif juga mengirimnya | 1 |
| `unlock_wallet` | `{ passphrase }`, lewat HTTP bukan WebSocket | 1 |
| `subscribe_token` | `{ address }` | 2 |
| `chart_price` | `{ address, mcapUsd }`, market cap yang sedang ditampilkan chart. Inilah yang mengkalibrasi kurs ETH ke USD | 2 |
| `chart_bars` | `{ address, bars[] }`, deret candle dari chart gmgn. Datang lebih jarang daripada `chart_price` karena isinya ratusan bar dan jawaban detektor tidak berubah dalam hitungan detik | 6 |
| `select_pool` | `{ poolId }`, user memilih pool lain dari daftar | 2 |
| `select_quote` | `{ quote }`, user berpindah ke pasangan token lawan yang lain. Tidak menyentuh jaringan, seluruh kelompok sudah ada di memori sejak token dimuat | 2 |
| `set_range` | `{ lowerMcap, upperMcap }`, user menggeser garis. Batas dalam market cap USD karena itu satuan chart | 2 |
| `set_full_range` | tanpa isi. Pasang range selebar yang diizinkan protokol. Pesan tersendiri, bukan `set_range` dengan angka raksasa: market cap dikali sepuluh pangkat empat puluh menghasilkan harga yang tidak lagi bisa diubah menjadi tick, dan engine menolaknya tanpa ada yang berubah di layar. Batas sebenarnya hanya diketahui engine, karena ia bergantung pada tick spacing pool | 6 |
| `invalidate_plan` | tanpa isi, membatalkan rencana saat setoran sedang disunting | 3 |
| `preview_deposit` | `{ deposit: { side, amount, zap }, request_id }`. Menghitung nominal pasangan dari snapshot pool yang sudah ada di memori tanpa RPC, saldo, allowance, simulasi, atau estimasi gas. Tidak pernah mengaktifkan tombol transaksi | 7 |
| `quote_range` | `{ deposit: { side, amount, zap }, request_id }`. `side` bernilai `meme` atau `quote`, `amount` string desimal token sumber. Engine membaca ulang pool, menghitung sisi pasangan, serta membatasi dana pada anggaran tersebut. Tidak mengirim transaksi | 3 |
| `approve_token` | `{ plan_id, step_id }`. Setujui langkah izin pertama dalam rencana itu. Token, spender, dan jumlah diambil dari langkah tersimpan, termasuk izin UniversalRouter. Setiap klik paling banyak satu transaksi | 3 |
| `run_open_flow` | `{ plan_id }`. Satu persetujuan user untuk alur pembukaan. Engine menjalankan approval satu per satu, menyusun ulang satu kali setelah seluruh approval berhasil, menjalankan zap bila perlu, lalu membuka posisi. Alur dikunci agar pesan ganda tidak mengirim langkah yang sama dua kali | 7 |
| `swap_zap` | `{ plan_id }`. Jalankan swap zap dari rencana itu. Setelah receipt sukses, engine menyusun preview mint dari sisa modal dan hasil swap terukur; mint tetap memerlukan persetujuan terpisah | 4 |
| `open_position` | `{ plan_id }`. Setujui rencana tersimpan yang ID, revisi, serta masa berlakunya masih cocok. Rencana dikeluarkan sebelum pengiriman. Calldata tetap hanya berada di engine | 3 |
| `rebalance` | `{ positionId, tickLower, tickUpper }` | 3 |
| `claim` | `{ positionId, swapToEth }` | 3 |
| `close` | `{ positionId, swapToEth }` | 3 |
| `get_positions` | tanpa isi. Muat ulang posisi LP wallet dari rantai. Menyentuh jaringan, jadi jawabannya datang belakangan sebagai `positions` | 5 |
| `get_transactions` | tanpa isi. Baca jurnal transaksi lokal untuk wallet aktif. Tidak membutuhkan signer dan tidak menyiarkan ulang transaksi | 5 |
| `get_notifications` | tanpa isi. Minta riwayat notifikasi terakhir | 5 |
| `plan_action` | `{ position_id, request_id, action, input }`. Identitas posisi adalah chain:manager:token_id. `input` membawa amount0/amount1 untuk tambah likuiditas; rebalance memakai range aktif yang terlihat di chart. Engine membaca ulang NFT yang dipilih dan kepemilikannya sebelum menyusun dan menyimulasikan aksi. Quote aksi tidak lagi mengenumerasi seluruh NFT V3 atau memindai Transfer V4 | 8 |
| `execute_action` | `{ plan_id }`. Setujui ID aksi yang ditampilkan. Rencana lama, kedaluwarsa, atau belum lolos gate ditolak | 5 |
| `run_action_flow` | `{ plan_id }`. Satu persetujuan user untuk aksi posisi. Engine menyelesaikan seluruh approval, menyusun ulang satu kali dari state rantai, lalu mengirim aksi akhir secara berurutan. Setelah close sukses, token biasa dari delta close dikonversi ke ETH di belakang layar; ETH, WETH, dan USDG dipertahankan | 7 |
| `approve_hook` | `{ hook }`. Catat persetujuan sadar user atas sebuah hook V4 yang belum lolos kriteria adopsi 5.4. Engine menilai ulang hooknya sendiri sebelum mencatat, karena penilaian yang datang dari panel bisa saja sudah diubah, dan bagian yang paling menarik untuk diubah justru kesimpulan bisa diupgrade atau tidaknya. Hook yang masih bisa diupgrade ditolak di sini juga. Penilaian menyimpan code_hash; quote, approval token, zap, dan mint memeriksa ulang kode serta slot proxy. Pencabutan membatalkan revisi rencana, dan hasil penilaian lama tidak boleh menghidupkan kembali persetujuan | 6 |
| `forget_hook` | `{ hook }`. Cabut persetujuan sebuah hook | 6 |
| `add_rpc` | `{ purpose, endpoint }`. Tambahkan RPC ke jalur `transaction`, `read`, atau `logs`. Konfigurasi disimpan dan provider baru langsung memakai daftar tersebut; permintaan yang sudah berjalan menyelesaikan snapshot lamanya | 8 |
| `remove_rpc` | `{ id }`. Hapus RPC melalui ID aman dari `engine_status`, sehingga URL berkunci tidak perlu dikirim kembali dari panel. RPC transaksi terakhir tidak dapat dihapus | 8 |
| `get_depth` | tanpa isi. Minta distribusi likuiditas pool yang sedang aktif. Terpisah dari `range_view` dengan sengaja: distribusi butuh dua perjalanan ke rantai sedangkan `range_view` dikirim ulang setiap geseran garis, dan menggabungkannya membuat garis terasa berat persis saat digeser | 6 |
| `scan` | tanpa isi. Pindai pool paling ramai di rantai dan nilai kelayakannya. Mahal, karena engine membaca setiap swap pada jendela terakhir, jadi ia hanya berjalan saat user memintanya. Jawabannya datang belakangan sebagai `scan_results`. Penyaring belum ikut dikirim panel: seluruh penyaringan masih berjalan di engine, lihat 6.13 | 6 |

Engine ke panel:

| Pesan | Isi | Fase |
| --- | --- | --- |
| `hello` | `{ engineVersion, protocolVersion }`, dikirim sekali saat tersambung | 1 |
| `engine_status` | `{ engineVersion, uptimeSecs, chain, wallet, registry, canExecute, blockedReasons[], rpcEndpoints[] }`. Setiap RPC membawa ID aman, URL tersunting, fungsi, dan prioritas failover | 8 |
| `pong` | balasan denyut | 1 |
| `open_plan` | `{ plan_id, request_id, deposit, ... }`, seluruh gate termasuk R1/R2, setor, APR yang sudah memakai versi kalibrasi aktif, gas_biaya, gas_gwei, version, zap, expires_at. Langkah izin membawa `{ step_id, jenis, token, spender, needed, current, label }`. Panel hanya menerima request_id miliknya dan memakai rencana untuk jumlah pasangan serta tombol utama; rincian gate dan izin tidak ditampilkan | 3 |
| `deposit_preview` | `{ request_id, deposit, setor, error }`. Jawaban ringan untuk `preview_deposit`; request_id membuat jawaban input lama dibuang. `setor` hanya untuk tampilan dan tidak pernah dipakai sebagai rencana transaksi | 7 |
| `send_result` | `{ kind, tx_hash, nonce, gas_limit, status, block_number, gas_used, gas_cost_wei, minted_amounts, token_id, verification, sent_at }`. Gas berasal dari gasUsed dan effectiveGasPrice receipt; null jika belum diketahui. minted_amounts berisi amount0/amount1 aktual dari event manager V3, null jika belum dibuktikan. `kind`: `approve`, `zap`, `open`, `action`, `exit_swap`. `status`: `sukses`, `revert`, `menggantung`. Setelah 15 detik tanpa receipt, UI menerima `menggantung` dan jurnal meneruskan rekonsiliasi di belakang layar; hasil akhirnya dipancarkan sebagai notifikasi | 3 |
| `alert` | `{ level, code, message, positionId }` | 1 |
| `positions` | `{ positions[], tertutup[], owner, keluar_range, ringkasan, learning, error, dimuat_pada }`. Posisi membawa position_id berisi chain:manager:token_id. PnL membawa `deposited`, `value`, `pnl_percent`, `opened_at`, fee, gas, slippage, dan net; angka yang belum terbukti tetap null. `tertutup[]` menggabungkan jurnal close dengan posisi yang likuiditasnya menjadi nol atau tidak lagi dimiliki setelah snapshot on-chain lengkap. Close dari aplikasi lain memakai snapshot aktif terakhir, lalu mencoba membuktikan jumlah diterima dari transfer ERC20 pada receipt transaksi penurunan likuiditas. Hasilnya ditandai perkiraan ketika pemisahan fee tidak diketahui, dipersistenkan setelah restart, dan tidak lagi masuk `positions[]`. Nilai posisi tertutup memisahkan pokok yang diterima dari fee. `learning` tetap membawa data kalibrasi untuk engine, tetapi tidak ditampilkan sebagai filter teknis di PnL. Pemuatan berkala tiap 30 detik tetap berjalan ketika panel tertutup | 8 |
| `transactions` | `{ owner, items[], error }`. Tiap item memuat operasi, termasuk `exit_swap`, hash, plan/flow/position ID, nonce, gas, jumlah diterima, status, dan finalitas. Request lengkap serta calldata tidak keluar dari engine; detail error transport disunting karena dapat memuat URL RPC berkunci | 5 |
| `notify` | `{ level, code, message, subject, at, ditahan }`. Berbeda dari `alert`: pesan ini melewati pendinginan, masuk riwayat, dan membawa petunjuk durasi serta suara. `alert` dipakai untuk kegagalan seketika pada aksi yang sedang dijalankan user | 5 |
| `notify_history` | daftar kejadian terakhir, terbaru lebih dulu, paling banyak 50 | 5 |
| `depth` | `{ poolId, tickSekarang, hargaSekarang, tickSpacing, pita[], quoteAdalahToken1, error }`. Setiap pita membawa batas tick, batas harga, likuiditas, modal yang tertanam dalam token kutipan, dan penanda apakah harga sekarang ada di dalamnya. Batas harga selalu terurut naik apa pun sisi mana memecoinnya berada, karena panel tidak boleh mengulang perhitungan arah itu sendiri. Lihat 6.14 | 6 |
| `scan_results` | `{ rows[], hooks[], adopsi_lengkap, jendelaPindaiMenit, jendelaVolumeMenit, volumeJendelaPendek, modalAcuan, totalSwap, dipindaiPada, error }`. Setiap baris membawa simbol, pasangan, market cap, volume, kedalaman, hasil jendela, APR, efisiensi, umur, risiko, label, dan alasan. `hooks[]` membawa code_hash, jumlah pool, umur, TVL USD, LP unik aktif, lima status kriteria, serta lolos_otomatis. `adopsi_lengkap` benar setelah sejarah Initialize dan ModifyLiquidity selesai | 6 |
| `signals` | `{ address, barTerpakai, harga, sideways, basis, saran, konsentrasiDibandingLebar, dihitungPada }`. `sideways` dan `basis` kosong berarti datanya belum cukup, BUKAN berarti kondisinya tidak menarik | 6 |
| `action_plan` | `{ plan_id, position_id, request_id, token_id, action, nama, gates[], gas_estimate, dry_run, menutup, siap, expires_at, fee0, fee1, allowances[], amounts, tick_lower_baru, tick_upper_baru }`. Tambah dan rebalance memakai R1; tambah modal baru juga memakai R2. Panel memakai hasilnya untuk mengaktifkan satu tombol, sedangkan gate, simulasi, dan approval tetap berada di alur engine | 5 |
| `token_state` | `{ address, meta, quotes[], pools[], selectedQuote, selectedPoolId, saldo, error }`. `saldo` memuat saldo wallet untuk kedua sisi pasangan terpilih beserta saldo token gas, dan bernilai kosong kalau wallet belum dibuka, yang berbeda dari saldo nol. Setiap pool membawa `stats` berisi volume beserta jendelanya, TVL, dan proyeksi APR kalau sudah terukur. Setiap `quote` membawa `{ quote, quoteSymbol, quoteDecimals, canonical, pools[] }`. Panel menahan snapshot parsial. Snapshot pertama yang terlihat memakai cache atau pengukuran sekitar tiga jam, lalu pengukuran 24 jam menggantikannya di latar. Lihat 4.3c | 2 |
| `range_view` | `{ poolId, tickSpacing, ethUsd, current, lower, upper, widthPct, inRange, warnings[] }`. Tiap batas memuat `tick`, `priceWeth`, `priceUsd`, `mcapUsd`, dan `driftBps`, yaitu selisih akibat pembulatan ke grid tick | 2 |
| `price_tick` | `{ address, priceUsd, priceWeth, mcapUsd }` | 2 |
| `range_quote` | `{ quoteId, feeApr, expectedHoursInRange, ilBreakevenHours, crowdedLiquidityUsd, ourSharePct, amount0, amount1, swapNeeded, priceImpact, gasEstimate, warnings[] }` | 3 |
| `tx_status` | `{ txId, stage, hash, error }` | 3 |
| `position_update` | `{ positionId, inRange, pricePct, feesUsd, netPnlUsd }` | 5 |
| `signal_list` | `{ items[] }` | 6 |

Content script ke panel, lewat service worker. Identitas token dan URL
divalidasi oleh relay dan service worker; tab ID berasal dari sender Chrome.
Side panel memeriksa ulang tab aktif lewat `chrome.tabs.query` dan memakai
nomor revisi agar hasil query navigasi lama tidak mengganti token baru:

```
token_page         { token, url, chain }
chart_price        { token, url, chain, bar }
chart_bars         { token, url, chain, bars[] }
chart_line_moved   { token, url, chain, side, price }
```

Panel ke content script melalui chrome.tabs.sendMessage, lalu ke MAIN world.
Relay dan bridge memeriksa token terhadap URL tab sebelum menjalankan perintah:

```
draw_lines         { token, upperPrice, lowerPrice }
clear_lines        { token }
probe              { token }
```

Attach chart memakai generasi navigasi. `dataReady` dibatasi 15 detik.
Pembuatan garis yang selesai setelah pindah token dibersihkan dari chart
pemiliknya, dan tidak boleh mengirim hasil beridentitas token baru. Target
`set_line_price`, `gmgn_request`, serta `gmgn_response` belum terhubung ke
jalur runtime. `chart_capabilities` masih laporan lokal bridge.

Setiap `quote_range` menghasilkan `open_plan` dengan plan_id baru dan seluruh asumsi tersimpan di engine. Approval, swap, dan mint hanya menerima ID yang masih berlaku, maksimal 180 detik. Pergantian token, pool, range, atau input setoran membatalkannya lebih awal.

### 7.4 Model data

Implementasi redb saat ini menyimpan catatan posisi dengan kunci
`pnl:v2:<chain>:<manager>:<token_id>` dan cache NFT dengan
`posisi_v4:v2:<chain>:<manager>:<owner>`. Jurnal transaksi memakai
`tx:v1:<chain>:<owner>:<tx_hash>`, ditulis setelah signing dan sebelum
broadcast. Receipt diperiksa ulang sampai 20 konfirmasi; nonce yang sudah mined
oleh hash lain ditandai `replaced`. Kunci lama tidak dihapus atau
dipasangkan ke manager secara spekulatif. Self-improvement memakai namespace
berikut dalam tabel JSON yang sama:

```
learning:v1:position:<position_id>       fitur, versi, waktu in-range, hasil
learning:v1:calibration:<version>        faktor, bobot, ambang, hasil bayangan
learning:v1:active:<regime>              versi juara yang dipakai
learning:v1:challenger:<regime>          versi penantang yang dinilai
learning:v1:trained:<regime>             jumlah hasil pada latihan terakhir
risk/token:v1:<chain>:<pool>:<token>     bukti R1 dan R2 terakhir
risk/hook-disetujui/<hook>               sumber allowlist dan code hash
```

Skema konseptual berikut tetap menjelaskan relasi datanya. Pada implementasi
redb, kolom berada di nilai JSON dan identitas tabel berada di prefiks kunci.

```
tokens           address, symbol, name, decimals, total_supply, created_at,
                 twitter_username, tax_buy, tax_sell, tax_verified_at

pools            id, version, address_or_poolkey, token0, token1, fee,
                 tick_spacing, hook, liquidity, sqrt_price_x96, updated_at

positions        id, pool_id, version, token_id, tick_lower, tick_upper,
                 liquidity, opened_at, closed_at, strategy_label,
                 entry_price, entry_amount0, entry_amount1,
                 fees_collected_usd, gas_spent_usd, swap_cost_usd,
                 il_usd, net_pnl_usd, seconds_in_range, seconds_out_of_range

position_events  position_id, ts, kind, tx_hash, payload_json

quotes           id, pool_id, tick_lower, tick_upper, size_usd,
                 predicted_fee_apr, predicted_hours_in_range,
                 predicted_il_breakeven_hours, created_at, consumed_by_position

signals          id, token_address, ts, features_json, score, label,
                 outcome_fee_apr_6h, outcome_realized_vol_6h, outcome_label

settings         key, value_json
calibrations     id, regime, parent_id, created_at, weights_json,
                 thresholds_json, status, training_positions,
                 shadow_positions, shadow_wins, challenger_error,
                 champion_error
```

`learning:v1:position` menyatukan quote, fitur sinyal, identitas posisi, dan
hasil close agar pasangannya tidak dapat tertukar. Field sumber yang belum
tersedia, misalnya metrik holder, tetap null. Tanpa menyimpan prediksi beserta
hasilnya, kalibrasi ulang tidak mungkin dilakukan.

### 7.5 Ketahanan data

Chain ini secara resmi dinyatakan tidak stabil, jadi tidak boleh ada satu titik kepercayaan tunggal.

- **Harga.** Tiga sumber: `slot0` pool on chain, harga gmgn, dan harga hasil kalkulasi dari trade terakhir. Jika selisih antar sumber lebih dari 2 persen, panel menampilkan status data tidak sehat dan tombol eksekusi dinonaktifkan.
- **Blok.** Engine memantau selisih antara waktu blok terakhir dan waktu lokal. Lag di atas 5 detik menonaktifkan eksekusi otomatis dan menampilkan peringatan.
- **RPC.** Daftar RPC berurutan dengan failover otomatis. Kegagalan dicatat dan endpoint yang bermasalah didinginkan.
- **gmgn.** Jika tab gmgn tertutup atau API menolak, fitur yang bergantung padanya berdegradasi dan panel menyebutkan fitur mana yang mati. Posisi yang sedang berjalan tetap terpantau karena hanya butuh RPC.

---

## 8. Design System

### 8.1 Prinsip

Tidak ada gradient di mana pun. Warna datar, pembeda kedalaman memakai border 1 piksel dan pergeseran nilai permukaan. Sudut membulat sedang, cukup untuk terasa halus tapi tidak sampai terlihat seperti mainan. Tidak ada eyebrow di atas judul. Judul langsung, keterangan hanya jika benar benar menambah informasi, dan ditulis pendek.

### 8.2 Tipografi

Plus Jakarta Sans, dibundel lokal sebagai woff2 di dalam ekstensi. Tidak memuat dari CDN, karena Content Security Policy MV3 dan karena side panel harus tetap benar saat offline.

| Peran | Ukuran | Bobot | Catatan |
| --- | --- | --- | --- |
| Judul tab | 15 px | 600 | |
| Judul kartu | 13 px | 600 | |
| Isi | 13 px | 400 | |
| Label | 11 px | 500 | Huruf kecil, tanpa kapital semua |
| Angka | 13 px | 500 | `font-variant-numeric: tabular-nums` wajib supaya angka tidak bergoyang saat berubah |
| Angka besar | 20 px | 600 | Untuk nilai utama di kartu PnL |

### 8.3 Radius dan jarak

| Elemen | Radius |
| --- | --- |
| Kartu dan panel | 10 px |
| Tombol, input, select | 8 px |
| Chip dan badge | 6 px |
| Indikator kecil dan bar | 4 px |

Skala jarak kelipatan 4: 4, 8, 12, 16, 24, 32.

### 8.4 Warna

Gelap, default:

```
bg              #0F1115
surface         #161920
surface-raised  #1D212A
border          #262B36
border-strong   #333A47
text            #E7EAF0
text-muted      #8D95A5
accent          #4C8DF6
upper / in      #2E9E63
lower / out     #D8484D
warn            #C98A2E
```

Terang:

```
bg              #FBFBFC
surface         #FFFFFF
surface-raised  #F4F5F7
border          #E3E6EB
border-strong   #CDD2DA
text            #14171C
text-muted      #5D6470
accent          #2563D9
upper / in      #1E7F4E
lower / out     #C0393E
warn            #9A6A16
```

Hijau untuk batas atas dan merah untuk batas bawah, konsisten antara chart dan panel, mengikuti gambar referensi. Accent hanya untuk elemen interaktif, tidak pernah untuk dekorasi.

### 8.5 Layout

Side panel bekerja pada lebar 320 sampai 480 piksel dan wajib benar di seluruh rentang itu. Di bawah 360 piksel, kartu proyeksi berubah dari dua kolom menjadi satu kolom. Tidak ada scroll horizontal di kondisi apa pun. Tabel yang lebar dibungkus kontainer dengan scroll sendiri.

---

## 9. Security dan Privacy

### 9.1 Kunci privat

- Disimpan sebagai keystore terenkripsi di disk memakai AES-256-GCM. Kunci turunan dari passphrase user lewat Argon2id.
- Passphrase tidak pernah ditulis ke disk. Setelah engine dibuka, kunci hasil dekripsi hanya ada di memori, di dalam wadah yang di-zeroize saat engine berhenti atau saat wallet dikunci.
- Opsi menyimpan passphrase di Windows Credential Manager, mati secara default.
- Wallet mengunci diri otomatis setelah periode tidak aktif yang bisa diatur, default 30 menit.
- Profil layanan lokal mengaktifkan Credential Manager dan memakai nilai
  auto-lock nol. Wallet lama meminta passphrase sekali untuk mengisi credential,
  lalu engine membukanya otomatis pada restart berikutnya.
- Kunci privat tidak pernah dikirim ke side panel, ke content script, atau keluar dari proses engine dalam bentuk apa pun.

### 9.2 Batas risiko

Dipaksakan di sisi engine, bukan di UI, sehingga tidak bisa dilewati dengan memanipulasi panel.

```
maxPositionUsd        nilai maksimum satu posisi                 bawaan: 5 USD
maxDailyDeployUsd     total maksimum yang dikerahkan per hari    bawaan: 5 USD
maxOpenPositions      jumlah posisi terbuka bersamaan            bawaan: 2
newPoolFirstSizeUsd   batas ukuran posisi pertama di pool baru   bawaan: 2,50 USD
minEthReserve         saldo ETH yang disisakan untuk gas         bawaan: 0,002 ETH
maxSlippageBps        batas slippage zap                         bawaan: 200 bps
```

Profil ini ditujukan untuk modal operasional sampai sekitar 50 USD. Satu posisi
dibatasi 5 USD, paling banyak satu jatah baru ditempatkan per hari, dan eksposur
dari dua posisi berukuran maksimum berhenti pada 10 USD. Posisi pertama di pool
yang belum pernah dipakai dimulai dari setengah ukuran biasa. Semua batas
diperiksa engine terhadap jurnal dan posisi on-chain, termasuk posisi yang
dibuka dari aplikasi lain.

Cadangan ETH mencegah wallet kehabisan gas sehingga posisi tidak bisa ditutup.
Batas slippage 2 persen membuat Zap batal jika harga bergerak terlalu jauh dari
kutipan. Batas yang lebih ketat lebih tepat untuk modal kecil karena satu swap
buruk dapat menghapus fee beberapa hari.

Selain itu ada satu saklar terpisah:

```
execution.dry_run     menyusun dan menyimulasikan, tapi tidak pernah mengirim
                      bawaan: MENYALA
```

Dry run menyala secara bawaan dan itu disengaja. Engine harus bisa dipasang di mesin bersih tanpa risiko memindahkan dana pada percobaan pertama. Mengirim transaksi sungguhan adalah tindakan sadar yang dinyalakan user, bukan bawaan yang kebetulan aktif. Config yang tidak menyebut bagian `execution` sama sekali tetap mendapat dry run menyala, bukan mati. Paragraf ini menjelaskan perilaku implementasi saat ini sampai migrasi mode di bawah selesai.

Melampaui batas yang aktif berarti transaksi ditolak di engine dengan alasan yang dikembalikan ke panel.

#### 9.2.1 Rancangan mode Simulasi dan Tx Asli

Status: disepakati untuk implementasi berikutnya dan belum diterapkan pada kode.

Sakelar `Simulasi | Tx Asli` akan menggantikan `execution.dry_run` dan indikator status eksekusi di bagian bawah side panel. Bawaannya `Simulasi`. Memilih mode membatalkan rencana aksi yang sudah tersusun, sehingga rencana dari satu mode tidak pernah bisa dikirim melalui mode lain.

Mode Simulasi menjalankan pengalaman produk yang sama dengan transaksi asli: user menyusun rencana, melewati gate, membuka posisi, memantau posisi, mengklaim fee, melakukan rebalance, dan menutup posisi. Perbedaannya hanya pada lapisan pengiriman. Engine tidak meminta tanda tangan, tidak memesan nonce, tidak mengirim raw transaction, dan tidak mengubah allowance atau saldo on chain.

Posisi simulasi wajib:

- disimpan di basis data dan tetap ada setelah engine dimulai ulang;
- muncul di tab Positions dan PnL dengan label `simulasi` yang selalu terlihat;
- memakai harga, tick, likuiditas, gas, dan hasil quote dari snapshot rantai nyata saat aksi dilakukan;
- mempunyai siklus hidup buka, tambah, klaim, rebalance, dan tutup yang sama dengan posisi asli;
- menghasilkan fee, impermanent loss, biaya gas, slippage, serta PnL bersih sebagai estimasi hipotetis;
- tidak dicampurkan ke saldo, fee yang bisa diklaim, riwayat transaksi, atau total PnL posisi asli.

Fee simulasi dihitung dari perubahan fee growth pool dan likuiditas virtual posisi pada setiap pembaruan. Nilai posisi dan impermanent loss dihitung dari jumlah token virtual saat masuk dibanding nilai kedua token pada harga terakhir. Gas dan slippage memakai estimasi yang dihasilkan jalur quote yang sama dengan Tx Asli. Semua angka itu adalah hasil model untuk pengujian dan tidak dapat diklaim on chain.

Setiap posisi, aksi, hasil, notifikasi, dan ekspor yang berasal dari mode ini membawa `execution_mode: "simulation"` serta label manusia `simulasi`. ID posisi simulasi memakai namespace terpisah, sehingga tidak mungkin dianggap sebagai token ID NFT atau position key on chain.

Mode Tx Asli memakai seluruh pipeline tanda tangan, approval, simulasi `eth_call`, pengiriman, receipt, dan verifikasi pasca kirim yang sudah ditentukan dokumen ini. Perpindahan ke Tx Asli harus merupakan tindakan user yang eksplisit. Posisi simulasi tidak dapat diubah menjadi posisi asli; user harus menyusun rencana baru dan mengirim aksi baru.

Migrasi konfigurasi yang direncanakan:

```text
execution.mode        "simulation" | "live"
                      bawaan: "simulation"
```

Config lama dengan `dry_run = true`, atau tanpa bagian `execution`, dipetakan ke `simulation`. Config lama dengan `dry_run = false` dipetakan ke `live`, tetapi panel tetap membatalkan rencana lama dan meminta user menyusun ulang sebelum transaksi pertama. Setelah masa migrasi, `dry_run` dihapus dari schema, protokol, gate G6, dan UI.

Uji penerimaan mode ini wajib membuktikan tiga hal: alur simulasi lengkap bertahan setelah restart dan menghasilkan PnL, tidak ada RPC pengiriman atau mutasi allowance selama simulasi, serta rencana dan posisi tidak pernah melintasi batas antara `simulation` dan `live`.

### 9.3 Permukaan lokal

- Engine hanya mengikat ke `127.0.0.1`, tidak pernah ke `0.0.0.0`.
- Setiap request membawa bearer token. Token dibuat acak saat engine pertama kali jalan, ditampilkan sebagai kode pairing, dan dimasukkan user ke side panel satu kali.
- CORS dibatasi ke origin ekstensi.
- Aksi yang menandatangani transaksi butuh wallet dalam keadaan terbuka, dan setiap aksi tetap butuh klik user.
- Log tidak pernah memuat kunci, passphrase, atau token pairing.

### 9.4 Data pengguna

Tidak ada telemetri. Tidak ada server jarak jauh. Seluruh basis data, log, dan keystore berada di mesin user. Ekstensi tidak membaca tab selain gmgn.ai.

### 9.5 Batasan yang disadari

Hot wallet lokal berarti perangkat yang sudah disusupi sama dengan dana yang hilang. Ini konsekuensi yang diterima secara sadar demi otomasi dan kecepatan. Pembatas nominal mengurangi nilai yang bisa dipakai oleh satu flow, tetapi isi wallet tetap menjadi batas kerugian terburuk jika perangkat atau kunci sudah dikuasai pihak lain. Mitigasi yang wajib disampaikan ke user di layar setup: pakai wallet terpisah khusus operasi ini, isi hanya sebanyak yang siap dipertaruhkan, dan jangan menyimpan dana jangka panjang di sana.

---

## 10. Evaluation Strategy

Tidak ada fitur yang dianggap selesai tanpa uji yang sesuai di kolom ketiga.

| Area | Yang diukur | Cara |
| --- | --- | --- |
| Matematika tick dan likuiditas | Kebenaran mutlak | Uji unit terhadap vektor referensi Uniswap. Uji properti dengan input acak untuk memastikan `tick -> price -> tick` kembali ke nilai semula |
| Calldata V3 dan V4 | Kesamaan byte | Golden vector direkam dari UI Uniswap resmi, dibandingkan byte per byte |
| Konsistensi lintas versi | V3 dan V4 sepakat | Uji diferensial, input range sama, hasil dibandingkan satu sama lain dan dengan QuoterV2 serta V4Quoter, toleransi satu wei per basis |
| Router fork | Field `minHopPriceX36` benar | Golden vector khusus, plus simulasi terhadap state nyata |
| Injeksi chart | Tidak regresi saat gmgn berubah | Uji integrasi terhadap snapshot HTML gmgn yang disimpan di repo, satu snapshot per tingkat degradasi |
| Latensi | Target di bagian 1 | Benchmark otomatis pada setiap build, gagal jika melewati ambang |
| Kalibrasi Range Optimizer | Prediksi cocok kenyataan | Backtest replay: jalankan optimizer atas candle historis, bandingkan prediksi waktu bertahan dengan hasil sebenarnya. Target: median error di bawah 30 persen |
| Detektor sideways | Sinyal benar benar berguna | Bandingkan token berlabel `LP Sideways` dengan kelompok kontrol token bervolume tinggi tanpa label, ukur rasio fee terhadap IL selama 6 jam berikutnya. Target: median kelompok berlabel minimal 1,8 kali kelompok kontrol |
| Gate risiko | Tidak ada token berbahaya lolos | Kumpulkan daftar token berpajak dan honeypot yang sudah diketahui, pastikan semuanya diblokir |
| Akuntansi PnL | Angka cocok on chain | Rekonsiliasi otomatis antara PnL internal dan hasil pembacaan ulang on chain, selisih di atas 0,5 persen memicu alarm |

---

## 11. Rekomendasi untuk Menaikkan Peluang Profit

Bagian ini menjawab permintaan saran tambahan. Setiap poin berdiri sendiri dan bisa dipotong dari cakupan tanpa merusak yang lain, tapi urutannya sudah disusun dari dampak paling besar.

### 11.1 Pusatkan range di point of control, bukan di harga terakhir

Kebiasaan umum adalah menaruh range simetris mengelilingi harga sekarang. Itu keliru. Yang membayar fee adalah volume yang lewat, dan volume paling padat berada di sekitar point of control, yaitu level harga dengan volume terbanyak pada profil volume. Engine menghitung volume profile dari candle 1 menit, menemukan POC dan area nilai 70 persen, lalu menyarankan range yang berpusat di POC. Kalau harga sekarang jauh dari POC, itu justru sinyal bahwa harga cenderung kembali ke sana. Fitur ini murah dihitung dan dampaknya pada fee besar.

### 11.2 Range Optimizer berbasis Monte Carlo bootstrap

Rumus lebar range yang mengandaikan distribusi normal akan salah besar di memecoin, karena ekornya tebal. Solusinya tidak mengandaikan apa apa: ambil sampel ulang return 1 menit token itu sendiri, jalankan 2000 lintasan, dan ukur langsung berapa lama sampai lintasan menyentuh salah satu batas. Dari situ engine bisa menyapu berbagai lebar range dan menampilkan kurva:

```
untuk setiap lebar w:
    E[fee]   = fee per jam pada lebar w  x  ekspektasi jam di dalam range
    E[IL]    = rata rata kerugian divergensi pada lintasan yang keluar range
    E[biaya] = gas buka  +  gas tutup  +  slippage zap
    skor(w)  = E[fee] - E[IL] - E[biaya]
```

Lebar yang memaksimalkan skor menjadi saran awal, dan user tetap bebas menggeser garis. Perhitungan 2000 lintasan atas 240 bar selesai dalam hitungan milidetik di Rust. Inilah pembenaran paling kuat untuk memakai Rust di proyek ini.

### 11.3 Peta kepadatan likuiditas pesaing

Fee yang diterima sebanding dengan porsi likuiditas kita di dalam range aktif. Range yang terlihat sempurna bisa membayar sedikit sekali kalau sudah ramai. Engine membaca peta tick lewat TickLens untuk V3 dan StateView untuk V4, lalu menggambar kepadatan likuiditas per pita tick sebagai overlay di sisi chart. Panel menampilkan porsi kita dalam persen sebelum posisi dibuka. Tidak ada alat lain yang menampilkan ini di atas chart gmgn, dan ini mengubah keputusan range secara langsung.

### 11.4 Jam impas impermanent loss

Satu angka yang paling menentukan tapi hampir tidak pernah ditampilkan alat mana pun: berapa jam posisi harus bertahan di dalam range, pada laju fee saat ini, untuk menutup kerugian divergensi jika harga menyentuh salah satu batas.

```
IL_batas       = nilai posisi pada batas  /  nilai jika hanya hold  -  1
jam_impas      = |IL_batas| x nilai posisi  /  fee per jam
```

Kalau jam impas lebih panjang daripada ekspektasi waktu bertahan dari 11.2, posisi itu secara matematis kalah sebelum dibuka. Panel menyatakannya dengan jelas dan menandai kartunya.

### 11.5 Pemilihan fee tier otomatis

Satu pair bisa punya pool di beberapa fee tier. Yang paling likuid belum tentu paling menguntungkan untuk LP. Engine menghitung `volume x fee_tier / likuiditas` untuk setiap pool yang ada, termasuk lintas V3 dan V4, lalu mengurutkan berdasarkan proyeksi fee per dolar modal. Pada memecoin, pool 1 persen sering mengalahkan pool 0,3 persen meski volumenya lebih kecil.

Discovery menyiapkan pilihan kasar di engine, lalu panel membukanya bersama
snapshot statistik awal sekitar tiga jam. Setelah statistik 24 jam selesai,
pilihan aktif dapat berpindah ke pool ber-APR marginal tertinggi yang lolos
ambang kapasitas selama user belum memilih pool lain.

Pemeringkatan ini berjalan **setelah** penyaringan fee di luar akal sehat pada 4.3b, bukan sebelumnya. Tanpa urutan itu, rumus `volume x fee_tier / likuiditas` justru memilih pool fee 80 persen yang volumenya nol.

APR marginal pool yang nyaris kosong mudah membesar hanya karena penyebutnya
kecil. Karena itu kandidat utama harus mempunyai sedikitnya satu persen volume
24 jam dan TVL aktif terbesar pada kelompok pasangan yang sama. Pool di bawah
ambang tetap terlihat, tetapi tidak dipilih otomatis mengalahkan pasar yang
benar benar aktif.

Terpasang di `signals::optimizer` dan bisa dijalankan langsung lewat
`cargo xtask optimizer <alamat_token>`. Pool tanpa volume tidak diberi proyeksi
nol lalu diurutkan di bawah, melainkan ditandai terpisah: nol di sini berarti
belum ada perdagangan pada jendela yang diukur, dan itu berbeda dari pool yang
ramai tapi likuiditasnya terlalu tebal. Fee dinamis tidak diproyeksikan sama
sekali, karena besarannya baru diketahui saat swap terjadi.

Rumus di atas dipakai untuk **memeringkat pool pada token yang sama**, dan hanya
berarti sebagai perbandingan. Untuk menyatakan hasilnya sebagai angka yang bisa
dibaca sendiri, 6.13 memakai bentuk yang memuat modal acuan di penyebut.

### 11.6 LP satu sisi sebagai alat eksekusi

Range yang seluruhnya berada di bawah harga hanya berisi WETH dan berperan sebagai tangga limit buy yang dibayar fee. Kalau harga tidak pernah turun ke sana, tidak ada yang terbeli dan tidak ada yang rugi. Range yang seluruhnya di atas harga hanya berisi token dan berperan sebagai tangga limit sell. Ini mengubah Rangefinder dari alat yang hanya berguna saat pasar menyamping menjadi alat yang berguna setiap hari, dan cocok dengan pesan playbook untuk membeli penurunan alih alih mengejar lilin hijau.

### 11.7 Range bertingkat

Alih alih satu range lebar, sebar modal ke dua atau tiga range sempit yang bertumpuk. Efisiensi modal naik di zona inti, sementara lapisan luar menangkap sumbu harga tanpa menyeret seluruh posisi keluar range. Chart menampilkan beberapa pasang garis dengan ketebalan berbeda. Rebalance bisa dilakukan per lapisan, bukan seluruhnya.

### 11.8 Peringatan dilusi likuiditas

Ketika ada pihak lain menambahkan likuiditas besar yang bertumpang tindih dengan range kita, porsi fee kita langsung turun tanpa ada yang berubah di harga. Engine memantau event `Mint` dan `Burn` pada pool, menghitung ulang porsi kita, dan mengirim peringatan jika turun melewati ambang. Sebaliknya, penarikan besar oleh pihak lain adalah kabar baik dan juga dilaporkan. Sejauh yang saya tahu tidak ada alat ritel yang melakukan ini.

### 11.9 Aturan keluar berbasis imbal hasil, bukan hanya harga

Posisi yang tetap berada di dalam range tapi tidak menghasilkan fee adalah modal mati yang menanggung risiko tanpa dibayar. Selain aturan keluar range, tambahkan: jika setelah `N` menit rasio fee terhadap modal masih di bawah ambang, tutup. Pola ini sudah terbukti di proyek Meridian milik user pada `minFeePerTvl24h` dan `minAgeBeforeYieldCheck`, jadi bukan spekulasi.

### 11.10 Akuntansi biaya penuh

Fee kotor menipu. Engine wajib memisahkan gas buka, gas tutup, slippage zap, dan dampak harga sebelum menghitung net. Kartu PnL menampilkan net sebagai angka utama dan fee sebagai angka pendamping; rincian teknis tetap tersedia di jurnal untuk audit.

### 11.11 Manfaatkan sequencer 100 milidetik

Chain ini melayani berdasarkan urutan kedatangan, bukan lelang biaya. Setelah sumbu harga tajam yang menendang posisi keluar range, siapa yang paling cepat kembali masuk akan menangkap ledakan volume berikutnya. Engine menyiapkan dan menandatangani transaksi rebalance lebih awal saat harga mendekati batas, sehingga saat user menekan tombol yang tersisa hanya pengiriman byte. Ini keunggulan nyata yang hanya bisa diraih dengan proses lokal yang cepat.

### 11.12 Mode replay untuk kalibrasi

Sebelum modal nyata dipakai, jalankan strategi atas data historis dan lihat hasilnya. Selain memberi rasa percaya, replay adalah sumber data untuk loop self improvement, dan satu satunya cara jujur untuk menilai apakah bobot detektor sideways masuk akal.

### 11.13 Sinyal wallet pintar dan aktivitas komunitas

gmgn menyediakan daftar follow wallet dan data PnL per wallet. Bangun daftar wallet yang lolos saringan di 5.3, lalu jadikan pembelian mereka sebagai penambah skor, bukan sebagai pemicu tunggal. Gabungkan dengan kecepatan posting akun X token tersebut. Playbook menyebut komunitas yang masih hidup sebagai syarat, dan ini cara mengukurnya.

### 11.14 Batasi kerugian terbesar, yaitu masuk ke token yang salah

Kerugian terbesar seorang LP bukan dari range yang meleset, melainkan dari menyediakan likuiditas pada token yang runtuh, karena posisi berakhir memegang seratus persen sisi yang tidak berharga. Karena itu gate risiko di 5.4 bukan fitur tambahan, melainkan fitur penghasil keuntungan. Menolak sepuluh token buruk bernilai lebih besar daripada mengoptimalkan lebar range pada sepuluh token bagus.

---

## 12. Loop Self Improvement

Diminta sebagai kewajiban, jadi dirancang sejak awal, bukan ditempel belakangan.

### 12.1 Apa yang direkam

Setiap kali open lolos verifikasi pasca kirim, engine menyimpan skor sideways
beserta komponennya, rezim, lebar range dalam ATR, porsi likuiditas aktif, fee
tier, versi protokol, jarak ke POC, prediksi fee APR mentah dan terkoreksi,
prediksi jam in-range, jam impas IL, serta versi kalibrasi. Umur token dan
metrik holder ikut dalam skema tetapi tetap null ketika sumbernya belum
memberi bukti. Setiap pemuatan posisi mengakumulasi waktu in-range. Setelah
close final, rekaman dipasangkan dengan fee terealisasi, IL, gas, slippage,
net, waktu in-range, dan total durasi.

### 12.2 Bagaimana kalibrasi berjalan

Setiap 30 posisi tertutup, atau saat diminta manual, engine menjalankan kalibrasi:

1. Regresi prediksi terhadap hasil, untuk mengukur bias sistematis pada proyeksi fee dan proyeksi waktu bertahan. Bias yang konsisten dikoreksi dengan faktor penyesuaian, bukan dengan mengubah rumus.
2. Model klasifikasi ringan atas vektor fitur terhadap hasil net positif, untuk menemukan fitur mana yang benar benar memprediksi.
3. Penyetelan bobot dan ambang detektor 5.2 dalam batas keras. Gate keamanan
   R1/R2 dan minimum allowlist hook 5.4 tidak boleh dilonggarkan oleh hasil
   trading; keputusan ini dicatat di DECISIONS 0009.

### 12.3 Pengaman

Loop yang menyetel dirinya sendiri bisa menyetel dirinya ke arah yang salah. Karena itu:

- Kalibrasi baru tidak aktif sebelum ada minimal 30 posisi tertutup pada rezim pasar yang sama.
- Setiap bobot punya batas atas dan bawah keras. Kalibrasi tidak boleh keluar dari batas itu.
- Skema penantang: kalibrasi baru berjalan berdampingan sebagai bayangan selama 20 posisi. Kalibrasi lama tetap yang mengambil keputusan. Penantang baru menggantikan juara jika unggul secara konsisten.
- Semua versi kalibrasi tersimpan di tabel `calibrations` dan bisa dikembalikan dengan satu perintah.
- Tab PnL bisa memfilter berdasarkan versi kalibrasi, sehingga efeknya terlihat, bukan sekadar diklaim.

Implementasi memakai median rasio aktual terhadap prediksi untuk faktor APR
dan waktu agar satu outlier memecoin tidak menguasai hasil. Bobot klasifikasi
dibandingkan antara hasil net positif dan nonpositif lalu dinormalisasi kembali
ke satu. Versi baru tidak mengubah keputusan sampai 20 hasil setelah waktu
pembuatannya selesai; promosi butuh sedikitnya 12 kemenangan dan jumlah galat
lebih kecil dari juara.

Operasi manual menggunakan satu perintah lokal dan basis data yang sama:

```
cargo xtask calibration status
cargo xtask calibration train [regime]
cargo xtask calibration replay [version]
cargo xtask calibration replay-bars <bars.json> [version]
cargo xtask calibration rollback <version>
```

`replay` mengevaluasi prediksi terhadap hasil posisi tersimpan. `replay-bars`
menjalankan ulang detektor setiap 30 candle setelah jendela 240 candle tersedia,
lalu membandingkan prediksi waktu bertahan dengan sentuhan batas pertama pada
candle berikutnya. Keduanya hanya membaca dan tidak dapat mempromosikan versi.

---

## 13. Memori Repo

Diminta supaya setiap agent yang masuk ke repo ini paham keseluruhan kode tanpa membacanya dari nol.

| Berkas | Isi |
| --- | --- |
| `CLAUDE.md` | Manual teknik. Arsitektur, tanggung jawab tiap modul, invarian yang mudah dirusak, pola yang harus ditiru, utang teknis yang sudah diketahui. Ditulis untuk sesi berikutnya, bukan untuk pengguna akhir. |
| `docs/PRD.md` | Dokumen ini. Sumber kebenaran untuk cakupan dan alasan keputusan. |
| `docs/DECISIONS.md` | Catatan keputusan arsitektural. Satu entri per keputusan: konteks, pilihan, alasan, konsekuensi, tanggal. Keputusan yang dibatalkan ditandai, tidak dihapus. |
| `docs/GMGN.md` | Kontrak terhadap gmgn: format URL, slug chain, jalur akses chart, daftar endpoint, bentuk field. Setiap poin disertai cara memverifikasi ulang, karena semuanya bisa berubah sewaktu waktu. |
| `docs/CHAIN.md` | Alamat kontrak Robinhood Chain beserta hash bytecode yang dipin, plus cara memverifikasi ulang lewat explorer. |
| `docs/RUNBOOK.md` | Cara menjalankan, cara membuka wallet, cara masuk mode dry run, apa yang harus dilakukan saat transaksi tersangkut atau RPC mati. |
| `memory/` | Catatan per topik yang tumbuh seiring waktu: jebakan yang sudah ditemukan, kesalahan yang pernah dibuat, alasan sebuah pendekatan ditinggalkan. |

### 13.1 Aturan sinkronisasi, wajib

Ini aturan yang diminta user secara eksplisit dan berlaku untuk setiap sesi, setiap agent, tanpa pengecualian.

**Setiap perubahan wajib dicatat di memori pada sesi yang sama.** Bukan di akhir minggu, bukan di sesi berikutnya. Kalau sebuah pendekatan dicoba lalu gagal, alasannya masuk ke `memory/`. Kalau sebuah asumsi tentang gmgn atau tentang chain terbukti salah, koreksinya masuk ke `memory/` saat itu juga.

**Setiap perubahan sistem wajib diikuti pembaruan dokumen yang terdampak, dalam commit yang sama.** Kalau kontrak pesan berubah, bagian 7.3 ikut berubah. Kalau gate risiko bertambah, bagian 5.4 ikut berubah. Kalau alamat kontrak diperbarui, `docs/CHAIN.md` ikut berubah. Dokumen yang menyimpang dari kode lebih berbahaya daripada tidak ada dokumen sama sekali, karena agent berikutnya akan mempercayainya.

Pemeriksaan berikut menegakkan aturan itu secara mekanis, bukan mengandalkan niat baik:

| Pemeriksa | Yang dicek |
| --- | --- |
| `cargo xtask verify` | Alamat kontrak, hash bytecode, jalur akses chart, bentuk respons endpoint gmgn. Menjaga dokumen tetap jujur terhadap dunia luar |
| `cargo xtask docs-check` | Setiap tipe pesan di kode punya pasangan di bagian 7.3. Setiap tabel basis data di kode punya pasangan di 7.4. Setiap gate di modul `exec` punya baris di 6.4. Selisih berarti build gagal |
| Templat commit | Punya kotak centang untuk dokumen terdampak dan catatan memori. Tidak dicentang berarti belum selesai |

Aturan tambahan: setiap modul Rust dibuka dengan komentar kepala yang menyatakan invarian modul itu, sehingga pembaca tahu apa yang tidak boleh dirusak sebelum menyentuh isinya.

---

## 14. Roadmap

| Fase | Isi | Definisi selesai |
| --- | --- | --- |
| 0 | Spesifikasi | Dokumen ini disetujui, repo dibuat, `CLAUDE.md` dan `docs/` terisi |
| 1 | Fondasi | Engine Rust jalan, pairing dengan side panel berhasil, keystore berfungsi, RPC sehat, `cargo xtask verify` hijau |
| 2 | Chart dan matematika | Garis muncul otomatis di setiap halaman token gmgn dan bisa digeser, konversi satuan benar, tab Range menampilkan tick dan proyeksi dasar. Belum ada transaksi |
| 3 | Eksekusi V3 | Buka, klaim, tutup di V3 dengan seluruh tujuh gate aktif. Mode dry run dulu, lalu nominal kecil |
| 4 | Eksekusi V4 dan zap | Jalur V4 termasuk penanganan hook dan `minHopPriceX36`, zap satu sisi, uji diferensial hijau |
| 5 | Positions, PnL, dan notifikasi | Kedua tab lengkap termasuk posisi dari luar Rangefinder yang dikelola penuh, akuntansi biaya penuh, ekspor kartu, toast di halaman gmgn beserta suaranya |
| 6 | Signals, Optimizer, dan doktrin penempatan | Detektor sideways, detektor dump lalu basis, filter playbook, gate risiko, Range Optimizer, peta kepadatan likuiditas, penempatan otomatis menurut 5.7 |
| 7 | Self improvement | Perekaman fitur dan hasil, kalibrasi, skema penantang, mode replay |

Fase 3 tidak dimulai sebelum fase 2 punya uji integrasi terhadap snapshot gmgn. Fase 4 tidak dimulai sebelum golden vector V3 lengkap.

---

## 15. Risiko

| Risiko | Dampak | Mitigasi |
| --- | --- | --- |
| gmgn mengganti bundle sehingga `tradingViewApi` hilang | Fitur inti mati | Empat tingkat degradasi di 4.4, uji terhadap snapshot, status kapabilitas terlihat di panel |
| gmgn memblokir pola akses API | Signals dan data token mati | Semua panggilan lewat sesi user dengan laju terbatas. Posisi berjalan tidak bergantung gmgn, hanya butuh RPC |
| Calldata router fork salah | Transaksi gagal atau dana tersangkut | Golden vector, simulasi wajib, batas nominal untuk jalur baru |
| Hook V4 tak dikenal berperilaku bermusuhan | Kehilangan dana | Allowlist hook, tolak secara default |
| Token berpajak merusak matematika posisi | Posisi tidak bisa ditutup dengan benar | Blokir keras berdasarkan simulasi transfer, bukan berdasarkan metadata |
| Chain tidak stabil, data tertunda | Keputusan berdasar harga basi | Tiga sumber harga, pemantauan lag blok, eksekusi dinonaktifkan saat data tidak sehat |
| Perangkat disusupi | Hot wallet terkuras | Wallet terpisah, batas harian, kunci otomatis, tidak ada penyimpanan passphrase secara default |
| Loop self improvement menyetel ke arah salah | Kualitas keputusan menurun perlahan | Batas keras pada bobot, skema penantang, kemampuan mengembalikan versi |
| Basis pasca dump ternyata kaki turun kedua | LP membeli sepanjang jalan turun | Penjaga dead cat di 5.2b syarat 3 sampai 5, level pembatalan wajib, notifikasi kritis saat pembatalan tertembus |
| Notifikasi membanjiri layar saat harga bergetar di batas | User berhenti membaca notifikasi, lalu melewatkan yang penting | Periode pendinginan per jenis kejadian per posisi, tingkat keparahan yang jelas, suara bisa dimatikan per tingkat |
| Dokumen menyimpang dari kode | Agent berikutnya bekerja atas informasi salah | Aturan sinkronisasi di 13.1, ditegakkan `cargo xtask docs-check` dan templat commit |
| Tanpa batas nominal, satu kesalahan bisa memakai seluruh saldo | Kerugian sebesar isi wallet | Diterima secara sadar oleh user. Pengamannya adalah memakai wallet terpisah yang isinya dibatasi, dinyatakan di 9.5 |
| Cakupan melar | Tidak pernah selesai | Daftar non goal di 3.8 dan urutan fase yang mengikat |

---

## 16. Pertanyaan yang Sudah Dijawab

Lima pertanyaan terbuka pada draft awal sudah dijawab user pada 5 September 2026. Dicatat di sini beserta konsekuensinya, supaya alasan di balik keputusan tidak hilang.

| Pertanyaan | Jawaban | Konsekuensi pada rancangan |
| --- | --- | --- |
| Perlu banyak wallet, atau satu cukup | Satu saja | Tidak ada pemilih wallet di UI. Kolom `wallet_address` tetap disimpan supaya penambahan nanti tidak butuh migrasi basis data |
| Notifikasi keluar dari side panel, misalnya ke Telegram | Tidak. Injeksi saja ke pojok kanan atas halaman gmgn | Tidak ada integrasi Telegram. Toast di halaman jadi saluran utama, notifikasi Chrome jadi cadangan saat tidak ada tab gmgn. Rincian di 3.7 |
| Profil risiko modal di bawah 50 USD | 5 USD per posisi dan per hari, dua posisi aktif, 2,50 USD untuk pool baru | Menahan satu kegagalan pada nominal kecil dan membatasi eksposur maksimum dua posisi ke 10 USD |
| Posisi dari platform lain dikelola penuh atau ditampilkan saja | Dikelola penuh | Set aksi sama persis dengan posisi milik sendiri. Cost basis direkonstruksi dari log on chain. Rincian di 3.5 |
| BagsV4Hook langsung masuk allowlist atau tunggu audit | Masuk allowlist kalau memang banyak dipakai | Allowlist jadi berbasis ukuran adopsi on chain, bukan penilaian manual, dengan satu syarat mutlak: hook yang masih bisa diupgrade tetap ditolak berapa pun adopsinya. Kriteria di 5.4 |

### Pertanyaan yang masih terbuka

1. Berapa ambang minimum porsi likuiditas aktif yang masuk akal pada langkah 5 di 5.7. Angka ini paling baik ditetapkan setelah ada 20 sampai 30 posisi nyata sebagai pembanding.
2. Apakah range bertingkat di 11.7 masuk v1 atau ditunda. Fitur ini menambah kompleksitas rebalance secara nyata, dan mungkin lebih baik menunggu sampai penempatan satu lapis terbukti menguntungkan.
