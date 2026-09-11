//! Kontrak pesan antara panel dan engine.
//!
//! INVARIAN
//! - Setiap tipe pesan di sini WAJIB punya pasangan di PRD bagian 7.3.
//!   `cargo xtask docs-check` menegakkan hal itu, dan build gagal kalau ada
//!   yang menyimpang. Lihat aturan sinkronisasi di PRD 13.1.
//! - Tidak ada pesan yang boleh membawa kunci privat atau passphrase keluar
//!   dari engine. Passphrase hanya boleh mengalir masuk.
//! - Tag serde memakai `snake_case` dan field `type`, sama persis dengan yang
//!   diharapkan sisi TypeScript di `extension/src/shared/protocol.ts`.

use serde::{Deserialize, Serialize};

use crate::chain::{ChainHealth, RegistryCheck};
use crate::wallet::WalletStatus;

/// Pesan dari panel ke engine lewat WebSocket.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Minta kiriman status terbaru sekarang juga.
    GetStatus,
    /// Denyut untuk menjaga koneksi dan memundurkan kunci otomatis.
    Ping,
    /// Minta engine mencari pool untuk sebuah token dan mengirim `token_state`.
    ///
    /// Pencariannya menyentuh jaringan dan bisa makan beberapa detik, jadi
    /// dijalankan di tugas terpisah dan jawabannya datang belakangan.
    SubscribeToken { address: String },

    /// Market cap yang sedang ditampilkan chart gmgn.
    ///
    /// Inilah yang mengkalibrasi kurs ETH ke USD. Datang tiap detik dari
    /// content script selama halaman token terbuka.
    ChartPrice { address: String, mcap_usd: f64 },

    /// User memilih pool lain dari daftar.
    SelectPool { pool_id: String },

    /// User berpindah ke pasangan token lawan yang lain.
    ///
    /// Satu token bisa punya pasar terhadap beberapa token sekaligus, dan
    /// yang dipilihkan otomatis belum tentu yang diinginkan. Berpindah
    /// kelompok mengganti daftar pool sekaligus desimal kutipan, jadi ia
    /// tidak bisa diselesaikan di sisi panel.
    SelectQuote { quote: String },

    /// User menggeser garis. Batasnya dalam market cap USD, satuan chart.
    SetRange { lower_mcap: f64, upper_mcap: f64 },

    /// Pasang range selebar yang diizinkan protokol.
    ///
    /// Pesan tersendiri, bukan `set_range` dengan angka raksasa. Market cap
    /// dikali sepuluh pangkat empat puluh menghasilkan harga yang tidak lagi
    /// bisa diubah menjadi tick, dan engine menolaknya tanpa ada yang berubah
    /// di layar. Batas sebenarnya hanya diketahui engine, karena ia bergantung
    /// pada tick spacing pool.
    SetFullRange,
    /// Input setoran sedang disunting, rencana yang tampil tidak lagi berlaku.
    InvalidatePlan,

    /// Hitung nominal pasangan dari snapshot pool yang sudah ada.
    ///
    /// Jalur ini tidak membaca RPC, saldo, allowance, simulasi, atau gas. Ia
    /// hanya membuat kotak nominal terasa langsung saat user mengetik.
    PreviewDeposit {
        deposit: super::deposit::Deposit,
        request_id: String,
    },

    /// Susun rencana membuka posisi sebesar sekian dolar pada range sekarang.
    ///
    /// Tidak mengirim apa pun. Seluruh gate dijalankan dan hasilnya
    /// dikirim balik sebagai `open_plan`, supaya user melihat persis apa
    /// yang akan terjadi sebelum menyetujuinya.
    QuoteRange {
        deposit: super::deposit::Deposit,
        request_id: String,
    },

    /// Kirim `approve` ERC20 untuk satu token, sebesar yang dibutuhkan
    /// rencana terakhir. Alamat spender datang dari chain.toml, tidak
    /// pernah dari panel.
    ApproveToken { plan_id: String, step_id: String },

    /// Jalankan seluruh rangkaian pembukaan dari satu persetujuan user.
    /// Engine mengerjakan approval, zap bila dibutuhkan, penyusunan ulang,
    /// lalu mint secara berurutan. Detail langkah tidak perlu keluar ke UI.
    RunOpenFlow { plan_id: String },

    /// Deret candle dari chart gmgn, dipakai seluruh detektor sinyal.
    ///
    /// Datang lebih jarang daripada `chart_price` karena isinya ratusan bar
    /// dan jawaban detektor tidak berubah dalam hitungan detik.
    ChartBars {
        address: String,
        bars: Vec<crate::signals::bar::Bar>,
    },

    /// Susun rencana sebuah aksi pada posisi. Tidak mengirim apa pun.
    PlanAction {
        position_id: String,
        request_id: String,
        action: crate::exec::aksi::JenisAksi,
        #[serde(default)]
        input: crate::exec::aksi::ActionInput,
    },

    /// Setujui aksi terakhir dan kirim transaksinya.
    ExecuteAction { plan_id: String },

    /// Jalankan approval yang dibutuhkan lalu aksi posisi terakhir.
    RunActionFlow { plan_id: String },

    /// Minta riwayat notifikasi terakhir.
    GetNotifications,

    /// Setujui sebuah hook V4 secara sadar, PRD 5.4.
    ///
    /// Dipakai untuk hook yang belum lolos kriteria adopsi. Engine memeriksa
    /// ulang penilaiannya sendiri sebelum mencatat: hook yang masih bisa
    /// diupgrade ditolak di sini juga, bukan hanya di panel.
    ApproveHook { hook: String },

    /// Cabut persetujuan sebuah hook.
    ForgetHook { hook: String },

    /// Ganti daftar endpoint RPC di config.toml.
    ///
    /// Hanya menulis berkas. Endpoint yang sedang dipakai tidak ditukar saat
    /// engine berjalan, karena permintaan yang sudah berangkat ke satu RPC
    /// tidak boleh ditunggu jawabannya dari RPC yang lain.
    SetRpc { http: Vec<String> },

    /// Minta distribusi likuiditas pool yang sedang aktif.
    ///
    /// Terpisah dari `range_view` dengan sengaja. Distribusi butuh dua
    /// perjalanan ke rantai, sekitar setengah detik, sedangkan `range_view`
    /// dikirim ulang setiap kali user menggeser garis. Menggabungkannya akan
    /// membuat garis terasa berat persis saat ia sedang digeser.
    ///
    /// Bentuk pool tidak berubah secepat harganya, jadi satu pembacaan cukup
    /// untuk menemani banyak geseran.
    GetDepth,

    /// Pindai pool paling ramai di rantai dan nilai kelayakannya.
    ///
    /// Membaca setiap swap pada jendela terakhir, jadi ia mahal dan tidak
    /// dijalankan sendiri secara berkala. User yang memintanya, dan jawabannya
    /// datang belakangan sebagai `scan_results`.
    Scan,

    /// Minta daftar posisi LP milik wallet, dimuat ulang dari rantai.
    ///
    /// Menyentuh jaringan dan bisa makan beberapa detik, jadi jawabannya
    /// datang belakangan sebagai `positions`.
    GetPositions,

    /// Minta jurnal transaksi persisten. Hanya membaca penyimpanan lokal.
    GetTransactions,

    /// Jalankan swap zap dari rencana terakhir.
    ///
    /// Transaksi terpisah dari membuka posisi, dan itu bukan pilihan:
    /// swap berjalan lewat UniversalRouter sedangkan posisi dibuat lewat
    /// PositionManager, dan tidak ada kontrak yang bisa keduanya sekaligus.
    SwapZap { plan_id: String },

    /// Setujui rencana terakhir dan kirim transaksinya.
    ///
    /// Tidak membawa rencana, hanya persetujuan. Rencana yang datang
    /// kembali dari panel bisa saja sudah diubah, dan yang diubah itu
    /// justru calldata.
    OpenPosition { plan_id: String },
}

