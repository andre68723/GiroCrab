//! Versioned vault schema (GitHub #206).
//!
//! `PRAGMA user_version` is the number of steps in [`STEPS`] a vault has run.
//! Opening a vault runs the missing steps in order, all in one transaction
//! ([`migrate`]); a vault with a higher version than this build
//! knows is refused with [`CoreError::SchemaTooNew`] and left untouched.
//!
//! - 1: the complete schema for a new vault ([`BASELINE`]).
//! - 2–8: empty. Vaults written before #206 carry these numbers but reached
//!   them through idempotent probes, so their schema may lack pieces.
//! - 9: [`repair_to_v8`] runs those probes once more, so every vault ends up
//!   with the same schema whatever its history (no-op on a new vault).
//! - 10: opening balances (#212): table `opening_balances` and
//!   `accounts.balance_derived`.
//! - 11: sync log (#238): table `sync_log`, local to the device.
//! - 12: SEPA references on bookings: `end_to_end_id`, `mandate_id`, `creditor_id`.
//! - 13: Sachwerte (#87): items, dated quantities and values, metal prices.
//! - 14: securities transactions (#314): table `security_transactions`, extra
//!   position fields on `holdings` (previous day, profit/loss, custody type).
//! - 15: item references on bookings (#354): `transactions.item_refs`, the
//!   product ids (ASIN) of an Amazon order, one entry per item.
//! - 16: asset classification (#351): persistent per-field user overrides and local holding metadata.
//! - 17: scheduled transfers (#240): table `planned_transfers`.
//! - 18: standing orders (#239): table `standing_orders`, replaced per fetch.
//! - 19: repair: vaults that an early draft of the scheduled transfers had migrated as "15" skipped
//!   step 15 and lack `transactions.item_refs`; adds it when missing.
//! - 20: direct debit mandates and creditor IDs (#143): tables `mandates`, `creditor_ids`,
//!   `mandate_deletions` (marks for the vault merge).
//! - 21: `planned_transfers.kind` (#143): `transfer` or `debit` (submitted collection, incoming).
//! - 22: `mandates.returns_ack` (#143): returns acknowledged up to this date.
//! - 23: tags on bookings (#228): tables `tags`, `transaction_tags` and the
//!   edit stamp `transactions.tags_ms`.
//! - 24: rules for several accounts (#562): table `rule_accounts`; `rules.account_id` mirrors the
//!   lowest id of the set (foreign key / older readers), the table is the source of truth.
//! - 25: index `tx_amount_date`: the payment-service transfer pairing
//!   (`vault/transfer_pairs.rs`) looks up counter bookings by amount and date.
//! - 26: `depot_values`: value of a securities depot per day, one point per fetch.
//!
//! Schema changes are a new step at the end of [`STEPS`]; never edit an
//! existing step. After a bump, run
//! `cargo run -p girocrab-core --example make_fixture_vault` and commit the
//! new fixture (see `docs/ARCHITECTURE.md`).

use rusqlite::{Connection, OptionalExtension};

use crate::error::{CoreError, CoreResult};

