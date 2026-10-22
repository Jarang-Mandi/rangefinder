//! Adapter resmi Uniswap Liquidity API.
//!
//! API ini dipakai sebagai jalur cepat untuk membaca state pool yang sudah
//! ditemukan dan untuk merakit transaksi LP. Penemuan pool, verifikasi state
//! sebelum eksekusi, simulasi, tanda tangan, broadcast, serta pemeriksaan
//! receipt tetap dilakukan engine. Dengan begitu kegagalan atau data basi dari
//! layanan HTTP tidak pernah menjadi alasan melewati pemeriksaan on-chain.
//!
//! Kunci API hanya disimpan di Windows Credential Manager dan hanya hidup di
//! proses engine. Ia tidak pernah masuk config.toml, status, log, atau kode
//! extension.

use std::collections::HashMap;
use std::time::Duration;

use alloy::primitives::{Address, Bytes, U256};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use zeroize::Zeroizing;

use crate::pools::{PoolInfo, PoolStats, PoolVersion, DYNAMIC_FEE_FLAG};

const BASE_URL: &str = "https://liquidity.api.uniswap.org";
const DATA_POOLS_URL: &str =
    "https://entry-gateway.backend-prod.api.uniswap.org/data.v2.DataApiService/ListPools";
const CREDENTIAL_SERVICE: &str = "Rangefinder Uniswap API";
const CREDENTIAL_USER: &str = "liquidity-api-key";
const MAX_KEY_LEN: usize = 512;
const MAX_REFERENCES: usize = 20;
const DATA_POOL_LIMIT: usize = 100;

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub configured: bool,
    /// `None` berarti belum ada permintaan sejak engine hidup.
    pub connected: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct PoolState {
    pub id: String,
    pub sqrt_price_x96: U256,
    pub tick: i32,
    pub liquidity: u128,
    pub reserve0: Option<U256>,
    pub reserve1: Option<U256>,
}

/// Metadata token yang ikut dalam respons indeks pool Uniswap.
#[derive(Debug, Clone)]
pub struct IndexedToken {
    pub address: String,
    pub symbol: String,
    pub decimals: u8,
}

/// Snapshot pool lengkap dari Data API yang sama dengan tabel Explore
/// Uniswap. State pool dan statistik USD tiba dalam satu permintaan.
#[derive(Debug, Clone)]
pub struct IndexedPools {
    pub pools: Vec<PoolInfo>,
    pub tokens: HashMap<String, IndexedToken>,
}

impl PoolState {
    /// Ubah cadangan kedua token menjadi satuan manusiawi token kutipan.
    /// `raw_price` adalah token1 mentah per token0 mentah, sehingga faktor
    /// desimal saling menghapus saat kedua cadangan dijumlahkan.
    pub fn tvl_dalam_kutipan(
        &self,
        quote_token1: bool,
        quote_decimals: u8,
        raw_price: f64,
    ) -> Option<f64> {
        let reserve0 = self.reserve0?.to_string().parse::<f64>().ok()?;
        let reserve1 = self.reserve1?.to_string().parse::<f64>().ok()?;
        if !raw_price.is_finite() || raw_price <= 0.0 {
            return None;
        }
        let raw_quote = if quote_token1 {
            reserve1 + reserve0 * raw_price
        } else {
            reserve0 + reserve1 / raw_price
        };
        let tvl = raw_quote / 10f64.powi(i32::from(quote_decimals));
        (tvl.is_finite() && tvl > 0.0).then_some(tvl)
    }
}

#[derive(Debug, Clone)]
pub struct LpToken {
    pub address: Address,
    pub amount: U256,
}

#[derive(Debug, Clone)]
pub struct LpTransaction {
    pub to: Address,
    pub from: Address,
    pub data: Bytes,
    pub value: U256,
    pub chain_id: u64,
}