/// Pesan dari engine ke panel.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Dikirim sekali saat koneksi terbentuk.
    Hello {
        engine_version: String,
        protocol_version: u32,
    },
    /// Status gabungan engine, dikirim berkala dan saat diminta.
    EngineStatus(Box<EngineStatus>),
    /// Balasan denyut.
    Pong,
    /// Hasil pencarian pool untuk satu token.
    TokenState(Box<TokenState>),
    /// Range dalam tiga satuan sekaligus, hasil kalibrasi dan snap tick.
    RangeView(Box<crate::range::RangeView>),
    /// Rencana membuka posisi beserta hasil seluruh gate.
    OpenPlan(Box<crate::api::execute::OpenPlanView>),

    /// Preview nominal kedua sisi yang tidak mengandung keputusan eksekusi.
    DepositPreview {
        request_id: String,
        deposit: crate::api::deposit::Deposit,
        setor: Option<crate::api::execute::SetoranView>,
        error: Option<String>,
    },

    /// Hasil satu pengiriman transaksi, `approve` maupun `open`.
    SendResult(Box<crate::api::execute::SendResultView>),

    /// Satu kejadian yang layak dimunculkan sebagai toast di halaman gmgn.
    ///
    /// Berbeda dari `alert`, yang dipakai untuk kegagalan seketika pada aksi
    /// yang sedang dijalankan user. `notify` melewati pendinginan, masuk
    /// riwayat, dan membawa petunjuk durasi serta suara.
    Notify(Box<crate::notify::Kejadian>),

    /// Riwayat notifikasi, terbaru lebih dulu.
    NotifyHistory { items: Vec<crate::notify::Kejadian> },

    /// Hasil seluruh detektor sinyal beserta saran penempatannya.
    Signals(Box<SignalsView>),

    /// Rencana aksi pada posisi beserta hasil pemeriksaannya.
    ActionPlan(Box<crate::api::execute::ActionPlanView>),

    /// Distribusi likuiditas pool yang sedang aktif.
    ///
    /// Menjawab pertanyaan yang menentukan hasil LP: berapa banyak modal orang
    /// lain yang sudah ada di pita tempat harga bergerak. Porsi fee kita adalah
    /// modal kita dibagi angka itu.
    Depth(Box<crate::pools::distribusi::Distribusi>),
    /// Hasil pemindaian pool ramai untuk tab Signals.
    ///
    /// Jendela pengukuran ikut di dalamnya, bukan disimpulkan panel. Angka
    /// hasil dan proyeksi tidak berarti tanpa jendelanya.
    ScanResults(Box<ScanView>),
    /// Daftar posisi LP milik wallet beserta keadaannya.
    Positions(Box<PositionsView>),
    /// Jurnal approve, zap, open, klaim, dan penutupan milik wallet.
    Transactions(Box<TransactionsView>),

    /// Kejadian yang layak diberitahukan ke user. Lihat PRD 3.7.
    Alert {
        level: AlertLevel,
        code: String,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertLevel {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Serialize)]
