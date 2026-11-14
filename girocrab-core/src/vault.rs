//! The encrypted ledger ("vault"): one SQLCipher database file.
//!
//! Schema (clean-room design, see `SCHEMA` below):
//!   bank_connections  1─n accounts 1─n transactions n─1 categories (tree)
//!   rules             n─1 categories, optional n─1 accounts
//!   settings          key/value, secrets flagged, all inside the encrypted file
//!   account_groups    sidebar grouping by account kind (Giro, Credit cards, ...)
//!   user_groups       user-defined sidebar groups ("Privat", "Geschäft", ...), synced by uuid
//!
//! This file only opens, keys, migrates and snapshots the vault. Feature SQL lives
//! in one `impl Vault` per feature module (`vault/*.rs`, `loans.rs`, `connectors/…`),
//! see `docs/ARCHITECTURE.md` › "Vault modules".

use std::ops::{Deref, DerefMut};
use std::panic::Location;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rusqlite::{params, Connection};
use zeroize::Zeroizing;

use crate::error::{CoreError, CoreResult};

mod accounts;
mod bookings;
mod categories;
mod merge;
mod rules;
mod settings;

pub(crate) use bookings::enrich;
pub use merge::MergeStats;
pub use settings::KNOWN_SETTINGS;

/// Securities positions of depot accounts (replaced on every sync).
pub(crate) const HOLDINGS_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS holdings (
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
CREATE INDEX IF NOT EXISTS holdings_account ON holdings(account_id);
";

/// Indexes for the queries every reload runs (idempotent, also for existing
/// vaults; measured on a vault with ~19 000 bookings):
/// - `tx_date_id`: the booking list is ordered by date, then id – with the
///   old `tx_date` (date only) SQLite sorted every result again.
/// - The sidebar counters ("ohne Kategorie", "neu") per account use
///   `tx_account_category` / `tx_account_checked`; they also cover
///   `deleted_ms` and are defined in `sync_merge::COVERING_INDEXES`.
/// - `tx_salary_marked`: the user-marked salary bookings, looked up on every
///   booking list, without scanning all bookings.
pub(crate) const PERF_INDEXES: &str = "
DROP INDEX IF EXISTS tx_date;
CREATE INDEX IF NOT EXISTS tx_date_id ON transactions(booking_date DESC, id DESC);
CREATE INDEX IF NOT EXISTS tx_salary_marked ON transactions(counterparty_iban) WHERE salary_flag = 1;
";

pub(crate) use crate::schema::SCHEMA_VERSION;

/// Base schema before #206 (`version == 0` branch of the probe-based setup):
/// no sync, group or salary columns. Tests build old vaults from it; new vaults
/// get `schema::BASELINE`.
#[cfg(test)]
pub(crate) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS bank_connections (
    id              INTEGER PRIMARY KEY,
    connector       TEXT    NOT NULL,              -- ConnectorKind::as_str()
    display_name    TEXT    NOT NULL,
    login_name      TEXT    NOT NULL DEFAULT '',
    bank_code       TEXT    NOT NULL DEFAULT '',
    sf_symbol       TEXT    NOT NULL DEFAULT 'building.columns',
    state_json      TEXT    NOT NULL DEFAULT '{}',  -- connector state (FinTS system id, BPD, PSD2 session ...)
    last_sync_unix  INTEGER,
    created_unix    INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);

CREATE TABLE IF NOT EXISTS account_groups (
    id          INTEGER PRIMARY KEY,
    name        TEXT    NOT NULL UNIQUE,
    sf_symbol   TEXT    NOT NULL DEFAULT 'folder',
    sort_order  INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS accounts (
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
    UNIQUE (connection_id, external_ref)
);

CREATE TABLE IF NOT EXISTS categories (
    id            INTEGER PRIMARY KEY,
    parent_id     INTEGER REFERENCES categories(id) ON DELETE CASCADE,
    name          TEXT    NOT NULL,
    sf_symbol     TEXT    NOT NULL DEFAULT 'tag',
    color_hex     TEXT    NOT NULL DEFAULT '#8E8E93',
    budget_cents  INTEGER,                         -- optional monthly budget, i64 cents
    sort_order    INTEGER NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX IF NOT EXISTS categories_unique_name ON categories(IFNULL(parent_id, 0), name);

CREATE TABLE IF NOT EXISTS rules (
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
    enabled           INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS transactions (
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
    UNIQUE (account_id, external_id)
);
CREATE INDEX IF NOT EXISTS tx_account_date ON transactions(account_id, booking_date DESC);
CREATE INDEX IF NOT EXISTS tx_category ON transactions(category_id);
CREATE INDEX IF NOT EXISTS tx_import_fp ON transactions(account_id, import_fp) WHERE import_fp != '';

CREATE TABLE IF NOT EXISTS merchant_logo_cache (
    domain        TEXT PRIMARY KEY,
    image_bytes   BLOB,                            -- PNG/ICO; NULL/empty = nothing found (negative cache)
    updated_at    INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    source        TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    is_secret  INTEGER NOT NULL DEFAULT 0
);
"#;

#[derive(uniffi::Object)]
pub struct Vault {
    conn: Mutex<Connection>,
    pub(crate) pending_account_fetches: Mutex<
        std::collections::HashMap<
            i64,
            (crate::models::ConnectorKind, crate::connectors::FetchResult),
        >,
    >,
    path: String,
    /// The master key, kept for sync snapshots and peer merges so the app
    /// doesn't ask the keychain again. Wiped on drop.
    key: Mutex<Zeroizing<Vec<u8>>>,
    _open: OpenVault,
}

/// Vaults alive in this process (the app's own plus peer copies a merge opens).
static OPEN_VAULTS: AtomicUsize = AtomicUsize::new(0);

/// Counts a vault as open; the last one to drop clears the process-wide
/// merchant cache, which holds booking texts in plain text (#267).
struct OpenVault;

impl OpenVault {
    fn new() -> Self {
        OPEN_VAULTS.fetch_add(1, Ordering::SeqCst);
        OpenVault
    }
}

impl Drop for OpenVault {
    fn drop(&mut self) {
        if OPEN_VAULTS.fetch_sub(1, Ordering::SeqCst) == 1 {
            crate::merchants::clear_resolve_cache();
        }
    }
}

/// Lock wait or hold time above which debug builds report the `db()` call site.
/// Every FFI call serialises on the vault mutex, so a long holder stalls the
/// main thread even when it runs off-main (#266).
const SLOW_LOCK: Duration = Duration::from_millis(50);

/// The locked connection. Debug builds log the `db()` call site when the lock
/// was waited for or held longer than `SLOW_LOCK`.
pub(crate) struct DbGuard<'a> {
    conn: MutexGuard<'a, Connection>,
    #[cfg(debug_assertions)]
    since: Instant,
    #[cfg(debug_assertions)]
    caller: &'static Location<'static>,
}

impl Deref for DbGuard<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.conn
    }
}

impl DerefMut for DbGuard<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

#[cfg(debug_assertions)]
impl Drop for DbGuard<'_> {
    fn drop(&mut self) {
        let held = self.since.elapsed();
        if held > SLOW_LOCK {
            eprintln!(
                "vault: lock held {} ms by {}",
                held.as_millis(),
                self.caller
            );
        }
    }
}

