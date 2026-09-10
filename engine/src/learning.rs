//! Loop kalibrasi lokal dari prediksi posisi ke hasil yang sudah terbukti.
//!
//! INVARIAN
//! - Prediksi dan hasil disimpan sebagai pasangan yang tidak dapat ditukar
//!   antar posisi. Tanpa pasangan itu, kalibrasi hanya menyesuaikan cerita
//!   setelah hasil diketahui.
//! - Data yang belum tersedia tetap `None`. Umur token, holder, gas dalam
//!   kutipan, atau fee historis tidak pernah diisi nol untuk membuat rekaman
//!   terlihat lengkap.
//! - Satu rezim membutuhkan sedikitnya 30 hasil lengkap. Versi baru menjadi
//!   penantang selama 20 hasil berikutnya dan hanya dipromosikan jika galatnya
//!   lebih kecil serta menang pada sedikitnya 12 perbandingan.
//! - Semua faktor, bobot, dan ambang dijepit ke batas keras sebelum disimpan.
//!   Rollback hanya memindahkan penunjuk versi aktif dan tidak menghapus data.

use std::collections::{BTreeMap, HashMap};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::pnl::{Hasil, PosisiTertutup, Ringkasan};
use crate::positions::Posisi;
use crate::signals::{bar::Bar, dump_basis, sideways};
use crate::store::Store;

const RECORD_PREFIX: &str = "learning:v1:position:";
const CALIBRATION_PREFIX: &str = "learning:v1:calibration:";
const ACTIVE_PREFIX: &str = "learning:v1:active:";
const CHALLENGER_PREFIX: &str = "learning:v1:challenger:";
const TRAINED_PREFIX: &str = "learning:v1:trained:";

pub const MIN_TRAINING_POSITIONS: usize = 30;
pub const SHADOW_POSITIONS: usize = 20;
const MIN_SHADOW_WINS: usize = 12;
const MAX_OBSERVATION_GAP_SECONDS: i64 = 120;

const FACTOR_MIN: f64 = 0.50;
const FACTOR_MAX: f64 = 1.50;
const WEIGHT_MIN: f64 = 0.10;
const WEIGHT_MAX: f64 = 0.45;
const MAX_ER_MIN: f64 = 0.20;
const MAX_ER_MAX: f64 = 0.50;
const MIN_VOLUME_Z_MIN: f64 = 0.25;
const MIN_VOLUME_Z_MAX: f64 = 1.50;
const MAX_BANDWIDTH_MIN: f64 = 0.20;
const MAX_BANDWIDTH_MAX: f64 = 0.50;
const MAX_CONTRACTION_MIN: f64 = 0.60;
const MAX_CONTRACTION_MAX: f64 = 0.95;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Regime {
    Sideways,
    DumpBasis,
    Trend,
    Volatile,
    Unknown,
}

