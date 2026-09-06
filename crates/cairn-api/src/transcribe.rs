//! ElevenLabs Scribe.
//!
//! Transcription is best-effort by design. §12 fixes the behaviour: if this
//! fails, the receipt is still computed, with an empty transcript, and is
//! marked transcript-unavailable. A vendor outage must not stop a recipient
//! from being paid.

use anyhow::{Context, Result};

const ENDPOINT: &str = "https://api.elevenlabs.io/v1/speech-to-text";
const MODEL: &str = "scribe_v1";

pub struct Transcript {
    pub text: String,
    pub locale: String,
}

#[derive(Clone)]
pub struct Transcriber {
    client: reqwest::Client,
    api_key: Option<String>,
}

impl Transcriber {
    pub fn new(api_key: Option<String>) -> Self {
        if api_key.is_none() {
            tracing::warn!(
                "ELEVENLABS_API_KEY is unset; receipts will be issued with empty transcripts"
            );
        }
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .expect("reqwest client"),
            api_key,
        }
    }

    pub fn enabled(&self) -> bool {
        self.api_key.is_some()
    }

    /// `Ok(None)` = no API key configured. `Err` = the call was attempted and
    /// failed; callers log it and fall through to the empty-transcript path.
    pub async fn transcribe(
        &self,
        audio: Vec<u8>,
        filename: &str,
        content_type: &str,
    ) -> Result<Option<Transcript>> {
        let Some(key) = &self.api_key else { return Ok(None) };

        let part = reqwest::multipart::Part::bytes(audio)
            .file_name(filename.to_string())
            .mime_str(content_type.split(';').next().unwrap_or("audio/webm").trim())
            .context("audio content type was not a usable MIME type")?;

        let form = reqwest::multipart::Form::new().text("model_id", MODEL).part("file", part);

        let res = self
            .client
            .post(ENDPOINT)
            .header("xi-api-key", key)
            .multipart(form)
            .send()
            .await
            .context("speech-to-text request failed")?;

        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        anyhow::ensure!(status.is_success(), "scribe returned {status}: {body}");

        let parsed: serde_json::Value =
            serde_json::from_str(&body).context("scribe returned a non-JSON body")?;

        Ok(Some(Transcript {
            text: parsed["text"].as_str().unwrap_or_default().to_string(),
            locale: parsed["language_code"].as_str().unwrap_or("und").to_string(),
        }))
    }
}
