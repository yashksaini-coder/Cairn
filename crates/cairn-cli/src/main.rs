//! Cairn, the whole client.
//!
//! There is no web app. This binary funds escrows, records receipts, releases
//! funds and verifies releases, and it does the hashing itself with the same
//! crate the on-chain program uses -- so there is no second implementation of
//! anything for the two to disagree about.
//!
//! What it never does is hold anything on your behalf. Every command that
//! moves money takes `--keypair` and signs locally. The server this talks to
//! stores audio and indexes the chain; it cannot sign, and there is no
//! operator override anywhere in the protocol for it to reach for.

use std::{path::PathBuf, str::FromStr};

use anchor_lang::InstructionData;
use anyhow::{bail, Context, Result};
use cairn_api::{
    config::Config,
    db,
    rpc::Rpc,
    state::now,
    views::{
        Challenge, EscrowDetail, EscrowList, EscrowView, IssuedReceipt, Verdict, Verification,
    },
};
use cairn_core::{escrow_pda, vault_pda, CanonicalReceipt, Escrow, EscrowState, NEED_ID_LEN};
use clap::{Parser, Subcommand};
use rand::RngCore;
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::Keypair,
    signer::{keypair::read_keypair_file, Signer},
    transaction::Transaction,
};

const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

#[derive(Parser)]
#[command(name = "cairn", version, about = "Proof of arrival for charitable giving")]
#[command(long_about = "Lock funds in a Solana escrow that only the recipient can release, \
                        carrying the hash of a voice recording in which they say what they \
                        received. Devnet only.")]
struct Cli {
    #[arg(
        long,
        env = "SOLANA_RPC_URL",
        default_value = "https://api.devnet.solana.com",
        global = true
    )]
    rpc: String,

    #[arg(long, env = "CAIRN_PROGRAM_ID", global = true)]
    program_id: Option<String>,

    #[arg(long, env = "CAIRN_API_URL", default_value = "http://localhost:8080", global = true)]
    api: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Lock funds for someone, against a description of what they are for.
    Give {
        #[arg(long)]
        keypair: PathBuf,
        /// Recipient wallet. This key, and only this key, can release the funds.
        #[arg(long)]
        to: String,
        /// Amount in SOL.
        #[arg(long)]
        amount: String,
        /// What the money is for. This text is hashed on chain and cannot be
        /// changed afterwards.
        #[arg(long = "for")]
        description: String,
        #[arg(long)]
        title: Option<String>,
        /// How long the recipient has: `30m`, `24h`, `7d`. Minimum 5m, maximum 90d.
        #[arg(long, default_value = "24h")]
        window: String,
    },

    /// List escrows from the index.
    List {
        #[arg(long)]
        donor: Option<String>,
        #[arg(long)]
        recipient: Option<String>,
        #[arg(long)]
        state: Option<String>,
        /// Only escrows past their deadline that nobody has refunded yet.
        #[arg(long)]
        expired: bool,
        #[arg(long, default_value_t = 25)]
        limit: i64,
    },

    /// Everything about one escrow, cross-checked against the chain.
    Show { escrow: String },

    /// Record a receipt and release the funds. Recipient signs.
    Receive {
        #[arg(long)]
        keypair: PathBuf,
        #[arg(long)]
        escrow: String,
        /// The recording. Anything ffmpeg-ish will do:
        /// `arecord -f cd -d 10 receipt.wav`
        #[arg(long)]
        audio: PathBuf,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },

    /// Take back an expired escrow. Donor signs.
    Refund {
        #[arg(long)]
        keypair: PathBuf,
        #[arg(long)]
        escrow: String,
    },

    /// Check a release: re-fetch, re-hash, recompute, compare.
    Verify { signature: String },

    /// Recompute hashes with nothing but a file and this binary.
    #[command(subcommand)]
    Hash(HashCommand),

    /// Run one indexer pass into the local read model.
    Backfill,
}

