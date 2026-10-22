/**
 * Nada notifikasi, dibunyikan dari halaman gmgn.
 *
 * KENAPA DI SINI, BUKAN DI SERVICE WORKER
 * Service worker MV3 tidak punya akses ke Web Audio sama sekali. Pilihannya
 * `chrome.offscreen` dengan alasan `AUDIO_PLAYBACK`, atau content script di
 * halaman yang sudah terbuka. Content script dipilih karena halaman gmgn memang
 * sudah ada di sana, dan itu satu bagian bergerak lebih sedikit.
 *
 * KENAPA NADA DIBANGKITKAN, BUKAN BERKAS AUDIO
 * PRD 3.7 menyebut berkas audio pendek yang dibundel. Nada yang dibangkitkan
 * oscillator lebih baik untuk keperluan ini: tidak ada berkas yang harus dimuat,
 * tidak ada jeda pemuatan pertama, dan ukurannya nol. Bunyinya juga bisa
 * dibedakan per tingkat hanya dengan mengubah jumlah ketukan.
 *
 * INVARIAN
 * - Chrome memblokir audio sampai ada interaksi user pada halaman itu. Karena
 *   itu `AudioContext` baru dibuat setelah `aktifkan` dipanggil, dan kegagalan
 *   memutar TIDAK pernah dilempar ke pemanggil: notifikasi tanpa suara jauh
 *   lebih baik daripada notifikasi yang gagal muncul karena audionya bermasalah.
 * - Volume sengaja rendah. Nada yang mengagetkan akan dimatikan user, dan
 *   notifikasi yang dimatikan tidak memberi tahu apa apa.
 */

export type TingkatSuara = "info" | "warning" | "critical";

let ctx: AudioContext | null = null;
let aktif = false;

/**
 * Nyalakan audio. Wajib dipanggil dari dalam penanganan interaksi user.
 *
 * Mengembalikan benar kalau berhasil. Kegagalan biasanya berarti browser masih
 * menganggap belum ada interaksi.
 */
export function aktifkan(): boolean {
  try {
    const Konstruktor =
      self.AudioContext ??
      (self as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
    if (!Konstruktor) return false;
    ctx ??= new Konstruktor();
    void ctx.resume();
    aktif = true;
    return true;
  } catch {
    return false;
  }
}

export function sudahAktif(): boolean {
  return aktif;
}

export function matikan(): void {
  aktif = false;
}

/** Frekuensi dasar per tingkat. Makin tinggi tingkatnya, makin tegas nadanya. */
const FREKUENSI: Record<TingkatSuara, number> = {
  info: 660,
  warning: 550,
  critical: 440,
};

/** Berapa ketukan per tingkat, sesuai tabel PRD 3.7. */
export function ketukan(level: TingkatSuara): number {
  return level === "info" ? 1 : level === "warning" ? 2 : 3;
}

/** Jarak antar ketukan, dalam detik. */
const JEDA_KETUKAN = 0.16;
/** Panjang satu ketukan, dalam detik. */
const PANJANG_KETUKAN = 0.09;
/** Volume puncak. Rendah dengan sengaja. */
const VOLUME = 0.06;

/**
 * Bunyikan nada untuk sebuah tingkat.
 *
 * Diam saja kalau audio belum diaktifkan. Tidak pernah melempar.
 */
export function bunyikan(level: TingkatSuara): void {
  if (!aktif || !ctx) return;

  try {
    const mulai = ctx.currentTime;
    const jumlah = ketukan(level);

    for (let i = 0; i < jumlah; i += 1) {
      const t = mulai + i * JEDA_KETUKAN;
      const osc = ctx.createOscillator();
      const gain = ctx.createGain();

      osc.type = "sine";
      osc.frequency.value = FREKUENSI[level];

      // Amplop naik turun cepat. Tanpa ini, mulai dan berhentinya oscillator
      // menghasilkan letupan yang terdengar seperti kerusakan.
      gain.gain.setValueAtTime(0, t);
      gain.gain.linearRampToValueAtTime(VOLUME, t + 0.012);
      gain.gain.linearRampToValueAtTime(0, t + PANJANG_KETUKAN);

      osc.connect(gain);
      gain.connect(ctx.destination);
      osc.start(t);
      osc.stop(t + PANJANG_KETUKAN + 0.02);
    }
  } catch {
    // Audio yang gagal tidak boleh menjatuhkan notifikasinya. Toast tetap
    // muncul, hanya tanpa suara.
  }
}
