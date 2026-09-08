/**
 * Kontrak pesan antara panel dan engine.
 *
 * INVARIAN
 * - Berkas ini adalah cerminan `engine/src/api/protocol.rs`. Kalau salah satu
 *   berubah, yang lain wajib ikut berubah dalam commit yang sama, begitu juga
 *   PRD bagian 7.3. Lihat aturan sinkronisasi di PRD 13.1.
 * - Token pairing TIDAK PERNAH masuk URL. Untuk HTTP ia ada di header
 *   Authorization, untuk WebSocket ia dititipkan di subprotocol, karena browser
 *   tidak mengizinkan memasang header pada koneksi WebSocket.
 */

export const PROTOCOL_VERSION = 1;
export const WS_SUBPROTOCOL = "rangefinder";
export const DEFAULT_ENGINE_ORIGIN = "http://127.0.0.1:8787";

export type AlertLevel = "info" | "warning" | "critical";

export type RegistryStatus = "verified" | "unpinned" | "no_code" | "mismatch";

export interface RegistryCheck {
  section: string;
  name: string;
  address: string;
  has_code: boolean;
  code_hash: string;
  pinned: string | null;
  status: RegistryStatus;
}

export interface RegistrySummary {
  total: number;
  verified: number;
  unpinned: number;
  no_code: number;
  checks: RegistryCheck[];
}

export interface ChainHealth {
  healthy: boolean;
  endpoint: string;
  chain_id: number | null;
  block_number: number | null;
  block_lag_secs: number | null;
  max_block_lag_secs: number;
  error: string | null;
  checked_at: string;
}

export interface WalletStatus {
  exists: boolean;
  locked: boolean;
  address: string | null;
  auto_lock_minutes: number;
  seconds_until_auto_lock: number | null;
}

export interface EngineStatus {
  engine_version: string;
  uptime_secs: number;
  chain: ChainHealth;
  wallet: WalletStatus;
  registry: RegistrySummary;
  can_execute: boolean;
  blocked_reasons: string[];
  /** Endpoint HTTP yang sedang dipakai, kunci API sudah disunting engine. */
  rpc_http: string[];
}

export type PoolVersion = "v3" | "v4";

/**
 * Angka pembanding antar pool pada token yang sama.
 *
 * Seluruhnya dalam token kutipan pool itu, bukan dolar. `tvl` hanya mencakup
 * pita di sekitar harga selebar `tvl_lebar_persen`, karena likuiditas yang jauh
 * dari harga tidak menerima fee apa pun.
 */
export interface PoolStats {
  volume: number;
  jendela_menit: number;
  tvl: number;
  tvl_lebar_persen: number;
  apr: number | null;
}

export interface PoolInfo {
  version: PoolVersion;
  /** Alamat pool untuk V3, atau poolId heksadesimal untuk V4. */
  id: string;
  token0: string;
  token1: string;
  /**
   * Angka fee apa adanya dari pool.
   *
   * Untuk pool V4 dengan dynamic fee, ini penanda `0x800000` yaitu 8388608,
   * bukan besaran fee. Periksa `dynamic_fee` sebelum menampilkannya sebagai
   * persen, kalau tidak akan tertulis 838,86 persen.
   */
  fee: number;
  /** Benar kalau fee ditentukan hook saat swap, bukan tetap. */
  dynamic_fee: boolean;
  tick_spacing: number;
  hooks: string | null;
  sqrt_price_x96: string;
  tick: number;
  liquidity: string;
  /** Harga mentah token1 per token0 pada satuan terkecil. */
  raw_price: number;
  /** Angka pembanding antar pool. Null berarti belum diukur, bukan nol. */
  stats?: PoolStats | null;
}

export interface TokenMeta {
  address: string;
  symbol: string;
  decimals: number;
  total_supply: number;
}

/**
 * Jawaban atas `subscribe_token`.
 *
 * `pools` sudah disaring engine, pool tanpa likuiditas tidak pernah ikut,
 * karena harganya berasal dari saat pool dibuat dan bisa meleset puluhan kali
 * lipat. Panel tidak perlu tahu pool itu ada.
 */
