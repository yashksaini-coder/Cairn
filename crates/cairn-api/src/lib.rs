//! Cairn's backend, as a library.
//!
//! Split lib/bin so `cairn-cli` can reuse the RPC client, the read model and
//! the indexer rather than growing a second copy of each. The binary in
//! `main.rs` is a thin shell around this.

pub mod auth;
pub mod blob;
pub mod config;
pub mod db;
pub mod error;
pub mod indexer;
pub mod routes;
pub mod rpc;
pub mod state;
pub mod transcribe;
pub mod views;
