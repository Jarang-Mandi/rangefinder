//! Pengkodean aksi V4.
//!
//! KENAPA BENTUKNYA BEGINI
//! V4 tidak punya fungsi `mint`. Satu satunya pintu masuk adalah
//! `modifyLiquidities(bytes unlockData, uint256 deadline)`, dan `unlockData`
//! adalah `abi.encode(bytes actions, bytes[] params)`. Setiap byte di `actions`
//! adalah satu aksi, dan `params[i]` adalah parameter untuk aksi ke-i.
//!
//! Bentuk ini TIDAK disalin dari repo Uniswap. Ia dibaca langsung dari
//! transaksi sungguhan di Robinhood Chain, yaitu
//! `0x92acbe0bc669fbdff06263646d45399825c1a398090fbe42e8530bde610f1ccd`, yang
//! membuka posisi USDG pada pool berfee 10 persen. Uji di bawah membangun ulang
//! calldata transaksi itu dan membandingkannya byte per byte.
//!
//! INVARIAN
//! - Jumlah aksi WAJIB sama dengan jumlah params. Kontrak membaca keduanya
//!   berpasangan menurut indeks, dan ketidakcocokan menghasilkan revert yang
//!   sulit dibaca, atau lebih buruk, aksi yang berjalan dengan parameter aksi
//!   lain.
//! - MINT_POSITION selalu diikuti SETTLE_PAIR. Tanpa penyelesaian, token tidak
//!   pernah berpindah dan seluruh transaksi revert dengan `CurrencyNotSettled`.
//! - Currency nol berarti ETH asli. Ia tidak punya `approve`, dan nilainya
//!   ikut sebagai `msg.value`, bukan ditarik lewat Permit2.

use alloy::primitives::{Address, Bytes, U256};
use alloy::sol_types::SolValue;

use super::abi::IPositionManagerV4::PoolKey;

/// Kode aksi, seperti yang terbaca di transaksi sungguhan.
///
/// Angka ini bagian dari kontrak, bukan pilihan kita. Mengubahnya berarti
/// menjalankan aksi yang sama sekali berbeda dengan parameter yang salah.
pub mod kode {
    pub const INCREASE_LIQUIDITY: u8 = 0x00;
    pub const DECREASE_LIQUIDITY: u8 = 0x01;
    pub const MINT_POSITION: u8 = 0x02;
    pub const BURN_POSITION: u8 = 0x03;
    pub const SETTLE_PAIR: u8 = 0x0d;
    pub const TAKE_PAIR: u8 = 0x11;
    pub const SWEEP: u8 = 0x14;
}

/// Alamat currency nol, yang di V4 berarti ETH asli.
pub const CURRENCY_NATIVE: Address = Address::ZERO;

/// Satu aksi beserta parameternya yang sudah dikodekan.
#[derive(Debug, Clone)]
pub struct Aksi {
    pub kode: u8,
    pub params: Bytes,
}

/// Parameter `MINT_POSITION`.
///
/// Urutan field mengikuti apa yang terbaca di transaksi sungguhan:
/// PoolKey diratakan menjadi lima kata, lalu tickLower, tickUpper, liquidity,
/// amount0Max, amount1Max, owner, dan hookData.
#[derive(Debug, Clone)]
pub struct MintPosition {
    pub pool_key: PoolKey,
    pub tick_lower: i32,
    pub tick_upper: i32,
    pub liquidity: u128,
    /// Batas atas token0 yang boleh terpakai. Ini pengaman slippage-nya.
    pub amount0_max: u128,
    pub amount1_max: u128,
    pub owner: Address,
    pub hook_data: Bytes,
}

impl MintPosition {
    pub fn encode(&self) -> Aksi {
        let params = (
            self.pool_key.clone(),
            alloy::primitives::aliases::I24::try_from(self.tick_lower).unwrap_or_default(),
            alloy::primitives::aliases::I24::try_from(self.tick_upper).unwrap_or_default(),
            U256::from(self.liquidity),
            self.amount0_max,
            self.amount1_max,
            self.owner,
            self.hook_data.clone(),
        )
            .abi_encode_params();

        Aksi {
            kode: kode::MINT_POSITION,
            params: Bytes::from(params),
        }
    }
}