#[derive(Subcommand)]
enum HashCommand {
    /// Hash a need description, normalised exactly as the server does.
    Need { text: String },
    /// Rebuild a canonical receipt from its parts and print its hash.
    Receipt {
        #[arg(long)]
        escrow: String,
        #[arg(long)]
        audio: PathBuf,
        #[arg(long, default_value = "")]
        transcript: String,
        #[arg(long, default_value = "und")]
        locale: String,
        #[arg(long)]
        recorded_at: i64,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let rpc = Rpc::new(cli.rpc.clone());
    let api = cli.api.trim_end_matches('/').to_string();
    let program = cli.program_id.clone();

    match cli.command {
        Command::Hash(cmd) => hash(cmd),
        Command::Verify { signature } => verify(&api, &signature).await,
        Command::Backfill => backfill(rpc).await,
        Command::List { donor, recipient, state, expired, limit } => {
            list(&api, donor, recipient, state, expired, limit).await
        }
        Command::Show { escrow } => show(&api, &rpc, &program, &escrow).await,
        Command::Give { keypair, to, amount, description, title, window } => {
            give(&rpc, &api, &program, keypair, to, amount, description, title, window).await
        }
        Command::Receive { keypair, escrow, audio, yes } => {
            receive(&rpc, &api, &program, keypair, escrow, audio, yes).await
        }
        Command::Refund { keypair, escrow } => refund(&rpc, &program, keypair, escrow).await,
    }
}

// ------------------------------------------------------------------ commands

fn hash(cmd: HashCommand) -> Result<()> {
    match cmd {
        HashCommand::Need { text } => {
            println!("{}", hex::encode(cairn_core::need_hash(&text)));
        }
        HashCommand::Receipt { escrow, audio, transcript, locale, recorded_at } => {
            let bytes =
                std::fs::read(&audio).with_context(|| format!("reading {}", audio.display()))?;
            let audio_sha256 = solana_program::hash::hash(&bytes).to_bytes();
            let receipt = CanonicalReceipt::new(
                Pubkey::from_str(&escrow)?,
                audio_sha256,
                cairn_core::normalize::normalize_text(&transcript),
                cairn_core::receipt::encode_locale(&locale),
                recorded_at,
            );
            kv("audio_sha256", &hex::encode(audio_sha256));
            kv("receipt_hash", &hex::encode(receipt.hash()));
        }
    }
    Ok(())
}

async fn verify(api: &str, signature: &str) -> Result<()> {
    let result: Verification = get(&format!("{api}/v1/verify/{signature}")).await?;

    let (mark, colour) = match result.verdict {
        Verdict::Verified => ("PASS", GREEN),
        Verdict::Mismatch | Verdict::TransactionFailed => ("FAIL", RED),
        Verdict::AudioUnavailable | Verdict::ArtifactsUnknown => ("PARTIAL", YELLOW),
    };
    println!("{}", paint(colour, &format!("  {mark}  {}", verdict_name(result.verdict))));
    println!("  {}\n", dim(&wrap(&result.detail, 68, "       ")));

    if let Some(h) = &result.onchain_receipt_hash {
        kv("committed on chain", h);
    }
    if let Some(h) = &result.recomputed_hash {
        kv("recomputed here", h);
    }
    kv("audio re-hashed", if result.audio_available { "yes" } else { "no, not available" });
    if let Some(e) = &result.escrow {
        kv("escrow", e);
    }
    if let Some(t) = result.transcript.as_deref().filter(|t| !t.is_empty()) {
        println!();
        println!("  {}", paint(BOLD, "what was said"));
        println!("  {}", wrap(t, 68, "  "));
    }

    if !result.matches {
        bail!("verification did not pass");
    }
    Ok(())
}

async fn backfill(rpc: Rpc) -> Result<()> {
    // Reuses the server's own indexer so there is no second decoder to keep
    // in step with the program.
    let cfg = Config::from_env().context("backfill reads the same environment as the API")?;
    let pool = db::connect(&cfg.database_url).await?;
    let state = cairn_api::state::AppState {
        rpc,
        blobs: cairn_api::blob::BlobStore::new(&cfg.blob, &cfg.public_base_url).await?,
        transcriber: std::sync::Arc::new(cairn_api::transcribe::Transcriber::new(None)),
        challenges: std::sync::Arc::new(cairn_api::auth::Challenges::new(
            std::time::Duration::from_secs(120),
        )),
        cfg: std::sync::Arc::new(cfg),
        pool,
    };
    println!("indexed {} escrows", cairn_api::indexer::sync_once(&state).await?);
    Ok(())
}

async fn list(
    api: &str,
    donor: Option<String>,
    recipient: Option<String>,
    state: Option<String>,
    expired: bool,
    limit: i64,
) -> Result<()> {
    let mut url = format!("{api}/v1/escrows?limit={limit}");
    if let Some(d) = donor {
        url.push_str(&format!("&donor={d}"));
    }
    if let Some(r) = recipient {
        url.push_str(&format!("&recipient={r}"));
    }
    if let Some(s) = state {
        url.push_str(&format!("&state={s}"));
    }
    if expired {
        url.push_str("&expired=true");
    }

    let page: EscrowList = get(&url).await?;
    if page.escrows.is_empty() {
        println!("{}", dim("  nothing here yet"));
        return Ok(());
    }

    for view in &page.escrows {
        let title = view.need.as_ref().map(|n| n.title.as_str()).unwrap_or("(no description)");
        println!(
            "  {}  {}  {}",
            paint(BOLD, &short(&view.escrow.pubkey)),
            sol(view.escrow.amount).to_string()
                + &" ".repeat(9usize.saturating_sub(sol(view.escrow.amount).len())),
            title
        );
        println!("  {}", dim(&format!("     {} · {}", status(view), remaining(view))));
    }
    Ok(())
}

async fn show(api: &str, rpc: &Rpc, program: &Option<String>, escrow: &str) -> Result<()> {
    let detail: EscrowDetail = get(&format!("{api}/v1/escrows/{escrow}")).await?;
    let view = &detail.escrow;
    let row = &view.escrow;

    println!(
        "  {}  {}",
        paint(BOLD, view.need.as_ref().map(|n| n.title.as_str()).unwrap_or("(no description)")),
        badge(view)
    );
    println!();

    // §9.2, now in the terminal: re-derive the need hash from the text we were
    // handed and compare it with what the donor committed. The server could
    // serve any description it liked; this is what makes that not matter.
    match &view.need {
        Some(need) => {
            let computed = hex::encode(cairn_core::need_hash(&need.description));
            println!("  {}", wrap(&need.description, 68, "  "));
            if computed == row.need_hash {
                println!("  {}", paint(GREEN, "✓ hashes to the value committed on chain"));
            } else {
                println!("  {}", paint(RED, "✗ TAMPERED: this text is not what was funded"));
                println!("  {}", dim(&format!("    committed {}", row.need_hash)));
                println!("  {}", dim(&format!("    this text {computed}")));
            }
        }
        None => println!("  {}", dim("no description registered; only the hash is on chain")),
    }

    println!();
    kv("escrow", &row.pubkey);
    kv("amount", &sol(row.amount));
    kv("donor", &row.donor);
    kv("recipient", &row.recipient);
    kv("deadline", &format!("{} ({})", row.deadline, remaining(view)));

    if let Some(receipt) = &detail.receipt {
        println!();
        println!("  {}", paint(BOLD, "receipt"));
        if receipt.transcript_available != 0 {
            println!("  {}", wrap(&receipt.transcript, 68, "  "));
        } else {
            println!(
                "  {}",
                dim("(transcription was unavailable; the hash still covers the audio)")
            );
        }
        kv("receipt_hash", row.receipt_hash.as_deref().unwrap_or("-"));
        kv("audio_sha256", &receipt.audio_sha256);
        if let Some(url) = &detail.audio_url {
            kv("audio", url);
        }
    }

    // The read model is a cache. Ask the chain directly and say whether the
    // two agree -- an indexer that has quietly drifted should not be able to
    // hide behind a confident-looking record.
    if let Ok(program_id) = program_id(program) {
        let key = Pubkey::from_str(escrow)?;
        println!();
        match rpc.account(&key).await? {
            Some(account) if account.data.starts_with(&cairn_core::state::ESCROW_DISCRIMINATOR) => {
                let onchain = Escrow::try_decode(&account.data)?;
                let vault = vault_pda(&program_id, &key).0;
                let vault_lamports = rpc.balance(&vault).await?;
                let agrees = onchain.state.as_str() == row.state;
                let sound = onchain.invariants_hold(vault_lamports);

                kv(
                    "vault",
                    &format!(
                        "{} holding {}",
                        short(&vault.to_string()),
                        sol(vault_lamports as i64)
                    ),
                );
                println!(
                    "  {}",
                    if agrees && sound {
                        paint(GREEN, "✓ chain and index agree, and the invariants hold")
                    } else if !agrees {
                        paint(
                            YELLOW,
                            &format!(
                                "! index says {}, chain says {} -- the index is behind",
                                row.state,
                                onchain.state.as_str()
                            ),
                        )
                    } else {
                        paint(RED, "✗ on-chain invariants VIOLATED")
                    }
                );
            }
            _ => println!("  {}", dim("not readable on chain from this RPC")),
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn give(
    rpc: &Rpc,
    api: &str,
    program: &Option<String>,
    keypair: PathBuf,
    to: String,
    amount: String,
    description: String,
    title: Option<String>,
    window: String,
) -> Result<()> {
    let donor = load_keypair(&keypair)?;
    let program_id = program_id(program)?;
    let recipient = Pubkey::from_str(to.trim()).with_context(|| format!("{to} is not a pubkey"))?;
    let lamports = parse_sol(&amount)?;
    let window = parse_duration(&window)?;

    if recipient == donor.pubkey() {
        bail!("a donor cannot be their own recipient");
    }
    if lamports < cairn_core::MIN_ESCROW_LAMPORTS {
        bail!("the minimum is {}", sol(cairn_core::MIN_ESCROW_LAMPORTS as i64));
    }

    let normalized = cairn_core::normalize::normalize_text(&description);
    if normalized.is_empty() {
        bail!("--for cannot be empty; the description is what gets hashed on chain");
    }
    let need_hash = cairn_core::need_hash_of_normalized(&normalized);

    let mut need_id = [0u8; NEED_ID_LEN];
    rand::thread_rng().fill_bytes(&mut need_id);
    let (escrow, _) = escrow_pda(&program_id, &donor.pubkey(), &recipient, &need_id);
    let (vault, _) = vault_pda(&program_id, &escrow);

    // §12: no airdrop, ever. Devnet rate-limits them, and a failed airdrop
    // mid-demo is indistinguishable from a broken program.
    let balance = rpc.balance(&donor.pubkey()).await?;
    if balance < lamports + 10_000_000 {
        bail!(
            "{} holds {} but needs about {}. Fund it first; this tool will not airdrop.",
            donor.pubkey(),
            sol(balance as i64),
            sol((lamports + 10_000_000) as i64)
        );
    }

    let ix = Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new(donor.pubkey(), true),
            AccountMeta::new_readonly(recipient, false),
            AccountMeta::new(escrow, false),
            AccountMeta::new(vault, false),
            AccountMeta::new_readonly(anchor_lang::system_program::ID, false),
        ],
        data: cairn::instruction::CreateEscrow {
            need_id,
            amount: lamports,
            need_hash,
            deadline: now() + window,
        }
        .data(),
    };

    kv("need_hash", &hex::encode(need_hash));
    let signature = submit(rpc, ix, &donor).await?;
    println!(
        "  {}",
        paint(
            GREEN,
            &format!("✓ {} locked for {}", sol(lamports as i64), short(&recipient.to_string()))
        )
    );
    kv("escrow", &escrow.to_string());
    kv("tx", &explorer(&signature));

    // The API only accepts text that hashes to the value already on chain, so
    // this call can supply a description but can never contradict one.
    let body = serde_json::json!({
        "escrow": escrow.to_string(),
        "title": title.unwrap_or_else(|| truncate(&normalized, 48)),
        "description": description,
    });
    match reqwest::Client::new().post(format!("{api}/v1/needs")).json(&body).send().await {
        Ok(r) if r.status().is_success() => {}
        Ok(r) => eprintln!(
            "  {}",
            dim(&format!(
                "description not registered ({}): {}",
                r.status(),
                r.text().await.unwrap_or_default()
            ))
        ),
        Err(e) => eprintln!(
            "  {}",
            dim(&format!("could not reach the API to register the description: {e}"))
        ),
    }

    println!("\n  {}", dim(&format!("they release it with:  cairn receive --escrow {escrow} --audio receipt.wav --keypair <theirs>")));
    Ok(())
}

async fn receive(
    rpc: &Rpc,
    api: &str,
    program: &Option<String>,
    keypair: PathBuf,
    escrow: String,
    audio: PathBuf,
    yes: bool,
) -> Result<()> {
    let me = load_keypair(&keypair)?;
    let program_id = program_id(program)?;
    let escrow_key = Pubkey::from_str(escrow.trim())?;

    let detail: EscrowDetail = get(&format!("{api}/v1/escrows/{escrow}")).await?;
    let row = &detail.escrow.escrow;
    if row.recipient != me.pubkey().to_string() {
        bail!("this escrow names {} as its recipient, not you", row.recipient);
    }
    if row.state != "funded" {
        bail!("this escrow is already {}", row.state);
    }
    if detail.escrow.expired {
        bail!("the deadline passed; only the donor can recover these funds now");
    }

    let bytes = std::fs::read(&audio).with_context(|| format!("reading {}", audio.display()))?;
    let content_type = mime_for(&audio);
    let duration_ms = wav_duration_ms(&bytes);

    // 1. Prove control of this key before the server spends money on
    //    transcription. Cost control, not authorization -- step 5 is what
    //    actually releases anything.
    let challenge: Challenge = post_json(
        &format!("{api}/v1/receipts/challenge"),
        &serde_json::json!({ "escrow": escrow }),
    )
    .await?;
    let signed = me.sign_message(challenge.message.as_bytes()).to_string();

    // 2. Upload. The server hashes, stores and transcribes.
    let recorded_at = now();
    let mut form = reqwest::multipart::Form::new()
        .text("escrow", escrow.clone())
        .text("nonce", challenge.nonce)
        .text("signature", signed)
        .text("recorded_at", recorded_at.to_string())
        .part(
            "file",
            reqwest::multipart::Part::bytes(bytes.clone())
                .file_name(
                    audio
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "receipt".into()),
                )
                .mime_str(content_type)?,
        );
    if let Some(ms) = duration_ms {
        form = form.text("duration_ms", ms.to_string());
    }

    let issued: IssuedReceipt = post_form(&format!("{api}/v1/receipts"), form).await?;

    // 3. Recompute locally before signing anything. This is the same
    //    `cairn-core` function the server just ran, so it does not prove the
    //    hash algorithm is right -- it proves the hash covers the bytes on
    //    this disk and the words printed below, which is the part a recipient
    //    is actually being asked to stand behind.
    let local = CanonicalReceipt::new(
        escrow_key,
        solana_program::hash::hash(&bytes).to_bytes(),
        issued.transcript.clone(),
        cairn_core::receipt::encode_locale(&issued.locale),
        issued.recorded_at,
    )
    .hash();

    println!();
    if issued.transcript_available {
        println!("  {}", paint(BOLD, "you are about to sign for these words"));
        println!("  {}", wrap(&issued.transcript, 68, "  "));
    } else {
        println!("  {}", dim("transcription was unavailable; the receipt carries an empty"));
        println!("  {}", dim("transcript and still covers your recording"));
    }
    println!();
    kv("receipt_hash", &issued.receipt_hash);
    kv("audio_sha256", &issued.audio_sha256);

    if hex::encode(local) != issued.receipt_hash {
        println!("  {}", paint(RED, "✗ that hash does not match your recording"));
        kv("recomputed here", &hex::encode(local));
        bail!("refusing to sign a hash that does not cover the file you gave me");
    }
    println!("  {}", paint(GREEN, "✓ recomputed from your file; the hash covers exactly this"));

    if !yes {
        print!("\n  release {} to yourself? [y/N] ", sol(row.amount));
        use std::io::Write;
        std::io::stdout().flush().ok();
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
            println!("  {}", dim("nothing signed, nothing sent"));
            return Ok(());
        }
    }

