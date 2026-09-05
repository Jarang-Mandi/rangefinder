# Rangefinder

Product Requirements Document

| Field | Value |
| --- | --- |
| Versi | 0.1 (draft untuk review) |
| Tanggal | 5 September 2026 |
| Status | Belum ada kode. Dokumen ini adalah kontrak sebelum implementasi. |
| Repo | `D:\Rangefinder` |

## Keputusan yang sudah terkunci

| Keputusan | Pilihan | Konsekuensi |
| --- | --- | --- |
| Custody | Hot wallet lokal terenkripsi di dalam engine Rust, dengan hard cap per transaksi dan per hari | Engine bisa mengeksekusi tanpa popup wallet. Semua pengaman risiko ada di sisi engine. |
| Protokol | Uniswap V3 dan V4 sekaligus | Cakupan pool paling luas. Wajib lapisan validasi berlapis karena permukaan bug dua kali lipat. |
| Otomasi | Semi otomatis: engine memantau, side panel menampilkan kartu aksi, user klik sekali | Tidak ada transaksi yang terjadi tanpa persetujuan eksplisit per aksi. |
| Chain | Robinhood Chain saja (chain id 4663) | Kode tetap memakai chain abstraction supaya penambahan chain lain nanti murah. |
| Wallet | Satu wallet saja | Tidak ada pemilih wallet di UI. Struktur data tetap menyimpan `wallet_address` supaya penambahan nanti tidak butuh migrasi. |
| Batas nominal | Tanpa batas secara bawaan | `maxPositionUsd` dan `maxDailyDeployUsd` tetap ada di kode dan bisa diisi user, tapi nilai bawaannya tidak terbatas. Rincian di 9.2. |
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

**Acceptance criteria.**

- Paste alamat valid menavigasi tab gmgn dalam waktu di bawah 1 detik.
- Alamat tidak valid, bukan checksum EVM, atau token tanpa pool ditolak dengan pesan spesifik, bukan pesan umum.
- Dua garis muncul dengan warna hijau untuk atas dan merah untuk bawah, ketebalan 2, label harga aktif.
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
- Setiap kartu menampilkan pair, versi protokol, fee tier, range, harga sekarang relatif range, status in range atau out of range, durasi, fee terkumpul yang belum diklaim, dan net PnL berjalan.
- Indikator posisi harga di dalam range berupa bar horizontal solid, bukan gradient.
- Klik kartu membuka daftar aksi: Tambah likuiditas, Rebalance ke range baru, Klaim fee, Tutup posisi, Tutup lalu swap ke ETH.
- Setiap aksi menampilkan simulasi hasil sebelum tombol konfirmasi aktif, termasuk perkiraan gas dan slippage.
- Posisi yang keluar range lebih dari ambang waktu yang diset user naik ke puncak daftar dan ditandai.
- Data posisi diperbarui melalui WebSocket dari engine, tanpa polling dari sisi panel.

### 3.6 Tab PnL

**Story.** Sebagai operator LP, saya ingin kartu hasil untuk posisi yang selesai maupun yang masih jalan, supaya saya tahu strategi mana yang benar benar menghasilkan.

**Acceptance criteria.**

- Kartu memisahkan dengan jelas fee yang diperoleh, impermanent loss, biaya gas, biaya slippage zap, dan net akhir.
- Ada ringkasan agregat: total net, win rate, median time in range, dan fee APR terealisasi.
- Bisa difilter berdasarkan rentang waktu, token, versi protokol, dan tipe strategi.
- Kartu bisa diekspor jadi PNG.
- Angka net PnL memakai basis yang bisa dipilih user antara USD atau ETH.

### 3.7 Notifikasi

**Story.** Sebagai operator LP, saya ingin diberi tahu setiap ada kejadian penting pada posisi saya, tanpa harus menatap side panel terus menerus.

**Tempat munculnya.** Notifikasi utama adalah toast yang diinjeksi content script ke **pojok kanan atas halaman gmgn**. Ini pilihan yang disengaja: side panel hanya hidup saat dibuka, sedangkan halaman gmgn hampir selalu terbuka saat user bekerja. Toast menumpuk ke bawah, maksimal empat sekaligus, sisanya masuk antrean.

