# Kontrak terhadap gmgn.ai

Semua yang ada di sini adalah hasil pengamatan langsung di browser pada
5 September 2026, bukan dokumentasi resmi. gmgn tidak menjanjikan apa pun
kepada kita, jadi setiap poin disertai cara memverifikasi ulang. Kalau ada satu
saja yang tidak lagi benar, perbaiki berkas ini pada sesi yang sama, dan catat
apa yang berubah di `memory/`.

## Cara memverifikasi ulang seluruh berkas ini

Buka halaman token mana pun di gmgn, buka konsol, lalu jalankan potongan kode
pada tiap bagian di bawah. Kalau salah satu tidak menghasilkan yang dijanjikan,
berarti gmgn sudah berubah dan lapisan degradasi di PRD 4.4 yang berlaku.

## Bentuk URL

```
https://gmgn.ai/robinhood/token/<contract_address>
```

Slug chain yang ada di bundle: `sol`, `eth`, `bsc`, `base`, `tron`, `blast`,
`monad`, `hyperevm`, `xlayer`, `arc`, `robinhood`.

Cara mengambil ulang daftar slug, jalankan di konsol halaman gmgn:

```js
const scripts = [...document.querySelectorAll('script[src]')]
  .map(s => s.src).filter(s => s.startsWith('https://gmgn.ai/_next'));
const found = new Set();
for (const s of scripts) {
  const t = await fetch(s).then(r => r.text());
  for (const m of t.matchAll(/chain["']?\s*[:=]\s*["']([a-z0-9_]{2,12})["']/g)) found.add(m[1]);
}
[...found].sort();
```

gmgn adalah aplikasi Next.js satu halaman. Berpindah token TIDAK memuat ulang
dokumen, jadi content script wajib mendeteksi navigasi lewat History API dan
`MutationObserver`, bukan lewat `load`.

## Chart

Chart adalah TradingView Charting Library di dalam iframe yang **same origin**,
sehingga isinya bisa diakses penuh.

| Hal | Nilai |
| --- | --- |
| Id iframe | berpola `tradingview_<hash>`, contoh `tradingview_e6ba9` |
| Objek API | `iframe.contentWindow.tradingViewApi` |
| Global lain di iframe | `TradingViewApi`, `chartWidget`, `chartWidgetCollection`, `widgetReady`, `doWhenApiIsReady` |
| Format simbol | `robinhood/<address>/USD/MCAP` |
| Resolution | `"1"` untuk satu menit |

Method yang tersedia pada `activeChart()`:

```
createShape            createMultipointShape   createPositionLine
createOrderLine        getAllShapes            getShapeById
removeEntity           getVisiblePriceRange    getVisibleRange
onDataLoaded           crossHairMoved          dataReady
priceFormatter         getSeries               executeActionById
getPanes
```

Method pada `tradingViewApi` sendiri: `subscribe`, `unsubscribe`, `chart`,
`activeChart`.

Properti shape yang bisa dioverride:

```
linecolor  textcolor  linewidth  linestyle  showPrice  fontsize
bold       italic     horzLabelsAlign       vertLabelsAlign
text       frozen     visible
```

Cara memverifikasi ulang:

```js
const f = document.querySelector('iframe[id^="tradingview_"]');
const c = f.contentWindow.tradingViewApi.activeChart();
({
  simbol: c.symbol(),
  resolusi: c.resolution(),
  rentangHarga: c.getVisiblePriceRange(),
  method: ['createShape','getShapeById','removeEntity','getAllShapes']
    .filter(m => typeof c[m] === 'function'),
});
```

### Membaca harga terakhir dari chart

Charting Library tidak menyediakan API publik untuk membaca lilin terakhir.
Jalurnya ada, tapi lewat struktur internal:

```js
const bars = chart.getSeries().data().m_bars;
const [time, open, high, low, close, volume] = bars.last().value;
```

Diverifikasi 5 September 2026 pada PONS: `close` bernilai 937.234.340
sementara judul halaman menampilkan $938.86M pada saat yang hampir sama, dan
`high` serta `low` konsisten mengapit `open` dan `close`.

