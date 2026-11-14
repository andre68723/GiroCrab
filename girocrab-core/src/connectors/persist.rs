//! Vault side of syncs and transfers: storing fetch results, starting
//! sessions, statement automation, recipients and pending transfers.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use rusqlite::{params, OptionalExtension};

use crate::error::{CoreError, CoreResult};
use crate::i18n::{tr, trf};
use crate::models::{AccountKind, ConnectorKind, KnownRecipient, SepaTransferRequest};
use crate::secret::SecretString;
use crate::vault::Vault;

use super::*;

pub(super) fn is_content_id(id: &str, prefix: char) -> bool {
    let mut c = id.chars();
    id.len() == 17 && c.next() == Some(prefix) && c.all(|c| c.is_ascii_hexdigit())
}

/// One-time migration of ids from older versions: a booking stored under a
/// legacy content id (`h…`, `DefaultHasher` / list position) with exactly the
/// same content gets the new id instead of being imported a second time.
fn adopt_legacy_content_id(
    tx: &rusqlite::Connection,
    account_id: i64,
    t: &RemoteTransaction,
) -> CoreResult<()> {
    if !is_content_id(&t.external_id, CONTENT_ID_PREFIX) {
        return Ok(());
    }
    let pattern = format!("h{}", "[0-9a-f]".repeat(16));
    tx.execute(
        "UPDATE transactions SET external_id = ?1
          WHERE id = (SELECT id FROM transactions
                       WHERE account_id = ?2 AND external_id GLOB ?3
                         AND booking_date = ?4 AND value_date = ?5 AND amount_cents = ?6
                         AND counterparty = ?7 AND purpose = ?8
                       ORDER BY id LIMIT 1)
            AND NOT EXISTS (SELECT 1 FROM transactions WHERE account_id = ?2 AND external_id = ?1)",
        params![
            t.external_id,
            account_id,
            pattern,
            t.booking_date,
            t.value_date,
            t.amount_cents,
            t.counterparty,
            t.purpose
        ],
    )?;
    Ok(())
}

