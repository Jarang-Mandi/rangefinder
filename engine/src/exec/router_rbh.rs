//! Encoder swap untuk UniversalRouter Robinhood Chain.
//!
//! KENAPA DITULIS TANGAN, BUKAN MEMAKAI SDK
//! UniversalRouter di rantai ini adalah fork. Struct
//! `ExactInputSingleParams`-nya membawa satu field tambahan, `minHopPriceX36`,
//! yang disisipkan tepat sebelum `hookData`. Calldata dari SDK Uniswap standar
//! akan menaruh `hookData` di posisi yang salah, dan kontraknya akan membaca
//! sampah.
//!
//! CARA MEMBUKTIKANNYA
//! Field itu tidak dilihat dari kode sumber, karena kita tidak punya kodenya.
//! Ia terbaca dari aritmetika offset pada transaksi swap sungguhan
//! `0xf15c5f65d6f0372698805158351bf8c13de3578692a721a06094a365449fcd86`:
//! offset `hookData` di sana bernilai 0x140, yaitu sepuluh kata dari awal
//! struct, sedangkan struct Uniswap standar hanya sembilan kata. Selisih satu
//! kata itulah fieldnya, dan nilainya nol pada transaksi itu.
//!
//! Uji di bawah membangun ulang seluruh calldata transaksi tersebut, 1124 byte,
//! dan membandingkannya byte per byte.
//!
//! INVARIAN
//! - Setiap swap WAJIB diikuti penyelesaian kedua sisi. Urutan aksinya
//!   `SWAP_EXACT_IN_SINGLE`, `SETTLE_ALL`, `TAKE_ALL`, persis seperti pada
//!   transaksi sungguhan. Tanpa itu router menahan dana.
//! - `amountOutMinimum` adalah satu satunya pengaman slippage swap. Nol berarti
//!   bersedia menerima berapa pun, dan pada memecoin itu berarti bersedia
//!   kehilangan hampir seluruhnya.
//! - `minHopPriceX36` diisi nol kecuali ada alasan yang dipahami. Menebak nilai
//!   lain untuk field yang perilakunya belum kita ketahui lebih berbahaya
//!   daripada tidak memakainya.

use alloy::primitives::{Address, Bytes, U256};
use alloy::sol;
use alloy::sol_types::{SolCall, SolValue};

sol! {
    #[sol(rpc)]
    interface IUniversalRouter {
        function execute(bytes commands, bytes[] inputs, uint256 deadline) external payable;
    }
}

/// Perintah UniversalRouter yang kita pakai.
///
/// Angkanya bagian dari kontrak, bukan pilihan kita.
pub mod perintah {
    /// Menjalankan rangkaian aksi V4. Terbaca sebagai byte tunggal `0x10` pada
    /// transaksi swap sungguhan.
    pub const V4_SWAP: u8 = 0x10;
}

/// Aksi V4 di dalam `V4_SWAP`.
pub mod aksi {
    pub const SWAP_EXACT_IN_SINGLE: u8 = 0x06;
    pub const SETTLE_ALL: u8 = 0x0c;
    pub const TAKE_ALL: u8 = 0x0f;
}

/// Kunci pool untuk jalur swap.
///
/// Isinya sama dengan PoolKey di jalur posisi, tapi ditulis ulang di sini supaya
/// modul ini tidak bergantung pada modul posisi. Keduanya menggambarkan hal yang
/// sama, dan konversinya ada di `dari_pool_key`.
#[derive(Debug, Clone, PartialEq)]
pub struct KunciPool {
    pub currency0: Address,
    pub currency1: Address,
    pub fee: u32,
    pub tick_spacing: i32,
    pub hooks: Address,
}

impl KunciPool {
    pub fn dari_pool_key(k: &super::v4::abi::IPositionManagerV4::PoolKey) -> Self {
        Self {
            currency0: k.currency0,
            currency1: k.currency1,
            fee: k.fee.to::<u32>(),
            tick_spacing: k.tickSpacing.as_i32(),
            hooks: k.hooks,
        }
    }
}