/** Sekelompok pool yang berbagi token lawan yang sama. */
export interface QuoteGroup {
  /** Alamat token lawan. Nol berarti ETH asli, yang hanya ada di V4. */
  quote: string;
  quote_symbol: string;
  quote_decimals: number;
  /**
   * Benar kalau token lawannya kutipan yang dikenal rantai ini, yaitu ETH asli
   * atau salah satu entri tokens di chain.toml. Kelompok kanonik didahulukan
   * saat memilih pasar utama.
   */
  canonical: boolean;
  pools: PoolInfo[];
}

export interface TokenState {
  address: string;
  meta: TokenMeta | null;
  /**
   * Pool dikelompokkan menurut token lawan.
   *
   * Satu token bisa berpasangan dengan beberapa token sekaligus. Pada FIG di
   * Robinhood Chain, 33 dari 39 poolnya berpasangan dengan USDG dan tidak satu
   * pun dengan WETH, jadi menebak pasangannya akan melewatkan seluruh pasar.
   */
  quotes: QuoteGroup[];
  /** Pool dari kelompok terpilih, diratakan. */
  pools: PoolInfo[];
  selected_quote: string | null;
  selected_pool_id: string | null;
  error: string | null;
}

/** Satu batas range, dalam tiga satuan sekaligus. */
export interface RangeBound {
  tick: number;
  price_weth: number;
  price_usd: number;
  mcap_usd: number;
  /** Selisih akibat pembulatan ke grid tick, dalam basis poin. */
  drift_bps: number;
}

export interface CurrentPrice {
  tick: number;
  price_weth: number;
  price_usd: number;
  mcap_usd: number;
}

/**
 * Hasil kalibrasi dan snap tick dari engine.
 *
 * Panel menampilkan angka ini apa adanya. Semua konversi satuan dikerjakan
 * engine, karena dua tempat yang menghitung hal yang sama pasti menyimpang, dan
 * pada proyek yang mengirim transaksi itu berarti angka di layar berbeda dari
 * yang tercatat on chain.
 */
export interface RangeView {
  pool_id: string;
  tick_spacing: number;
  /** Kurs yang tersirat dari chart, bukan dari sumber luar. */
  eth_usd: number;
  current: CurrentPrice;
  lower: RangeBound;
  upper: RangeBound;
  width_pct: number;
  in_range: boolean;
  warnings: string[];
}

/** Hasil satu gate validasi, ditampilkan apa adanya di panel. */
export interface GateResult {
  id: string;
  name: string;
  passed: boolean;
  detail: string;
}

/**
 * Satu langkah izin yang masih harus dikerjakan.
 *
 * Bentuknya seragam untuk V3 dan V4. V3 butuh satu langkah per token, yaitu
 * allowance ERC20 ke position manager. V4 butuh dua, karena ia menarik token
 * lewat Permit2: token mengizinkan Permit2, lalu Permit2 mengizinkan position
 * manager.
 *
 * Panel tidak perlu tahu bedanya. Ia menekan tombolnya sampai daftar ini
 * kosong, dan engine yang memutuskan langkah mana yang dikerjakan.
 */
export interface LangkahIzin {
  /** `erc20_ke_position_manager`, `erc20_ke_permit2`, atau `permit2_ke_position_manager`. */
  jenis: string;
  token: string;
  needed: string;
  current: string;
  /** Teks siap tampil, disusun engine supaya panel tidak menyusun kalimat sendiri. */
  label: string;
}

/**
 * Rencana membuka posisi yang sudah lolos gate, menunggu persetujuan.
 *
 * Calldata TIDAK ikut dikirim. Ia tinggal di engine, karena rencana yang
 * bolak balik lewat panel bisa saja kembali dalam keadaan sudah diubah, dan
 * bagian yang paling menarik untuk diubah justru calldata-nya.
 */
export interface OpenPlanView {
  pool_id: string;
  tick_lower: number;
  tick_upper: number;
  amount0: string;
  amount1: string;
  liquidity: string;
  token_id_simulasi: string;
  gas_estimate: number;
  dry_run: boolean;
  gates: GateResult[];
  warnings: string[];
  allowances: LangkahIzin[];
  /** `v3` atau `v4`. */
  version: string;
  /** Terisi kalau modal perlu ditukar lebih dulu supaya cocok rasio range. */
  zap: ZapView | null;
  /** Jumlah tiap sisi yang akan disetor, sudah berdesimal. */
  setor: SetoranView | null;
  /**
   * Proyeksi fee setahun untuk ukuran posisi ini, dalam persen.
   *
   * Berbeda dari APR di daftar pool, yang memakai modal acuan tetap. Yang ini
   * memakai modal yang benar benar disetor, jadi pengenceran oleh modal kita
   * sendiri ikut terhitung dan angkanya mengecil seiring posisi membesar.
   */
  apr_estimasi: number | null;
  expires_at: string;
}

