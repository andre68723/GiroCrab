//! GiroCrab core (früher CashCrab).
//!
//! A clean-room personal-finance engine: an encrypted SQLCipher ledger,
//! hierarchical categories, keyword/regex auto-categorisation rules and a
//! modular two-phase bank connector layer (FinTS 3.0, comdirect REST,
//! PayPal Transaction Search, Trade Republic WebSocket, Enable Banking PSD2).
//!
//! Money is *always* an `i64` amount of cents (minor units). No floating point
//! value ever touches a balance or a transaction amount.
//!
//! Everything public is exported to Swift (and Kotlin, if you want) through
//! Mozilla UniFFI proc-macros.

// Stylistic lints: amounts are written as `euros_cents` (e.g. 27_65), bank
// rows are plain tuples, and `from_str` constructors return Option for UniFFI.
#![allow(
    clippy::inconsistent_digit_grouping,
    clippy::type_complexity,
    clippy::too_many_arguments,
    clippy::should_implement_trait,
    clippy::if_same_then_else
)]

uniffi::setup_scaffolding!();

pub mod asset_classification;
pub mod assets;
pub mod banking_gate;
pub mod batch_csv;
pub mod connections;
pub mod connectors;
mod data_tables;
pub mod demo;
pub mod depot_debug;
pub mod depot_values;
pub mod documents;
pub mod error;
pub mod export;
pub mod i18n;
pub mod importers;
pub mod insights;
pub mod loans;
pub mod local_api;
pub mod mandates;
pub mod merchants;
pub mod models;
pub mod money;
pub mod opening_balance;
pub mod planned;
pub mod planning;
pub mod recurring;
pub mod report_sanitizer;
pub mod rules;
pub mod salary;
mod schema;
pub mod search_expr;
pub mod secret;
pub mod security_transactions;
pub mod standing_orders;
pub mod sync_log;
mod sync_merge;
pub mod tags;
pub mod text;
pub mod user_groups;
pub mod vault;

#[cfg(test)]
pub(crate) mod test_support;

pub use connectors::{
    bank_catalog, BankDescriptor, Challenge, ChallengeKind, SyncSession, SyncStep, TransferSession,
    TransferStep,
};
pub use error::CoreError;
pub use merchants::{resolve_merchant, FlowKind, MerchantInfo, MerchantKind, MerchantResolver};
pub use models::*;
pub use money::{format_cents, parse_amount_to_cents};
pub use vault::Vault;

/// Version string of the core library (shown in the About/Settings screen).
#[uniffi::export]
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// True when this build carries GiroCrab's own FinTS product registration
/// (release builds only; the ID itself is never exposed).
#[uniffi::export]
pub fn fints_builtin_product_id_available() -> bool {
    connectors::fints::product_id::builtin_available()
}

/// Generates 32 random bytes for a new SQLCipher master key.
/// The Swift side stores it in the Keychain behind Face ID / Touch ID.
#[uniffi::export]
pub fn generate_master_key() -> Vec<u8> {
    use rand::RngCore;
    let mut key = vec![0u8; 32];
    rand::rng().fill_bytes(&mut key);
    key
}
