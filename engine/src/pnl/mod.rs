//! Akuntansi hasil posisi LP.
//!
//! KENAPA FEE KOTOR MENIPU
//! Posisi yang mengumpulkan fee besar bisa tetap rugi. Impermanent loss, gas
//! buka, gas tutup, dan slippage zap semuanya mengurangi, dan tidak satu pun
//! terlihat kalau yang ditampilkan hanya fee. PRD 11 menyebutnya eksplisit:
//! kesalahan itu akan terulang karena tidak pernah terlihat.
//!
//! Karena itu modul ini selalu memisahkan komponennya, dan `net` selalu
//! dihitung dari selisihnya, tidak pernah disimpan terpisah.
//!
//! SATUAN
//! Seluruh perhitungan dilakukan dalam satuan token kutipan, yaitu token1 pada
//! pool yang kutipannya stablecoin. Alasannya, itu satuan yang paling stabil
//! yang tersedia tanpa sumber harga luar. Nilai token0 dinyatakan dalam token1
//! lewat harga pool.
//!
//! INVARIAN
//! - Impermanent loss dihitung terhadap MEMEGANG, bukan terhadap modal awal.
//!   Selisih terhadap modal awal adalah perubahan harga, bukan kerugian akibat
//!   menjadi LP, dan mencampurnya membuat strategi yang bagus terlihat buruk
//!   saat pasar turun.
//! - Catatan yang tidak lengkap ditandai sebagai perkiraan, bukan disembunyikan.
//!   Posisi yang dibuka di luar Rangefinder tetap harus punya angka.

use serde::{Deserialize, Serialize};

use alloy::primitives::{Address, U256};

/// Catatan biaya sebuah posisi, disimpan saat posisi dibuka.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Catatan {
    pub position_id: String,
    pub token_id: String,
    /// Mengikat approval, zap, dan open dari satu niat user.
    #[serde(default)]
    pub flow_id: Option<String>,
    /// Jumlah token0 dan token1 yang benar benar disetor.
    pub setor0: String,
    pub setor1: String,
    /// Harga token1 per token0 saat posisi dibuka, sebagai pecahan.
    pub harga_buka: Option<f64>,
    /// Benar setelah seluruh biaya, fee, dan perubahan modal tercatat.
    #[serde(default)]
    pub lengkap: bool,
    #[serde(default)]
    pub setoran_aktual: bool,
    #[serde(default)]
    pub gas_wei: Option<String>,
    /// Biaya gas yang sudah dibayar, dalam satuan token kutipan.
    pub gas_kutipan: Option<f64>,
    /// Kerugian slippage zap, dalam satuan token kutipan.
    pub slippage_kutipan: Option<f64>,
    pub dibuka_pada: String,
    pub tx_buka: Option<String>,
    /// Ringkasan event posisi V3 yang dibuka di luar Rangefinder.
    ///
    /// `Collect` memuat fee dan principal hasil decrease dalam satu angka.
    /// Menyimpan keduanya memungkinkan fee seumur posisi dihitung tanpa
    /// menebak bagian mana yang dipungut lebih dulu.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rekonstruksi_log: Option<RekonstruksiLog>,
    /// Basis V4 dari batas jumlah pada calldata PositionManager.
    ///
    /// V4 tidak memancarkan jumlah token aktual pada ModifyLiquidity. Karena
    /// itu basis ini berguna sebagai perkiraan, tetapi tidak boleh diberi label
    /// setoran aktual atau fee seumur posisi yang lengkap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rekonstruksi_v4: Option<RekonstruksiV4>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RekonstruksiLog {
    pub collected0: String,
    pub collected1: String,
    pub decreased0: String,
    pub decreased1: String,
    pub sampai_blok: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RekonstruksiV4 {
    pub sampai_blok: u64,
}

/// Fee seumur posisi dari event V3 dan hasil collect yang bisa diambil sekarang.
///
/// Identitasnya adalah `seluruh collect + collect sekarang - seluruh decrease`.
/// Ini tetap benar ketika principal hasil decrease belum dipungut, karena
/// principal itu masih berada di hasil collect sekarang dan dibatalkan oleh
/// suku decrease.
pub(crate) fn total_fee_dari_log(
    current0: U256,
    current1: U256,
    log: &RekonstruksiLog,
) -> Option<(U256, U256)> {
    let collected0 = log.collected0.parse::<U256>().ok()?;
    let collected1 = log.collected1.parse::<U256>().ok()?;
    let decreased0 = log.decreased0.parse::<U256>().ok()?;
    let decreased1 = log.decreased1.parse::<U256>().ok()?;
    Some((
        current0.checked_add(collected0)?.checked_sub(decreased0)?,
        current1.checked_add(collected1)?.checked_sub(decreased1)?,
    ))
}