impl LpTransaction {
    /// Periksa batas yang dapat dibuktikan tanpa mempercayai payload API.
    pub fn validate(&self, chain_id: u64, wallet: Address, target: Address) -> Result<()> {
        if self.chain_id != chain_id {
            bail!(
                "transaksi Uniswap untuk chain {}, bukan {chain_id}",
                self.chain_id
            );
        }
        if self.from != wallet {
            bail!("pengirim transaksi Uniswap berbeda dari wallet aktif");
        }
        if self.to != target {
            bail!("tujuan transaksi Uniswap bukan PositionManager yang dipin");
        }
        if self.data.len() < 4 {
            bail!("calldata transaksi Uniswap kosong");
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct LpBuild {
    pub request_id: String,
    pub token0: Option<LpToken>,
    pub token1: Option<LpToken>,
    pub tick_lower: Option<i32>,
    pub tick_upper: Option<i32>,
    pub transaction: LpTransaction,
}

#[derive(Debug, Clone)]
pub struct CreateParams {
    pub wallet: Address,
    pub chain_id: u64,
    pub pool: PoolInfo,
    pub amount0: U256,
    pub amount1: U256,
    pub tick_lower: i32,
    pub tick_upper: i32,
    pub deadline: u64,
    pub slippage_bps: u32,
    pub native_balance: Option<U256>,
}

#[derive(Debug, Clone)]
pub struct IncreaseParams {
    pub wallet: Address,
    pub chain_id: u64,
    pub version: PoolVersion,
    pub token0: Address,
    pub token1: Address,
    pub token_id: U256,
    pub amount0: U256,
    pub amount1: U256,
    pub deadline: u64,
    pub slippage_bps: u32,
}

#[derive(Debug, Clone)]
pub struct PositionParams {
    pub wallet: Address,
    pub chain_id: u64,
    pub version: PoolVersion,
    pub token0: Address,
    pub token1: Address,
    pub token_id: U256,
    pub deadline: u64,
    pub slippage_bps: u32,
}

pub struct Client {
    http: reqwest::Client,
    api_key: RwLock<Option<Zeroizing<String>>>,
    connected: RwLock<Option<bool>>,
}

impl Client {
    pub fn load() -> Result<Self> {
        let api_key = match credential_entry() {
            Ok(entry) => match entry.get_password() {
                Ok(value) => Some(Zeroizing::new(value)),
                Err(keyring::Error::NoEntry) => None,
                Err(error) => {
                    tracing::warn!(%error, "kunci API Uniswap tidak dapat dibaca");
                    None
                }
            },
            Err(error) => {
                tracing::warn!(%error, "Windows Credential Manager tidak tersedia untuk Uniswap");
                None
            }
        };
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(6))
            .build()
            .context("HTTP client Uniswap tidak dapat dibuat")?;
        Ok(Self {
            http,
            api_key: RwLock::new(api_key),
            connected: RwLock::new(None),
        })
    }

    #[cfg(test)]
    fn with_key(key: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_key: RwLock::new(Some(Zeroizing::new(key.to_owned()))),
            connected: RwLock::new(None),
        }
    }

    pub async fn configured(&self) -> bool {
        self.api_key.read().await.is_some()
    }

    pub async fn status(&self) -> Status {
        Status {
            configured: self.configured().await,
            connected: *self.connected.read().await,
        }
    }

    pub async fn set_api_key(&self, key: &str) -> Result<()> {
        let key = key.trim();
        if key.is_empty() {
            bail!("API key Uniswap kosong");
        }
        if key.len() > MAX_KEY_LEN || key.chars().any(char::is_whitespace) {
            bail!("API key Uniswap tidak valid");
        }
        credential_entry()?
            .set_password(key)
            .context("API key tidak dapat disimpan di Windows Credential Manager")?;
        *self.api_key.write().await = Some(Zeroizing::new(key.to_owned()));
        *self.connected.write().await = None;
        Ok(())
    }

    pub async fn forget_api_key(&self) -> Result<()> {
        match credential_entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(error) => {
                return Err(error)
                    .context("API key tidak dapat dihapus dari Windows Credential Manager")
            }
        }
        *self.api_key.write().await = None;
        *self.connected.write().await = None;
        Ok(())
    }

