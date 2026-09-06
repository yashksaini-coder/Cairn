use std::{env, net::SocketAddr, time::Duration};

use anyhow::{Context, Result};
use solana_program::pubkey::Pubkey;
use std::str::FromStr;

/// Everything the service needs, read once at boot.
///
/// Note what is absent: there is no private key, no keypair path, no signer
/// of any kind. That is not an oversight to be fixed later -- it is KPI P6/P7,
/// and it is the reason a compromise of this process cannot move funds.
#[derive(Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub database_url: String,
    pub rpc_url: String,
    pub program_id: Pubkey,
    /// Absent means transcription is skipped and receipts are issued with an
    /// empty transcript (§12). The pipeline stays usable without a vendor.
    pub elevenlabs_api_key: Option<String>,
    pub blob: BlobConfig,
    pub public_base_url: String,
    pub index_interval: Duration,
    pub max_audio_bytes: usize,
    pub min_audio_bytes: usize,
    pub max_audio_ms: i64,
    pub min_audio_ms: i64,
    pub challenge_ttl: Duration,
}

#[derive(Clone, Debug)]
pub enum BlobConfig {
    /// Default. Works with no credentials, which matters because the demo
    /// must be runnable before anyone provisions a bucket.
    Local { dir: String },
    /// Cloudflare R2 or any S3-compatible endpoint. Requires the `s3` feature.
    S3 { bucket: String, endpoint: String, region: String, public_base: Option<String> },
}

fn var(key: &str) -> Option<String> {
    env::var(key).ok().filter(|v| !v.trim().is_empty())
}

fn parsed<T: FromStr>(key: &str, default: T) -> Result<T>
where
    T::Err: std::fmt::Display,
{
    match var(key) {
        None => Ok(default),
        Some(v) => v.parse().map_err(|e| anyhow::anyhow!("{key} is not valid: {e}")),
    }
}

// Hand-written so the ElevenLabs key cannot reach a log line through a
// stray `{:?}`. Deriving Debug on a struct that holds a secret is how
// secrets end up in log aggregators.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("bind", &self.bind)
            .field("rpc_url", &self.rpc_url)
            .field("program_id", &self.program_id)
            .field("blob", &self.blob)
            .field("public_base_url", &self.public_base_url)
            .field("index_interval", &self.index_interval)
            .field("elevenlabs_api_key", &self.elevenlabs_api_key.as_ref().map(|_| "<redacted>"))
            .finish_non_exhaustive()
    }
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let program_id = var("CAIRN_PROGRAM_ID")
            .context("CAIRN_PROGRAM_ID is required (run `anchor keys sync` and copy it here)")?;

        let blob = match var("R2_BUCKET") {
            Some(bucket) => BlobConfig::S3 {
                bucket,
                endpoint: var("R2_ENDPOINT").context("R2_ENDPOINT is required with R2_BUCKET")?,
                region: var("R2_REGION").unwrap_or_else(|| "auto".into()),
                public_base: var("R2_PUBLIC_BASE"),
            },
            None => BlobConfig::Local { dir: var("BLOB_DIR").unwrap_or_else(|| "./blobs".into()) },
        };

        Ok(Self {
            bind: parsed("BIND", SocketAddr::from(([0, 0, 0, 0], 8080)))?,
            database_url: var("DATABASE_URL")
                .unwrap_or_else(|| "sqlite://cairn.db?mode=rwc".into()),
            rpc_url: var("SOLANA_RPC_URL")
                .unwrap_or_else(|| "https://api.devnet.solana.com".into()),
            program_id: Pubkey::from_str(&program_id)
                .with_context(|| format!("CAIRN_PROGRAM_ID is not a pubkey: {program_id}"))?,
            elevenlabs_api_key: var("ELEVENLABS_API_KEY"),
            blob,
            public_base_url: var("PUBLIC_BASE_URL")
                .unwrap_or_else(|| "http://localhost:8080".into()),
            // KPI P4 wants read consistency within 5s of confirmation.
            index_interval: Duration::from_secs(parsed("INDEX_INTERVAL_SECS", 3u64)?),
            max_audio_bytes: parsed("MAX_AUDIO_BYTES", 10 * 1024 * 1024usize)?,
            // A webm/opus header alone is a few hundred bytes, so anything
            // this small cannot contain three seconds of speech. See the note
            // in routes::receipts about why duration is checked twice.
            min_audio_bytes: parsed("MIN_AUDIO_BYTES", 4096usize)?,
            max_audio_ms: parsed("MAX_AUDIO_MS", 120_000i64)?,
            min_audio_ms: parsed("MIN_AUDIO_MS", 3_000i64)?,
            challenge_ttl: Duration::from_secs(parsed("CHALLENGE_TTL_SECS", 120u64)?),
        })
    }
}
