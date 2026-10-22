# 003 RPC publik membatasi laju, dan kesalahan yang ditelan menyesatkan

Ditulis 6 September 2026, saat fase 3.

## Kejadiannya

Alat `cargo xtask dryrun` melaporkan saldo token nol untuk alamat yang jelas
jelas memegang token. Gate G6 lalu menolak dengan alasan cadangan ETH kurang,
padahal alamatnya punya 0,18 ETH.

Dua laporan itu salah, dan keduanya berasal dari satu sebab yang sama.

## Sebabnya

Kode saya menulis:

```rust
let eth_balance = provider.get_balance(from).await.unwrap_or(U256::ZERO);
let a = IERC20Write::new(t0, provider).balanceOf(pemilik).call().await
    .unwrap_or(U256::ZERO);
```

`unwrap_or(U256::ZERO)` mengubah kegagalan menjadi nol yang terlihat sah.
Setelah kegagalannya disurfacekan, penyebab sebenarnya kelihatan:

```
error code 429: Too Many Requests
```

RPC publik Robinhood Chain membatasi laju, dan membaca beberapa saldo berturut
turut sudah cukup untuk memicunya.

## Dua pelajaran

**Jangan pernah menelan kegagalan pembacaan menjadi nilai bawaan yang tampak
sah.** Nol yang palsu jauh lebih berbahaya daripada error, karena ia mengalir
ke logika berikutnya dan menghasilkan keputusan yang salah dengan penjelasan
yang meyakinkan. Pada proyek ini, itu berarti gate menolak dengan alasan yang
mengada ada, dan user akan mengejar masalah yang tidak ada.

Pola yang benar: naikkan errornya, atau kalau memang boleh gagal, bedakan
"tidak tahu" dari "nol".

**Rate limit adalah kondisi normal di chain ini, bukan kejadian luar biasa.**
Penanganannya sekarang ada di `chain::ChainClient::provider_for`, yang memasang
`RetryBackoffLayer` pada klien RPC. Lima percobaan dengan jeda awal 200 ms.
Karena dipasang di lapisan penyedia, seluruh pemanggil ikut terlindungi tanpa
perlu tahu apa apa.

Setelah lapisan itu dipasang, pembacaan saldo yang sama berhasil dan pipeline
berjalan sampai gate G4.

## Yang perlu diperiksa lagi nanti

Kalau pemakaian meningkat, misalnya saat pemindaian sinyal di fase 6 berjalan
terus menerus, lima percobaan mungkin tidak cukup. Tambahkan endpoint RPC
cadangan di `rpc.http`, karena `ChainClient` sudah bisa berpindah endpoint
sendiri saat satu endpoint gagal.