impl Vault {
    #[track_caller]
    pub(crate) fn db(&self) -> CoreResult<DbGuard<'_>> {
        let caller = Location::caller();
        let asked = Instant::now();
        let conn = self.conn.lock().map_err(|_| CoreError::Database {
            message: "vault lock poisoned".into(),
        })?;
        let waited = asked.elapsed();
        if cfg!(debug_assertions) && waited > SLOW_LOCK {
            eprintln!(
                "vault: waited {} ms for the lock at {caller}",
                waited.as_millis()
            );
        }
        Ok(DbGuard {
            conn,
            #[cfg(debug_assertions)]
            since: Instant::now(),
            #[cfg(debug_assertions)]
            caller,
        })
    }

    fn kept_key(&self) -> CoreResult<MutexGuard<'_, Zeroizing<Vec<u8>>>> {
        self.key.lock().map_err(|_| CoreError::Database {
            message: "vault key lock poisoned".into(),
        })
    }

    pub(crate) fn apply_key(conn: &Connection, key: &[u8]) -> CoreResult<()> {
        if key.len() < 16 {
            return Err(CoreError::InvalidInput {
                message: "master key must be at least 16 bytes".into(),
            });
        }
        // Raw key syntax: x'..hex..' -> no PBKDF2 on every open, key comes from Keychain.
        let pragma = Zeroizing::new(format!("PRAGMA key = \"x'{}'\";", hex::encode(key)));
        conn.execute_batch(&pragma)?;
        // Best effort: lock + wipe SQLCipher's in-memory key/page buffers.
        let _ = conn.query_row("PRAGMA cipher_log_level = NONE;", [], |_| Ok(()));
        let _ = conn.query_row("PRAGMA cipher_memory_security = ON;", [], |_| Ok(()));
        Ok(())
    }

    /// True if the file starts with the plaintext SQLite header.
    fn is_plaintext_sqlite(path: &str) -> bool {
        use std::io::Read;
        let mut buf = [0u8; 16];
        std::fs::File::open(path)
            .and_then(|mut f| f.read_exact(&mut buf))
            .map(|_| &buf == b"SQLite format 3\0")
            .unwrap_or(false)
    }

    fn migrate(conn: &mut Connection) -> CoreResult<()> {
        // Fails with NotADatabase if the key is wrong.
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
            r.get::<_, i64>(0)
        })?;
        // A vault from a newer build stays untouched (not even the pragmas below).
        crate::schema::check_not_too_new(conn)?;
        // Connection settings first: neither pragma takes effect inside a transaction.
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
        // All pending steps in one transaction (SQLite DDL is transactional): an
        // aborted run leaves the old schema and `user_version`, never half of it.
        crate::schema::migrate(conn)
    }

    pub fn path(&self) -> &str {
        &self.path
    }
}

