/** Judul singkat yang dibaca pengguna untuk tiap kode kejadian. */
export function judulUntuk(k: { code: string }): string {
  switch (k.code) {
    case "position_out_of_range":
      return "Posisi keluar range";
    case "position_back_in_range":
      return "Posisi kembali masuk range";
    case "open_unverified":
      return "Posisi belum terverifikasi";
    case "open_pending":
      return "Pembukaan sedang diproses";
    case "open_failed":
      return "Gagal membuka posisi";
    case "open_started":
      return "Membuka posisi";
    case "open_succeeded":
      return "Posisi dibuka";
    case "zap_succeeded":
      return "Zap selesai";
    case "zap_failed":
      return "Zap gagal";
    case "approve_failed":
      return "Pemberian izin gagal";
    case "approve_succeeded":
      return "Izin selesai";
    case "increase_started":
      return "Menambah likuiditas";
    case "increase_succeeded":
      return "Likuiditas ditambahkan";
    case "increase_pending":
      return "Tambah likuiditas diproses";
    case "increase_failed":
      return "Tambah likuiditas gagal";
    case "rebalance_started":
      return "Rebalance dimulai";
    case "rebalance_succeeded":
      return "Rebalance selesai";
    case "rebalance_pending":
      return "Rebalance diproses";
    case "rebalance_failed":
      return "Rebalance gagal";
    case "claim_started":
      return "Mengklaim fee";
    case "claim_succeeded":
      return "Fee diklaim";
    case "claim_pending":
      return "Klaim fee diproses";
    case "claim_failed":
      return "Klaim fee gagal";
    case "close_started":
      return "Menutup posisi";
    case "close_succeeded":
      return "Posisi ditutup";
    case "close_pending":
      return "Penutupan sedang diproses";
    case "close_failed":
      return "Penutupan posisi gagal";
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
