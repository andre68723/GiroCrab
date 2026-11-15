//! Valuation history of securities depots: one value per depot and day.
//!
//! A depot has no cash bookings to walk back over, and the bank reports only today's valuation.
//! So each fetch leaves a point: the depot value the bank reported, stamped with its valuation
//! date. The history therefore starts with the first fetch and never claims a past it did not
//! see: before its first point a depot's value is **unknown** (it is not 0 and not the first
//! value). A confirmed value of 0 (a depot that was sold off) is a point like any other. Between
//! two points the last value carries on (no interpolation). Values are the reported depot value,
//! so a change in them may also come from purchases, sales and transfers: it is no return.
//! A sum over several depots says per day how many of them were covered.

use std::collections::HashMap;

use chrono::NaiveDate;
use rusqlite::{params, Connection};

use crate::error::CoreResult;

/// Step 26. A point of a day comes from a source (`fetch`: the bank's own report at a fetch,
/// `import`: an imported history, `prices`: computed from prices and quantities) and has a
/// quality (`observed`, `reconstructed`, `estimated`). Several sources of a day live side by
/// side; [`series`] shows the best one. `changed_ms` decides a vault merge: the newer value of
/// a (day, source) wins. No soft delete yet: the rows disappear with their account
/// (`ON DELETE CASCADE`).
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS depot_values (
    account_id  INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    value_date  TEXT    NOT NULL,            -- YYYY-MM-DD, the valuation day
    source      TEXT    NOT NULL DEFAULT 'fetch',    -- fetch | import | prices
    quality     TEXT    NOT NULL DEFAULT 'observed', -- observed | reconstructed | estimated
    value_cents INTEGER NOT NULL,
    changed_ms  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (account_id, value_date, source)
) WITHOUT ROWID;
";

pub(crate) const SOURCE_FETCH: &str = "fetch";
pub(crate) const QUALITY_OBSERVED: &str = "observed";

fn is_iso_date(s: &str) -> bool {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
}

/// Inserts or replaces the value of one day of a fetch (the later fetch of a day wins).
pub(crate) fn upsert(
    conn: &Connection,
    account_id: i64,
    date: &str,
    value_cents: i64,
    changed_ms: i64,
) -> CoreResult<()> {
    upsert_from(
        conn,
        account_id,
        date,
        value_cents,
        changed_ms,
        SOURCE_FETCH,
        QUALITY_OBSERVED,
    )
}

/// Inserts or replaces the value of one (day, source).
pub(crate) fn upsert_from(
    conn: &Connection,
    account_id: i64,
    date: &str,
    value_cents: i64,
    changed_ms: i64,
    source: &str,
    quality: &str,
) -> CoreResult<()> {
    conn.execute(
        "INSERT INTO depot_values(account_id, value_date, source, quality, value_cents, changed_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(account_id, value_date, source) DO UPDATE SET
             quality = excluded.quality, value_cents = excluded.value_cents,
             changed_ms = excluded.changed_ms
         WHERE excluded.changed_ms >= depot_values.changed_ms",
        params![account_id, date, source, quality, value_cents, changed_ms],
    )?;
    Ok(())
}

