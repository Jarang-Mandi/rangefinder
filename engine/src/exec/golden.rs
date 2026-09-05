//! Gate G3: golden vector dari transaksi sungguhan di chain.
//!
//! GAGASANNYA
//! Calldata yang kita bangun harus sama persis dengan yang benar benar
//! diterima kontrak di produksi. Cara membuktikannya bukan dengan membandingkan
//! terhadap harapan kita sendiri, karena itu berputar, melainkan terhadap
//! transaksi `mint` nyata yang sudah berhasil di rantai.
//!
//! ALURNYA
//! `cargo xtask golden` mencari transaksi `mint` sungguhan, mendekode
//! parameternya, lalu menyimpan parameter dan calldata aslinya sebagai berkas
//! di `engine/fixtures/golden/`. Ujinya kemudian berjalan offline: bangun ulang
//! calldata dari parameter itu, lalu bandingkan byte per byte.
//!
//! INVARIAN
//! - Pengambilan dan pengujian DIPISAH. Uji yang menyentuh jaringan akan gagal
//!   secara acak dan berhenti dipercaya. Uji golden vector wajib deterministik.
//! - Kalau perbandingan gagal, yang salah adalah builder kita, bukan
//!   fixture-nya. Jangan pernah memperbarui fixture supaya uji lewat.

use alloy::primitives::{Address, Bytes, U256};
use alloy::sol_types::SolCall;
use serde::{Deserialize, Serialize};

use super::abi_write::INonfungiblePositionManager as Npm;
use super::calldata::MintRequest;

/// Selector `mint(MintParams)`.
pub const MINT_SELECTOR: [u8; 4] = [0x88, 0x31, 0x64, 0x56];
/// Selector `multicall(bytes[])`.
pub const MULTICALL_SELECTOR: [u8; 4] = [0xac, 0x96, 0x50, 0xd8];

/// Satu golden vector, seperti yang tersimpan di berkas.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoldenVector {
    /// Dari mana vektor ini diambil, supaya bisa diperiksa ulang di explorer.
    pub source_tx: String,
    pub block_number: u64,
    pub captured_at: String,
    /// Calldata `mint` apa adanya, heksadesimal dengan awalan 0x.
    pub calldata: String,
    pub params: GoldenMintParams,
}

/// Parameter `mint` yang sudah didekode, dalam bentuk teks supaya berkasnya
/// bisa dibaca dan diperiksa manusia.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoldenMintParams {
    pub token0: String,
    pub token1: String,
    pub fee: u32,
    pub tick_lower: i32,
    pub tick_upper: i32,
    pub amount0_desired: String,
    pub amount1_desired: String,
    pub amount0_min: String,
    pub amount1_min: String,
    pub recipient: String,
    pub deadline: String,
}

impl GoldenMintParams {
    pub fn to_request(&self) -> Result<MintRequest, String> {
        let alamat = |s: &str| -> Result<Address, String> {
            s.parse().map_err(|_| format!("alamat `{s}` tidak valid"))
        };
        let angka = |s: &str| -> Result<U256, String> {
            s.parse().map_err(|_| format!("angka `{s}` tidak valid"))
        };

        Ok(MintRequest {
            token0: alamat(&self.token0)?,
            token1: alamat(&self.token1)?,
            fee: self.fee,
            tick_lower: self.tick_lower,
            tick_upper: self.tick_upper,
            amount0_desired: angka(&self.amount0_desired)?,
            amount1_desired: angka(&self.amount1_desired)?,
            amount0_min: angka(&self.amount0_min)?,
            amount1_min: angka(&self.amount1_min)?,
            recipient: alamat(&self.recipient)?,
            deadline: angka(&self.deadline)?,
        })
    }
}

/// Ambil calldata `mint` dari input transaksi.
///
/// Transaksi bisa memanggil `mint` langsung, atau membungkusnya dalam
/// `multicall`. Keduanya lazim, jadi keduanya ditangani.
pub fn extract_mint_calldata(input: &Bytes) -> Option<Bytes> {
    if input.len() < 4 {
        return None;
    }
    let selector: [u8; 4] = input[..4].try_into().ok()?;

    if selector == MINT_SELECTOR {
        return Some(input.clone());
    }

    if selector == MULTICALL_SELECTOR {
        let decoded = Npm::multicallCall::abi_decode(input).ok()?;
        return decoded.data.into_iter().find(|call| {
            call.len() >= 4 && call[..4] == MINT_SELECTOR
        });
    }

    None
}

/// Dekode calldata `mint` menjadi parameter yang bisa disimpan.
pub fn decode_mint(calldata: &Bytes) -> Result<GoldenMintParams, String> {
    let call = Npm::mintCall::abi_decode(calldata)
        .map_err(|e| format!("calldata bukan mint yang sah: {e}"))?;
    let p = call.params;

    Ok(GoldenMintParams {
        token0: p.token0.to_string(),
        token1: p.token1.to_string(),
        fee: p.fee.to::<u32>(),
        tick_lower: p.tickLower.as_i32(),
        tick_upper: p.tickUpper.as_i32(),
        amount0_desired: p.amount0Desired.to_string(),
        amount1_desired: p.amount1Desired.to_string(),
        amount0_min: p.amount0Min.to_string(),
        amount1_min: p.amount1Min.to_string(),
        recipient: p.recipient.to_string(),
        deadline: p.deadline.to_string(),
    })
}