    /// Ambil pool, state terbaru, TVL, volume 1D/30D, dan APR dari indeks yang
    /// dipakai halaman Explore Uniswap. Endpoint ini tidak memakai API key
    /// Liquidity; satu request menggantikan discovery log dan pengukuran log
    /// tiga jam/24 jam pada jalur normal.
    pub async fn indexed_pools(&self, token: Address, chain_id: u64) -> Result<IndexedPools> {
        let body = json!({
            "chainIds": [chain_id],
            "sort": {
                "orderBy": "POOLS_ORDER_BY_TVL",
                "ascending": false,
            },
            "filter": {
                "protocolVersions": ["PROTOCOL_VERSION_V3", "PROTOCOL_VERSION_V4"],
                "includeSpam": false,
                "tokenFilter": {
                    "tokens": [token.to_string()],
                    "logicalOperator": "POOL_TOKEN_LOGICAL_OPERATOR_OR",
                },
            },
            "page": { "pageSize": DATA_POOL_LIMIT },
        });
        let started = std::time::Instant::now();
        let response = self
            .http
            .post(DATA_POOLS_URL)
            .header("connect-protocol-version", "1")
            .header("x-request-source", "rangefinder")
            .header("accept", "application/json")
            .json(&body)
            .send()
            .await
            .context("Data API Uniswap tidak dapat dihubungi")?;
        let status = response.status();
        let text = response
            .text()
            .await
            .context("respons Data API Uniswap tidak dapat dibaca")?;
        if !status.is_success() {
            bail!("Data API Uniswap {status}: {}", api_error_detail(&text));
        }
        let response: DataPoolsResponse =
            serde_json::from_str(&text).context("respons pool Data API Uniswap tidak valid")?;
        let indexed = parse_indexed_pools(response, chain_id)?;
        tracing::info!(
            durasi_ms = started.elapsed().as_millis(),
            pools = indexed.pools.len(),
            "pool dan statistik Uniswap siap"
        );
        Ok(indexed)
    }

    /// Ambil state pool yang sudah diketahui. Referensi dipotong maksimal 20
    /// per permintaan sesuai batas resmi dan V3/V4 tidak dicampur.
    pub async fn pool_states(&self, pools: &[PoolInfo], chain_id: u64) -> Result<Vec<PoolState>> {
        if pools.is_empty() {
            return Ok(Vec::new());
        }
        let mut requests = Vec::new();
        for version in [PoolVersion::V3, PoolVersion::V4] {
            let refs: Vec<&PoolInfo> = pools
                .iter()
                .filter(|pool| pool.version == version)
                .collect();
            for chunk in refs.chunks(MAX_REFERENCES) {
                requests.push(pool_request(version, chunk, chain_id));
            }
        }

        let hasil = futures_util::future::join_all(
            requests
                .iter()
                .map(|body| self.post_value("/lp/pool_info", body)),
        )
        .await;

        let known: HashMap<String, &PoolInfo> = pools
            .iter()
            .map(|pool| (pool.id.to_lowercase(), pool))
            .collect();
        let mut out = Vec::new();
        let mut errors = Vec::new();
        for result in hasil {
            match result {
                Ok(value) => {
                    let entries = value
                        .get("pools")
                        .and_then(Value::as_array)
                        .ok_or_else(|| anyhow::anyhow!("respons pool_info tidak punya pools"))?;
                    for entry in entries {
                        match parse_pool_state(entry, &known, chain_id) {
                            Ok(state) => out.push(state),
                            Err(error) => errors.push(error.to_string()),
                        }
                    }
                }
                Err(error) => errors.push(error.to_string()),
            }
        }
        if out.is_empty() && !errors.is_empty() {
            bail!("{}", errors.join("; "));
        }
        if !errors.is_empty() {
            tracing::warn!(gagal = errors.len(), "sebagian state pool Uniswap ditolak");
        }
        Ok(out)
    }

    pub async fn create_position(&self, params: &CreateParams) -> Result<LpBuild> {
        let token0: Address = params
            .pool
            .token0
            .parse()
            .context("token0 pool bukan alamat")?;
        let token1: Address = params
            .pool
            .token1
            .parse()
            .context("token1 pool bukan alamat")?;
        let (independent, dependent) =
            independent_tokens(token0, params.amount0, token1, params.amount1)?;
        let mut body = json!({
            "walletAddress": params.wallet.to_string(),
            "protocol": protocol(params.pool.version),
            "chainId": params.chain_id,
            "existingPool": {
                "token0Address": token0.to_string(),
                "token1Address": token1.to_string(),
                "poolReference": params.pool.id,
            },
            "independentToken": lp_token_json(&independent),
            "tickBounds": {
                "tickLower": params.tick_lower,
                "tickUpper": params.tick_upper,
            },
            "slippageTolerance": slippage_percent(params.slippage_bps),
            "deadline": params.deadline,
            "simulateTransaction": false,
        });
        if let Some(dependent) = dependent.filter(|token| !token.amount.is_zero()) {
            body["dependentToken"] = lp_token_json(&dependent);
        }
        if let Some(balance) = params.native_balance {
            body["nativeTokenBalance"] = Value::String(balance.to_string());
        }
        let value = self.post_value("/lp/create", &body).await?;
        parse_lp_build(&value, "create")
    }