export interface SetoranView {
  simbol_memecoin: string;
  jumlah_memecoin: number;
  simbol_quote: string;
  jumlah_quote: number;
  nilai_quote: number;
}

/**
 * Swap yang perlu dijalankan sebelum membuka posisi.
 *
 * Transaksi terpisah, dan itu bukan pilihan: swap berjalan lewat
 * UniversalRouter sedangkan posisi dibuat lewat PositionManager, dan tidak ada
 * kontrak yang bisa keduanya sekaligus.
 */
export interface ZapView {
  dari: string;
  ke: string;
  jumlah: string;
  perkiraan_diterima: string;
  minimum_diterima: string;
  porsi_bps: number;
  /** Modal yang akan menganggur kalau zap tidak dijalankan. */
  menganggur_tanpa_zap: string;
}

export type TxStatus = "sukses" | "revert" | "menggantung";

/** Hasil satu pengiriman transaksi. */
export interface SendResultView {
  /** `approve` atau `open`, supaya panel tahu kartu mana yang diperbarui. */
  kind: string;
  tx_hash: string;
  nonce: number;
  gas_limit: number;
  status: TxStatus;
  block_number: number | null;
  gas_used: number | null;
  token_id: string | null;
  verification: GateResult | null;
  sent_at: string;
}

/**
 * Komponen hasil sebuah posisi, dalam satuan token kutipan pool.
 *
 * Selalu dipisah, tidak pernah satu angka. Fee kotor menipu: posisi yang
 * mengumpulkan fee besar bisa tetap rugi setelah impermanent loss, gas, dan
 * slippage.
 */
export interface HasilPnl {
  fee: number;
  /** Positif berarti rugi. Ditampilkan sebagai pengurang. */
  impermanent_loss: number;
  gas: number;
  slippage: number;
  net: number;
  /** Benar kalau sebagian angkanya direkonstruksi, bukan dari catatan. */
  perkiraan: boolean;
}

export interface RingkasanPnl {
  jumlah: number;
  total_net: number;
  total_fee: number;
  total_biaya: number;
  win_rate_bps: number;
  perkiraan: number;
}

/** Satu candle. Waktu dalam detik unix. */
export interface Bar {
  time: number;
  open: number;
  high: number;
  low: number;
  close: number;
  volume: number;
}

/** Hasil detektor sideways, PRD 5.2. */
export interface SinyalSideways {
  skor: number;
  er: number;
  volume_z: number;
  containment: number;
  bandwidth: number;
  kontraksi: number;
  tape: number;
  /** Kosong berarti seluruh gate lolos. */
  ditolak: string[];
  kanal_bawah: number;
  kanal_atas: number;
}

/** Jangkar harga dari detektor dump lalu basis. */
export interface Jangkar {
  l_basis: number;
  h_basis: number;
  poc: number;
  p_invalid: number;
}

/** Hasil detektor dump lalu basis, PRD 5.2b. */
export interface HasilBasis {
  skor: number;
  drawdown: number;
  bar_turun: number;
  usia_basis: number;
  kemiringan_basis: number;
  rasio_volume_hidup: number;
  rasio_volume_mereda: number;
  kontraksi_atr: number;
  er_basis: number;
  tape_basis: number;
  /** Kosong berarti seluruh syarat terpenuhi. */
  gagal: string[];
  jangkar: Jangkar;
  penguat: string[];
}

export type BentukRange = "dua_sisi" | "beli_turun" | "jual_naik";

/**
 * Saran penempatan beserta alasannya.
 *
 * Alasan selalu ikut, dan selalu dalam bentuk keputusan beli dan jual. Angka
 * tanpa alasan tidak membantu user belajar, dan user yang tidak belajar akan
 * terus bergantung pada alat ini bahkan saat alatnya salah.
 */
export interface SaranPenempatan {
  bentuk: BentukRange;
  bawah: number;
  atas: number;
  lebar_persen: number;
  konsentrasi: number;
  alasan: string[];
  pembatalan: number | null;
}