/// Komponen hasil sebuah posisi, semuanya dalam satuan token kutipan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Currency {
    pub id: String,
    pub symbol: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hasil {
    pub currency: Currency,
    /// Fee yang diperoleh, yang sudah diklaim maupun belum.
    pub fee: Option<f64>,
    /// Nilai setoran awal dalam token kutipan.
    pub deposited: Option<f64>,
    /// Nilai principal sekarang, atau pokok yang diterima saat ditutup.
    pub value: Option<f64>,
    /// Hasil bersih dibanding nilai setoran, dalam persen.
    pub pnl_percent: Option<f64>,
    /// Waktu posisi pertama kali dibuka.
    pub opened_at: Option<String>,
    /// Kerugian akibat menjadi LP dibandingkan sekadar memegang.
    ///
    /// Positif berarti rugi. Ditampilkan sebagai pengurang, bukan sebagai nilai
    /// negatif, supaya kartunya terbaca sebagai daftar biaya.
    pub impermanent_loss: f64,
    pub gas: Option<f64>,
    pub slippage: Option<f64>,
    /// Fee dikurangi seluruh biaya. Selalu dihitung, tidak pernah disimpan.
    pub net: Option<f64>,
    /// Benar kalau sebagian angkanya direkonstruksi, bukan dari catatan.
    pub perkiraan: bool,
}

/// Hasil posisi yang sudah ditutup dan tidak lagi muncul dari enumerasi NFT.
#[derive(Debug, Clone, Serialize)]
pub struct PosisiTertutup {
    pub position_id: String,
    pub token0: String,
    pub token1: String,
    pub symbol0: Option<String>,
    pub symbol1: Option<String>,
    pub tx_hash: String,
    pub ditutup_pada: String,
    pub diterima0: String,
    pub diterima1: String,
    pub bukti_penerimaan: String,
    pub pnl: Option<Hasil>,
    /// Alasan angka hasil belum dapat disusun. Tidak memuat detail transport.
    pub error: Option<String>,
}

/// Nilai sekumpulan token dalam satuan token kutipan.
///
/// `harga` adalah token1 per token0.
pub fn nilai(amount0: f64, amount1: f64, harga: f64) -> f64 {
    amount0 * harga + amount1
}

/// Hitung impermanent loss terhadap memegang.
///
/// Keduanya dinilai pada harga SEKARANG. Itu yang membuatnya benar benar
/// mengukur akibat menjadi LP: kalau harganya sama sekali tidak bergerak,
/// hasilnya nol berapa pun harganya.
pub fn impermanent_loss(
    setor0: f64,
    setor1: f64,
    posisi0: f64,
    posisi1: f64,
    harga_sekarang: f64,
) -> f64 {
    let memegang = nilai(setor0, setor1, harga_sekarang);
    let sebagai_lp = nilai(posisi0, posisi1, harga_sekarang);
    memegang - sebagai_lp
}

/// Rakit seluruh komponen menjadi satu hasil.
#[allow(clippy::too_many_arguments)]
pub fn hitung(
    setor0: f64,
    setor1: f64,
    posisi0: f64,
    posisi1: f64,
    fee0: f64,
    fee1: f64,
    harga_sekarang: f64,
    gas: f64,
    slippage: f64,
    perkiraan: bool,
) -> Hasil {
    let fee = nilai(fee0, fee1, harga_sekarang);
    let il = impermanent_loss(setor0, setor1, posisi0, posisi1, harga_sekarang);
    let deposited = nilai(setor0, setor1, harga_sekarang);
    let value = nilai(posisi0, posisi1, harga_sekarang);
    let net = fee - il - gas - slippage;
    Hasil {
        currency: Currency {
            id: "unit".into(),
            symbol: "unit".into(),
        },
        fee: Some(fee),
        deposited: Some(deposited),
        value: Some(value),
        pnl_percent: (deposited > 0.0).then_some(net / deposited * 100.0),
        opened_at: None,
        impermanent_loss: il,
        gas: Some(gas),
        slippage: Some(slippage),
        net: Some(net),
        perkiraan,
    }
}

/// Ringkasan sekumpulan posisi.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Ringkasan {
    pub jumlah: usize,
    /// Total lintas posisi hanya tersedia jika satuan seluruhnya sama.
    pub total_net: Option<f64>,
    pub total_fee: Option<f64>,
    pub total_biaya: Option<f64>,
    pub currencies: Vec<RingkasanCurrency>,
    /// Porsi posisi yang net-nya positif, dalam basis poin.
    pub win_rate_bps: Option<u32>,
    /// Berapa yang sebagian angkanya direkonstruksi.
    pub perkiraan: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RingkasanCurrency {
    pub currency: Currency,
    pub jumlah: usize,
    pub total_net: Option<f64>,
    pub total_fee: Option<f64>,
    pub total_biaya: Option<f64>,
}

