//! Gate G2: pembangun calldata bertipe.
//!
//! INVARIAN
//! - Calldata dibangun dari tipe Rust hasil ABI, TIDAK PERNAH dari rangkaian
//!   string heksadesimal. Merangkai hex dengan tangan membuat kesalahan urutan
//!   field mustahil terlihat saat dibaca.
//! - Setiap fungsi di sini murni, tidak menyentuh jaringan, dan hasilnya
//!   deterministik. Itu yang membuat golden vector di gate G3 bisa
//!   membandingkan byte per byte secara offline.
//! - `deadline` dan `recipient` datang dari pemanggil, bukan dibuat di sini,
//!   supaya keduanya ikut terkunci golden vector dan tidak berubah diam diam.
//!
//! Gate berikutnya, yaitu G3 golden vector, G4 simulasi, dan G5 asersi
//! invarian, ada di modulnya masing masing. Lihat PRD 6.4.

use alloy::primitives::{
    aliases::{I24, U24},
    Address, Bytes, U256,
};
use alloy::sol_types::SolCall;

use crate::math::{MAX_TICK, MIN_TICK};

use super::abi_write::INonfungiblePositionManager as Npm;

/// Parameter membuka posisi, dalam bentuk yang sudah tidak ambigu.
///
/// Semua nilai sudah dalam satuan terkecil dan tick sudah di-snap ke grid.
/// Modul ini tidak melakukan konversi apa pun, karena konversi yang tersembunyi
/// di dalam pembangun calldata adalah tempat kesalahan bersembunyi.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintRequest {
    pub token0: Address,
    pub token1: Address,
    pub fee: u32,
    pub tick_lower: i32,
    pub tick_upper: i32,
    pub amount0_desired: U256,
    pub amount1_desired: U256,
    pub amount0_min: U256,
    pub amount1_min: U256,
    pub recipient: Address,
    pub deadline: U256,
}

#[derive(Debug, thiserror::Error)]
pub enum CalldataError {
    #[error("tick bawah {lower} tidak boleh sama atau melebihi tick atas {upper}")]
    TickOrder { lower: i32, upper: i32 },

    #[error("tick {tick} tidak pas di grid dengan spacing {spacing}")]
    TickOffGrid { tick: i32, spacing: i32 },

    #[error("token0 {0} dan token1 harus berbeda dan urut menaik")]
    TokenOrder(Address),

    #[error("jumlah minimum {min} melebihi jumlah yang diinginkan {desired}")]
    MinAboveDesired { min: U256, desired: U256 },

    #[error("kedua jumlah nol, tidak ada yang bisa disetor")]
    NothingToDeposit,

    #[error("fee {0} di luar rentang uint24")]
    FeeTooLarge(u32),

    #[error("deadline nol berarti transaksi tidak akan pernah sah")]
    ZeroDeadline,

    #[error("tick {0} di luar rentang Uniswap")]
    TickOutOfRange(i32),
}

impl MintRequest {
    /// Periksa hal hal yang membuat calldata pasti ditolak kontrak.
    ///
    /// Ini bukan pengganti simulasi di gate G4. Yang diperiksa di sini hanyalah
    /// kesalahan bentuk yang bisa diketahui tanpa menyentuh rantai, supaya
    /// kegagalannya cepat dan pesannya jelas, bukan berupa revert tanpa alasan.
    pub fn validate(&self, tick_spacing: i32) -> Result<(), CalldataError> {
        if self.token0 >= self.token1 {
            return Err(CalldataError::TokenOrder(self.token0));
        }
        if self.tick_lower >= self.tick_upper {
            return Err(CalldataError::TickOrder {
                lower: self.tick_lower,
                upper: self.tick_upper,
            });
        }
        // Rentang tick diperiksa di sini supaya konversi ke `I24` di
        // `to_params` dijamin berhasil dan tidak perlu mengembalikan Result.
        for tick in [self.tick_lower, self.tick_upper] {
            if !(MIN_TICK..=MAX_TICK).contains(&tick) {
                return Err(CalldataError::TickOutOfRange(tick));
            }
        }
        if tick_spacing > 0 {
            for tick in [self.tick_lower, self.tick_upper] {
                if tick % tick_spacing != 0 {
                    return Err(CalldataError::TickOffGrid {
                        tick,
                        spacing: tick_spacing,
                    });
                }
            }
        }
        if self.fee > 0xFF_FFFF {
            return Err(CalldataError::FeeTooLarge(self.fee));
        }
        if self.amount0_desired.is_zero() && self.amount1_desired.is_zero() {
            return Err(CalldataError::NothingToDeposit);
        }
        if self.amount0_min > self.amount0_desired {
            return Err(CalldataError::MinAboveDesired {
                min: self.amount0_min,
                desired: self.amount0_desired,
            });
        }
        if self.amount1_min > self.amount1_desired {
            return Err(CalldataError::MinAboveDesired {
                min: self.amount1_min,
                desired: self.amount1_desired,
            });
        }
        if self.deadline.is_zero() {
            return Err(CalldataError::ZeroDeadline);
        }
        Ok(())
    }