#[uniffi::export]
impl Vault {
    /// Opens (or creates) the encrypted vault at `path` with the raw 32-byte
    /// master key. The key bytes are wiped from memory after use.
    #[uniffi::constructor]
    pub fn open(path: String, key: Vec<u8>) -> Result<Arc<Self>, CoreError> {
        let key = Zeroizing::new(key);
        // Default (SQLCipher 4 settings) first; if the key check fails, retry with
        // explicit compatibility modes for vaults written by older SQLCipher builds.
        // The cipher_compatibility pragma must be issued before the key.
        let mut tried: Vec<&str> = Vec::new();
        let mut opened: Option<Connection> = None;
        let mut first_err: Option<CoreError> = None;
        for compat in [None, Some(4u8), Some(3u8)] {
            let mut conn = Connection::open(&path)?;
            Self::apply_key(&conn, &key)?;
            if let Some(c) = compat {
                let _ = conn.query_row(&format!("PRAGMA cipher_compatibility = {c};"), [], |_| {
                    Ok(())
                });
            }
            match Self::migrate(&mut conn) {
                Ok(()) => {
                    if compat.is_some() {
                        tried.push("ok");
                        eprintln!("vault: opened with cipher_compatibility={compat:?}");
                    }
                    opened = Some(conn);
                    break;
                }
                Err(e) if matches!(e, CoreError::WrongKey) => {
                    tried.push(match compat {
                        None => "default",
                        Some(4) => "compat4",
                        _ => "compat3",
                    });
                    first_err.get_or_insert(e);
                }
                // Any other error is real (not a key/format mismatch): report as is.
                Err(e) => return Err(e),
            }
        }
        let Some(conn) = opened else {
            if Self::is_plaintext_sqlite(&path) {
                return Err(CoreError::InvalidInput {
                    message: "Die Tresordatei ist unverschlüsselt (Klartext-SQLite) und passt nicht zum Schlüssel. Sie wurde nicht verändert.".into(),
                });
            }
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let wal = std::fs::metadata(format!("{path}-wal"))
                .map(|m| m.len())
                .unwrap_or(0);
            let name = std::path::Path::new(&path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            crate::depot_debug::log(&format!(
                "vault open failed: HMAC check on page 1 failed for all modes tried: {} (file={name} size={size} wal_size={wal} key_len={})",
                tried.join(", "),
                key.len()
            ));
            return Err(first_err.unwrap_or(CoreError::WrongKey));
        };
        // categories/rules without a sync key (rows written outside the app's own
        // paths); used to happen as part of the per-open schema probes
        let _ = crate::sync_merge::fill_missing_uuids(&conn);
        // merge duplicate bank logins left by older versions (keeps bookings)
        let _ = crate::connections::deduplicate(&conn);
        // fold MoneyMoney accounts a merge added next to the bank account
        // that took them over (keeps bookings)
        let _ = crate::sync_merge::twins::fold_twins(&conn);
        // repair "MÃ¼ller" -> "Müller" stored by older versions
        let _ = crate::text::repair_stored_text(&conn);
        // one-time purge of favicons that don't belong to a known brand
        let _ = crate::insights::purge_untrusted_logos(&conn);
        Ok(Arc::new(Vault {
            conn: Mutex::new(conn),
            pending_account_fetches: Mutex::new(std::collections::HashMap::new()),
            path,
            key: Mutex::new(key),
            _open: OpenVault::new(),
        }))
    }

    /// Changes the master key (SQLCipher `PRAGMA rekey`).
    pub fn rekey(&self, new_key: Vec<u8>) -> Result<(), CoreError> {
        let key = Zeroizing::new(new_key);
        if key.len() < 16 {
            return Err(CoreError::InvalidInput {
                message: "master key must be at least 16 bytes".into(),
            });
        }
        let pragma = Zeroizing::new(format!("PRAGMA rekey = \"x'{}'\";", hex::encode(&*key)));
        self.db()?.execute_batch(&pragma)?;
        *self.kept_key()? = key;
        Ok(())
    }

    /// Writes a consistent, encrypted copy of this vault to `dest_path`
    /// (replacing it), for the sync folder. Runs under the connection lock,
    /// so concurrent writes never end up half in the copy.
    pub fn write_snapshot(&self, dest_path: String) -> Result<(), CoreError> {
        let raw_key = Zeroizing::new(format!("x'{}'", hex::encode(&**self.kept_key()?)));
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let p = format!("{dest_path}{suffix}");
            if std::path::Path::new(&p).exists() {
                std::fs::remove_file(&p).map_err(|e| CoreError::Database {
                    message: format!("{p}: {e}"),
                })?;
            }
        }
        let conn = self.db()?;
        conn.execute(
            "ATTACH DATABASE ?1 AS snap KEY ?2",
            params![dest_path, &*raw_key],
        )?;
        let res = conn
            .query_row("SELECT sqlcipher_export('snap')", [], |_| Ok(()))
            .and_then(|_| {
                conn.execute_batch(&format!("PRAGMA snap.user_version = {SCHEMA_VERSION}"))
            });
        let _ = conn.execute_batch("DETACH DATABASE snap");
        res?;
        Ok(())
    }