pub fn ringkas(hasil: &[Hasil]) -> Ringkasan {
    if hasil.is_empty() {
        return Ringkasan {
            jumlah: 0,
            total_net: None,
            total_fee: None,
            total_biaya: None,
            currencies: Vec::new(),
            win_rate_bps: None,
            perkiraan: 0,
        };
    }

    let mut groups = std::collections::BTreeMap::<String, RingkasanCurrency>::new();
    for h in hasil {
        let group = groups
            .entry(h.currency.id.clone())
            .or_insert_with(|| RingkasanCurrency {
                currency: h.currency.clone(),
                jumlah: 0,
                total_net: Some(0.0),
                total_fee: Some(0.0),
                total_biaya: Some(0.0),
            });
        group.jumlah += 1;
        group.total_net = group.total_net.zip(h.net).map(|(a, b)| a + b);
        group.total_fee = group.total_fee.zip(h.fee).map(|(a, b)| a + b);
        group.total_biaya = group
            .total_biaya
            .zip(h.gas)
            .zip(h.slippage)
            .map(|((total, gas), slippage)| total + h.impermanent_loss + gas + slippage);
        if h.perkiraan {
            group.total_biaya = None;
        }
    }
    let currencies: Vec<_> = groups.into_values().collect();
    let satu = (currencies.len() == 1).then(|| &currencies[0]);
    let menang = hasil
        .iter()
        .filter(|h| h.net.is_some_and(|n| n > 0.0))
        .count();

    Ringkasan {
        jumlah: hasil.len(),
        total_net: satu.and_then(|c| c.total_net),
        total_fee: satu.and_then(|c| c.total_fee),
        total_biaya: satu.and_then(|c| c.total_biaya),
        currencies,
        win_rate_bps: hasil
            .iter()
            .all(|h| h.net.is_some())
            .then_some((menang * 10_000 / hasil.len()) as u32),
        perkiraan: hasil.iter().filter(|h| h.perkiraan).count(),
    }
}

fn kunci_catatan(position_id: &str) -> String {
    format!("pnl:v2:{}", position_id.to_lowercase())
}

/// Simpan catatan biaya sebuah posisi.
pub fn simpan(store: &crate::store::Store, c: &Catatan) {
    if let Err(e) = store.put(&kunci_catatan(&c.position_id), c) {
        // Catatan yang gagal disimpan hanya membuat PnL-nya jadi perkiraan
        // nanti. Itu tidak sepadan dengan menggagalkan pembukaan posisi.
        tracing::warn!(token_id = %c.token_id, error = %e, "catatan biaya gagal disimpan");
    }
}

/// Tambahkan setoran aktual ke basis posisi yang sudah dicatat Rangefinder.
///
/// Posisi eksternal tidak mempunyai basis awal yang dapat dibuktikan, sehingga
/// fungsi ini membiarkannya tetap tanpa catatan sampai rekonstruksi log selesai.
pub fn tambah_setoran(
    store: &crate::store::Store,
    position_id: &str,
    amount0: U256,
    amount1: U256,
    aktual: bool,
) -> anyhow::Result<bool> {
    let key = kunci_catatan(position_id);
    let Some(existing): Option<Catatan> = store.get(&key)? else {
        return Ok(false);
    };
    store.update(&key, |old: Option<Catatan>| {
        let mut old = old.ok_or_else(|| anyhow::anyhow!("catatan posisi hilang"))?;
        let setor0 = old
            .setor0
            .parse::<U256>()?
            .checked_add(amount0)
            .ok_or_else(|| anyhow::anyhow!("basis token0 melampaui uint256"))?;
        let setor1 = old
            .setor1
            .parse::<U256>()?
            .checked_add(amount1)
            .ok_or_else(|| anyhow::anyhow!("basis token1 melampaui uint256"))?;
        old.setor0 = setor0.to_string();
        old.setor1 = setor1.to_string();
        old.setoran_aktual &= aktual;
        old.lengkap = false;
        Ok(old)
    })?;
    Ok(existing.position_id.eq_ignore_ascii_case(position_id))
}

/// Ambil catatan biaya kalau ada.
pub fn ambil(store: &crate::store::Store, token_id: &str) -> Option<Catatan> {
    let mut catatan: Catatan = store.get(&kunci_catatan(token_id)).ok().flatten()?;
    // Versi lama mengisi biaya nol tanpa bukti. Nol itu bukan biaya aktual.
    if catatan.gas_wei.is_none() && !catatan.lengkap {
        catatan.gas_kutipan = None;
        catatan.slippage_kutipan = None;
    }
    Some(catatan)
}

