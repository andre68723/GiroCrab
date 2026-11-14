//! Connector trait and the data types exchanged with connectors and the app.

use std::collections::HashMap;

use async_trait::async_trait;

use crate::error::{CoreError, CoreResult};
use crate::i18n::tr;
use crate::models::{AccountKind, ConnectorKind};
use crate::secret::SecretString;

// ------------------------------------------------------------------ FFI types

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ChallengeKind {
    /// Approve in the bank's app (FinTS decoupled, comdirect P_TAN_PUSH).
    /// The UI shows a spinner and calls `poll()` every few seconds.
    DecoupledPush,
    /// Type a code (SMS/app TAN, Trade Republic 2FA PIN).
    Code,
    /// Scan a photoTAN/chipTAN image, then type the code.
    ImageCode,
    /// Open `redirect_url` in the browser, then paste the resulting URL/code.
    Redirect,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct Challenge {
    pub kind: ChallengeKind,
    pub title: String,
    pub message: String,
    /// Name of the TAN method ("SecureGo plus", "pushTAN", "SMS") if known.
    pub method_name: String,
    pub redirect_url: Option<String>,
    /// PNG/JPEG bytes for photoTAN (ImageCode).
    pub image: Option<Vec<u8>>,
    /// Seconds until the challenge expires, if the bank says so.
    pub expires_in_seconds: Option<u32>,
    /// Recommended poll interval for DecoupledPush.
    pub poll_interval_seconds: u32,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct SyncSummary {
    pub accounts_updated: u32,
    pub transactions_imported: u32,
    pub transactions_categorized: u32,
    /// New bookings the bank delivered for archived accounts (not stored).
    pub transactions_skipped_archived: u32,
    /// Accounts this sync added to a connection that already had accounts
    /// (e.g. included in a renewed PSD2 consent) – named in the sync notice.
    pub new_accounts: Vec<String>,
    /// Accounts the bank / consent no longer grants access to (newly marked).
    pub revoked_accounts: Vec<String>,
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum SyncStep {
    NeedsChallenge {
        challenge: Challenge,
    },
    /// Decoupled approval not yet given; keep polling.
    StillWaiting {
        challenge: Challenge,
    },
    Finished {
        summary: SyncSummary,
    },
}

/// Progress of a SEPA transfer (`TransferSession`).
#[derive(Debug, Clone, uniffi::Enum)]
pub enum TransferStep {
    NeedsChallenge {
        challenge: Challenge,
    },
    StillWaiting {
        challenge: Challenge,
    },
    /// Name check (Verification of Payee) did not confirm the recipient:
    /// show `check`, then `confirm_payee()` to send anyway or `cancel()`.
    NeedsPayeeConfirmation {
        check: PayeeCheck,
    },
    /// Bank accepted the transfer; an optimistic booking was stored.
    Finished {
        outcome: TransferOutcome,
    },
    /// SEPA Instant was not possible and nothing was approved or executed
    /// (bank does not offer it, or rejected the order before any approval).
    /// No second order is sent automatically: the app may offer to send it
    /// as a regular transfer, which is a new order with its own approval.
    InstantDeclined {
        reason: String,
    },
    /// The order reached the bank but its result is unclear (ambiguous reply
    /// after the approval, connection lost). It MAY have been executed. A
    /// booking marked "Status unklar" was stored; the user has to check the
    /// account before trying again. Never resend automatically.
    OutcomeUnknown {
        reason: String,
        end_to_end_id: String,
    },
}

/// Result of the recipient name check (Verification of Payee).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PayeeMatch {
    /// Name matches the account holder (RCVC).
    Match,
    /// Name is close; `bank_name` holds the account holder's name (RVMC).
    CloseMatch,
    /// Name does not match the account holder (RVNM).
    NoMatch,
    /// The check could not be carried out (RVNA, or no result).
    NotPossible,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PayeeCheck {
    pub result: PayeeMatch,
    /// Name as entered by the user.
    pub entered_name: String,
    pub iban: String,
    /// Account holder's name reported by the recipient bank (close match).
    pub bank_name: Option<String>,
    /// Why the check was not possible (RVNA reason).
    pub reason: Option<String>,
    /// Bank's text on authorising despite a mismatch (must be shown).
    pub notice: Option<String>,
}

/// How an executed transfer was sent.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TransferOutcome {
    /// true = accepted as SEPA Instant, false = regular SEPA transfer.
    pub instant: bool,
    /// EndToEndId of the order (shows up in the bank statement).
    pub end_to_end_id: String,
}

/// Result of `TransferSession::cancel()`. Closing the session in GiroCrab is
/// not the same as the bank confirming that nothing can be paid any more.
/// Everything except `Cancelled` leaves the order tracked: `cancel()` can be
/// called again and re-checks the bank's status before anything is resent.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum CancelOutcome {
    /// Confirmed: no order exists at the bank, or the bank reports it as
    /// rejected/cancelled/expired.
    Cancelled,
    /// The bank took the cancellation request but still shows the order as
    /// open. Retry to re-check.
    CancellationPending { reason: String },
    /// The request or the status check got no answer (network, timeout):
    /// the order may or may not be cancelled. Retry to re-check.
    CancellationUnknown { reason: String },
    /// The bank refused the cancellation (HTTP error) and the order is
    /// still open, or it was already approved/executed. `reason` says which.
    CancellationFailed { reason: String },
}

/// Connector-side result of a transfer once it reached `Phase::Ready`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferResult {
    Executed {
        instant: bool,
    },
    /// Instant not possible, nothing approved or executed.
    InstantDeclined {
        reason: String,
    },
    /// Order sent, result unclear (may have been executed).
    Unknown {
        reason: String,
    },
}