/// Permintaan swap satu lompatan dengan jumlah masuk yang pasti.
#[derive(Debug, Clone)]
pub struct SwapExactIn {
    pub pool: KunciPool,
    /// Benar berarti menukar currency0 menjadi currency1.
    pub zero_for_one: bool,
    pub amount_in: u128,
    /// Batas bawah yang masih diterima. Ini satu satunya pengaman slippage.
    pub amount_out_minimum: u128,
    pub deadline: U256,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum RouterError {
    #[error("jumlah masuk nol, tidak ada yang ditukar")]
    JumlahNol,
    #[error("amountOutMinimum nol berarti bersedia menerima berapa pun, termasuk hampir nol")]
    TanpaPengamanSlippage,
    #[error("currency0 tidak lebih kecil dari currency1, urutan PoolKey salah dan poolId-nya akan berbeda")]
    UrutanCurrencySalah,
    #[error("deadline nol, transaksi tidak akan pernah dianggap sah")]
    DeadlineNol,
}

impl SwapExactIn {
    pub fn validate(&self) -> Result<(), RouterError> {
        if self.amount_in == 0 {
            return Err(RouterError::JumlahNol);
        }
        if self.amount_out_minimum == 0 {
            return Err(RouterError::TanpaPengamanSlippage);
        }
        if self.pool.currency0 >= self.pool.currency1 {
            return Err(RouterError::UrutanCurrencySalah);
        }
        if self.deadline.is_zero() {
            return Err(RouterError::DeadlineNol);
        }
        Ok(())
    }

    /// Currency yang kita bayarkan, dan yang kita terima.
    pub fn sisi(&self) -> (Address, Address) {
        if self.zero_for_one {
            (self.pool.currency0, self.pool.currency1)
        } else {
            (self.pool.currency1, self.pool.currency0)
        }
    }

    /// Nilai ETH yang ikut, bukan nol hanya kalau yang dibayarkan ETH asli.
    pub fn value(&self) -> U256 {
        let (bayar, _) = self.sisi();
        if bayar.is_zero() {
            U256::from(self.amount_in)
        } else {
            U256::ZERO
        }
    }