/// Persists accounts, bookings and documents of one fetch (shared by bank
/// syncs and the keyless web-portal / statement import).
pub(crate) fn store_fetch(
    vault: &Vault,
    kind: ConnectorKind,
    connection_id: i64,
    data: FetchResult,
) -> CoreResult<SyncSummary> {
    {
        let db = vault.db()?;
        let tx = db.unchecked_transaction()?;
        let generic_bank_name = descriptor(kind).name;
        // No name from the bank: the connection's display name (the bank picked
        // when adding it, or renamed later) beats the generic "FinTS-Bank".
        let display_name: Option<String> = tx
            .query_row(
                "SELECT display_name FROM bank_connections WHERE id = ?1",
                [connection_id],
                |r| r.get(0),
            )
            .optional()?;
        let bank_name = data
            .bank_name
            .clone()
            .into_iter()
            .chain(display_name)
            .map(|b| b.trim().to_string())
            .find(|b| !b.is_empty())
            .unwrap_or_else(|| generic_bank_name.clone());
        let (blocked_refs, pending_refs) =
            super::account_selection::prepare(&tx, kind, connection_id, &data)?;
        let deferred = (!pending_refs.is_empty())
            .then(|| super::account_selection::deferred(&data, &pending_refs));
        let mut data = data;
        // safety net for every connector: repair UTF-8/Latin-1 mojibake
        for a in &mut data.accounts {
            a.name = crate::text::clean_bank_text(&a.name);
            a.owner = crate::text::clean_bank_text(&a.owner);
        }
        for t in &mut data.transactions {
            t.counterparty = crate::text::clean_bank_text(&t.counterparty);
            t.purpose = crate::text::clean_bank_text(&t.purpose);
            t.booking_text = crate::text::clean_bank_text(&t.booking_text);
        }
        // A connector without a bank id must not collapse bookings into one
        // `(account, "")` row: give them a deterministic content id instead.
        let mut missing_ids: HashMap<String, ContentIds> = HashMap::new();
        for t in &mut data.transactions {
            if t.external_id.trim().is_empty() {
                t.external_id = missing_ids
                    .entry(t.account_ref.clone())
                    .or_default()
                    .next(t);
            }
        }
        for (old, new) in &data.renamed_refs {
            if old.is_empty() || new.is_empty() || old == new {
                continue;
            }
            // includes soft-deleted rows (the new ref must stay unique)
            tx.execute(
                "UPDATE accounts SET external_ref = ?3
                 WHERE connection_id = ?1 AND external_ref = ?2
                   AND NOT EXISTS (SELECT 1 FROM accounts WHERE connection_id = ?1 AND external_ref = ?3)",
                params![connection_id, old, new],
            )?;
        }
        for (r, old_name, new_name) in &data.name_updates {
            if new_name.trim().is_empty() || old_name == new_name {
                continue;
            }
            tx.execute(
                "UPDATE accounts SET name = ?4 WHERE connection_id = ?1 AND external_ref = ?2 AND name = ?3",
                params![connection_id, r, old_name, new_name],
            )?;
        }
        let mut account_ids: HashMap<String, i64> = HashMap::new();
        // Archived accounts the bank still delivers: nothing is stored for
        // them (no balance, bookings, positions or documents).
        let mut archived_ids: HashMap<String, i64> = HashMap::new();
        // the deleted ones among them (their bookings are not "skipped")
        let mut deleted_refs: HashSet<String> = HashSet::new();
        let mut accounts_updated = 0u32;
        let had_accounts: i64 = tx.query_row(
            "SELECT count(*) FROM accounts WHERE connection_id = ?1 AND deleted_ms IS NULL",
            [connection_id],
            |r| r.get(0),
        )?;
        let mut new_accounts: Vec<String> = Vec::new();
        for acc in &data.accounts {
            if blocked_refs.contains(&acc.external_ref) {
                continue;
            }
            let existing: Option<i64> = tx
                .query_row(
                    // includes soft-deleted rows
                    "SELECT id FROM accounts WHERE connection_id = ?1 AND external_ref = ?2",
                    params![connection_id, acc.external_ref],
                    |r| r.get(0),
                )
                .optional()?;
            // First sync after a MoneyMoney database import: take over the
            // unconnected imported account with the same IBAN (its bookings are
            // then adopted via the import fingerprint) instead of a second account.
            let existing = match existing {
                Some(id) => Some(id),
                None => crate::importers::moneymoney_db::adopt_account(
                    &tx,
                    connection_id,
                    &acc.external_ref,
                    &acc.iban,
                    &acc.currency,
                    acc.kind,
                )?,
            };
            // Sub-accounts known from an earlier import/sync without a connection
            // (e.g. comdirect Depot / Tagesgeld): link them to this connection.
            let existing = match existing {
                Some(id) => Some(id),
                None => adopt_unlinked(&tx, connection_id, &bank_name, acc)?,
            };
            if let Some(id) = existing {
                // Deleted accounts stay deleted (like archived ones, nothing is
                // stored), unless the connection was added again since.
                if crate::sync_merge::fetch_keeps_account_deleted(&tx, connection_id, id)? {
                    deleted_refs.insert(acc.external_ref.clone());
                    archived_ids.insert(acc.external_ref.clone(), id);
                    continue;
                }
                crate::sync_merge::revive_account_if_deleted(&tx, id)?;
                let archived: bool = tx.query_row(
                    "SELECT archived_unix IS NOT NULL FROM accounts WHERE id = ?1",
                    [id],
                    |r| r.get(0),
                )?;
                if archived {
                    archived_ids.insert(acc.external_ref.clone(), id);
                    continue;
                }
            }
            let is_new = existing.is_none();
            let id = match existing {
                Some(id) => {
                    if !data.adjust_balances {
                        // A card reporting 0 (no /balances data) must not wipe a known balance.
                        let keep = acc.balance_cents == 0
                            && acc.kind == AccountKind::CreditCard
                            && tx.query_row(
                                "SELECT balance_cents FROM accounts WHERE id = ?1",
                                [id],
                                |r| r.get::<_, i64>(0),
                            )? != 0;
                        if keep {
                            tx.execute(
                                "UPDATE accounts SET currency = ?1 WHERE id = ?2",
                                params![acc.currency, id],
                            )?;
                        } else {
                            tx.execute(
                                "UPDATE accounts SET balance_cents = ?1, balance_date = ?2, currency = ?3 WHERE id = ?4",
                                params![acc.balance_cents, acc.balance_date, acc.currency, id],
                            )?;
                        }
                    }
                    // Bank name / owner learned later (e.g. "C24" instead of the
                    // generic "Bank per PSD2"); never overwrite what is set.
                    if bank_name != generic_bank_name {
                        tx.execute(
                            "UPDATE accounts SET bank_name = ?1 WHERE id = ?2 AND bank_name IN (?3, '')",
                            params![bank_name, id, generic_bank_name],
                        )?;
                    }
                    if !acc.owner.is_empty() {
                        tx.execute(
                            "UPDATE accounts SET owner = ?1 WHERE id = ?2 AND IFNULL(owner, '') = ''",
                            params![acc.owner, id],
                        )?;
                    }
                    // Fill in an account number learned later (e.g. a BBAN of a
                    // PSD2 account without IBAN); never overwrite a known one.
                    if !acc.account_number.is_empty() {
                        tx.execute(
                            "UPDATE accounts SET account_number = ?1 WHERE id = ?2 AND IFNULL(account_number, '') = ''",
                            params![acc.account_number, id],
                        )?;
                    }
                    // Trade Republic cash side is a clearing account, not a Girokonto.
                    if kind == ConnectorKind::TradeRepublic
                        && acc.kind != AccountKind::Brokerage
                        && acc.name == "Verrechnungskonto"
                    {
                        tx.execute(
                            "UPDATE accounts SET name = ?1 WHERE id = ?2 AND name = 'Girokonto'",
                            params![acc.name, id],
                        )?;
                    }
                    // Brokerage: depots found later. Savings: PSD2 savings accounts
                    // first stored as checking (e.g. C24 PocketZINS, typed by product).
                    if acc.kind == AccountKind::Brokerage || acc.kind == AccountKind::Savings {
                        let (group, sym, order) = group_for(acc.kind);
                        let gid = Vault::ensure_group(&tx, group, sym, order)?;
                        tx.execute(
                            // a symbol the user picked (symbol_ms > 0) stays
                            "UPDATE accounts SET kind = ?1, sf_symbol = CASE WHEN symbol_ms > 0 THEN sf_symbol ELSE ?2 END, group_id = ?3 WHERE id = ?4 AND kind <> ?1",
                            params![acc.kind.as_str(), acc.kind.sf_symbol(), gid, id],
                        )?;
                    }
                    id
                }
                None => {
                    let (group, sym, order) = group_for(acc.kind);
                    let gid = Vault::ensure_group(&tx, group, sym, order)?;
                    tx.execute(
                        "INSERT INTO accounts(connection_id, group_id, external_ref, name, bank_name, owner, iban, account_number, currency, kind,
                                              balance_cents, balance_date, sf_symbol, sort_order, created_ms)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, (SELECT IFNULL(MAX(sort_order),0)+1 FROM accounts), ?14)",
                        params![connection_id, gid, acc.external_ref, acc.name, bank_name, acc.owner, acc.iban, acc.account_number,
                                acc.currency, acc.kind.as_str(), acc.balance_cents, acc.balance_date, acc.kind.sf_symbol(),
                                crate::user_groups::now_ms()],
                    )?;
                    let id = tx.last_insert_rowid();
                    crate::user_groups::place_new_account(&tx, id)?;
                    id
                }
            };
            accounts_updated += 1;
            account_ids.insert(acc.external_ref.clone(), id);
            if is_new && had_accounts > 0 {
                new_accounts.push(acc.name.clone());
            }
            // Delivered again: access is back (e.g. included in a new consent).
            tx.execute(
                "UPDATE accounts SET access_revoked = 0 WHERE id = ?1 AND access_revoked != 0",
                [id],
            )?;
        }
        let mut revoked_accounts: Vec<String> = Vec::new();
        let mut revoked_refs = data.revoked_refs.clone();
        // Archived accounts are the user's business: skipped above even when
        // the bank still delivers them, so they must not count as "missing".
        let delivered = |r: &str| {
            account_ids.contains_key(r) || archived_ids.contains_key(r) || blocked_refs.contains(r)
        };
        if data.accounts_complete && !data.accounts.is_empty() {
            let mut q = tx.prepare(
                "SELECT external_ref FROM accounts
                 WHERE connection_id = ?1 AND access_revoked = 0 AND archived_unix IS NULL AND deleted_ms IS NULL",
            )?;
            let stored: Vec<String> = q
                .query_map([connection_id], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            drop(q);
            revoked_refs.extend(stored.into_iter().filter(|r| !delivered(r)));
        }
        revoked_refs.sort();
        revoked_refs.dedup();
        for r in revoked_refs
            .iter()
            .filter(|r| !r.is_empty() && !delivered(r))
        {
            let name: Option<String> = tx
                .query_row(
                    "SELECT name FROM accounts WHERE connection_id = ?1 AND external_ref = ?2
                       AND access_revoked = 0 AND archived_unix IS NULL AND deleted_ms IS NULL",
                    params![connection_id, r],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(name) = name {
                tx.execute(
                    "UPDATE accounts SET access_revoked = 1 WHERE connection_id = ?1 AND external_ref = ?2",
                    params![connection_id, r],
                )?;
                revoked_accounts.push(name);
            }
        }
        // #212: a connector without balances accumulates the bookings; such an
        // account's balance can start from an opening balance.
        if !data.simulated {
            for acc in &data.accounts {
                if let Some(&id) = account_ids.get(&acc.external_ref) {
                    tx.execute(
                        "UPDATE accounts SET balance_derived = ?1 WHERE id = ?2",
                        params![data.adjust_balances, id],
                    )?;
                }
            }
        }
        // Credit cards without a connector balance: derive the open amount.
        if !data.adjust_balances {
            for acc in data.accounts.iter().filter(|a| a.balance_cents == 0) {
                let Some(&id) = account_ids.get(&acc.external_ref) else {
                    continue;
                };
                let stored: i64 = tx.query_row(
                    "SELECT balance_cents FROM accounts WHERE id = ?1",
                    [id],
                    |r| r.get(0),
                )?;
                if stored != 0 {
                    continue;
                }
                let is_card = acc.kind == AccountKind::CreditCard
                    || is_amex_name(&acc.name)
                    || is_amex_name(bank_name.as_str());
                if !is_card {
                    continue;
                }
                let mut st = tx.prepare(
                    "SELECT booking_date, amount_cents, counterparty || ' ' || purpose || ' ' || booking_text
                     FROM transactions WHERE account_id = ?1 AND deleted_ms IS NULL ORDER BY booking_date, id",
                )?;
                let rows: Vec<(String, i64, String)> = st
                    .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                    .collect::<Result<_, _>>()?;
                drop(st);
                if let Some(open) = open_card_balance(&rows) {
                    tx.execute(
                        "UPDATE accounts SET balance_cents = ?1 WHERE id = ?2",
                        params![open, id],
                    )?;
                }
            }
        }
        // Resolve each position to a stored account: external ref, else IBAN /
        // account number (depots often have no IBAN, numbers differ in leading
        // zeros), else the only depot of this fetch.
        let norm = |x: &str| {
            x.chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>()
                .trim_start_matches('0')
                .to_uppercase()
        };
        let depots: Vec<&RemoteAccount> = data
            .accounts
            .iter()
            .filter(|a| !archived_ids.contains_key(&a.external_ref))
            .filter(|a| a.kind == AccountKind::Brokerage || a.name.to_lowercase().contains("depot"))
            .collect();
        let resolve = |r: &str| -> Option<i64> {
            if let Some(id) = account_ids.get(r) {
                return Some(*id);
            }
            let n = norm(r);
            if !n.is_empty() {
                for a in &data.accounts {
                    if [&a.external_ref, &a.iban, &a.account_number]
                        .iter()
                        .any(|f| !f.is_empty() && norm(f) == n)
                    {
                        return account_ids.get(&a.external_ref).copied();
                    }
                }
            }
            match depots.as_slice() {
                [only] => account_ids.get(&only.external_ref).copied(),
                _ => None,
            }
        };
        let mut by_account: HashMap<i64, Vec<&crate::models::Holding>> = HashMap::new();
        // An explicitly empty inventory must remove stale positions too.
        for account_ref in &data.holdings_fetched {
            if let Some(id) = account_ids.get(account_ref) {
                by_account.entry(*id).or_default();
            }
        }
        let mut unresolved = 0usize;
        for h in &data.holdings {
            if let Some(id) = resolve(&h.account_ref) {
                by_account.entry(id).or_default().push(&h.holding);
            } else {
                unresolved += 1;
            }
        }
        crate::depot_debug::log(&format!(
            "store_fetch {kind:?}: incoming_holdings={} depot_accounts={} resolved_accounts={} unresolved={unresolved}",
            data.holdings.len(),
            depots.len(),
            by_account.len()
        ));
        for (acc_id, list) in &by_account {
            tx.execute("DELETE FROM holdings WHERE account_id = ?1", [acc_id])?;
            for h in list {
                tx.execute(
                    "INSERT INTO holdings(account_id, isin, wkn, name, quantity, current_price_cents, purchase_price_cents, value_cents)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![acc_id, h.isin, h.wkn, h.name, h.quantity, h.current_price_cents, h.purchase_price_cents, h.value_cents],
                )?;
            }
        }
        let mut imported = 0u32;
        let mut skipped_archived = 0u32;
        {
            let mut ins = tx.prepare(
                "INSERT OR IGNORE INTO transactions(account_id, external_id, booking_date, value_date, amount_cents, currency,
                         counterparty, counterparty_iban, purpose, booking_text, purpose_code, gvc,
                         end_to_end_id, mandate_id, creditor_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            )?;
            for t in &data.transactions {
                let Some(acc_id) = account_ids.get(&t.account_ref) else {
                    if let Some(id) = archived_ids.get(&t.account_ref) {
                        let known: bool = tx.query_row(
                            // includes soft-deleted rows
                            "SELECT EXISTS (SELECT 1 FROM transactions WHERE account_id = ?1 AND external_id = ?2)",
                            params![id, t.external_id],
                            |r| r.get(0),
                        )?;
                        if !deleted_refs.contains(&t.account_ref) {
                            skipped_archived += u32::from(!known);
                        }
                    }
                    continue;
                };
                adopt_legacy_content_id(&tx, *acc_id, t)?;
                // Deleted bookings stay deleted: neither inserted nor updated again.
                if crate::sync_merge::booking_is_deleted(&tx, *acc_id, &t.external_id)? {
                    continue;
                }
                // Already imported from MoneyMoney? Adopt that row (keeps category + note).
                if crate::importers::moneymoney_import::adopt_import(
                    &tx,
                    *acc_id,
                    &t.external_id,
                    &t.booking_date,
                    t.amount_cents,
                    &t.counterparty_iban,
                    &t.counterparty,
                    &t.purpose,
                    &t.booking_text,
                )? {
                    continue;
                }
                let n = ins.execute(params![
                    acc_id,
                    t.external_id,
                    t.booking_date,
                    t.value_date,
                    t.amount_cents,
                    t.currency,
                    t.counterparty,
                    t.counterparty_iban,
                    t.purpose,
                    t.booking_text,
                    t.purpose_code,
                    t.gvc,
                    t.end_to_end_id,
                    t.mandate_id,
                    t.creditor_id
                ])? as u32;
                if n > 0 && data.adjust_balances {
                    tx.execute(
                        "UPDATE accounts SET balance_cents = balance_cents + ?1, balance_date = ?2 WHERE id = ?3",
                        params![t.amount_cents, t.booking_date, acc_id],
                    )?;
                }
                imported += n;
                if n == 0 {
                    // Old duplicate: bank row exists although an MoneyMoney twin does too.
                    if crate::importers::moneymoney_import::merge_synced_twin(
                        &tx,
                        *acc_id,
                        &t.external_id,
                        &t.booking_date,
                        t.amount_cents,
                        &t.counterparty_iban,
                        &t.counterparty,
                        &t.purpose,
                    )? {
                        continue;
                    }
                    // Existing row: heal corrupted text (U+FFFD / mojibake) with freshly decoded text.
                    let bad = |s: &str| s.contains('\u{FFFD}') || crate::text::has_mojibake(s);
                    let good = |s: &str| !bad(s);
                    let existing: Option<(i64, String, String)> = tx
                        .query_row(
                            // includes soft-deleted rows
                            "SELECT id, counterparty, purpose FROM transactions WHERE account_id = ?1 AND external_id = ?2",
                            params![acc_id, t.external_id],
                            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                        )
                        .ok();
                    if let Some((row_id, cp, pu)) = existing {
                        // Backfill bank codes for bookings imported before they were stored.
                        if !t.purpose_code.is_empty() || !t.gvc.is_empty() {
                            tx.execute(
                                "UPDATE transactions SET purpose_code = ?1, gvc = ?2
                                 WHERE id = ?3 AND purpose_code = '' AND gvc = ''",
                                params![t.purpose_code, t.gvc, row_id],
                            )?;
                        }
                        // Same for the SEPA references (bookings imported before step 12).
                        if !t.end_to_end_id.is_empty()
                            || !t.mandate_id.is_empty()
                            || !t.creditor_id.is_empty()
                        {
                            tx.execute(
                                "UPDATE transactions SET end_to_end_id = ?1, mandate_id = ?2, creditor_id = ?3
                                 WHERE id = ?4 AND end_to_end_id = '' AND mandate_id = '' AND creditor_id = ''",
                                params![t.end_to_end_id, t.mandate_id, t.creditor_id, row_id],
                            )?;
                        }
                        if bad(&cp) && good(&t.counterparty) {
                            tx.execute(
                                "UPDATE transactions SET counterparty = ?1 WHERE id = ?2",
                                params![t.counterparty, row_id],
                            )?;
                        }
                        if bad(&pu) && good(&t.purpose) {
                            tx.execute(
                                "UPDATE transactions SET purpose = ?1 WHERE id = ?2",
                                params![t.purpose, row_id],
                            )?;
                        }
                    }
                }
            }
        }
        // Drop optimistic transfer bookings once the bank's real booking arrived
        // (same account, amount and recipient IBAN) or after 14 days.
        tx.execute(
            "UPDATE transactions SET deleted_ms = ?1
              WHERE external_id LIKE 'pending-%' AND deleted_ms IS NULL AND (
                imported_unix < strftime('%s','now') - 14*86400 OR EXISTS (
                    SELECT 1 FROM transactions r WHERE r.account_id = transactions.account_id
                      AND r.deleted_ms IS NULL
                      AND r.external_id NOT LIKE 'pending-%' AND r.amount_cents = transactions.amount_cents
                      AND r.counterparty_iban = transactions.counterparty_iban
                      AND r.booking_date >= transactions.booking_date))",
            [crate::user_groups::now_ms()],
        )?;
        // Card holds: keep exactly the ones the bank still reports as open.
        if let Some(prefix) = data.hold_prefix.as_deref().filter(|p| !p.is_empty()) {
            let current: std::collections::HashSet<&str> = data
                .transactions
                .iter()
                .filter(|t| t.external_id.starts_with(prefix))
                .map(|t| t.external_id.as_str())
                .collect();
            let like = format!("{}%", prefix.replace('%', "").replace('_', "\\_"));
            for acc_id in account_ids.values() {
                let stale: Vec<(i64, String)> = {
                    let mut q = tx.prepare(
                        "SELECT id, external_id FROM transactions
                         WHERE account_id = ?1 AND external_id LIKE ?2 ESCAPE '\\' AND deleted_ms IS NULL",
                    )?;
                    let rows =
                        q.query_map(params![acc_id, like], |r| Ok((r.get(0)?, r.get(1)?)))?;
                    rows.collect::<Result<_, _>>()?
                };
                for (row_id, ext) in stale {
                    if !current.contains(ext.as_str()) {
                        crate::sync_merge::soft_delete_booking(&tx, row_id)?;
                    }
                }
            }
        }
        for d in &data.documents {
            if archived_ids.contains_key(&d.account_ref) || blocked_refs.contains(&d.account_ref) {
                continue;
            }
            let by_ref = |tx: &rusqlite::Transaction| -> Option<i64> {
                tx.query_row(
                    // includes soft-deleted rows
                    "SELECT id FROM accounts WHERE connection_id = ?1 AND external_ref = ?2",
                    params![connection_id, d.account_ref],
                    |r| r.get(0),
                )
                .optional()
                .ok()
                .flatten()
            };
            let Some(acc_id) = account_ids
                .get(&d.account_ref)
                .copied()
                .or_else(|| by_ref(&tx))
                .or_else(|| account_ids.values().next().copied())
            else {
                continue;
            };
            let booking_id: Option<i64> = match &d.booking_external_id {
                Some(ext) => tx
                    .query_row(
                        // includes soft-deleted rows
                        "SELECT id FROM transactions WHERE account_id = ?1 AND external_id = ?2",
                        params![acc_id, ext],
                        |r| r.get(0),
                    )
                    .optional()?,
                None => None,
            };
            let _ = crate::documents::insert_doc(
                &tx,
                booking_id,
                Some(acc_id),
                &d.title,
                &d.doc_type,
                &d.mime_type,
                &d.file_name,
                &d.bytes,
                &d.source_url,
                &d.date,
            );
        }
        for r in &data.statements_fetched {
            if let Some(id) = account_ids.get(r) {
                tx.execute(
                    "INSERT INTO settings(key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![statements_last_key(*id), today()],
                )?;
            }
        }
        for acc in data.accounts.iter().filter(|a| a.kind == AccountKind::Loan) {
            if let Some(id) = account_ids.get(&acc.external_ref) {
                Vault::derive_initial_balance(&tx, *id)?;
            }
        }
        let categorized = Vault::apply_rules_inner(&tx, true)?;
        if let Some(state) = data.state {
            tx.execute(
                "UPDATE bank_connections SET state_json = ?1 WHERE id = ?2",
                params![state.to_string(), connection_id],
            )?;
        }
        tx.execute(
            "UPDATE bank_connections SET last_sync_unix = strftime('%s','now') WHERE id = ?1",
            [connection_id],
        )?;
        tx.commit()?;
        if let Some(data) = deferred {
            vault
                .pending_account_fetches
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(connection_id, (kind, data));
        }
        Ok(SyncSummary {
            accounts_updated,
            transactions_imported: imported,
            transactions_categorized: categorized,
            transactions_skipped_archived: skipped_archived,
            new_accounts,
            revoked_accounts,
        })
    }
}