    /// Number of rows changed through this vault since it was opened; the
    /// app compares it to notice local changes worth syncing.
    pub fn change_count(&self) -> u64 {
        self.db().map(|c| c.total_changes()).unwrap_or(0)
    }
    /// Flushes the WAL into the main vault file (`wal_checkpoint(TRUNCATE)`),
    /// so copying the file (e.g. to iCloud) never misses committed frames.
    pub fn checkpoint(&self) -> Result<(), CoreError> {
        let conn = self.db()?;
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        Ok(())
    }

    pub fn file_path(&self) -> String {
        self.path.clone()
    }

    pub fn is_empty(&self) -> Result<bool, CoreError> {
        let db = self.db()?;
        let n: i64 = db.query_row(
            "SELECT (SELECT count(*) FROM accounts WHERE deleted_ms IS NULL)
                  + (SELECT count(*) FROM categories WHERE deleted_ms IS NULL)",
            [],
            |r| r.get(0),
        )?;
        Ok(n == 0)
    }
}

#[cfg(test)]
mod upgrade_tests {
    use super::*;
    use crate::models::BookingFilter;

    /// A vault written before #35/#37/#57 (no bank codes, salary flag,
    /// recurring table or mirror column) opens, migrates and keeps its data.
    #[test]
    fn pre_feature_vault_is_migrated_in_place() {
        let dir = std::env::temp_dir().join(format!("gc-upgrade-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("v.db").to_string_lossy().to_string();
        {
            let v = Vault::open(path.clone(), vec![2u8; 32]).unwrap();
            let c = v.db().unwrap();
            c.execute("INSERT INTO account_groups(name) VALUES ('G')", [])
                .unwrap();
            c.execute(
                "INSERT INTO accounts(group_id, external_ref, name) VALUES ((SELECT id FROM account_groups), 'A', 'Giro')",
                [],
            )
            .unwrap();
            c.execute(
                "INSERT INTO transactions(account_id, external_id, booking_date, amount_cents, counterparty, purpose)
                 VALUES ((SELECT id FROM accounts), 'x', date('now','-3 days'), 250000, 'ACME', 'Gehalt')",
                [],
            )
            .unwrap();
            c.execute_batch(
                "DROP INDEX tx_salary_marked; -- (an old vault has neither the index nor its column)
                 ALTER TABLE transactions DROP COLUMN purpose_code;
                 ALTER TABLE transactions DROP COLUMN gvc;
                 ALTER TABLE transactions DROP COLUMN salary_flag;
                 ALTER TABLE booking_documents DROP COLUMN exported_at;
                 DROP TABLE recurring_series; DROP TABLE sync_log;
                 PRAGMA user_version = 8; -- written by a probe-based build (before #206)",
            )
            .unwrap();
        }
        let v = Vault::open(path, vec![2u8; 32]).unwrap();
        let cols: Vec<String> = {
            let c = v.db().unwrap();
            let mut st = c
                .prepare("SELECT name FROM pragma_table_info('transactions')")
                .unwrap();
            let names = st
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            names
        };
        for col in ["purpose_code", "gvc", "salary_flag"] {
            assert!(cols.iter().any(|c| c == col), "{col} missing");
        }
        let page = v.list_transactions(BookingFilter::default()).unwrap();
        assert_eq!(page.transactions.len(), 1);
        assert!(page.transactions[0].is_salary);
        assert!(v.salary_cycle(vec![]).unwrap().is_some());
        assert!(v.list_recurring(vec![]).unwrap().is_empty());
        v.set_booking_salary_flag(page.transactions[0].id, Some(false))
            .unwrap();
        v.reset_document_mirror().unwrap();
        drop(v);
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod index_tests {
    use super::*;

    fn plan(db: &Connection, sql: &str) -> String {
        db.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap()
            .query_map([], |r| r.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join(" | ")
    }

    /// The queries every reload runs use their indexes (shapes as in
    /// `list_transactions`, `list_accounts` and `marked_salary_ibans`).
    #[test]
    fn reload_queries_use_their_indexes() {
        let path_dir = crate::test_support::TempDir::new("gc-index");
        let path = path_dir.join("v.db");
        let v = Vault::open(path.to_string_lossy().into(), vec![4u8; 32]).unwrap();
        let db = v.db().unwrap();
        // booking list: in index order, no sorting afterwards
        let p = plan(
            &db,
            "SELECT t.id FROM transactions t JOIN accounts a ON a.id = t.account_id
             LEFT JOIN categories cat ON cat.id = t.category_id
             WHERE 1=1 ORDER BY t.booking_date DESC, t.id DESC",
        );
        assert!(
            p.contains("tx_date_id") && !p.contains("TEMP B-TREE"),
            "{p}"
        );
        // sidebar counters: from the index, without reading the bookings
        for (cond, index) in [
            (
                "t.category_id IS NULL",
                "COVERING INDEX tx_account_category",
            ),
            ("t.checked = 0", "COVERING INDEX tx_account_checked"),
        ] {
            let p = plan(
                &db,
                &format!("SELECT count(*) FROM transactions t WHERE t.account_id = 1 AND {cond}"),
            );
            assert!(p.contains(index), "{p}");
        }
        let p = plan(
            &db,
            "SELECT DISTINCT counterparty_iban FROM transactions
             WHERE salary_flag = 1 AND counterparty_iban != '' AND amount_cents > 0",
        );
        assert!(p.contains("tx_salary_marked"), "{p}");
        // the superseded index is gone, also in vaults that had it
        let old: i64 = db
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'tx_date'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(old, 0);
        drop(db);
        let _ = std::fs::remove_file(path);
    }
}