    /// Parameter aksi `SWAP_EXACT_IN_SINGLE`, dengan field fork di tempatnya.
    fn params_swap(&self) -> Bytes {
        // Struct dikodekan sebagai satu tuple dinamis, karena ia memuat
        // `hookData`. Urutan fieldnya:
        //   poolKey (5 kata), zeroForOne, amountIn, amountOutMinimum,
        //   minHopPriceX36, hookData.
        //
        // `minHopPriceX36` inilah yang tidak ada di Uniswap standar. Nilainya
        // nol, dan itu yang terbaca pada transaksi sungguhan.
        let isi = (
            self.pool.currency0,
            self.pool.currency1,
            alloy::primitives::aliases::U24::from(self.pool.fee),
            alloy::primitives::aliases::I24::try_from(self.pool.tick_spacing).unwrap_or_default(),
            self.pool.hooks,
            self.zero_for_one,
            self.amount_in,
            self.amount_out_minimum,
            U256::ZERO,
            Bytes::new(),
        );
        // Dibungkus satu lapis lagi supaya struct dinamis mendapat offsetnya
        // sendiri, persis seperti pada transaksi sungguhan.
        Bytes::from((isi,).abi_encode_params())
    }
}

/// GATE G2 untuk swap. Bangun calldata `execute` lengkap.
pub fn swap_exact_in(req: &SwapExactIn) -> Result<Bytes, RouterError> {
    req.validate()?;

    let (bayar, terima) = req.sisi();

    let aksi_bytes = vec![aksi::SWAP_EXACT_IN_SINGLE, aksi::SETTLE_ALL, aksi::TAKE_ALL];
    let params: Vec<Bytes> = vec![
        req.params_swap(),
        // SETTLE_ALL: currency yang dibayarkan beserta batas atasnya.
        Bytes::from((bayar, U256::from(req.amount_in)).abi_encode_params()),
        // TAKE_ALL: currency yang diterima beserta batas bawahnya.
        Bytes::from((terima, U256::from(req.amount_out_minimum)).abi_encode_params()),
    ];

    let input = Bytes::from((Bytes::from(aksi_bytes), params).abi_encode_params());

    Ok(Bytes::from(
        IUniversalRouter::executeCall {
            commands: Bytes::from(vec![perintah::V4_SWAP]),
            inputs: vec![input],
            deadline: req.deadline,
        }
        .abi_encode(),
    ))
}

use alloy::primitives::address;

/// Parameter apa adanya dari transaksi
/// 0xf15c5f65d6f0372698805158351bf8c13de3578692a721a06094a365449fcd86.
fn swap_asli() -> SwapExactIn {
    SwapExactIn {
        pool: KunciPool {
            currency0: Address::ZERO,
            currency1: address!("7fdcc669014afa2c33df27a96169f900b17f737d"),
            fee: 0,
            tick_spacing: 200,
            hooks: address!("e5e702641ea86f4ae6cc3cdaed2b886f976be044"),
        },
        zero_for_one: true,
        amount_in: 70_000_000_000_000_008,
        amount_out_minimum: 285_364_610_143_729_065_835_706,
        deadline: U256::from(1_788_789_936u64),
    }
}

pub(crate) fn golden_matches() -> Result<(), String> {
    if hex::encode(swap_exact_in(&swap_asli()).map_err(|e| e.to_string())?)
        == include_str!("../../fixtures/golden/swap-f15c5f65.hex").trim()
    {
        Ok(())
    } else {
        Err("calldata berbeda dari transaksi acuan".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn calldata_sama_persis_dengan_transaksi_swap_sungguhan() {
        // Ini pembuktian terpenting di modul ini. Kalau posisi `minHopPriceX36`
        // salah, atau urutan aksinya keliru, uji ini yang menangkapnya.
        let asli = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures/golden/swap-f15c5f65.hex"),
        )
        .expect("fixture swap harus ada");
        let asli = asli.trim();

        let hasil = swap_exact_in(&swap_asli()).unwrap();
        assert_eq!(hex::encode(&hasil), asli);
        assert_eq!(
            hasil.len(),
            1124,
            "panjang harus sama dengan transaksi asli"
        );
    }

    #[test]
    fn field_fork_menggeser_offset_hookdata() {
        // Penjaga langsung terhadap kekeliruan yang paling mungkin: memakai
        // struct Uniswap standar. Tanpa field tambahan, offset hookData akan
        // bernilai 0x120, bukan 0x140.
        let data = swap_exact_in(&swap_asli()).unwrap();
        let hex = hex::encode(&data);
        assert!(
            hex.contains("0000000000000000000000000000000000000000000000000000000000000140"),
            "offset hookData harus 0x140, yaitu sepuluh kata, bukan sembilan"
        );
    }

    #[test]
    fn urutan_aksi_swap_settle_take() {
        let data = swap_exact_in(&swap_asli()).unwrap();
        let hex = hex::encode(&data);
        assert!(
            hex.contains("060c0f00000000000000000000000000000000000000000000000000000000"),
            "aksi harus 06 0c 0f, yaitu swap lalu settle lalu take"
        );
    }

    #[test]
    fn tanpa_pengaman_slippage_ditolak() {
        // amountOutMinimum nol berarti bersedia menerima berapa pun. Pada
        // memecoin itu berarti bersedia kehilangan hampir seluruhnya.
        let mut r = swap_asli();
        r.amount_out_minimum = 0;
        assert_eq!(
            r.validate().unwrap_err(),
            RouterError::TanpaPengamanSlippage
        );
    }

    #[test]
    fn jumlah_masuk_nol_ditolak() {
        let mut r = swap_asli();
        r.amount_in = 0;
        assert_eq!(r.validate().unwrap_err(), RouterError::JumlahNol);
    }

    #[test]
    fn urutan_currency_terbalik_ditolak() {
        let mut r = swap_asli();
        std::mem::swap(&mut r.pool.currency0, &mut r.pool.currency1);
        assert_eq!(r.validate().unwrap_err(), RouterError::UrutanCurrencySalah);
    }

    #[test]
    fn deadline_nol_ditolak() {
        let mut r = swap_asli();
        r.deadline = U256::ZERO;
        assert_eq!(r.validate().unwrap_err(), RouterError::DeadlineNol);
    }

    #[test]
    fn membayar_eth_asli_mengirim_nilai() {
        // ETH asli tidak ditarik lewat izin, ia harus ikut sebagai msg.value.
        assert_eq!(swap_asli().value(), U256::from(70_000_000_000_000_008u64));
    }

    #[test]
    fn membayar_token_tidak_mengirim_nilai() {
        let mut r = swap_asli();
        r.pool.currency0 = address!("0000000000000000000000000000000000000001");
        assert_eq!(r.value(), U256::ZERO);
    }

    #[test]
    fn arah_terbalik_menukar_sisi_bayar_dan_terima() {
        let mut r = swap_asli();
        r.zero_for_one = false;
        let (bayar, terima) = r.sisi();
        assert_eq!(bayar, r.pool.currency1);
        assert_eq!(terima, r.pool.currency0);
        // Yang dibayarkan token, jadi tidak ada ETH yang ikut.
        assert_eq!(r.value(), U256::ZERO);
    }
}