/// Susun histori posisi tertutup dari jurnal yang persisten.
///
/// NFT yang dibakar tidak lagi dapat ditemukan lewat enumerasi posisi. Karena
/// itu sumber riwayatnya adalah transaksi close yang sukses, jumlah aktual dari
/// event atau perubahan saldo, dan dasar harga yang disimpan sebelum broadcast.
pub fn histori_tertutup(
    store: &crate::store::Store,
    chain: u64,
    owner: Address,
) -> anyhow::Result<Vec<PosisiTertutup>> {
    let mut closes: Vec<_> = crate::exec::journal::entries(store, chain, owner)?
        .into_iter()
        .filter(|entry| {
            entry.context.operation == crate::exec::journal::Operation::Close
                && entry.status == crate::exec::journal::Status::Succeeded
                && entry.context.position_id.is_some()
        })
        .collect();
    closes.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for entry in closes {
        let position_id = entry.context.position_id.clone().unwrap_or_default();
        if !seen.insert(position_id.clone()) {
            continue;
        }
        let Some(received) = entry.received.clone() else {
            out.push(PosisiTertutup {
                position_id,
                token0: entry
                    .context
                    .token0
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
                token1: entry
                    .context
                    .token1
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
                symbol0: entry
                    .context
                    .close_basis
                    .as_ref()
                    .and_then(|b| b.symbol0.clone()),
                symbol1: entry
                    .context
                    .close_basis
                    .as_ref()
                    .and_then(|b| b.symbol1.clone()),
                tx_hash: format!("{:#x}", entry.tx_hash),
                ditutup_pada: entry.created_at,
                diterima0: String::new(),
                diterima1: String::new(),
                bukti_penerimaan: String::new(),
                pnl: None,
                error: Some("jumlah yang diterima saat penutupan belum terbukti".into()),
            });
            continue;
        };

        let result = hasil_tertutup(store, chain, owner, &entry, &received);
        let (pnl, error) = match result {
            Ok(pnl) => (Some(pnl), None),
            Err(error) => (None, Some(error)),
        };
        out.push(PosisiTertutup {
            position_id,
            token0: entry
                .context
                .token0
                .map(|v| v.to_string())
                .unwrap_or_default(),
            token1: entry
                .context
                .token1
                .map(|v| v.to_string())
                .unwrap_or_default(),
            symbol0: entry
                .context
                .close_basis
                .as_ref()
                .and_then(|b| b.symbol0.clone()),
            symbol1: entry
                .context
                .close_basis
                .as_ref()
                .and_then(|b| b.symbol1.clone()),
            tx_hash: format!("{:#x}", entry.tx_hash),
            ditutup_pada: entry.created_at,
            diterima0: received.amount0,
            diterima1: received.amount1,
            bukti_penerimaan: received.evidence,
            pnl,
            error,
        });
    }
    Ok(out)
}