/// Parameter `INCREASE_LIQUIDITY` untuk NFT V4 yang sudah ada.
pub fn increase_liquidity(
    token_id: U256,
    liquidity: u128,
    amount0_max: u128,
    amount1_max: u128,
    hook_data: Bytes,
) -> Aksi {
    Aksi {
        kode: kode::INCREASE_LIQUIDITY,
        params: Bytes::from(
            (
                token_id,
                U256::from(liquidity),
                amount0_max,
                amount1_max,
                hook_data,
            )
                .abi_encode_params(),
        ),
    }
}

/// Parameter `SETTLE_PAIR`: dua currency yang harus dibayar penuh.
pub fn settle_pair(currency0: Address, currency1: Address) -> Aksi {
    Aksi {
        kode: kode::SETTLE_PAIR,
        params: Bytes::from((currency0, currency1).abi_encode_params()),
    }
}

/// Parameter `TAKE_PAIR`: dua currency yang hasilnya diambil ke sebuah alamat.
pub fn take_pair(currency0: Address, currency1: Address, penerima: Address) -> Aksi {
    Aksi {
        kode: kode::TAKE_PAIR,
        params: Bytes::from((currency0, currency1, penerima).abi_encode_params()),
    }
}

/// Parameter `BURN_POSITION`.
pub fn burn_position(
    token_id: U256,
    amount0_min: u128,
    amount1_min: u128,
    hook_data: Bytes,
) -> Aksi {
    Aksi {
        kode: kode::BURN_POSITION,
        params: Bytes::from((token_id, amount0_min, amount1_min, hook_data).abi_encode_params()),
    }
}