/// Sidebar sections (fixed order).
pub(crate) fn group_for(kind: AccountKind) -> (&'static str, &'static str, i32) {
    match kind {
        AccountKind::Checking
        | AccountKind::Savings
        | AccountKind::Cash
        | AccountKind::PaymentService => ("Giro & Tagesgeld", "building.columns", 1),
        AccountKind::CreditCard => ("Kreditkarten", "creditcard", 2),
        AccountKind::Brokerage => ("Depots & ETFs", "chart.pie", 3),
        AccountKind::Loan => ("Darlehen & Kredite", "house", 4),
    }
}

/// Setting key marking a statements-only session (value = external_ref of the account).
pub(crate) const STATEMENTS_ONLY: &str = "__statements_only";
/// Setting listing the accounts (external refs, comma-separated) whose PDF
/// statements a regular sync should also fetch (opt-in, GitHub #57).
pub(crate) const STATEMENTS_AUTO: &str = "__statements_auto";
/// Days between two automatic statement fetches of one account.
const STATEMENTS_AUTO_DAYS: i64 = 7;

/// Vault setting key: fetch PDF statements of this account automatically.
pub(crate) fn statements_auto_key(account_id: i64) -> String {
    format!("statements.auto.{account_id}")
}

/// Vault setting key: date (YYYY-MM-DD) of the last automatic statement fetch.
pub(crate) fn statements_last_key(account_id: i64) -> String {
    format!("statements.last.{account_id}")
}

