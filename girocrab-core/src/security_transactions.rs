//! Securities transactions of depots (#314): buy, sell, dividend, fee, tax.
//!
//! They live beside the cash bookings, never in `transactions`: no category,
//! no rules, no cash flow. The cash side of a trade is the booking on the
//! settlement account; [`link_cash_bookings`] connects both when exactly one
//! cash booking fits. Positions (`holdings`) stay the connector's inventory
//! and are never derived from these rows.

use std::collections::HashMap;

use rusqlite::{params, Connection};

use crate::error::CoreResult;
use crate::models::{SecurityDirection, SecurityTransaction};

fn now_ms() -> i64 {
    crate::user_groups::now_ms()
}

/// Inserts or refreshes a transaction of `account_id`, keyed by `external_id`.
pub(crate) fn upsert(
    conn: &Connection,
    account_id: i64,
    t: &SecurityTransaction,
) -> CoreResult<()> {
    conn.execute(
        "INSERT INTO security_transactions(account_id, external_id, booking_date, settlement_date,
                isin, wkn, name, direction, quantity, price_cents, currency, value_cents,
                fees_cents, taxes_cents, accrued_interest_cents, fx_rate, booking_text, changed_ms)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)
         ON CONFLICT(account_id, external_id) DO UPDATE SET
                booking_date = excluded.booking_date, settlement_date = excluded.settlement_date,
                isin = excluded.isin, wkn = excluded.wkn, name = excluded.name,
                direction = excluded.direction, quantity = excluded.quantity,
                price_cents = excluded.price_cents, currency = excluded.currency,
                value_cents = excluded.value_cents, fees_cents = excluded.fees_cents,
                taxes_cents = excluded.taxes_cents,
                accrued_interest_cents = excluded.accrued_interest_cents,
                fx_rate = excluded.fx_rate, booking_text = excluded.booking_text,
                changed_ms = excluded.changed_ms
         WHERE security_transactions.booking_date IS NOT excluded.booking_date
            OR security_transactions.settlement_date IS NOT excluded.settlement_date
            OR security_transactions.isin IS NOT excluded.isin
            OR security_transactions.direction IS NOT excluded.direction
            OR security_transactions.quantity IS NOT excluded.quantity
            OR security_transactions.price_cents IS NOT excluded.price_cents
            OR security_transactions.value_cents IS NOT excluded.value_cents
            OR security_transactions.fees_cents IS NOT excluded.fees_cents
            OR security_transactions.taxes_cents IS NOT excluded.taxes_cents
            OR security_transactions.accrued_interest_cents IS NOT excluded.accrued_interest_cents
            OR security_transactions.name IS NOT excluded.name
            OR security_transactions.deleted_ms IS NOT NULL",
        params![
            account_id,
            t.external_id,
            t.booking_date,
            t.settlement_date,
            t.isin,
            t.wkn,
            t.name,
            t.direction.as_str(),
            t.quantity,
            t.price_cents,
            if t.currency.is_empty() {
                "EUR"
            } else {
                &t.currency
            },
            t.value_cents,
            t.fees_cents,
            t.taxes_cents,
            t.accrued_interest_cents,
            t.fx_rate,
            t.booking_text,
            now_ms(),
        ],
    )?;
    // a transaction the bank lists again is live again
    conn.execute(
        "UPDATE security_transactions SET deleted_ms = NULL
         WHERE account_id = ?1 AND external_id = ?2 AND deleted_ms IS NOT NULL",
        params![account_id, t.external_id],
    )?;
    Ok(())
}