    // 4. The only thing in this whole flow that moves money.
    let mut receipt_hash = [0u8; 32];
    receipt_hash.copy_from_slice(&hex::decode(&issued.receipt_hash)?);

    let ix = Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new(escrow_key, false),
            AccountMeta::new(vault_pda(&program_id, &escrow_key).0, false),
            AccountMeta::new(me.pubkey(), true),
            AccountMeta::new_readonly(anchor_lang::system_program::ID, false),
        ],
        data: cairn::instruction::SubmitReceipt { receipt_hash }.data(),
    };

    let signature = submit(rpc, ix, &me).await?;
    println!("  {}", paint(GREEN, &format!("✓ {} released to you", sol(row.amount))));
    kv("tx", &explorer(&signature));
    println!("\n  {}", dim(&format!("anyone can now check it:  cairn verify {signature}")));
    Ok(())
}

async fn refund(
    rpc: &Rpc,
    program: &Option<String>,
    keypair: PathBuf,
    escrow: String,
) -> Result<()> {
    let donor = load_keypair(&keypair)?;
    let program_id = program_id(program)?;
    let escrow_key = Pubkey::from_str(escrow.trim())?;

    let ix = Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new(escrow_key, false),
            AccountMeta::new(vault_pda(&program_id, &escrow_key).0, false),
            AccountMeta::new(donor.pubkey(), true),
            AccountMeta::new_readonly(anchor_lang::system_program::ID, false),
        ],
        data: cairn::instruction::Refund {}.data(),
    };

    let signature = submit(rpc, ix, &donor).await?;
    println!("  {}", paint(GREEN, "✓ refunded"));
    kv("tx", &explorer(&signature));
    Ok(())
}

