use std::sync::Arc;

use rusqlite::params;

use super::*;
use crate::error::CoreError;
use crate::models::{AccountKind, BookingFilter, BookingScope, ConnectorKind, SepaTransferRequest};
use crate::vault::Vault;

#[test]
fn errors_after_submit_leave_outcome_unknown() {
    use super::{error_leaves_outcome_unknown as unknown, CoreError};
    assert!(unknown(&CoreError::Network {
        message: "x".into()
    }));
    assert!(unknown(&CoreError::Protocol {
        message: "x".into()
    }));
    assert!(!unknown(&CoreError::Authentication {
        message: "x".into()
    }));
    assert!(!unknown(&CoreError::InvalidInput {
        message: "x".into()
    }));
}

#[test]
fn store_fetch_matches_depot_holdings_without_iban() {
    let p_dir = crate::test_support::TempDir::new("girocrab-depot");
    let p = p_dir.join("v.db");
    let v = Vault::open(p.to_string_lossy().to_string(), vec![3u8; 32]).unwrap();
    let conn = v
        .add_bank_connection(ConnectorKind::Comdirect, String::new(), "login".into())
        .unwrap();
    let mk = |r: &str| RemoteHolding {
        account_ref: r.into(),
        holding: crate::models::Holding {
            isin: "DE0001234567".into(),
            name: "X".into(),
            quantity: 2.0,
            value_cents: 100,
            ..Default::default()
        },
    };
    let fetch = |hold_ref: &str| FetchResult {
        accounts: vec![RemoteAccount {
            external_ref: "0012345678".into(),
            name: "Depot".into(),
            owner: String::new(),
            iban: String::new(),
            account_number: "0012345678".into(),
            currency: "EUR".into(),
            kind: AccountKind::Brokerage,
            balance_cents: 5000,
            balance_date: "2026-11-10".into(),
        }],
        holdings: vec![mk(hold_ref)],
        ..Default::default()
    };
    // differently formatted ref (no leading zeros) and an unknown ref (single-depot fallback)
    for r in ["12345678", "unknown"] {
        store_fetch(&v, ConnectorKind::Comdirect, conn, fetch(r)).unwrap();
        let acc = v.list_accounts().unwrap().pop().unwrap();
        assert_eq!(v.list_holdings(acc.id).unwrap().len(), 1, "ref {r}");
    }
}