enum Step {
    Sql(&'static str),
    Hook(fn(&Connection) -> CoreResult<()>),
}

const STEPS: &[Step] = &[
    Step::Sql(BASELINE), // 1
    Step::Sql(""),       // 2–8: reached through probes before #206
    Step::Sql(""),
    Step::Sql(""),
    Step::Sql(""),
    Step::Sql(""),
    Step::Sql(""),
    Step::Sql(""),
    Step::Hook(repair_to_v8),                                  // 9
    Step::Hook(opening_balances),                              // 10
    Step::Sql(SYNC_LOG),                                       // 11
    Step::Hook(sepa_refs),                                     // 12
    Step::Hook(tangible_assets),                               // 13
    Step::Hook(security_transactions),                         // 14
    Step::Hook(item_refs),                                     // 15
    Step::Sql(crate::asset_classification::store::SCHEMA),     // 16: asset classification (#351)
    Step::Hook(crate::planned::planned_transfers),             // 17
    Step::Hook(crate::standing_orders::standing_orders_table), // 18
    Step::Hook(item_refs),                                     // 19 (repair, see above)
    Step::Hook(crate::mandates::mandates_tables),              // 20
    Step::Hook(crate::planned::planned_kind),                  // 21
    Step::Hook(crate::mandates::mandate_return_ack),           // 22
    Step::Hook(crate::tags::tags_tables),                      // 23
    Step::Sql(RULE_ACCOUNTS),                                  // 24
    Step::Sql(TX_AMOUNT_DATE),                                 // 25
    Step::Sql(crate::depot_values::SCHEMA),                    // 26: depot value history
];

/// Step 25. Not partial on `deleted_ms IS NULL`: SQLite then picked it for
/// plain scans of live bookings (`list_categories`), six times slower.
const TX_AMOUNT_DATE: &str = "
CREATE INDEX IF NOT EXISTS tx_amount_date ON transactions(amount_cents, booking_date);
";

/// Step 24 (#562): a rule can be restricted to several accounts.
const RULE_ACCOUNTS: &str = "
CREATE TABLE IF NOT EXISTS rule_accounts (
    rule_id    INTEGER NOT NULL REFERENCES rules(id)    ON DELETE CASCADE,
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    PRIMARY KEY (rule_id, account_id)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS rule_accounts_account ON rule_accounts(account_id);
INSERT OR IGNORE INTO rule_accounts(rule_id, account_id)
    SELECT id, account_id FROM rules WHERE account_id IS NOT NULL;
";

/// Step 14 (#314): securities transactions (buy, sell, dividend, fee, tax …)
/// of a depot, beside the cash bookings: they never count as cash flow and
/// have no category. `UNIQUE(account_id, external_id)` is the merge key.
/// `cash_transaction_id` is the matching cash booking on a settlement account
/// (local id, set by `link_cash_bookings`, not merged). Also the position
/// fields the banks report besides the stock. Idempotent like the steps above.
/// Step 15 (#354): product ids of an order's items (Amazon ASIN), comma
/// separated, one entry per item in purpose order ("" entry = unknown).
/// "" = none. Idempotent like [`sepa_refs`].
fn item_refs(conn: &Connection) -> CoreResult<()> {
    if !has_column(conn, "transactions", "item_refs")? {
        conn.execute_batch(
            "ALTER TABLE transactions ADD COLUMN item_refs TEXT NOT NULL DEFAULT '';",
        )?;
    }
    Ok(())
}

fn security_transactions(conn: &Connection) -> CoreResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS security_transactions (
            id                     INTEGER PRIMARY KEY,
            account_id             INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
            external_id            TEXT    NOT NULL,
            booking_date           TEXT    NOT NULL,
            settlement_date        TEXT    NOT NULL DEFAULT '',
            isin                   TEXT    NOT NULL DEFAULT '',
            wkn                    TEXT    NOT NULL DEFAULT '',
            name                   TEXT    NOT NULL DEFAULT '',
            direction              TEXT    NOT NULL DEFAULT 'other', -- buy | sell | dividend | fee | tax | other
            quantity               REAL    NOT NULL DEFAULT 0,
            price_cents            INTEGER NOT NULL DEFAULT 0,
            currency               TEXT    NOT NULL DEFAULT 'EUR',
            value_cents            INTEGER NOT NULL DEFAULT 0,
            fees_cents             INTEGER NOT NULL DEFAULT 0,
            taxes_cents            INTEGER NOT NULL DEFAULT 0,
            accrued_interest_cents INTEGER NOT NULL DEFAULT 0,
            fx_rate                REAL,
            booking_text           TEXT    NOT NULL DEFAULT '',
            cash_transaction_id    INTEGER REFERENCES transactions(id) ON DELETE SET NULL,
            changed_ms             INTEGER NOT NULL DEFAULT 0,
            deleted_ms             INTEGER,
            UNIQUE(account_id, external_id)
        );
        CREATE INDEX IF NOT EXISTS security_transactions_account
            ON security_transactions(account_id, booking_date, deleted_ms);
        CREATE INDEX IF NOT EXISTS security_transactions_isin
            ON security_transactions(isin, deleted_ms);",
    )?;
    for (col, def) in [
        ("prev_day_price_cents", "INTEGER"),
        ("prev_day_value_cents", "INTEGER"),
        ("profit_loss_purchase_cents", "INTEGER"),
        ("profit_loss_purchase_rel", "REAL"),
        ("profit_loss_prev_day_cents", "INTEGER"),
        ("profit_loss_prev_day_rel", "REAL"),
        ("custody_type", "TEXT NOT NULL DEFAULT ''"),
    ] {
        if !has_column(conn, "holdings", col)? {
            conn.execute_batch(&format!("ALTER TABLE holdings ADD COLUMN {col} {def};"))?;
        }
    }
    Ok(())
}