/// Direktori tempat golden vector disimpan.
pub fn fixtures_dir(repo_root: &std::path::Path) -> std::path::PathBuf {
    repo_root.join("engine/fixtures/golden")
}

/// Muat semua golden vector yang tersimpan.
pub fn load_all(repo_root: &std::path::Path) -> Result<Vec<GoldenVector>, String> {
    let dir = fixtures_dir(repo_root);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("gagal membaca {dir:?}: {e}"))?;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("gagal membaca {path:?}: {e}"))?;
        let vector: GoldenVector = serde_json::from_str(&text)
            .map_err(|e| format!("{path:?} bukan golden vector yang sah: {e}"))?;
        out.push(vector);
    }

    out.sort_by(|a, b| a.source_tx.cmp(&b.source_tx));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::calldata;

    /// GATE G3.
    ///
    /// Untuk setiap transaksi `mint` sungguhan yang tersimpan, bangun ulang
    /// calldata dari parameternya lalu bandingkan byte per byte.
    ///
    /// Kalau uji ini gagal, JANGAN memperbarui fixture. Yang salah adalah
    /// builder kita, dan memperbarui fixture hanya menyembunyikan kesalahan
    /// yang akan muncul lagi saat dana bergerak.
    #[test]
    fn calldata_kita_sama_dengan_transaksi_sungguhan() {
        let root = crate::config::repo_root();
        let vectors = load_all(&root).expect("golden vector harus bisa dimuat");

        if vectors.is_empty() {
            // Belum ada yang diambil. Bukan kegagalan, tapi harus terlihat,
            // karena gate G3 tidak benar benar aktif sampai ada isinya.
            eprintln!(
                "PERINGATAN: belum ada golden vector di {:?}. \
                 Gate G3 belum aktif. Jalankan `cargo xtask golden` untuk mengambilnya.",
                fixtures_dir(&root)
            );
            return;
        }

        for v in &vectors {
            let req = v.params.to_request().expect("parameter harus bisa diurai");
            let asli = v
                .calldata
                .parse::<Bytes>()
                .expect("calldata di fixture harus hex yang sah");

            // Tick spacing nol mematikan pemeriksaan grid, karena transaksi
            // sungguhan sudah pasti pas di grid pool-nya, dan kita tidak
            // menyimpan fee tier pool di fixture.
            let kita = calldata::mint(&req, 0).expect("builder harus menerima parameter nyata");

            assert_eq!(
                alloy::hex::encode(&kita),
                alloy::hex::encode(&asli),
                "calldata berbeda dari transaksi {}",
                v.source_tx
            );
        }

        println!("gate G3: {} golden vector cocok byte per byte", vectors.len());
    }

    #[test]
    fn mengenali_mint_langsung() {
        let mut data = MINT_SELECTOR.to_vec();
        data.extend_from_slice(&[0u8; 32 * 11]);
        let bytes = Bytes::from(data);
        assert_eq!(extract_mint_calldata(&bytes), Some(bytes.clone()));
    }

    #[test]
    fn mengabaikan_selector_yang_bukan_mint() {
        let bytes = Bytes::from(vec![0x01, 0x02, 0x03, 0x04]);
        assert_eq!(extract_mint_calldata(&bytes), None);
        assert_eq!(extract_mint_calldata(&Bytes::from(vec![0x88])), None);
        assert_eq!(extract_mint_calldata(&Bytes::new()), None);
    }

    #[test]
    fn menemukan_mint_di_dalam_multicall() {
        // Bangun multicall yang berisi burn lalu mint, untuk memastikan
        // pencariannya tidak sekadar mengambil panggilan pertama.
        let mint_palsu = {
            let mut d = MINT_SELECTOR.to_vec();
            d.extend_from_slice(&[0u8; 32 * 11]);
            Bytes::from(d)
        };
        let bungkus = calldata::multicall(vec![
            calldata::burn(U256::from(1u8)),
            mint_palsu.clone(),
        ]);

        assert_eq!(extract_mint_calldata(&bungkus), Some(mint_palsu));
    }

    #[test]
    fn parameter_bolak_balik_lewat_teks() {
        let req = MintRequest {
            token0: "0x0Bd7D308f8E1639FAb988df18A8011f41EAcAD73".parse().unwrap(),
            token1: "0x39dbed3a2bd333467115de45665cc57f813c4571".parse().unwrap(),
            fee: 3_000,
            tick_lower: -60,
            tick_upper: 60,
            amount0_desired: U256::from(1_000u32),
            amount1_desired: U256::from(2_000u32),
            amount0_min: U256::from(900u32),
            amount1_min: U256::from(1_800u32),
            recipient: "0x48958E7e35b96fb58fCa9198436C3CB00d81b3Cb".parse().unwrap(),
            deadline: U256::from(1_788_700_000u64),
        };

        let data = calldata::mint(&req, 60).unwrap();
        let params = decode_mint(&data).unwrap();
        let kembali = params.to_request().unwrap();

        assert_eq!(kembali, req, "parameter harus utuh setelah bolak balik");

        // Dan calldata hasil bangun ulang harus identik.
        let lagi = calldata::mint(&kembali, 60).unwrap();
        assert_eq!(data, lagi);
    }
}
