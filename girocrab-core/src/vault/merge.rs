//! Merging another vault file (iCloud peer snapshot or a second vault) into this one.

use std::collections::HashMap;

use rusqlite::{params, Connection};
use zeroize::Zeroizing;

use super::Vault;
use crate::error::{CoreError, CoreResult};

impl Vault {
    // includes soft-deleted rows: the merge reads and matches every row of
    // both vaults, deleted ones too
    /// `legacy`: the other vault was written by a v7 build (no uuids).
    fn merge_attached(conn: &Connection, legacy: bool) -> CoreResult<MergeStats> {
        let mut st = MergeStats::default();
        let tx = conn.unchecked_transaction()?;

        // account groups (by name)
        let mut group_map: HashMap<i64, i64> = HashMap::new();
        {
            let rows: Vec<(i64, String, String, i64)> = tx
                .prepare("SELECT id, name, sf_symbol, sort_order FROM other.account_groups")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect::<Result<_, _>>()?;
            for (oid, name, sym, ord) in rows {
                tx.execute(
                    "INSERT OR IGNORE INTO account_groups(name, sf_symbol, sort_order) VALUES (?1,?2,?3)",
                    params![name, sym, ord],
                )?;
                let id: i64 = tx.query_row(
                    "SELECT id FROM account_groups WHERE name = ?1",
                    [&name],
                    |r| r.get(0),
                )?;
                group_map.insert(oid, id);
            }
        }

        // bank connections (by sync key) and accounts (by connection +
        // external_ref): newest creation or deletion wins, deletions cascade
        let conn_map = crate::sync_merge::merge_connections(&tx)?;
        let (acc_map, acc_inserted) =
            crate::sync_merge::merge_accounts(&tx, &conn_map, &group_map, &mut st)?;
        // user groups, account placements and names (newest edit wins)
        st.groups_updated = crate::user_groups::merge_attached(&tx, &acc_map, &acc_inserted)?;

        // categories and rules (newest edit wins, deletions stay deleted)
        let cat_map = crate::sync_merge::merge_categories(&tx, legacy, &mut st)?;
        let rule_map = crate::sync_merge::merge_rules(&tx, &cat_map, &acc_map, legacy, &mut st)?;
        // bookings only point at live categories and rules
        let cat_map = crate::sync_merge::live_targets(&tx, "categories", &cat_map)?;
        let rule_map = crate::sync_merge::live_targets(&tx, "rules", &rule_map)?;

        // tags (by uuid, newest edit wins), before the bookings that carry them
        crate::tags::merge::merge_attached(&tx)?;
        // bookings, splits and tag sets (newest edit per field wins, deletions stay)
        crate::sync_merge::merge_bookings(&tx, &acc_map, &cat_map, &rule_map, &mut st)?;
        crate::loans::merge_attached(&tx, &acc_map)?;
        crate::opening_balance::merge_attached(&tx, &acc_map)?;
        crate::assets::merge_attached(&tx, &acc_map)?;
        crate::security_transactions::merge_attached(&tx, &acc_map)?;
        crate::depot_values::merge_attached(&tx, &acc_map)?;
        crate::planned::merge_attached(&tx, &acc_map)?;
        crate::asset_classification::store::merge_attached(&tx)?;
        crate::mandates::merge_attached(&tx, &acc_map)?;
        crate::mandates::merge_ack(&tx, &acc_map)?;
        tx.commit()?;
        Ok(st)
    }
}

/// Result of [`Vault::merge_other_vault`].
#[derive(Debug, Clone, Default, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MergeStats {
    pub accounts_added: u32,
    pub categories_added: u32,
    pub rules_added: u32,
    pub bookings_added: u32,
    pub bookings_updated: u32,
    /// User groups and account placements taken over from the other vault.
    pub groups_updated: u32,
    /// Rows this merge changed in the vault (part of [`Vault::change_count`]),
    /// so the app can tell its own changes from merged ones.
    pub changes: u64,
}

#[uniffi::export]
impl Vault {
    /// Merges another device's sync snapshot using this vault's own key
    /// (see [`merge_other_vault`](Self::merge_other_vault)).
    pub fn merge_peer_snapshot(&self, other_path: String) -> Result<MergeStats, CoreError> {
        let key = self.kept_key()?.to_vec();
        self.merge_other_vault(other_path, key)
    }