pub struct EngineStatus {
    pub engine_version: String,
    pub uptime_secs: u64,
    pub chain: ChainHealth,
    pub wallet: WalletStatus,
    pub registry: RegistrySummary,
    /// Benar hanya kalau semua prasyarat eksekusi terpenuhi.
    pub can_execute: bool,
    /// Alasan `can_execute` bernilai salah, supaya panel tidak perlu menebak.
    pub blocked_reasons: Vec<String>,
    /// Endpoint HTTP yang sedang dipakai, kunci API sudah disunting.
    ///
    /// Penyuntingan terjadi di `chain::endpoint` saat endpointnya dibuat, bukan
    /// saat ditampilkan, supaya tidak ada jalur mana pun yang bisa lolos
    /// membawa kunci utuh.
    pub rpc_http: Vec<String>,
}

/// Keadaan satu token beserta pool yang bisa dipakai.
///
/// `pools` sudah disaring dan diurutkan oleh `pools::usable_pools`, jadi panel
/// tidak perlu tahu bahwa pool kosong itu ada. Kalau pencariannya gagal,
/// `error` terisi dan `pools` kosong, bukan pesan gagal yang menggantikan
/// seluruh muatan.
#[derive(Debug, Clone, Serialize)]
pub struct TokenState {
    pub address: String,
    pub meta: Option<crate::pools::TokenMeta>,
    /// Pool dikelompokkan menurut token lawan, karena satu token bisa
    /// berpasangan dengan beberapa token sekaligus.
    pub quotes: Vec<crate::pools::QuoteGroup>,
    /// Pool dari kelompok terpilih. Diratakan supaya panel yang sudah ada
    /// tidak perlu berubah untuk kasus satu pasangan.
    pub pools: Vec<crate::pools::PoolInfo>,
    pub selected_quote: Option<String>,
    /// Pool peringkat teratas, dipilih otomatis supaya panel punya titik awal.
    pub selected_pool_id: Option<String>,
    /// Saldo wallet untuk kedua sisi pasangan terpilih.
    ///
    /// `None` berarti wallet belum dibuka, bukan berarti saldonya nol. Nol
    /// berarti user memang tidak punya token itu, dan keduanya menuntun ke
    /// tindakan yang berbeda: yang satu membuka wallet, yang lain membeli dulu.
    pub saldo: Option<SaldoPasangan>,
    pub error: Option<String>,
}