    pub async fn increase_position(&self, params: &IncreaseParams) -> Result<LpBuild> {
        let (independent, _) =
            independent_tokens(params.token0, params.amount0, params.token1, params.amount1)?;
        let body = json!({
            "walletAddress": params.wallet.to_string(),
            "chainId": params.chain_id,
            "protocol": protocol(params.version),
            "token0Address": params.token0.to_string(),
            "token1Address": params.token1.to_string(),
            "nftTokenId": params.token_id.to_string(),
            "independentToken": lp_token_json(&independent),
            "slippageTolerance": slippage_percent(params.slippage_bps),
            "deadline": params.deadline,
            "simulateTransaction": false,
        });
        let value = self.post_value("/lp/increase", &body).await?;
        parse_lp_build(&value, "increase")
    }

    pub async fn decrease_position(&self, params: &PositionParams) -> Result<LpBuild> {
        let body = json!({
            "walletAddress": params.wallet.to_string(),
            "chainId": params.chain_id,
            "protocol": protocol(params.version),
            "token0Address": params.token0.to_string(),
            "token1Address": params.token1.to_string(),
            "nftTokenId": params.token_id.to_string(),
            "liquidityPercentageToDecrease": 100,
            "slippageTolerance": slippage_percent(params.slippage_bps),
            "deadline": params.deadline,
            "simulateTransaction": false,
        });
        let value = self.post_value("/lp/decrease", &body).await?;
        parse_lp_build(&value, "decrease")
    }

    pub async fn claim_fees(&self, params: &PositionParams) -> Result<LpBuild> {
        let body = json!({
            "protocol": protocol(params.version),
            "walletAddress": params.wallet.to_string(),
            "chainId": params.chain_id,
            "tokenId": params.token_id.to_string(),
            "simulateTransaction": false,
        });
        let value = self.post_value("/lp/claim_fees", &body).await?;
        parse_lp_build(&value, "claim")
    }