impl Default for TransferResult {
    fn default() -> Self {
        TransferResult::Executed { instant: false }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct BankDescriptor {
    pub kind: ConnectorKind,
    pub name: String,
    pub protocol: String,
    pub bank_code: String,
    /// SF Symbol shown instead of a proprietary bank logo.
    pub sf_symbol: String,
    pub login_label: String,
    pub secret_label: String,
    /// Setting keys that must be filled before a sync can start.
    pub required_settings: Vec<String>,
    pub note: String,
}

// ------------------------------------------------------------------ connector trait

/// Answer to a challenge.
#[derive(Debug)]
pub enum ChallengeResponse {
    /// Ask the bank whether the decoupled approval happened.
    Poll,
    /// Code typed by the user (TAN, 2FA PIN, redirect URL/code).
    Code(SecretString),
    /// The user confirmed the transfer despite a name-check mismatch.
    ConfirmPayee,
}

#[derive(Debug)]
pub enum Phase {
    Ready,
    Challenge(Challenge),
    /// Decoupled approval still pending.
    Pending(Challenge),
    /// Transfers only: the name check needs the user's confirmation.
    ConfirmPayee(PayeeCheck),
}

/// Everything a connector needs to log in. Secrets are wiped on drop.
pub struct ConnectContext {
    pub connection_id: i64,
    pub login_name: String,
    pub secret: SecretString,
    /// Resolved settings (product ID, API keys ...), secrets included.
    pub settings: HashMap<String, SecretString>,
    /// Opaque connector state persisted between syncs (FinTS system id, PSD2 session ...).
    pub state: serde_json::Value,
    /// Fetch transactions booked on or after this date (YYYY-MM-DD): the
    /// earliest per-account start of [`since_by_account`](Self::since_by_account),
    /// so connectors that fetch all accounts at once miss none of them.
    pub since: String,
    /// Per-account start (account external ref → YYYY-MM-DD): the account's
    /// own newest booking minus 10 days. Accounts not listed use `since`.
    pub since_by_account: HashMap<String, String>,
}

/// Start dates of an incremental sync. `newest` = (account external ref,
/// newest stored booking date) of the connection's active accounts.
///
/// Each account starts 10 days before its *own* newest booking – one account
/// lagging behind (failed or rate-limited last time) must not be skipped
/// because its siblings moved on. The lower bound is `days` back, or earlier
/// when the whole connection has not been synced for longer. Accounts without
/// bookings (new ones, depots) get that lower bound but do not pull the
/// connection-wide `since` back.
pub(crate) fn since_window(
    today: chrono::NaiveDate,
    days: i64,
    newest: &[(String, Option<String>)],
) -> (String, HashMap<String, String>) {
    let parse = |d: &Option<String>| {
        d.as_deref()
            .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
    };
    let back10 = |d: chrono::NaiveDate| d - chrono::Duration::days(10);
    let fmt = |d: chrono::NaiveDate| d.format("%Y-%m-%d").to_string();
    let default_start = today - chrono::Duration::days(days);
    let floor = match newest.iter().filter_map(|(_, d)| parse(d)).max() {
        Some(g) => default_start.min(back10(g)),
        None => default_start,
    };
    let mut per = HashMap::new();
    let mut earliest: Option<chrono::NaiveDate> = None;
    for (r, d) in newest {
        let start = match parse(d) {
            Some(d) => {
                let s = back10(d).max(floor);
                earliest = Some(earliest.map_or(s, |e| e.min(s)));
                s
            }
            None => floor,
        };
        per.insert(r.clone(), fmt(start));
    }
    (fmt(earliest.unwrap_or(floor)), per)
}

impl ConnectContext {
    /// Start date for one account (see [`since_by_account`](Self::since_by_account)).
    pub fn since_for(&self, account_ref: &str) -> &str {
        self.since_by_account
            .get(account_ref)
            .map(String::as_str)
            .unwrap_or(&self.since)
    }

    pub fn setting(&self, key: &str) -> CoreResult<&str> {
        self.settings
            .get(key)
            .map(|s| s.expose())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CoreError::MissingSetting {
                key: key.to_string(),
            })
    }
    pub fn setting_or<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.settings
            .get(key)
            .map(|s| s.expose())
            .filter(|s| !s.is_empty())
            .unwrap_or(default)
    }
}