pub(crate) fn list(conn: &Connection, account_id: i64) -> CoreResult<Vec<SecurityTransaction>> {
    let mut q = conn.prepare(
        "SELECT id, account_id, external_id, booking_date, settlement_date, isin, wkn, name,
                direction, quantity, price_cents, currency, value_cents, fees_cents, taxes_cents,
                accrued_interest_cents, fx_rate, booking_text, cash_transaction_id
         FROM security_transactions s
         WHERE account_id = ?1 AND deleted_ms IS NULL
           AND EXISTS (SELECT 1 FROM accounts a WHERE a.id = s.account_id AND a.deleted_ms IS NULL)
         ORDER BY booking_date DESC, id DESC",
    )?;
    let rows = q.query_map([account_id], |r| {
        Ok(SecurityTransaction {
            id: r.get(0)?,
            account_id: r.get(1)?,
            external_id: r.get(2)?,
            booking_date: r.get(3)?,
            settlement_date: r.get(4)?,
            isin: r.get(5)?,
            wkn: r.get(6)?,
            name: r.get(7)?,
            direction: SecurityDirection::from_db(&r.get::<_, String>(8)?),
            quantity: r.get(9)?,
            price_cents: r.get(10)?,
            currency: r.get(11)?,
            value_cents: r.get(12)?,
            fees_cents: r.get(13)?,
            taxes_cents: r.get(14)?,
            accrued_interest_cents: r.get(15)?,
            fx_rate: r.get(16)?,
            booking_text: r.get(17)?,
            cash_transaction_id: r.get(18)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// The cash amount (in the cash booking's sign) a transaction settles to, or
/// `None` for rows without a cash side of their own (fee/tax/other lines are
/// matched only when they carry their own value).
fn expected_cash(
    d: SecurityDirection,
    value: i64,
    fees: i64,
    taxes: i64,
    accrued: i64,
) -> Option<i64> {
    match d {
        SecurityDirection::Buy => Some(-(value + fees + taxes + accrued)),
        SecurityDirection::Sell => Some(value - fees - taxes + accrued),
        SecurityDirection::Dividend => Some(value - taxes),
        SecurityDirection::Fee | SecurityDirection::Tax => Some(-(value.abs())),
        SecurityDirection::Other => None,
    }
}

/// Links every unlinked transaction to its cash booking: a live booking on
/// another account of the same login (any other non-depot account when the
/// depot has no login) with the same currency, a booking or value date equal
/// to the transaction's booking or settlement date and exactly the expected
/// amount. Only a unique candidate that no other transaction has claimed is
/// linked, a tie stays unlinked. Returns the number of new links.
pub(crate) fn link_cash_bookings(conn: &Connection) -> CoreResult<u32> {
    struct Row {
        id: i64,
        account_id: i64,
        booking: String,
        settle: String,
        currency: String,
        expected: i64,
    }
    let rows: Vec<Row> = {
        let mut q = conn.prepare(
            "SELECT id, account_id, booking_date, settlement_date, currency, direction,
                    value_cents, fees_cents, taxes_cents, accrued_interest_cents
             FROM security_transactions
             WHERE cash_transaction_id IS NULL AND deleted_ms IS NULL",
        )?;
        let it = q.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, i64>(7)?,
                r.get::<_, i64>(8)?,
                r.get::<_, i64>(9)?,
            ))
        })?;
        let mut v = Vec::new();
        for x in it {
            let (id, account_id, booking, settle, currency, dir, val, fee, tax, acc) = x?;
            if let Some(expected) =
                expected_cash(SecurityDirection::from_db(&dir), val, fee, tax, acc)
            {
                if expected != 0 {
                    v.push(Row {
                        id,
                        account_id,
                        booking,
                        settle,
                        currency,
                        expected,
                    });
                }
            }
        }
        v
    };
    if rows.is_empty() {
        return Ok(0);
    }
    let mut cand = conn.prepare(
        "SELECT t.id FROM transactions t
           JOIN accounts a ON a.id = t.account_id
           JOIN accounts d ON d.id = ?1
          WHERE t.deleted_ms IS NULL AND a.deleted_ms IS NULL
            AND a.kind != 'brokerage' AND a.id != d.id
            AND (d.connection_id IS NULL OR a.connection_id = d.connection_id)
            AND t.amount_cents = ?2 AND t.currency = ?3
            AND (t.booking_date IN (?4, ?5) OR (t.value_date != '' AND t.value_date IN (?4, ?5)))
            AND NOT EXISTS (SELECT 1 FROM security_transactions s WHERE s.cash_transaction_id = t.id)",
    )?;
    let mut claimed: HashMap<i64, i64> = HashMap::new();
    let mut links: Vec<(i64, i64)> = Vec::new();
    for r in &rows {
        let settle = if r.settle.is_empty() {
            &r.booking
        } else {
            &r.settle
        };
        let ids: Vec<i64> = cand
            .query_map(
                params![r.account_id, r.expected, r.currency, r.booking, settle],
                |x| x.get(0),
            )?
            .collect::<Result<_, _>>()?;
        if let [only] = ids.as_slice() {
            *claimed.entry(*only).or_insert(0) += 1;
            links.push((r.id, *only));
        }
    }
    let mut n = 0;
    for (sec, cash) in links {
        // two transactions fit the same booking: ambiguous, leave both open
        if claimed.get(&cash).copied().unwrap_or(0) == 1 {
            n += conn.execute(
                "UPDATE security_transactions SET cash_transaction_id = ?1 WHERE id = ?2",
                params![cash, sec],
            )? as u32;
        }
    }
    Ok(n)
}

/// Links a transaction to the cash booking the bank named: a booking with
/// `cash_external_id` on an account of the same login as the depot.
pub(crate) fn link_by_external_id(
    conn: &Connection,
    account_id: i64,
    external_id: &str,
    cash_external_id: &str,
) -> CoreResult<()> {
    if cash_external_id.is_empty() {
        return Ok(());
    }
    conn.execute(
        "UPDATE security_transactions SET cash_transaction_id = (
             SELECT t.id FROM transactions t
               JOIN accounts a ON a.id = t.account_id
               JOIN accounts d ON d.id = ?1
              WHERE t.external_id = ?3 AND t.deleted_ms IS NULL AND a.id != d.id
                AND a.connection_id IS d.connection_id
              LIMIT 1)
         WHERE account_id = ?1 AND external_id = ?2 AND cash_transaction_id IS NULL",
        params![account_id, external_id, cash_external_id],
    )?;
    Ok(())
}