    async fn post_value(&self, path: &str, body: &Value) -> Result<Value> {
        let key = self
            .api_key
            .read()
            .await
            .as_ref()
            .cloned()
            .context("API key Uniswap belum dipasang")?;
        let url = format!("{BASE_URL}{path}");
        let mut last_error = None;
        for attempt in 0..3u64 {
            let response = self
                .http
                .post(&url)
                .header("x-api-key", key.as_str())
                .header("accept", "application/json")
                .header(
                    "x-agent-info",
                    r#"{"decision_origin":"human_mediated","integration_name":"Rangefinder"}"#,
                )
                .json(body)
                .send()
                .await;
            match response {
                Ok(response) => {
                    let status = response.status();
                    let text = response
                        .text()
                        .await
                        .context("respons Uniswap tidak dapat dibaca")?;
                    if status.is_success() {
                        let value = serde_json::from_str(&text)
                            .context("respons Uniswap bukan JSON yang valid")?;
                        *self.connected.write().await = Some(true);
                        return Ok(value);
                    }
                    let detail = api_error_detail(&text);
                    let error = anyhow::anyhow!("Uniswap API {status}: {detail}");
                    if !(status.as_u16() == 429 || status.is_server_error()) {
                        *self.connected.write().await = Some(false);
                        return Err(error);
                    }
                    last_error = Some(error);
                }
                Err(error) => last_error = Some(anyhow::anyhow!(error.to_string())),
            }
            if attempt < 2 {
                tokio::time::sleep(Duration::from_millis(150 * (attempt + 1))).await;
            }
        }
        *self.connected.write().await = Some(false);
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Uniswap API tidak menjawab")))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DataPoolsResponse {
    #[serde(default)]
    pools: Vec<DataRankedPool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DataRankedPool {
    pool: DataPool,
    stats: Option<DataPoolStats>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DataPool {
    chain_id: u64,
    pool_id: String,
    token0: DataToken,
    token1: DataToken,
    protocol_version: String,
    tick_spacing: Option<i32>,
    fee_tier: Option<u32>,
    #[serde(default)]
    is_dynamic_fee: bool,
    hook_address: Option<String>,
    current_tick: Option<i32>,
    sqrt_price_x96: Option<String>,
    liquidity: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DataToken {
    address: String,
    symbol: String,
    decimals: u8,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DataPoolStats {
    tvl: Option<f64>,
    volume1d: Option<f64>,
    volume30d: Option<f64>,
    apr: Option<f64>,
}

fn parse_indexed_pools(response: DataPoolsResponse, chain_id: u64) -> Result<IndexedPools> {
    let mut pools = Vec::with_capacity(response.pools.len());
    let mut tokens = HashMap::new();
    let mut rejected = 0usize;
    for ranked in response.pools {
        let data = ranked.pool;
        let indexed_tokens = [data.token0.clone(), data.token1.clone()];
        let parsed = (|| -> Result<PoolInfo> {
            anyhow::ensure!(data.chain_id == chain_id, "chain pool berbeda");
            let version = match data.protocol_version.as_str() {
                "PROTOCOL_VERSION_V3" => PoolVersion::V3,
                "PROTOCOL_VERSION_V4" => PoolVersion::V4,
                _ => bail!("versi pool tidak didukung"),
            };
            let sqrt: U256 = data
                .sqrt_price_x96
                .as_deref()
                .context("sqrtPriceX96 kosong")?
                .parse()
                .context("sqrtPriceX96 bukan uint256")?;
            let liquidity: u128 = data
                .liquidity
                .as_deref()
                .context("liquidity kosong")?
                .parse()
                .context("liquidity bukan uint128")?;
            anyhow::ensure!(!sqrt.is_zero() && liquidity > 0, "state pool kosong");
            let dynamic_fee = data.is_dynamic_fee;
            let fee = if dynamic_fee {
                DYNAMIC_FEE_FLAG
            } else {
                data.fee_tier.unwrap_or_default()
            };
            let stats = ranked.stats.map(|stats| PoolStats {
                volume: valid_metric(stats.volume1d),
                volume_30d: valid_metric(stats.volume30d),
                jendela_menit: valid_metric(stats.volume1d).map(|_| 1_440.0),
                tvl: valid_metric(stats.tvl),
                apr: valid_metric(stats.apr),
                dalam_usd: true,
            });
            Ok(PoolInfo {
                version,
                id: data.pool_id,
                token0: data.token0.address.clone(),
                token1: data.token1.address.clone(),
                fee,
                dynamic_fee,
                tick_spacing: data.tick_spacing.context("tick spacing kosong")?,
                hooks: (version == PoolVersion::V4).then(|| {
                    data.hook_address
                        .unwrap_or_else(|| Address::ZERO.to_string())
                }),
                sqrt_price_x96: sqrt.to_string(),
                tick: data.current_tick.context("tick pool kosong")?,
                liquidity: liquidity.to_string(),
                raw_price: crate::pools::raw_price_from_sqrt_x96(sqrt),
                stats,
            })
        })();

        match parsed {
            Ok(pool) => {
                for token in indexed_tokens {
                    tokens
                        .entry(token.address.to_lowercase())
                        .or_insert(IndexedToken {
                            address: token.address,
                            symbol: token.symbol,
                            decimals: token.decimals,
                        });
                }
                pools.push(pool);
            }
            Err(error) => {
                rejected += 1;
                tracing::debug!(%error, "pool Data API Uniswap dilewati");
            }
        }
    }
    if pools.is_empty() && rejected > 0 {
        bail!("seluruh pool Data API Uniswap tidak memiliki state yang dapat dipakai");
    }
    Ok(IndexedPools { pools, tokens })
}

fn valid_metric(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite() && *value >= 0.0)
}

fn credential_entry() -> Result<keyring::Entry> {
    keyring::Entry::new(CREDENTIAL_SERVICE, CREDENTIAL_USER)
        .context("Windows Credential Manager tidak tersedia")
}

fn protocol(version: PoolVersion) -> &'static str {
    match version {
        PoolVersion::V3 => "V3",
        PoolVersion::V4 => "V4",
    }
}

fn pool_request(version: PoolVersion, pools: &[&PoolInfo], chain_id: u64) -> Value {
    json!({
        "protocol": protocol(version),
        "poolReferences": pools.iter().map(|pool| json!({
            "protocol": protocol(version),
            "chainId": chain_id,
            "referenceIdentifier": pool.id,
        })).collect::<Vec<_>>(),
        "chainId": chain_id,
    })
}

fn parse_pool_state(
    value: &Value,
    known: &HashMap<String, &PoolInfo>,
    chain_id: u64,
) -> Result<PoolState> {
    let id = text_field(value, "poolReferenceIdentifier")?;
    let pool = known
        .get(&id.to_lowercase())
        .context("pool_info mengembalikan pool yang tidak diminta")?;
    let returned_chain = integer_field(value, "chainId")?;
    if returned_chain != chain_id {
        bail!("pool_info mengembalikan chain yang berbeda");
    }
    let returned_protocol = text_field(value, "poolProtocol")?;
    if !returned_protocol.eq_ignore_ascii_case(protocol(pool.version)) {
        bail!("pool_info mengembalikan versi pool yang berbeda");
    }
    let a = text_field(value, "tokenAddressA")?;
    let b = text_field(value, "tokenAddressB")?;
    let pair_ok = (a.eq_ignore_ascii_case(&pool.token0) && b.eq_ignore_ascii_case(&pool.token1))
        || (a.eq_ignore_ascii_case(&pool.token1) && b.eq_ignore_ascii_case(&pool.token0));
    if !pair_ok {
        bail!("pool_info mengembalikan pasangan token yang berbeda");
    }
    if let Some(fee) = optional_integer_field(value, "fee")? {
        if fee != u64::from(pool.fee) {
            bail!("pool_info mengembalikan fee yang berbeda");
        }
    }
    if let Some(spacing) = optional_signed_field(value, "tickSpacing")? {
        if spacing != i64::from(pool.tick_spacing) {
            bail!("pool_info mengembalikan tick spacing yang berbeda");
        }
    }
    let sqrt_price_x96 = u256_field(value, "sqrtRatioX96")?;
    let liquidity = u256_field(value, "poolLiquidity")?
        .try_into()
        .map_err(|_| anyhow::anyhow!("likuiditas pool_info melebihi u128"))?;
    let tick = signed_field(value, "currentTick")?
        .try_into()
        .map_err(|_| anyhow::anyhow!("tick pool_info melebihi i32"))?;
    if sqrt_price_x96.is_zero() || liquidity == 0 {
        bail!("pool_info mengembalikan harga atau likuiditas nol");
    }
    Ok(PoolState {
        id: id.to_owned(),
        sqrt_price_x96,
        tick,
        liquidity,
        reserve0: optional_u256_field(value, "token0Reserves")?,
        reserve1: optional_u256_field(value, "token1Reserves")?,
    })
}

fn independent_tokens(
    token0: Address,
    amount0: U256,
    token1: Address,
    amount1: U256,
) -> Result<(LpToken, Option<LpToken>)> {
    if amount0.is_zero() && amount1.is_zero() {
        bail!("jumlah LP untuk API Uniswap nol");
    }
    if !amount0.is_zero() {
        Ok((
            LpToken {
                address: token0,
                amount: amount0,
            },
            Some(LpToken {
                address: token1,
                amount: amount1,
            }),
        ))
    } else {
        Ok((
            LpToken {
                address: token1,
                amount: amount1,
            },
            Some(LpToken {
                address: token0,
                amount: amount0,
            }),
        ))
    }
}

fn lp_token_json(token: &LpToken) -> Value {
    json!({
        "tokenAddress": token.address.to_string(),
        "amount": token.amount.to_string(),
    })
}

fn slippage_percent(bps: u32) -> f64 {
    f64::from(bps) / 100.0
}

fn parse_lp_build(value: &Value, transaction_field: &str) -> Result<LpBuild> {
    let transaction = value
        .get(transaction_field)
        .with_context(|| format!("respons Uniswap tidak punya {transaction_field}"))?;
    Ok(LpBuild {
        request_id: value
            .get("requestId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        token0: value.get("token0").map(parse_lp_token).transpose()?,
        token1: value.get("token1").map(parse_lp_token).transpose()?,
        tick_lower: optional_signed_field(value, "tickLower")?
            .map(i32::try_from)
            .transpose()
            .context("tickLower Uniswap melebihi i32")?,
        tick_upper: optional_signed_field(value, "tickUpper")?
            .map(i32::try_from)
            .transpose()
            .context("tickUpper Uniswap melebihi i32")?,
        transaction: parse_transaction(transaction)?,
    })
}

fn parse_lp_token(value: &Value) -> Result<LpToken> {
    Ok(LpToken {
        address: text_field(value, "tokenAddress")?
            .parse()
            .context("alamat token respons Uniswap tidak valid")?,
        amount: u256_field(value, "amount")?,
    })
}

fn parse_transaction(value: &Value) -> Result<LpTransaction> {
    let data = text_field(value, "data")?;
    let raw = data
        .strip_prefix("0x")
        .context("calldata Uniswap bukan hex")?;
    let bytes = hex::decode(raw).context("calldata Uniswap bukan hex")?;
    Ok(LpTransaction {
        to: text_field(value, "to")?
            .parse()
            .context("tujuan transaksi Uniswap tidak valid")?,
        from: text_field(value, "from")?
            .parse()
            .context("pengirim transaksi Uniswap tidak valid")?,
        data: Bytes::from(bytes),
        value: u256_field(value, "value")?,
        chain_id: integer_field(value, "chainId")?,
    })
}

fn text_field<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .with_context(|| format!("field {field} tidak ada atau bukan string"))
}

fn integer_field(value: &Value, field: &str) -> Result<u64> {
    let value = value
        .get(field)
        .with_context(|| format!("field {field} tidak ada"))?;
    if let Some(number) = value.as_u64() {
        return Ok(number);
    }
    value
        .as_str()
        .with_context(|| format!("field {field} bukan angka"))?
        .parse()
        .with_context(|| format!("field {field} bukan angka"))
}

fn optional_integer_field(value: &Value, field: &str) -> Result<Option<u64>> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => integer_field(value, field).map(Some),
    }
}

fn signed_field(value: &Value, field: &str) -> Result<i64> {
    let value = value
        .get(field)
        .with_context(|| format!("field {field} tidak ada"))?;
    if let Some(number) = value.as_i64() {
        return Ok(number);
    }
    value
        .as_str()
        .with_context(|| format!("field {field} bukan angka"))?
        .parse()
        .with_context(|| format!("field {field} bukan angka"))
}

fn optional_signed_field(value: &Value, field: &str) -> Result<Option<i64>> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => signed_field(value, field).map(Some),
    }
}

