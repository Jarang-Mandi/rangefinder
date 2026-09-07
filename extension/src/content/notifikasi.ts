/**
 * Penerima notifikasi di halaman gmgn.
 *
 * KENAPA MENYAMBUNG SENDIRI KE ENGINE
 * Notifikasi harus tetap sampai saat side panel tertutup, dan itu justru
 * keadaan yang paling umum: user menatap chart, bukan panel. Kalau jalurnya
 * lewat panel, notifikasi berhenti persis saat ia paling dibutuhkan.
 *
 * Karena itu berkas ini membuka koneksi WebSocket-nya sendiri ke engine. Ia
 * hanya mendengarkan, tidak pernah mengirim perintah apa pun selain denyut.
 *
 * INVARIAN
 * - Token pairing dibaca dari `chrome.storage.local` dan dititipkan di
 *   subprotocol, tidak pernah masuk URL. Sama seperti di panel.
 * - Koneksi ini TIDAK boleh mengirim pesan yang mengubah keadaan. Halaman gmgn
 *   adalah lingkungan pihak ketiga, dan skrip di sana bisa berubah kapan saja.
 *   Yang bisa dilakukan paling jauh hanyalah menampilkan sesuatu.
 * - Audio baru dinyalakan setelah ada interaksi user pada halaman ini, karena
 *   Chrome memblokirnya sampai saat itu. Sebelum itu toast tetap muncul, hanya
 *   tanpa suara.
 */

import {
  DEFAULT_ENGINE_ORIGIN,
  WS_SUBPROTOCOL,
  durasiToastMs,
  type AlertLevel,
  type Kejadian,
} from "../shared/protocol";
import { bunyikan, aktifkan, sudahAktif } from "./suara";
import { tampilkanToast } from "./toast";

const TOKEN_KEY = "rangefinder.pairingToken";
const SUARA_KEY = "rangefinder.suaraAktif";

/** Jeda sambung ulang, naik berlipat sampai batas. */
const SAMBUNG_ULANG_MIN_MS = 2_000;
const SAMBUNG_ULANG_MAKS_MS = 30_000;
/** Selang denyut, menjaga koneksi tetap hidup. */
const DENYUT_MS = 20_000;

let jeda = SAMBUNG_ULANG_MIN_MS;
let socket: WebSocket | null = null;
let denyut: number | undefined;

/** Judul yang enak dibaca untuk tiap kode kejadian. */
function judulUntuk(k: Kejadian): string {
  switch (k.code) {
    case "position_out_of_range":
      return "Posisi keluar range";
    case "position_back_in_range":
      return "Posisi kembali masuk range";
    case "open_unverified":
      return "Posisi belum terverifikasi";
    case "open_failed":
      return "Gagal membuka posisi";
    case "zap_failed":
      return "Zap gagal";
    case "approve_failed":
      return "Pemberian izin gagal";
    case "plan_failed":
      return "Rencana tidak bisa disusun";
    case "rpc_unhealthy":
      return "RPC bermasalah";
    case "rpc_healthy":
      return "RPC kembali sehat";
    case "wallet_auto_locked":
      return "Wallet terkunci otomatis";
    default:
      return "Rangefinder";
  }
}

function tampilkan(k: Kejadian): void {
  // Yang tertahan pendinginan tidak pernah dikirim engine, tapi penjaga ini
  // tetap ada supaya perubahan di sisi engine tidak diam diam membanjiri layar.
  if (k.ditahan) return;

  tampilkanToast({
    level: k.level as AlertLevel as "info" | "warning" | "critical",
    judul: judulUntuk(k),
    keterangan: k.message,
    durasiMs: durasiToastMs(k.level),
  });

  bunyikan(k.level as "info" | "warning" | "critical");
}

function bersihkanDenyut(): void {
  if (denyut !== undefined) {
    self.clearInterval(denyut);
    denyut = undefined;
  }
}

async function sambung(): Promise<void> {
  let token = "";
  try {
    const tersimpan = await chrome.storage.local.get(TOKEN_KEY);
    const nilai = tersimpan[TOKEN_KEY];
    if (typeof nilai === "string") token = nilai;
  } catch {
    // Konteks ekstensi bisa hilang saat ekstensi dimuat ulang. Bukan error.
    return;
  }
  if (!token) {
    // Belum dipasangkan. Coba lagi nanti, karena user bisa memasangkannya
    // kapan saja tanpa memuat ulang halaman ini.
    self.setTimeout(() => void sambung(), SAMBUNG_ULANG_MAKS_MS);
    return;
  }

  const url = DEFAULT_ENGINE_ORIGIN.replace(/^http/, "ws") + "/ws";
  try {
    socket = new WebSocket(url, [WS_SUBPROTOCOL, token]);
  } catch {
    self.setTimeout(() => void sambung(), jeda);
    return;
  }

  socket.onopen = () => {
    jeda = SAMBUNG_ULANG_MIN_MS;
    bersihkanDenyut();
    denyut = self.setInterval(() => {
      socket?.send(JSON.stringify({ type: "ping" }));
    }, DENYUT_MS);
  };

  socket.onmessage = (ev) => {
    let m: Record<string, unknown>;
    try {
      m = JSON.parse(ev.data as string) as Record<string, unknown>;
    } catch {
      return;
    }
    if (m.type !== "notify") return;
    const { type: _t, ...isi } = m;
    tampilkan(isi as unknown as Kejadian);
  };

  socket.onclose = () => {
    bersihkanDenyut();
    socket = null;
    self.setTimeout(() => void sambung(), jeda);
    jeda = Math.min(jeda * 2, SAMBUNG_ULANG_MAKS_MS);
  };

  socket.onerror = () => {
    socket?.close();
  };
}

/**
 * Nyalakan audio pada interaksi pertama user di halaman ini.
 *
 * Chrome memblokir audio sampai ada gesture, dan gesture apa pun cukup. Karena
 * user pasti mengklik sesuatu di gmgn, ini hampir selalu berhasil tanpa dia
 * harus melakukan apa apa. Panel tetap punya tombol eksplisit untuk kasus
 * halaman yang dibuka lalu didiamkan.
 */
function pasangPemicuAudio(): void {
  const coba = () => {
    void chrome.storage.local.get(SUARA_KEY).then((s) => {
      // Bawaannya menyala. Yang tersimpan hanya dipakai untuk mematikan.
      if (s[SUARA_KEY] === false) return;
      if (!sudahAktif()) aktifkan();
    });
  };
  self.addEventListener("pointerdown", coba, { once: false, passive: true });
  self.addEventListener("keydown", coba, { once: false, passive: true });
}

export function mulaiNotifikasi(): void {
  pasangPemicuAudio();
  void sambung();
}