export interface SignalsView {
  address: string;
  bar_terpakai: number;
  harga: number;
  /** Null berarti datanya belum cukup, BUKAN berarti tidak menarik. */
  sideways: SinyalSideways | null;
  basis: HasilBasis | null;
  saran: SaranPenempatan | null;
  konsentrasi_dibanding_lebar: number | null;
  dihitung_pada: string;
}

/** Aksi yang bisa dijalankan pada sebuah posisi. */
export type JenisAksi = "klaim_fee" | "tutup_posisi";

/**
 * Rencana aksi beserta hasil pemeriksaannya.
 *
 * `siap` datang dari engine, bukan disimpulkan panel. Panel yang
 * menyimpulkan sendiri akan menyimpang begitu ada gate baru ditambahkan.
 */
export interface ActionPlanView {
  token_id: string;
  action: JenisAksi;
  nama: string;
  gates: GateResult[];
  gas_estimate: number;
  dry_run: boolean;
  /** Benar kalau aksi ini akan menghabiskan posisinya. */
  menutup: boolean;
  siap: boolean;
  expires_at: string;
  fee0: string;
  fee1: string;
}

/** Satu posisi LP beserta keadaannya sekarang. */
export interface Posisi {
  token_id: string;
  version: PoolVersion;
  token0: string;
  token1: string;
  fee: number;
  tick_lower: number;
  tick_upper: number;
  liquidity: string;
  /** Tick pool sekarang. Null kalau poolnya tidak terbaca. */
  tick_sekarang: number | null;
  in_range: boolean;
  /** Fee yang belum diklaim, dalam satuan terkecil. */
  fee0: string;
  fee1: string;
  hooks: string | null;
  sqrt_price_x96: string | null;
  /** Jumlah token yang benar benar ada di dalam posisi pada harga sekarang. */
  amount0: string;
  amount1: string;
  /** Null berarti nilainya tidak bisa dinyatakan dalam satuan yang berarti. */
  pnl: HasilPnl | null;
  /** Benar kalau posisinya sudah tidak punya likuiditas sama sekali. */
  kosong: boolean;
}

/**
 * Satu pita distribusi likuiditas.
 *
 * `harga_bawah` selalu lebih kecil daripada `harga_atas`, apa pun sisi mana
 * memecoinnya berada. Engine yang mengurutkannya, karena pada pool yang
 * memecoinnya token1 tick yang lebih besar justru berarti harga yang lebih
 * rendah, dan panel tidak boleh mengulang perhitungan itu sendiri.
 */
export interface Pita {
  tick_bawah: number;
  tick_atas: number;
  harga_bawah: number;
  harga_atas: number;
  liquidity: string;
  /** Modal yang tertanam di pita ini, dalam token kutipan. Tinggi batang. */
  nilai_quote: number;
  aktif: boolean;
}

export interface Distribusi {
  pool_id: string;
  tick_sekarang: number;
  harga_sekarang: number;
  tick_spacing: number;
  pita: Pita[];
  quote_adalah_token1: boolean;
  error: string | null;
}

/** Label aksi yang tegas untuk satu baris hasil pemindaian, PRD 3.4. */
export type LabelScan =
  | "lp_sideways"
  | "lp_beli_turun"
  | "lp_jual_naik"
  | "pantau"
  | "hindari";

/**
 * Satu baris daftar kandidat tab Signals.
 *
 * Setiap angka uang di sini dinyatakan dalam token kutipan pool itu, bukan
 * dalam dolar. Baris yang kutipannya tidak dikenal tidak punya angka uang sama
 * sekali, dan itu bukan kekurangan yang perlu ditambal panel dengan tebakan.
 */
export interface BarisScan {
  pool_id: string;
  token: string;
  symbol: string;
  quote: string;
  quote_symbol: string;
  /**
   * Benar kalau ALAMAT kutipannya ada di registry.
   *
   * Simbol ditentukan penerbit token, jadi siapa pun bisa menerbitkan token
   * bersimbol USDG, dan pemindaian memang menemukannya. Panel harus
   * membedakan keduanya secara kasat mata.
   */
  quote_dikenal: boolean;
  fee: number;
  dynamic_fee: boolean;
  hooks: string | null;
  tick: number;
  tick_spacing: number;
  liquidity: string;
  harga: number | null;
  mcap: number | null;
  volume: number | null;
  swap: number;
  /** Kutipan yang dibutuhkan untuk menutup rentang yang harga lewati. */
  kedalaman: number | null;
  umur_jam: number;
  /** Hasil fee sepanjang jendela pengukuran atas modal acuan, persen. */
  hasil_jendela: number | null;
  /** Hasil di atas yang direntangkan menjadi setahun, persen. */
  fee_apr: number | null;
  modal_acuan: number;
  efisiensi: number | null;
  lebar_persen: number | null;
  perpindahan_persen: number | null;
  risiko: string[];
  label: LabelScan;
  alasan: string;
}