fn u256_field(value: &Value, field: &str) -> Result<U256> {
    let value = value
        .get(field)
        .with_context(|| format!("field {field} tidak ada"))?;
    let text = match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => bail!("field {field} bukan angka"),
    };
    if let Some(hex) = text.strip_prefix("0x") {
        U256::from_str_radix(hex, 16).with_context(|| format!("field {field} bukan uint256"))
    } else {
        text.parse()
            .with_context(|| format!("field {field} bukan uint256"))
    }
}

fn optional_u256_field(value: &Value, field: &str) -> Result<Option<U256>> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => u256_field(value, field).map(Some),
    }
}

fn api_error_detail(text: &str) -> String {
    let parsed: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let detail = parsed
        .get("message")
        .or_else(|| parsed.get("error"))
        .and_then(Value::as_str)
        .unwrap_or(text)
        .trim();
    detail.chars().take(240).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool() -> PoolInfo {
        PoolInfo {
            version: PoolVersion::V4,
            id: format!("0x{}", "11".repeat(32)),
            token0: Address::repeat_byte(1).to_string(),
            token1: Address::repeat_byte(2).to_string(),
            fee: 3_000,
            dynamic_fee: false,
            tick_spacing: 60,
            hooks: Some(Address::ZERO.to_string()),
            sqrt_price_x96: "1".into(),
            tick: 0,
            liquidity: "1".into(),
            raw_price: 1.0,
            stats: None,
        }
    }

    #[test]
    fn referensi_pool_memakai_skema_resmi() {
        let p = pool();
        let request = pool_request(PoolVersion::V4, &[&p], 4663);
        assert_eq!(request["protocol"], "V4");
        assert_eq!(request["chainId"], 4663);
        assert_eq!(request["poolReferences"][0]["referenceIdentifier"], p.id);
    }

    #[test]
    fn transaksi_harus_menuju_manager_yang_dipin() {
        let wallet = Address::repeat_byte(3);
        let manager = Address::repeat_byte(4);
        let tx = LpTransaction {
            to: manager,
            from: wallet,
            data: Bytes::from(vec![1, 2, 3, 4]),
            value: U256::ZERO,
            chain_id: 4663,
        };
        assert!(tx.validate(4663, wallet, manager).is_ok());
        assert!(tx.validate(4663, wallet, Address::repeat_byte(5)).is_err());
    }

    #[test]
    fn parse_pool_state_menolak_pasangan_yang_berbeda() {
        let p = pool();
        let known = HashMap::from([(p.id.to_lowercase(), &p)]);
        let mut response = json!({
            "poolReferenceIdentifier": p.id,
            "poolProtocol": "V4",
            "tokenAddressA": p.token0,
            "tokenAddressB": p.token1,
            "tickSpacing": 60,
            "fee": "3000",
            "chainId": 4663,
            "poolLiquidity": "99",
            "sqrtRatioX96": "79228162514264337593543950336",
            "currentTick": 0,
            "token0Reserves": "1000000000000000000",
            "token1Reserves": "2000000"
        });
        let state = parse_pool_state(&response, &known, 4663).unwrap();
        let tvl = state.tvl_dalam_kutipan(true, 6, 0.000_000_000_002).unwrap();
        assert!((tvl - 4.0).abs() < 1e-9);
        response["tokenAddressB"] = Value::String(Address::repeat_byte(9).to_string());
        assert!(parse_pool_state(&response, &known, 4663).is_err());
    }

    #[test]
    fn parse_indeks_pool_membawa_statistik_satu_dan_tiga_puluh_hari() {
        let swarm = "0x462dff4be800c77a61e69dc2ea6010e4237f674d";
        let usdg = "0xe8cefbef2673b4d19fc4e2c879e80d7b9d70d38c";
        let response: DataPoolsResponse = serde_json::from_value(json!({
            "pools": [{
                "pool": {
                    "chainId": 4663,
                    "poolId": format!("0x{}", "11".repeat(32)),
                    "token0": { "address": swarm, "symbol": "SWARM", "decimals": 18 },
                    "token1": { "address": usdg, "symbol": "USDG", "decimals": 18 },
                    "protocolVersion": "PROTOCOL_VERSION_V4",
                    "tickSpacing": 800,
                    "feeTier": 40000,
                    "isDynamicFee": false,
                    "hookAddress": Address::ZERO.to_string(),
                    "currentTick": -1250,
                    "sqrtPriceX96": "79228162514264337593543950336",
                    "liquidity": "987654321"
                },
                "stats": {
                    "tvl": 5652.83,
                    "volume1d": 24619.52,
                    "volume30d": 1011497.58,
                    "apr": 6352.31
                }
            }]
        }))
        .unwrap();

        let indexed = parse_indexed_pools(response, 4663).unwrap();
        assert_eq!(indexed.pools.len(), 1);
        let pool = &indexed.pools[0];
        assert_eq!(pool.version, PoolVersion::V4);
        assert_eq!(pool.fee, 40_000);
        let stats = pool.stats.as_ref().unwrap();
        assert_eq!(stats.volume, Some(24_619.52));
        assert_eq!(stats.volume_30d, Some(1_011_497.58));
        assert_eq!(stats.jendela_menit, Some(1_440.0));
        assert_eq!(stats.tvl, Some(5_652.83));
        assert_eq!(stats.apr, Some(6_352.31));
        assert!(stats.dalam_usd);
        assert_eq!(indexed.tokens[swarm].symbol, "SWARM");
    }

    #[tokio::test]
    async fn status_tidak_membuka_kunci_api() {
        let client = Client::with_key("rahasia");
        let status = client.status().await;
        assert!(status.configured);
        assert_eq!(status.connected, None);
        let serialized = serde_json::to_string(&status).unwrap();
        assert!(!serialized.contains("rahasia"));
    }
}