fn hasil_tertutup(
    store: &crate::store::Store,
    chain: u64,
    owner: Address,
    close: &crate::exec::journal::Entry,
    received: &crate::exec::journal::ReceivedAmounts,
) -> Result<Hasil, String> {
    let position_id = close
        .context
        .position_id
        .as_deref()
        .ok_or("identitas posisi pada transaksi close hilang")?;
    let basis = close
        .context
        .close_basis
        .as_ref()
        .ok_or("dasar harga penutupan belum tercatat")?;
    let catatan = ambil(store, position_id).ok_or("catatan setoran posisi tidak tersedia")?;
    let ledger = crate::exec::journal::position_ledger(store, chain, owner, position_id)
        .map_err(|_| "ledger aksi posisi tidak terbaca")?;

    let sqrt = basis
        .sqrt_price_x96
        .parse::<U256>()
        .map_err(|_| "harga penutupan tidak valid")?;
    let harga = crate::pools::raw_price_from_sqrt_x96(sqrt);
    if !(harga.is_finite() && harga > 0.0) {
        return Err("harga penutupan tidak valid".into());
    }

    let parse = |value: &str, nama: &str| {
        value
            .parse::<U256>()
            .map_err(|_| format!("{nama} pada histori tidak valid"))
    };
    let received0 = parse(&received.amount0, "amount0 diterima")?;
    let received1 = parse(&received.amount1, "amount1 diterima")?;
    let fee0 = parse(
        basis
            .fee0
            .as_deref()
            .ok_or("fee0 saat penutupan belum diketahui")?,
        "fee0",
    )?;
    let fee1 = parse(
        basis
            .fee1
            .as_deref()
            .ok_or("fee1 saat penutupan belum diketahui")?,
        "fee1",
    )?;
    let principal0 = received0
        .checked_sub(fee0)
        .ok_or("amount0 penutupan lebih kecil daripada fee0")?;
    let principal1 = received1
        .checked_sub(fee1)
        .ok_or("amount1 penutupan lebih kecil daripada fee1")?;
    let total_fee = if catatan.rekonstruksi_v4.is_some() {
        // ModifyLiquidity V4 tidak membawa jumlah token dan tidak membuktikan
        // fee yang pernah diambil sebelum Rangefinder melihat posisi ini.
        None
    } else if let Some(log) = catatan.rekonstruksi_log.as_ref() {
        let (historical0, historical1) = total_fee_dari_log(fee0, fee1, log)
            .ok_or("total fee dari rekonstruksi log tidak konsisten")?;
        Some((
            historical0
                .checked_add(ledger.claimed0)
                .ok_or("total fee0 melampaui uint256")?,
            historical1
                .checked_add(ledger.claimed1)
                .ok_or("total fee1 melampaui uint256")?,
        ))
    } else {
        Some((
            fee0.checked_add(ledger.claimed0)
                .ok_or("total fee0 melampaui uint256")?,
            fee1.checked_add(ledger.claimed1)
                .ok_or("total fee1 melampaui uint256")?,
        ))
    };

    let number = |value: U256| {
        value
            .to_string()
            .parse::<f64>()
            .map_err(|_| "jumlah token terlalu besar untuk tampilan".to_string())
    };
    let scale = 10f64.powi(i32::from(basis.quote_decimals));
    let to_quote_at = |amount0: U256, amount1: U256, raw_price: f64| -> Result<f64, String> {
        let amount0 = number(amount0)?;
        let amount1 = number(amount1)?;
        Ok(if basis.quote_token0 {
            (amount0 + amount1 / raw_price) / scale
        } else {
            (amount0 * raw_price + amount1) / scale
        })
    };
    let to_quote = |amount0: U256, amount1: U256| to_quote_at(amount0, amount1, harga);

    let deposit0 = parse(&catatan.setor0, "setor0")?;
    let deposit1 = parse(&catatan.setor1, "setor1")?;
    let il = to_quote(deposit0, deposit1)? - to_quote(principal0, principal1)?;
    let fee = total_fee
        .filter(|_| ledger.complete)
        .map(|(fee0, fee1)| to_quote(fee0, fee1))
        .transpose()?;

    let flow = catatan
        .flow_id
        .as_deref()
        .map(|flow_id| crate::exec::journal::flow_costs(store, chain, owner, flow_id))
        .transpose()
        .map_err(|_| "biaya transaksi pembukaan tidak terbaca")?;
    let gas = match flow.as_ref() {
        Some(flow)
            if flow.complete
                && ledger.complete
                && (basis.currency_symbol == "ETH" || basis.currency_symbol == "WETH") =>
        {
            flow.gas_wei
                .checked_add(ledger.action_gas_wei)
                .and_then(|value| value.to_string().parse::<f64>().ok())
                .map(|wei| wei / 1e18)
        }
        _ => catatan.gas_kutipan.filter(|_| ledger.complete),
    };
    let slippage = catatan.slippage_kutipan;
    let complete = close.confirmed_at.is_some()
        && catatan.setoran_aktual
        && ledger.complete
        && fee.is_some()
        && gas.is_some()
        && slippage.is_some();
    let net = fee
        .zip(gas)
        .zip(slippage)
        .map(|((fee, gas), slippage)| fee - il - gas - slippage);

    let harga_setor = catatan
        .harga_buka
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(harga);
    let deposited = to_quote_at(deposit0, deposit1, harga_setor)?;
    // Nilai posisi yang dikembalikan dipisahkan dari fee agar kartu PnL tidak
    // menghitung hasil fee dua kali.
    let value = to_quote(principal0, principal1)?;
    Ok(Hasil {
        currency: Currency {
            id: basis.currency_id.clone(),
            symbol: basis.currency_symbol.clone(),
        },
        fee,
        deposited: Some(deposited),
        value: Some(value),
        pnl_percent: net
            .filter(|_| deposited > 0.0)
            .map(|net| net / deposited * 100.0),
        opened_at: Some(catatan.dibuka_pada.clone()),
        impermanent_loss: il,
        gas,
        slippage,
        net,
        perkiraan: !complete,
    })
}