/// Records the depot value a fetch just stored: the account's balance on its valuation date
/// (today when the bank named none). Only securities depots (a Sachwerte account has its own
/// value series). A balance of 0 is a point only when the fetch confirmed it
/// (`confirmed_zero`: the bank delivered the inventory, possibly an empty one); otherwise 0 may
/// just mean "not reported" and leaves no point.
pub(crate) fn record(conn: &Connection, account_id: i64, confirmed_zero: bool) -> CoreResult<()> {
    let row: Option<(String, i64, String)> = conn
        .query_row(
            "SELECT kind, balance_cents, balance_date FROM accounts WHERE id = ?1 AND deleted_ms IS NULL",
            [account_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok();
    let Some((kind, cents, date)) = row else {
        return Ok(());
    };
    if kind != "brokerage" || crate::assets::is_tangible(conn, account_id)? {
        return Ok(());
    }
    if cents == 0 && !confirmed_zero {
        return Ok(());
    }
    let day = if is_iso_date(date.trim()) {
        date.trim().to_string()
    } else {
        chrono::Local::now()
            .date_naive()
            .format("%Y-%m-%d")
            .to_string()
    };
    upsert(conn, account_id, &day, cents, crate::user_groups::now_ms())
}

/// Per-day values of the depots among `ids` that have a history, summed, for `start..=end`.
/// `None` when none has a point. A depot counts from its first point on; before that it adds
/// nothing and is not counted as covered.
pub(crate) struct Series {
    /// Sum of the known depot values per day (depots not yet covered add nothing).
    pub values: Vec<i64>,
    /// Per day: how many depots had a value (a confirmed 0 counts).
    pub covered: Vec<u32>,
    /// Securities depots among `ids`, with or without a history. The chart reads it next to
    /// `covered` ("2 of 3 depots") in `Vault::depot_value_history`.
    pub total_depots: u32,
    /// The depots with at least one point: the caller leaves them out of its own walk back over
    /// bookings.
    pub accounts: Vec<i64>,
}

pub(crate) fn series(
    conn: &Connection,
    ids: &[i64],
    start: NaiveDate,
    end: NaiveDate,
) -> CoreResult<Option<Series>> {
    if ids.is_empty() || end < start {
        return Ok(None);
    }
    let days = (end - start).num_days() as usize + 1;
    let mut total = vec![0_i64; days];
    let mut covered_days = vec![0_u32; days];
    let mut with_points = Vec::new();
    let mut total_depots = 0_u32;
    let mut q = conn.prepare(
        "SELECT value_date, value_cents FROM depot_values
         WHERE account_id = ?1 AND value_date <= ?2
         ORDER BY value_date,
                  CASE quality WHEN 'observed' THEN 0 WHEN 'reconstructed' THEN 1 ELSE 2 END,
                  CASE source WHEN 'fetch' THEN 0 WHEN 'import' THEN 1 ELSE 2 END",
    )?;
    let last = end.format("%Y-%m-%d").to_string();
    for &id in ids {
        let kind: Option<String> = conn
            .query_row("SELECT kind FROM accounts WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .ok();
        if kind.as_deref() == Some("brokerage") && !crate::assets::is_tangible(conn, id)? {
            total_depots += 1;
        }
        let mut points: Vec<(String, i64)> = q
            .query_map(params![id, last], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        // the best source of a day comes first (see the ORDER BY)
        points.dedup_by(|later, first| later.0 == first.0);
        if points.is_empty() {
            // A depot that was never recorded (a vault from before the history) is known from its
            // reported balance on: one value on its valuation day, unknown before it. Its balance
            // is never carried backwards over the days it was not seen.
            match reported_point(conn, id, end)? {
                Some(point) => points.push(point),
                None => continue,
            }
        }
        with_points.push(id);
        let mut next = 0;
        let mut current: Option<i64> = None;
        for i in 0..days {
            let day = (start + chrono::Duration::days(i as i64))
                .format("%Y-%m-%d")
                .to_string();
            while next < points.len() && points[next].0 <= day {
                current = Some(points[next].1);
                next += 1;
            }
            if let Some(v) = current {
                total[i] += v;
                covered_days[i] += 1;
            }
        }
    }
    Ok((!with_points.is_empty()).then_some(Series {
        values: total,
        covered: covered_days,
        total_depots,
        accounts: with_points,
    }))
}

/// The balance an account reports, as one point (valuation day or `end`), for a securities depot
/// that has no recorded value and no bookings. A balance of 0 is not a reported value.
fn reported_point(conn: &Connection, id: i64, end: NaiveDate) -> CoreResult<Option<(String, i64)>> {
    let row: Option<(String, i64, String)> = conn
        .query_row(
            "SELECT kind, balance_cents, balance_date FROM accounts WHERE id = ?1 AND deleted_ms IS NULL",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok();
    let Some((kind, cents, date)) = row else {
        return Ok(None);
    };
    if kind != "brokerage" || cents == 0 || crate::assets::is_tangible(conn, id)? {
        return Ok(None);
    }
    // An account with bookings keeps the walk back over them: that history is real. Only a depot
    // without any would show nothing but today's value carried backwards.
    let bookings: i64 = conn.query_row(
        "SELECT count(*) FROM transactions WHERE account_id = ?1 AND deleted_ms IS NULL",
        [id],
        |r| r.get(0),
    )?;
    if bookings > 0 {
        return Ok(None);
    }
    let day = if is_iso_date(date.trim()) {
        date.trim().to_string()
    } else {
        end.format("%Y-%m-%d").to_string()
    };
    Ok(Some((day, cents)))
}

/// Vault merge: adds the other vault's values of mapped accounts; the newer change of a day wins.
pub(crate) fn merge_attached(conn: &Connection, acc_map: &HashMap<i64, i64>) -> CoreResult<()> {
    let has: i64 = conn.query_row(
        "SELECT count(*) FROM other.sqlite_master WHERE type = 'table' AND name = 'depot_values'",
        [],
        |r| r.get(0),
    )?;
    if has == 0 {
        return Ok(());
    }
    type Row = (i64, String, String, String, i64, i64);
    let rows: Vec<Row> = {
        let mut q = conn.prepare(
            "SELECT account_id, value_date, source, quality, value_cents, changed_ms
             FROM other.depot_values",
        )?;
        let it = q.query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })?;
        it.collect::<Result<_, _>>()?
    };
    for (other, date, source, quality, cents, changed) in rows {
        if let Some(own) = acc_map.get(&other) {
            upsert_from(conn, *own, &date, cents, changed, &source, &quality)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AccountKind;
    use crate::vault::Vault;

    fn open(name: &str) -> (crate::test_support::TempDir, std::sync::Arc<Vault>) {
        let dir = crate::test_support::TempDir::new(name);
        let v = Vault::open(
            dir.join("v.db").to_string_lossy().to_string(),
            vec![7u8; 32],
        )
        .unwrap();
        (dir, v)
    }

    fn depot(v: &Vault, balance: i64, date: &str) -> i64 {
        let db = v.db().unwrap();
        db.execute(
            "INSERT INTO accounts(group_id, name, kind, balance_cents, balance_date, currency)
             VALUES ((SELECT COALESCE(MIN(id), 1) FROM account_groups), 'D', ?1, ?2, ?3, 'EUR')",
            params![AccountKind::Brokerage.as_str(), balance, date],
        )
        .or_else(|_| {
            db.execute(
                "INSERT INTO account_groups(name, sf_symbol, sort_order) VALUES ('G', 'x', 0)",
                [],
            )?;
            db.execute(
                "INSERT INTO accounts(group_id, name, kind, balance_cents, balance_date, currency)
                 VALUES (last_insert_rowid(), 'D', ?1, ?2, ?3, 'EUR')",
                params![AccountKind::Brokerage.as_str(), balance, date],
            )
        })
        .unwrap();
        db.last_insert_rowid()
    }

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn the_later_fetch_of_a_day_replaces_the_earlier_one() {
        let (_d, v) = open("depot-values-record");
        let id = depot(&v, 100_000, "2026-03-01");
        let db = v.db().unwrap();
        record(&db, id, false).unwrap();
        db.execute(
            "UPDATE accounts SET balance_cents = 120_000 WHERE id = ?1",
            [id],
        )
        .unwrap();
        record(&db, id, false).unwrap();
        let rows: Vec<(String, i64)> = db
            .prepare("SELECT value_date, value_cents FROM depot_values WHERE account_id = ?1")
            .unwrap()
            .query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(rows, vec![("2026-03-01".to_string(), 120_000)]);
    }

    #[test]
    fn an_unconfirmed_zero_and_other_accounts_leave_no_point() {
        let (_d, v) = open("depot-values-skip");
        let empty = depot(&v, 0, "2026-03-01");
        let db = v.db().unwrap();
        record(&db, empty, false).unwrap();
        db.execute(
            "UPDATE accounts SET kind = 'checking', balance_cents = 5 WHERE id = ?1",
            [empty],
        )
        .unwrap();
        record(&db, empty, true).unwrap();
        let n: i64 = db
            .query_row("SELECT count(*) FROM depot_values", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn a_confirmed_zero_is_a_point_and_ends_the_last_positive_value() {
        let (_d, v) = open("depot-values-zero");
        let id = depot(&v, 50_000, "2026-03-02");
        let db = v.db().unwrap();
        record(&db, id, true).unwrap();
        db.execute(
            "UPDATE accounts SET balance_cents = 0, balance_date = '2026-03-05' WHERE id = ?1",
            [id],
        )
        .unwrap();
        record(&db, id, true).unwrap();
        let s = series(&db, &[id], day("2026-03-02"), day("2026-03-07"))
            .unwrap()
            .unwrap();
        assert_eq!(s.values, vec![50_000, 50_000, 50_000, 0, 0, 0]);
        assert_eq!(s.covered, vec![1; 6], "a confirmed zero counts as covered");
    }

    #[test]
    fn values_carry_forward_and_stay_unknown_before_the_first_point() {
        let (_d, v) = open("depot-values-series");
        let id = depot(&v, 130_000, "2026-03-10");
        let db = v.db().unwrap();
        upsert(&db, id, "2026-03-03", 100_000, 1).unwrap();
        upsert(&db, id, "2026-03-07", 110_000, 2).unwrap();
        upsert(&db, id, "2026-03-10", 130_000, 3).unwrap();
        let s = series(&db, &[id], day("2026-03-01"), day("2026-03-10"))
            .unwrap()
            .unwrap();
        assert_eq!(
            s.values,
            vec![0, 0, 100_000, 100_000, 100_000, 100_000, 110_000, 110_000, 110_000, 130_000]
        );
        assert_eq!(s.covered, vec![0, 0, 1, 1, 1, 1, 1, 1, 1, 1]);
        assert_eq!(s.total_depots, 1);
        assert_eq!(s.accounts, vec![id]);
        assert!(
            series(&db, &[id + 99], day("2026-03-01"), day("2026-03-10"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_second_depot_counts_from_its_first_point_and_the_coverage_says_so() {
        let (_d, v) = open("depot-values-coverage");
        let a = depot(&v, 100, "2026-03-04");
        let b = depot(&v, 40, "2026-03-04");
        let none = depot(&v, 0, "2026-03-04");
        let db = v.db().unwrap();
        upsert(&db, a, "2026-03-01", 100, 1).unwrap();
        upsert(&db, b, "2026-03-03", 40, 1).unwrap();
        let s = series(&db, &[a, b, none], day("2026-03-01"), day("2026-03-04"))
            .unwrap()
            .unwrap();
        assert_eq!(s.values, vec![100, 100, 140, 140]);
        assert_eq!(s.covered, vec![1, 1, 2, 2]);
        assert_eq!(
            s.total_depots, 3,
            "a depot without any point is still a depot"
        );
        assert_eq!(s.accounts, vec![a, b]);
    }

    #[test]
    fn the_history_uses_the_depot_values_even_when_the_balance_differs() {
        let (_d, v) = open("depot-values-history");
        let today = chrono::Local::now().date_naive();
        let f = |d: NaiveDate| d.format("%Y-%m-%d").to_string();
        // the balance (100) differs from the last point (120), as after a device merge
        let id = depot(&v, 100_00, &f(today));
        {
            let db = v.db().unwrap();
            upsert(&db, id, &f(today - chrono::Duration::days(1)), 80_00, 1).unwrap();
            upsert(&db, id, &f(today), 120_00, 2).unwrap();
        }
        let h = v.balance_history(vec![id], 5).unwrap();
        let values: Vec<i64> = h.iter().map(|p| p.balance_cents).collect();
        // starts on the first known day: no invented past, no shifted line
        assert_eq!(values, vec![80_00, 120_00]);
    }

    #[test]
    fn a_depot_without_history_is_known_only_from_its_reported_balance_on() {
        let (_d, v) = open("depot-values-unrecorded");
        let a = depot(&v, 100, "2026-03-04"); // never recorded, balance reported on the 4th
        let b = depot(&v, 40, "2026-03-04");
        let db = v.db().unwrap();
        upsert(&db, b, "2026-03-01", 40, 1).unwrap();
        let s = series(&db, &[a, b], day("2026-03-01"), day("2026-03-05"))
            .unwrap()
            .unwrap();
        // before the 4th only the recorded depot is known; today's balance is not carried back
        assert_eq!(s.values, vec![40, 40, 40, 140, 140]);
        assert_eq!(s.covered, vec![1, 1, 1, 2, 2]);
        assert_eq!(s.total_depots, 2);
    }

    #[test]
    fn the_total_history_does_not_invent_a_past_for_a_depot_without_history() {
        let (_d, v) = open("depot-values-total");
        let today = chrono::Local::now().date_naive();
        let f = |n: i64| {
            (today - chrono::Duration::days(n))
                .format("%Y-%m-%d")
                .to_string()
        };
        let a = depot(&v, 100, &f(0)); // no recorded value
        let b = depot(&v, 40, &f(0));
        {
            let db = v.db().unwrap();
            upsert(&db, b, &f(3), 40, 1).unwrap();
        }
        let h = v.balance_history(vec![a, b], 5).unwrap();
        let values: Vec<i64> = h.iter().map(|p| p.balance_cents).collect();
        // the unrecorded depot adds nothing before its balance day (it was 100 only today)
        assert_eq!(values, vec![40, 40, 40, 140]);
    }

    #[test]
    fn the_best_source_of_a_day_is_shown_and_the_others_stay() {
        let (_d, v) = open("depot-values-sources");
        let id = depot(&v, 1, "2026-03-03");
        let db = v.db().unwrap();
        upsert_from(&db, id, "2026-03-01", 900, 1, "prices", "estimated").unwrap();
        upsert_from(&db, id, "2026-03-01", 950, 1, "import", "observed").unwrap();
        upsert_from(&db, id, "2026-03-02", 970, 1, "prices", "reconstructed").unwrap();
        upsert_from(&db, id, "2026-03-02", 960, 1, "prices", "estimated").unwrap();
        upsert_from(&db, id, "2026-03-03", 990, 1, "import", "observed").unwrap();
        upsert(&db, id, "2026-03-03", 1000, 2).unwrap();
        let s = series(&db, &[id], day("2026-03-01"), day("2026-03-03"))
            .unwrap()
            .unwrap();
        // observed beats reconstructed beats estimated; a fetch beats an import of the same quality
        assert_eq!(s.values, vec![950, 960, 1000]);
        let n: i64 = db
            .query_row("SELECT count(*) FROM depot_values", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 5, "nothing is overwritten across sources");
    }

    #[test]
    fn the_depot_history_says_how_many_depots_each_day_covers() {
        let (_d, v) = open("depot-values-api");
        let today = chrono::Local::now().date_naive();
        let f = |n: i64| {
            (today - chrono::Duration::days(n))
                .format("%Y-%m-%d")
                .to_string()
        };
        let a = depot(&v, 100, &f(0));
        let b = depot(&v, 40, &f(0));
        {
            let db = v.db().unwrap();
            upsert(&db, a, &f(3), 100, 1).unwrap();
            upsert(&db, b, &f(1), 40, 1).unwrap();
        }
        let h = v.depot_value_history(vec![], 10).unwrap();
        let shape: Vec<(i64, u32, u32)> = h
            .iter()
            .map(|p| (p.value_cents, p.covered_depots, p.total_depots))
            .collect();
        // starts on the first known day; the second depot joins on day -1
        assert_eq!(h.first().unwrap().date, f(3));
        assert_eq!(
            shape,
            vec![(100, 1, 2), (100, 1, 2), (140, 2, 2), (140, 2, 2)]
        );
        let only_b = v.depot_value_history(vec![b], 10).unwrap();
        assert_eq!(only_b.len(), 2);
        assert_eq!(only_b[0].total_depots, 1);
        assert!(v.depot_value_history(vec![a + 99], 10).unwrap().is_empty());
    }

    #[test]
    fn the_newer_value_of_a_day_wins_a_merge_and_an_older_one_does_not() {
        let (_d, v) = open("depot-values-merge");
        let id = depot(&v, 1, "2026-03-01");
        let db = v.db().unwrap();
        upsert(&db, id, "2026-03-01", 100, 50).unwrap();
        upsert(&db, id, "2026-03-01", 90, 40).unwrap(); // older edit
        let c: i64 = db
            .query_row(
                "SELECT value_cents FROM depot_values WHERE account_id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(c, 100);
        upsert(&db, id, "2026-03-01", 120, 60).unwrap();
        let c: i64 = db
            .query_row(
                "SELECT value_cents FROM depot_values WHERE account_id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(c, 120);
    }
}