impl Regime {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sideways => "sideways",
            Self::DumpBasis => "dump_basis",
            Self::Trend => "trend",
            Self::Volatile => "volatile",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_lowercase().as_str() {
            "sideways" => Some(Self::Sideways),
            "dump_basis" | "basis" => Some(Self::DumpBasis),
            "trend" => Some(Self::Trend),
            "volatile" | "volatil" => Some(Self::Volatile),
            "unknown" | "tidak_diketahui" => Some(Self::Unknown),
            _ => None,
        }
    }

    fn all() -> [Self; 5] {
        [
            Self::Sideways,
            Self::DumpBasis,
            Self::Trend,
            Self::Volatile,
            Self::Unknown,
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureVector {
    pub token: String,
    pub pool_id: String,
    pub regime: Regime,
    pub sideways_score: Option<f64>,
    pub efficiency_ratio: Option<f64>,
    pub volume_z: Option<f64>,
    pub containment: Option<f64>,
    pub bandwidth: Option<f64>,
    pub contraction: Option<f64>,
    pub tape_balance: Option<f64>,
    pub range_width_atr: Option<f64>,
    pub liquidity_share_bps: Option<u32>,
    pub fee_tier: u32,
    pub protocol_version: String,
    pub token_age_hours: Option<f64>,
    pub holder_count: Option<u64>,
    pub top10_holder_percent: Option<f64>,
    pub poc_distance_atr: Option<f64>,
    /// Prediksi sebelum koreksi versi kalibrasi.
    pub predicted_fee_apr_raw: Option<f64>,
    pub predicted_hours_in_range_raw: Option<f64>,
    /// Prediksi yang benar benar ditampilkan dan dipakai saat posisi dibuka.
    pub predicted_fee_apr: Option<f64>,
    pub predicted_hours_in_range: Option<f64>,
    pub il_break_even_hours: Option<f64>,
    pub position_value_quote: Option<f64>,
    pub calibration_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    pub realized_fee: Option<f64>,
    pub impermanent_loss: Option<f64>,
    pub gas: Option<f64>,
    pub slippage: Option<f64>,
    pub net: Option<f64>,
    pub hours_in_range: f64,
    pub duration_hours: f64,
    pub closed_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionRecord {
    pub position_id: String,
    pub opened_at: String,
    pub last_observed_at: String,
    pub last_in_range: bool,
    pub in_range_seconds: u64,
    pub feature: FeatureVector,
    pub outcome: Option<Outcome>,
}

#[derive(Debug, Clone)]
pub struct Draft {
    pub feature: FeatureVector,
}

pub struct DraftInput<'a> {
    pub token: &'a str,
    pub pool_id: &'a str,
    pub protocol_version: &'a str,
    pub fee_tier: u32,
    pub range_lower: f64,
    pub range_upper: f64,
    pub current_price: f64,
    pub planned_liquidity: u128,
    pub active_liquidity: u128,
    pub raw_fee_apr: Option<f64>,
    pub position_value_quote: Option<f64>,
    pub gas_cost_quote: Option<f64>,
    pub bars: &'a [Bar],
    pub sideways: Option<&'a sideways::Sinyal>,
    pub basis: Option<&'a dump_basis::Hasil>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalibrationStatus {
    Champion,
    Challenger,
    Archived,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Calibration {
    pub version: String,
    pub parent_version: Option<String>,
    pub regime: Regime,
    pub status: CalibrationStatus,
    pub fee_factor: f64,
    pub hours_factor: f64,
    pub volume_weight: f64,
    pub er_weight: f64,
    pub containment_weight: f64,
    pub tape_weight: f64,
    pub max_er: f64,
    pub min_volume_z: f64,
    pub max_bandwidth: f64,
    pub max_contraction: f64,
    pub training_positions: usize,
    pub shadow_positions: Vec<String>,
    pub shadow_wins: usize,
    pub challenger_error: f64,
    pub champion_error: f64,
    pub created_at: String,
}

impl Calibration {
    fn baseline(regime: Regime) -> Self {
        let rules = sideways::Aturan::default();
        Self {
            version: baseline_version(regime),
            parent_version: None,
            regime,
            status: CalibrationStatus::Champion,
            fee_factor: 1.0,
            hours_factor: 1.0,
            volume_weight: rules.bobot_volume,
            er_weight: rules.bobot_er,
            containment_weight: rules.bobot_containment,
            tape_weight: rules.bobot_tape,
            max_er: rules.maks_er,
            min_volume_z: rules.min_volume_z,
            max_bandwidth: rules.maks_bandwidth,
            max_contraction: rules.maks_kontraksi,
            training_positions: 0,
            shadow_positions: Vec::new(),
            shadow_wins: 0,
            challenger_error: 0.0,
            champion_error: 0.0,
            created_at: "baseline".into(),
        }
    }

    fn bounded(mut self) -> Self {
        self.fee_factor = finite_or(self.fee_factor, 1.0).clamp(FACTOR_MIN, FACTOR_MAX);
        self.hours_factor = finite_or(self.hours_factor, 1.0).clamp(FACTOR_MIN, FACTOR_MAX);
        let weights = normalize_weights([
            self.volume_weight,
            self.er_weight,
            self.containment_weight,
            self.tape_weight,
        ]);
        self.volume_weight = weights[0];
        self.er_weight = weights[1];
        self.containment_weight = weights[2];
        self.tape_weight = weights[3];
        self.max_er = finite_or(self.max_er, 0.35).clamp(MAX_ER_MIN, MAX_ER_MAX);
        self.min_volume_z =
            finite_or(self.min_volume_z, 0.50).clamp(MIN_VOLUME_Z_MIN, MIN_VOLUME_Z_MAX);
        self.max_bandwidth =
            finite_or(self.max_bandwidth, 0.35).clamp(MAX_BANDWIDTH_MIN, MAX_BANDWIDTH_MAX);
        self.max_contraction =
            finite_or(self.max_contraction, 0.80).clamp(MAX_CONTRACTION_MIN, MAX_CONTRACTION_MAX);
        self
    }

    fn rules(&self) -> sideways::Aturan {
        sideways::Aturan {
            bobot_volume: self.volume_weight,
            bobot_er: self.er_weight,
            bobot_containment: self.containment_weight,
            bobot_tape: self.tape_weight,
            maks_er: self.max_er,
            min_volume_z: self.min_volume_z,
            maks_bandwidth: self.max_bandwidth,
            maks_kontraksi: self.max_contraction,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CalibrationView {
    pub version: String,
    pub regime: Regime,
    pub status: CalibrationStatus,
    pub fee_factor: f64,
    pub hours_factor: f64,
    pub training_positions: usize,
    pub shadow_count: usize,
    pub shadow_wins: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct VersionSummary {
    pub version: String,
    pub summary: Ringkasan,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct LearningView {
    pub records: usize,
    pub closed_records: usize,
    pub position_versions: BTreeMap<String, String>,
    pub calibrations: Vec<CalibrationView>,
    pub summaries: Vec<VersionSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReplayReport {
    pub version: String,
    pub regime: Regime,
    pub positions: usize,
    pub median_fee_error_percent: Option<f64>,
    pub median_hours_error_percent: Option<f64>,
    pub positive_net_accuracy_percent: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoricalReplayReport {
    pub version: String,
    pub bars: usize,
    pub samples: usize,
    pub median_exit_time_error_percent: Option<f64>,
    pub target_below_30_percent: bool,
}

pub fn aturan_sideways(store: &Store) -> sideways::Aturan {
    active_calibration(store, Regime::Sideways)
        .unwrap_or_else(|_| Calibration::baseline(Regime::Sideways))
        .bounded()
        .rules()
}

pub fn create_draft(store: &Store, input: DraftInput<'_>) -> Draft {
    let regime = classify_regime(input.sideways, input.basis);
    let calibration = active_calibration(store, regime)
        .unwrap_or_else(|_| Calibration::baseline(regime))
        .bounded();
    let atr = crate::signals::metrik::atr(input.bars, input.bars.len().min(240));
    let width = input.range_upper - input.range_lower;
    let range_width_atr = atr.filter(|v| *v > 0.0).and_then(|v| positive(width / v));
    let raw_hours = predict_hours(
        input.bars,
        input.current_price,
        input.range_lower,
        input.range_upper,
        atr,
    );
    let predicted_fee_apr = input
        .raw_fee_apr
        .and_then(|v| positive(v * calibration.fee_factor));
    let predicted_hours_in_range = raw_hours.and_then(|v| positive(v * calibration.hours_factor));
    let fee_per_hour = predicted_fee_apr
        .zip(input.position_value_quote)
        .map(|(apr, value)| value * apr / 100.0 / (365.0 * 24.0))
        .and_then(positive);
    let il_break_even_hours = input
        .gas_cost_quote
        .zip(fee_per_hour)
        .and_then(|(gas, hourly)| positive(gas / hourly));
    let liquidity_share_bps = if input.planned_liquidity == 0 {
        None
    } else {
        let total = input
            .active_liquidity
            .saturating_add(input.planned_liquidity);
        (total > 0).then_some(
            ((input.planned_liquidity as f64 / total as f64) * 10_000.0)
                .round()
                .clamp(0.0, 10_000.0) as u32,
        )
    };
    let poc_distance_atr = input
        .basis
        .map(|basis| (input.current_price - basis.jangkar.poc).abs())
        .zip(atr)
        .and_then(|(distance, atr)| positive(distance / atr));

    Draft {
        feature: FeatureVector {
            token: input.token.to_lowercase(),
            pool_id: input.pool_id.to_lowercase(),
            regime,
            sideways_score: input.sideways.map(|v| v.skor),
            efficiency_ratio: input.sideways.map(|v| v.er),
            volume_z: input.sideways.map(|v| v.volume_z),
            containment: input.sideways.map(|v| v.containment),
            bandwidth: input.sideways.map(|v| v.bandwidth),
            contraction: input.sideways.map(|v| v.kontraksi),
            tape_balance: input.sideways.map(|v| v.tape),
            range_width_atr,
            liquidity_share_bps,
            fee_tier: input.fee_tier,
            protocol_version: input.protocol_version.into(),
            token_age_hours: None,
            holder_count: None,
            top10_holder_percent: None,
            poc_distance_atr,
            predicted_fee_apr_raw: input.raw_fee_apr,
            predicted_hours_in_range_raw: raw_hours,
            predicted_fee_apr,
            predicted_hours_in_range,
            il_break_even_hours,
            position_value_quote: input.position_value_quote,
            calibration_version: calibration.version,
        },
    }
}

pub fn record_open(
    store: &Store,
    position_id: &str,
    opened_at: &str,
    draft: Draft,
) -> Result<bool> {
    let key = record_key(position_id);
    if store.get::<PositionRecord>(&key)?.is_some() {
        return Ok(false);
    }
    DateTime::parse_from_rfc3339(opened_at).context("waktu buka posisi tidak valid")?;
    store.put(
        &key,
        &PositionRecord {
            position_id: position_id.to_lowercase(),
            opened_at: opened_at.into(),
            last_observed_at: opened_at.into(),
            last_in_range: false,
            in_range_seconds: 0,
            feature: draft.feature,
            outcome: None,
        },
    )?;
    Ok(true)
}

/// Perbarui waktu di dalam range dan pasangkan histori close dengan prediksi.
/// Mengembalikan jumlah posisi yang baru mendapat hasil pada pemanggilan ini.
pub fn synchronize(
    store: &Store,
    positions: &[Posisi],
    closed: &[PosisiTertutup],
    now: DateTime<Utc>,
) -> Result<usize> {
    let open: HashMap<String, &Posisi> = positions
        .iter()
        .map(|p| (p.position_id.to_lowercase(), p))
        .collect();
    let closed: HashMap<String, &PosisiTertutup> = closed
        .iter()
        .map(|p| (p.position_id.to_lowercase(), p))
        .collect();
    let mut newly_closed = Vec::new();

    for (key, mut record) in store.prefix::<PositionRecord>(RECORD_PREFIX)? {
        if record.outcome.is_some() {
            continue;
        }
        if let Some(position) = open.get(&record.position_id.to_lowercase()) {
            accrue_interval(&mut record, now, position.in_range);
            store.put(&key, &record)?;
            continue;
        }
        let Some(position) = closed.get(&record.position_id.to_lowercase()) else {
            continue;
        };
        let closed_at = DateTime::parse_from_rfc3339(&position.ditutup_pada)
            .context("waktu tutup posisi tidak valid")?
            .with_timezone(&Utc);
        accrue_until_close(&mut record, closed_at);
        let opened_at = DateTime::parse_from_rfc3339(&record.opened_at)
            .context("waktu buka rekaman tidak valid")?
            .with_timezone(&Utc);
        let duration_hours = closed_at
            .signed_duration_since(opened_at)
            .num_seconds()
            .max(0) as f64
            / 3600.0;
        let pnl = position.pnl.as_ref();
        record.outcome = Some(Outcome {
            realized_fee: pnl.and_then(|p| p.fee),
            impermanent_loss: pnl.map(|p| p.impermanent_loss),
            gas: pnl.and_then(|p| p.gas),
            slippage: pnl.and_then(|p| p.slippage),
            net: pnl.and_then(|p| p.net),
            hours_in_range: record.in_range_seconds as f64 / 3600.0,
            duration_hours,
            closed_at: position.ditutup_pada.clone(),
        });
        store.put(&key, &record)?;
        newly_closed.push(record);
    }

    for record in &newly_closed {
        update_challenger(store, record)?;
    }
    train_automatically(store)?;
    Ok(newly_closed.len())
}

pub fn view(
    store: &Store,
    positions: &[Posisi],
    closed: &[PosisiTertutup],
) -> Result<LearningView> {
    let records: Vec<PositionRecord> = store
        .prefix::<PositionRecord>(RECORD_PREFIX)?
        .into_iter()
        .map(|(_, value)| value)
        .collect();
    let position_versions = records
        .iter()
        .map(|record| {
            (
                record.position_id.clone(),
                record.feature.calibration_version.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();

    let mut grouped = BTreeMap::<String, Vec<Hasil>>::new();
    for position in positions {
        if let (Some(version), Some(pnl)) = (
            position_versions.get(&position.position_id.to_lowercase()),
            position.pnl.clone(),
        ) {
            grouped.entry(version.clone()).or_default().push(pnl);
        }
    }
    for position in closed {
        if let (Some(version), Some(pnl)) = (
            position_versions.get(&position.position_id.to_lowercase()),
            position.pnl.clone(),
        ) {
            grouped.entry(version.clone()).or_default().push(pnl);
        }
    }
    let summaries = grouped
        .into_iter()
        .map(|(version, values)| VersionSummary {
            version,
            summary: crate::pnl::ringkas(&values),
        })
        .collect();

    let mut calibrations = store
        .prefix::<Calibration>(CALIBRATION_PREFIX)?
        .into_iter()
        .map(|(_, c)| calibration_view(&c))
        .collect::<Vec<_>>();
    for regime in Regime::all() {
        let baseline = baseline_version(regime);
        if active_version(store, regime)? == baseline
            && !calibrations.iter().any(|c| c.version == baseline)
        {
            calibrations.push(calibration_view(&Calibration::baseline(regime)));
        }
    }
    calibrations.sort_by(|a, b| a.version.cmp(&b.version));

    Ok(LearningView {
        records: records.len(),
        closed_records: records.iter().filter(|r| r.outcome.is_some()).count(),
        position_versions,
        calibrations,
        summaries,
    })
}

pub fn train_manual(store: &Store, regime: Option<Regime>) -> Result<Calibration> {
    let records = complete_records(store)?;
    let selected = match regime {
        Some(regime) => regime,
        None => Regime::all()
            .into_iter()
            .max_by_key(|regime| {
                records
                    .iter()
                    .filter(|r| r.feature.regime == *regime)
                    .count()
            })
            .context("belum ada rekaman")?,
    };
    train_regime(store, selected, &records, true)?
        .context("belum ada 30 posisi tertutup lengkap pada rezim yang sama untuk kalibrasi")
}

pub fn rollback(store: &Store, version: &str) -> Result<Calibration> {
    let mut target = calibration_by_version(store, version)?
        .with_context(|| format!("versi kalibrasi {version} tidak ditemukan"))?;
    let current_version = active_version(store, target.regime)?;
    if current_version != target.version {
        if let Some(mut current) = calibration_by_version(store, &current_version)? {
            current.status = CalibrationStatus::Archived;
            save_calibration(store, &current)?;
        }
    }
    target.status = CalibrationStatus::Champion;
    if !target.version.starts_with("baseline-v1-") {
        save_calibration(store, &target)?;
    }
    store.put(&active_key(target.regime), &target.version)?;
    let active_challenger: Option<String> = store.get(&challenger_key(target.regime))?;
    if let Some(challenger) = active_challenger {
        if challenger != target.version {
            if let Some(mut record) = calibration_by_version(store, &challenger)? {
                record.status = CalibrationStatus::Rejected;
                save_calibration(store, &record)?;
            }
        }
        store.delete(&challenger_key(target.regime))?;
    }
    Ok(target)
}

pub fn replay(store: &Store, version: Option<&str>) -> Result<ReplayReport> {
    let calibration = match version {
        Some(version) => calibration_by_version(store, version)?
            .with_context(|| format!("versi kalibrasi {version} tidak ditemukan"))?,
        None => {
            let records = complete_records(store)?;
            let regime = Regime::all()
                .into_iter()
                .max_by_key(|regime| {
                    records
                        .iter()
                        .filter(|r| r.feature.regime == *regime)
                        .count()
                })
                .context("belum ada hasil untuk replay")?;
            active_calibration(store, regime)?
        }
    };
    let records = complete_records(store)?;
    replay_with(&calibration, &records)
}

/// Jalankan kembali detektor dan proyeksi waktu keluar pada candle historis.
///
/// Satu sampel diambil tiap 30 bar setelah jendela 240 bar tersedia. Hasilnya
/// dibandingkan dengan sentuhan batas pertama pada bar sesudahnya. Fungsi ini
/// murni baca dan tidak dapat mempromosikan kalibrasi.
pub fn replay_bars(
    store: &Store,
    bars: Vec<Bar>,
    version: Option<&str>,
) -> Result<HistoricalReplayReport> {
    let bars = crate::signals::bar::bersihkan(bars);
    let calibration = match version {
        Some(version) => calibration_by_version(store, version)?
            .with_context(|| format!("versi kalibrasi {version} tidak ditemukan"))?,
        None => active_calibration(store, Regime::Sideways)?,
    }
    .bounded();
    let rules = calibration.rules();
    let mut errors = Vec::new();
    if bars.len() > rules.jendela {
        for index in (rules.jendela - 1..bars.len() - 1).step_by(30) {
            let history = &bars[..=index];
            let Some(signal) = sideways::ukur(history, &rules) else {
                continue;
            };
            let current = bars[index].close;
            let Some(suggestion) = crate::signals::penempatan::dari_kanal(
                current,
                signal.kanal_bawah,
                signal.kanal_atas,
            ) else {
                continue;
            };
            let atr = crate::signals::metrik::atr(history, rules.jendela);
            let Some(predicted) =
                predict_hours(history, current, suggestion.bawah, suggestion.atas, atr)
                    .map(|hours| hours * calibration.hours_factor)
            else {
                continue;
            };
            let Some(exit) = bars[index + 1..]
                .iter()
                .find(|bar| bar.low <= suggestion.bawah || bar.high >= suggestion.atas)
            else {
                continue;
            };
            let actual = (exit.time - bars[index].time).max(1) as f64 / 3600.0;
            if let Some(error) = relative_error(predicted, actual) {
                errors.push(error);
            }
        }
    }
    let samples = errors.len();
    let median = median(errors).map(|value| value * 100.0);
    Ok(HistoricalReplayReport {
        version: calibration.version,
        bars: bars.len(),
        samples,
        median_exit_time_error_percent: median,
        target_below_30_percent: median.is_some_and(|value| value < 30.0),
    })
}

fn train_automatically(store: &Store) -> Result<()> {
    let records = complete_records(store)?;
    for regime in Regime::all() {
        let _ = train_regime(store, regime, &records, false)?;
    }
    Ok(())
}

fn train_regime(
    store: &Store,
    regime: Regime,
    all: &[PositionRecord],
    force: bool,
) -> Result<Option<Calibration>> {
    if store.get::<String>(&challenger_key(regime))?.is_some() {
        if force {
            bail!("rezim {} masih mempunyai penantang aktif", regime.as_str());
        }
        return Ok(None);
    }
    let records = all
        .iter()
        .filter(|record| record.feature.regime == regime)
        .collect::<Vec<_>>();
    if records.len() < MIN_TRAINING_POSITIONS {
        return Ok(None);
    }
    let trained = store.get::<usize>(&trained_key(regime))?.unwrap_or(0);
    if !force && records.len() < trained.saturating_add(MIN_TRAINING_POSITIONS) {
        return Ok(None);
    }

    let champion = active_calibration(store, regime)?;
    let fee_factor = median(
        records
            .iter()
            .filter_map(|record| actual_fee_apr(record).zip(record.feature.predicted_fee_apr_raw))
            .filter_map(|(actual, predicted)| positive(actual / predicted))
            .collect(),
    )
    .unwrap_or(champion.fee_factor);
    let hours_factor = median(
        records
            .iter()
            .filter_map(|record| {
                record.outcome.as_ref().and_then(|outcome| {
                    record
                        .feature
                        .predicted_hours_in_range_raw
                        .and_then(|predicted| positive(outcome.hours_in_range / predicted))
                })
            })
            .collect(),
    )
    .unwrap_or(champion.hours_factor);
    let successful = records
        .iter()
        .filter(|record| {
            record
                .outcome
                .as_ref()
                .and_then(|o| o.net)
                .is_some_and(|n| n > 0.0)
        })
        .copied()
        .collect::<Vec<_>>();
    let failed = records
        .iter()
        .filter(|record| {
            record
                .outcome
                .as_ref()
                .and_then(|o| o.net)
                .is_some_and(|n| n <= 0.0)
        })
        .copied()
        .collect::<Vec<_>>();
    let base = sideways::Aturan::default();
    let adjusted = adjusted_weights(&successful, &failed, &base);
    let positive_median = |select: fn(&FeatureVector) -> Option<f64>| {
        median(
            successful
                .iter()
                .filter_map(|record| select(&record.feature))
                .collect(),
        )
    };
    let now = Utc::now();
    let candidate = Calibration {
        version: format!(
            "cal-{}-{}-{}",
            regime.as_str(),
            now.timestamp_millis(),
            records.len()
        ),
        parent_version: Some(champion.version.clone()),
        regime,
        status: CalibrationStatus::Challenger,
        fee_factor,
        hours_factor,
        volume_weight: adjusted[0],
        er_weight: adjusted[1],
        containment_weight: adjusted[2],
        tape_weight: adjusted[3],
        max_er: positive_median(|f| f.efficiency_ratio)
            .map(|v| v + 0.05)
            .unwrap_or(champion.max_er),
        min_volume_z: positive_median(|f| f.volume_z)
            .map(|v| v * 0.80)
            .unwrap_or(champion.min_volume_z),
        max_bandwidth: positive_median(|f| f.bandwidth)
            .map(|v| v * 1.20)
            .unwrap_or(champion.max_bandwidth),
        max_contraction: positive_median(|f| f.contraction)
            .map(|v| v * 1.10)
            .unwrap_or(champion.max_contraction),
        training_positions: records.len(),
        shadow_positions: Vec::new(),
        shadow_wins: 0,
        challenger_error: 0.0,
        champion_error: 0.0,
        created_at: now.to_rfc3339(),
    }
    .bounded();
    save_calibration(store, &candidate)?;
    store.put(&challenger_key(regime), &candidate.version)?;
    store.put(&trained_key(regime), &records.len())?;
    Ok(Some(candidate))
}

fn update_challenger(store: &Store, record: &PositionRecord) -> Result<()> {
    let regime = record.feature.regime;
    let Some(version): Option<String> = store.get(&challenger_key(regime))? else {
        return Ok(());
    };
    let Some(mut challenger) = calibration_by_version(store, &version)? else {
        store.delete(&challenger_key(regime))?;
        return Ok(());
    };
    if challenger
        .shadow_positions
        .iter()
        .any(|id| id.eq_ignore_ascii_case(&record.position_id))
    {
        return Ok(());
    }
    let outcome_time = record
        .outcome
        .as_ref()
        .and_then(|outcome| DateTime::parse_from_rfc3339(&outcome.closed_at).ok());
    let created = DateTime::parse_from_rfc3339(&challenger.created_at).ok();
    if outcome_time
        .zip(created)
        .is_some_and(|(outcome, created)| outcome <= created)
    {
        return Ok(());
    }
    let champion = active_calibration(store, regime)?;
    let Some((candidate_error, champion_error)) =
        compare_prediction(&challenger, &champion, record)
    else {
        return Ok(());
    };
    challenger.shadow_positions.push(record.position_id.clone());
    challenger.challenger_error += candidate_error;
    challenger.champion_error += champion_error;
    if candidate_error < champion_error {
        challenger.shadow_wins += 1;
    }

    if challenger.shadow_positions.len() >= SHADOW_POSITIONS {
        if challenger.shadow_wins >= MIN_SHADOW_WINS
            && challenger.challenger_error < challenger.champion_error
        {
            if !champion.version.starts_with("baseline-v1-") {
                let mut archived = champion;
                archived.status = CalibrationStatus::Archived;
                save_calibration(store, &archived)?;
            }
            challenger.status = CalibrationStatus::Champion;
            store.put(&active_key(regime), &challenger.version)?;
        } else {
            challenger.status = CalibrationStatus::Rejected;
        }
        store.delete(&challenger_key(regime))?;
    }
    save_calibration(store, &challenger)
}

fn replay_with(calibration: &Calibration, all: &[PositionRecord]) -> Result<ReplayReport> {
    let records = all
        .iter()
        .filter(|record| record.feature.regime == calibration.regime)
        .collect::<Vec<_>>();
    let fee_errors = records
        .iter()
        .filter_map(|record| {
            let actual = actual_fee_apr(record)?;
            let predicted = record.feature.predicted_fee_apr_raw? * calibration.fee_factor;
            relative_error(predicted, actual)
        })
        .collect::<Vec<_>>();
    let hour_errors = records
        .iter()
        .filter_map(|record| {
            let actual = record.outcome.as_ref()?.hours_in_range;
            let predicted = record.feature.predicted_hours_in_range_raw? * calibration.hours_factor;
            relative_error(predicted, actual)
        })
        .collect::<Vec<_>>();
    let classified = records
        .iter()
        .filter_map(|record| {
            let actual = record.outcome.as_ref()?.net? > 0.0;
            let predicted = classification_score(calibration, &record.feature)? >= 0.50;
            Some(actual == predicted)
        })
        .collect::<Vec<_>>();
    Ok(ReplayReport {
        version: calibration.version.clone(),
        regime: calibration.regime,
        positions: records.len(),
        median_fee_error_percent: median(fee_errors).map(|v| v * 100.0),
        median_hours_error_percent: median(hour_errors).map(|v| v * 100.0),
        positive_net_accuracy_percent: (!classified.is_empty()).then(|| {
            classified.iter().filter(|correct| **correct).count() as f64 / classified.len() as f64
                * 100.0
        }),
    })
}

fn compare_prediction(
    challenger: &Calibration,
    champion: &Calibration,
    record: &PositionRecord,
) -> Option<(f64, f64)> {
    let mut challenger_errors = Vec::new();
    let mut champion_errors = Vec::new();
    if let (Some(actual), Some(raw)) =
        (actual_fee_apr(record), record.feature.predicted_fee_apr_raw)
    {
        challenger_errors.push(relative_error(raw * challenger.fee_factor, actual)?);
        champion_errors.push(relative_error(raw * champion.fee_factor, actual)?);
    }
    if let (Some(outcome), Some(raw)) = (
        record.outcome.as_ref(),
        record.feature.predicted_hours_in_range_raw,
    ) {
        challenger_errors.push(relative_error(
            raw * challenger.hours_factor,
            outcome.hours_in_range,
        )?);
        champion_errors.push(relative_error(
            raw * champion.hours_factor,
            outcome.hours_in_range,
        )?);
    }
    if challenger_errors.is_empty() {
        return None;
    }
    Some((mean(&challenger_errors), mean(&champion_errors)))
}

fn actual_fee_apr(record: &PositionRecord) -> Option<f64> {
    let outcome = record.outcome.as_ref()?;
    let fee = outcome.realized_fee?;
    let capital = record.feature.position_value_quote?;
    let duration = positive(outcome.duration_hours)?;
    positive(fee / capital * 100.0 * 365.0 * 24.0 / duration)
}

fn classification_score(calibration: &Calibration, feature: &FeatureVector) -> Option<f64> {
    let volume = (feature.volume_z? / 3.0).clamp(0.0, 1.0);
    Some(
        calibration.volume_weight * volume
            + calibration.er_weight * (1.0 - feature.efficiency_ratio?).clamp(0.0, 1.0)
            + calibration.containment_weight * feature.containment?.clamp(0.0, 1.0)
            + calibration.tape_weight * feature.tape_balance?.clamp(0.0, 1.0),
    )
}

fn adjusted_weights(
    successful: &[&PositionRecord],
    failed: &[&PositionRecord],
    base: &sideways::Aturan,
) -> [f64; 4] {
    let score = |records: &[&PositionRecord], select: fn(&FeatureVector) -> Option<f64>| {
        let values = records
            .iter()
            .filter_map(|record| select(&record.feature))
            .collect::<Vec<_>>();
        (!values.is_empty()).then(|| mean(&values))
    };
    let selectors: [fn(&FeatureVector) -> Option<f64>; 4] = [
        |f| f.volume_z.map(|v| (v / 3.0).clamp(0.0, 1.0)),
        |f| f.efficiency_ratio.map(|v| (1.0 - v).clamp(0.0, 1.0)),
        |f| f.containment,
        |f| f.tape_balance,
    ];
    let bases = [
        base.bobot_volume,
        base.bobot_er,
        base.bobot_containment,
        base.bobot_tape,
    ];
    let mut weights = bases;
    for i in 0..4 {
        let difference = score(successful, selectors[i])
            .zip(score(failed, selectors[i]))
            .map(|(positive, negative)| positive - negative)
            .unwrap_or(0.0);
        weights[i] = bases[i] * (1.0 + difference * 0.5);
    }
    normalize_weights(weights)
}

fn normalize_weights(mut weights: [f64; 4]) -> [f64; 4] {
    for weight in &mut weights {
        *weight = finite_or(*weight, 0.25).clamp(WEIGHT_MIN, WEIGHT_MAX);
    }
    // Dua lintasan cukup untuk mengembalikan jumlah ke satu sambil menjaga
    // setiap unsur di dalam batas. Koreksi sisa diberikan ke unsur yang masih
    // memiliki ruang paling besar.
    for _ in 0..2 {
        let total: f64 = weights.iter().sum();
        if total > 0.0 {
            for weight in &mut weights {
                *weight = (*weight / total).clamp(WEIGHT_MIN, WEIGHT_MAX);
            }
        }
    }
    let difference = 1.0 - weights.iter().sum::<f64>();
    let index = weights
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| {
            let room_a = if difference >= 0.0 {
                WEIGHT_MAX - **a
            } else {
                **a - WEIGHT_MIN
            };
            let room_b = if difference >= 0.0 {
                WEIGHT_MAX - **b
            } else {
                **b - WEIGHT_MIN
            };
            room_a
                .partial_cmp(&room_b)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(index, _)| index)
        .unwrap_or(0);
    weights[index] = (weights[index] + difference).clamp(WEIGHT_MIN, WEIGHT_MAX);
    weights
}

fn classify_regime(
    sideways: Option<&sideways::Sinyal>,
    basis: Option<&dump_basis::Hasil>,
) -> Regime {
    if basis.is_some_and(|value| value.lolos()) {
        Regime::DumpBasis
    } else if sideways.is_some_and(|value| value.lolos()) {
        Regime::Sideways
    } else if sideways.is_some_and(|value| value.bandwidth >= 0.35 || value.kontraksi >= 0.80) {
        Regime::Volatile
    } else if sideways.is_some_and(|value| value.er >= 0.35) {
        Regime::Trend
    } else {
        Regime::Unknown
    }
}

fn predict_hours(
    bars: &[Bar],
    current: f64,
    lower: f64,
    upper: f64,
    atr: Option<f64>,
) -> Option<f64> {
    let atr = positive(atr?)?;
    let distance = positive((current - lower).min(upper - current))?;
    let interval_seconds = bars
        .windows(2)
        .filter_map(|pair| (pair[1].time > pair[0].time).then_some(pair[1].time - pair[0].time))
        .next_back()
        .unwrap_or(60)
        .clamp(1, 86_400) as f64;
    positive((distance / atr).powi(2).clamp(1.0, 10_000.0) * interval_seconds / 3600.0)
}

fn accrue_interval(record: &mut PositionRecord, now: DateTime<Utc>, current_in_range: bool) {
    let last = DateTime::parse_from_rfc3339(&record.last_observed_at)
        .ok()
        .map(|value| value.with_timezone(&Utc))
        .unwrap_or(now);
    let seconds = now
        .signed_duration_since(last)
        .num_seconds()
        .clamp(0, MAX_OBSERVATION_GAP_SECONDS) as u64;
    if record.last_in_range {
        record.in_range_seconds = record.in_range_seconds.saturating_add(seconds);
    }
    record.last_observed_at = now.to_rfc3339();
    record.last_in_range = current_in_range;
}

fn accrue_until_close(record: &mut PositionRecord, closed_at: DateTime<Utc>) {
    let last = DateTime::parse_from_rfc3339(&record.last_observed_at)
        .ok()
        .map(|value| value.with_timezone(&Utc))
        .unwrap_or(closed_at);
    let seconds = closed_at
        .signed_duration_since(last)
        .num_seconds()
        .clamp(0, MAX_OBSERVATION_GAP_SECONDS) as u64;
    if record.last_in_range {
        record.in_range_seconds = record.in_range_seconds.saturating_add(seconds);
    }
    record.last_observed_at = closed_at.to_rfc3339();
}

fn complete_records(store: &Store) -> Result<Vec<PositionRecord>> {
    Ok(store
        .prefix::<PositionRecord>(RECORD_PREFIX)?
        .into_iter()
        .map(|(_, value)| value)
        .filter(|record| record.outcome.as_ref().and_then(|o| o.net).is_some())
        .collect())
}

fn active_calibration(store: &Store, regime: Regime) -> Result<Calibration> {
    let version = active_version(store, regime)?;
    Ok(calibration_by_version(store, &version)?.unwrap_or_else(|| Calibration::baseline(regime)))
}

fn active_version(store: &Store, regime: Regime) -> Result<String> {
    Ok(store
        .get::<String>(&active_key(regime))?
        .unwrap_or_else(|| baseline_version(regime)))
}

fn calibration_by_version(store: &Store, version: &str) -> Result<Option<Calibration>> {
    if let Some(regime) = version.strip_prefix("baseline-v1-").and_then(Regime::parse) {
        return Ok(Some(Calibration::baseline(regime)));
    }
    store.get(&format!("{CALIBRATION_PREFIX}{version}"))
}

fn save_calibration(store: &Store, calibration: &Calibration) -> Result<()> {
    let bounded = calibration.clone().bounded();
    store.put(
        &format!("{CALIBRATION_PREFIX}{}", bounded.version),
        &bounded,
    )
}

fn calibration_view(value: &Calibration) -> CalibrationView {
    CalibrationView {
        version: value.version.clone(),
        regime: value.regime,
        status: value.status,
        fee_factor: value.fee_factor,
        hours_factor: value.hours_factor,
        training_positions: value.training_positions,
        shadow_count: value.shadow_positions.len(),
        shadow_wins: value.shadow_wins,
    }
}

fn record_key(position_id: &str) -> String {
    format!("{RECORD_PREFIX}{}", position_id.to_lowercase())
}

fn baseline_version(regime: Regime) -> String {
    format!("baseline-v1-{}", regime.as_str())
}

fn active_key(regime: Regime) -> String {
    format!("{ACTIVE_PREFIX}{}", regime.as_str())
}

fn challenger_key(regime: Regime) -> String {
    format!("{CHALLENGER_PREFIX}{}", regime.as_str())
}

fn trained_key(regime: Regime) -> String {
    format!("{TRAINED_PREFIX}{}", regime.as_str())
}

fn finite_or(value: f64, fallback: f64) -> f64 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

fn positive(value: f64) -> Option<f64> {
    (value.is_finite() && value > 0.0).then_some(value)
}

fn relative_error(predicted: f64, actual: f64) -> Option<f64> {
    positive(actual.abs()).map(|denominator| (predicted - actual).abs() / denominator)
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len().max(1) as f64
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    values.retain(|value| value.is_finite());
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let middle = values.len() / 2;
    match values.len() {
        0 => None,
        len if len % 2 == 0 => Some((values[middle - 1] + values[middle]) / 2.0),
        _ => Some(values[middle]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "rangefinder-learning-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        Store::open(&dir).unwrap()
    }

    fn feature(regime: Regime, version: &str) -> FeatureVector {
        FeatureVector {
            token: "0xtoken".into(),
            pool_id: "0xpool".into(),
            regime,
            sideways_score: Some(70.0),
            efficiency_ratio: Some(0.20),
            volume_z: Some(1.2),
            containment: Some(0.8),
            bandwidth: Some(0.2),
            contraction: Some(0.7),
            tape_balance: Some(0.6),
            range_width_atr: Some(4.0),
            liquidity_share_bps: Some(100),
            fee_tier: 3000,
            protocol_version: "v4".into(),
            token_age_hours: None,
            holder_count: None,
            top10_holder_percent: None,
            poc_distance_atr: Some(0.5),
            predicted_fee_apr_raw: Some(100.0),
            predicted_hours_in_range_raw: Some(10.0),
            predicted_fee_apr: Some(100.0),
            predicted_hours_in_range: Some(10.0),
            il_break_even_hours: Some(1.0),
            position_value_quote: Some(100.0),
            calibration_version: version.into(),
        }
    }

    fn complete_record(id: usize, closed_hour: i64) -> PositionRecord {
        PositionRecord {
            position_id: format!("p{id}"),
            opened_at: "2026-01-01T00:00:00Z".into(),
            last_observed_at: "2026-01-01T00:00:00Z".into(),
            last_in_range: true,
            in_range_seconds: 8 * 3600,
            feature: feature(Regime::Sideways, "baseline-v1-sideways"),
            outcome: Some(Outcome {
                realized_fee: Some(0.12),
                impermanent_loss: Some(0.02),
                gas: Some(0.01),
                slippage: Some(0.0),
                net: Some(if id.is_multiple_of(3) { -0.01 } else { 0.09 }),
                hours_in_range: 8.0,
                duration_hours: 10.0,
                closed_at: format!("2026-02-{:02}T{:02}:00:00Z", id % 20 + 1, closed_hour),
            }),
        }
    }

    #[test]
    fn parameter_selalu_di_dalam_batas_keras() {
        let calibration = Calibration {
            version: "bad".into(),
            parent_version: None,
            regime: Regime::Sideways,
            status: CalibrationStatus::Challenger,
            fee_factor: 100.0,
            hours_factor: -10.0,
            volume_weight: 100.0,
            er_weight: -2.0,
            containment_weight: f64::NAN,
            tape_weight: 9.0,
            max_er: 9.0,
            min_volume_z: -1.0,
            max_bandwidth: 4.0,
            max_contraction: 0.0,
            training_positions: 30,
            shadow_positions: Vec::new(),
            shadow_wins: 0,
            challenger_error: 0.0,
            champion_error: 0.0,
            created_at: Utc::now().to_rfc3339(),
        }
        .bounded();
        assert!((FACTOR_MIN..=FACTOR_MAX).contains(&calibration.fee_factor));
        assert!((FACTOR_MIN..=FACTOR_MAX).contains(&calibration.hours_factor));
        for weight in [
            calibration.volume_weight,
            calibration.er_weight,
            calibration.containment_weight,
            calibration.tape_weight,
        ] {
            assert!((WEIGHT_MIN..=WEIGHT_MAX).contains(&weight));
        }
        let total = calibration.volume_weight
            + calibration.er_weight
            + calibration.containment_weight
            + calibration.tape_weight;
        assert!((total - 1.0).abs() < 1e-9);
    }

    #[test]
    fn kurang_dari_tiga_puluh_tidak_membuat_penantang() {
        let store = store("minimum");
        for id in 0..29 {
            let record = complete_record(id, 1);
            store
                .put(&record_key(&record.position_id), &record)
                .unwrap();
        }
        assert!(train_manual(&store, Some(Regime::Sideways)).is_err());
        assert!(store
            .get::<String>(&challenger_key(Regime::Sideways))
            .unwrap()
            .is_none());
    }

    #[test]
    fn tiga_puluh_rezim_sama_membuat_penantang_dan_rollback_satu_perintah() {
        let store = store("train");
        for id in 0..30 {
            let record = complete_record(id, 1);
            store
                .put(&record_key(&record.position_id), &record)
                .unwrap();
        }
        let candidate = train_manual(&store, Some(Regime::Sideways)).unwrap();
        assert_eq!(candidate.status, CalibrationStatus::Challenger);
        assert_eq!(candidate.training_positions, 30);
        let restored = rollback(&store, "baseline-v1-sideways").unwrap();
        assert_eq!(restored.version, "baseline-v1-sideways");
        assert_eq!(
            active_version(&store, Regime::Sideways).unwrap(),
            "baseline-v1-sideways"
        );
    }

    #[test]
    fn penantang_menunggu_dua_puluh_hasil_baru() {
        let store = store("shadow");
        for id in 0..30 {
            let record = complete_record(id, 1);
            store
                .put(&record_key(&record.position_id), &record)
                .unwrap();
        }
        let candidate = train_manual(&store, Some(Regime::Sideways)).unwrap();
        for id in 30..49 {
            let mut record = complete_record(id, 2);
            record.outcome.as_mut().unwrap().closed_at =
                (Utc::now() + chrono::Duration::seconds(id as i64 + 1)).to_rfc3339();
            update_challenger(&store, &record).unwrap();
        }
        let still = calibration_by_version(&store, &candidate.version)
            .unwrap()
            .unwrap();
        assert_eq!(still.status, CalibrationStatus::Challenger);
        assert_eq!(still.shadow_positions.len(), 19);

        let mut twentieth = complete_record(50, 2);
        twentieth.outcome.as_mut().unwrap().closed_at =
            (Utc::now() + chrono::Duration::minutes(1)).to_rfc3339();
        update_challenger(&store, &twentieth).unwrap();
        let finished = calibration_by_version(&store, &candidate.version)
            .unwrap()
            .unwrap();
        assert!(matches!(
            finished.status,
            CalibrationStatus::Champion | CalibrationStatus::Rejected
        ));
        assert_eq!(finished.shadow_positions.len(), 20);
    }

    #[test]
    fn replay_mengukur_galat_tanpa_mengubah_versi() {
        let store = store("replay");
        for id in 0..30 {
            let record = complete_record(id, 1);
            store
                .put(&record_key(&record.position_id), &record)
                .unwrap();
        }
        let before = active_version(&store, Regime::Sideways).unwrap();
        let report = replay(&store, Some("baseline-v1-sideways")).unwrap();
        assert_eq!(report.positions, 30);
        assert!(report.median_fee_error_percent.is_some());
        assert_eq!(active_version(&store, Regime::Sideways).unwrap(), before);
    }

    #[test]
    fn pencatatan_buka_idempoten() {
        let store = store("open");
        let draft = Draft {
            feature: feature(Regime::Sideways, "baseline-v1-sideways"),
        };
        assert!(record_open(
            &store,
            "CHAIN:MANAGER:1",
            "2026-01-01T00:00:00Z",
            draft.clone()
        )
        .unwrap());
        assert!(!record_open(&store, "chain:manager:1", "2026-01-01T00:00:00Z", draft).unwrap());
    }

    #[test]
    fn monitor_memasangkan_close_dan_mengukur_waktu_dalam_range() {
        let store = store("close");
        let mut record = complete_record(1, 1);
        record.outcome = None;
        record.opened_at = "2026-01-01T00:00:00Z".into();
        record.last_observed_at = "2026-01-01T00:58:00Z".into();
        record.last_in_range = true;
        record.in_range_seconds = 1800;
        store
            .put(&record_key(&record.position_id), &record)
            .unwrap();
        let closed = PosisiTertutup {
            position_id: record.position_id.clone(),
            token0: "0x0".into(),
            token1: "0x1".into(),
            tx_hash: "0xtx".into(),
            ditutup_pada: "2026-01-01T01:00:00Z".into(),
            diterima0: "1".into(),
            diterima1: "2".into(),
            bukti_penerimaan: "uji".into(),
            pnl: Some(Hasil {
                currency: crate::pnl::Currency {
                    id: "quote".into(),
                    symbol: "Q".into(),
                },
                fee: Some(0.2),
                impermanent_loss: 0.05,
                gas: Some(0.01),
                slippage: Some(0.0),
                net: Some(0.14),
                perkiraan: false,
            }),
            error: None,
        };
        let now = DateTime::parse_from_rfc3339("2026-01-01T01:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(synchronize(&store, &[], &[closed], now).unwrap(), 1);
        let saved: PositionRecord = store
            .get(&record_key(&record.position_id))
            .unwrap()
            .unwrap();
        let outcome = saved.outcome.unwrap();
        assert!((outcome.hours_in_range - 32.0 / 60.0).abs() < 1e-9);
        assert_eq!(outcome.realized_fee, Some(0.2));
        assert_eq!(outcome.net, Some(0.14));
    }

    #[test]
    fn replay_candle_historis_menghasilkan_sampel_tanpa_menulis_state() {
        let store = store("bars");
        let bars = (0..420)
            .map(|index| {
                let close = 100.0 + ((index % 20) as f64 - 10.0) * 0.1;
                Bar {
                    time: index * 60,
                    open: if index % 2 == 0 {
                        close - 0.1
                    } else {
                        close + 0.1
                    },
                    high: close + 0.25,
                    low: close - 0.25,
                    close,
                    volume: 1000.0 + (index % 7) as f64 * 10.0,
                }
            })
            .collect();
        let report = replay_bars(&store, bars, None).unwrap();
        assert!(report.samples > 0);
        assert!(report.median_exit_time_error_percent.is_some());
        assert_eq!(view(&store, &[], &[]).unwrap().records, 0);
    }
}