/// Vault merge: adds the other vault's transactions of mapped accounts
/// (`acc_map`: other account id -> own account id), keyed by
/// `(account, external_id)`; the newer edit wins, a deletion stays deleted.
/// Cash links are local ids and are recomputed instead of copied.
pub(crate) fn merge_attached(conn: &Connection, acc_map: &HashMap<i64, i64>) -> CoreResult<()> {
    let has: i64 = conn.query_row(
        "SELECT count(*) FROM other.sqlite_master WHERE type = 'table' AND name = 'security_transactions'",
        [],
        |r| r.get(0),
    )?;
    if has == 0 {
        return Ok(());
    }
    struct Other {
        account: i64,
        t: SecurityTransaction,
        changed: i64,
        deleted: Option<i64>,
    }
    let rows: Vec<Other> = {
        let mut q = conn.prepare(
            "SELECT account_id, external_id, booking_date, settlement_date, isin, wkn, name,
                    direction, quantity, price_cents, currency, value_cents, fees_cents,
                    taxes_cents, accrued_interest_cents, fx_rate, booking_text, changed_ms, deleted_ms
             FROM other.security_transactions",
        )?;
        let it = q.query_map([], |r| {
            Ok(Other {
                account: r.get(0)?,
                t: SecurityTransaction {
                    external_id: r.get(1)?,
                    booking_date: r.get(2)?,
                    settlement_date: r.get(3)?,
                    isin: r.get(4)?,
                    wkn: r.get(5)?,
                    name: r.get(6)?,
                    direction: SecurityDirection::from_db(&r.get::<_, String>(7)?),
                    quantity: r.get(8)?,
                    price_cents: r.get(9)?,
                    currency: r.get(10)?,
                    value_cents: r.get(11)?,
                    fees_cents: r.get(12)?,
                    taxes_cents: r.get(13)?,
                    accrued_interest_cents: r.get(14)?,
                    fx_rate: r.get(15)?,
                    booking_text: r.get(16)?,
                    ..Default::default()
                },
                changed: r.get(17)?,
                deleted: r.get(18)?,
            })
        })?;
        it.collect::<Result<_, _>>()?
    };
    for o in rows {
        let Some(acc) = acc_map.get(&o.account) else {
            continue;
        };
        let own: Option<(i64, i64, Option<i64>)> = conn
            .query_row(
                "SELECT id, changed_ms, deleted_ms FROM security_transactions
                 WHERE account_id = ?1 AND external_id = ?2",
                params![acc, o.t.external_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok();
        match own {
            None => {
                upsert(conn, *acc, &o.t)?;
                conn.execute(
                    "UPDATE security_transactions SET changed_ms = ?3, deleted_ms = ?4
                     WHERE account_id = ?1 AND external_id = ?2",
                    params![acc, o.t.external_id, o.changed, o.deleted],
                )?;
            }
            Some((id, changed, _)) if o.changed > changed => {
                upsert(conn, *acc, &o.t)?;
                conn.execute(
                    "UPDATE security_transactions SET changed_ms = ?2, deleted_ms = ?3 WHERE id = ?1",
                    params![id, o.changed, o.deleted],
                )?;
            }
            _ => {}
        }
    }
    link_cash_bookings(conn)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectors::{
        store_fetch, FetchResult, RemoteAccount, RemoteHolding, RemoteSecurityTransaction,
        RemoteTransaction,
    };
    use crate::models::{AccountKind, BookingFilter, BookingScope, ConnectorKind, Holding};
    use crate::vault::Vault;

    fn open(dir: &crate::test_support::TempDir, name: &str) -> std::sync::Arc<Vault> {
        Vault::open(dir.join(name).to_string_lossy().to_string(), vec![5u8; 32]).unwrap()
    }

    fn acc(external_ref: &str, kind: AccountKind) -> RemoteAccount {
        RemoteAccount {
            external_ref: external_ref.into(),
            name: external_ref.into(),
            owner: String::new(),
            iban: String::new(),
            account_number: external_ref.into(),
            currency: "EUR".into(),
            kind,
            balance_cents: 100_000,
            balance_date: "2026-11-10".into(),
        }
    }

    fn cash_tx(id: &str, date: &str, cents: i64) -> RemoteTransaction {
        RemoteTransaction {
            account_ref: "cash".into(),
            external_id: id.into(),
            booking_date: date.into(),
            value_date: date.into(),
            amount_cents: cents,
            currency: "EUR".into(),
            ..Default::default()
        }
    }

    fn sec(id: &str, dir: SecurityDirection, value: i64, fees: i64) -> RemoteSecurityTransaction {
        RemoteSecurityTransaction {
            account_ref: "depot".into(),
            external_id: id.into(),
            cash_external_id: String::new(),
            transaction: SecurityTransaction {
                booking_date: "2026-11-05".into(),
                settlement_date: "2026-11-07".into(),
                isin: "IE00BK5BQT80".into(),
                name: "ETF".into(),
                direction: dir,
                quantity: 2.0,
                price_cents: value / 2,
                currency: "EUR".into(),
                value_cents: value,
                fees_cents: fees,
                ..Default::default()
            },
        }
    }

    fn fetch(sec_txs: Vec<RemoteSecurityTransaction>, cash: Vec<RemoteTransaction>) -> FetchResult {
        FetchResult {
            accounts: vec![
                acc("cash", AccountKind::Checking),
                acc("depot", AccountKind::Brokerage),
            ],
            transactions: cash,
            security_transactions: sec_txs,
            ..Default::default()
        }
    }

    fn depot_id(v: &Vault) -> i64 {
        v.list_accounts()
            .unwrap()
            .into_iter()
            .find(|a| a.kind == AccountKind::Brokerage)
            .unwrap()
            .id
    }

    #[test]
    fn stored_once_linked_to_the_cash_booking_and_not_counted_as_cash() {
        let dir = crate::test_support::TempDir::new("sectx-store");
        let v = open(&dir, "v.db");
        let c = v
            .add_bank_connection(ConnectorKind::Comdirect, String::new(), "l".into())
            .unwrap();
        // buy 2 × 50.00 + 1.50 fees settles as -101.50 on the cash account
        let data = || {
            fetch(
                vec![
                    sec("b1", SecurityDirection::Buy, 10_000, 150),
                    sec("d1", SecurityDirection::Dividend, 500, 0),
                ],
                vec![
                    cash_tx("c1", "2026-11-07", -10_150),
                    cash_tx("c2", "2026-11-09", 999),
                ],
            )
        };
        store_fetch(&v, ConnectorKind::Comdirect, c, data()).unwrap();
        store_fetch(&v, ConnectorKind::Comdirect, c, data()).unwrap();
        let list = v.list_security_transactions(depot_id(&v)).unwrap();
        assert_eq!(list.len(), 2, "no duplicates after a second sync");
        let buy = list.iter().find(|t| t.external_id == "b1").unwrap();
        assert!(
            buy.cash_transaction_id.is_some(),
            "buy matches the -101.50 booking"
        );
        let dividend = list.iter().find(|t| t.external_id == "d1").unwrap();
        assert!(
            dividend.cash_transaction_id.is_none(),
            "no cash booking of 5.00"
        );
        // the cash account holds only its own two bookings
        let bookings = v
            .list_transactions(BookingFilter {
                scope: BookingScope::All,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(bookings.transactions.len(), 2);
    }

    #[test]
    fn ambiguous_cash_bookings_stay_unlinked() {
        let dir = crate::test_support::TempDir::new("sectx-ambiguous");
        let v = open(&dir, "v.db");
        let c = v
            .add_bank_connection(ConnectorKind::Comdirect, String::new(), "l".into())
            .unwrap();
        let data = fetch(
            vec![sec("b1", SecurityDirection::Buy, 10_000, 0)],
            vec![
                cash_tx("c1", "2026-11-05", -10_000),
                cash_tx("c2", "2026-11-07", -10_000),
            ],
        );
        store_fetch(&v, ConnectorKind::Comdirect, c, data).unwrap();
        let list = v.list_security_transactions(depot_id(&v)).unwrap();
        assert!(list[0].cash_transaction_id.is_none());
    }

    #[test]
    fn two_trades_do_not_claim_the_same_cash_booking() {
        let dir = crate::test_support::TempDir::new("sectx-claim");
        let v = open(&dir, "v.db");
        let c = v
            .add_bank_connection(ConnectorKind::Comdirect, String::new(), "l".into())
            .unwrap();
        let data = fetch(
            vec![
                sec("b1", SecurityDirection::Buy, 10_000, 0),
                sec("b2", SecurityDirection::Buy, 10_000, 0),
            ],
            vec![cash_tx("c1", "2026-11-07", -10_000)],
        );
        store_fetch(&v, ConnectorKind::Comdirect, c, data).unwrap();
        let list = v.list_security_transactions(depot_id(&v)).unwrap();
        assert!(list.iter().all(|t| t.cash_transaction_id.is_none()));
    }

    #[test]
    fn named_cash_booking_links_across_currencies() {
        let dir = crate::test_support::TempDir::new("sectx-named");
        let v = open(&dir, "v.db");
        let c = v
            .add_bank_connection(ConnectorKind::Ibkr, String::new(), "l".into())
            .unwrap();
        let mut s = sec("b1", SecurityDirection::Buy, 40_000, 100);
        s.transaction.currency = "USD".into();
        s.cash_external_id = "c1".into();
        // base-currency amount differs from the USD value: only the id can match
        store_fetch(
            &v,
            ConnectorKind::Ibkr,
            c,
            fetch(vec![s], vec![cash_tx("c1", "2026-11-05", -36_090)]),
        )
        .unwrap();
        let list = v.list_security_transactions(depot_id(&v)).unwrap();
        assert!(list[0].cash_transaction_id.is_some());
    }

    #[test]
    fn holdings_keep_the_extra_position_fields() {
        let dir = crate::test_support::TempDir::new("sectx-holding");
        let v = open(&dir, "v.db");
        let c = v
            .add_bank_connection(ConnectorKind::Comdirect, String::new(), "l".into())
            .unwrap();
        let mut data = fetch(vec![], vec![]);
        data.holdings = vec![RemoteHolding {
            classification: Default::default(),
            direct_crypto: false,

            account_ref: "depot".into(),
            holding: Holding {
                isin: "DE0001234567".into(),
                quantity: 2.0,
                value_cents: 100,
                prev_day_price_cents: Some(48),
                prev_day_value_cents: Some(96),
                profit_loss_purchase_cents: Some(-5),
                profit_loss_purchase_rel: Some(-4.76),
                profit_loss_prev_day_cents: Some(4),
                profit_loss_prev_day_rel: Some(4.17),
                custody_type: "GS".into(),
                ..Default::default()
            },
        }];
        store_fetch(&v, ConnectorKind::Comdirect, c, data).unwrap();
        let h = v.list_holdings(depot_id(&v)).unwrap().pop().unwrap();
        assert_eq!(h.prev_day_value_cents, Some(96));
        assert_eq!(h.profit_loss_purchase_rel, Some(-4.76));
        assert_eq!(h.custody_type, "GS");
        assert_eq!(h.profit_loss_prev_day_cents, Some(4));
    }

    fn holding_fetch() -> FetchResult {
        let mut data = fetch(vec![], vec![]);
        data.holdings = vec![RemoteHolding {
            classification: Default::default(),
            direct_crypto: false,
            account_ref: "depot".into(),
            holding: Holding {
                isin: "DE0001234567".into(),
                quantity: 2.0,
                value_cents: 100,
                ..Default::default()
            },
        }];
        data
    }

    fn value_points(v: &Vault) -> Vec<(String, i64)> {
        v.db()
            .unwrap()
            .prepare("SELECT value_date, value_cents FROM depot_values ORDER BY value_date")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn every_fetched_depot_leaves_one_value_point_per_day_and_cash_accounts_none() {
        let dir = crate::test_support::TempDir::new("sectx-values");
        let v = open(&dir, "v.db");
        let c = v
            .add_bank_connection(ConnectorKind::Comdirect, String::new(), "l".into())
            .unwrap();
        store_fetch(&v, ConnectorKind::Comdirect, c, holding_fetch()).unwrap();
        store_fetch(&v, ConnectorKind::Comdirect, c, holding_fetch()).unwrap();
        // two fetches on one day: one point, and only for the depot (not the checking account)
        assert_eq!(value_points(&v).len(), 1);
        let depot = depot_id(&v);
        let n: i64 = v
            .db()
            .unwrap()
            .query_row(
                "SELECT count(DISTINCT account_id) FROM depot_values",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        let only: i64 = v
            .db()
            .unwrap()
            .query_row("SELECT account_id FROM depot_values", [], |r| r.get(0))
            .unwrap();
        assert_eq!(only, depot);
    }

    #[test]
    fn a_vault_merge_brings_the_value_points_along() {
        let dir = crate::test_support::TempDir::new("sectx-values-merge");
        let key = vec![5u8; 32];
        let a = open(&dir, "a.db");
        let b = open(&dir, "b.db");
        for v in [&a, &b] {
            let c = v
                .add_bank_connection(ConnectorKind::Comdirect, String::new(), "l".into())
                .unwrap();
            store_fetch(v, ConnectorKind::Comdirect, c, holding_fetch()).unwrap();
        }
        b.db()
            .unwrap()
            .execute("UPDATE depot_values SET value_date = '2020-01-01'", [])
            .unwrap();
        a.merge_other_vault(b.file_path(), key).unwrap();
        let dates: Vec<String> = value_points(&a).into_iter().map(|p| p.0).collect();
        assert_eq!(dates.len(), 2);
        assert_eq!(dates[0], "2020-01-01");
    }

    #[test]
    fn merge_adds_missing_rows_once_and_keeps_deletions() {
        let dir = crate::test_support::TempDir::new("sectx-merge");
        let key = vec![5u8; 32];
        let a = open(&dir, "a.db");
        let b = open(&dir, "b.db");
        for (v, rows) in [
            (&a, vec![sec("b1", SecurityDirection::Buy, 10_000, 0)]),
            (
                &b,
                vec![
                    sec("b1", SecurityDirection::Buy, 10_000, 0),
                    sec("s1", SecurityDirection::Sell, 7_000, 0),
                ],
            ),
        ] {
            let c = v
                .add_bank_connection(ConnectorKind::Comdirect, String::new(), "l".into())
                .unwrap();
            store_fetch(v, ConnectorKind::Comdirect, c, fetch(rows, vec![])).unwrap();
        }
        a.merge_other_vault(b.file_path(), key.clone()).unwrap();
        let ids = |v: &Vault| -> Vec<String> {
            let mut x: Vec<String> = v
                .list_security_transactions(depot_id(v))
                .unwrap()
                .into_iter()
                .map(|t| t.external_id)
                .collect();
            x.sort();
            x
        };
        assert_eq!(ids(&a), ["b1", "s1"]);
        a.merge_other_vault(b.file_path(), key).unwrap();
        assert_eq!(ids(&a), ["b1", "s1"], "merging again adds nothing");
    }

    #[test]
    fn expected_cash_signs() {
        assert_eq!(
            expected_cash(SecurityDirection::Buy, 100, 5, 0, 2),
            Some(-107)
        );
        assert_eq!(
            expected_cash(SecurityDirection::Sell, 100, 5, 10, 0),
            Some(85)
        );
        assert_eq!(
            expected_cash(SecurityDirection::Dividend, 100, 0, 26, 0),
            Some(74)
        );
        assert_eq!(expected_cash(SecurityDirection::Other, 100, 0, 0, 0), None);
    }
}
