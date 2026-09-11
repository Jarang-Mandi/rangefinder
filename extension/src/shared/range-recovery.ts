import type { RangeView } from "./protocol";

/**
 * Halaman dengan token yang sama tetap perlu menerima garis setelah reload.
 * State sidepanel bertahan, sedangkan objek garis milik halaman tidak.
 */
export function perluGambarUlang(
  tokenAktif: string | null,
  tokenHalaman: string,
  view: RangeView | null,
): view is RangeView {
  return tokenAktif?.toLowerCase() === tokenHalaman.toLowerCase() && view !== null;
}
