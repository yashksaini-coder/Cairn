//! A deliberately small JSON-RPC client.
//!
//! `solana-rpc-client` would do this too, but it pulls a large slice of the
//! validator's type tree in to give us three methods, and the two shapes we
//! actually parse -- a base64 account and a JSON-encoded transaction -- are
//! stable parts of the public RPC surface. This file is the entire cost.

use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde_json::{json, Value};
use solana_program::pubkey::Pubkey;
use std::str::FromStr;

#[derive(Clone)]
pub struct Rpc {
    client: reqwest::Client,
    url: String,
}

pub struct ProgramAccount {
    pub pubkey: Pubkey,
    pub data: Vec<u8>,
    pub lamports: u64,
}

impl Rpc {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                .build()
                .expect("reqwest client"),
            url: url.into(),
        }
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let res: Value = self
            .client
            .post(&self.url)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("rpc {method} request failed"))?
            .json()
            .await
            .with_context(|| format!("rpc {method} returned a non-JSON body"))?;

        if let Some(err) = res.get("error") {
            bail!("rpc {method} error: {err}");
        }
        res.get("result").cloned().ok_or_else(|| anyhow!("rpc {method} returned no result"))
    }

    /// Every escrow the program owns, filtered by discriminator so a future
    /// second account type does not poison the indexer.
    pub async fn escrow_accounts(&self, program: &Pubkey) -> Result<Vec<ProgramAccount>> {
        let discriminator = bs58::encode(cairn_core::state::ESCROW_DISCRIMINATOR).into_string();
        let result = self
            .call(
                "getProgramAccounts",
                json!([program.to_string(), {
                    "encoding": "base64",
                    "commitment": "confirmed",
                    "filters": [{ "memcmp": { "offset": 0, "bytes": discriminator } }],
                }]),
            )
            .await?;

        let arr = result.as_array().ok_or_else(|| anyhow!("getProgramAccounts: expected array"))?;
        let mut out = Vec::with_capacity(arr.len());
        for entry in arr {
            let pubkey = entry["pubkey"].as_str().context("account entry without a pubkey")?;
            let encoded = entry["account"]["data"][0]
                .as_str()
                .context("account entry without base64 data")?;
            out.push(ProgramAccount {
                pubkey: Pubkey::from_str(pubkey)?,
                data: B64.decode(encoded).context("account data was not valid base64")?,
                lamports: entry["account"]["lamports"].as_u64().unwrap_or_default(),
            });
        }
        Ok(out)
    }

    /// A confirmed transaction in `json` (not `jsonParsed`) encoding.
    ///
    /// `json` keeps `accountKeys` as a flat array of base58 strings and
    /// instruction data as base58, which is exactly what the verifier needs
    /// and nothing more. `Ok(None)` means the signature is unknown to this
    /// RPC node -- distinct from a transport failure.
    pub async fn transaction(&self, signature: &str) -> Result<Option<Value>> {
        let result = self
            .call(
                "getTransaction",
                json!([signature, {
                    "encoding": "json",
                    "commitment": "confirmed",
                    "maxSupportedTransactionVersion": 0,
                }]),
            )
            .await?;
        Ok(if result.is_null() { None } else { Some(result) })
    }

    pub async fn latest_blockhash(&self) -> Result<String> {
        let result =
            self.call("getLatestBlockhash", json!([{ "commitment": "confirmed" }])).await?;
        Ok(result["value"]["blockhash"]
            .as_str()
            .context("getLatestBlockhash returned no blockhash")?
            .to_string())
    }

    /// Submit a signed, bincode-serialised transaction.
    ///
    /// Preflight is left on. This is only ever called from the operator CLI,
    /// where a clear simulation error beats a confirmed failure every time.
    pub async fn send_transaction(&self, wire: &[u8]) -> Result<String> {
        let result = self
            .call(
                "sendTransaction",
                json!([B64.encode(wire), { "encoding": "base64", "preflightCommitment": "confirmed" }]),
            )
            .await?;
        Ok(result.as_str().context("sendTransaction returned no signature")?.to_string())
    }

    /// Poll until the signature reaches `confirmed`, or give up.
    ///
    /// §12: devnet RPC times out. The caller gets the signature immediately
    /// either way, so a timeout here means "check the explorer", not "it
    /// failed".
    pub async fn confirm(&self, signature: &str, attempts: u32) -> Result<bool> {
        for i in 0..attempts {
            let result = self
                .call(
                    "getSignatureStatuses",
                    json!([[signature], { "searchTransactionHistory": true }]),
                )
                .await?;
            let status = &result["value"][0];
            if !status.is_null() {
                if !status["err"].is_null() {
                    bail!("transaction {signature} failed: {}", status["err"]);
                }
                if matches!(status["confirmationStatus"].as_str(), Some("confirmed" | "finalized"))
                {
                    return Ok(true);
                }
            }
            // Linear backoff, capped. Devnet confirms in a second or two on a
            // good day and not at all on a bad one.
            tokio::time::sleep(std::time::Duration::from_millis(400 + 200 * i.min(8) as u64)).await;
        }
        Ok(false)
    }

    /// One account, decoded from base64. `Ok(None)` means it does not exist.
    pub async fn account(&self, address: &Pubkey) -> Result<Option<ProgramAccount>> {
        let result = self
            .call(
                "getAccountInfo",
                json!([address.to_string(), { "encoding": "base64", "commitment": "confirmed" }]),
            )
            .await?;

        let value = &result["value"];
        if value.is_null() {
            return Ok(None);
        }
        let encoded = value["data"][0].as_str().context("account had no base64 data")?;
        Ok(Some(ProgramAccount {
            pubkey: *address,
            data: B64.decode(encoded).context("account data was not valid base64")?,
            lamports: value["lamports"].as_u64().unwrap_or_default(),
        }))
    }

    pub async fn balance(&self, address: &Pubkey) -> Result<u64> {
        let result = self
            .call("getBalance", json!([address.to_string(), { "commitment": "confirmed" }]))
            .await?;
        Ok(result["value"].as_u64().unwrap_or_default())
    }
}