Jika tidak ada satu pun tab gmgn yang terbuka, engine jatuh ke `chrome.notifications` bawaan sistem sehingga kejadian kritis tetap sampai.

**Tingkat dan perilaku.**

| Tingkat | Contoh kejadian | Perilaku toast | Suara |
| --- | --- | --- | --- |
| Info | Fee terkumpul melewati ambang klaim, sinyal baru berskor tinggi, posisi kembali masuk range | Hilang sendiri setelah 6 detik | Nada pendek satu ketukan, pelan |
| Peringatan | Harga mendekati batas range, dilusi likuiditas melewati ambang, aturan keluar berbasis imbal hasil terpicu | Hilang sendiri setelah 12 detik | Nada dua ketukan |
| Kritis | Posisi keluar range, transaksi gagal atau revert, data harga tidak sehat, RPC mati, verifikasi pasca kirim tidak cocok | Menetap sampai diklik | Nada tiga ketukan, lebih tegas |

**Daftar kejadian yang wajib memicu notifikasi.**

```
posisi dibuka berhasil            posisi dibuka gagal
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
2. Chrome memblokir autoplay audio sampai ada interaksi user pada halaman itu. Karena itu side panel punya tombol sekali pakai untuk mengaktifkan suara, yang menyalakan `AudioContext` dan menyimpan izinnya. Sebelum tombol itu ditekan, notifikasi tetap muncul tanpa suara, dan panel menjelaskan kenapa.
3. Berkas audio dibundel di dalam ekstensi sebagai file pendek, bukan diambil dari jaringan.

**Acceptance criteria.**

- Toast muncul di pojok kanan atas halaman gmgn dalam waktu di bawah 300 ms setelah engine memancarkan kejadian.
- Toast tidak pernah menutupi harga terakhir atau tombol beli jual milik gmgn.
- Setiap toast punya judul, satu baris keterangan, dan aksi lanjutan jika relevan, misalnya Buka posisi atau Rebalance sekarang.
- Suara bisa dimatikan seluruhnya, atau dimatikan per tingkat.
- Kejadian yang sama untuk posisi yang sama tidak diulang dalam periode pendinginan yang bisa diatur, bawaan 5 menit, supaya harga yang bergetar di sekitar batas tidak membanjiri layar.
- Riwayat 50 notifikasi terakhir bisa dibuka dari side panel.

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
  -> probe kapabilitas, kirim laporan ke side panel
  -> gambar garis
  -> subscribe('drawing_event')
```