/// Parameter `DECREASE_LIQUIDITY`.
pub fn decrease_liquidity(
    token_id: U256,
    liquidity: u128,
    amount0_min: u128,
    amount1_min: u128,
    hook_data: Bytes,
) -> Aksi {
    Aksi {
        kode: kode::DECREASE_LIQUIDITY,
        params: Bytes::from(
            (
                token_id,
                U256::from(liquidity),
                amount0_min,
                amount1_min,
                hook_data,
            )
                .abi_encode_params(),
        ),
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ActionError {
    #[error("daftar aksi kosong, tidak ada yang bisa dikerjakan")]
    Kosong,
    #[error("aksi mengubah likuiditas wajib diikuti penyelesaian currency, kalau tidak seluruh transaksi revert dengan CurrencyNotSettled")]
    TanpaPenyelesaian,
}

/// Rakit daftar aksi menjadi `unlockData`.
///
/// `abi.encode(bytes actions, bytes[] params)`, persis seperti yang terbaca di
/// transaksi sungguhan. Jumlah aksi dan jumlah params selalu sama karena
/// keduanya dibangun dari daftar yang sama, bukan dari dua daftar terpisah yang
/// bisa menyimpang.
pub fn unlock_data(aksi: &[Aksi]) -> Result<Bytes, ActionError> {
    if aksi.is_empty() {
        return Err(ActionError::Kosong);
    }

    let mengubah = aksi.iter().any(|a| {
        matches!(
            a.kode,
            kode::MINT_POSITION
                | kode::INCREASE_LIQUIDITY
                | kode::DECREASE_LIQUIDITY
                | kode::BURN_POSITION
        )
    });
    let menyelesaikan = aksi
        .iter()
        .any(|a| matches!(a.kode, kode::SETTLE_PAIR | kode::TAKE_PAIR));
    if mengubah && !menyelesaikan {
        return Err(ActionError::TanpaPenyelesaian);
    }

    let kode_bytes: Vec<u8> = aksi.iter().map(|a| a.kode).collect();
    let params: Vec<Bytes> = aksi.iter().map(|a| a.params.clone()).collect();

    Ok(Bytes::from(
        (Bytes::from(kode_bytes), params).abi_encode_params(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    /// PoolKey dari transaksi sungguhan yang dipakai sebagai acuan.
    fn pool_key_asli() -> PoolKey {
        PoolKey {
            currency0: address!("5fc5360d0400a0fd4f2af552add042d716f1d168"),
            currency1: address!("791e750fa8b2237288a6fe6c47ea81c50093449a"),
            fee: alloy::primitives::aliases::U24::from(100_000u32),
            tickSpacing: alloy::primitives::aliases::I24::try_from(1000i32).unwrap(),
            hooks: Address::ZERO,
        }
    }

    fn mint_asli() -> MintPosition {
        MintPosition {
            pool_key: pool_key_asli(),
            tick_lower: 357_000,
            tick_upper: 367_000,
            liquidity: 14_279_001_511_288_904,
            amount0_max: 100_000_000,
            amount1_max: 0,
            owner: address!("420ac1fcca78c4cdfed3216201216ed80e7b2680"),
            hook_data: Bytes::new(),
        }
    }

    /// `unlockData` apa adanya dari transaksi
    /// 0x92acbe0bc669fbdff06263646d45399825c1a398090fbe42e8530bde610f1ccd.
    ///
    /// Diambil dari rantai, bukan dibangun dari harapan kita, karena
    /// membandingkan hasil terhadap harapan sendiri itu berputar.
    const UNLOCK_ASLI: &str = concat!(
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
    fn unlock_data_sama_persis_dengan_transaksi_sungguhan() {
        let key = pool_key_asli();
        let aksi = vec![
            mint_asli().encode(),
            settle_pair(key.currency0, key.currency1),
        ];
        let hasil = unlock_data(&aksi).unwrap();
        assert_eq!(
            hex::encode(&hasil),
            UNLOCK_ASLI,
            "encoder kita harus menghasilkan byte yang sama dengan transaksi yang sudah berhasil"
        );
    }

    #[test]
    fn kode_aksi_mint_dan_settle_sesuai_rantai() {
        // Dua byte pertama daftar aksi pada transaksi sungguhan adalah 02 0d.
        assert_eq!(kode::MINT_POSITION, 0x02);
        assert_eq!(kode::SETTLE_PAIR, 0x0d);
    }

    #[test]
    fn aksi_kosong_ditolak() {
        assert_eq!(unlock_data(&[]).unwrap_err(), ActionError::Kosong);
    }

    #[test]
    fn mint_tanpa_penyelesaian_ditolak() {
        // Kontrak akan revert dengan CurrencyNotSettled, dan revert itu terjadi
        // setelah gas terbakar. Lebih baik ditolak di sini.
        let hasil = unlock_data(&[mint_asli().encode()]);
        assert_eq!(hasil.unwrap_err(), ActionError::TanpaPenyelesaian);
    }

    #[test]
    fn take_pair_diterima_sebagai_penyelesaian() {
        // Menutup posisi menyelesaikan lewat TAKE_PAIR, bukan SETTLE_PAIR.
        let key = pool_key_asli();
        let aksi = vec![
            burn_position(U256::from(1u8), 0, 0, Bytes::new()),
            take_pair(key.currency0, key.currency1, Address::ZERO),
        ];
        assert!(unlock_data(&aksi).is_ok());
    }

    #[test]
    fn jumlah_aksi_dan_params_selalu_sama() {
        // Keduanya dibangun dari daftar yang sama, jadi tidak bisa menyimpang.
        // Uji ini menjaga sifat itu kalau nanti ada yang memisahkannya.
        let key = pool_key_asli();
        let aksi = vec![
            mint_asli().encode(),
            settle_pair(key.currency0, key.currency1),
        ];
        let data = unlock_data(&aksi).unwrap();
        let hex = hex::encode(&data);
        // Kata ke-2 adalah panjang daftar aksi, kata ke-4 panjang array params.
        let kata = |i: usize| &hex[i * 64..(i + 1) * 64];
        assert_eq!(
            u64::from_str_radix(kata(2).trim_start_matches('0'), 16).unwrap(),
            2
        );
        assert_eq!(
            u64::from_str_radix(kata(4).trim_start_matches('0'), 16).unwrap(),
            2
        );
    }

    #[test]
    fn tick_negatif_terkodekan_sebagai_dua_pelengkap() {
        // Tick di bawah nol lazim pada pool yang token0-nya lebih mahal.
        // Kesalahan tanda di sini menempatkan range di tempat yang sama sekali
        // berbeda, dan posisinya langsung berada di luar range.
        let mut m = mint_asli();
        m.tick_lower = -60;
        m.tick_upper = 60;
        let hex = hex::encode(&m.encode().params);
        let kata = |i: usize| &hex[i * 64..(i + 1) * 64];
        assert_eq!(
            kata(5),
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffc4",
            "tick -60 harus jadi dua pelengkap penuh 32 byte"
        );
        assert_eq!(
            kata(6),
            "000000000000000000000000000000000000000000000000000000000000003c"
        );
    }
}