/// Step 12: SEPA references of a booking, bank-independent (end-to-end id,
/// direct-debit mandate reference, creditor id). "" = unknown. Idempotent:
/// a vault rewound by a test or a development build may carry the columns.
fn sepa_refs(conn: &Connection) -> CoreResult<()> {
    for col in ["end_to_end_id", "mandate_id", "creditor_id"] {
        if !has_column(conn, "transactions", col)? {
            conn.execute_batch(&format!(
                "ALTER TABLE transactions ADD COLUMN {col} TEXT NOT NULL DEFAULT '';"
            ))?;
        }
    }
    Ok(())
}

/// Step 13 (#87): Sachwerte – items, dated quantities and values, metal
/// prices. Idempotent: vaults of a development build of the feature already
/// carry the tables, maybe without later columns. Those builds numbered
/// this step 12, so such a vault never ran [`sepa_refs`]: run it here as well.
fn tangible_assets(conn: &Connection) -> CoreResult<()> {
    sepa_refs(conn)?;
    crate::assets::migrate(conn)
}

/// Step 11 (#238): the Abrufprotokoll. No foreign key: the log outlives a
/// deleted connection (its name is kept in the row). Not part of a vault merge.
const SYNC_LOG: &str = "
CREATE TABLE sync_log (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    ts_ms           INTEGER NOT NULL,
    connection_id   INTEGER,
    connection_name TEXT    NOT NULL DEFAULT '',
    source          TEXT    NOT NULL DEFAULT 'manual', -- manual | auto | statements
    outcome         TEXT    NOT NULL,                  -- ok | error | cancelled
    message         TEXT    NOT NULL DEFAULT '',       -- scrubbed, see sync_log.rs
    imported        INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX sync_log_ts ON sync_log(ts_ms);
";

/// Schema version of a vault written by this build.
pub(crate) const SCHEMA_VERSION: i64 = STEPS.len() as i64;

/// Step 10 (#212): balance computed from bookings (connector reports none)
/// plus the opening balance per account. Idempotent: vaults opened by a
/// development build of the feature already carry both at version 8 or 9.
fn opening_balances(conn: &Connection) -> CoreResult<()> {
    if !has_column(conn, "accounts", "balance_derived")? {
        conn.execute_batch(
            "ALTER TABLE accounts ADD COLUMN balance_derived INTEGER NOT NULL DEFAULT 0;",
        )?;
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS opening_balances (
            account_id    INTEGER PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
            opening_date  TEXT    NOT NULL DEFAULT '', -- YYYY-MM-DD, start of the day; '' = before the first booking
            opening_cents INTEGER,                     -- i64 cents; NULL = cleared
            updated_ms    INTEGER NOT NULL DEFAULT 0   -- newest edit wins in a vault merge
        );",
    )?;
    Ok(())
}

/// Refuses a vault written by a newer build, before anything touches the file.
pub(crate) fn check_not_too_new(conn: &Connection) -> CoreResult<()> {
    let vault_version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if vault_version > SCHEMA_VERSION {
        return Err(CoreError::SchemaTooNew {
            vault_version,
            app_version: SCHEMA_VERSION,
        });
    }
    Ok(())
}

/// Brings the vault to [`SCHEMA_VERSION`]; nothing changes if a step fails.
pub(crate) fn migrate(conn: &mut Connection) -> CoreResult<()> {
    check_not_too_new(conn)?;
    let tx = conn.transaction()?;
    // read again inside the transaction: another connection may have migrated meanwhile
    let done: i64 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if done >= SCHEMA_VERSION {
        return Ok(());
    }
    for step in &STEPS[done as usize..] {
        match step {
            Step::Sql(sql) => tx.execute_batch(sql)?,
            Step::Hook(f) => f(&tx)?,
        }
    }
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(())
}

fn has_column(conn: &Connection, table: &str, col: &str) -> CoreResult<bool> {
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM pragma_table_info(?1) WHERE name = ?2",
        [table, col],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Step 9: the probe-based schema setup of schema 8, run once on every vault.
/// Each part checks what is there (never `user_version`), so it repairs a
/// vault left half-migrated by an older build and is a no-op on [`BASELINE`].
fn repair_to_v8(conn: &Connection) -> CoreResult<()> {
    // v5: loan_principal_cents -> initial_balance_cents (v1 had neither column)
    if !has_column(conn, "accounts", "initial_balance_cents")? {
        if has_column(conn, "accounts", "loan_principal_cents")? {
            conn.execute_batch(
                "ALTER TABLE accounts RENAME COLUMN loan_principal_cents TO initial_balance_cents;",
            )?;
        } else {
            conn.execute_batch("ALTER TABLE accounts ADD COLUMN initial_balance_cents INTEGER;")?;
        }
    }
    // v3: merchant_logo_cache(domain, image_bytes, updated_at, source) – a cache,
    // rebuilt only if it is missing or still has the v2 layout
    let has_logo_cache: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'merchant_logo_cache'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !has_logo_cache || !has_column(conn, "merchant_logo_cache", "source")? {
        conn.execute_batch(
            "DROP TABLE IF EXISTS merchant_logo_cache;
             CREATE TABLE merchant_logo_cache (domain TEXT PRIMARY KEY, image_bytes BLOB,
                updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now')), source TEXT NOT NULL DEFAULT '');",
        )?;
    }
    // v6: user notes + import fingerprint
    if !has_column(conn, "transactions", "note")? {
        conn.execute_batch("ALTER TABLE transactions ADD COLUMN note TEXT NOT NULL DEFAULT '';")?;
    }
    if !has_column(conn, "transactions", "import_fp")? {
        conn.execute_batch(
            "ALTER TABLE transactions ADD COLUMN import_fp TEXT NOT NULL DEFAULT '';",
        )?;
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS tx_import_fp ON transactions(account_id, import_fp) WHERE import_fp != '';",
    )?;
    // accounts.access_revoked (PSD2 access withdrawn)
    if !has_column(conn, "accounts", "access_revoked")? {
        conn.execute_batch(
            "ALTER TABLE accounts ADD COLUMN access_revoked INTEGER NOT NULL DEFAULT 0;",
        )?;
    }
    // transactions.purpose_code / gvc / salary_flag (salary detection)
    for (col, def) in [
        ("purpose_code", "TEXT NOT NULL DEFAULT ''"),
        ("gvc", "TEXT NOT NULL DEFAULT ''"),
        ("salary_flag", "INTEGER"),
    ] {
        if !has_column(conn, "transactions", col)? {
            conn.execute_batch(&format!("ALTER TABLE transactions ADD COLUMN {col} {def};"))?;
        }
    }
    // v4: encrypted documents (Beleg-Abruf & PostBox)
    conn.execute_batch(crate::documents::DOC_SCHEMA)?;
    if !has_column(conn, "booking_documents", "exported_at")? {
        conn.execute_batch("ALTER TABLE booking_documents ADD COLUMN exported_at INTEGER;")?;
    }
    conn.execute_batch(crate::insights::OVERRIDE_SCHEMA)?;
    conn.execute_batch(crate::vault::HOLDINGS_SCHEMA)?;
    conn.execute_batch(crate::planning::PLANNING_SCHEMA)?;
    conn.execute_batch(crate::recurring::RECURRING_SCHEMA)?;
    // v7: user-defined account groups
    crate::user_groups::migrate(conn)?;
    // v8: sync keys, per-field edit stamps and soft deletes
    crate::sync_merge::migrate(conn)?;
    // manual loans (#36)
    conn.execute_batch(crate::loans::LOAN_SCHEMA)?;
    // after all column migrations: the indexes need the newest columns
    conn.execute_batch(crate::vault::PERF_INDEXES)?;
    Ok(())
}

/// Step 1: the schema of a new vault – identical (up to layout and comments)
/// to what schema 8 built step by step; `tests/fixtures/schema-v9.sql` pins it.
const BASELINE: &str = r#"
CREATE TABLE bank_connections (
    id              INTEGER PRIMARY KEY,
    connector       TEXT    NOT NULL,              -- ConnectorKind::as_str()
    display_name    TEXT    NOT NULL,
    login_name      TEXT    NOT NULL DEFAULT '',
    bank_code       TEXT    NOT NULL DEFAULT '',
    sf_symbol       TEXT    NOT NULL DEFAULT 'building.columns',
    state_json      TEXT    NOT NULL DEFAULT '{}',  -- connector state (FinTS system id, BPD, PSD2 session ...)
    last_sync_unix  INTEGER,
    created_unix    INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    created_ms      INTEGER NOT NULL DEFAULT 0,
    deleted_ms      INTEGER                        -- NULL = live (sync tombstone)
);

CREATE TABLE account_groups (
    id          INTEGER PRIMARY KEY,
    name        TEXT    NOT NULL UNIQUE,
    sf_symbol   TEXT    NOT NULL DEFAULT 'folder',
    sort_order  INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE accounts (
    id              INTEGER PRIMARY KEY,
    connection_id   INTEGER REFERENCES bank_connections(id) ON DELETE SET NULL,
    group_id        INTEGER NOT NULL REFERENCES account_groups(id),
    external_ref    TEXT    NOT NULL DEFAULT '',   -- IBAN / bank account id / PSD2 uid
    name            TEXT    NOT NULL,
    bank_name       TEXT    NOT NULL DEFAULT '',
    owner           TEXT    NOT NULL DEFAULT '',
    iban            TEXT    NOT NULL DEFAULT '',
    account_number  TEXT    NOT NULL DEFAULT '',
    currency        TEXT    NOT NULL DEFAULT 'EUR',
    kind            TEXT    NOT NULL DEFAULT 'checking',
    balance_cents   INTEGER NOT NULL DEFAULT 0,    -- i64 cents
    balance_date    TEXT    NOT NULL DEFAULT '',
    sf_symbol       TEXT    NOT NULL DEFAULT 'building.columns',
    sort_order      INTEGER NOT NULL DEFAULT 0,
    initial_balance_cents INTEGER,                 -- original amount of a Darlehen (i64 cents)
    access_revoked  INTEGER NOT NULL DEFAULT 0,    -- bank / PSD2 consent no longer grants access
    user_group_uuid TEXT,                          -- user_groups.uuid (NULL = not placed)
    user_sort_order INTEGER NOT NULL DEFAULT 0,
    placement_ms    INTEGER NOT NULL DEFAULT 0,
    name_ms         INTEGER NOT NULL DEFAULT 0,
    color_hex       TEXT,
    color_ms        INTEGER NOT NULL DEFAULT 0,
    symbol_ms       INTEGER NOT NULL DEFAULT 0,
    archived_unix   INTEGER,
    archived_ms     INTEGER NOT NULL DEFAULT 0,
    created_ms      INTEGER NOT NULL DEFAULT 0,
    deleted_ms      INTEGER,
    UNIQUE (connection_id, external_ref)
);

CREATE TABLE categories (
    id            INTEGER PRIMARY KEY,
    parent_id     INTEGER REFERENCES categories(id) ON DELETE CASCADE,
    name          TEXT    NOT NULL,
    sf_symbol     TEXT    NOT NULL DEFAULT 'tag',
    color_hex     TEXT    NOT NULL DEFAULT '#8E8E93',
    budget_cents  INTEGER,                         -- optional monthly budget, i64 cents
    sort_order    INTEGER NOT NULL DEFAULT 0,
    uuid          TEXT,                            -- sync key
    name_ms       INTEGER NOT NULL DEFAULT 0,
    parent_ms     INTEGER NOT NULL DEFAULT 0,
    symbol_ms     INTEGER NOT NULL DEFAULT 0,
    color_ms      INTEGER NOT NULL DEFAULT 0,
    budget_ms     INTEGER NOT NULL DEFAULT 0,
    order_ms      INTEGER NOT NULL DEFAULT 0,
    deleted_ms    INTEGER,
    merged_into   TEXT                             -- tombstone of a combined category → survivor uuid
);

CREATE UNIQUE INDEX categories_unique_name
    ON categories(IFNULL(parent_id, 0), name) WHERE deleted_ms IS NULL;
CREATE UNIQUE INDEX categories_uuid ON categories(uuid);

CREATE TABLE rules (
    id                INTEGER PRIMARY KEY,
    name              TEXT    NOT NULL,
    category_id       INTEGER NOT NULL REFERENCES categories(id) ON DELETE CASCADE,
    field             TEXT    NOT NULL DEFAULT 'any',
    match_kind        TEXT    NOT NULL DEFAULT 'keyword',   -- keyword | regex | expression
    pattern           TEXT    NOT NULL,
    case_sensitive    INTEGER NOT NULL DEFAULT 0,
    min_amount_cents  INTEGER,
    max_amount_cents  INTEGER,
    account_id        INTEGER REFERENCES accounts(id) ON DELETE CASCADE,
    priority          INTEGER NOT NULL DEFAULT 100,
    enabled           INTEGER NOT NULL DEFAULT 1,
    uuid              TEXT,                                 -- sync key
    updated_ms        INTEGER NOT NULL DEFAULT 0,
    deleted_ms        INTEGER,
    merged_into       TEXT
);

CREATE UNIQUE INDEX rules_uuid ON rules(uuid);

CREATE TABLE transactions (
    id                 INTEGER PRIMARY KEY,
    account_id         INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    external_id        TEXT    NOT NULL,           -- de-duplication key from the bank
    booking_date       TEXT    NOT NULL,           -- YYYY-MM-DD
    value_date         TEXT    NOT NULL DEFAULT '',
    amount_cents       INTEGER NOT NULL,           -- i64 cents, negative = debit
    currency           TEXT    NOT NULL DEFAULT 'EUR',
    counterparty       TEXT    NOT NULL DEFAULT '',
    counterparty_iban  TEXT    NOT NULL DEFAULT '',
    purpose            TEXT    NOT NULL DEFAULT '',
    booking_text       TEXT    NOT NULL DEFAULT '',
    category_id        INTEGER REFERENCES categories(id) ON DELETE SET NULL,
    category_source    TEXT    NOT NULL DEFAULT 'none',   -- none | rule | manual
    rule_id            INTEGER REFERENCES rules(id) ON DELETE SET NULL,
    checked            INTEGER NOT NULL DEFAULT 0,
    imported_unix      INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    note               TEXT    NOT NULL DEFAULT '',        -- user note (MoneyMoney import / detail view)
    import_fp          TEXT    NOT NULL DEFAULT '',        -- MoneyMoney de-duplication fingerprint
    purpose_code       TEXT    NOT NULL DEFAULT '',        -- SEPA purpose code (SALA, PENS, …)
    gvc                TEXT    NOT NULL DEFAULT '',        -- German Geschäftsvorfallcode (153, …)
    salary_flag        INTEGER,                            -- NULL = auto, 1 = salary, 0 = not salary (user)
    category_ms        INTEGER NOT NULL DEFAULT 0,         -- per-field edit stamps (sync)
    note_ms            INTEGER NOT NULL DEFAULT 0,
    checked_ms         INTEGER NOT NULL DEFAULT 0,
    text_ms            INTEGER NOT NULL DEFAULT 0,
    splits_ms          INTEGER NOT NULL DEFAULT 0,
    deleted_ms         INTEGER,                            -- NULL = live
    UNIQUE (account_id, external_id)
);

CREATE INDEX tx_account_date ON transactions(account_id, booking_date DESC);
CREATE INDEX tx_category ON transactions(category_id);
CREATE INDEX tx_import_fp ON transactions(account_id, import_fp) WHERE import_fp != '';
-- counts filter `deleted_ms IS NULL`: these indexes must cover it
CREATE INDEX tx_rule ON transactions(rule_id, deleted_ms);
CREATE INDEX tx_account_category ON transactions(account_id, category_id, deleted_ms);
CREATE INDEX tx_account_checked ON transactions(account_id, checked, deleted_ms);
CREATE INDEX tx_date_id ON transactions(booking_date DESC, id DESC);
CREATE INDEX tx_salary_marked ON transactions(counterparty_iban) WHERE salary_flag = 1;

CREATE TABLE merchant_logo_cache (
    domain        TEXT PRIMARY KEY,
    image_bytes   BLOB,                            -- PNG/ICO; NULL/empty = nothing found (negative cache)
    updated_at    INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    source        TEXT NOT NULL DEFAULT ''
);

CREATE TABLE settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    is_secret  INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE booking_documents (
    id          INTEGER PRIMARY KEY,
    booking_id  INTEGER REFERENCES transactions(id) ON DELETE SET NULL,
    account_id  INTEGER REFERENCES accounts(id) ON DELETE SET NULL,
    title       TEXT    NOT NULL DEFAULT '',
    doc_type    TEXT    NOT NULL DEFAULT 'beleg',   -- beleg | abrechnung | kontoauszug | postbox | steuer
    mime_type   TEXT    NOT NULL DEFAULT 'application/pdf',
    file_name   TEXT    NOT NULL DEFAULT '',
    pdf_bytes   BLOB    NOT NULL,
    source_url  TEXT    NOT NULL DEFAULT '',
    doc_date    TEXT    NOT NULL DEFAULT '',
    created_at  INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    exported_at INTEGER                                -- copied to the mirror folder (GitHub #57)
);

CREATE UNIQUE INDEX booking_documents_source ON booking_documents(source_url) WHERE source_url != '';
CREATE INDEX booking_documents_booking ON booking_documents(booking_id);
CREATE INDEX booking_documents_account ON booking_documents(account_id);
CREATE INDEX booking_documents_info ON booking_documents(
    doc_date DESC, id DESC, booking_id, account_id, title, doc_type, mime_type, file_name,
    length(pdf_bytes), source_url, created_at);

CREATE TABLE merchant_domain_overrides (
    merchant_key TEXT PRIMARY KEY,
    domain       TEXT NOT NULL,
    updated_at   INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);

CREATE TABLE holdings (
    id                   INTEGER PRIMARY KEY,
    account_id           INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    isin                 TEXT    NOT NULL DEFAULT '',
    wkn                  TEXT    NOT NULL DEFAULT '',
    name                 TEXT    NOT NULL DEFAULT '',
    quantity             REAL    NOT NULL DEFAULT 0,
    current_price_cents  INTEGER NOT NULL DEFAULT 0,
    purchase_price_cents INTEGER NOT NULL DEFAULT 0,
    value_cents          INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX holdings_account ON holdings(account_id);

CREATE TABLE transaction_splits (
    id             INTEGER PRIMARY KEY,
    transaction_id INTEGER NOT NULL REFERENCES transactions(id) ON DELETE CASCADE,
    category_id    INTEGER REFERENCES categories(id) ON DELETE SET NULL,
    amount_cents   INTEGER NOT NULL,
    note           TEXT    NOT NULL DEFAULT ''
);

CREATE INDEX split_tx ON transaction_splits(transaction_id);
CREATE INDEX split_cat ON transaction_splits(category_id);

CREATE TABLE transfer_templates (
    id                   INTEGER PRIMARY KEY,
    name                 TEXT    NOT NULL,
    recipient_name       TEXT    NOT NULL,
    recipient_iban       TEXT    NOT NULL,
    recipient_bic        TEXT    NOT NULL DEFAULT '',
    default_amount_cents INTEGER,
    default_purpose      TEXT    NOT NULL DEFAULT ''
);

CREATE TABLE recurring_series (
    id            INTEGER PRIMARY KEY,
    series_key    TEXT    NOT NULL UNIQUE,
    status        TEXT    NOT NULL DEFAULT 'manual',   -- manual | ignored
    interval      TEXT    NOT NULL DEFAULT 'monthly',  -- monthly | quarterly | halfyearly | yearly | custom
    custom_dates  TEXT    NOT NULL DEFAULT '',         -- custom: comma-separated MM-DD
    created_at    INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);

CREATE TABLE user_groups (
    uuid             TEXT    PRIMARY KEY,
    name             TEXT    NOT NULL,
    sf_symbol        TEXT    NOT NULL DEFAULT 'folder',
    sort_order       INTEGER NOT NULL DEFAULT 0,
    include_in_total INTEGER NOT NULL DEFAULT 1,
    updated_ms       INTEGER NOT NULL DEFAULT 0,     -- last edit of any field
    name_ms          INTEGER NOT NULL DEFAULT 0,
    symbol_ms        INTEGER NOT NULL DEFAULT 0,
    total_ms         INTEGER NOT NULL DEFAULT 0,
    order_ms         INTEGER NOT NULL DEFAULT 0,
    deleted_ms       INTEGER,                      -- tombstone, kept for sync
    merged_into      TEXT,                         -- tombstone of a combined group → survivor
    parent_uuid      TEXT,                         -- subgroup of this group (NULL = top level)
    parent_ms        INTEGER NOT NULL DEFAULT 0,
    color_hex        TEXT,
    color_ms         INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE loan_terms (
    account_id           INTEGER PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
    anchor_date          TEXT    NOT NULL,           -- YYYY-MM-DD, Stand at the end of this day
    anchor_balance_cents INTEGER NOT NULL,           -- outstanding balance (positive, i64 cents)
    interest_rate_ppm    INTEGER,                    -- nominal rate p.a. in millionths (4.99 % = 49900)
    match_expr           TEXT    NOT NULL DEFAULT '' -- search expression that links bookings ('' = none)
);

CREATE TABLE loan_links (
    transaction_id  INTEGER PRIMARY KEY REFERENCES transactions(id) ON DELETE CASCADE,
    loan_account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    source          TEXT    NOT NULL DEFAULT 'manual'  -- manual | rule | excluded
);

CREATE INDEX loan_links_loan ON loan_links(loan_account_id);
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_version_counts_the_steps() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        assert_eq!(SCHEMA_VERSION, 26);
    }

    /// A vault that an early draft migrated as version 15 (scheduled transfers
    /// in place of the item references) gets `item_refs` back.
    #[test]
    fn vault_that_skipped_item_refs_is_repaired() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        conn.execute_batch(
            "ALTER TABLE transactions DROP COLUMN item_refs; PRAGMA user_version = 15;",
        )
        .unwrap();
        assert!(!has_column(&conn, "transactions", "item_refs").unwrap());
        migrate(&mut conn).unwrap();
        assert!(has_column(&conn, "transactions", "item_refs").unwrap());
    }

    #[test]
    fn newer_version_is_refused_without_changes() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA user_version = 99").unwrap();
        match migrate(&mut conn) {
            Err(CoreError::SchemaTooNew {
                vault_version: 99,
                app_version,
            }) => assert_eq!(app_version, SCHEMA_VERSION),
            other => panic!("expected SchemaTooNew, got {other:?}"),
        }
        let tables: i64 = conn
            .query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tables, 0);
    }

    #[test]
    fn failed_step_leaves_the_vault_untouched() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        conn.execute_batch(
            "PRAGMA user_version = 10; DROP TABLE sync_log; CREATE TABLE sync_log (x);",
        )
        .unwrap();
        assert!(migrate(&mut conn).is_err());
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 10);
    }
}