// ------------------------------------------------------------------- plumbing

async fn submit(rpc: &Rpc, ix: Instruction, payer: &Keypair) -> Result<String> {
    let blockhash = rpc.latest_blockhash().await?;
    let mut tx = Transaction::new_with_payer(&[ix], Some(&payer.pubkey()));
    tx.sign(&[payer], blockhash.parse().context("RPC returned an unparseable blockhash")?);

    let signature = rpc.send_transaction(&bincode::serialize(&tx)?).await?;
    // Printed by the caller either way: §12 requires that a devnet stall
    // still leaves you holding an explorer link rather than an error.
    if !rpc.confirm(&signature, 20).await? {
        eprintln!("  {}", dim("not confirmed within the timeout; check the explorer"));
    }
    Ok(signature)
}

async fn get<T: serde::de::DeserializeOwned>(url: &str) -> Result<T> {
    let res = reqwest::get(url).await.with_context(|| format!("GET {url}"))?;
    decode(res, url).await
}

async fn post_json<T: serde::de::DeserializeOwned>(
    url: &str,
    body: &serde_json::Value,
) -> Result<T> {
    let res = reqwest::Client::new()
        .post(url)
        .json(body)
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    decode(res, url).await
}

async fn post_form<T: serde::de::DeserializeOwned>(
    url: &str,
    form: reqwest::multipart::Form,
) -> Result<T> {
    let res = reqwest::Client::new()
        .post(url)
        .multipart(form)
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    decode(res, url).await
}

