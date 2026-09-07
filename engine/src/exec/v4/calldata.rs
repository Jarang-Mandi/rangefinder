//! Gate G2 untuk V4: penyusun calldata `modifyLiquidities`.
//!
//! INVARIAN
//! - Setiap nilai diperiksa SEBELUM dikodekan. Calldata yang salah bentuknya
//!   akan revert, dan itu murah. Calldata yang benar bentuknya tapi salah
//!   artinya akan berhasil, dan itu mahal.
//! - Tick wajib kelipatan `tickSpacing` pool. V4 menolak yang tidak, dan
//!   penolakan itu terjadi setelah gas terbakar.
//! - `amount0Max` dan `amount1Max` adalah satu satunya pengaman slippage di
//!   jalur ini. V4 tidak punya `amountMin` seperti V3, jadi batas atas inilah
//!   yang mencegah kita membayar lebih dari yang dimaksud.
//! - Nilai ETH asli TIDAK boleh dikirim untuk pool yang kedua currency-nya
//!   token. ETH yang ikut tanpa currency nol akan tersangkut di kontrak.

use alloy::primitives::{Address, Bytes, U256};
use alloy::sol_types::SolCall;

use super::abi::IPositionManagerV4 as Pm;
use super::abi::IPositionManagerV4::PoolKey;
use super::actions::{self, MintPosition};

/// Permintaan membuka posisi V4.
#[derive(Debug, Clone)]
pub struct MintV4Request {
    pub pool_key: PoolKey,
    pub tick_lower: i32,
    pub tick_upper: i32,
    pub liquidity: u128,
    pub amount0_max: u128,
    pub amount1_max: u128,
    pub owner: Address,
    pub deadline: U256,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum CalldataV4Error {
    #[error("tickLower {lower} tidak lebih kecil dari tickUpper {upper}")]
    TickTerbalik { lower: i32, upper: i32 },
    #[error("tick {tick} bukan kelipatan tickSpacing {spacing}, kontrak akan menolaknya")]
    TickTidakDiGrid { tick: i32, spacing: i32 },
    #[error("tickSpacing {0} tidak masuk akal")]
    SpacingTidakValid(i32),
    #[error("likuiditas nol, tidak ada posisi yang tercipta")]
    LikuiditasNol,
    #[error("kedua batas jumlah nol, tidak ada token yang boleh dipakai")]
    BatasJumlahNol,
    #[error("currency0 {c0} tidak lebih kecil dari currency1 {c1}, urutan PoolKey salah dan poolId-nya akan berbeda")]
    UrutanCurrencySalah { c0: Address, c1: Address },
    #[error("deadline nol, transaksi tidak akan pernah dianggap sah")]
    DeadlineNol,
    #[error("owner kosong, posisi akan tercipta atas nama alamat nol dan hilang selamanya")]
    OwnerKosong,
}

impl MintV4Request {
    /// Periksa seluruh nilai sebelum ada byte yang dikodekan.
    ///
    /// Mengembalikan kesalahan PERTAMA yang ditemukan, karena setiap kesalahan
    /// di sini cukup untuk membatalkan, dan menumpuknya tidak membantu.
    pub fn validate(&self) -> Result<(), CalldataV4Error> {
        let spacing = self.pool_key.tickSpacing.as_i32();
        if spacing <= 0 {
            return Err(CalldataV4Error::SpacingTidakValid(spacing));
        }
        if self.tick_lower >= self.tick_upper {
            return Err(CalldataV4Error::TickTerbalik {
                lower: self.tick_lower,
                upper: self.tick_upper,
            });
        }
        for tick in [self.tick_lower, self.tick_upper] {
            if tick % spacing != 0 {
                return Err(CalldataV4Error::TickTidakDiGrid { tick, spacing });
            }
        }
        if self.liquidity == 0 {
            return Err(CalldataV4Error::LikuiditasNol);
        }
        if self.amount0_max == 0 && self.amount1_max == 0 {
            return Err(CalldataV4Error::BatasJumlahNol);
        }
        // Urutan currency menentukan poolId. Terbalik berarti kita menyebut
        // pool yang sama sekali lain, yang biasanya tidak ada, tapi bisa saja
        // ada dan berisi dana orang lain.
        if self.pool_key.currency0 >= self.pool_key.currency1 {
            return Err(CalldataV4Error::UrutanCurrencySalah {
                c0: self.pool_key.currency0,
                c1: self.pool_key.currency1,
            });
        }
        if self.deadline.is_zero() {
            return Err(CalldataV4Error::DeadlineNol);
        }
        if self.owner.is_zero() {
            return Err(CalldataV4Error::OwnerKosong);
        }
        Ok(())
    }