    fn to_params(&self) -> Npm::MintParams {
        Npm::MintParams {
            token0: self.token0,
            token1: self.token1,
            fee: U24::from(self.fee),
            // `validate` sudah memastikan kedua tick ada di rentang Uniswap,
            // yang seluruhnya muat di i24, jadi konversi ini tidak bisa gagal.
            tickLower: I24::try_from(self.tick_lower).expect("tick sudah divalidasi"),
            tickUpper: I24::try_from(self.tick_upper).expect("tick sudah divalidasi"),
            amount0Desired: self.amount0_desired,
            amount1Desired: self.amount1_desired,
            amount0Min: self.amount0_min,
            amount1Min: self.amount1_min,
            recipient: self.recipient,
            deadline: self.deadline,
        }
    }
}

/// Calldata untuk `mint`.
pub fn mint(req: &MintRequest, tick_spacing: i32) -> Result<Bytes, CalldataError> {
    req.validate(tick_spacing)?;
    Ok(Npm::mintCall {
        params: req.to_params(),
    }
    .abi_encode()
    .into())
}

/// Calldata untuk menambah likuiditas ke NFT V3 yang sudah ada.
pub fn increase_liquidity(
    token_id: U256,
    amount0_desired: U256,
    amount1_desired: U256,
    amount0_min: U256,
    amount1_min: U256,
    deadline: U256,
) -> Result<Bytes, CalldataError> {
    if amount0_desired.is_zero() && amount1_desired.is_zero() {
        return Err(CalldataError::NothingToDeposit);
    }
    if amount0_min > amount0_desired {
        return Err(CalldataError::MinAboveDesired {
            min: amount0_min,
            desired: amount0_desired,
        });
    }
    if amount1_min > amount1_desired {
        return Err(CalldataError::MinAboveDesired {
            min: amount1_min,
            desired: amount1_desired,
        });
    }
    if deadline.is_zero() {
        return Err(CalldataError::ZeroDeadline);
    }
    Ok(Npm::increaseLiquidityCall {
        params: Npm::IncreaseLiquidityParams {
            tokenId: token_id,
            amount0Desired: amount0_desired,
            amount1Desired: amount1_desired,
            amount0Min: amount0_min,
            amount1Min: amount1_min,
            deadline,
        },
    }
    .abi_encode()
    .into())
}

/// Calldata untuk `decreaseLiquidity`.
pub fn decrease_liquidity(
    token_id: U256,
    liquidity: u128,
    amount0_min: U256,
    amount1_min: U256,
    deadline: U256,
) -> Bytes {
    Npm::decreaseLiquidityCall {
        params: Npm::DecreaseLiquidityParams {
            tokenId: token_id,
            liquidity,
            amount0Min: amount0_min,
            amount1Min: amount1_min,
            deadline,
        },
    }
    .abi_encode()
    .into()
}

/// Nilai maksimum `uint128`, dipakai saat mengumpulkan seluruh fee.
pub fn max_u128() -> u128 {
    u128::MAX
}

/// Calldata untuk `collect`.
///
/// Memakai `amount_max` penuh berarti ambil semua yang tersedia. Itu perilaku
/// yang benar untuk klaim fee, karena jumlah pastinya baru diketahui kontrak.
pub fn collect(token_id: U256, recipient: Address) -> Bytes {
    Npm::collectCall {
        params: Npm::CollectParams {
            tokenId: token_id,
            recipient,
            amount0Max: max_u128(),
            amount1Max: max_u128(),
        },
    }
    .abi_encode()
    .into()
}

/// Calldata untuk `burn`.
pub fn burn(token_id: U256) -> Bytes {
    Npm::burnCall { tokenId: token_id }.abi_encode().into()
}

/// Bungkus beberapa panggilan menjadi satu transaksi atomik.
///
/// Ini yang membuat tutup posisi tidak bisa setengah jalan. Menarik likuiditas,
/// mengumpulkan fee, dan membakar NFT harus terjadi bersama sama, atau tidak
/// sama sekali. Lihat PRD 6.2.
pub fn multicall(calls: Vec<Bytes>) -> Bytes {
    Npm::multicallCall { data: calls }.abi_encode().into()
}