#[uniffi::export]
impl Vault {
    /// Prepares a sync for one bank connection. `secret` is the PIN/password
    /// (wiped from memory when the session ends). With `demo = true` the bank
    /// is simulated (push-TAN / code challenge included) – no network.
    pub fn begin_sync(
        self: Arc<Self>,
        connection_id: i64,
        secret: String,
        demo: bool,
    ) -> Result<Arc<SyncSession>, CoreError> {
        self.begin_sync_inner(connection_id, secret, demo, None)
    }

    /// Turns the automatic PDF statement fetch (FinTS HKEKA, at most weekly,
    /// during regular syncs) of one account on or off.
    pub fn set_auto_statements(&self, account_id: i64, enabled: bool) -> Result<(), CoreError> {
        let db = self.db()?;
        if enabled {
            db.execute(
                "INSERT INTO settings(key, value) VALUES (?1, '1')
                 ON CONFLICT(key) DO UPDATE SET value = '1'",
                [statements_auto_key(account_id)],
            )?;
        } else {
            db.execute(
                "DELETE FROM settings WHERE key IN (?1, ?2)",
                params![
                    statements_auto_key(account_id),
                    statements_last_key(account_id)
                ],
            )?;
        }
        Ok(())
    }

    /// Is the automatic statement fetch on for this account?
    pub fn auto_statements_enabled(&self, account_id: i64) -> Result<bool, CoreError> {
        let db = self.db()?;
        Ok(Vault::setting_inner(&db, &statements_auto_key(account_id))?.as_deref() == Some("1"))
    }