#[test]
fn archived_psd2_account_is_not_reported_as_revoked() {
    // #65 (archive) + #66 (revoked marking): an account the user archived
    // is skipped by store_fetch, so it must not look "missing" from a
    // complete PSD2 sync.
    let p_dir = crate::test_support::TempDir::new("girocrab-arch-eb");
    let p = p_dir.join("v.db");
    let v = Vault::open(p.to_string_lossy().to_string(), vec![3u8; 32]).unwrap();
    let conn = v
        .add_bank_connection(ConnectorKind::EnableBanking, String::new(), "C24".into())
        .unwrap();
    let acc = |r: &str, name: &str| RemoteAccount {
        external_ref: r.into(),
        name: name.into(),
        owner: String::new(),
        iban: String::new(),
        account_number: String::new(),
        currency: "EUR".into(),
        kind: AccountKind::Checking,
        balance_cents: 100,
        balance_date: "2026-11-11".into(),
    };
    let both = || vec![acc("G", "C24 Smartkonto"), acc("P", "C24 Pocket")];
    store_fetch(
        &v,
        ConnectorKind::EnableBanking,
        conn,
        FetchResult {
            accounts: both(),
            accounts_complete: true,
            ..Default::default()
        },
    )
    .unwrap();
    let pocket = v
        .list_accounts()
        .unwrap()
        .into_iter()
        .find(|a| a.name == "C24 Pocket")
        .unwrap();
    v.set_account_archived(pocket.id, true).unwrap();
    // the bank still delivers both
    let s = store_fetch(
        &v,
        ConnectorKind::EnableBanking,
        conn,
        FetchResult {
            accounts: both(),
            accounts_complete: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(s.revoked_accounts.is_empty(), "{:?}", s.revoked_accounts);
    // the bank no longer delivers the archived one: still no notice
    let s = store_fetch(
        &v,
        ConnectorKind::EnableBanking,
        conn,
        FetchResult {
            accounts: vec![acc("G", "C24 Smartkonto")],
            accounts_complete: true,
            revoked_refs: vec!["P".into()],
            ..Default::default()
        },
    )
    .unwrap();
    assert!(s.revoked_accounts.is_empty(), "{:?}", s.revoked_accounts);
    let st = v.connection_status(conn).unwrap();
    assert_eq!(st.revoked_account_count, 0);
    std::fs::remove_file(p).ok();
}

#[test]
fn store_fetch_skips_archived_accounts() {
    let p_dir = crate::test_support::TempDir::new("girocrab-arch");
    let p = p_dir.join("v.db");
    let v = Vault::open(p.to_string_lossy().to_string(), vec![3u8; 32]).unwrap();
    let conn = v
        .add_bank_connection(ConnectorKind::Comdirect, String::new(), "login".into())
        .unwrap();
    let acc = |r: &str, name: &str, balance: i64| RemoteAccount {
        external_ref: r.into(),
        name: name.into(),
        owner: String::new(),
        iban: String::new(),
        account_number: r.into(),
        currency: "EUR".into(),
        kind: AccountKind::Checking,
        balance_cents: balance,
        balance_date: "2026-11-10".into(),
    };
    let tx = |r: &str, id: &str| RemoteTransaction {
        account_ref: r.into(),
        external_id: id.into(),
        booking_date: "2026-11-09".into(),
        value_date: "2026-11-09".into(),
        amount_cents: -500,
        currency: "EUR".into(),
        counterparty: "Laden".into(),
        counterparty_iban: String::new(),
        purpose: String::new(),
        booking_text: String::new(),
        ..Default::default()
    };
    let fetch = |balance: i64, ids: &[&str]| FetchResult {
        accounts: vec![
            acc("alt", "Altes Konto", balance),
            acc("neu", "Neues Konto", balance),
        ],
        transactions: ids
            .iter()
            .flat_map(|id| [tx("alt", id), tx("neu", id)])
            .collect(),
        ..Default::default()
    };
    let s = store_fetch(&v, ConnectorKind::Comdirect, conn, fetch(1000, &["t1"])).unwrap();
    assert_eq!(
        (s.transactions_imported, s.transactions_skipped_archived),
        (2, 0)
    );
    let old = v
        .list_accounts()
        .unwrap()
        .into_iter()
        .find(|a| a.name == "Altes Konto")
        .unwrap();
    v.set_account_archived(old.id, true).unwrap();
    assert!(!v.list_bank_connections().unwrap()[0].all_accounts_archived);

    // t1 is known, t2 is new: only the active account takes it
    let s = store_fetch(
        &v,
        ConnectorKind::Comdirect,
        conn,
        fetch(2000, &["t1", "t2"]),
    )
    .unwrap();
    assert_eq!(
        (
            s.accounts_updated,
            s.transactions_imported,
            s.transactions_skipped_archived
        ),
        (1, 1, 1)
    );
    let accs = v.list_accounts().unwrap();
    assert_eq!(accs.len(), 2, "no second account for the archived one");
    let by = |n: &str| accs.iter().find(|a| a.name == n).unwrap();
    assert_eq!(
        (
            by("Altes Konto").balance_cents,
            by("Altes Konto").transaction_count
        ),
        (1000, 1)
    );
    assert_eq!(
        (
            by("Neues Konto").balance_cents,
            by("Neues Konto").transaction_count
        ),
        (2000, 2)
    );

    v.set_account_archived(by("Neues Konto").id, true).unwrap();
    assert!(v.list_bank_connections().unwrap()[0].all_accounts_archived);
    let _ = std::fs::remove_file(p);
}

#[test]
fn card_open_balance_since_settlement() {
    let r = |d: &str, a: i64, t: &str| (d.to_string(), a, t.to_string());
    let rows = vec![
        r("2026-09-20", -5000, "Shop"),
        r("2026-10-05", 5000, "LASTSCHRIFT Kartenabrechnung"),
        r("2026-10-10", -1200, "Cafe"),
        r("2026-11-02", -3400, "Hotel"),
    ];
    assert_eq!(open_card_balance(&rows), Some(-4600));
    let no_settle = vec![
        r("2026-10-30", -100, "x"),
        r("2026-11-01", -200, "y"),
        r("2026-11-03", -300, "z"),
    ];
    assert_eq!(open_card_balance(&no_settle), Some(-600));
    let with_refund = vec![
        r("2026-10-20", -170224, "Shops"),
        r("2026-11-02", 33853, "Refund Merchant"),
        r("2026-11-05", -1000, "Cafe"),
    ];
    assert_eq!(open_card_balance(&with_refund), Some(-137371));
    assert_eq!(open_card_balance(&[]), None);
}

fn demo_vault() -> (Arc<Vault>, std::path::PathBuf) {
    let path_dir = crate::test_support::TempDir::new("girocrab-sync");
    let path = path_dir.join("v.db");
    let v = Vault::open(path.to_string_lossy().to_string(), vec![3u8; 32]).unwrap();
    v.seed_demo_data().unwrap();
    (v, path)
}

#[test]
fn card_holds_are_replaced_on_each_sync() {
    let (v, path) = demo_vault();
    let conn = v
        .add_bank_connection(ConnectorKind::Bunq, String::new(), "me@example.com".into())
        .unwrap();
    let acc = RemoteAccount {
        external_ref: "bunq:1".into(),
        name: "bunq".into(),
        owner: String::new(),
        iban: "NL00BUNQ0000000000".into(),
        account_number: String::new(),
        currency: "EUR".into(),
        kind: AccountKind::Checking,
        balance_cents: 0,
        balance_date: "2026-11-10".into(),
    };
    let hold = |id: &str, cents: i64| RemoteTransaction {
        account_ref: "bunq:1".into(),
        external_id: format!("pending-hold-bunq-{id}"),
        booking_date: "2026-11-09".into(),
        value_date: "2026-11-09".into(),
        amount_cents: cents,
        currency: "EUR".into(),
        counterparty: "Hotel".into(),
        counterparty_iban: String::new(),
        purpose: String::new(),
        booking_text: "Kartenzahlung (vorgemerkt)".into(),
        ..Default::default()
    };
    let fetch = |txs: Vec<RemoteTransaction>| FetchResult {
        accounts: vec![acc.clone()],
        transactions: txs,
        hold_prefix: Some("pending-hold-bunq-".into()),
        ..Default::default()
    };
    let holds = || -> Vec<String> {
        let db = v.db().unwrap();
        let mut q = db
            .prepare("SELECT external_id FROM transactions WHERE external_id LIKE 'pending-hold-%' AND deleted_ms IS NULL ORDER BY 1")
            .unwrap();
        q.query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    store_fetch(
        &v,
        ConnectorKind::Bunq,
        conn,
        fetch(vec![hold("1", -50_00), hold("2", -7_10)]),
    )
    .unwrap();
    assert_eq!(holds(), vec!["pending-hold-bunq-1", "pending-hold-bunq-2"]);
    // Hold 1 cleared (booked elsewhere), hold 2 still open.
    store_fetch(&v, ConnectorKind::Bunq, conn, fetch(vec![hold("2", -7_10)])).unwrap();
    assert_eq!(holds(), vec!["pending-hold-bunq-2"]);
    std::fs::remove_file(path).ok();
}

#[test]
fn revoked_accounts_are_marked_and_new_ones_named() {
    let (v, path) = demo_vault();
    let conn = v
        .add_bank_connection(ConnectorKind::EnableBanking, String::new(), "C24".into())
        .unwrap();
    let acc = |r: &str, name: &str| RemoteAccount {
        external_ref: r.into(),
        name: name.into(),
        owner: String::new(),
        iban: String::new(),
        account_number: String::new(),
        currency: "EUR".into(),
        kind: AccountKind::Checking,
        balance_cents: 100_00,
        balance_date: "2026-11-11".into(),
    };
    let revoked = || -> Vec<(String, bool)> {
        let db = v.db().unwrap();
        let mut q = db
            .prepare("SELECT external_ref, access_revoked FROM accounts WHERE connection_id = ?1 ORDER BY external_ref")
            .unwrap();
        q.query_map([conn], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    // first sync: nothing is "new"
    let s = store_fetch(
        &v,
        ConnectorKind::EnableBanking,
        conn,
        FetchResult {
            accounts: vec![
                acc("A", "Girokonto"),
                acc("B", "Pocket"),
                acc("C", "Gemeinschaftskonto"),
            ],
            ..Default::default()
        },
    )
    .unwrap();
    assert!(s.new_accounts.is_empty() && s.revoked_accounts.is_empty());
    // reconnect with only A: B and C marked, named once, bookings kept
    let s = store_fetch(
        &v,
        ConnectorKind::EnableBanking,
        conn,
        FetchResult {
            accounts: vec![acc("A", "Girokonto")],
            revoked_refs: vec!["B".into(), "C".into()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(s.revoked_accounts, vec!["Pocket", "Gemeinschaftskonto"]);
    assert_eq!(
        revoked(),
        vec![("A".into(), false), ("B".into(), true), ("C".into(), true)]
    );
    let st = v.connection_status(conn).unwrap();
    assert_eq!((st.account_count, st.revoked_account_count), (3, 2));
    // repeated report: no second notice
    let s = store_fetch(
        &v,
        ConnectorKind::EnableBanking,
        conn,
        FetchResult {
            accounts: vec![acc("A", "Girokonto")],
            revoked_refs: vec!["B".into()],
            ..Default::default()
        },
    )
    .unwrap();
    assert!(s.revoked_accounts.is_empty());
    // later consent includes B again plus a new account D
    let s = store_fetch(
        &v,
        ConnectorKind::EnableBanking,
        conn,
        FetchResult {
            accounts: vec![
                acc("A", "Girokonto"),
                acc("B", "Pocket"),
                acc("D", "GLS Giro"),
            ],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(s.new_accounts, vec!["GLS Giro"]);
    assert_eq!(
        revoked(),
        vec![
            ("A".into(), false),
            ("B".into(), false),
            ("C".into(), true),
            ("D".into(), false)
        ]
    );
    let accs = v.list_accounts().unwrap();
    assert!(accs
        .iter()
        .any(|a| a.name == "Gemeinschaftskonto" && a.access_revoked));
    std::fs::remove_file(path).ok();
}

#[test]
fn automatic_names_and_bank_name_are_updated_user_names_kept() {
    let (v, path) = demo_vault();
    let conn = v
        .add_bank_connection(ConnectorKind::EnableBanking, String::new(), "C24".into())
        .unwrap();
    let acc = |r: &str, name: &str, owner: &str| RemoteAccount {
        external_ref: r.into(),
        name: name.into(),
        owner: owner.into(),
        iban: String::new(),
        account_number: String::new(),
        currency: "EUR".into(),
        kind: AccountKind::Checking,
        balance_cents: 0,
        balance_date: "2026-11-11".into(),
    };
    // as stored by the earlier build: holder as name, generic bank name
    store_fetch(
        &v,
        ConnectorKind::EnableBanking,
        conn,
        FetchResult {
            accounts: vec![acc("G", "Jay Licina", ""), acc("S", "Jay Licina", "")],
            ..Default::default()
        },
    )
    .unwrap();
    let db_rename = |r: &str, n: &str| {
        v.db()
            .unwrap()
            .execute(
                "UPDATE accounts SET name = ?1 WHERE external_ref = ?2",
                params![n, r],
            )
            .unwrap();
    };
    db_rename("S", "Mein Sparkonto"); // the user renamed this one
    store_fetch(
        &v,
        ConnectorKind::EnableBanking,
        conn,
        FetchResult {
            accounts: vec![
                acc("G", "C24 Smartkonto", "Jay Licina"),
                acc("S", "C24 PocketZINS Smart", "Jay Licina"),
            ],
            bank_name: Some("C24".into()),
            name_updates: vec![
                ("G".into(), "Jay Licina".into(), "C24 Smartkonto".into()),
                (
                    "S".into(),
                    "Jay Licina".into(),
                    "C24 PocketZINS Smart".into(),
                ),
            ],
            ..Default::default()
        },
    )
    .unwrap();
    let rows: Vec<(String, String, String, String)> = {
        let db = v.db().unwrap();
        let mut q = db
            .prepare("SELECT external_ref, name, bank_name, owner FROM accounts WHERE connection_id = ?1 ORDER BY external_ref")
            .unwrap();
        q.query_map([conn], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert_eq!(
        rows,
        vec![
            (
                "G".into(),
                "C24 Smartkonto".into(),
                "C24".into(),
                "Jay Licina".into()
            ),
            (
                "S".into(),
                "Mein Sparkonto".into(),
                "C24".into(),
                "Jay Licina".into()
            ),
        ]
    );
    std::fs::remove_file(path).ok();
}

#[test]
fn complete_account_list_marks_the_missing_ones() {
    let (v, path) = demo_vault();
    let conn = v
        .add_bank_connection(ConnectorKind::EnableBanking, String::new(), "C24".into())
        .unwrap();
    let acc = |r: &str, name: &str| RemoteAccount {
        external_ref: r.into(),
        name: name.into(),
        owner: String::new(),
        iban: String::new(),
        account_number: String::new(),
        currency: "EUR".into(),
        kind: AccountKind::Checking,
        balance_cents: 0,
        balance_date: "2026-11-11".into(),
    };
    let three = || {
        vec![
            acc("DE23", "Girokonto"),
            acc("hs", "PocketZINS"),
            acc("DE48", "Gemeinschaftskonto"),
        ]
    };
    store_fetch(
        &v,
        ConnectorKind::EnableBanking,
        conn,
        FetchResult {
            accounts: three(),
            accounts_complete: true,
            ..Default::default()
        },
    )
    .unwrap();
    // a partial sync (e.g. bank's daily limit) marks nothing
    let s = store_fetch(
        &v,
        ConnectorKind::EnableBanking,
        conn,
        FetchResult {
            accounts: vec![acc("DE23", "Girokonto")],
            accounts_complete: false,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(s.revoked_accounts.is_empty());
    // an empty "complete" list is never trusted
    let s = store_fetch(
        &v,
        ConnectorKind::EnableBanking,
        conn,
        FetchResult {
            accounts_complete: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(s.revoked_accounts.is_empty());
    // the real case: new consent with only the main account
    let s = store_fetch(
        &v,
        ConnectorKind::EnableBanking,
        conn,
        FetchResult {
            accounts: vec![acc("DE23", "Girokonto")],
            accounts_complete: true,
            ..Default::default()
        },
    )
    .unwrap();
    let mut names = s.revoked_accounts.clone();
    names.sort();
    assert_eq!(names, vec!["Gemeinschaftskonto", "PocketZINS"]);
    assert_eq!(v.connection_status(conn).unwrap().revoked_account_count, 2);
    // next sync: no repeated notice
    let s = store_fetch(
        &v,
        ConnectorKind::EnableBanking,
        conn,
        FetchResult {
            accounts: vec![acc("DE23", "Girokonto")],
            accounts_complete: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(s.revoked_accounts.is_empty());
    // all three granted again: marks cleared
    store_fetch(
        &v,
        ConnectorKind::EnableBanking,
        conn,
        FetchResult {
            accounts: three(),
            accounts_complete: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(v.connection_status(conn).unwrap().revoked_account_count, 0);
    std::fs::remove_file(path).ok();
}

#[test]
fn renamed_refs_keep_the_account() {
    let (v, path) = demo_vault();
    let conn = v
        .add_bank_connection(ConnectorKind::EnableBanking, String::new(), "C24".into())
        .unwrap();
    let acc = |r: &str| RemoteAccount {
        external_ref: r.into(),
        name: "Girokonto".into(),
        owner: String::new(),
        iban: "DE00100000000000000001".into(),
        account_number: String::new(),
        currency: "EUR".into(),
        kind: AccountKind::Checking,
        balance_cents: 100_00,
        balance_date: "2026-11-10".into(),
    };
    let count = || -> i64 {
        v.db()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM accounts WHERE connection_id = ?1",
                [conn],
                |r| r.get(0),
            )
            .unwrap()
    };
    let uid = "3fa85f64-5717-4562-b3fc-2c963f66afa6";
    let stable = "DE00100000000000000001";
    store_fetch(
        &v,
        ConnectorKind::EnableBanking,
        conn,
        FetchResult {
            accounts: vec![acc(uid)],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(count(), 1);
    store_fetch(
        &v,
        ConnectorKind::EnableBanking,
        conn,
        FetchResult {
            accounts: vec![acc(stable)],
            renamed_refs: vec![(uid.into(), stable.into())],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(count(), 1, "rename must not create a second account");
    // applying the same rename again is harmless
    store_fetch(
        &v,
        ConnectorKind::EnableBanking,
        conn,
        FetchResult {
            accounts: vec![acc(stable)],
            renamed_refs: vec![(uid.into(), stable.into())],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(count(), 1);
    // a later sync recognises it as savings and learns its BBAN
    let mut savings = acc(stable);
    savings.kind = AccountKind::Savings;
    savings.account_number = "500240240000000099".into();
    store_fetch(
        &v,
        ConnectorKind::EnableBanking,
        conn,
        FetchResult {
            accounts: vec![savings],
            ..Default::default()
        },
    )
    .unwrap();
    let (kind, number): (String, String) = v
        .db()
        .unwrap()
        .query_row(
            "SELECT kind, IFNULL(account_number, '') FROM accounts WHERE connection_id = ?1",
            [conn],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        (kind.as_str(), number.as_str()),
        ("savings", "500240240000000099")
    );
    assert_eq!(count(), 1);
    std::fs::remove_file(path).ok();
}

#[tokio::test]
async fn magic_demo_login_syncs_without_network_or_product_id() {
    let dir = std::env::temp_dir().join(format!("gc-magic-demo-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("v.girocrab");
    let v = Vault::open(path.to_str().unwrap().into(), vec![9u8; 32]).unwrap();
    // Real (non-demo) flag, no FinTS product ID set: only the login "Demo" makes it work.
    let id = v
        .add_fints_connection(
            "99999999".into(),
            "https://demo.invalid/fints".into(),
            "Demo-Bank".into(),
            " Demo ".into(),
            String::new(),
        )
        .unwrap();
    let s = v.clone().begin_sync(id, "12345".into(), false).unwrap();
    assert!(matches!(
        s.start().await.unwrap(),
        SyncStep::NeedsChallenge { .. }
    ));
    let _ = s.poll().await.unwrap();
    match s.poll().await.unwrap() {
        SyncStep::Finished { .. } => {}
        other => panic!("expected finish, got {other:?}"),
    }
    assert!(v
        .list_accounts()
        .unwrap()
        .iter()
        .any(|a| a.connection_id == Some(id)));
    // Any other login still needs the product ID.
    let other = v
        .add_fints_connection(
            "88888888".into(),
            "https://x.example/fints".into(),
            "X".into(),
            "someone".into(),
            String::new(),
        )
        .unwrap();
    // (Only without an embedded GiroCrab registration: release builds carry one.)
    if !fints::product_id::builtin_available() {
        assert!(matches!(
            v.clone().begin_sync(other, "1".into(), false),
            Err(CoreError::MissingSetting { .. })
        ));
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn demo_push_tan_flow_imports_and_categorises() {
    let (v, path) = demo_vault();
    let conn = v
        .list_bank_connections()
        .unwrap()
        .into_iter()
        .find(|c| c.connector == ConnectorKind::VolksbankKasselGoettingen)
        .unwrap();
    let before = v
        .list_accounts()
        .unwrap()
        .into_iter()
        .find(|a| a.name == "Musterbank Girokonto" && a.connection_id == Some(conn.id))
        .unwrap();
    let s = v.clone().begin_sync(conn.id, "pin".into(), true).unwrap();
    match s.start().await.unwrap() {
        SyncStep::NeedsChallenge { challenge } => {
            assert_eq!(challenge.kind, ChallengeKind::DecoupledPush)
        }
        other => panic!("expected push challenge, got {other:?}"),
    }
    assert!(matches!(
        s.poll().await.unwrap(),
        SyncStep::StillWaiting { .. }
    ));
    match s.poll().await.unwrap() {
        SyncStep::Finished { summary } => {
            assert_eq!(summary.transactions_imported, 1);
            assert_eq!(
                summary.transactions_categorized, 1,
                "REWE booking is categorised by a rule"
            );
        }
        other => panic!("expected finish, got {other:?}"),
    }
    let after = v
        .list_accounts()
        .unwrap()
        .into_iter()
        .find(|a| a.id == before.id)
        .unwrap();
    assert_eq!(after.balance_cents, before.balance_cents - 27_65);
    assert!(matches!(s.poll().await, Err(CoreError::SessionClosed)));
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn demo_code_and_redirect_flows() {
    let (v, path) = demo_vault();
    let conns = v.list_bank_connections().unwrap();
    let tr = conns
        .iter()
        .find(|c| c.connector == ConnectorKind::TradeRepublic)
        .unwrap();
    let s = v.clone().begin_sync(tr.id, "1234".into(), true).unwrap();
    assert!(
        matches!(s.start().await.unwrap(), SyncStep::NeedsChallenge { challenge } if challenge.kind == ChallengeKind::Code)
    );
    assert!(
        s.submit_code("12".into()).await.is_err(),
        "bad code rejected"
    );
    let amex = conns
        .iter()
        .find(|c| c.connector == ConnectorKind::AmericanExpress)
        .unwrap();
    let s = v.clone().begin_sync(amex.id, String::new(), true).unwrap();
    assert!(
        matches!(s.start().await.unwrap(), SyncStep::NeedsChallenge { challenge } if challenge.kind == ChallengeKind::Redirect)
    );
    assert!(matches!(
        s.submit_code("https://localhost/cb?code=x".into())
            .await
            .unwrap(),
        SyncStep::Finished { .. }
    ));
    let pp_id = v
        .add_bank_connection(
            ConnectorKind::PayPal,
            String::new(),
            "demo@example.org".into(),
        )
        .unwrap();
    let pp = v
        .list_bank_connections()
        .unwrap()
        .into_iter()
        .find(|c| c.id == pp_id)
        .unwrap();
    let s = v.clone().begin_sync(pp.id, String::new(), true).unwrap();
    assert!(matches!(
        s.start().await.unwrap(),
        SyncStep::Finished { .. }
    ));
    // Real (non-demo) sync refuses to start without the user's own keys.
    assert!(matches!(
        v.clone().begin_sync(pp.id, String::new(), false),
        Err(CoreError::MissingSetting { .. })
    ));
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn demo_transfer_flow_adds_pending_booking_and_suggests_recipient() {
    let (v, path) = demo_vault();
    let acc = v
        .list_accounts()
        .unwrap()
        .into_iter()
        .find(|a| a.name == "Musterbank Girokonto" && a.connection_id.is_some())
        .unwrap();
    let req = |iban: &str, amount: i64| SepaTransferRequest {
        account_id: acc.id,
        recipient_name: "Müller GmbH".into(),
        recipient_iban: iban.into(),
        recipient_bic: None,
        amount_cents: amount,
        purpose: "Rechnung 42".into(),
        instant: true,
        execution_date: None,
    };
    assert!(v
        .clone()
        .begin_transfer(req("DE89370400440532013001", 100), "pin".into(), true)
        .is_err());
    assert!(v
        .clone()
        .begin_transfer(req("DE89370400440532013000", 0), "pin".into(), true)
        .is_err());
    let s = v
        .clone()
        .begin_transfer(req("DE89 3704 0044 0532 0130 00", 1250), "pin".into(), true)
        .unwrap();
    let mut step = s.start().await.unwrap();
    let mut challenges = 0;
    while matches!(
        step,
        TransferStep::NeedsChallenge { .. } | TransferStep::StillWaiting { .. }
    ) {
        challenges += 1;
        step = s.poll().await.unwrap();
    }
    assert!(challenges >= 2, "login + transfer challenge");
    let TransferStep::Finished { outcome } = &step else {
        panic!("{step:?}")
    };
    assert!(outcome.instant);
    assert!(outcome.end_to_end_id.starts_with("GC") && outcome.end_to_end_id.len() == 32);
    let page = v
        .list_transactions(BookingFilter {
            account_ids: vec![acc.id],
            category_id: None,
            include_subcategories: false,
            uncategorized_only: false,
            search: Some("Müller GmbH".into()),
            date_from: None,
            date_to: None,
            limit: 10,
            offset: 0,
            scope: BookingScope::All,
            merchant_key: None,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(page.transactions.len(), 1);
    assert_eq!(page.transactions[0].amount_cents, -1250);
    assert_eq!(
        page.transactions[0].counterparty_iban,
        "DE89370400440532013000"
    );
    let sug = v.suggest_recipients("müller".into(), 5).unwrap();
    assert_eq!(sug.len(), 1);
    assert_eq!(sug[0].iban, "DE89370400440532013000");
    assert!(matches!(s.poll().await, Err(CoreError::SessionClosed)));

    // Runs a transfer to the end, approving every challenge.
    async fn run(s: &TransferSession) -> (TransferStep, Vec<String>) {
        let mut step = s.start().await.unwrap();
        let mut titles = Vec::new();
        while let TransferStep::NeedsChallenge { challenge }
        | TransferStep::StillWaiting { challenge } = &step
        {
            titles.push(challenge.title.clone());
            step = s.poll().await.unwrap();
        }
        (step, titles)
    }
    let bookings = |name: &str| {
        v.list_transactions(BookingFilter {
            account_ids: vec![acc.id],
            category_id: None,
            include_subcategories: false,
            uncategorized_only: false,
            search: Some(name.into()),
            date_from: None,
            date_to: None,
            limit: 10,
            offset: 0,
            scope: BookingScope::All,
            merchant_key: None,
            ..Default::default()
        })
        .unwrap()
        .transactions
    };

    // Bank does not offer instant: declined, nothing sent, no booking,
    // and no automatic regular transfer.
    let mut slow = req("DE89370400440532013000", 990);
    slow.recipient_name = "Langsam AG".into();
    let s = v.clone().begin_transfer(slow, "pin".into(), true).unwrap();
    let (step, titles) = run(&s).await;
    assert!(
        matches!(step, TransferStep::InstantDeclined { .. }),
        "{step:?}"
    );
    assert!(
        !titles.iter().any(|t| t.contains("Überweisung")),
        "only the login, no transfer approval: {titles:?}"
    );
    assert!(bookings("Langsam AG").is_empty());
    assert!(matches!(s.poll().await, Err(CoreError::SessionClosed)));

    // Ambiguous reply after the approval: unknown, booking marked as
    // unclear, session over (nothing resent).
    let mut unclear = req("DE89370400440532013000", 770);
    unclear.recipient_name = "Unklar KG".into();
    let s = v
        .clone()
        .begin_transfer(unclear, "pin".into(), true)
        .unwrap();
    let (step, _) = run(&s).await;
    let TransferStep::OutcomeUnknown { end_to_end_id, .. } = &step else {
        panic!("{step:?}")
    };
    assert!(end_to_end_id.starts_with("GC"));
    let b = bookings("Unklar KG");
    assert_eq!(b.len(), 1);
    assert!(
        b[0].booking_text.contains("Status unklar"),
        "{}",
        b[0].booking_text
    );
    assert!(matches!(s.poll().await, Err(CoreError::SessionClosed)));

    // Connection lost after the approval: unknown as well, not an error.
    let mut lost = req("DE89370400440532013000", 660);
    lost.recipient_name = "Netzfehler GmbH".into();
    let s = v.clone().begin_transfer(lost, "pin".into(), true).unwrap();
    let (step, _) = run(&s).await;
    assert!(
        matches!(step, TransferStep::OutcomeUnknown { .. }),
        "{step:?}"
    );
    assert_eq!(bookings("Netzfehler GmbH").len(), 1);

    // Regular transfer: never a second order either.
    let mut regular = req("DE89370400440532013000", 550);
    regular.instant = false;
    regular.recipient_name = "Regulär OHG".into();
    let s = v
        .clone()
        .begin_transfer(regular, "pin".into(), true)
        .unwrap();
    let (step, _) = run(&s).await;
    let TransferStep::Finished { outcome } = &step else {
        panic!("{step:?}")
    };
    assert!(!outcome.instant);

    // Name check: close match needs confirmation, then the approval.
    let mut close = req("DE89370400440532013000", 500);
    close.recipient_name = "Fast Mustermann".into();
    let s = v.clone().begin_transfer(close, "pin".into(), true).unwrap();
    let mut step = s.start().await.unwrap();
    while matches!(
        step,
        TransferStep::NeedsChallenge { .. } | TransferStep::StillWaiting { .. }
    ) {
        step = s.poll().await.unwrap();
    }
    let TransferStep::NeedsPayeeConfirmation { check } = step else {
        panic!("{step:?}")
    };
    assert_eq!(check.result, PayeeMatch::CloseMatch);
    assert_eq!(check.bank_name.as_deref(), Some("Erika Mustermann"));
    let mut step = s.confirm_payee().await.unwrap();
    while matches!(
        step,
        TransferStep::NeedsChallenge { .. } | TransferStep::StillWaiting { .. }
    ) {
        step = s.poll().await.unwrap();
    }
    assert!(matches!(step, TransferStep::Finished { .. }));

    // Cancelling at the name check sends nothing.
    let mut wrong = req("DE89370400440532013000", 700);
    wrong.recipient_name = "Falsch GmbH".into();
    let s = v.clone().begin_transfer(wrong, "pin".into(), true).unwrap();
    let mut step = s.start().await.unwrap();
    while matches!(
        step,
        TransferStep::NeedsChallenge { .. } | TransferStep::StillWaiting { .. }
    ) {
        step = s.poll().await.unwrap();
    }
    assert!(matches!(
        step,
        TransferStep::NeedsPayeeConfirmation {
            check: PayeeCheck {
                result: PayeeMatch::NoMatch,
                ..
            }
        }
    ));
    s.cancel().await;
    assert!(matches!(
        s.confirm_payee().await,
        Err(CoreError::SessionClosed)
    ));
    let _ = std::fs::remove_file(path);
}

mod account_selection;
mod bank_name;
mod banking_gate;
mod content_id;
mod statements_auto;
mod transfer_cancel;