#[derive(Debug, Clone)]
pub struct RemoteAccount {
    /// Stable id at the bank (IBAN, account id, PSD2 uid).
    pub external_ref: String,
    pub name: String,
    pub owner: String,
    pub iban: String,
    pub account_number: String,
    pub currency: String,
    pub kind: AccountKind,
    pub balance_cents: i64,
    pub balance_date: String,
}

/// Securities position delivered by a connector for a depot account.
#[derive(Debug, Clone)]
pub struct RemoteHolding {
    pub account_ref: String,
    pub holding: crate::models::Holding,
}

#[derive(Debug, Clone, Default)]
pub struct RemoteTransaction {
    pub account_ref: String,
    /// De-duplication key; derived from bank ids or a content hash.
    pub external_id: String,
    pub booking_date: String,
    pub value_date: String,
    pub amount_cents: i64,
    pub currency: String,
    pub counterparty: String,
    pub counterparty_iban: String,
    pub purpose: String,
    pub booking_text: String,
    /// SEPA purpose code (ISO 20022 `Purp/Cd`, e.g. `SALA`); "" = unknown.
    pub purpose_code: String,
    /// German Geschäftsvorfallcode (3 digits, e.g. `153`); "" = unknown.
    pub gvc: String,
    /// SEPA end-to-end reference (`EndToEndId`); "" = unknown or `NOTPROVIDED`.
    pub end_to_end_id: String,
    /// SEPA direct-debit mandate reference; "" = unknown.
    pub mandate_id: String,
    /// SEPA creditor identifier (Gläubiger-ID) of a direct debit; "" = unknown.
    pub creditor_id: String,
}

impl RemoteTransaction {
    /// Booking content without the occurrence counter (dates, amount,
    /// counterparty, purpose), fields separated by U+001F.
    pub fn content_key(&self) -> String {
        format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
            self.booking_date, self.value_date, self.amount_cents, self.counterparty, self.purpose
        )
    }

    /// Content-based id for banks that don't deliver a transaction id.
    /// `occurrence` numbers identical bookings (same [`content_key`](Self::content_key))
    /// within one fetch, so two real, identical payments stay two rows.
    /// FNV-1a 64: stable across Rust versions (unlike `DefaultHasher`).
    pub fn content_id(&self, occurrence: u32) -> String {
        format!(
            "{CONTENT_ID_PREFIX}{:016x}",
            fnv1a64(&format!("{}\u{1f}{occurrence}", self.content_key()))
        )
    }
}

/// Prefix of [`RemoteTransaction::content_id`] ids. Ids of older versions
/// (`h` + 16 hex digits, `DefaultHasher`) are adopted on the next sync, see
/// [`adopt_legacy_content_id`].
pub(crate) const CONTENT_ID_PREFIX: char = 'c';