    /// Merges another vault file (e.g. a copy synced via iCloud) into this one.
    /// Categories and rules: newest edit wins, deletions stay deleted, same
    /// ones created on both sides are combined. Missing connections, accounts
    /// and bookings are added; existing bookings (matched by account +
    /// external id, else import fingerprint) take the newest edit per field
    /// (category, note, checked, text, splits, tags) and stay deleted once deleted.
    pub fn merge_other_vault(
        &self,
        other_path: String,
        key: Vec<u8>,
    ) -> Result<MergeStats, CoreError> {
        let key = Zeroizing::new(key);
        if other_path == self.path {
            return Err(CoreError::InvalidInput {
                message: "cannot merge a vault into itself".into(),
            });
        }
        // Validate key and bring the other file up to the current schema;
        // a file from a v7 build has no uuids before that.
        let legacy = {
            let mut o = Connection::open(&other_path)?;
            Self::apply_key(&o, &key)?;
            let has_uuid: bool = o
                .query_row(
                    "SELECT count(*) > 0 FROM pragma_table_info('categories') WHERE name = 'uuid'",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(true);
            Self::migrate(&mut o)?;
            crate::sync_merge::fill_missing_uuids(&o)?;
            !has_uuid
        };
        let conn = self.db()?;
        let before = conn.total_changes();
        let raw_key = Zeroizing::new(format!("x'{}'", hex::encode(&*key)));
        conn.execute(
            "ATTACH DATABASE ?1 AS other KEY ?2",
            params![other_path, &*raw_key],
        )?;
        let res = Self::merge_attached(&conn, legacy);
        let detach_res = conn.execute_batch("DETACH DATABASE other");
        // MoneyMoney twins the merge brought in from a device that did not
        // fetch (best effort, like on open: the merge itself is committed)
        let res = res.and_then(|st| {
            detach_res?;
            let _ = crate::sync_merge::twins::fold_twins(&conn);
            Ok(st)
        });
        // Fold WAL frames into the main file so a plain file copy is complete.
        let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
        let mut stats = res?;
        stats.changes = conn.total_changes() - before;
        Ok(stats)
    }
}

#[cfg(test)]
mod merge_tests {
    use super::*;
    use crate::schema::SCHEMA_VERSION;

    fn seed(v: &Vault, checked: bool, note: &str, extra: bool) {
        let c = v.db().unwrap();
        c.execute(
            "INSERT OR IGNORE INTO account_groups(name) VALUES ('G')",
            [],
        )
        .unwrap();
        c.execute(
            "INSERT OR IGNORE INTO accounts(group_id, external_ref, name) \
             VALUES ((SELECT id FROM account_groups WHERE name='G'), 'A1', 'Konto')",
            [],
        )
        .unwrap();
        c.execute(
            "INSERT INTO transactions(account_id, external_id, booking_date, amount_cents, checked, note) \
             VALUES ((SELECT id FROM accounts LIMIT 1), 'T1', '2024-01-01', -100, ?1, ?2)",
            params![checked, note],
        )
        .unwrap();
        if extra {
            c.execute("INSERT INTO categories(name) VALUES ('Food')", [])
                .unwrap();
            c.execute(
                "INSERT INTO transactions(account_id, external_id, booking_date, amount_cents) \
                 VALUES ((SELECT id FROM accounts LIMIT 1), 'T2', '2024-01-02', -5)",
                [],
            )
            .unwrap();
        }
    }