Probe kapabilitas memeriksa keberadaan setiap method yang dipakai, lalu mengirim hasilnya ke panel sebagai status chip. Jika satu method hilang karena gmgn mengganti versi Charting Library, panel menampilkan mode degradasi yang aktif, bukan gagal diam diam.

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
mcap_usd  ->  price_usd  ->  price_weth  ->  raw_price  ->  tick
```

1. `price_usd = mcap_usd / total_supply`. Ambil `total_supply` dari endpoint rank gmgn, verifikasi silang dengan `totalSupply()` on chain. Jika beda lebih dari 0,5 persen, pakai nilai on chain dan tandai.
2. `price_weth = price_usd / eth_usd`. Harga ETH diambil dari `/api/v1/major_coin_prices` dan diverifikasi silang dengan pool WETH stablecoin on chain.
3. Tentukan urutan token. `token0` adalah alamat yang lebih kecil secara numerik. Jika memecoin adalah token0, maka harga pool adalah WETH per memecoin. Jika terbalik, semua nilai harus diinversi sebelum masuk rumus tick.
4. `raw_price = price_weth * 10^(dec1 - dec0)`.
5. `tick = round( ln(raw_price) / ln(1.0001) )`.
6. Snap ke grid: `tickLower = floor(tick / tickSpacing) * tickSpacing`, `tickUpper = ceil(tick / tickSpacing) * tickSpacing`.

Tick spacing V3 per fee tier: 100 memakai 1, 500 memakai 10, 3000 memakai 60, 10000 memakai 200. Di V4, `tickSpacing` adalah bagian dari PoolKey dan bisa bernilai apa saja, jadi wajib dibaca dari pool, bukan diasumsikan.

Aturan tampilan: panel selalu menunjukkan harga hasil pembulatan tick, bukan harga mentah yang digeser user, supaya angka di panel sama persis dengan yang akan tercatat on chain. Selisih pembulatan ditampilkan dalam basis poin.

Chart gmgn punya toggle `Price / MC` dan `USD / ETH`, dan toggle skala `log`. Content script membaca simbol aktif melalui `activeChart().symbol()` untuk mendeteksi mode, lalu menyesuaikan konversi. Perubahan mode oleh user memicu penggambaran ulang garis.

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

   Hasil uji disimpan di `tokens.tax_verified_at` beserta nilainya, dan diuji ulang jika sudah lebih tua dari 24 jam, karena kontrak dengan owner aktif bisa mengubah daftar pengecualian kapan saja.
2. **Uji honeypot.** Simulasikan jual kecil lewat router. Jika revert, token ditolak.
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

Zap dihitung dengan menyelesaikan berapa bagian dari modal yang harus ditukar supaya sisa dua token tepat memenuhi rasio di atas, dengan memperhitungkan dampak harga dari swap itu sendiri. Iterasi Newton dua sampai tiga putaran sudah cukup konvergen, dan hasilnya diverifikasi dengan QuoterV2 atau V4Quoter sebelum dikirim.

Mengikuti LPZenith: jika grid tick tidak memungkinkan penempatan range dalam 1 persen dari harga live, posisi ditolak dan engine menyarankan pool dengan fee tier lebih halus.

### 6.2 Jalur V3

Mint lewat `NonfungiblePositionManager.mint(MintParams)`. Posisi berupa NFT ERC721. Approval memakai Permit2 supaya tidak perlu approve terpisah tiap token. Seluruh langkah dibungkus `multicall` milik NonfungiblePositionManager agar atomik.

Aksi lain: `increaseLiquidity`, `decreaseLiquidity`, `collect`, `burn`. Rebalance adalah `decreaseLiquidity` penuh, lalu `collect`, lalu `mint` baru, semuanya dalam satu multicall.

### 6.3 Jalur V4

V4 memakai singleton PoolManager dan PositionManager periphery. Likuiditas diubah lewat `modifyLiquidities(bytes unlockData, uint256 deadline)`, di mana `unlockData` adalah pasangan `abi.encode(actions, params[])`. Aksi yang dipakai: `MINT_POSITION`, `INCREASE_LIQUIDITY`, `DECREASE_LIQUIDITY`, `BURN_POSITION`, `TAKE_PAIR`, `SETTLE_PAIR`, `CLOSE_CURRENCY`.

Tiga hal khusus chain ini:

1. **UniversalRouter fork.** Struct swap V4 membawa field tambahan `minHopPriceX36`. Encoder dibuat manual di modul `router_rbh`, tidak memakai encoder SDK. Modul ini punya suite golden vector tersendiri.
2. **Hook.** `PoolKey` berisi `currency0`, `currency1`, `fee`, `tickSpacing`, `hooks`. Alamat hook wajib dicek terhadap allowlist sebelum transaksi apa pun disusun.
3. **Native ETH.** V4 mendukung ETH asli sebagai currency, bukan hanya WETH. Engine memilih jalur dengan biaya paling rendah dan mencatat pilihan itu di log posisi.

Matematika likuiditas V4 identik dengan V3, jadi modul `math` dipakai bersama. Yang berbeda hanya pembungkus transaksi dan akuntansi currency.

### 6.4 Gate validasi berlapis

Ini permintaan eksplisit: V3 dan V4 sekaligus, tapi bug tidak boleh terjadi. Tujuh lapis, setiap transaksi wajib melewati semuanya, tidak ada jalan pintas.

| Lapis | Nama | Isi |
| --- | --- | --- |
| G1 | Verifikasi registry saat start | Setiap alamat kontrak di konfigurasi dicek `eth_getCode`, hash bytecode dibandingkan dengan nilai yang dipin di repo. Beda berarti engine menolak jalan. |
| G2 | Builder calldata bertipe | Semua struct dibangun lewat tipe Rust dari ABI, bukan string. Unit test per fungsi. |
| G3 | Golden vector | Calldata hasil builder dibandingkan byte per byte dengan calldata yang direkam dari UI Uniswap resmi untuk aksi yang setara. Wajib untuk V3 mint, V4 mint, dan setiap jalur router. Perbedaan sekecil apa pun menggagalkan build. |
| G4 | Simulasi | `eth_call` terhadap state pending sebelum kirim. Revert berarti batal, dan pesan revert didekode ke bahasa manusia. |
| G5 | Asersi invarian | Hasil simulasi diperiksa: tokenId tercipta, delta likuiditas sesuai, jumlah token dalam batas slippage, tidak ada token tersisa di kontrak, tick akhir sama dengan yang diminta. |
| G6 | Pembatas nilai | Mode dry run untuk semua fitur baru. Batas nominal per transaksi dan per hari. Posisi pertama pada pool baru dibatasi ke ukuran kecil yang bisa diatur. |
| G7 | Verifikasi pasca kirim | Setelah receipt, engine membaca ulang posisi on chain dan membandingkan dengan yang diharapkan. Selisih memicu alarm dan menghentikan otomasi. |

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
  store       SQLite lewat sqlx
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

Panel ke engine:

| Pesan | Isi | Fase |
| --- | --- | --- |
| `get_status` | tanpa isi, minta kiriman status terbaru sekarang juga | 1 |
| `ping` | denyut, sekaligus memundurkan kunci otomatis | 1 |
| `unlock_wallet` | `{ passphrase }`, lewat HTTP bukan WebSocket | 1 |
| `subscribe_token` | `{ address }` | 2 |
| `quote_range` | `{ pool, tickLower, tickUpper, sizeUsd, sourceToken }` | 3 |
| `open_position` | `{ quoteId }` | 3 |
| `rebalance` | `{ positionId, tickLower, tickUpper }` | 3 |
| `claim` | `{ positionId, swapToEth }` | 3 |
| `close` | `{ positionId, swapToEth }` | 3 |
| `scan` | `{ filters }` | 6 |

Engine ke panel:

| Pesan | Isi | Fase |
| --- | --- | --- |
| `hello` | `{ engineVersion, protocolVersion }`, dikirim sekali saat tersambung | 1 |
| `engine_status` | `{ engineVersion, uptimeSecs, chain, wallet, registry, canExecute, blockedReasons[] }` | 1 |
| `pong` | balasan denyut | 1 |
| `alert` | `{ level, code, message, positionId }` | 1 |
| `price_tick` | `{ address, priceUsd, priceWeth, mcapUsd }` | 2 |
| `range_quote` | `{ quoteId, feeApr, expectedHoursInRange, ilBreakevenHours, crowdedLiquidityUsd, ourSharePct, amount0, amount1, swapNeeded, priceImpact, gasEstimate, warnings[] }` | 3 |
| `tx_status` | `{ txId, stage, hash, error }` | 3 |
| `position_update` | `{ positionId, inRange, pricePct, feesUsd, netPnlUsd }` | 5 |
| `signal_list` | `{ items[] }` | 6 |

Content script ke engine, lewat service worker:

```
chart_capabilities { tier, methodsPresent[] }
chart_line_moved   { side, price }
gmgn_response      { requestId, status, body }
```

Engine ke content script:

```
draw_lines         { upperPrice, lowerPrice }
set_line_price     { side, price }
clear_lines        { }
gmgn_request       { requestId, path, method, body }
```

Semua pesan `quote_range` yang menghasilkan `range_quote` menyimpan `quoteId` beserta seluruh asumsi. `open_position` hanya menerima `quoteId` yang belum kedaluwarsa, default 20 detik. Ini mencegah eksekusi memakai angka yang sudah basi.

### 7.4 Model data

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
calibrations     id, created_at, weights_json, thresholds_json, active
```

