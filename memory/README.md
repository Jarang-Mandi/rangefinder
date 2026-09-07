# Memori repo

Catatan per topik yang tumbuh seiring waktu. Isinya jebakan yang sudah
ditemukan, kesalahan yang pernah dibuat, dan alasan sebuah pendekatan
ditinggalkan.

## Aturan

Aturan ini diminta user secara eksplisit dan berlaku untuk setiap sesi dan
setiap agent.

1. **Setiap perubahan dicatat pada sesi yang sama.** Bukan nanti, bukan di
   akhir minggu.
2. **Setiap perubahan sistem diikuti pembaruan dokumen dalam commit yang sama.**
   Kontrak pesan berubah berarti PRD 7.3 ikut berubah. Alamat berubah berarti
   `docs/CHAIN.md` ikut berubah.
3. **Asumsi yang terbukti salah dicatat di sini, bukan hanya diperbaiki.**
   Perbaikan menyelesaikan hari ini. Catatan mencegah pengulangan.

## Bedanya dengan docs

| Tempat | Isi |
| --- | --- |
| `docs/PRD.md` | apa yang dibangun dan kenapa |
| `docs/DECISIONS.md` | keputusan arsitektural beserta pemicu peninjauan ulang |
| `docs/GMGN.md`, `docs/CHAIN.md` | kontrak terhadap sistem luar dan cara memverifikasinya |
| `memory/` | pelajaran. Hal hal yang membuang waktu, dan cara tidak mengulanginya |

## Penamaan

`NNN-topik-singkat.md`, nomor berurutan. Satu topik per berkas.

## Daftar

| Berkas | Isi |
| --- | --- |
| `001-toolchain-windows.md` | Kenapa build gagal di mesin ini dan apa yang sudah dicoba |
| `002-tradingview-gmgn.md` | Jebakan saat menyentuh chart gmgn |
| `003-rpc-rate-limit.md` | RPC membatasi laju, dan bahaya menelan kegagalan jadi nol |
| `004-rust-di-disk-d.md` | Rust dipindah ke D, dan jebakan saat memindahkannya |
| `005-pasangan-token-dan-fee-jebakan.md` | Token lawan tidak boleh ditebak, dan pool ber-fee ekstrem |