/** Satu kriteria adopsi hook. Tiga keadaan, bukan dua. */
export type Kriteria = "lolos" | "gagal" | "belum_terukur";

/**
 * Penilaian sebuah hook V4 terhadap kriteria adopsi, PRD 5.4.
 *
 * `belum_terukur` berbeda dari `gagal`. Yang pertama berarti kita belum tahu,
 * yang kedua berarti kita sudah tahu dan jawabannya tidak. Panel harus
 * menampilkan keduanya secara berbeda, karena user yang membaca "gagal" pada
 * sesuatu yang sebenarnya belum diukur akan menyimpulkan hal yang salah.
 */
export interface PenilaianHook {
  hook: string;
  jumlah_pool: number | null;
  umur_hari: number | null;
  bisa_diupgrade: boolean | null;
  kriteria_pool: Kriteria;
  kriteria_umur: Kriteria;
  kriteria_upgrade: Kriteria;
  kriteria_tvl: Kriteria;
  kriteria_lp: Kriteria;
  lolos_otomatis: boolean;
  /** Benar kalau hook ini tidak boleh dipakai bahkan lewat persetujuan sadar. */
  terlarang: boolean;
  catatan: string[];
}

export interface ScanView {
  rows: BarisScan[];
  hooks: PenilaianHook[];
  adopsi_lengkap: boolean;
  jendela_pindai_menit: number;
  jendela_volume_menit: number;
  volume_jendela_pendek: boolean;
  modal_acuan: number;
  total_swap: number;
  dipindai_pada: string;
  error: string | null;
}

export interface PositionsView {
  positions: Posisi[];
  owner: string;
  keluar_range: number;
  ringkasan: RingkasanPnl;
  error: string | null;
  dimuat_pada: string;
}

/**
 * Satu kejadian yang layak dimunculkan sebagai toast.
 *
 * Berbeda dari `alert`, yang dipakai untuk kegagalan seketika pada aksi yang
 * sedang dijalankan user. Kejadian melewati pendinginan dan masuk riwayat.
 */
export interface Kejadian {
  level: AlertLevel;
  code: string;
  message: string;
  /** Biasanya token id posisi. Null untuk kejadian yang tidak terikat posisi. */
  subject: string | null;
  at: string;
  /** Benar kalau tertahan pendinginan sehingga tidak dimunculkan. */
  ditahan: boolean;
}

/**
 * Berapa lama toast bertahan, dalam milidetik. Nol berarti menetap sampai
 * diklik. Angkanya mengikuti tabel di PRD 3.7, dan disalin di sini karena
 * content script tidak bisa memanggil engine.
 */
export function durasiToastMs(level: AlertLevel): number {
  switch (level) {
    case "info":
      return 6_000;
    case "warning":
      return 12_000;
    default:
      return 0;
  }
}

/** Berapa ketukan nada yang dibunyikan untuk sebuah tingkat. */
export function ketukanNada(level: AlertLevel): number {
  return level === "info" ? 1 : level === "warning" ? 2 : 3;
}

export type ServerMessage =
  | { type: "hello"; engine_version: string; protocol_version: number }
  | ({ type: "engine_status" } & Partial<EngineStatus>)
  | { type: "pong" }
  | ({ type: "token_state" } & Partial<TokenState>)
  | ({ type: "range_view" } & Partial<RangeView>)
  | ({ type: "open_plan" } & Partial<OpenPlanView>)
  | ({ type: "send_result" } & Partial<SendResultView>)
  | ({ type: "positions" } & Partial<PositionsView>)
  | ({ type: "scan_results" } & Partial<ScanView>)
  | ({ type: "depth" } & Partial<Distribusi>)
  | ({ type: "action_plan" } & Partial<ActionPlanView>)
  | ({ type: "signals" } & Partial<SignalsView>)
  | ({ type: "notify" } & Partial<Kejadian>)
  | { type: "notify_history"; items: Kejadian[] }
  | { type: "alert"; level: AlertLevel; code: string; message: string };