/// Rangkaian panggilan untuk menutup posisi sepenuhnya.
pub fn close_position(
    token_id: U256,
    liquidity: u128,
    amount0_min: U256,
    amount1_min: U256,
    recipient: Address,
    deadline: U256,
) -> Bytes {
    multicall(vec![
        decrease_liquidity(token_id, liquidity, amount0_min, amount1_min, deadline),
        collect(token_id, recipient),
        burn(token_id),
    ])
}

/// Tutup NFT lama dan buka NFT baru dalam satu transaksi V3.
///
/// Semua subpanggilan berada di `multicall`, sehingga kegagalan mint baru
/// mengembalikan seluruh penarikan posisi lama.
pub fn rebalance_position(
    old_token_id: U256,
    old_liquidity: u128,
    amount0_min_ditarik: U256,
    amount1_min_ditarik: U256,
    new_position: &MintRequest,
    tick_spacing: i32,
) -> Result<Bytes, CalldataError> {
    let mint_new = mint(new_position, tick_spacing)?;
    Ok(multicall(vec![
        decrease_liquidity(
            old_token_id,
            old_liquidity,
            amount0_min_ditarik,
            amount1_min_ditarik,
            new_position.deadline,
        ),
        collect(old_token_id, new_position.recipient),
        burn(old_token_id),
        mint_new,
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    fn token_a() -> Address {
        address!("0bd7d308f8e1639fab988df18a8011f41eacad73")
    }
    fn token_b() -> Address {
        address!("39dbed3a2bd333467115de45665cc57f813c4571")
    }
    fn dompet() -> Address {
        address!("48958e7e35b96fb58fca9198436c3cb00d81b3cb")
    }

    fn permintaan() -> MintRequest {
        MintRequest {
            token0: token_a(),
            token1: token_b(),
            fee: 3_000,
            tick_lower: 76_200,
            tick_upper: 81_000,
            amount0_desired: U256::from(1_000_000_000_000_000_000u64),
            amount1_desired: U256::from(2_000_000_000_000_000_000u64),
            amount0_min: U256::from(990_000_000_000_000_000u64),
            amount1_min: U256::from(1_980_000_000_000_000_000u64),
            recipient: dompet(),
            deadline: U256::from(1_788_700_000u64),
        }
    }

    #[test]
    fn calldata_mint_deterministik() {
        let a = mint(&permintaan(), 60).unwrap();
        let b = mint(&permintaan(), 60).unwrap();
        assert_eq!(a, b, "calldata harus sama untuk masukan yang sama");
        // Selector empat byte plus satu blok 32 byte per field, 11 field.
        assert_eq!(a.len(), 4 + 32 * 11);
    }

    #[test]
    fn selector_mint_sesuai_tanda_tangan() {
        // Selector `mint((address,address,uint24,int24,int24,uint256,uint256,
        // uint256,uint256,address,uint256))` adalah 0x88316456. Nilai ini
        // tercantum di explorer mana pun untuk NonfungiblePositionManager.
        let data = mint(&permintaan(), 60).unwrap();
        assert_eq!(&data[..4], &[0x88, 0x31, 0x64, 0x56]);
    }

    #[test]
    fn selector_fungsi_lain_sesuai() {
        assert_eq!(&burn(U256::from(1u8))[..4], &[0x42, 0x96, 0x6c, 0x68]);
        assert_eq!(
            &collect(U256::from(1u8), dompet())[..4],
            &[0xfc, 0x6f, 0x78, 0x65]
        );
        assert_eq!(
            &decrease_liquidity(U256::from(1u8), 1, U256::ZERO, U256::ZERO, U256::from(1u8))[..4],
            &[0x0c, 0x49, 0xcc, 0xbe]
        );
        assert_eq!(&multicall(vec![])[..4], &[0xac, 0x96, 0x50, 0xd8]);
    }

    #[test]
    fn urutan_token_salah_ditolak() {
        let mut req = permintaan();
        std::mem::swap(&mut req.token0, &mut req.token1);
        assert!(matches!(mint(&req, 60), Err(CalldataError::TokenOrder(_))));

        // Token yang sama juga tidak sah.
        req.token1 = req.token0;
        assert!(mint(&req, 60).is_err());
    }

    #[test]
    fn tick_terbalik_ditolak() {
        let mut req = permintaan();
        std::mem::swap(&mut req.tick_lower, &mut req.tick_upper);
        assert!(matches!(
            mint(&req, 60),
            Err(CalldataError::TickOrder { .. })
        ));
    }

    #[test]
    fn tick_di_luar_grid_ditolak() {
        let mut req = permintaan();
        req.tick_lower = 76_201;
        match mint(&req, 60) {
            Err(CalldataError::TickOffGrid { tick, spacing }) => {
                assert_eq!(tick, 76_201);
                assert_eq!(spacing, 60);
            }
            other => panic!("harusnya TickOffGrid, dapat {other:?}"),
        }
    }

    #[test]
    fn minimum_di_atas_yang_diinginkan_ditolak() {
        let mut req = permintaan();
        req.amount0_min = req.amount0_desired + U256::from(1u8);
        assert!(matches!(
            mint(&req, 60),
            Err(CalldataError::MinAboveDesired { .. })
        ));
    }

    #[test]
    fn setoran_kosong_ditolak() {
        let mut req = permintaan();
        req.amount0_desired = U256::ZERO;
        req.amount1_desired = U256::ZERO;
        req.amount0_min = U256::ZERO;
        req.amount1_min = U256::ZERO;
        assert!(matches!(
            mint(&req, 60),
            Err(CalldataError::NothingToDeposit)
        ));
    }

    #[test]
    fn deadline_nol_ditolak() {
        let mut req = permintaan();
        req.deadline = U256::ZERO;
        assert!(matches!(mint(&req, 60), Err(CalldataError::ZeroDeadline)));
    }

    #[test]
    fn tick_negatif_terkodekan_benar() {
        // Tick negatif memakai representasi dua pelengkap pada int24. Kalau
        // pengkodeannya salah, kontrak akan membaca tick yang sangat besar dan
        // posisinya mendarat di tempat yang sama sekali berbeda.
        let mut req = permintaan();
        req.tick_lower = -60;
        req.tick_upper = 60;
        let data = mint(&req, 60).unwrap();

        // Field tickLower ada di blok ke tiga setelah selector.
        let blok = &data[4 + 32 * 3..4 + 32 * 4];
        // Dua pelengkap untuk -60 pada int24 yang dipadatkan ke 32 byte akan
        // berisi 0xff berulang lalu 0xc4.
        assert_eq!(blok[31], 0xc4, "byte terendah -60 seharusnya 0xc4");
        assert!(
            blok[..31].iter().all(|&b| b == 0xff),
            "tick negatif harus dipadatkan dengan 0xff"
        );
    }

    #[test]
    fn menutup_posisi_membungkus_tiga_panggilan() {
        let data = close_position(
            U256::from(42u8),
            1_000,
            U256::ZERO,
            U256::ZERO,
            dompet(),
            U256::from(1_788_700_000u64),
        );
        assert_eq!(&data[..4], &[0xac, 0x96, 0x50, 0xd8], "harus multicall");
        // Ketiga selector anak harus muncul di dalam muatannya.
        let hex = alloy::hex::encode(&data);
        assert!(hex.contains("0c49ccbe"), "decreaseLiquidity harus ada");
        assert!(hex.contains("fc6f7865"), "collect harus ada");
        assert!(hex.contains("42966c68"), "burn harus ada");
    }

    #[test]
    fn tambah_likuiditas_memakai_selector_resmi() {
        let data = increase_liquidity(
            U256::from(42u8),
            U256::from(10u8),
            U256::from(20u8),
            U256::from(9u8),
            U256::from(18u8),
            U256::from(1u8),
        )
        .unwrap();
        assert_eq!(&data[..4], &[0x21, 0x9f, 0x5d, 0x17]);
    }

    #[test]
    fn rebalance_v3_atomik_memuat_empat_subpanggilan() {
        use alloy::sol_types::SolCall;

        let data = rebalance_position(
            U256::from(42u8),
            100,
            U256::from(9u8),
            U256::from(18u8),
            &permintaan(),
            60,
        )
        .unwrap();
        let decoded = Npm::multicallCall::abi_decode(&data).unwrap();
        assert_eq!(decoded.data.len(), 4);
        assert_eq!(&decoded.data[0][..4], &[0x0c, 0x49, 0xcc, 0xbe]);
        assert_eq!(&decoded.data[1][..4], &[0xfc, 0x6f, 0x78, 0x65]);
        assert_eq!(&decoded.data[2][..4], &[0x42, 0x96, 0x6c, 0x68]);
        assert_eq!(&decoded.data[3][..4], &[0x88, 0x31, 0x64, 0x56]);
    }
}