/// FNV-1a 64-bit hash.
pub(crate) fn fnv1a64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Hands out content ids, counting occurrences per identical content (not
/// per list position): the ids stay the same when the fetch window moves or
/// other bookings appear in between.
#[derive(Default)]
pub(crate) struct ContentIds(HashMap<String, u32>);

impl ContentIds {
    pub(crate) fn next(&mut self, t: &RemoteTransaction) -> String {
        let n = self.0.entry(t.content_key()).or_insert(0);
        let id = t.content_id(*n);
        *n += 1;
        id
    }
}

#[derive(Debug, Default, Clone)]
pub struct FetchResult {
    pub accounts: Vec<RemoteAccount>,
    pub transactions: Vec<RemoteTransaction>,
    /// Depot positions; replace the stored holdings of each listed account.
    pub holdings: Vec<RemoteHolding>,
    /// Explicitly fetched complete inventories, including empty depots.
    pub holdings_fetched: Vec<String>,
    /// Updated connector state to persist.
    pub state: Option<serde_json::Value>,
    /// Keep stored balances and add newly imported bookings to them
    /// (for sources that don't report a balance).
    pub adjust_balances: bool,
    /// The balances are simulated (demo bank): `adjust_balances` applies, but
    /// the accounts do not count as "balance computed from bookings" (#212).
    pub simulated: bool,
    /// Accounts (external refs) whose statements were fetched automatically.
    pub statements_fetched: Vec<String>,
    /// PDFs fetched from the bank (trade confirmations, PostBox, statements).
    pub documents: Vec<crate::documents::RemoteDocument>,
    /// external_id prefix of provisional bookings (card holds) this fetch
    /// lists completely: stored ones missing from it are removed.
    pub hold_prefix: Option<String>,
    /// `(old, new)` account external refs to rewrite before storing (a
    /// connector switched to a stable account key).
    pub renamed_refs: Vec<(String, String)>,
    /// External refs of accounts the bank / consent no longer grants access
    /// to: marked as revoked (kept with their bookings, never deleted).
    pub revoked_refs: Vec<String>,
    /// `accounts` is the complete list the bank grants access to (e.g. every
    /// account of a PSD2 consent was fetched): the connection's other
    /// accounts are marked as revoked.
    pub accounts_complete: bool,
    /// Bank name for these accounts when it differs from the connector's
    /// generic name (e.g. "C24" for an Enable Banking connection).
    pub bank_name: Option<String>,
    /// `(external_ref, old automatic name, new name)`: rename accounts that
    /// still carry the old automatic name; names the user chose are kept.
    pub name_updates: Vec<(String, String, String)>,
}

#[async_trait]
pub trait BankConnector: Send {
    /// Phase 1: log in, possibly returning a challenge.
    async fn begin(&mut self, ctx: &ConnectContext) -> CoreResult<Phase>;
    /// Phase 2: answer or poll a challenge.
    async fn respond(
        &mut self,
        ctx: &ConnectContext,
        response: ChallengeResponse,
    ) -> CoreResult<Phase>;
    /// Download accounts + transactions (only valid after `Ready`).
    async fn fetch(&mut self, ctx: &ConnectContext) -> CoreResult<FetchResult>;
    /// Close the bank session (FinTS dialog end, logout). Best effort.
    async fn finish(&mut self, _ctx: &ConnectContext) {}
    /// User cancelled a transfer that is not finished. Connectors whose order
    /// can outlive the session (bunq draft) override this to withdraw it and
    /// report whether the bank confirmed. Default: `finish`, nothing to withdraw.
    async fn cancel_transfer(&mut self, ctx: &ConnectContext) -> CancelOutcome {
        self.finish(ctx).await;
        CancelOutcome::Cancelled
    }
    /// Submit a SEPA credit transfer (only valid after `Ready`). `Ready`
    /// means the bank accepted it; challenges are answered via `respond`.
    async fn send_transfer(
        &mut self,
        _ctx: &ConnectContext,
        _job: &TransferJob,
    ) -> CoreResult<Phase> {
        Err(CoreError::InvalidInput {
            message: tr("Für diese Bank werden Überweisungen nicht unterstützt.").into(),
        })
    }
    /// After `send_transfer` reached `Ready`: executed (instant or regular),
    /// instant declined before any approval, or result unknown.
    /// Default: executed as a regular transfer.
    fn transfer_result(&self) -> TransferResult {
        TransferResult::default()
    }
    /// true once the order itself was handed to the bank. A connection error
    /// after that point means the result is unknown, not "not executed".
    fn order_submitted(&self) -> bool {
        false
    }
    /// true while the bank may still hold an order that could be paid
    /// (bunq: draft not confirmed rejected). Lets `cancel()` retry.
    fn has_open_order(&self) -> bool {
        false
    }
}

/// Transfer request plus the source account data resolved from the vault.
#[derive(Debug, Clone)]
pub struct TransferJob {
    pub source_iban: String,
    pub source_owner: String,
    pub recipient_name: String,
    pub recipient_iban: String,
    pub recipient_bic: Option<String>,
    pub amount_cents: i64,
    pub purpose: String,
    /// SEPA Instant requested. If the bank does not offer it, the connector
    /// reports `TransferResult::InstantDeclined`; it never resends on its own.
    pub instant: bool,
    pub execution_date: Option<String>,
    /// Stable per-transfer EndToEndId (max. 35 chars, `[A-Z0-9]`).
    pub end_to_end_id: String,
}