Tabel `quotes` dan `signals` ada khusus untuk loop self improvement di bagian 12. Tanpa menyimpan prediksi beserta hasilnya, kalibrasi ulang tidak mungkin dilakukan.

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
- Kunci privat tidak pernah dikirim ke side panel, ke content script, atau keluar dari proses engine dalam bentuk apa pun.

### 9.2 Batas risiko

Dipaksakan di sisi engine, bukan di UI, sehingga tidak bisa dilewati dengan memanipulasi panel.

```
maxPositionUsd        nilai maksimum satu posisi                 bawaan: tidak terbatas
maxDailyDeployUsd     total maksimum yang dikerahkan per hari    bawaan: tidak terbatas
maxOpenPositions      jumlah posisi terbuka bersamaan            bawaan: tidak terbatas
newPoolFirstSizeUsd   batas ukuran posisi pertama di pool baru   bawaan: tidak terbatas
minEthReserve         saldo ETH yang disisakan untuk gas         bawaan: aktif
maxSlippageBps        batas slippage zap                         bawaan: aktif
```

Keputusan user: keempat pembatas nominal bernilai tidak terbatas secara bawaan. Mekanismenya tetap dibangun dan tetap dipaksakan di engine, sehingga user bisa menyalakannya kapan saja lewat konfigurasi tanpa perubahan kode.

