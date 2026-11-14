//! Modular two-phase bank connectors.
//!
//! Phase 1 `begin()` logs in. It either finishes (`Phase::Ready`) or returns a
//! `Challenge` the user must answer: a decoupled push-TAN (approve in the
//! banking app, then we poll), a code (SMS/app TAN, 2FA PIN) or a redirect
//! (PSD2 consent in the browser).
//!
//! Phase 2 `respond()` answers the challenge (or polls the decoupled one) and
//! again returns `Ready` or another `Challenge`.
//!
//! Once ready, `fetch()` downloads accounts and transactions, which the
//! `SyncSession` merges into the vault and runs the categorisation rules over.

use crate::error::{CoreError, CoreResult};
use crate::i18n::trf;

pub mod bank_directory;
pub mod bunq;
pub mod comdirect;
pub mod demo;
pub mod dpop;
pub mod enable_banking;
pub mod fints;
pub mod fints_directory;
pub mod fints_presets;
pub mod fnz;
pub mod ibkr;
pub mod paypal;
pub mod quirion;
pub mod scalable;
pub mod template;
pub mod trade_republic;

pub(crate) mod account_selection;
pub use account_selection::ConnectionAccountChoice;
mod catalog;
mod persist;
mod sync_session;
mod transfer_session;
mod types;

pub use catalog::*;
pub use persist::*;
pub use sync_session::*;
pub use transfer_session::*;
pub use types::*;

pub(crate) fn http_client() -> CoreResult<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!(
            "GiroCrab/",
            env!("CARGO_PKG_VERSION"),
            " (open source; +https://github.com/)"
        ))
        .timeout(std::time::Duration::from_secs(60))
        .cookie_store(true)
        .build()?)
}

/// HTTP client for FinTS: never follows redirects, so POST bodies carrying
/// PIN/TAN cannot be replayed to a different location.
pub(crate) fn http_client_no_redirect() -> CoreResult<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!(
            "GiroCrab/",
            env!("CARGO_PKG_VERSION"),
            " (open source; +https://github.com/)"
        ))
        .timeout(std::time::Duration::from_secs(60))
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()?)
}

/// FinTS URLs must be https. Plain http to localhost is accepted in tests only.
pub(crate) fn ensure_secure_fints_url(url: &str) -> CoreResult<()> {
    let lower = url.trim().to_ascii_lowercase();
    if lower.starts_with("https://") {
        return Ok(());
    }
    #[cfg(test)]
    if lower.starts_with("http://127.0.0.1") || lower.starts_with("http://localhost") {
        return Ok(());
    }
    Err(CoreError::Protocol {
        message: "FinTS URL must use https://".to_string(),
    })
}

/// Upper bound for JSON / text / FinTS responses.
pub(crate) const MAX_TEXT_BYTES: usize = 10 * 1024 * 1024;
/// Upper bound for downloaded documents (PDF statements, receipts, images).
pub(crate) const MAX_DOC_BYTES: usize = 25 * 1024 * 1024;

fn too_large(limit: usize) -> CoreError {
    CoreError::Network {
        message: trf(
            "Antwort der Bank zu groß (über {0} MB) – abgebrochen.",
            &[&(limit / (1024 * 1024))],
        ),
    }
}

/// Reads a response body, but never more than `limit` bytes (no OOM on a
/// huge or endless body): larger bodies are an error.
pub(crate) async fn read_body_limited(
    mut resp: reqwest::Response,
    limit: usize,
) -> CoreResult<Vec<u8>> {
    if resp.content_length().is_some_and(|n| n > limit as u64) {
        return Err(too_large(limit));
    }
    let mut out = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if out.len() + chunk.len() > limit {
            return Err(too_large(limit));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// [`read_body_limited`] with [`MAX_TEXT_BYTES`], decoded as UTF-8 (lossy).
pub(crate) async fn read_text_limited(resp: reqwest::Response) -> CoreResult<String> {
    let b = read_body_limited(resp, MAX_TEXT_BYTES).await?;
    Ok(String::from_utf8(b).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
}

/// [`read_body_limited`] with [`MAX_TEXT_BYTES`], parsed as JSON; an
/// unreadable or oversized body yields `Value::Null`.
pub(crate) async fn read_json_or_null(resp: reqwest::Response) -> serde_json::Value {
    match read_body_limited(resp, MAX_TEXT_BYTES).await {
        Ok(b) => serde_json::from_slice(&b).unwrap_or(serde_json::Value::Null),
        Err(_) => serde_json::Value::Null,
    }
}

pub(crate) fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests;