async fn decode<T: serde::de::DeserializeOwned>(res: reqwest::Response, url: &str) -> Result<T> {
    let status = res.status();
    let body = res.text().await.unwrap_or_default();
    if !status.is_success() {
        // The API always answers with `{"error":{"code","message"}}`, but a
        // cold start or a proxy can put something else in front of it.
        let message = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
            .unwrap_or(body);
        bail!("{status} from {url}: {message}");
    }
    serde_json::from_str(&body).with_context(|| format!("unexpected response from {url}: {body}"))
}

fn load_keypair(path: &PathBuf) -> Result<Keypair> {
    read_keypair_file(path)
        .map_err(|e| anyhow::anyhow!("could not read keypair {}: {e}", path.display()))
}

fn program_id(raw: &Option<String>) -> Result<Pubkey> {
    let raw = raw
        .as_deref()
        .context("--program-id (or CAIRN_PROGRAM_ID) is required for on-chain commands")?;
    Pubkey::from_str(raw).with_context(|| format!("{raw} is not a pubkey"))
}

fn parse_sol(input: &str) -> Result<u64> {
    let value: f64 = input.trim().parse().with_context(|| format!("{input} is not a number"))?;
    if !value.is_finite() || value <= 0.0 {
        bail!("amount must be positive");
    }
    // Rounded, not truncated: 0.1 SOL is 99999999.99999999 in binary floating
    // point, and truncating would quietly short the recipient a lamport.
    Ok((value * LAMPORTS_PER_SOL as f64).round() as u64)
}