    /// Berapa ETH asli yang harus ikut sebagai `msg.value`.
    ///
    /// Bukan nol hanya kalau salah satu currency adalah alamat nol, yang di V4
    /// berarti ETH asli. Untuk pool yang kedua sisinya token, mengirim ETH akan
    /// membuatnya tersangkut di kontrak.
    pub fn value(&self) -> U256 {
        if self.pool_key.currency0 == actions::CURRENCY_NATIVE {
            U256::from(self.amount0_max)
        } else if self.pool_key.currency1 == actions::CURRENCY_NATIVE {
            U256::from(self.amount1_max)
        } else {
            U256::ZERO
        }
    }
}

/// GATE G2. Bangun calldata `modifyLiquidities` untuk membuka posisi.
pub fn mint(req: &MintV4Request) -> Result<Bytes, CalldataV4Error> {
    req.validate()?;

    let mint = MintPosition {
        pool_key: req.pool_key.clone(),
        tick_lower: req.tick_lower,
        tick_upper: req.tick_upper,
        liquidity: req.liquidity,
        amount0_max: req.amount0_max,
        amount1_max: req.amount1_max,
        owner: req.owner,
        // Hook data kosong. Hook yang menuntut data khusus belum didukung, dan
        // menebak isinya lebih berbahaya daripada menolak poolnya.
        hook_data: Bytes::new(),
    };

    let aksi = vec![
        mint.encode(),
        actions::settle_pair(req.pool_key.currency0, req.pool_key.currency1),
    ];

    // Kegagalan di sini berarti daftar aksinya sendiri tidak masuk akal, dan
    // itu kesalahan kode kita, bukan masukan user.
    let unlock = actions::unlock_data(&aksi).expect("mint selalu berpasangan dengan settle");

    Ok(Bytes::from(
        Pm::modifyLiquiditiesCall {
            unlockData: unlock,
            deadline: req.deadline,
        }
        .abi_encode(),
    ))
}

/// Bangun calldata menutup posisi: kurangi seluruh likuiditas, bakar NFT-nya,
/// lalu ambil kedua currency.
pub fn close_position(
    token_id: U256,
    currency0: Address,
    currency1: Address,
    penerima: Address,
    deadline: U256,
) -> Result<Bytes, CalldataV4Error> {
    if deadline.is_zero() {
        return Err(CalldataV4Error::DeadlineNol);
    }
    if penerima.is_zero() {
        return Err(CalldataV4Error::OwnerKosong);
    }

    // BURN_POSITION sudah mengurangi seluruh likuiditas sekaligus membakar
    // NFT-nya, jadi DECREASE_LIQUIDITY terpisah tidak diperlukan. Batas minimum
    // nol dipakai karena yang ditutup adalah posisi kita sendiri secara penuh,
    // bukan pertukaran yang bisa dijepit.
    let aksi = vec![
        actions::burn_position(token_id, 0, 0, Bytes::new()),
        actions::take_pair(currency0, currency1, penerima),
    ];
    let unlock = actions::unlock_data(&aksi).expect("burn selalu berpasangan dengan take");

    Ok(Bytes::from(
        Pm::modifyLiquiditiesCall {
            unlockData: unlock,
            deadline,
        }
        .abi_encode(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;
    use alloy::primitives::aliases::{I24, U24};

    fn key() -> PoolKey {
        PoolKey {
            currency0: address!("5fc5360d0400a0fd4f2af552add042d716f1d168"),
            currency1: address!("791e750fa8b2237288a6fe6c47ea81c50093449a"),
            fee: U24::from(100_000u32),
            tickSpacing: I24::try_from(1000i32).unwrap(),
            hooks: Address::ZERO,
        }
    }

    fn req() -> MintV4Request {
        MintV4Request {
            pool_key: key(),
            tick_lower: 357_000,
            tick_upper: 367_000,
            liquidity: 14_279_001_511_288_904,
            amount0_max: 100_000_000,
            amount1_max: 0,
            owner: address!("420ac1fcca78c4cdfed3216201216ed80e7b2680"),
            deadline: U256::from(1_788_785_927u64),
        }
    }

    /// Calldata apa adanya dari transaksi
    /// 0x92acbe0bc669fbdff06263646d45399825c1a398090fbe42e8530bde610f1ccd.
    const CALLDATA_ASLI: &str = concat!(
        "dd46508f",
        "0000000000000000000000000000000000000000000000000000000000000040",
        "000000000000000000000000000000000000000000000000000000006a9eb507",
        "0000000000000000000000000000000000000000000000000000000000000300",
        "0000000000000000000000000000000000000000000000000000000000000040",
        "0000000000000000000000000000000000000000000000000000000000000080",
        "0000000000000000000000000000000000000000000000000000000000000002",
        "020d000000000000000000000000000000000000000000000000000000000000",
        "0000000000000000000000000000000000000000000000000000000000000002",
        "0000000000000000000000000000000000000000000000000000000000000040",
        "0000000000000000000000000000000000000000000000000000000000000200",
        "00000000000000000000000000000000000000000000000000000000000001a0",
        "0000000000000000000000005fc5360d0400a0fd4f2af552add042d716f1d168",
        "000000000000000000000000791e750fa8b2237288a6fe6c47ea81c50093449a",
        "00000000000000000000000000000000000000000000000000000000000186a0",
        "00000000000000000000000000000000000000000000000000000000000003e8",
        "0000000000000000000000000000000000000000000000000000000000000000",
        "0000000000000000000000000000000000000000000000000000000000057288",
        "0000000000000000000000000000000000000000000000000000000000059998",
        "0000000000000000000000000000000000000000000000000032baad1cd3e048",
        "0000000000000000000000000000000000000000000000000000000005f5e100",
        "0000000000000000000000000000000000000000000000000000000000000000",
        "000000000000000000000000420ac1fcca78c4cdfed3216201216ed80e7b2680",
        "0000000000000000000000000000000000000000000000000000000000000180",
        "0000000000000000000000000000000000000000000000000000000000000000",
        "0000000000000000000000000000000000000000000000000000000000000040",
        "0000000000000000000000005fc5360d0400a0fd4f2af552add042d716f1d168",
        "000000000000000000000000791e750fa8b2237288a6fe6c47ea81c50093449a",
    );

    #[test]
    fn calldata_sama_persis_dengan_transaksi_sungguhan() {
        let hasil = mint(&req()).unwrap();
        assert_eq!(hex::encode(&hasil), CALLDATA_ASLI);
        assert_eq!(hasil.len(), 868, "panjang harus sama dengan transaksi asli");
    }

    #[test]
    fn tick_di_luar_grid_ditolak() {
        let mut r = req();
        r.tick_lower = 357_001;
        assert_eq!(
            r.validate().unwrap_err(),
            CalldataV4Error::TickTidakDiGrid {
                tick: 357_001,
                spacing: 1000
            }
        );
    }

    #[test]
    fn tick_terbalik_ditolak() {
        let mut r = req();
        std::mem::swap(&mut r.tick_lower, &mut r.tick_upper);
        assert!(matches!(
            r.validate().unwrap_err(),
            CalldataV4Error::TickTerbalik { .. }
        ));
    }

    #[test]
    fn urutan_currency_terbalik_ditolak() {
        // poolId adalah hash dari PoolKey. Urutan yang terbalik menunjuk pool
        // yang sama sekali berbeda, dan itu bisa saja pool yang ada isinya.
        let mut r = req();
        std::mem::swap(&mut r.pool_key.currency0, &mut r.pool_key.currency1);
        assert!(matches!(
            r.validate().unwrap_err(),
            CalldataV4Error::UrutanCurrencySalah { .. }
        ));
    }

    #[test]
    fn likuiditas_nol_ditolak() {
        let mut r = req();
        r.liquidity = 0;
        assert_eq!(r.validate().unwrap_err(), CalldataV4Error::LikuiditasNol);
    }

    #[test]
    fn kedua_batas_nol_ditolak() {
        // Tanpa batas atas, tidak ada token yang boleh dipakai, jadi posisinya
        // pasti gagal. Ditolak lebih awal supaya tidak membakar gas.
        let mut r = req();
        r.amount0_max = 0;
        r.amount1_max = 0;
        assert_eq!(r.validate().unwrap_err(), CalldataV4Error::BatasJumlahNol);
    }

    #[test]
    fn owner_nol_ditolak() {
        let mut r = req();
        r.owner = Address::ZERO;
        assert_eq!(r.validate().unwrap_err(), CalldataV4Error::OwnerKosong);
    }

    #[test]
    fn deadline_nol_ditolak() {
        let mut r = req();
        r.deadline = U256::ZERO;
        assert_eq!(r.validate().unwrap_err(), CalldataV4Error::DeadlineNol);
    }

    #[test]
    fn pool_token_saja_tidak_mengirim_eth() {
        assert_eq!(req().value(), U256::ZERO);
    }

    #[test]
    fn pool_eth_asli_mengirim_nilai_sisi_yang_benar() {
        // ETH asli selalu currency0 karena alamatnya nol, yang terkecil.
        let mut r = req();
        r.pool_key.currency0 = Address::ZERO;
        r.amount0_max = 5_000;
        assert_eq!(r.value(), U256::from(5_000u64));

        // Kalau ETH ada di sisi satu, yang ikut adalah batas sisi satu.
        let mut r2 = req();
        r2.pool_key.currency1 = Address::ZERO;
        r2.pool_key.currency0 = address!("0000000000000000000000000000000000000001");
        r2.amount1_max = 77;
        assert_eq!(r2.value(), U256::from(77u64));
    }

    #[test]
    fn tick_negatif_di_grid_diterima() {
        // Sisa bagi bilangan negatif di Rust bertanda, jadi -1000 % 1000 == 0
        // tetap benar. Uji ini menjaga sifat itu kalau perhitungannya diubah.
        let mut r = req();
        r.tick_lower = -2000;
        r.tick_upper = -1000;
        assert!(r.validate().is_ok());
    }

    #[test]
    fn menutup_posisi_memakai_burn_lalu_take() {
        let data = close_position(
            U256::from(42u8),
            key().currency0,
            key().currency1,
            address!("420ac1fcca78c4cdfed3216201216ed80e7b2680"),
            U256::from(1u8),
        )
        .unwrap();
        let hex = hex::encode(&data);
        assert!(hex.starts_with("dd46508f"), "tetap lewat modifyLiquidities");
        // Daftar aksinya 03 lalu 11, yaitu BURN_POSITION lalu TAKE_PAIR.
        assert!(
            hex.contains("0311000000000000000000000000000000000000000000000000000000000000"),
            "urutan aksi menutup posisi harus burn lalu take"
        );
    }
}