    /// Explicitly fetches the electronic bank statements (FinTS HKEKA, PDF) of ONE account
    /// into the document archive. Same session / TAN flow as `begin_sync`; bookings and
    /// balances stay untouched. Regular syncs never fetch statements.
    pub fn fetch_account_statements(
        self: Arc<Self>,
        account_id: i64,
        secret: String,
        demo: bool,
    ) -> Result<Arc<SyncSession>, CoreError> {
        let (conn_id, ext): (Option<i64>, String) = {
            let db = self.db()?;
            db.query_row(
                "SELECT connection_id, external_ref FROM accounts WHERE id = ?1 AND deleted_ms IS NULL",
                [account_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| CoreError::NotFound {
                what: trf("Konto {0}", &[&account_id]),
            })?
        };
        let conn_id = conn_id.ok_or_else(|| CoreError::InvalidInput {
            message: tr("Dieses Konto hat keine Bankverbindung.").into(),
        })?;
        let kind: String = self.db()?.query_row(
            "SELECT connector FROM bank_connections WHERE id = ?1",
            [conn_id],
            |r| r.get(0),
        )?;
        let is_fints = ConnectorKind::from_str(&kind).is_some_and(|k| {
            matches!(
                k,
                ConnectorKind::FinTsGeneric
                    | ConnectorKind::VolksbankKasselGoettingen
                    | ConnectorKind::Direkt1822
            )
        });
        if !is_fints {
            return Err(CoreError::InvalidInput {
                message: tr("Kontoauszüge können nur per FinTS abgerufen werden.").into(),
            });
        }
        self.begin_sync_inner(conn_id, secret, demo, Some(ext))
    }
}

impl Vault {
    /// External refs of this connection's accounts whose automatic statement
    /// fetch is on and due (never, or more than `STATEMENTS_AUTO_DAYS` ago).
    pub(super) fn statements_due(&self, connection_id: i64) -> CoreResult<Vec<String>> {
        let db = self.db()?;
        let mut stmt = db.prepare(
            "SELECT id, external_ref FROM accounts WHERE connection_id = ?1 AND kind != 'loan' AND deleted_ms IS NULL",
        )?;
        let accounts: Vec<(i64, String)> = stmt
            .query_map([connection_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        let today = chrono::Local::now().date_naive();
        let mut due = Vec::new();
        for (id, ext) in accounts {
            if Vault::setting_inner(&db, &statements_auto_key(id))?.as_deref() != Some("1") {
                continue;
            }
            let last = Vault::setting_inner(&db, &statements_last_key(id))?
                .and_then(|d| chrono::NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok());
            if last.is_none_or(|d| (today - d).num_days() >= STATEMENTS_AUTO_DAYS) {
                due.push(ext);
            }
        }
        Ok(due)
    }

    fn begin_sync_inner(
        self: Arc<Self>,
        connection_id: i64,
        secret: String,
        demo: bool,
        statements_only: Option<String>,
    ) -> Result<Arc<SyncSession>, CoreError> {
        // No PIN typed: use the one stored in the vault ("PIN im Tresor speichern").
        let secret = if secret.is_empty() {
            let db = self.db()?;
            Vault::setting_inner(&db, &crate::connections::pin_key(connection_id))?
                .unwrap_or_default()
        } else {
            secret
        };
        let secret = SecretString::new(secret);
        let (kind, key_str, login, state_json, ctx_settings, since, demo) = {
            let db = self.db()?;
            let (kind, login, state): (String, String, String) = db
                .query_row(
                    "SELECT connector, login_name, state_json FROM bank_connections WHERE id = ?1 AND deleted_ms IS NULL",
                    [connection_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?
                .ok_or_else(|| CoreError::NotFound {
                    what: trf("Bankverbindung {0}", &[&connection_id]),
                })?;
            let key_str = kind.clone();
            // Magic demo login ("demo"): simulated bank, no network, no FinTS product ID.
            let magic = crate::demo::is_magic_demo_login(&login);
            let demo = demo || magic;
            if !magic {
                crate::banking_gate::check_connector(&key_str)?;
            }
            let kind = ConnectorKind::from_str(&kind).ok_or_else(|| CoreError::InvalidInput {
                message: format!("unknown connector {kind}"),
            })?;
            let mut settings = HashMap::new();
            for (key, ..) in crate::vault::KNOWN_SETTINGS {
                if let Some(v) = Vault::setting_inner(&db, key)? {
                    settings.insert(key.to_string(), SecretString::new(v));
                }
            }
            if descriptor(kind)
                .required_settings
                .iter()
                .any(|k| k == fints::product_id::OWN_KEY)
            {
                let mode =
                    Vault::setting_inner(&db, fints::product_id::MODE_KEY)?.unwrap_or_default();
                let own =
                    Vault::setting_inner(&db, fints::product_id::OWN_KEY)?.unwrap_or_default();
                if let Some((_, id)) = fints::product_id::resolve(&mode, &own) {
                    settings.insert(
                        fints::product_id::OWN_KEY.to_string(),
                        SecretString::new(id),
                    );
                }
            }
            if let Some(blz) = ConnectorKind::fints_blz(&key_str) {
                let k = format!("fints.url.{blz}");
                match Vault::setting_inner(&db, &k)? {
                    Some(v) if !v.is_empty() => {
                        settings.insert(k, SecretString::new(v));
                    }
                    // e.g. a MoneyMoney import created the login without a URL
                    _ => {
                        if let Some(url) = fints_directory::url_for_blz(blz) {
                            settings.insert(k, SecretString::new(url.to_string()));
                        }
                    }
                }
            }
            let tm_key = format!("fints.tan_medium.{connection_id}");
            if let Some(v) = Vault::setting_inner(&db, &tm_key)? {
                if !v.trim().is_empty() {
                    settings.insert(tm_key, SecretString::new(v));
                }
            }
            if !demo {
                for key in descriptor(kind).required_settings {
                    if !settings.contains_key(&key) {
                        return Err(CoreError::MissingSetting { key });
                    }
                }
            }
            // Incremental, per account: from its newest booking minus 10 days.
            let newest: Vec<(String, Option<String>)> = {
                let mut st = db.prepare(
                    "SELECT a.external_ref,
                            (SELECT MAX(t.booking_date) FROM transactions t
                              WHERE t.account_id = a.id AND t.external_id NOT LIKE 'pending-%' AND t.deleted_ms IS NULL)
                       FROM accounts a
                      WHERE a.connection_id = ?1 AND a.archived_unix IS NULL AND a.deleted_ms IS NULL",
                )?;
                let rows = st
                    .query_map([connection_id], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                rows
            };
            let days: i64 = Vault::setting_inner(&db, "sync.days_back")?
                .and_then(|d| d.parse().ok())
                .unwrap_or(90);
            let since = since_window(chrono::Local::now().date_naive(), days, &newest);
            (kind, key_str, login, state, settings, since, demo)
        };
        let (since, since_by_account) = since;
        let mut ctx_settings = ctx_settings;
        if let Some(ext) = statements_only {
            ctx_settings.insert(STATEMENTS_ONLY.into(), SecretString::new(ext));
        } else {
            let due = self.statements_due(connection_id)?;
            if !due.is_empty() {
                ctx_settings.insert(STATEMENTS_AUTO.into(), SecretString::new(due.join(",")));
            }
        }
        let ctx = ConnectContext {
            connection_id,
            login_name: login,
            secret,
            settings: ctx_settings,
            state: serde_json::from_str(&state_json).unwrap_or(serde_json::json!({})),
            since,
            since_by_account,
        };
        Ok(Arc::new(SyncSession::new(
            self.clone(),
            kind,
            make_connector_for(kind, &key_str, demo),
            ctx,
        )))
    }
}

#[uniffi::export]
impl Vault {
    /// Prepares a SEPA transfer from a FinTS account. `secret` = PIN (empty =
    /// PIN stored in the vault). `demo` simulates the bank.
    pub fn begin_transfer(
        self: Arc<Self>,
        request: SepaTransferRequest,
        secret: String,
        demo: bool,
    ) -> Result<Arc<TransferSession>, CoreError> {
        use crate::connectors::fints::sepa;
        if request.amount_cents <= 0 {
            return Err(CoreError::InvalidInput {
                message: tr("Der Betrag muss größer als 0 sein.").into(),
            });
        }
        if !sepa::validate_iban(&request.recipient_iban) {
            return Err(CoreError::InvalidInput {
                message: tr("Die Empfänger-IBAN ist ungültig.").into(),
            });
        }
        if let Some(b) = request
            .recipient_bic
            .as_deref()
            .filter(|b| !b.trim().is_empty())
        {
            if !sepa::validate_bic(b) {
                return Err(CoreError::InvalidInput {
                    message: tr("Die BIC ist ungültig.").into(),
                });
            }
        }
        if request.recipient_name.trim().is_empty() {
            return Err(CoreError::InvalidInput {
                message: tr("Der Empfängername fehlt.").into(),
            });
        }
        if request.purpose.chars().count() > 140 {
            return Err(CoreError::InvalidInput {
                message: tr("Der Verwendungszweck darf höchstens 140 Zeichen lang sein.").into(),
            });
        }
        let (connection_id, iban, owner) = {
            let db = self.db()?;
            let row: (Option<i64>, String, String) = db
                .query_row(
                    "SELECT connection_id, iban, owner FROM accounts WHERE id = ?1 AND deleted_ms IS NULL",
                    [request.account_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?
                .ok_or_else(|| CoreError::NotFound {
                    what: trf("Konto {0}", &[&request.account_id]),
                })?;
            let cid = row.0.ok_or_else(|| CoreError::InvalidInput {
                message: tr("Dieses Konto hat keine Bankverbindung.").into(),
            })?;
            (cid, row.1, row.2)
        };
        let (key_str, login_name): (String, String) = {
            let db = self.db()?;
            db.query_row(
                "SELECT connector, login_name FROM bank_connections WHERE id = ?1",
                [connection_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?
        };
        let magic = crate::demo::is_magic_demo_login(&login_name);
        let demo = demo || magic;
        if !magic {
            crate::banking_gate::check_connector(&key_str)?;
        }
        let kind = ConnectorKind::from_str(&key_str).ok_or_else(|| CoreError::InvalidInput {
            message: format!("unknown connector {key_str}"),
        })?;
        if !matches!(
            kind,
            ConnectorKind::VolksbankKasselGoettingen
                | ConnectorKind::Direkt1822
                | ConnectorKind::FinTsGeneric
                | ConnectorKind::Bunq
        ) {
            return Err(CoreError::InvalidInput {
                message: tr("Überweisungen sind nur für FinTS- und bunq-Konten möglich.").into(),
            });
        }
        let exec_date = request
            .execution_date
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(str::to_string);
        if request.instant && exec_date.is_some() {
            return Err(CoreError::InvalidInput {
                message: tr("Echtzeitüberweisungen können nicht terminiert werden.").into(),
            });
        }
        if let Some(d) = &exec_date {
            let parsed = chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").map_err(|_| {
                CoreError::InvalidInput {
                    message: tr("Das Ausführungsdatum ist ungültig (JJJJ-MM-TT).").into(),
                }
            })?;
            if parsed <= chrono::Local::now().date_naive() {
                return Err(CoreError::InvalidInput {
                    message: tr("Das Ausführungsdatum muss in der Zukunft liegen.").into(),
                });
            }
        }
        if (request.instant || exec_date.is_some()) && matches!(kind, ConnectorKind::Bunq) {
            return Err(CoreError::InvalidInput {
                message: tr("Echtzeit- und Terminüberweisungen sind nur für FinTS-Konten möglich.")
                    .into(),
            });
        }
        if iban.is_empty() {
            return Err(CoreError::InvalidInput {
                message: tr("Das Konto hat keine IBAN.").into(),
            });
        }
        // Reuse the sync session set-up (settings, PIN, connector state).
        let sync = self.clone().begin_sync(connection_id, secret, demo)?;
        let ctx = sync.clone_context()?;
        let job = TransferJob {
            source_iban: iban,
            source_owner: owner,
            recipient_name: request.recipient_name.trim().to_string(),
            recipient_iban: sepa::normalize_iban(&request.recipient_iban),
            recipient_bic: request
                .recipient_bic
                .filter(|b| !b.trim().is_empty())
                .map(|b| sepa::normalize_iban(&b)),
            amount_cents: request.amount_cents,
            purpose: request.purpose.trim().to_string(),
            instant: request.instant,
            execution_date: exec_date,
            end_to_end_id: new_end_to_end_id(),
        };
        Ok(Arc::new(TransferSession::new(
            self.clone(),
            make_connector_for(kind, &key_str, demo),
            ctx,
            job,
            request.account_id,
        )))
    }

    /// Known recipients (name + IBAN) from past outgoing bookings, most used
    /// first. `query` filters by name/IBAN substring (empty = all).
    pub fn suggest_recipients(
        &self,
        query: String,
        limit: u32,
    ) -> Result<Vec<KnownRecipient>, CoreError> {
        let db = self.db()?;
        let q = query.trim().replace(' ', "").to_lowercase();
        let mut stmt = db.prepare(
            "SELECT counterparty, counterparty_iban, count(*) AS n FROM transactions
             WHERE counterparty_iban <> '' AND counterparty <> '' AND amount_cents < 0 AND deleted_ms IS NULL
             GROUP BY counterparty, counterparty_iban
             ORDER BY n DESC, MAX(booking_date) DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(KnownRecipient {
                name: r.get(0)?,
                iban: r.get(1)?,
                times_used: r.get(2)?,
            })
        })?;
        Ok(rows
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|r| {
                q.is_empty()
                    || r.name.replace(' ', "").to_lowercase().contains(&q)
                    || r.iban.replace(' ', "").to_lowercase().contains(&q)
            })
            .filter(|r| crate::connectors::fints::sepa::validate_iban(&r.iban))
            .take(limit as usize)
            .collect())
    }
}

impl Vault {
    /// Optimistic booking after a confirmed transfer. It disappears as soon
    /// as the bank's own booking for it arrives in a sync.
    pub(crate) fn record_pending_transfer(
        &self,
        account_id: i64,
        job: &TransferJob,
        booking_text: &str,
    ) -> CoreResult<()> {
        let db = self.db()?;
        db.execute(
            "INSERT INTO transactions(account_id, external_id, booking_date, value_date, amount_cents, currency,
                     counterparty, counterparty_iban, purpose, booking_text)
             VALUES (?1, ?2, ?3, ?3, ?4, 'EUR', ?5, ?6, ?7, ?8)",
            params![
                account_id,
                format!("pending-{}", job.end_to_end_id),
                today(),
                -job.amount_cents,
                job.recipient_name,
                job.recipient_iban,
                job.purpose,
                booking_text
            ],
        )?;
        Vault::apply_rules_inner(&db, true)?;
        Ok(())
    }
}

/// Links an unconnected stored account to `connection_id` when it matches the
/// remote one by account number / IBAN (ignoring spaces and leading zeros) or is
/// the single unlinked depot of the same bank. Sets `connection_id`, `external_ref`, `kind`.
fn adopt_unlinked(
    tx: &rusqlite::Connection,
    connection_id: i64,
    bank_name: &str,
    acc: &RemoteAccount,
) -> CoreResult<Option<i64>> {
    let norm = |x: &str| {
        x.chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .trim_start_matches('0')
            .to_uppercase()
    };
    let mut st = tx.prepare(
        // includes soft-deleted rows
        "SELECT id, IFNULL(iban,''), IFNULL(account_number,''), name, IFNULL(bank_name,'') FROM accounts WHERE connection_id IS NULL",
    )?;
    let rows: Vec<(i64, String, String, String, String)> = st
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?
        .collect::<Result<_, _>>()?;
    drop(st);
    let ours = [
        norm(&acc.iban),
        norm(&acc.account_number),
        norm(&acc.external_ref),
    ];
    let mut found = rows
        .iter()
        .find(|(_, iban, num, _, _)| {
            [norm(iban), norm(num)]
                .iter()
                .any(|f| !f.is_empty() && ours.contains(f))
        })
        .map(|r| r.0);
    if found.is_none()
        && (acc.kind == AccountKind::Brokerage || acc.name.to_lowercase().contains("depot"))
    {
        let same_bank = |b: &str| b.eq_ignore_ascii_case(bank_name);
        let depots: Vec<i64> = rows
            .iter()
            .filter(|(_, _, _, name, bank)| {
                name.to_lowercase().contains("depot") && same_bank(bank)
            })
            .map(|r| r.0)
            .collect();
        if let [only] = depots.as_slice() {
            found = Some(*only);
        }
    }
    if let Some(id) = found {
        // a deleted account that stays deleted keeps its sync key
        if crate::sync_merge::fetch_keeps_account_deleted(tx, connection_id, id)? {
            return Ok(found);
        }
        tx.execute(
            "UPDATE accounts SET connection_id = ?1, external_ref = ?2, kind = ?3 WHERE id = ?4",
            params![connection_id, acc.external_ref, acc.kind.as_str(), id],
        )?;
    }
    Ok(found)
}

fn is_amex_name(s: &str) -> bool {
    let l = s.to_lowercase();
    l.contains("amex") || l.contains("american express")
}

/// Open card balance (negative = owed) from `(date, amount_cents, text)` rows
/// sorted ascending: sum of everything after the latest settlement booking
/// (a positive KARTENABRECHNUNG / LASTSCHRIFT / AUSGLEICH / ZAHLUNG ERHALTEN
/// entry). Without a settlement, the sum of all bookings.
pub(crate) fn open_card_balance(rows: &[(String, i64, String)]) -> Option<i64> {
    if rows.is_empty() {
        return None;
    }
    let is_settlement = |(_, amt, text): &(String, i64, String)| {
        let t = text.to_uppercase();
        *amt > 0
            && [
                "KARTENABRECHNUNG",
                "LASTSCHRIFT",
                "AUSGLEICH",
                "ZAHLUNG ERHALTEN",
            ]
            .iter()
            .any(|k| t.contains(k))
    };
    let start = match rows.iter().rposition(is_settlement) {
        Some(i) => i + 1,
        // Billing cycles cross month boundaries and a refund is no settlement:
        // without one, everything fetched belongs to the open cycle.
        None => 0,
    };
    Some(rows[start..].iter().map(|r| r.1).sum())
}
