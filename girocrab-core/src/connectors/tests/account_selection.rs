//! Same account reachable through two logins (#346): detection, skip, replay.
use crate::connectors::*;
use crate::models::{AccountKind, ConnectorKind};
use crate::vault::Vault;

fn acc(r: &str, iban: &str, number: &str) -> RemoteAccount {
    RemoteAccount {
        external_ref: r.into(),
        name: format!("Konto {r}"),
        owner: String::new(),
        iban: iban.into(),
        account_number: number.into(),
        currency: "EUR".into(),
        kind: AccountKind::Checking,
        balance_cents: 100,
        balance_date: "2026-11-10".into(),
    }
}

fn fetch(accounts: Vec<RemoteAccount>) -> FetchResult {
    FetchResult {
        accounts,
        bank_name: Some("Testbank".into()),
        ..Default::default()
    }
}

fn open(name: &str) -> (crate::test_support::TempDir, std::sync::Arc<Vault>) {
    let dir = crate::test_support::TempDir::new(name);
    let v = Vault::open(
        dir.join("v.db").to_string_lossy().to_string(),
        vec![7u8; 32],
    )
    .unwrap();
    (dir, v)
}

fn login(v: &Vault, connector: &str, name: &str) -> i64 {
    let db = v.db().unwrap();
    db.execute(
        "INSERT INTO bank_connections(connector, display_name, login_name) VALUES (?1, ?2, ?2)",
        rusqlite::params![connector, name],
    )
    .unwrap();
    db.last_insert_rowid()
}

fn count(v: &Vault) -> usize {
    v.list_accounts().unwrap().len()
}

#[test]
fn second_login_defers_the_shared_child_account() {
    let (_d, v) = open("girocrab-accsel-1");
    let a = login(&v, "fnz", "Mutter");
    let b = login(&v, "fnz", "Vater");
    let own_a = acc("A-own", "", "111");
    let child = acc("KIND", "", "999");
    store_fetch(
        &v,
        ConnectorKind::Fnz,
        a,
        fetch(vec![own_a.clone(), child.clone()]),
    )
    .unwrap();
    assert_eq!(count(&v), 2);
    // nothing to choose on the first login
    assert!(v
        .pending_account_selection_connections()
        .unwrap()
        .is_empty());

    store_fetch(
        &v,
        ConnectorKind::Fnz,
        b,
        fetch(vec![acc("B-own", "", "222"), child]),
    )
    .unwrap();
    // own account imported, shared one held back until the user decides
    assert_eq!(count(&v), 3);
    assert_eq!(v.pending_account_selection_connections().unwrap(), vec![b]);
    let choices = v.connection_account_choices(b).unwrap();
    let kid = choices.iter().find(|c| c.account_ref == "KIND").unwrap();
    assert!(kid.pending && !kid.enabled);
    assert_eq!(kid.existing_connection, "Mutter");
    // a later sync keeps holding it back
    store_fetch(
        &v,
        ConnectorKind::Fnz,
        b,
        fetch(vec![acc("B-own", "", "222"), acc("KIND", "", "999")]),
    )
    .unwrap();
    assert_eq!(count(&v), 3);
}

#[test]
fn choosing_fetch_imports_the_held_back_account() {
    let (_d, v) = open("girocrab-accsel-2");
    let a = login(&v, "fnz", "Mutter");
    let b = login(&v, "fnz", "Vater");
    store_fetch(
        &v,
        ConnectorKind::Fnz,
        a,
        fetch(vec![acc("KIND", "", "999")]),
    )
    .unwrap();
    store_fetch(
        &v,
        ConnectorKind::Fnz,
        b,
        fetch(vec![acc("KIND", "", "999")]),
    )
    .unwrap();
    assert_eq!(count(&v), 1);
    v.clone()
        .set_connection_account_choices(b, vec!["KIND".into()])
        .unwrap();
    assert_eq!(count(&v), 2);
    assert!(v
        .pending_account_selection_connections()
        .unwrap()
        .is_empty());
}

#[test]
fn choosing_skip_keeps_it_out_permanently() {
    let (_d, v) = open("girocrab-accsel-3");
    let a = login(&v, "fnz", "Mutter");
    let b = login(&v, "fnz", "Vater");
    store_fetch(
        &v,
        ConnectorKind::Fnz,
        a,
        fetch(vec![acc("KIND", "", "999")]),
    )
    .unwrap();
    store_fetch(
        &v,
        ConnectorKind::Fnz,
        b,
        fetch(vec![acc("KIND", "", "999")]),
    )
    .unwrap();
    v.clone().set_connection_account_choices(b, vec![]).unwrap();
    assert!(v
        .pending_account_selection_connections()
        .unwrap()
        .is_empty());
    store_fetch(
        &v,
        ConnectorKind::Fnz,
        b,
        fetch(vec![acc("KIND", "", "999")]),
    )
    .unwrap();
    assert_eq!(count(&v), 1);
    // still listed (and re-enableable) in the settings
    let choices = v.connection_account_choices(b).unwrap();
    assert!(choices
        .iter()
        .any(|c| c.account_ref == "KIND" && !c.enabled));
}

#[test]
fn same_number_at_a_different_bank_is_no_overlap() {
    let (_d, v) = open("girocrab-accsel-4");
    let a = login(&v, "fnz", "FNZ");
    let b = login(&v, "paypal", "Andere Bank");
    store_fetch(&v, ConnectorKind::Fnz, a, fetch(vec![acc("X", "", "999")])).unwrap();
    store_fetch(
        &v,
        ConnectorKind::PayPal,
        b,
        fetch(vec![acc("Y", "", "999")]),
    )
    .unwrap();
    assert_eq!(count(&v), 2);
    assert!(v
        .pending_account_selection_connections()
        .unwrap()
        .is_empty());
}

#[test]
fn same_iban_matches_across_connectors() {
    let (_d, v) = open("girocrab-accsel-5");
    let a = login(&v, "fnz", "FNZ");
    let b = login(&v, "paypal", "Zweiter Zugang");
    store_fetch(
        &v,
        ConnectorKind::Fnz,
        a,
        fetch(vec![acc("X", "DE02 1203 0000 0000 2020 51", "")]),
    )
    .unwrap();
    store_fetch(
        &v,
        ConnectorKind::PayPal,
        b,
        fetch(vec![acc("Y", "de02120300000000202051", "")]),
    )
    .unwrap();
    assert_eq!(v.pending_account_selection_connections().unwrap(), vec![b]);
}

#[test]
fn deleting_a_login_forgets_its_account_choice() {
    let (_d, v) = open("girocrab-accsel-6");
    let a = login(&v, "fnz", "Mutter");
    let b = login(&v, "fnz", "Vater");
    store_fetch(
        &v,
        ConnectorKind::Fnz,
        a,
        fetch(vec![acc("KIND", "", "999")]),
    )
    .unwrap();
    store_fetch(
        &v,
        ConnectorKind::Fnz,
        b,
        fetch(vec![acc("KIND", "", "999")]),
    )
    .unwrap();
    assert_eq!(v.pending_account_selection_connections().unwrap(), vec![b]);
    let key_b = crate::connectors::account_selection::key(&v.db().unwrap(), b).unwrap();
    v.delete_bank_connection(b).unwrap();
    assert!(v
        .pending_account_selection_connections()
        .unwrap()
        .is_empty());
    let left: i64 = v
        .db()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM settings WHERE key = ?1",
            [key_b],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(left, 0);
}
