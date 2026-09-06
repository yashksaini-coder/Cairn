//! Audio storage.
//!
//! Integrity does not live here -- it lives in the on-chain `receipt_hash`.
//! That is what makes this component swappable, and why a missing blob
//! degrades a verification to `audio_available: false` rather than to a
//! failure (§8.4). Losing the bucket loses convenience, not evidence.

use crate::config::BlobConfig;
use anyhow::{Context, Result};
use std::path::PathBuf;

#[derive(Clone)]
pub enum BlobStore {
    Local {
        dir: PathBuf,
        public_base: String,
    },
    #[cfg(feature = "s3")]
    S3 {
        client: aws_sdk_s3::Client,
        bucket: String,
        public_base: Option<String>,
    },
}

impl BlobStore {
    pub async fn new(cfg: &BlobConfig, public_base_url: &str) -> Result<Self> {
        match cfg {
            BlobConfig::Local { dir } => {
                let dir = PathBuf::from(dir);
                tokio::fs::create_dir_all(&dir)
                    .await
                    .with_context(|| format!("could not create blob dir {}", dir.display()))?;
                Ok(BlobStore::Local {
                    dir,
                    public_base: public_base_url.trim_end_matches('/').into(),
                })
            }
            #[cfg(feature = "s3")]
            BlobConfig::S3 { bucket, endpoint, region, public_base } => {
                let shared = aws_config::defaults(aws_config::BehaviorVersion::latest())
                    .region(aws_sdk_s3::config::Region::new(region.clone()))
                    .endpoint_url(endpoint)
                    .load()
                    .await;
                let client = aws_sdk_s3::Client::from_conf(
                    aws_sdk_s3::config::Builder::from(&shared).force_path_style(true).build(),
                );
                Ok(BlobStore::S3 {
                    client,
                    bucket: bucket.clone(),
                    public_base: public_base.clone(),
                })
            }
            #[cfg(not(feature = "s3"))]
            BlobConfig::S3 { .. } => anyhow::bail!(
                "R2_BUCKET is set but cairn-api was built without the `s3` feature; \
                 rebuild with --features s3 or unset R2_BUCKET to use local storage"
            ),
        }
    }

    /// Objects are keyed by their own SHA-256, so the store is
    /// content-addressed: re-uploading identical audio is idempotent, and a
    /// key can never point at bytes that hash to something else.
    pub fn key_for(audio_sha256: &[u8; 32], content_type: &str) -> String {
        format!("audio/{}.{}", hex::encode(audio_sha256), extension_for(content_type))
    }

    pub async fn put(&self, key: &str, bytes: Vec<u8>, content_type: &str) -> Result<()> {
        match self {
            BlobStore::Local { dir, .. } => {
                // The local store has nowhere to record a content type. It
                // does not need one: keys carry the extension, and `mime_for`
                // maps it back on the way out.
                let _ = content_type;
                let path = dir.join(key);
                if let Some(parent) = path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::write(&path, &bytes)
                    .await
                    .with_context(|| format!("writing blob {}", path.display()))?;
                Ok(())
            }
            #[cfg(feature = "s3")]
            BlobStore::S3 { client, bucket, .. } => {
                client
                    .put_object()
                    .bucket(bucket)
                    .key(key)
                    .content_type(content_type)
                    .body(bytes.into())
                    .send()
                    .await
                    .context("uploading to the blob bucket")?;
                Ok(())
            }
        }
    }

    /// `Ok(None)` means the object is genuinely absent. Anything else is an
    /// error, because the verifier must not report a transport failure as a
    /// missing recording.
    pub async fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        match self {
            BlobStore::Local { dir, .. } => match tokio::fs::read(dir.join(key)).await {
                Ok(bytes) => Ok(Some(bytes)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e).context("reading blob"),
            },
            #[cfg(feature = "s3")]
            BlobStore::S3 { client, bucket, .. } => {
                match client.get_object().bucket(bucket).key(key).send().await {
                    Ok(out) => Ok(Some(out.body.collect().await?.into_bytes().to_vec())),
                    Err(e) if matches!(e.as_service_error(), Some(err) if err.is_no_such_key()) => {
                        Ok(None)
                    }
                    Err(e) => Err(e).context("fetching from the blob bucket"),
                }
            }
        }
    }

    pub fn public_url(&self, key: &str) -> String {
        match self {
            BlobStore::Local { public_base, .. } => format!("{public_base}/v1/blobs/{key}"),
            #[cfg(feature = "s3")]
            BlobStore::S3 { public_base, bucket, .. } => match public_base {
                Some(base) => format!("{}/{key}", base.trim_end_matches('/')),
                None => format!("s3://{bucket}/{key}"),
            },
        }
    }
}

fn extension_for(content_type: &str) -> &'static str {
    // Browsers hand back whatever MediaRecorder chose, usually with codec
    // parameters attached, so match on the prefix rather than the full string.
    match content_type.split(';').next().unwrap_or("").trim() {
        "audio/webm" | "video/webm" => "webm",
        "audio/ogg" | "audio/opus" => "ogg",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/mp4" | "audio/x-m4a" => "m4a",
        "audio/wav" | "audio/x-wav" => "wav",
        _ => "bin",
    }
}
