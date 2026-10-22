//! Kebijakan yang mengikat persetujuan hook ke modal masuk dan bytecode aktual.
//! Penarikan/penutupan tetap tersedia ketika kebijakan modal masuk menolak.

use crate::store::Store;
use alloy::{primitives::Address, providers::Provider};
use anyhow::{bail, Context, Result};

pub async fn periksa<P: Provider>(
    provider: &P,
    store: &Store,
    hook: Option<Address>,
) -> Result<()> {
    let Some(address) = hook.filter(|a| !a.is_zero()) else {
        return Ok(());
    };
    let address_text = address.to_string();
    let approved = super::persetujuan::baca(store, &address_text)
        .context("hook belum disetujui; periksa penilaian hook sebelum memasukkan modal")?;
    let pinned = approved
        .penilaian
        .code_hash
        .as_deref()
        .context("persetujuan lama belum mengikat bytecode; nilai dan setujui ulang hook")?;
    if approved.otomatis {
        if !approved.penilaian.lolos_otomatis || approved.penilaian.terlarang {
            bail!("catatan allowlist otomatis tidak memenuhi seluruh kriteria");
        }
        let pada = chrono::DateTime::parse_from_rfc3339(&approved.pada)
            .context("waktu allowlist otomatis tidak valid")?
            .with_timezone(&chrono::Utc);
        if chrono::Utc::now().signed_duration_since(pada) > chrono::Duration::hours(24) {
            bail!("penilaian adopsi hook lebih tua dari 24 jam; pindai ulang");
        }
    } else if !approved.penilaian.boleh_disetujui_manual() {
        bail!("catatan persetujuan hook tidak memenuhi kebijakan");
    }
    let current = super::hook::code_hash(provider, address).await?;
    if !pinned.eq_ignore_ascii_case(&current) {
        bail!("bytecode hook berubah sejak persetujuan; susun penilaian baru");
    }
    if super::hook::periksa_upgrade(provider, address).await? {
        bail!("hook masih bisa diupgrade dan tidak boleh menerima modal");
    }
    // Pemanggil menahan execution_lock saat transaksi/ubah persetujuan.
    // Pembacaan terakhir ini juga melindungi quote yang tidak memegang lock itu.
    let latest = super::persetujuan::baca(store, &address_text)
        .context("persetujuan hook dicabut ketika pemeriksaan berjalan")?;
    if latest.penilaian.code_hash.as_deref() != Some(pinned) || latest.otomatis != approved.otomatis
    {
        bail!("persetujuan hook berubah; periksa ulang rencana");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::{
        primitives::{Bytes, U256},
        providers::ProviderBuilder,
        transports::mock::Asserter,
    };

    const CODE: &[u8] = &[0x60, 0x00];
    fn store(name: &str) -> Store {
        let dir =
            std::env::temp_dir().join(format!("rangefinder-policy-{name}-{}", std::process::id()));
        Store::open(&dir).unwrap()
    }
    fn approve(store: &Store, address: Address) {
        let mut rating =
            super::super::hook::susun(&address.to_string(), Some(7), Some(2.0), Some(false), true)
                .unwrap();
        rating.code_hash = Some(format!("{:#x}", alloy::primitives::keccak256(CODE)));
        super::super::persetujuan::setujui(store, &rating).unwrap();
    }
    fn approve_automatic(store: &Store, address: Address) {
        let mut rating = super::super::hook::susun_dengan_metrik(
            &address.to_string(),
            Some(100),
            Some(30.0),
            Some(false),
            true,
            super::super::hook::MetrikAdopsi {
                tvl_usd: Some(300_000.0),
                tvl_lengkap: true,
                lp_unik: Some(250),
                lp_lengkap: true,
            },
        )
        .unwrap();
        rating.code_hash = Some(format!("{:#x}", alloy::primitives::keccak256(CODE)));
        super::super::persetujuan::catat_otomatis(store, &rating).unwrap();
    }
    fn clean_rpc() -> Asserter {
        let rpc = Asserter::new();
        rpc.push_success(&Bytes::copy_from_slice(CODE));
        for _ in 0..4 {
            rpc.push_success(&U256::ZERO);
        }
        rpc
    }

    #[tokio::test]
    async fn tanpa_hook_tidak_memerlukan_rpc_atau_persetujuan() {
        let provider = ProviderBuilder::new().connect_mocked_client(Asserter::new());
        let store = store("zero");
        periksa(&provider, &store, None).await.unwrap();
        periksa(&provider, &store, Some(Address::ZERO))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn hook_asing_dan_persetujuan_dicabut_ditolak() {
        let address = Address::repeat_byte(1);
        let store = store("revoke");
        let provider = ProviderBuilder::new().connect_mocked_client(Asserter::new());
        assert!(periksa(&provider, &store, Some(address))
            .await
            .unwrap_err()
            .to_string()
            .contains("belum disetujui"));
        approve(&store, address);
        let provider = ProviderBuilder::new().connect_mocked_client(clean_rpc());
        periksa(&provider, &store, Some(address)).await.unwrap();
        super::super::persetujuan::cabut(&store, &address.to_string());
        assert!(periksa(&provider, &store, Some(address))
            .await
            .unwrap_err()
            .to_string()
            .contains("belum disetujui"));
    }

    #[tokio::test]
    async fn allowlist_otomatis_segar_lolos_dengan_bytecode_dan_slot_bersih() {
        let address = Address::repeat_byte(8);
        let store = store("auto");
        approve_automatic(&store, address);
        let provider = ProviderBuilder::new().connect_mocked_client(clean_rpc());
        periksa(&provider, &store, Some(address)).await.unwrap();
    }

    #[tokio::test]
    async fn allowlist_otomatis_kedaluwarsa_ditolak_sebelum_rpc() {
        let address = Address::repeat_byte(9);
        let store = store("auto-stale");
        approve_automatic(&store, address);
        let mut record = super::super::persetujuan::baca(&store, &address.to_string()).unwrap();
        record.pada = (chrono::Utc::now() - chrono::Duration::hours(25)).to_rfc3339();
        store
            .put(
                &format!("risk/hook-disetujui/{}", address.to_string().to_lowercase()),
                &record,
            )
            .unwrap();
        let provider = ProviderBuilder::new().connect_mocked_client(Asserter::new());
        assert!(periksa(&provider, &store, Some(address))
            .await
            .unwrap_err()
            .to_string()
            .contains("lebih tua dari 24 jam"));
    }

    #[tokio::test]
    async fn persetujuan_tidak_meloloskan_bytecode_yang_berubah() {
        let address = Address::repeat_byte(2);
        let store = store("code");
        approve(&store, address);
        let rpc = Asserter::new();
        rpc.push_success(&Bytes::from_static(&[0x60, 0x01]));
        let provider = ProviderBuilder::new().connect_mocked_client(rpc);
        assert!(periksa(&provider, &store, Some(address))
            .await
            .unwrap_err()
            .to_string()
            .contains("bytecode hook berubah"));
    }

    #[tokio::test]
    async fn slot_proxy_berubah_dan_rpc_gagal_tidak_meloloskan_modal() {
        let address = Address::repeat_byte(3);
        let store = store("proxy");
        approve(&store, address);
        let rpc = Asserter::new();
        rpc.push_success(&Bytes::copy_from_slice(CODE));
        rpc.push_success(&U256::from(1));
        let provider = ProviderBuilder::new().connect_mocked_client(rpc);
        assert!(periksa(&provider, &store, Some(address))
            .await
            .unwrap_err()
            .to_string()
            .contains("diupgrade"));

        let rpc = Asserter::new();
        rpc.push_success(&Bytes::copy_from_slice(CODE));
        rpc.push_failure_msg("RPC offline");
        let provider = ProviderBuilder::new().connect_mocked_client(rpc);
        assert!(periksa(&provider, &store, Some(address)).await.is_err());
    }

    #[tokio::test]
    async fn alamat_tanpa_bytecode_tidak_bisa_menerima_persetujuan() {
        let address = Address::repeat_byte(4);
        let store = store("empty");
        approve(&store, address);
        let rpc = Asserter::new();
        rpc.push_success(&Bytes::new());
        let provider = ProviderBuilder::new().connect_mocked_client(rpc);
        assert!(periksa(&provider, &store, Some(address))
            .await
            .unwrap_err()
            .to_string()
            .contains("tidak mempunyai bytecode"));
        let rating = super::super::hook::susun(
            &address.to_string(),
            Some(100),
            Some(90.0),
            Some(false),
            true,
        )
        .unwrap();
        assert!(super::super::persetujuan::setujui(&store, &rating).is_err());
    }
}
