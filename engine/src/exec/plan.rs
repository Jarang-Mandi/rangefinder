//! Pemilih jalur eksekusi menurut versi pool, beserta daftar izin yang seragam.
//!
//! KENAPA ADA LAPISAN INI
//! V3 dan V4 butuh bentuk transaksi yang sama sekali berbeda, dan izinnya juga
//! berbeda: V3 satu lapis allowance ERC20 ke NonfungiblePositionManager, V4 dua
//! lapis lewat Permit2. Panel tidak boleh tahu perbedaan itu, karena kalau ia
//! tahu, ia akan menyimpulkan sendiri, dan dua tempat yang menyimpulkan hal yang
//! sama pasti menyimpang.
//!
//! Jadi engine yang memutuskan jalur mana yang dipakai, dan mengembalikan
//! daftar langkah izin yang bentuknya seragam. Panel hanya menekan tombolnya
//! satu per satu sampai daftarnya kosong.
//!
//! INVARIAN
//! - Versi pool menentukan segalanya. Tidak ada jalur yang bisa dipakai untuk
//!   versi yang salah, dan gate G0 di kedua planner menegakkannya.
//! - Urutan langkah izin WAJIB dipertahankan. Pada V4, izin Permit2 tidak
//!   berguna sebelum token mengizinkan Permit2 lebih dulu.

use alloy::primitives::{Address, U256};
use alloy::providers::Provider;
use serde::Serialize;

use crate::config::{ExecutionConfig, LimitsConfig};
use crate::pools::PoolVersion;

use super::pipeline::{OpenPlan, OpenRequest};

/// Satu langkah izin yang masih harus dikerjakan.
///
/// Bentuknya seragam untuk V3 dan V4 supaya panel tidak perlu tahu protokolnya.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LangkahIzin {
    pub step_id: String,
    pub spender: String,
    /// Salah satu dari `erc20_ke_position_manager`, `erc20_ke_permit2`, atau
    /// `permit2_ke_position_manager`.
    pub jenis: &'static str,
    pub token: String,
    pub needed: String,
    pub current: String,
    /// Teks siap tampil, supaya panel tidak menyusun kalimat sendiri.
    pub label: String,
}

/// Alamat kontrak yang dibutuhkan kedua jalur.
#[derive(Debug, Clone, Copy)]
pub struct AlamatEksekusi {
    pub position_manager_v3: Address,
    pub position_manager_v4: Address,
    pub permit2: Address,
    pub multicall: Address,
}

impl AlamatEksekusi {
    pub fn dari_registry(registry: &crate::chain::registry::Registry) -> anyhow::Result<Self> {
        Ok(Self {
            position_manager_v3: registry.contract("uniswap_v3", "position_manager")?,
            position_manager_v4: registry.contract("uniswap_v4", "position_manager")?,
            permit2: registry.contract("common", "permit2")?,
            multicall: registry.contract("common", "multicall3")?,
        })
    }

    /// Kontrak tujuan untuk sebuah versi pool.
    pub fn position_manager(&self, versi: PoolVersion) -> Address {
        match versi {
            PoolVersion::V3 => self.position_manager_v3,
            PoolVersion::V4 => self.position_manager_v4,
        }
    }
}

/// Susun rencana dengan jalur yang sesuai versi poolnya.
pub async fn plan_open_auto<P: Provider>(
    provider: &P,
    alamat: &AlamatEksekusi,
    req: &OpenRequest,
    limits_cfg: &LimitsConfig,
    exec_cfg: &ExecutionConfig,
) -> OpenPlan {
    match req.pool.version {
        PoolVersion::V3 => {
            super::pipeline::plan_open(
                provider,
                alamat.position_manager_v3,
                req,
                limits_cfg,
                exec_cfg,
            )
            .await
        }
        PoolVersion::V4 => {
            super::v4::pipeline::plan_open_v4(
                provider,
                alamat.position_manager_v4,
                req,
                limits_cfg,
                exec_cfg,
            )
            .await
        }
    }
}

/// Daftar langkah izin yang masih kurang untuk sebuah rencana.
///
/// Kosong berarti posisi bisa langsung dibuka.
#[allow(clippy::too_many_arguments)]
pub async fn izin_yang_kurang<P: Provider>(
    provider: &P,
    alamat: &AlamatEksekusi,
    versi: PoolVersion,
    pemilik: Address,
    token0: Address,
    amount0: U256,
    token1: Address,
    amount1: U256,
    sekarang_unix: u64,
) -> Result<Vec<LangkahIzin>, String> {
    match versi {
        PoolVersion::V3 => {
            let izin = super::approve::check_allowances(
                provider,
                alamat.multicall,
                pemilik,
                alamat.position_manager_v3,
                token0,
                amount0,
                token1,
                amount1,
            )
            .await?;

            Ok(izin
                .into_iter()
                .filter(|i| i.insufficient)
                .map(|i| LangkahIzin {
                    step_id: String::new(),
                    spender: alamat.position_manager_v3.to_string(),
                    jenis: "erc20_ke_position_manager",
                    label: format!("Beri izin {} ke position manager", ringkas(&i.token)),
                    token: i.token,
                    needed: i.needed,
                    current: i.current,
                })
                .collect())
        }
        PoolVersion::V4 => {
            let izin = super::v4::permit::periksa_izin(
                provider,
                alamat.multicall,
                pemilik,
                alamat.permit2,
                alamat.position_manager_v4,
                token0,
                amount0,
                token1,
                amount1,
                sekarang_unix,
            )
            .await?;

            // Urutan dipertahankan apa adanya: izin Permit2 tidak berguna
            // sebelum token mengizinkan Permit2 lebih dulu.
            Ok(izin
                .into_iter()
                .flat_map(|i| i.langkah_kurang)
                .map(|l| LangkahIzin {
                    step_id: String::new(),
                    spender: if l.jenis == "erc20_ke_permit2" {
                        alamat.permit2
                    } else {
                        alamat.position_manager_v4
                    }
                    .to_string(),
                    jenis: l.jenis,
                    label: match l.jenis {
                        "erc20_ke_permit2" => {
                            format!("Langkah 1, izinkan Permit2 memakai {}", ringkas(&l.token))
                        }
                        _ => format!(
                            "Langkah 2, izinkan position manager lewat Permit2 untuk {}",
                            ringkas(&l.token)
                        ),
                    },
                    token: l.token,
                    needed: l.needed,
                    current: l.current,
                })
                .collect())
        }
    }
}