Dua pembatas terakhir tetap aktif secara bawaan karena sifatnya berbeda. `minEthReserve` mencegah wallet kehabisan gas sehingga posisi tidak bisa ditutup, dan itu kegagalan operasional, bukan pilihan strategi. `maxSlippageBps` mencegah zap dieksekusi pada harga yang jauh meleset, dan tanpa itu satu swap buruk bisa menghapus fee berhari hari.

Melampaui batas yang aktif berarti transaksi ditolak di engine dengan alasan yang dikembalikan ke panel.

### 9.3 Permukaan lokal

- Engine hanya mengikat ke `127.0.0.1`, tidak pernah ke `0.0.0.0`.
- Setiap request membawa bearer token. Token dibuat acak saat engine pertama kali jalan, ditampilkan sebagai kode pairing, dan dimasukkan user ke side panel satu kali.
- CORS dibatasi ke origin ekstensi.
- Aksi yang menandatangani transaksi butuh wallet dalam keadaan terbuka, dan setiap aksi tetap butuh klik user.
- Log tidak pernah memuat kunci, passphrase, atau token pairing.

### 9.4 Data pengguna

Tidak ada telemetri. Tidak ada server jarak jauh. Seluruh basis data, log, dan keystore berada di mesin user. Ekstensi tidak membaca tab selain gmgn.ai.

### 9.5 Batasan yang disadari

Hot wallet lokal berarti perangkat yang sudah disusupi sama dengan dana yang hilang. Ini konsekuensi yang diterima secara sadar demi otomasi dan kecepatan. Karena pembatas nominal bawaannya tidak terbatas, satu satunya pengaman yang tersisa adalah berapa banyak dana yang benar benar ada di wallet itu. Mitigasi yang wajib disampaikan ke user di layar setup: pakai wallet terpisah khusus operasi ini, isi hanya sebanyak yang siap dipertaruhkan, dan jangan menyimpan dana jangka panjang di sana.

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

### 11.6 LP satu sisi sebagai alat eksekusi

Range yang seluruhnya berada di bawah harga hanya berisi WETH dan berperan sebagai tangga limit buy yang dibayar fee. Kalau harga tidak pernah turun ke sana, tidak ada yang terbeli dan tidak ada yang rugi. Range yang seluruhnya di atas harga hanya berisi token dan berperan sebagai tangga limit sell. Ini mengubah Rangefinder dari alat yang hanya berguna saat pasar menyamping menjadi alat yang berguna setiap hari, dan cocok dengan pesan playbook untuk membeli penurunan alih alih mengejar lilin hijau.

### 11.7 Range bertingkat

Alih alih satu range lebar, sebar modal ke dua atau tiga range sempit yang bertumpuk. Efisiensi modal naik di zona inti, sementara lapisan luar menangkap sumbu harga tanpa menyeret seluruh posisi keluar range. Chart menampilkan beberapa pasang garis dengan ketebalan berbeda. Rebalance bisa dilakukan per lapisan, bukan seluruhnya.

### 11.8 Peringatan dilusi likuiditas

Ketika ada pihak lain menambahkan likuiditas besar yang bertumpang tindih dengan range kita, porsi fee kita langsung turun tanpa ada yang berubah di harga. Engine memantau event `Mint` dan `Burn` pada pool, menghitung ulang porsi kita, dan mengirim peringatan jika turun melewati ambang. Sebaliknya, penarikan besar oleh pihak lain adalah kabar baik dan juga dilaporkan. Sejauh yang saya tahu tidak ada alat ritel yang melakukan ini.

### 11.9 Aturan keluar berbasis imbal hasil, bukan hanya harga

