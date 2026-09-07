/**
 * Toast notifikasi yang diinjeksi ke halaman gmgn.
 *
 * KENAPA DI HALAMAN GMGN, BUKAN DI SIDE PANEL
 * Side panel hanya hidup saat dibuka. Halaman gmgn hampir selalu terbuka saat
 * user bekerja, jadi di situlah kabar penting punya peluang benar benar
 * terlihat. Ini keputusan yang diminta user secara eksplisit, dan tercatat di
 * PRD 3.7.
 *
 * INVARIAN
 * - Toast TIDAK BOLEH menutupi harga terakhir atau tombol beli jual milik gmgn.
 *   Karena itu ia menempel di pojok kanan atas, di bawah tinggi header, dan
 *   lebarnya dibatasi.
 * - Seluruh gaya ditulis inline pada elemen, bukan lewat stylesheet halaman.
 *   Halaman gmgn punya CSS-nya sendiri yang bisa berubah kapan saja, dan
 *   selector yang bertabrakan akan membuat toast tampil rusak tanpa peringatan.
 * - Maksimal empat toast sekaligus. Sisanya masuk antrean, karena tumpukan yang
 *   lebih tinggi dari itu mulai menutupi isi halaman.
 * - Tidak ada gradient, sesuai aturan desain proyek. Kedalaman dibedakan lewat
 *   border dan pergeseran nilai permukaan.
 */

export type TingkatToast = "info" | "warning" | "critical";

export interface IsiToast {
  level: TingkatToast;
  judul: string;
  keterangan: string;
  /** Nol berarti menetap sampai diklik. */
  durasiMs: number;
}

/** Paling banyak toast yang tampil bersamaan. Sisanya mengantre. */
export const MAKS_TAMPIL = 4;

const ID_WADAH = "rangefinder-toast-wadah";

/** Jarak dari atas, cukup untuk melewati header gmgn tanpa menutupinya. */
const ATAS_PX = 72;

const WARNA: Record<TingkatToast, { garis: string; titik: string }> = {
  info: { garis: "#2e9e63", titik: "#2e9e63" },
  warning: { garis: "#c98a2e", titik: "#c98a2e" },
  critical: { garis: "#d8484d", titik: "#d8484d" },
};

const antrean: IsiToast[] = [];
let tampil = 0;

function wadah(): HTMLElement {
  const ada = document.getElementById(ID_WADAH);
  if (ada) return ada;

  const el = document.createElement("div");
  el.id = ID_WADAH;
  // `pointer-events: none` pada wadah supaya area kosong di sekitarnya tidak
  // menghalangi klik ke halaman gmgn di bawahnya. Tiap toast menyalakannya
  // kembali untuk dirinya sendiri.
  el.setAttribute(
    "style",
    [
      "position: fixed",
      `top: ${ATAS_PX}px`,
      "right: 16px",
      "z-index: 2147483000",
      "display: flex",
      "flex-direction: column",
      "gap: 8px",
      "max-width: 320px",
      "pointer-events: none",
      "font-family: 'Plus Jakarta Sans', system-ui, sans-serif",
    ].join(";"),
  );
  document.documentElement.appendChild(el);
  return el;
}

function buatElemen(isi: IsiToast, tutup: () => void): HTMLElement {
  const w = WARNA[isi.level];
  const el = document.createElement("div");
  el.setAttribute(
    "style",
    [
      "pointer-events: auto",
      "background: #161920",
      "color: #e7eaf0",
      "border: 1px solid #333a47",
      `border-left: 3px solid ${w.garis}`,
      "border-radius: 10px",
      "padding: 10px 12px",
      "box-shadow: 0 2px 8px rgba(0,0,0,0.4)",
      "cursor: pointer",
      "font-size: 12px",
      "line-height: 1.45",
    ].join(";"),
  );

  const judul = document.createElement("div");
  judul.setAttribute(
    "style",
    "font-weight: 600; display: flex; align-items: center; gap: 6px",
  );
  const titik = document.createElement("span");
  titik.setAttribute(
    "style",
    `width: 6px; height: 6px; border-radius: 50%; background: ${w.titik}; flex: none`,
  );
  judul.appendChild(titik);
  judul.appendChild(document.createTextNode(isi.judul));

  const ket = document.createElement("div");
  ket.setAttribute("style", "color: #8d95a5; margin-top: 2px");
  ket.textContent = isi.keterangan;

  el.appendChild(judul);
  el.appendChild(ket);
  el.addEventListener("click", tutup);
  return el;
}

/**
 * Tampilkan sebuah toast, atau antrekan kalau layar sudah penuh.
 */
export function tampilkanToast(isi: IsiToast): void {
  if (tampil >= MAKS_TAMPIL) {
    antrean.push(isi);
    return;
  }

  tampil += 1;
  const induk = wadah();
  let sudah = false;

  const tutup = () => {
    // Penjaga ganda: toast bisa ditutup oleh klik dan oleh waktu sekaligus,
    // dan tanpa ini penghitungnya akan turun dua kali sehingga slot bocor.
    if (sudah) return;
    sudah = true;
    el.remove();
    tampil -= 1;
    const berikut = antrean.shift();
    if (berikut) tampilkanToast(berikut);
  };

  const el = buatElemen(isi, tutup);
  induk.appendChild(el);

  if (isi.durasiMs > 0) {
    self.setTimeout(tutup, isi.durasiMs);
  }
}

/** Berapa toast yang sedang tampil. Dipakai uji. */
export function jumlahTampil(): number {
  return tampil;
}

/** Berapa yang mengantre. Dipakai uji. */
export function jumlahAntre(): number {
  return antrean.length;
}

/** Kosongkan seluruhnya. Dipakai uji dan saat halaman berpindah token. */
export function bersihkan(): void {
  document.getElementById(ID_WADAH)?.remove();
  antrean.length = 0;
  tampil = 0;
}