/// Langkah izin Permit2 untuk sebuah spender selain PositionManager.
///
/// Dipakai UniversalRouter untuk zap dan uji jual kecil. Router menarik token
/// lewat Permit2, jadi izin ke PositionManager tidak berlaku untuknya.
///
/// ETH asli dilewati karena ia ikut sebagai `msg.value`, bukan ditarik.
#[allow(clippy::too_many_arguments)]
pub async fn izin_permit2_untuk<P: Provider>(
    provider: &P,
    alamat: &AlamatEksekusi,
    pemilik: Address,
    spender: Address,
    nama_spender: &str,
    token: Address,
    jumlah: U256,
    sekarang_unix: u64,
) -> Result<Vec<LangkahIzin>, String> {
    if token.is_zero() || jumlah.is_zero() {
        return Ok(Vec::new());
    }

    let izin = super::v4::permit::periksa_izin(
        provider,
        alamat.multicall,
        pemilik,
        alamat.permit2,
        spender,
        token,
        jumlah,
        token,
        U256::ZERO,
        sekarang_unix,
    )
    .await?;

    Ok(izin
        .into_iter()
        .flat_map(|i| i.langkah_kurang)
        .map(|l| LangkahIzin {
            step_id: String::new(),
            spender: if l.jenis == "erc20_ke_permit2" {
                alamat.permit2
            } else {
                spender
            }
            .to_string(),
            jenis: l.jenis,
            label: match l.jenis {
                "erc20_ke_permit2" => {
                    format!(
                        "Izinkan Permit2 memakai {} untuk {nama_spender}",
                        ringkas(&l.token)
                    )
                }
                _ => format!("Izinkan {nama_spender} lewat Permit2"),
            },
            token: l.token,
            needed: l.needed,
            current: l.current,
        })
        .collect())
}

/// Langkah izin ERC20 langsung untuk spender selain PositionManager.
/// Dipakai simulasi jual kecil V3 melalui SwapRouter02.
pub async fn izin_erc20_untuk<P: Provider>(
    provider: &P,
    alamat: &AlamatEksekusi,
    pemilik: Address,
    spender: Address,
    nama_spender: &str,
    token: Address,
    jumlah: U256,
) -> Result<Vec<LangkahIzin>, String> {
    if token.is_zero() || jumlah.is_zero() {
        return Ok(Vec::new());
    }
    let mut izin = super::approve::check_allowances(
        provider,
        alamat.multicall,
        pemilik,
        spender,
        token,
        jumlah,
        token,
        U256::ZERO,
    )
    .await?;
    let i = izin.remove(0);
    Ok(i.insufficient
        .then(|| LangkahIzin {
            step_id: String::new(),
            spender: spender.to_string(),
            jenis: "erc20_ke_router",
            label: format!(
                "Izinkan {nama_spender} memakai {} untuk uji jual kecil",
                ringkas(&i.token)
            ),
            token: i.token,
            needed: i.needed,
            current: i.current,
        })
        .into_iter()
        .collect())
}

fn ringkas(alamat: &str) -> String {
    if alamat.len() <= 12 {
        return alamat.to_string();
    }
    format!("{}...{}", &alamat[..6], &alamat[alamat.len() - 4..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    fn alamat() -> AlamatEksekusi {
        AlamatEksekusi {
            position_manager_v3: address!("73991a25c818bf1f1128deaab1492d45638de0d3"),
            position_manager_v4: address!("58daec3116aae6d93017baaea7749052e8a04fa7"),
            permit2: address!("000000000022d473030f116ddee9f6b43ac78ba3"),
            multicall: address!("ca11bde05977b3631167028862be2a173976ca11"),
        }
    }

    #[test]
    fn versi_memilih_position_manager_yang_benar() {
        // Mengirim mint V4 ke NonfungiblePositionManager v3 akan revert, dan
        // mengirim mint V3 ke PositionManager V4 juga. Pemilihan ini satu
        // satunya tempat yang boleh memutuskannya.
        let a = alamat();
        assert_eq!(a.position_manager(PoolVersion::V3), a.position_manager_v3);
        assert_eq!(a.position_manager(PoolVersion::V4), a.position_manager_v4);
        assert_ne!(a.position_manager_v3, a.position_manager_v4);
    }

    #[test]
    fn ringkas_memendekkan_alamat_panjang() {
        assert_eq!(
            ringkas("0x5fc5360d0400a0fd4f2af552add042d716f1d168"),
            "0x5fc5...d168"
        );
    }

    #[test]
    fn ringkas_membiarkan_yang_sudah_pendek() {
        assert_eq!(ringkas("0xabc"), "0xabc");
    }
}