    #[test]
    fn merges_without_duplicates() {
        let dir = std::env::temp_dir().join(format!("gc-merge-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let key = vec![7u8; 32];
        let a = Vault::open(dir.join("a.db").to_string_lossy().into(), key.clone()).unwrap();
        let b = Vault::open(dir.join("b.db").to_string_lossy().into(), key.clone()).unwrap();
        seed(&a, false, "", false);
        seed(&b, true, "hello", true);
        let st = a.merge_other_vault(b.file_path(), key.clone()).unwrap();
        assert_eq!(st.bookings_added, 1);
        assert_eq!(st.bookings_updated, 1);
        assert_eq!(st.categories_added, 1);
        assert_eq!(st.accounts_added, 0);
        let (n, checked, note): (i64, bool, String) = a
            .db()
            .unwrap()
            .query_row(
                "SELECT (SELECT count(*) FROM transactions), checked, note FROM transactions WHERE external_id='T1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((n, checked, note.as_str()), (2, true, "hello"));
        // idempotent
        let st2 = a.merge_other_vault(b.file_path(), key).unwrap();
        assert_eq!((st2.bookings_added, st2.bookings_updated), (0, 0));
        let _ = std::fs::remove_dir_all(dir);
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("gc-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn path_in(dir: &std::path::Path, name: &str) -> String {
        dir.join(name).to_string_lossy().into()
    }

    fn count(v: &Vault, sql: &str) -> i64 {
        v.db().unwrap().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn merge_peer_snapshot_uses_own_key() {
        let dir = temp_dir("peer");
        let a = Vault::open(path_in(&dir, "a.db"), vec![7u8; 32]).unwrap();
        let b = Vault::open(path_in(&dir, "b.db"), vec![7u8; 32]).unwrap();
        seed(&b, false, "", true);
        let st = a.merge_peer_snapshot(b.file_path()).unwrap();
        assert!(st.bookings_added >= 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn merge_peer_snapshot_rejects_other_key() {
        let dir = temp_dir("peer-key");
        let a = Vault::open(path_in(&dir, "a.db"), vec![7u8; 32]).unwrap();
        let b = Vault::open(path_in(&dir, "b.db"), vec![8u8; 32]).unwrap();
        seed(&b, false, "", true);
        b.checkpoint().unwrap();
        assert!(matches!(
            a.merge_peer_snapshot(b.file_path()),
            Err(CoreError::WrongKey)
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn write_snapshot_round_trips() {
        let dir = temp_dir("snap");
        let a = Vault::open(path_in(&dir, "a.db"), vec![7u8; 32]).unwrap();
        seed(&a, true, "note", true);
        let snap = path_in(&dir, "snap.girocrab");
        a.write_snapshot(snap.clone()).unwrap();
        // a second write replaces the first
        a.write_snapshot(snap.clone()).unwrap();
        let s = Vault::open(snap, vec![7u8; 32]).unwrap();
        let q = "SELECT count(*) FROM transactions";
        assert_eq!(count(&s, q), count(&a, q));
        assert_eq!(count(&s, "PRAGMA user_version"), SCHEMA_VERSION);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn write_snapshot_is_consistent_during_writes() {
        let dir = temp_dir("snap-busy");
        let a = Vault::open(path_in(&dir, "a.db"), vec![7u8; 32]).unwrap();
        seed(&a, false, "", false);
        let writer = {
            let a = a.clone();
            std::thread::spawn(move || {
                for i in 0..500 {
                    a.db()
                        .unwrap()
                        .execute(
                            "INSERT INTO transactions(account_id, external_id, booking_date, amount_cents) \
                             VALUES ((SELECT id FROM accounts LIMIT 1), ?1, '2024-02-01', -1)",
                            params![format!("W{i}")],
                        )
                        .unwrap();
                }
            })
        };
        for i in 0..5 {
            let snap = path_in(&dir, &format!("snap{i}.girocrab"));
            a.write_snapshot(snap.clone()).unwrap();
            let s = Vault::open(snap, vec![7u8; 32]).unwrap();
            let ok: String = s
                .db()
                .unwrap()
                .query_row("PRAGMA integrity_check", [], |r| r.get(0))
                .unwrap();
            assert_eq!(ok, "ok");
        }
        writer.join().unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn change_count_moves_on_writes_only() {
        let dir = temp_dir("changes");
        let a = Vault::open(path_in(&dir, "a.db"), vec![7u8; 32]).unwrap();
        let before = a.change_count();
        count(&a, "SELECT count(*) FROM transactions");
        assert_eq!(a.change_count(), before);
        seed(&a, false, "", false);
        assert!(a.change_count() > before);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn merge_reports_the_rows_it_changed() {
        let dir = temp_dir("merge-changes");
        let a = Vault::open(path_in(&dir, "a.db"), vec![7u8; 32]).unwrap();
        let b = Vault::open(path_in(&dir, "b.db"), vec![7u8; 32]).unwrap();
        seed(&b, true, "hello", true);
        let before = a.change_count();
        let st = a.merge_peer_snapshot(b.file_path()).unwrap();
        assert!(st.changes > 0);
        assert_eq!(a.change_count() - before, st.changes);
        let again = a.merge_peer_snapshot(b.file_path()).unwrap();
        assert_eq!(again.changes, 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rekey_updates_kept_key() {
        let dir = temp_dir("rekey");
        let a = Vault::open(path_in(&dir, "a.db"), vec![7u8; 32]).unwrap();
        seed(&a, false, "", false);
        a.rekey(vec![9u8; 32]).unwrap();
        let snap = path_in(&dir, "snap.girocrab");
        a.write_snapshot(snap.clone()).unwrap();
        assert!(Vault::open(snap.clone(), vec![7u8; 32]).is_err());
        assert!(Vault::open(snap, vec![9u8; 32]).is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }
}