/// `30m`, `24h`, `7d`, `300s`, or a bare number of seconds.
fn parse_duration(input: &str) -> Result<i64> {
    let input = input.trim();
    let (digits, multiplier) = match input.chars().last() {
        Some('s') => (&input[..input.len() - 1], 1),
        Some('m') => (&input[..input.len() - 1], 60),
        Some('h') => (&input[..input.len() - 1], 3600),
        Some('d') => (&input[..input.len() - 1], 86400),
        _ => (input, 1),
    };
    let n: i64 = digits.parse().with_context(|| format!("{input} is not a duration like 24h"))?;
    let seconds = n * multiplier;
    if seconds < cairn_core::MIN_WINDOW_SECONDS {
        bail!("give them at least 5 minutes (--window 5m)");
    }
    if seconds > cairn_core::MAX_WINDOW_SECONDS {
        bail!("the deadline cannot be more than 90 days out");
    }
    Ok(seconds)
}

fn mime_for(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase().as_str() {
        "wav" => "audio/wav",
        "webm" => "audio/webm",
        "ogg" | "opus" => "audio/ogg",
        "mp3" => "audio/mpeg",
        "m4a" | "mp4" => "audio/mp4",
        _ => "application/octet-stream",
    }
}

/// Read a canonical WAV header and work out how long the recording actually
/// is, rather than taking the caller's word for it.
///
/// Only WAV, and only the canonical layout `arecord` and `ffmpeg` emit. For
/// anything else this returns `None` and the server falls back to its byte
/// floor -- which is the honest answer, because a duration this tool cannot
/// verify is a duration it should not assert.
fn wav_duration_ms(bytes: &[u8]) -> Option<i64> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    // Length was checked above, so these slices cannot be short.
    let u32_at = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap()) as u64;
    let byte_rate = u32_at(28);
    let data_len = u32_at(40);
    if byte_rate == 0 {
        return None;
    }
    Some((data_len * 1000 / byte_rate) as i64)
}