export type ClientMessage =
  | { type: "get_status" }
  | { type: "ping" }
  | { type: "subscribe_token"; address: string }
  | { type: "chart_price"; address: string; mcap_usd: number }
  | { type: "chart_bars"; address: string; bars: Bar[] }
  | { type: "select_pool"; pool_id: string }
  | { type: "select_quote"; quote: string }
  | { type: "set_range"; lower_mcap: number; upper_mcap: number }
  | { type: "quote_range"; size_usd: number }
  | { type: "approve_token"; token: string }
  | { type: "get_positions" }
  | { type: "get_notifications" }
  | { type: "scan" }
  | { type: "get_depth" }
  | { type: "set_rpc"; http: string[] }
  | { type: "approve_hook"; hook: string }
  | { type: "forget_hook"; hook: string }
  | { type: "plan_action"; token_id: string; action: JenisAksi }
  | { type: "execute_action" }
  | { type: "swap_zap" }
  | { type: "open_position" };

export function asRangeView(msg: unknown): RangeView | null {
  if (!msg || typeof msg !== "object") return null;
  const m = msg as Record<string, unknown>;
  if (m.type !== "range_view") return null;
  const { type: _type, ...rest } = m;
  return rest as unknown as RangeView;
}

export function asSignals(msg: unknown): SignalsView | null {
  return untag<SignalsView>(msg, "signals");
}

export function asActionPlan(msg: unknown): ActionPlanView | null {
  return untag<ActionPlanView>(msg, "action_plan");
}

export function asPositions(msg: unknown): PositionsView | null {
  return untag<PositionsView>(msg, "positions");
}

export function asScan(msg: unknown): ScanView | null {
  return untag<ScanView>(msg, "scan_results");
}

export function asDepth(msg: unknown): Distribusi | null {
  return untag<Distribusi>(msg, "depth");
}

export function asNotify(msg: unknown): Kejadian | null {
  return untag<Kejadian>(msg, "notify");
}

export function asOpenPlan(msg: unknown): OpenPlanView | null {
  return untag<OpenPlanView>(msg, "open_plan");
}

export function asSendResult(msg: unknown): SendResultView | null {
  return untag<SendResultView>(msg, "send_result");
}

/**
 * Pesan di kabel adalah muatan yang diratakan bersama field `type`, karena
 * serde memakai tag internal. Pembantu ini memisahkan keduanya supaya sisi UI
 * tidak perlu tahu detail itu.
 */
function untag<T>(msg: unknown, type: string): T | null {
  if (!msg || typeof msg !== "object") return null;
  const m = msg as Record<string, unknown>;
  if (m.type !== type) return null;
  const { type: _type, ...rest } = m;
  return rest as unknown as T;
}

export function asTokenState(msg: unknown): TokenState | null {
  if (!msg || typeof msg !== "object") return null;
  const m = msg as Record<string, unknown>;
  if (m.type !== "token_state") return null;
  const { type: _type, ...rest } = m;
  return rest as unknown as TokenState;
}

/** Alamat EVM yang bentuknya benar. Checksum tidak diperiksa di sini. */
export function isEvmAddress(value: string): boolean {
  return /^0x[a-fA-F0-9]{40}$/.test(value.trim());
}

export function gmgnTokenUrl(address: string): string {
  return `https://gmgn.ai/robinhood/token/${address.trim().toLowerCase()}`;
}

/**
 * Bentuk `engine_status` di kabel adalah EngineStatus yang diratakan bersama
 * field `type`, karena serde memakai tag internal. Pembantu ini memisahkan
 * keduanya supaya sisi UI tidak perlu tahu detail itu.
 */
export function asEngineStatus(msg: unknown): EngineStatus | null {
  if (!msg || typeof msg !== "object") return null;
  const m = msg as Record<string, unknown>;
  if (m.type !== "engine_status") return null;
  const { type: _type, ...rest } = m;
  return rest as unknown as EngineStatus;
}

export interface EnginePing {
  name: string;
  engine_version: string;
  protocol_version: number;
  requires_pairing: boolean;
}