Posisi yang tetap berada di dalam range tapi tidak menghasilkan fee adalah modal mati yang menanggung risiko tanpa dibayar. Selain aturan keluar range, tambahkan: jika setelah `N` menit rasio fee terhadap modal masih di bawah ambang, tutup. Pola ini sudah terbukti di proyek Meridian milik user pada `minFeePerTvl24h` dan `minAgeBeforeYieldCheck`, jadi bukan spekulasi.

### 11.10 Akuntansi biaya penuh

Fee kotor menipu. Kartu PnL wajib memisahkan gas buka, gas tutup, slippage zap, dan dampak harga, lalu menampilkan net. Tanpa ini, posisi kecil yang terlihat untung sebenarnya rugi setelah biaya, dan kesalahan itu akan terulang karena tidak pernah terlihat.

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

Setiap kali posisi dibuka, engine menyimpan vektor fitur lengkap: skor sideways beserta seluruh komponennya, lebar range dalam satuan ATR, porsi likuiditas kita, fee tier, versi protokol, umur token, metrik holder, jarak harga ke POC, prediksi fee APR, prediksi jam bertahan, dan jam impas IL. Setiap kali posisi ditutup, engine menyimpan hasilnya: fee terealisasi, IL, gas, slippage, net, dan durasi di dalam range.

### 12.2 Bagaimana kalibrasi berjalan

Setiap 30 posisi tertutup, atau saat diminta manual, engine menjalankan kalibrasi:

1. Regresi prediksi terhadap hasil, untuk mengukur bias sistematis pada proyeksi fee dan proyeksi waktu bertahan. Bias yang konsisten dikoreksi dengan faktor penyesuaian, bukan dengan mengubah rumus.
2. Model klasifikasi ringan atas vektor fitur terhadap hasil net positif, untuk menemukan fitur mana yang benar benar memprediksi.
3. Penyetelan bobot pada rumus skor di 5.2 dan ambang pada gate di 5.4, dalam batas yang sudah ditentukan.

### 12.3 Pengaman

Loop yang menyetel dirinya sendiri bisa menyetel dirinya ke arah yang salah. Karena itu:

- Kalibrasi baru tidak aktif sebelum ada minimal 30 posisi tertutup pada rezim pasar yang sama.
- Setiap bobot punya batas atas dan bawah keras. Kalibrasi tidak boleh keluar dari batas itu.
- Skema penantang: kalibrasi baru berjalan berdampingan sebagai bayangan selama 20 posisi. Kalibrasi lama tetap yang mengambil keputusan. Penantang baru menggantikan juara jika unggul secara konsisten.
- Semua versi kalibrasi tersimpan di tabel `calibrations` dan bisa dikembalikan dengan satu perintah.
- Tab PnL bisa memfilter berdasarkan versi kalibrasi, sehingga efeknya terlihat, bukan sekadar diklaim.

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
| Ambang bawaan `maxPositionUsd` dan `maxDailyDeployUsd` | Tanpa batas | Mekanisme tetap dibangun dan dipaksakan di engine, nilainya saja yang tidak terbatas. Karena itu pengaman satu satunya adalah isi wallet, dan hal itu dinyatakan terang terangan di 9.5 |
| Posisi dari platform lain dikelola penuh atau ditampilkan saja | Dikelola penuh | Set aksi sama persis dengan posisi milik sendiri. Cost basis direkonstruksi dari log on chain. Rincian di 3.5 |
| BagsV4Hook langsung masuk allowlist atau tunggu audit | Masuk allowlist kalau memang banyak dipakai | Allowlist jadi berbasis ukuran adopsi on chain, bukan penilaian manual, dengan satu syarat mutlak: hook yang masih bisa diupgrade tetap ditolak berapa pun adopsinya. Kriteria di 5.4 |

### Pertanyaan yang masih terbuka

1. Berapa ambang minimum porsi likuiditas aktif yang masuk akal pada langkah 5 di 5.7. Angka ini paling baik ditetapkan setelah ada 20 sampai 30 posisi nyata sebagai pembanding.
2. Apakah range bertingkat di 11.7 masuk v1 atau ditunda. Fitur ini menambah kompleksitas rebalance secara nyata, dan mungkin lebih baik menunggu sampai penempatan satu lapis terbukti menguntungkan.