**Kenapa ini dibutuhkan.** Chart bekerja dalam market cap USD, pool bekerja
dalam WETH. Untuk menempatkan garis pada nilai yang benar, kurs ETH ke USD
diturunkan dari chart itu sendiri, bukan dari sumber luar:

```
eth_usd = mcap_chart / (total_supply x harga_token_dalam_weth)
```

Dengan begitu garis mendarat persis di tempat yang ditunjukkan skala chart,
karena kalibrasinya memakai angka chart yang sama. Sumber harga luar akan
selalu sedikit meleset dari chart dan membuat garis bergeser.

**Ini jalur tidak resmi.** `m_bars` bisa berubah kapan saja. Karena itu
pembacanya memeriksa kewarasan OHLC dan melaporkan kegagalan secara eksplisit,
bukan diam diam mengembalikan angka yang salah.

### Jebakan yang sudah memakan waktu

**`createShape` mengembalikan Promise.** Menyimpan hasilnya tanpa `await` lalu
memanggil `getShapeById` menghasilkan `Error: There is no such shape`. Ini
sudah terjadi saat riset awal, dan penyebabnya tidak kelihatan dari pesan
errornya.

**`getAllShapes` mengembalikan shape milik user juga.** Jangan pernah menghapus
massal. Simpan entity id milik sendiri, hapus hanya itu. Entity id berupa
string pendek acak, contoh `jnm56k`.

**Satuan chart adalah market cap USD, bukan harga.** Konversi ke tick Uniswap
melewati empat langkah dan urutan token perlu diperiksa. Rumus lengkapnya di
PRD 4.3. Ini sumber bug paling halus di seluruh proyek.

## API internal

Panggilan dari dalam halaman gmgn berhasil dengan status 200 dan `code: 0`.

```
/defi/quotation/v1/rank/robinhood/swaps/{period}   orderby, direction, limit
/api/v1/mutil_window_token_info                    POST, dipakai halaman token
/api/v1/token_info_brief_multi_chain
/api/v1/token_holder_counts
/api/v1/token_prices
/api/v1/token_fee_info_polling
/api/v1/dex_trades_polling
/api/v1/major_coin_prices
/api/v1/gas_price_list
/api/v1/follow/follow_wallet_list
/api/v1/person_wallets_pnl_info
wss://ws.gmgn.ai                                   stream realtime
```

Field yang dikembalikan endpoint rank:

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

Request gmgn membawa parameter fingerprint: `device_id`, `tab_id`, `fp_did`,
`client_id`, `from_app`, `app_ver`, `tz_name`, `tz_offset`, `app_lang`, `os`.

**Karena itu engine tidak pernah memanggil gmgn sendiri.** Lihat
`docs/DECISIONS.md` entri 0001.

Cara memverifikasi ulang, jalankan di konsol halaman gmgn:

```js
const r = await fetch('/defi/quotation/v1/rank/robinhood/swaps/1h?orderby=volume&direction=desc&limit=5');
const j = await r.json();
({ status: r.status, code: j.code, jumlah: (j.data?.rank || []).length });
```

## Batas laju

Pemindaian tidak boleh melebihi satu request per dua detik. Ini bukan angka
resmi dari gmgn, melainkan batas yang kita tetapkan sendiri supaya sesi user
tidak diblokir. Sesi yang diblokir merugikan user jauh melebihi manfaat
memindai lebih cepat.

## Yang belum diketahui

- Bentuk pesan pada `wss://ws.gmgn.ai` belum dibedah. Dibutuhkan di fase 2
  untuk harga realtime.
- Perilaku endpoint saat user tidak login belum diuji menyeluruh. Saat riset,
  sesi dalam keadaan tidak login dan endpoint rank tetap menjawab.
- Chart tidak menampilkan lilin saat riset, kemungkinan karena data chain
  sedang tertunda. Jalur akses API tetap terbukti berfungsi, tapi penggambaran
  garis di atas chart berisi data belum pernah dilihat langsung.
