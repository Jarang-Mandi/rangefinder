import type { TokenState } from "../shared/protocol";

/** Pool hanya boleh diganti di layar setelah statistik satu snapshot lengkap. */
export function statistikPoolSiap(token: TokenState): boolean {
  return token.pools.length === 0 || token.pools.every((pool) =>
    pool.stats?.volume != null && pool.stats?.jendela_menit != null
  );
}

/**
 * Gabungkan kiriman token bertahap tanpa menahan saldo di belakang statistik.
 *
 * Engine sengaja mengirim saldo segera setelah pembacaan singkat selesai,
 * sementara statistik pool dapat menyusul beberapa detik kemudian. Snapshot
 * parsial tidak boleh mengganti daftar pool, tetapi saldonya tetap merupakan
 * data terbaru untuk token dan pasangan yang sama.
 */
export function gabungkanTokenState(
  sebelumnya: TokenState | null,
  berikutnya: TokenState,
): TokenState | null {
  const pasanganSama = Boolean(
    sebelumnya
    && sebelumnya.address.toLowerCase() === berikutnya.address.toLowerCase()
    && sebelumnya.selected_quote?.toLowerCase() === berikutnya.selected_quote?.toLowerCase(),
  );

  if (!statistikPoolSiap(berikutnya)) {
    if (!sebelumnya && berikutnya.saldo != null) {
      // Pada token yang baru dibuka belum ada snapshot pool lama. Tampilkan
      // metadata pairing dan saldo, tetapi kosongkan pool sampai seluruh angka
      // statistik siap sebagai satu daftar.
      return {
        ...berikutnya,
        quotes: berikutnya.quotes.map((quote) => ({ ...quote, pools: [] })),
        pools: [],
        selected_pool_id: null,
      };
    }
    return pasanganSama && berikutnya.saldo != null
      ? { ...sebelumnya!, saldo: berikutnya.saldo }
      : sebelumnya;
  }

  return berikutnya.saldo == null && pasanganSama && sebelumnya?.saldo != null
    ? { ...berikutnya, saldo: sebelumnya.saldo }
    : berikutnya;
}