/// Isi simbol histori lama yang belum sempat disimpan saat posisi ditutup.
///
/// Versi jurnal awal hanya membawa alamat token. Membiarkan simbol kosong
/// membuat kartu histori menampilkan dua potongan alamat meski metadata masih
/// bisa dibaca dari rantai. Angka PnL tidak diubah oleh fungsi ini.
pub async fn lengkapi_simbol_tertutup<P: alloy::providers::Provider>(
    provider: &P,
    registry: &crate::chain::registry::Registry,
    positions: &mut [PosisiTertutup],
) -> Result<(), String> {
    let mut alamat = Vec::<Address>::new();
    for posisi in positions.iter() {
        for (token, symbol) in [
            (&posisi.token0, &posisi.symbol0),
            (&posisi.token1, &posisi.symbol1),
        ] {
            if symbol.is_some() {
                continue;
            }
            let parsed = token
                .parse::<Address>()
                .map_err(|_| format!("alamat token histori tidak valid: {token}"))?;
            if !alamat.contains(&parsed) {
                alamat.push(parsed);
            }
        }
    }
    if alamat.is_empty() {
        return Ok(());
    }

    let meta = crate::pools::token_meta_batch(provider, registry, &alamat)
        .await
        .map_err(|e| format!("metadata token histori tidak terbaca: {e}"))?;
    let simbol: std::collections::HashMap<Address, String> = alamat
        .into_iter()
        .zip(meta)
        .filter_map(|(address, meta)| meta.map(|meta| (address, meta.symbol)))
        .collect();

    for posisi in positions {
        if posisi.symbol0.is_none() {
            posisi.symbol0 = posisi
                .token0
                .parse::<Address>()
                .ok()
                .and_then(|a| simbol.get(&a).cloned());
        }
        if posisi.symbol1.is_none() {
            posisi.symbol1 = posisi
                .token1
                .parse::<Address>()
                .ok()
                .and_then(|a| simbol.get(&a).cloned());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn biaya_hilang_tidak_dianggap_nol_atau_kemenangan() {
        let known = hitung(0.0, 0.0, 0.0, 0.0, 0.0, 10.0, 1.0, 1.0, 0.0, false);
        let mut unknown = known.clone();
        unknown.gas = None;
        unknown.net = None;
        unknown.perkiraan = true;
        let summary = ringkas(&[known, unknown]);
        assert_eq!(summary.total_fee, Some(20.0));
        assert_eq!(summary.total_net, None);
        assert_eq!(summary.total_biaya, None);
        assert_eq!(summary.win_rate_bps, None);
        assert_eq!(summary.currencies[0].total_net, None);
    }

    #[test]
    fn catatan_lama_tidak_tiba_tiba_dianggap_lengkap() {
        let legacy = serde_json::json!({ "position_id": "4663:manager:7", "token_id": "7",
            "setor0": "1", "setor1": "2", "harga_buka": 0.0,
            "gas_kutipan": 0.0, "slippage_kutipan": 0.0,
            "dibuka_pada": "2026-09-09T00:00:00Z", "tx_buka": null });
        let catatan: Catatan = serde_json::from_value(legacy).unwrap();
        assert!(!catatan.lengkap);
        assert!(!catatan.setoran_aktual);
        assert_eq!(catatan.gas_wei, None);
    }

    fn hampir(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn nilai_menjumlahkan_kedua_sisi() {
        // Dua token0 pada harga tiga, ditambah satu token1, sama dengan tujuh.
        assert!(hampir(nilai(2.0, 1.0, 3.0), 7.0));
    }

    #[test]
    fn harga_tidak_bergerak_berarti_tanpa_impermanent_loss() {
        // INI DEFINISI YANG BENAR.
        //
        // Impermanent loss mengukur akibat menjadi LP, bukan akibat harga
        // bergerak. Kalau posisinya masih memegang persis yang disetor, hasilnya
        // nol berapa pun harganya.
        for harga in [0.5, 1.0, 2.0, 100.0] {
            assert!(hampir(impermanent_loss(10.0, 20.0, 10.0, 20.0, harga), 0.0));
        }
    }

    #[test]
    fn komposisi_bergeser_menimbulkan_kerugian() {
        // Harga token0 naik, jadi posisi menjual sebagian token0. Dibandingkan
        // memegang, nilainya lebih rendah.
        //
        // Setor 10 token0 dan 0 token1. Harga naik jadi 2. Posisi sekarang
        // memegang 5 token0 dan 10 token1, yang nilainya 5*2+10 = 20.
        // Memegang saja bernilai 10*2 = 20. Sama, jadi belum ada kerugian.
        assert!(hampir(impermanent_loss(10.0, 0.0, 5.0, 10.0, 2.0), 0.0));

        // Kalau posisi hanya memegang 5 token0 dan 8 token1, nilainya 18,
        // sedangkan memegang bernilai 20. Kerugiannya 2.
        assert!(hampir(impermanent_loss(10.0, 0.0, 5.0, 8.0, 2.0), 2.0));
    }

    #[test]
    fn net_adalah_fee_dikurangi_seluruh_biaya() {
        let h = hitung(10.0, 0.0, 5.0, 8.0, 0.0, 5.0, 2.0, 0.5, 0.3, false);
        assert!(hampir(h.fee.unwrap(), 5.0));
        assert!(hampir(h.impermanent_loss, 2.0));
        assert!(hampir(h.net.unwrap(), 5.0 - 2.0 - 0.5 - 0.3));
        assert!(hampir(h.deposited.unwrap(), 20.0));
        assert!(hampir(h.value.unwrap(), 18.0));
        assert!(hampir(h.pnl_percent.unwrap(), 11.0));
    }

    #[test]
    fn fee_besar_tetap_bisa_rugi() {
        // Ini kasus yang paling penting ditampilkan. Fee lima terlihat bagus,
        // tapi impermanent loss enam membuatnya rugi. Tanpa pemisahan komponen,
        // user hanya melihat angka fee dan mengira untung.
        let h = hitung(10.0, 0.0, 4.0, 6.0, 0.0, 5.0, 2.0, 0.2, 0.0, false);
        assert!(h.fee.unwrap() > 0.0);
        assert!(h.net.unwrap() < 0.0, "net {:?} seharusnya negatif", h.net);
    }

    #[test]
    fn fee_dinilai_pada_harga_sekarang() {
        // Fee yang terkumpul dalam token0 harus dikonversi, kalau tidak posisi
        // yang fee-nya di sisi token0 akan terlihat jauh lebih kecil.
        let h = hitung(0.0, 0.0, 0.0, 0.0, 3.0, 0.0, 4.0, 0.0, 0.0, false);
        assert!(hampir(h.fee.unwrap(), 12.0));
    }

    #[test]
    fn ringkasan_kosong_tidak_membagi_dengan_nol() {
        let r = ringkas(&[]);
        assert_eq!(r.jumlah, 0);
        assert_eq!(r.win_rate_bps, None);
    }

    #[test]
    fn win_rate_dihitung_dari_net_bukan_fee() {
        // Posisi yang fee-nya besar tapi net-nya negatif bukan kemenangan.
        let menang = hitung(0.0, 0.0, 0.0, 0.0, 0.0, 10.0, 1.0, 0.0, 0.0, false);
        let kalah = hitung(10.0, 0.0, 4.0, 6.0, 0.0, 5.0, 2.0, 0.2, 0.0, false);
        let r = ringkas(&[menang, kalah]);
        assert_eq!(r.win_rate_bps, Some(5_000), "satu dari dua menang");
    }

    #[test]
    fn ringkasan_menghitung_yang_perkiraan() {
        let a = hitung(0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, true);
        let b = hitung(0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, false);
        assert_eq!(ringkas(&[a, b]).perkiraan, 1);
    }

    #[test]
    fn total_biaya_menjumlahkan_ketiganya() {
        let h = hitung(10.0, 0.0, 5.0, 8.0, 0.0, 0.0, 2.0, 0.5, 0.3, false);
        let r = ringkas(&[h]);
        assert!(hampir(r.total_biaya.unwrap(), 2.0 + 0.5 + 0.3));
    }

    #[test]
    fn kunci_catatan_tidak_peduli_besar_kecil_huruf() {
        assert_eq!(kunci_catatan("0xABC"), kunci_catatan("0xabc"));
    }

    #[test]
    fn token_id_sama_pada_manager_berbeda_tidak_bertabrakan() {
        let v3 = crate::positions::position_id(
            4663,
            alloy::primitives::Address::repeat_byte(1),
            alloy::primitives::U256::from(7),
        );
        let v4 = crate::positions::position_id(
            4663,
            alloy::primitives::Address::repeat_byte(2),
            alloy::primitives::U256::from(7),
        );
        assert_ne!(kunci_catatan(&v3), kunci_catatan(&v4));
        assert_ne!(
            kunci_catatan(&v3),
            kunci_catatan(&v3.replace("4663:", "1:"))
        );
    }

    #[test]
    fn tambah_likuiditas_memperbarui_basis_tanpa_membuat_basis_eksternal() {
        let dir =
            std::env::temp_dir().join(format!("rangefinder-pnl-increase-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = crate::store::Store::open(&dir).unwrap();
        let id = "4663:manager:9";
        simpan(
            &store,
            &Catatan {
                position_id: id.into(),
                token_id: "9".into(),
                flow_id: Some("flow".into()),
                setor0: "100".into(),
                setor1: "200".into(),
                harga_buka: None,
                lengkap: true,
                setoran_aktual: true,
                gas_wei: None,
                gas_kutipan: Some(0.0),
                slippage_kutipan: Some(0.0),
                dibuka_pada: chrono::Utc::now().to_rfc3339(),
                tx_buka: None,
                rekonstruksi_log: None,
                rekonstruksi_v4: None,
            },
        );
        assert!(tambah_setoran(&store, id, U256::from(7), U256::from(11), true).unwrap());
        let updated = ambil(&store, id).unwrap();
        assert_eq!(updated.setor0, "107");
        assert_eq!(updated.setor1, "211");
        assert!(updated.setoran_aktual);
        assert!(!updated.lengkap);
        assert!(!tambah_setoran(
            &store,
            "4663:manager:eksternal",
            U256::from(1),
            U256::from(1),
            false,
        )
        .unwrap());
    }

    #[tokio::test]
    async fn posisi_tertutup_direkonstruksi_setelah_receipt_final() {
        use alloy::{
            network::TransactionBuilder,
            primitives::B256,
            providers::ProviderBuilder,
            rpc::types::{TransactionReceipt, TransactionRequest},
            transports::mock::Asserter,
        };

        let dir =
            std::env::temp_dir().join(format!("rangefinder-pnl-closed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = crate::store::Store::open(&dir).unwrap();
        let owner = Address::repeat_byte(1);
        let position_id = "4663:0xmanager:7";
        simpan(
            &store,
            &Catatan {
                position_id: position_id.into(),
                token_id: "7".into(),
                flow_id: None,
                setor0: "100".into(),
                setor1: "100".into(),
                harga_buka: None,
                lengkap: false,
                setoran_aktual: true,
                gas_wei: Some("1".into()),
                gas_kutipan: Some(0.25),
                slippage_kutipan: Some(0.0),
                dibuka_pada: "2026-09-09T00:00:00Z".into(),
                tx_buka: None,
                rekonstruksi_log: None,
                rekonstruksi_v4: None,
            },
        );

        let mut context = crate::exec::journal::test_context();
        context.operation = crate::exec::journal::Operation::Close;
        context.position_id = Some(position_id.into());
        context.deployment_usd = None;
        context.close_basis = Some(crate::exec::journal::CloseBasis {
            sqrt_price_x96: (U256::from(1u8) << 96usize).to_string(),
            fee0: Some("10".into()),
            fee1: Some("10".into()),
            quote_token0: false,
            quote_decimals: 0,
            currency_id: "4663:quote".into(),
            currency_symbol: "USDG".into(),
            symbol0: Some("MEME".into()),
            symbol1: Some("USDG".into()),
        });
        let tx_hash = B256::repeat_byte(8);
        let request = TransactionRequest::default()
            .with_from(owner)
            .with_to(Address::repeat_byte(2))
            .with_chain_id(4663)
            .with_nonce(9)
            .with_gas_limit(100_000);
        let entry = crate::exec::journal::Journal {
            store: &store,
            context,
        }
        .prepare(4663, owner, tx_hash, request)
        .unwrap();
        let receipt: TransactionReceipt = serde_json::from_value(serde_json::json!({
            "transactionHash": tx_hash,
            "from": owner,
            "to": Address::repeat_byte(2),
            "gasUsed": "0x5208",
            "cumulativeGasUsed": "0x5208",
            "effectiveGasPrice": "0x3b9aca00",
            "status": "0x1",
            "type": "0x2",
            "logs": [],
            "logsBloom": format!("0x{}", "00".repeat(256)),
            "blockNumber": "0x10",
            "blockHash": B256::repeat_byte(4)
        }))
        .unwrap();
        crate::exec::journal::record_receipt(&store, &entry, receipt.clone()).unwrap();
        crate::exec::journal::record_received(
            &store,
            4663,
            owner,
            tx_hash,
            crate::exec::journal::ReceivedAmounts {
                amount0: "90".into(),
                amount1: "110".into(),
                evidence: "v3_collect_event".into(),
            },
        )
        .unwrap();

        let rpc = Asserter::new();
        rpc.push_success(&4663u64);
        rpc.push_success(&100u64);
        rpc.push_success(&receipt);
        let provider = ProviderBuilder::new().connect_mocked_client(rpc);
        crate::exec::journal::reconcile(&provider, &store, 4663, owner)
            .await
            .unwrap();

        let history = histori_tertutup(&store, 4663, owner).unwrap();
        assert_eq!(history.len(), 1);
        let result = history[0].pnl.as_ref().unwrap();
        assert_eq!(result.fee, Some(20.0));
        assert_eq!(result.impermanent_loss, 20.0);
        assert_eq!(result.gas, Some(0.25));
        assert_eq!(result.net, Some(-0.25));
        assert!(!result.perkiraan);

        let mut catatan = ambil(&store, position_id).unwrap();
        catatan.setoran_aktual = false;
        simpan(&store, &catatan);
        let perkiraan = histori_tertutup(&store, 4663, owner).unwrap();
        let result = perkiraan[0].pnl.as_ref().unwrap();
        assert_eq!(
            result.net,
            Some(-0.25),
            "komponen yang tersedia tetap harus menghasilkan net"
        );
        assert!(result.perkiraan);
    }

    #[test]
    fn eth_dan_usdg_tidak_dijumlahkan() {
        let mut eth = hitung(0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, false);
        eth.currency = Currency {
            id: "4663:eth".into(),
            symbol: "ETH".into(),
        };
        let mut usd = eth.clone();
        usd.currency = Currency {
            id: "4663:usdg".into(),
            symbol: "USDG".into(),
        };
        usd.net = Some(100.0);
        let r = ringkas(&[eth.clone(), usd]);
        assert_eq!(r.total_net, None);
        assert_eq!(r.currencies.len(), 2);
        assert_eq!(r.currencies[0].total_net, Some(1.0));
        assert_eq!(r.currencies[1].total_net, Some(100.0));
        assert_eq!(ringkas(&[eth.clone(), eth]).total_net, Some(2.0));
    }
}
