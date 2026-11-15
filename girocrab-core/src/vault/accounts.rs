//! Bank logins, accounts, account groups, holdings and balance history.

use std::collections::HashMap;

use rusqlite::{params, Connection, OptionalExtension};

use super::Vault;
use crate::error::{CoreError, CoreResult};
use crate::i18n::{display_name, tr, trf};
use crate::models::*;

impl Vault {
    /// Fills `initial_balance_cents` from the largest historical absolute
    /// balance, reconstructed backwards from the current balance over the
    /// account's own bookings. Never overwrites an existing value.
    pub(crate) fn derive_initial_balance(conn: &Connection, account_id: i64) -> CoreResult<()> {
        let (balance, existing): (i64, Option<i64>) = conn.query_row(
            "SELECT balance_cents, initial_balance_cents FROM accounts WHERE id = ?1",
            [account_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if existing.is_some() {
            return Ok(());
        }
        let mut stmt = conn.prepare("SELECT amount_cents FROM transactions WHERE account_id = ?1 AND deleted_ms IS NULL ORDER BY booking_date DESC, id DESC")?;
        let amounts: Vec<i64> = stmt
            .query_map([account_id], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        if amounts.is_empty() {
            return Ok(());
        }
        let mut running = balance;
        let mut max_abs = running.abs();
        for a in amounts {
            running -= a; // balance before this booking
            max_abs = max_abs.max(running.abs());
        }
        if max_abs > 0 {
            conn.execute(
                "UPDATE accounts SET initial_balance_cents = ?1 WHERE id = ?2",
                params![max_abs, account_id],
            )?;
        }
        Ok(())
    }

    pub(crate) fn ensure_group(
        conn: &Connection,
        name: &str,
        symbol: &str,
        order: i32,
    ) -> CoreResult<i64> {
        if let Some(id) = conn
            .query_row(
                "SELECT id FROM account_groups WHERE name = ?1",
                [name],
                |r| r.get(0),
            )
            .optional()?
        {
            return Ok(id);
        }
        conn.execute(
            "INSERT INTO account_groups(name, sf_symbol, sort_order) VALUES (?1, ?2, ?3)",
            params![name, symbol, order],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Adds a bank login or returns the existing one with the same provider and
    /// login (`(id, created)`).
    pub(crate) fn add_bank_connection_inner(
        conn: &Connection,
        connector: ConnectorKind,
        display_name: String,
        login_name: String,
    ) -> CoreResult<(i64, bool)> {
        Self::add_bank_connection_keyed(
            conn,
            connector,
            connector.as_str(),
            None,
            display_name,
            login_name,
        )
    }

    /// Like `add_bank_connection_inner`, with an explicit connector key
    /// (`fints:<blz>` for generic FinTS banks) and bank code.
    pub(crate) fn add_bank_connection_keyed(
        conn: &Connection,
        connector: ConnectorKind,
        key: &str,
        bank_code: Option<&str>,
        display_name: String,
        login_name: String,
    ) -> CoreResult<(i64, bool)> {
        let info = crate::connectors::descriptor(connector);
        let login_name = crate::connections::normalize_login(connector, &login_name);
        // Same provider + same login: update instead of creating a duplicate.
        // A live match wins over a deleted one; a deleted one is revived.
        let existing: Vec<(i64, String, bool)> = {
            let mut st = conn.prepare(
                // includes soft-deleted rows
                "SELECT id, login_name, deleted_ms IS NOT NULL FROM bank_connections
                 WHERE connector = ?1 ORDER BY deleted_ms IS NOT NULL, id",
            )?;
            let r = st
                .query_map([key], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            r
        };
        if let Some((id, _, deleted)) = existing.iter().find(|(_, l, _)| {
            !login_name.is_empty()
                && crate::connections::normalize_login(connector, l)
                    .eq_ignore_ascii_case(&login_name)
        }) {
            if !display_name.trim().is_empty() {
                let old_name: String = conn.query_row(
                    "SELECT display_name FROM bank_connections WHERE id = ?1",
                    [id],
                    |r| r.get(0),
                )?;
                conn.execute(
                    "UPDATE bank_connections SET display_name = ?1 WHERE id = ?2",
                    params![display_name.trim(), id],
                )?;
                // Accounts kept from the earlier login show `accounts.bank_name`: it
                // follows the new name like on a rename (`update_bank_connection`).
                conn.execute(
                    "UPDATE accounts SET bank_name = ?1
                     WHERE connection_id = ?2 AND bank_name IN (?3, ?4, '') AND bank_name <> ?1",
                    params![display_name.trim(), id, old_name, info.name],
                )?;
            }
            if *deleted {
                crate::sync_merge::revive_connection(conn, *id)?;
                return Ok((*id, true));
            }
            return Ok((*id, false));
        }
        conn.execute(
            "INSERT INTO bank_connections(connector, display_name, login_name, bank_code, sf_symbol, created_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                key,
                if display_name.trim().is_empty() { info.name.clone() } else { display_name },
                login_name,
                bank_code.map(str::to_string).unwrap_or(info.bank_code),
                info.sf_symbol,
                crate::user_groups::now_ms()
            ],
        )?;
        Ok((conn.last_insert_rowid(), true))
    }

    fn delete_account_rows(db: &Connection, account_id: i64) -> Result<(), CoreError> {
        crate::sync_merge::soft_delete_account(db, account_id)
    }

    fn purge_connection(db: &Connection, connection_id: i64) -> Result<(), CoreError> {
        crate::sync_merge::soft_delete_connection(db, connection_id)?;
        db.execute(
            "DELETE FROM settings WHERE key IN (?1, ?2, ?3)",
            params![
                format!("fints.rah10.private.{connection_id}"),
                format!("fints.rah10.counter.{connection_id}"),
                format!("fints.rah10.bank_keys.{connection_id}")
            ],
        )?;
        // stored secrets of this connection (PIN); the soft delete blanked the session (state_json)
        db.execute(
            "DELETE FROM settings WHERE key = ?1",
            [crate::connections::pin_key(connection_id)],
        )?;
        // the account choice of this login (#346): a re-added login starts fresh
        db.execute(
            "DELETE FROM settings WHERE key = ?1",
            [crate::connectors::account_selection::key(
                db,
                connection_id,
            )?],
        )?;
        Ok(())
    }
}

#[uniffi::export]
impl Vault {
    // ---------------------------------------------------------------- bank connections

    pub fn list_bank_connections(&self) -> Result<Vec<BankConnection>, CoreError> {
        let db = self.db()?;
        let mut stmt = db.prepare(
            "SELECT id, connector, display_name, login_name, bank_code, sf_symbol, last_sync_unix,
                    EXISTS (SELECT 1 FROM accounts a WHERE a.connection_id = bank_connections.id AND a.deleted_ms IS NULL)
                      AND NOT EXISTS (SELECT 1 FROM accounts a WHERE a.connection_id = bank_connections.id AND a.deleted_ms IS NULL AND a.archived_unix IS NULL)
             FROM bank_connections WHERE deleted_ms IS NULL ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            let kind: String = r.get(1)?;
            Ok((
                kind,
                BankConnection {
                    id: r.get(0)?,
                    connector: ConnectorKind::VolksbankKasselGoettingen,
                    display_name: r.get(2)?,
                    login_name: r.get(3)?,
                    bank_code: r.get(4)?,
                    sf_symbol: r.get(5)?,
                    last_sync_unix: r.get(6)?,
                    all_accounts_archived: r.get(7)?,
                },
            ))
        })?;
        let mut out = vec![];
        for row in rows {
            let (kind, mut c) = row?;
            if let Some(k) = ConnectorKind::from_str(&kind) {
                c.connector = k;
                out.push(c);
            }
        }
        Ok(out)
    }

    /// Connector key of a bank login (`fints:<BLZ>`,
    /// `enablebanking:<country>:<bank>`, `comdirect`, …) – what the banking
    /// policy's connector IDs are matched against.
    pub fn connection_key(&self, connection_id: i64) -> Result<String, CoreError> {
        let db = self.db()?;
        db.query_row(
            "SELECT connector FROM bank_connections WHERE id = ?1",
            [connection_id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| CoreError::NotFound {
            what: trf("Bankverbindung {0}", &[&connection_id]),
        })
    }

    /// Adds a bank login. Credentials (PIN/password) are *not* stored here;
    /// they are passed to `begin_sync` each time.
    pub fn add_bank_connection(
        &self,
        connector: ConnectorKind,
        display_name: String,
        login_name: String,
    ) -> Result<i64, CoreError> {
        if !crate::demo::is_magic_demo_login(&login_name) {
            crate::banking_gate::check_connector(connector.as_str())?;
        }
        let db = self.db()?;
        Ok(Self::add_bank_connection_inner(&db, connector, display_name, login_name)?.0)
    }

    /// Adds a login at any FinTS 3.0 PIN/TAN bank and stores the bank's URL
    /// (`fints.url.<blz>`). `blz` must be 8 digits, `url` an https URL.
    /// `tan_medium` is optional (stored as `fints.tan_medium.<connection id>`).
    pub fn add_fints_connection(
        &self,
        blz: String,
        url: String,
        display_name: String,
        login_name: String,
        tan_medium: String,
    ) -> Result<i64, CoreError> {
        let blz = blz.trim().to_string();
        let url = url.trim().to_string();
        if ConnectorKind::fints_blz(&ConnectorKind::fints_key(&blz)).is_none() {
            return Err(CoreError::InvalidInput {
                message: tr("Die BLZ muss aus genau 8 Ziffern bestehen.").into(),
            });
        }
        if !crate::connectors::fints_presets::is_valid_fints_url(&url) {
            return Err(CoreError::InvalidInput {
                message: tr("Die FinTS-URL muss mit https:// beginnen.").into(),
            });
        }
        let key = ConnectorKind::fints_key(&blz);
        if !crate::demo::is_magic_demo_login(&login_name) {
            crate::banking_gate::check_connector(&key)?;
        }
        let db = self.db()?;
        let (id, _) = Self::add_bank_connection_keyed(
            &db,
            ConnectorKind::FinTsGeneric,
            &key,
            Some(&blz),
            display_name,
            login_name,
        )?;
        db.execute(
            "INSERT INTO settings(key, value, is_secret) VALUES (?1, ?2, 0)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![format!("fints.url.{blz}"), url],
        )?;
        // Optional TAN medium name (HKTAN), e.g. Postbank BestSign "SO:Gerät".
        let tan_medium = tan_medium.trim();
        if !tan_medium.is_empty() {
            db.execute(
                "INSERT INTO settings(key, value, is_secret) VALUES (?1, ?2, 0)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![format!("fints.tan_medium.{id}"), tan_medium],
            )?;
        }
        Ok(id)
    }

    /// Adds a login at a bank reached through Enable Banking (PSD2). `aspsp`
    /// is the Enable Banking bank name, `country` its ISO alpha-2 code.
    /// `login_name` is only a label (defaults to the bank name).
    pub fn add_enable_banking_connection(
        &self,
        aspsp: String,
        country: String,
        display_name: String,
        login_name: String,
    ) -> Result<i64, CoreError> {
        let aspsp = aspsp.trim().to_string();
        let country = country.trim().to_uppercase();
        let key = ConnectorKind::enable_banking_key(&country, &aspsp);
        if aspsp.is_empty() || ConnectorKind::enable_banking_target(&key).is_none() {
            return Err(CoreError::InvalidInput {
                message: tr("Bank oder Ländercode ungültig.").into(),
            });
        }
        if !crate::demo::is_magic_demo_login(&login_name) {
            crate::banking_gate::check_connector(&key)?;
        }
        let login = if login_name.trim().is_empty() {
            aspsp.clone()
        } else {
            login_name
        };
        let display = if display_name.trim().is_empty() {
            aspsp.clone()
        } else {
            display_name
        };
        let db = self.db()?;
        let (id, _) = Self::add_bank_connection_keyed(
            &db,
            ConnectorKind::EnableBanking,
            &key,
            None,
            display,
            login,
        )?;
        Ok(id)
    }

    pub fn delete_bank_connection(&self, connection_id: i64) -> Result<(), CoreError> {
        let db = self.db()?;
        let tx = db.unchecked_transaction()?;
        Self::purge_connection(&tx, connection_id)?;
        tx.commit()?;
        drop(db);
        self.pending_account_fetches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&connection_id);
        Ok(())
    }

    /// Deletes one account with its bookings, documents and holdings. When it was the last
    /// account of its connection, the connection and its stored secrets (PIN, session) go too.
    /// Only archived accounts can be deleted (`set_account_archived` first).
    pub fn delete_account(&self, account_id: i64) -> Result<(), CoreError> {
        let db = self.db()?;
        let tx = db.unchecked_transaction()?;
        let Some((conn, archived)) = tx
            .query_row(
                "SELECT connection_id, archived_unix IS NOT NULL FROM accounts WHERE id = ?1 AND deleted_ms IS NULL",
                [account_id],
                |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, bool>(1)?)),
            )
            .optional()?
        else {
            return Err(CoreError::NotFound {
                what: trf("Konto {0}", &[&account_id]),
            });
        };
        if !archived {
            return Err(CoreError::InvalidInput {
                message: tr("Nur archivierte Konten können gelöscht werden. Bitte das Konto zuerst archivieren.")
                    .to_string(),
            });
        }
        Self::delete_account_rows(&tx, account_id)?;
        // linked loan payments of this account are gone with it
        crate::loans::refresh_all(&tx)?;
        if let Some(c) = conn {
            let left: i64 = tx.query_row(
                "SELECT count(*) FROM accounts WHERE connection_id = ?1 AND deleted_ms IS NULL",
                [c],
                |r| r.get(0),
            )?;
            if left == 0 {
                Self::purge_connection(&tx, c)?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    // ---------------------------------------------------------------- holdings

    pub fn list_holdings(&self, account_id: i64) -> Result<Vec<Holding>, CoreError> {
        holdings_from_db(&*self.db()?, account_id)
    }

    /// Securities transactions of a depot (#314), newest first.
    pub fn list_security_transactions(
        &self,
        account_id: i64,
    ) -> Result<Vec<SecurityTransaction>, CoreError> {
        let db = self.db()?;
        crate::security_transactions::list(&db, account_id)
    }

    // ---------------------------------------------------------------- accounts

    pub fn list_accounts(&self) -> Result<Vec<Account>, CoreError> {
        let db = self.db()?;
        // Sachwerte (#87): a stored value from an earlier day is recomputed first
        crate::assets::refresh_balances_if_stale(&db)?;
        let mut stmt = db.prepare(&format!(
            "SELECT a.id, a.connection_id, c.connector, a.group_id, a.name, a.bank_name, a.owner, a.iban, a.account_number,
                    a.currency, a.kind, a.balance_cents, a.balance_date, a.sf_symbol, a.sort_order,
                    (SELECT count(*) FROM transactions t WHERE t.account_id = a.id AND t.deleted_ms IS NULL),
                    -- uncategorised, split-aware like `category_match_sql(\"IS NULL\")`
                    (SELECT count(*) FROM transactions t WHERE t.account_id = a.id AND t.category_id IS NULL AND t.deleted_ms IS NULL
                        AND NOT EXISTS (SELECT 1 FROM transaction_splits sp WHERE sp.transaction_id = t.id))
                    + (SELECT count(DISTINCT sp.transaction_id) FROM transaction_splits sp
                        JOIN transactions t ON t.id = sp.transaction_id
                        WHERE t.account_id = a.id AND sp.category_id IS NULL AND t.deleted_ms IS NULL),
                    (SELECT count(*) FROM transactions t WHERE t.account_id = a.id AND t.checked = 0 AND t.deleted_ms IS NULL),
                    a.initial_balance_cents, g.uuid, IFNULL(g.include_in_total, 1) AND a.archived_unix IS NULL,
                    a.color_hex, IFNULL(c.connector, ''), IFNULL(c.bank_code, ''), IFNULL(c.login_name, ''), a.external_ref,
                    a.archived_unix, a.access_revoked,
                    o.opening_cents, IFNULL(o.opening_date, ''), {derived}
             FROM accounts a LEFT JOIN bank_connections c ON c.id = a.connection_id
             LEFT JOIN user_groups g ON g.uuid = a.user_group_uuid AND g.deleted_ms IS NULL
             LEFT JOIN opening_balances o ON o.account_id = a.id
             WHERE a.deleted_ms IS NULL
             ORDER BY a.sort_order, a.id",
            derived = crate::opening_balance::DERIVED_SQL
        ))?;
        let mut color_hashes: HashMap<i64, u64> = HashMap::new();
        let rows = stmt.query_map([], |r| {
            Ok(Account {
                id: r.get(0)?,
                connection_id: r.get(1)?,
                connector: r
                    .get::<_, Option<String>>(2)?
                    .and_then(|s| ConnectorKind::from_str(&s)),
                group_id: r.get(3)?,
                name: r.get(4)?,
                bank_name: r.get(5)?,
                owner: r.get(6)?,
                iban: r.get(7)?,
                account_number: r.get(8)?,
                currency: r.get(9)?,
                kind: AccountKind::from_str(&r.get::<_, String>(10)?),
                balance_cents: r.get(11)?,
                balance_date: r.get(12)?,
                sf_symbol: r.get(13)?,
                sort_order: r.get(14)?,
                transaction_count: r.get::<_, i64>(15)? as u32,
                uncategorized_count: r.get::<_, i64>(16)? as u32,
                new_count: r.get::<_, i64>(17)? as u32,
                initial_balance_cents: r
                    .get::<_, Option<i64>>(18)?
                    .map(i64::abs)
                    .filter(|c| *c > 0),
                user_group_uuid: r.get(19)?,
                in_total: r.get(20)?,
                // filled in below for accounts without a chosen colour
                color_hex: match r.get::<_, Option<String>>(21)? {
                    Some(c) => c,
                    None => {
                        color_hashes.insert(
                            r.get(0)?,
                            crate::user_groups::account_color_hash(
                                &r.get::<_, String>(22)?,
                                &r.get::<_, String>(23)?,
                                &r.get::<_, String>(24)?,
                                &r.get::<_, String>(25)?,
                            ),
                        );
                        String::new()
                    }
                },
                archived_unix: r.get(26)?,
                access_revoked: r.get::<_, i64>(27)? != 0,
                opening_balance_cents: r.get(28)?,
                opening_balance_date: r.get(29)?,
                balance_derived: r.get(30)?,
            })
        })?;
        let mut accounts = rows.collect::<Result<Vec<_>, _>>()?;
        // a subgroup of an excluded group is excluded too
        let excluded = crate::user_groups::excluded_from_total(&db)?;
        if !excluded.is_empty() {
            for a in accounts.iter_mut() {
                if a.user_group_uuid
                    .as_ref()
                    .is_some_and(|u| excluded.contains(u))
                {
                    a.in_total = false;
                }
            }
        }
        crate::user_groups::assign_default_colors(&mut accounts, &color_hashes);
        // Credit cards stored with balance 0: derive the open amount from bookings
        // (unless a Startsaldo drives the balance; bank-synced cards ignore one).
        for a in accounts.iter_mut().filter(|a| {
            a.kind == AccountKind::CreditCard
                && a.balance_cents == 0
                && a.transaction_count > 0
                && !(a.balance_derived && a.opening_balance_cents.is_some())
        }) {
            let mut st = db.prepare(
                "SELECT booking_date, amount_cents, counterparty || ' ' || purpose || ' ' || booking_text
                 FROM transactions WHERE account_id = ?1 AND deleted_ms IS NULL ORDER BY booking_date, id",
            )?;
            let tx_rows: Vec<(String, i64, String)> = st
                .query_map([a.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<Result<_, _>>()?;
            if let Some(open) = crate::connectors::open_card_balance(&tx_rows) {
                a.balance_cents = open;
            }
        }
        Ok(accounts)
    }

    /// Sidebar data: groups with their accounts and summed balances.
    pub fn list_account_groups(&self) -> Result<Vec<AccountGroup>, CoreError> {
        let accounts = self.list_accounts()?;
        let db = self.db()?;
        let mut stmt = db.prepare(
            "SELECT id, name, sf_symbol, sort_order FROM account_groups ORDER BY sort_order, id",
        )?;
        let groups = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i32>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(groups
            .into_iter()
            .map(|(id, name, sym, order)| {
                let accs: Vec<Account> = accounts
                    .iter()
                    .filter(|a| a.group_id == id)
                    .cloned()
                    .collect();
                // archived accounts stay listed but don't count (GiroCrab/GiroCrab#208)
                let balance = accs
                    .iter()
                    .filter(|a| a.currency == "EUR" && a.archived_unix.is_none())
                    .map(|a| a.balance_cents)
                    .sum();
                AccountGroup {
                    id,
                    name: display_name(&name),
                    sf_symbol: sym,
                    sort_order: order,
                    balance_cents: balance,
                    accounts: accs,
                }
            })
            .filter(|g| !g.accounts.is_empty())
            .collect())
    }

    /// End-of-day balances for the last `days` days (oldest first), for the
    /// header chart. Empty `account_ids` = all EUR accounts. Computed backwards
    /// from the current balance, so it is exact in cents.
    pub fn balance_history(
        &self,
        account_ids: Vec<i64>,
        days: u32,
    ) -> Result<Vec<DailyBalance>, CoreError> {
        let accounts = self.list_accounts()?;
        let single_account = account_ids.len() == 1;
        let chosen: Vec<&Account> = accounts
            .iter()
            .filter(|a| {
                (single_account || a.currency == "EUR")
                    && if account_ids.is_empty() {
                        a.in_total
                    } else {
                        account_ids.contains(&a.id)
                    }
            })
            .collect();
        let mut balance: i64 = chosen.iter().map(|a| a.balance_cents).sum();
        let ids: Vec<i64> = chosen.iter().map(|a| a.id).collect();
        let today = chrono::Local::now().date_naive();
        let start = today - chrono::Duration::days(days.max(1) as i64 - 1);
        // Securities depots with a valuation history (`depot_values`) take their course from it;
        // the walk back over bookings is only for the other accounts, so nothing counts twice.
        let depot_series = if ids.is_empty() {
            None
        } else {
            crate::depot_values::series(&*self.db()?, &ids, start, today)?
        };
        let book_ids: Vec<i64> = ids
            .iter()
            .copied()
            .filter(|id| {
                depot_series
                    .as_ref()
                    .is_none_or(|s| !s.accounts.contains(id))
            })
            .collect();
        // the depots with a history enter with their values of each day, not with today's balance
        if depot_series.is_some() {
            balance = chosen
                .iter()
                .filter(|a| book_ids.contains(&a.id))
                .map(|a| a.balance_cents)
                .sum();
        }
        let mut per_day: HashMap<String, i64> = HashMap::new();
        if !book_ids.is_empty() {
            let db = self.db()?;
            for a in chosen.iter().filter(|a| book_ids.contains(&a.id)) {
                let bdate = a.balance_date.trim();
                if !a.balance_derived && bdate.len() == 10 {
                    let after: i64 = db
                        .query_row(
                            "SELECT COALESCE(SUM(amount_cents), 0) FROM transactions WHERE account_id = ?1 AND booking_date > ?2 AND deleted_ms IS NULL AND external_id NOT LIKE 'pending-%'",
                            params![a.id, bdate],
                            |r| r.get(0),
                        )
                        .unwrap_or(0);
                    balance += after;
                }
            }
            let list = (0..book_ids.len())
                .map(|n| format!("?{}", n + 2))
                .collect::<Vec<_>>()
                .join(",");
            let mut stmt = db.prepare(&format!(
                "SELECT booking_date, SUM(amount_cents) FROM transactions WHERE account_id IN ({list}) AND booking_date > ?1 AND deleted_ms IS NULL AND external_id NOT LIKE 'pending-%' GROUP BY booking_date"
            ))?;
            let date = start.format("%Y-%m-%d").to_string();
            let args: Vec<rusqlite::types::Value> = std::iter::once(date.into())
                .chain(book_ids.iter().map(|&i| i.into()))
                .collect();
            let rows = stmt.query_map(rusqlite::params_from_iter(args), |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (d, s) = row?;
                per_day.insert(d, s);
            }
        }
        // Sachwerte (#87): no bookings, their value moves with prices – the walk
        // back keeps today's value, the daily valuation replaces it per day
        let valuation = if ids.is_empty() {
            None
        } else {
            crate::assets::value_series(&*self.db()?, &ids, start, today)?
        };
        let mut out = Vec::with_capacity(days as usize);
        let mut day = today;
        while day >= start {
            let key = day.format("%Y-%m-%d").to_string();
            out.push(DailyBalance {
                date: key.clone(),
                balance_cents: balance,
            });
            balance -= per_day.get(&key).copied().unwrap_or(0);
            day -= chrono::Duration::days(1);
        }
        out.reverse();
        // depot values enter directly; a depot adds nothing before its first point. Without other
        // accounts the history starts on the first day a depot has a value.
        if let Some(depots) = depot_series {
            let first_known = depots.covered.iter().position(|&c| c > 0).unwrap_or(0);
            for (p, v) in out.iter_mut().zip(&depots.values) {
                p.balance_cents += v;
            }
            if book_ids.is_empty() && valuation.is_none() {
                out.drain(..first_known);
            }
        }
        if let Some(values) = valuation {
            let now = values.last().copied().unwrap_or(0);
            for (p, v) in out.iter_mut().zip(values) {
                p.balance_cents += v - now;
            }
        }
        Ok(out)
    }

    /// Value history of securities depots for the last `days` days, from the first day any of
    /// them has a value. `account_ids` empty = all depots counted in the total. Each day says
    /// how many of the depots were covered (see [`crate::models::DepotValuePoint`]); a depot's
    /// first value starts a new coverage, so the chart can break the line there. Empty when no
    /// depot has a value.
    pub fn depot_value_history(
        &self,
        account_ids: Vec<i64>,
        days: u32,
    ) -> Result<Vec<DepotValuePoint>, CoreError> {
        let accounts = self.list_accounts()?;
        let single = account_ids.len() == 1;
        let ids: Vec<i64> = accounts
            .iter()
            .filter(|a| {
                a.kind == AccountKind::Brokerage
                    && (single || a.currency == "EUR")
                    && if account_ids.is_empty() {
                        a.in_total
                    } else {
                        account_ids.contains(&a.id)
                    }
            })
            .map(|a| a.id)
            .collect();
        let today = chrono::Local::now().date_naive();
        let start = today - chrono::Duration::days(days.max(1) as i64 - 1);
        let Some(series) = crate::depot_values::series(&*self.db()?, &ids, start, today)? else {
            return Ok(vec![]);
        };
        let Some(first) = series.covered.iter().position(|&c| c > 0) else {
            return Ok(vec![]);
        };
        Ok((first..series.values.len())
            .map(|i| DepotValuePoint {
                date: (start + chrono::Duration::days(i as i64))
                    .format("%Y-%m-%d")
                    .to_string(),
                value_cents: series.values[i],
                covered_depots: series.covered[i],
                total_depots: series.total_depots,
            })
            .collect())
    }

    pub fn rename_account(&self, account_id: i64, name: String) -> Result<(), CoreError> {
        let n = self.db()?.execute(
            "UPDATE accounts SET name = ?1, name_ms = ?3 WHERE id = ?2",
            params![name, account_id, crate::user_groups::now_ms()],
        )?;
        if n == 0 {
            return Err(CoreError::NotFound {
                what: trf("Konto {0}", &[&account_id]),
            });
        }
        Ok(())
    }

    /// Changes the account type (Kontoart), e.g. a checking account that is really a card.
    pub fn set_account_kind(&self, account_id: i64, kind: AccountKind) -> Result<(), CoreError> {
        let db = self.db()?;
        // The "by kind" sidebar sections are the `account_groups`: move the
        // account into the section of its new kind (#575).
        let (group, sym, order) = crate::connectors::group_for(kind);
        let gid = Self::ensure_group(&db, group, sym, order)?;
        let n = db.execute(
            "UPDATE accounts SET kind = ?1, group_id = ?2 WHERE id = ?3",
            params![kind.as_str(), gid, account_id],
        )?;
        if n == 0 {
            return Err(CoreError::NotFound {
                what: trf("Konto {0}", &[&account_id]),
            });
        }
        Ok(())
    }

    /// Sets the account owner (Kontoinhaber); empty clears it.
    pub fn set_account_owner(&self, account_id: i64, owner: String) -> Result<(), CoreError> {
        let n = self.db()?.execute(
            "UPDATE accounts SET owner = ?1 WHERE id = ?2",
            params![owner.trim(), account_id],
        )?;
        if n == 0 {
            return Err(CoreError::NotFound {
                what: trf("Konto {0}", &[&account_id]),
            });
        }
        Ok(())
    }

    /// Sets (or clears with `None`) the original loan amount of an account.
    pub fn set_account_initial_balance(
        &self,
        account_id: i64,
        cents: Option<i64>,
    ) -> Result<(), CoreError> {
        let cents = cents.map(|c| c.abs()).filter(|c| *c > 0);
        let n = self.db()?.execute(
            "UPDATE accounts SET initial_balance_cents = ?1 WHERE id = ?2",
            params![cents, account_id],
        )?;
        if n == 0 {
            return Err(CoreError::NotFound {
                what: trf("Konto {0}", &[&account_id]),
            });
        }
        Ok(())
    }
}

/// Columns of a securities position, in the order [`holding_from_row`] reads them.
pub(crate) const HOLDING_COLUMNS: &str =
    "h.isin, h.wkn, h.name, h.quantity, h.current_price_cents, h.purchase_price_cents, h.value_cents,
     h.prev_day_price_cents, h.prev_day_value_cents, h.profit_loss_purchase_cents, h.profit_loss_purchase_rel,
     h.profit_loss_prev_day_cents, h.profit_loss_prev_day_rel, h.custody_type";

/// Restricts `holdings h` to one active account, largest position first.
pub(crate) const HOLDING_FILTER: &str = "h.account_id = ?1
     AND EXISTS (SELECT 1 FROM accounts a WHERE a.id = h.account_id AND a.deleted_ms IS NULL)
     ORDER BY h.value_cents DESC, h.name, h.id";

pub(crate) fn holding_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Holding> {
    Ok(Holding {
        isin: r.get(0)?,
        wkn: r.get(1)?,
        name: r.get(2)?,
        quantity: r.get(3)?,
        current_price_cents: r.get(4)?,
        purchase_price_cents: r.get(5)?,
        value_cents: r.get(6)?,
        prev_day_price_cents: r.get(7)?,
        prev_day_value_cents: r.get(8)?,
        profit_loss_purchase_cents: r.get(9)?,
        profit_loss_purchase_rel: r.get(10)?,
        profit_loss_prev_day_cents: r.get(11)?,
        profit_loss_prev_day_rel: r.get(12)?,
        custody_type: r.get(13)?,
    })
}

pub(crate) fn holdings_from_db(
    db: &rusqlite::Connection,
    account_id: i64,
) -> Result<Vec<Holding>, CoreError> {
    // Sachwerte (#87): the items, valued today, in the same shape
    if crate::assets::is_tangible(db, account_id)? {
        return crate::assets::holdings(db, account_id);
    }
    let mut q = db.prepare(&format!(
        "SELECT {HOLDING_COLUMNS} FROM holdings h WHERE {HOLDING_FILTER}"
    ))?;
    let rows = q.query_map([account_id], holding_from_row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

#[cfg(test)]
mod kind_owner_tests {
    use super::*;

    #[test]
    fn set_kind_moves_account_to_kind_section_and_persists() {
        let dir = std::env::temp_dir().join(format!("gc-kind-move-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path: String = dir.join("v.db").to_string_lossy().into();
        let section_of = |v: &Vault, id: i64| {
            let groups = v.list_account_groups().unwrap();
            let hits: Vec<_> = groups
                .iter()
                .filter(|g| g.accounts.iter().any(|a| a.id == id))
                .map(|g| g.name.clone())
                .collect();
            assert_eq!(hits.len(), 1);
            hits[0].clone()
        };
        let v = Vault::open(path.clone(), vec![7u8; 32]).unwrap();
        let id = {
            let db = v.db().unwrap();
            let (g, s, o) = crate::connectors::group_for(AccountKind::Checking);
            let gid = Vault::ensure_group(&db, g, s, o).unwrap();
            db.execute(
                "INSERT INTO accounts(group_id, external_ref, name) VALUES (?1, 'X', 'Konto')",
                [gid],
            )
            .unwrap();
            db.last_insert_rowid()
        };
        let before = section_of(&v, id);
        v.set_account_kind(id, AccountKind::CreditCard).unwrap();
        let after = section_of(&v, id);
        assert_ne!(before, after);
        drop(v);
        let v = Vault::open(path, vec![7u8; 32]).unwrap();
        assert_eq!(section_of(&v, id), after);
        drop(v);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn set_kind_and_owner() {
        let dir = std::env::temp_dir().join(format!("gc-kind-owner-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let v = Vault::open(dir.join("v.db").to_string_lossy().into(), vec![7u8; 32]).unwrap();
        let id = {
            let db = v.db().unwrap();
            db.execute("INSERT INTO account_groups(name) VALUES ('G')", [])
                .unwrap();
            let g = db.last_insert_rowid();
            db.execute(
                "INSERT INTO accounts(group_id, external_ref, name) VALUES (?1, 'X', 'Konto')",
                [g],
            )
            .unwrap();
            db.last_insert_rowid()
        };
        v.set_account_kind(id, AccountKind::CreditCard).unwrap();
        v.set_account_owner(id, "  Erika Muster ".into()).unwrap();
        let a = v
            .list_accounts()
            .unwrap()
            .into_iter()
            .find(|a| a.id == id)
            .unwrap();
        assert_eq!(a.kind, AccountKind::CreditCard);
        assert_eq!(a.owner, "Erika Muster");
        v.set_account_owner(id, String::new()).unwrap();
        assert!(v.set_account_kind(id + 999, AccountKind::Cash).is_err());
        drop(v);
        let _ = std::fs::remove_dir_all(dir);
    }
}
