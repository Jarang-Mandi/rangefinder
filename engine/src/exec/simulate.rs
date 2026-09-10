//! Gate G4 simulasi dan gate G5 asersi invarian.
//!
//! INVARIAN
//! - TIDAK ADA transaksi yang dikirim tanpa disimulasikan lebih dulu. Simulasi
//!   itu gratis dan mengubah kegagalan yang memakan gas menjadi pesan di layar.
//! - Simulasi yang berhasil BELUM cukup. Hasilnya wajib diperiksa terhadap apa
//!   yang kita harapkan, karena transaksi bisa berhasil sambil melakukan hal
//!   yang berbeda dari niat kita. Itu gate G5.
//! - Pesan revert wajib diterjemahkan ke bahasa manusia. Revert mentah berupa
//!   hex tidak memberi tahu user apa yang harus diperbaiki, dan pada praktiknya
//!   membuat orang menebak nebak lalu mencoba lagi dengan uang sungguhan.

use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::Provider;
use alloy::rpc::types::TransactionRequest;
use alloy::sol_types::SolCall;

use super::abi_write::INonfungiblePositionManager as Npm;

/// Hasil simulasi `mint`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimulatedMint {
    pub token_id: U256,
    pub liquidity: u128,
    pub amount0: U256,
    pub amount1: U256,
}

/// Hasil simulasi aksi V3 yang menambah atau memindahkan likuiditas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimulatedLiquidityChange {
    pub liquidity: u128,
    pub amount0: U256,
    pub amount1: U256,
}

/// Dekode nilai kembali aksi modal V3.
///
/// Rebalance dibungkus `multicall`, sehingga hasil `mint` berada pada elemen
/// terakhir. Dekoder menolak bentuk lain alih alih menganggap `eth_call`
/// sukses sudah cukup.
pub fn decode_v3_liquidity_action(
    action: crate::exec::aksi::JenisAksi,
    output: &[u8],
) -> Result<SimulatedLiquidityChange, String> {
    let decoded = match action {
        crate::exec::aksi::JenisAksi::TambahLikuiditas => {
            let value =
                Npm::increaseLiquidityCall::abi_decode_returns(output).map_err(|error| {
                    format!("hasil increaseLiquidity tidak dapat didekode: {error}")
                })?;
            SimulatedLiquidityChange {
                liquidity: value.liquidity,
                amount0: value.amount0,
                amount1: value.amount1,
            }
        }
        crate::exec::aksi::JenisAksi::Rebalance => {
            let values = Npm::multicallCall::abi_decode_returns(output).map_err(|error| {
                format!("hasil multicall rebalance tidak dapat didekode: {error}")
            })?;
            let mint = values.last().ok_or("hasil multicall rebalance kosong")?;
            let value = Npm::mintCall::abi_decode_returns(mint)
                .map_err(|error| format!("hasil mint rebalance tidak dapat didekode: {error}"))?;
            SimulatedLiquidityChange {
                liquidity: value.liquidity,
                amount0: value.amount0,
                amount1: value.amount1,
            }
        }
        _ => return Err("aksi ini tidak menghasilkan perubahan likuiditas V3".into()),
    };
    Ok(decoded)
}