// -------------------------------------------------------------------- display

const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";

fn colour() -> bool {
    use std::io::IsTerminal;
    // Honours https://no-color.org. Piping into a file should give you plain
    // text, not escape codes.
    std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal()
}

fn paint(code: &str, text: &str) -> String {
    if colour() {
        format!("{code}{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

fn dim(text: &str) -> String {
    paint(DIM, text)
}

fn kv(label: &str, value: &str) {
    println!("  {:<20} {value}", dim(label));
}

fn short(value: &str) -> String {
    if value.len() <= 13 {
        value.to_string()
    } else {
        format!("{}…{}", &value[..6], &value[value.len() - 6..])
    }
}

fn sol(lamports: i64) -> String {
    let value = lamports as f64 / LAMPORTS_PER_SOL as f64;
    format!("{} SOL", format!("{value:.4}").trim_end_matches('0').trim_end_matches('.'))
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        text.chars().take(max - 1).collect::<String>() + "…"
    }
}

fn wrap(text: &str, width: usize, indent: &str) -> String {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines.join(&format!("\n{indent}"))
}

fn status(view: &EscrowView) -> String {
    if view.expired {
        "expired".into()
    } else {
        view.escrow.state.clone()
    }
}

fn badge(view: &EscrowView) -> String {
    let label = status(view);
    let colour = match label.as_str() {
        "funded" => YELLOW,
        "released" => GREEN,
        "expired" => RED,
        _ => DIM,
    };
    paint(colour, &label)
}

fn remaining(view: &EscrowView) -> String {
    if view.escrow.state != EscrowState::Funded.as_str() {
        return view.escrow.state.clone();
    }
    let delta = view.escrow.deadline - now();
    if delta <= 0 {
        return "deadline passed".into();
    }
    for (size, name) in [(86400, "day"), (3600, "hour"), (60, "minute")] {
        if delta >= size {
            let n = delta / size;
            return format!("{n} {name}{} left", if n == 1 { "" } else { "s" });
        }
    }
    format!("{delta} seconds left")
}

fn verdict_name(v: Verdict) -> &'static str {
    match v {
        Verdict::Verified => "verified",
        Verdict::Mismatch => "does not match",
        Verdict::AudioUnavailable => "recording unavailable",
        Verdict::ArtifactsUnknown => "no artifacts on file",
        Verdict::TransactionFailed => "transaction failed on chain",
    }
}

fn explorer(signature: &str) -> String {
    format!("https://explorer.solana.com/tx/{signature}?cluster=devnet")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(sample_rate: u32, channels: u16, bits: u16, data_len: u32) -> Vec<u8> {
        let byte_rate = sample_rate * channels as u32 * (bits as u32 / 8);
        let mut w = Vec::new();
        w.extend(b"RIFF");
        w.extend((36 + data_len).to_le_bytes());
        w.extend(b"WAVEfmt ");
        w.extend(16u32.to_le_bytes());
        w.extend(1u16.to_le_bytes());
        w.extend(channels.to_le_bytes());
        w.extend(sample_rate.to_le_bytes());
        w.extend(byte_rate.to_le_bytes());
        w.extend((channels * bits / 8).to_le_bytes());
        w.extend(bits.to_le_bytes());
        w.extend(b"data");
        w.extend(data_len.to_le_bytes());
        w.resize(44 + data_len as usize, 0);
        w
    }

    #[test]
    fn reads_a_real_wav_duration() {
        // 44.1kHz stereo 16-bit = 176400 bytes/second, the `arecord -f cd`
        // default. Ten seconds of it.
        let ten_seconds = wav(44_100, 2, 16, 176_400 * 10);
        assert_eq!(wav_duration_ms(&ten_seconds), Some(10_000));
    }

    #[test]
    fn declines_to_guess_at_anything_else() {
        // The point of returning None is that the server then falls back to
        // its byte floor. A duration this tool cannot verify is one it must
        // not assert.
        assert_eq!(wav_duration_ms(b"not audio at all"), None);
        assert_eq!(wav_duration_ms(&[0u8; 200]), None);
        let mut truncated = wav(44_100, 2, 16, 1000);
        truncated.truncate(20);
        assert_eq!(wav_duration_ms(&truncated), None);
        // A header claiming a zero byte rate would divide by zero.
        assert_eq!(wav_duration_ms(&wav(0, 0, 0, 1000)), None);
    }

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration("24h").unwrap(), 86_400);
        assert_eq!(parse_duration("90m").unwrap(), 5_400);
        assert_eq!(parse_duration("7d").unwrap(), 604_800);
        assert_eq!(parse_duration("600s").unwrap(), 600);
        assert_eq!(parse_duration(" 600 ").unwrap(), 600);
    }

    #[test]
    fn refuses_windows_the_program_would_reject() {
        // Better to fail here than to spend a transaction discovering it.
        assert!(parse_duration("1m").is_err());
        assert!(parse_duration("100d").is_err());
        assert!(parse_duration("soon").is_err());
    }

    #[test]
    fn parses_sol_without_shorting_anyone() {
        assert_eq!(parse_sol("1").unwrap(), 1_000_000_000);
        // 0.1 is 99999999.99999999 in binary floating point; truncating here
        // would quietly cost the recipient a lamport.
        assert_eq!(parse_sol("0.1").unwrap(), 100_000_000);
        assert_eq!(parse_sol("0.001").unwrap(), 1_000_000);
        assert!(parse_sol("0").is_err());
        assert!(parse_sol("-1").is_err());
        assert!(parse_sol("lots").is_err());
    }

    #[test]
    fn wraps_without_losing_words() {
        let text = "the quick brown fox jumps over the lazy dog";
        let wrapped = wrap(text, 20, "");
        assert!(wrapped.lines().all(|l| l.chars().count() <= 20));
        assert_eq!(
            wrapped.split_whitespace().collect::<Vec<_>>(),
            text.split(' ').collect::<Vec<_>>()
        );
    }
}