/// Saldo wallet untuk kedua sisi sebuah pasangan, sudah berdesimal.
#[derive(Debug, Clone, Serialize)]
pub struct SaldoPasangan {
    pub memecoin: f64,
    pub quote: f64,
    /// Saldo token gas, dipakai memeriksa apakah biaya transaksi tertutup.
    pub gas: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegistrySummary {
    pub total: usize,
    pub verified: usize,
    pub unpinned: usize,
    pub no_code: usize,
    pub checks: Vec<RegistryCheck>,
}

/// Hasil pemindaian pool ramai, PRD 3.4.
///
/// JENDELA IKUT DI SINI, BUKAN DISIMPULKAN PANEL
/// Angka hasil dan proyeksi APR keduanya bergantung pada lamanya jendela
/// pengukuran, dan jendela itu bisa menyempit sendiri kalau RPC menolak.
/// Panel yang menebak jendelanya akan menampilkan angka yang salah tanpa satu
/// pun error muncul, yang tepat pernah terjadi saat volume dan jendelanya
/// datang dari dua sumber berbeda.
#[derive(Debug, Clone, Serialize)]
pub struct ScanView {
    pub rows: Vec<crate::pools::kandidat::BarisScan>,
    /// Penilaian setiap hook yang muncul di hasil, PRD 5.4.
    ///
    /// Ikut dikirim supaya baris yang ditolak karena hooknya bisa menjelaskan
    /// diri sendiri, dan supaya user bisa menyetujuinya secara sadar tanpa
    /// harus meminta pembacaan kedua.
    pub hooks: Vec<crate::risk::hook::PenilaianHook>,
    /// Benar kalau sejarah adopsi hook sudah dipindai sampai blok pemasangan.
    ///
    /// Selama salah, kriteria jumlah pool dan umur hanya batas bawah, dan panel
    /// harus mengatakannya. Sejarah penuh rantai ini butuh belasan pemindaian,
    /// dan tiap pemindaian melanjutkan yang sebelumnya.
    pub adopsi_lengkap: bool,
    /// Jendela pemindaian swap, dalam menit.
    pub jendela_pindai_menit: f64,
    /// Jendela pengukuran volume, dalam menit.
    pub jendela_volume_menit: f64,
    /// Benar kalau volume terpaksa diukur pada jendela pemindaian yang pendek.
    pub volume_jendela_pendek: bool,
    /// Modal acuan yang dipakai menghitung kolom hasil dan APR.
    pub modal_acuan: f64,
    /// Total swap yang terbaca sebelum dipangkas ke pool teratas.
    pub total_swap: usize,
    pub dipindai_pada: String,
    /// Terisi kalau pemindaiannya gagal seluruhnya.
    ///
    /// Kegagalan dilaporkan sebagai kegagalan, bukan sebagai daftar kosong.
    /// Daftar kosong berarti rantainya sedang sepi, dan itu kesimpulan yang
    /// sangat berbeda dari tidak bisa membaca.
    pub error: Option<String>,
}

/// Daftar posisi beserta ringkasannya.
#[derive(Debug, Clone, Serialize)]
pub struct PositionsView {
    pub positions: Vec<crate::positions::Posisi>,
    /// Posisi yang NFT-nya sudah ditutup, direkonstruksi dari jurnal receipt.
    pub tertutup: Vec<crate::pnl::PosisiTertutup>,
    /// Alamat wallet yang posisinya ditampilkan. Panel menampilkannya
    /// supaya tidak ada keraguan posisi siapa yang sedang dilihat.
    pub owner: String,
    /// Berapa yang sedang di luar range. Angka ini yang menentukan apakah
    /// tab Positions perlu ditandai.
    pub keluar_range: usize,
    /// Ringkasan hasil seluruh posisi, dalam satuan token kutipan.
    ///
    /// Dihitung engine, bukan panel, supaya tidak ada dua tempat yang
    /// menjumlahkan hal yang sama lalu menyimpang.
    pub ringkasan: crate::pnl::Ringkasan,
    /// Versi kalibrasi per posisi, status penantang, dan ringkasan PnL yang
    /// sudah dikelompokkan engine. Panel memakai ini untuk filter tanpa
    /// menghitung ulang hasil uang sendiri.
    pub learning: crate::learning::LearningView,
    /// Kosong kalau pemuatannya berhasil.
    pub error: Option<String>,
    pub dimuat_pada: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TransactionsView {
    pub owner: String,
    pub items: Vec<crate::exec::journal::View>,
    pub error: Option<String>,
}

/// Hasil detektor sinyal untuk satu token.
#[derive(Debug, Clone, Serialize)]
pub struct SignalsView {
    pub address: String,
    /// Berapa bar yang benar benar dipakai setelah dibersihkan.
    pub bar_terpakai: usize,
    pub harga: f64,
    /// Kosong berarti datanya belum cukup untuk jendela yang diminta, BUKAN
    /// berarti kondisinya tidak menarik. Dua hal itu sangat berbeda.
    pub sideways: Option<crate::signals::sideways::Sinyal>,
    pub basis: Option<crate::signals::dump_basis::Hasil>,
    /// Saran penempatan, kalau ada detektor yang lolos.
    pub saran: Option<crate::signals::penempatan::Saran>,
    /// Berapa kali lipat konsentrasinya dibanding range lima kali lebih lebar.
    pub konsentrasi_dibanding_lebar: Option<f64>,
    pub dihitung_pada: String,
}

pub const PROTOCOL_VERSION: u32 = 6;

/// Tentukan apakah engine boleh mengeksekusi transaksi, beserta alasannya.
///
/// Aturan ini terpusat di satu fungsi supaya panel, WebSocket, dan jalur HTTP
/// tidak mungkin memberi jawaban yang berbeda.
pub fn evaluate_execution_readiness(
    chain: &ChainHealth,
    wallet: &WalletStatus,
) -> (bool, Vec<String>) {
    let mut reasons = Vec::new();

    if !chain.healthy {
        reasons.push(
            chain
                .error
                .clone()
                .unwrap_or_else(|| "RPC tidak sehat".into()),
        );
    }
    if !wallet.exists {
        reasons.push("wallet belum dibuat".into());
    } else if wallet.locked {
        reasons.push("wallet masih terkunci".into());
    }

    (reasons.is_empty(), reasons)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain_sehat() -> ChainHealth {
        ChainHealth {
            healthy: true,
            endpoint: "https://contoh".into(),
            chain_id: Some(4663),
            block_number: Some(100),
            block_lag_secs: Some(1),
            max_block_lag_secs: 5,
            error: None,
            checked_at: "2026-09-05T00:00:00Z".into(),
        }
    }

    fn wallet_terbuka() -> WalletStatus {
        WalletStatus {
            exists: true,
            locked: false,
            address: Some("0xabc".into()),
            auto_lock_minutes: 30,
            credential_manager: false,
            always_unlocked: false,
            seconds_until_auto_lock: Some(1800),
        }
    }

    #[test]
    fn siap_eksekusi_saat_semua_baik() {
        let (ok, reasons) = evaluate_execution_readiness(&chain_sehat(), &wallet_terbuka());
        assert!(ok);
        assert!(reasons.is_empty());
    }

    #[test]
    fn wallet_terkunci_memblokir_eksekusi() {
        let mut w = wallet_terbuka();
        w.locked = true;
        let (ok, reasons) = evaluate_execution_readiness(&chain_sehat(), &w);
        assert!(!ok);
        assert!(reasons.iter().any(|r| r.contains("terkunci")));
    }

    #[test]
    fn rpc_sakit_memblokir_eksekusi() {
        let mut c = chain_sehat();
        c.healthy = false;
        c.error = Some("lag blok 12 detik".into());
        let (ok, reasons) = evaluate_execution_readiness(&c, &wallet_terbuka());
        assert!(!ok);
        assert!(reasons.iter().any(|r| r.contains("lag blok")));
    }

    #[test]
    fn beberapa_masalah_dilaporkan_sekaligus() {
        let mut c = chain_sehat();
        c.healthy = false;
        c.error = None;
        let mut w = wallet_terbuka();
        w.exists = false;
        let (ok, reasons) = evaluate_execution_readiness(&c, &w);
        assert!(!ok);
        assert_eq!(
            reasons.len(),
            2,
            "kedua masalah harus dilaporkan, bukan yang pertama saja"
        );
    }

    #[test]
    fn pesan_klien_terbaca_dari_json() {
        let m: ClientMessage = serde_json::from_str(r#"{"type":"get_status"}"#).unwrap();
        assert!(matches!(m, ClientMessage::GetStatus));
        let m: ClientMessage = serde_json::from_str(r#"{"type":"ping"}"#).unwrap();
        assert!(matches!(m, ClientMessage::Ping));

        let old: ClientMessage = serde_json::from_str(
            r#"{"type":"plan_action","position_id":"p","request_id":"r","action":"klaim_fee"}"#,
        )
        .unwrap();
        assert!(matches!(
            old,
            ClientMessage::PlanAction { input, .. }
                if input == crate::exec::aksi::ActionInput::default()
        ));

        let add: ClientMessage = serde_json::from_str(
            r#"{"type":"plan_action","position_id":"p","request_id":"r","action":"tambah_likuiditas","input":{"amount0":"1.25","amount1":"2"}}"#,
        )
        .unwrap();
        assert!(matches!(
            add,
            ClientMessage::PlanAction { action: crate::exec::aksi::JenisAksi::TambahLikuiditas, input, .. }
                if input.amount0.as_deref() == Some("1.25")
                    && input.amount1.as_deref() == Some("2")
        ));

        let zap: ClientMessage = serde_json::from_str(
            r#"{"type":"quote_range","request_id":"quote-zap","deposit":{"side":"quote","amount":"25.5","zap":true}}"#,
        )
        .unwrap();
        assert!(matches!(
            zap,
            ClientMessage::QuoteRange { request_id, deposit }
                if request_id == "quote-zap"
                    && deposit.side == super::super::deposit::Side::Quote
                    && deposit.amount == "25.5"
                    && deposit.zap
        ));

        let preview: ClientMessage = serde_json::from_str(
            r#"{"type":"preview_deposit","request_id":"preview-1","deposit":{"side":"meme","amount":"12.5","zap":false}}"#,
        )
        .unwrap();
        assert!(matches!(
            preview,
            ClientMessage::PreviewDeposit { request_id, deposit }
                if request_id == "preview-1"
                    && deposit.side == super::super::deposit::Side::Meme
                    && deposit.amount == "12.5"
                    && !deposit.zap
        ));

        let execute_zap: ClientMessage =
            serde_json::from_str(r#"{"type":"swap_zap","plan_id":"plan-123"}"#).unwrap();
        assert!(matches!(
            execute_zap,
            ClientMessage::SwapZap { plan_id } if plan_id == "plan-123"
        ));

        let open_flow: ClientMessage =
            serde_json::from_str(r#"{"type":"run_open_flow","plan_id":"open-123"}"#).unwrap();
        assert!(matches!(
            open_flow,
            ClientMessage::RunOpenFlow { plan_id } if plan_id == "open-123"
        ));

        let action_flow: ClientMessage =
            serde_json::from_str(r#"{"type":"run_action_flow","plan_id":"action-123"}"#).unwrap();
        assert!(matches!(
            action_flow,
            ClientMessage::RunActionFlow { plan_id } if plan_id == "action-123"
        ));
    }

    #[test]
    fn pesan_server_memakai_tag_type() {
        let json = serde_json::to_string(&ServerMessage::Pong).unwrap();
        assert_eq!(json, r#"{"type":"pong"}"#);
    }
}