pub fn check_liquidity_action(
    simulated: &SimulatedLiquidityChange,
    minimum_liquidity: u128,
    max_amount0: U256,
    max_amount1: U256,
) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    if simulated.liquidity < minimum_liquidity || simulated.liquidity == 0 {
        errors.push(format!(
            "likuiditas simulasi {} di bawah minimum {}",
            simulated.liquidity, minimum_liquidity
        ));
    }
    if simulated.amount0 > max_amount0 {
        errors.push(format!(
            "token0 terpakai {} melebihi batas {}",
            simulated.amount0, max_amount0
        ));
    }
    if simulated.amount1 > max_amount1 {
        errors.push(format!(
            "token1 terpakai {} melebihi batas {}",
            simulated.amount1, max_amount1
        ));
    }
    if simulated.amount0.is_zero() && simulated.amount1.is_zero() {
        errors.push("simulasi tidak memakai token apa pun".into());
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Apa yang kita harapkan terjadi. Dibandingkan dengan hasil simulasi di G5.
#[derive(Debug, Clone)]
pub struct MintExpectation {
    /// Likuiditas terkecil yang masih kita terima.
    pub min_liquidity: u128,
    /// Batas atas token yang boleh terpakai, yaitu jumlah yang kita sediakan.
    pub max_amount0: U256,
    pub max_amount1: U256,
}

#[derive(Debug, thiserror::Error)]
pub enum SimulateError {
    #[error("simulasi ditolak kontrak: {0}")]
    Reverted(String),

    #[error("RPC gagal saat simulasi: {0}")]
    Rpc(String),

    #[error("hasil simulasi tidak bisa didekode: {0}")]
    Decode(String),
}

/// Terjemahkan kegagalan `eth_call` menjadi kalimat yang bisa ditindaklanjuti.
///
/// Uniswap memakai pesan pendek seperti `STF` dan `LOK` yang tidak berarti apa
/// apa bagi user. Tabel ini yang mengubahnya jadi tindakan.
pub fn jelaskan_revert(mentah: &str) -> String {
    let r = mentah.to_lowercase();

    let kamus: &[(&str, &str)] = &[
        (
            "stf",
            "transfer token gagal. Saldo kurang, atau approval ke PositionManager belum diberikan",
        ),
        (
            "sta",
            "transfer token gagal saat pengambilan. Periksa approval dan saldo",
        ),
        (
            "price slippage check",
            "harga bergerak melewati batas slippage. Perbarui kutipan lalu coba lagi",
        ),
        (
            "transaction too old",
            "deadline sudah lewat. Kutipan kedaluwarsa, minta yang baru",
        ),
        ("lok", "pool sedang terkunci. Coba lagi sesaat lagi"),
        ("ti", "tick di luar batas yang diizinkan pool"),
        ("tlu", "tick bawah harus lebih kecil dari tick atas"),
        ("tlm", "tick bawah di bawah batas minimum Uniswap"),
        ("tum", "tick atas di atas batas maksimum Uniswap"),
        (
            "invalid tick",
            "tick tidak pas di grid pool. Bulatkan ke kelipatan tick spacing",
        ),
        (
            "insufficient",
            "saldo atau likuiditas tidak cukup untuk aksi ini",
        ),
        (
            "arithmetic",
            "perhitungan kontrak meluap. Biasanya berarti jumlah yang diminta tidak masuk akal",
        ),
    ];

    for (pola, penjelasan) in kamus {
        if r.contains(pola) {
            return format!("{penjelasan} (pesan kontrak: {mentah})");
        }
    }

    format!("kontrak menolak dengan pesan: {mentah}")
}

/// GATE G4. Jalankan `mint` sebagai `eth_call`, tanpa mengirim apa pun.
pub async fn simulate_mint<P: Provider>(
    provider: &P,
    position_manager: Address,
    from: Address,
    calldata: Bytes,
) -> Result<SimulatedMint, SimulateError> {
    let tx = TransactionRequest::default()
        .from(from)
        .to(position_manager)
        .input(calldata.into());

    let hasil = provider.call(tx).await.map_err(|e| {
        let teks = e.to_string();
        // Revert dan kegagalan jaringan sangat berbeda artinya bagi user, jadi
        // dibedakan di sini, bukan digabung jadi satu pesan gagal.
        if teks.contains("revert") || teks.contains("execution reverted") {
            SimulateError::Reverted(jelaskan_revert(&teks))
        } else {
            SimulateError::Rpc(teks)
        }
    })?;

    let keluaran = Npm::mintCall::abi_decode_returns(&hasil)
        .map_err(|e| SimulateError::Decode(e.to_string()))?;

    Ok(SimulatedMint {
        token_id: keluaran.tokenId,
        liquidity: keluaran.liquidity,
        amount0: keluaran.amount0,
        amount1: keluaran.amount1,
    })
}

/// GATE G5. Periksa hasil simulasi terhadap apa yang kita harapkan.
///
/// Simulasi yang berhasil belum berarti hasilnya yang kita mau. Transaksi bisa
/// lolos sambil menyetor jauh lebih sedikit dari niat kita, atau menghasilkan
/// posisi tanpa likuiditas sama sekali.
///
/// Mengembalikan seluruh pelanggaran sekaligus, bukan yang pertama saja, supaya
/// user tidak memperbaiki satu lalu menemukan yang berikutnya.
pub fn check_mint_invariants(
    sim: &SimulatedMint,
    exp: &MintExpectation,
) -> Result<(), Vec<String>> {
    let mut pelanggaran = Vec::new();

    if sim.token_id.is_zero() {
        pelanggaran.push("tokenId nol, posisi tidak tercipta".into());
    }

    if sim.liquidity == 0 {
        pelanggaran.push("likuiditas nol, posisi kosong tidak menghasilkan fee".into());
    } else if sim.liquidity < exp.min_liquidity {
        pelanggaran.push(format!(
            "likuiditas {} di bawah minimum {} yang diharapkan",
            sim.liquidity, exp.min_liquidity
        ));
    }

    // Batas atas ini yang mencegah transaksi memakai lebih banyak token
    // daripada yang kita sediakan.
    if sim.amount0 > exp.max_amount0 {
        pelanggaran.push(format!(
            "token0 terpakai {} melebihi batas {}",
            sim.amount0, exp.max_amount0
        ));
    }
    if sim.amount1 > exp.max_amount1 {
        pelanggaran.push(format!(
            "token1 terpakai {} melebihi batas {}",
            sim.amount1, exp.max_amount1
        ));
    }

    if sim.amount0.is_zero() && sim.amount1.is_zero() {
        pelanggaran.push("tidak ada token yang tersetor sama sekali".into());
    }

    if pelanggaran.is_empty() {
        Ok(())
    } else {
        Err(pelanggaran)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::SolCall;

    fn sim(liquidity: u128, a0: u64, a1: u64) -> SimulatedMint {
        SimulatedMint {
            token_id: U256::from(1234u32),
            liquidity,
            amount0: U256::from(a0),
            amount1: U256::from(a1),
        }
    }

    fn harapan() -> MintExpectation {
        MintExpectation {
            min_liquidity: 1_000,
            max_amount0: U256::from(1_000u32),
            max_amount1: U256::from(2_000u32),
        }
    }

    #[test]
    fn hasil_increase_v3_didekode_dan_dibatasi() {
        let output =
            Npm::increaseLiquidityCall::abi_encode_returns(&Npm::increaseLiquidityReturn {
                liquidity: 1_250u128,
                amount0: U256::from(90u8),
                amount1: U256::from(180u16),
            });
        let result =
            decode_v3_liquidity_action(crate::exec::aksi::JenisAksi::TambahLikuiditas, &output)
                .unwrap();
        assert_eq!(result.liquidity, 1_250);
        assert!(
            check_liquidity_action(&result, 1_200, U256::from(100u8), U256::from(200u16),).is_ok()
        );
        assert!(
            check_liquidity_action(&result, 1_300, U256::from(80u8), U256::from(200u16),).is_err()
        );
    }

    #[test]
    fn hasil_mint_terakhir_dari_multicall_rebalance_didekode() {
        let mint = Bytes::from(Npm::mintCall::abi_encode_returns(&Npm::mintReturn {
            tokenId: U256::from(77u8),
            liquidity: 2_500u128,
            amount0: U256::from(300u16),
            amount1: U256::from(400u16),
        }));
        let output = Npm::multicallCall::abi_encode_returns(&vec![
            Bytes::new(),
            Bytes::new(),
            Bytes::new(),
            mint,
        ]);
        let result =
            decode_v3_liquidity_action(crate::exec::aksi::JenisAksi::Rebalance, &output).unwrap();
        assert_eq!(result.liquidity, 2_500);
        assert_eq!(result.amount0, U256::from(300u16));
        assert_eq!(result.amount1, U256::from(400u16));
    }

    #[test]
    fn hasil_yang_wajar_lolos() {
        assert!(check_mint_invariants(&sim(5_000, 900, 1_900), &harapan()).is_ok());
    }

    #[test]
    fn token_id_nol_ditolak() {
        let mut s = sim(5_000, 900, 1_900);
        s.token_id = U256::ZERO;
        let e = check_mint_invariants(&s, &harapan()).unwrap_err();
        assert!(e.iter().any(|m| m.contains("tokenId nol")));
    }

    #[test]
    fn likuiditas_nol_ditolak() {
        let e = check_mint_invariants(&sim(0, 900, 1_900), &harapan()).unwrap_err();
        assert!(e.iter().any(|m| m.contains("likuiditas nol")));
    }

    #[test]
    fn likuiditas_di_bawah_minimum_ditolak() {
        let e = check_mint_invariants(&sim(999, 900, 1_900), &harapan()).unwrap_err();
        assert!(e.iter().any(|m| m.contains("di bawah minimum")));
    }

    #[test]
    fn token_terpakai_melebihi_batas_ditolak() {
        // Ini yang mencegah transaksi menyedot lebih banyak dari yang kita
        // sediakan, meski kontraknya sendiri tidak keberatan.
        let e = check_mint_invariants(&sim(5_000, 1_001, 1_900), &harapan()).unwrap_err();
        assert!(e.iter().any(|m| m.contains("token0 terpakai")));

        let e = check_mint_invariants(&sim(5_000, 900, 2_001), &harapan()).unwrap_err();
        assert!(e.iter().any(|m| m.contains("token1 terpakai")));
    }

    #[test]
    fn setoran_kosong_ditolak() {
        let e = check_mint_invariants(&sim(5_000, 0, 0), &harapan()).unwrap_err();
        assert!(e
            .iter()
            .any(|m| m.contains("tidak ada token yang tersetor")));
    }

    #[test]
    fn semua_pelanggaran_dilaporkan_sekaligus() {
        let mut s = sim(0, 5_000, 5_000);
        s.token_id = U256::ZERO;
        let e = check_mint_invariants(&s, &harapan()).unwrap_err();
        // tokenId nol, likuiditas nol, token0 lebih, token1 lebih.
        assert_eq!(
            e.len(),
            4,
            "keempat pelanggaran harus muncul sekaligus, dapat {e:?}"
        );
    }

    #[test]
    fn revert_uniswap_diterjemahkan() {
        let stf = jelaskan_revert("execution reverted: STF");
        assert!(
            stf.contains("approval"),
            "pesan STF harus menyebut approval"
        );

        let slippage = jelaskan_revert("execution reverted: Price slippage check");
        assert!(slippage.contains("slippage"));
        assert!(slippage.contains("kutipan"));

        let deadline = jelaskan_revert("Transaction too old");
        assert!(deadline.contains("kedaluwarsa"));
    }

    #[test]
    fn revert_tak_dikenal_tetap_menyertakan_pesan_asli() {
        // Pesan asing tidak boleh hilang. User atau agent berikutnya butuh
        // teks mentahnya untuk menelusuri.
        let hasil = jelaskan_revert("0xdeadbeef sesuatu yang aneh");
        assert!(hasil.contains("0xdeadbeef sesuatu yang aneh"));
    }

    #[test]
    fn pesan_terjemahan_selalu_menyertakan_aslinya() {
        for mentah in ["STF", "LOK", "Price slippage check", "TLU"] {
            let hasil = jelaskan_revert(mentah);
            assert!(
                hasil.contains(mentah),
                "terjemahan `{hasil}` kehilangan pesan asli `{mentah}`"
            );
        }
    }
}
