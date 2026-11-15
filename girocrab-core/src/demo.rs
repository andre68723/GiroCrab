//! Demo mode: five fictional German accounts and exactly 50 invented bookings
//! (`--demo-seed` / `GIROCRAB_DEMO_SEED=1`). Deterministic and relative to
//! today so screenshots are reproducible. Nothing in here is real data.

use chrono::{Datelike, Duration, Local, NaiveDate};
use rusqlite::{params, Connection};

use crate::connectors::{descriptor, group_for};
use crate::error::CoreResult;
use crate::models::{AccountKind, ConnectorKind};
use crate::vault::Vault;

/// Documents across three accounts, with origin and booking link varied independently.
fn demo_documents(conn: &Connection, today: NaiveDate) -> CoreResult<()> {
    // account, title, document type, days ago, bank document, linked counterparty
    let docs = [
        (
            "demo-giro",
            "REWE – Kassenbeleg",
            "beleg",
            2,
            false,
            Some("REWE Markt GmbH"),
        ),
        (
            "demo-giro",
            "Elektro Weber – Handwerkerrechnung",
            "beleg",
            1,
            false,
            None,
        ),
        (
            "demo-giro",
            "Musterbank – Kontoauszug",
            "kontoauszug",
            5,
            true,
            None,
        ),
        (
            "demo-giro",
            "Musterbank – Lastschriftbestätigung Strom",
            "postbox",
            20,
            true,
            Some("EnBW Energie Baden-Württemberg AG"),
        ),
        (
            "demo-visa",
            "IKEA – Rechnung Regal",
            "beleg",
            29,
            false,
            Some("IKEA Deutschland GmbH"),
        ),
        (
            "demo-visa",
            "Hotel Seeblick – Hotelrechnung",
            "beleg",
            3,
            false,
            None,
        ),
        (
            "demo-visa",
            "Visa Gold – Kreditkartenabrechnung",
            "kontoauszug",
            4,
            true,
            None,
        ),
        (
            "demo-depot",
            "MSCI World – Wertpapierabrechnung",
            "abrechnung",
            3,
            true,
            None,
        ),
        (
            "demo-depot",
            "Musterbank – Depotübersicht",
            "postbox",
            6,
            true,
            None,
        ),
    ];
    for (i, (account_ref, title, doc_type, days_ago, bank, counterparty)) in
        docs.into_iter().enumerate()
    {
        let (account_id, account_name): (i64, String) = conn.query_row(
            "SELECT id, name FROM accounts WHERE external_ref = ?1",
            [account_ref],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let mut date = (today - Duration::days(days_ago))
            .format("%Y-%m-%d")
            .to_string();
        let mut lines = vec![
            title.replace('–', "-"),
            "GiroCrab Demo - frei erfundene Beispieldaten".into(),
            format!("Konto: {account_name}"),
        ];
        let booking_id = if let Some(cp) = counterparty {
            let (id, booking_date, amount): (i64, String, i64) = conn.query_row(
                "SELECT id, booking_date, amount_cents FROM transactions
                 WHERE account_id = ?1 AND counterparty = ?2
                 ORDER BY booking_date DESC, id DESC LIMIT 1",
                params![account_id, cp],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            date = booking_date;
            lines.push(format!(
                "Betrag: {}",
                crate::money::format_cents(amount, "EUR".into())
            ));
            Some(id)
        } else {
            None
        };
        lines.push(format!("Datum: {date}"));
        lines.push(
            if bank {
                "Bankdokument aus der PostBox"
            } else {
                "Selbst hinzugefuegter Beleg"
            }
            .into(),
        );
        lines.push(
            if booking_id.is_some() {
                "Mit einer Buchung verknuepft"
            } else {
                "Ohne Buchungsverknuepfung"
            }
            .into(),
        );
        let source = if bank {
            format!("demo-bank:{account_ref}:{i}")
        } else {
            String::new()
        };
        crate::documents::insert_doc(
            conn,
            booking_id,
            Some(account_id),
            title,
            doc_type,
            "application/pdf",
            &format!("demo_dokument_{i}.pdf"),
            &crate::documents::simple_pdf(&lines),
            &source,
            &date,
        )?;
    }
    Ok(())
}

/// Login name that switches a bank connection to the simulated demo bank
/// (Apple App Review, guideline 2.1(a)): case-insensitive `demo`, PIN `12345`,
/// any 4- or 6-digit TAN. No network access and no FinTS product ID needed.
pub const MAGIC_DEMO_LOGIN: &str = "demo";
pub const MAGIC_DEMO_PIN: &str = "12345";

/// Tags across categories (#228), enough to try the tag sheet with: the car and
/// a trip share the rental car; two tags have no bookings (one with a long name).
/// Each: name, colour, counterparty patterns, purpose patterns (SQL `LIKE`).
type DemoTag = (
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
);

const DEMO_TAGS: [DemoTag; 12] = [
    (
        "Auto",
        "#E67E22",
        &["Shell Station", "Aral AG", "EnBW%", "Sixt%"],
        &[],
    ),
    ("Urlaub", "#2E86DE", &["Sixt%"], &["ICE Ticket%"]),
    (
        "Familie",
        "#27AE60",
        &["dm-drogerie markt", "Apotheke am Markt"],
        &[],
    ),
    (
        "Abos",
        "#8E44AD",
        &["Spotify%", "Netflix%", "Apple Distribution%"],
        &[],
    ),
    (
        "Wohnung",
        "#16A085",
        &["Hausverwaltung%", "Telekom%", "IKEA%"],
        &[],
    ),
    (
        "Lebensmittel",
        "#D4AC0D",
        &["REWE%", "Lidl%", "EDEKA%", "Alnatura%", "Bäckerei%"],
        &[],
    ),
    ("Pendeln", "#7F8C8D", &[], &["Deutschlandticket%"]),
    ("Online bestellt", "#C0392B", &["Amazon%"], &[]),
    (
        "Versicherungen",
        "#2E86DE",
        &["Allianz%", "Techniker%"],
        &[],
    ),
    ("Steuern", "#C0392B", &["Finanzamt%", "ARD ZDF%"], &[]),
    ("Umzug 2027", "#16A085", &[], &[]),
    (
        "Steuererklärung 2026 – Belege für das Finanzamt sammeln",
        "#8E44AD",
        &[],
        &[],
    ),
];

fn demo_tags(conn: &Connection) -> CoreResult<()> {
    let now = crate::user_groups::now_ms();
    for (i, (name, color, counterparties, purposes)) in DEMO_TAGS.iter().enumerate() {
        conn.execute(
            "INSERT INTO tags(uuid, name, color_hex, sort_order, updated_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                crate::sync_merge::tag_uuid_for_name(name),
                name,
                color,
                i as i64,
                now
            ],
        )?;
        let tag = conn.last_insert_rowid();
        for (column, patterns) in [("counterparty", counterparties), ("purpose", purposes)] {
            for pattern in patterns.iter() {
                conn.execute(
                    &format!(
                        "INSERT OR IGNORE INTO transaction_tags(transaction_id, tag_id)
                         SELECT id, ?1 FROM transactions WHERE {column} LIKE ?2"
                    ),
                    params![tag, pattern],
                )?;
            }
        }
    }
    conn.execute(
        "UPDATE transactions SET tags_ms = ?1
          WHERE id IN (SELECT transaction_id FROM transaction_tags)",
        [now],
    )?;
    Ok(())
}

pub fn is_magic_demo_login(login: &str) -> bool {
    login.trim().eq_ignore_ascii_case(MAGIC_DEMO_LOGIN)
}

pub struct DemoAccountSpec {
    pub connector: ConnectorKind,
    pub external_ref: &'static str,
    pub name: &'static str,
    pub kind: AccountKind,
    pub iban: &'static str,
    /// Account number (loans: referenced in the Giro's instalment bookings).
    pub account_number: &'static str,
    /// Today's balance in cents.
    pub opening_cents: i64,
    /// Overrides the institute name in the sidebar.
    pub bank_name: &'static str,
    /// Loan: original amount (drives the repayment progress bar).
    pub principal_cents: Option<i64>,
}

pub const DEMO_ACCOUNTS: &[DemoAccountSpec] = &[
    DemoAccountSpec {
        connector: ConnectorKind::VolksbankKasselGoettingen,
        external_ref: "demo-giro",
        name: "Musterbank Girokonto",
        kind: AccountKind::Checking,
        iban: "DE89370400440532013000",
        account_number: "0532013000",
        opening_cents: 4_280_50,
        bank_name: "Musterbank",
        principal_cents: None,
    },
    DemoAccountSpec {
        connector: ConnectorKind::VolksbankKasselGoettingen,
        external_ref: "demo-tagesgeld",
        name: "Tagesgeld Plus",
        kind: AccountKind::Savings,
        iban: "DE75512108001245126199",
        account_number: "1245126199",
        opening_cents: 15_000_00,
        bank_name: "Musterbank",
        principal_cents: None,
    },
    DemoAccountSpec {
        connector: ConnectorKind::TradeRepublic,
        external_ref: "demo-depot",
        name: "Welt-Depot ETF",
        kind: AccountKind::Brokerage,
        iban: "",
        account_number: "",
        opening_cents: 31_380_00,
        bank_name: "Musterbank",
        principal_cents: None,
    },
    DemoAccountSpec {
        connector: ConnectorKind::AmericanExpress,
        external_ref: "demo-visa",
        name: "Visa Gold Kreditkarte",
        kind: AccountKind::CreditCard,
        iban: "",
        account_number: "",
        opening_cents: -642_80,
        bank_name: "Musterbank",
        principal_cents: None,
    },
    DemoAccountSpec {
        connector: ConnectorKind::VolksbankKasselGoettingen,
        external_ref: "demo-darlehen",
        name: "Immobilien-Darlehen",
        kind: AccountKind::Loan,
        iban: "",
        account_number: "4009988",
        opening_cents: -182_500_00,
        bank_name: "Musterbank",
        principal_cents: Some(250_000_00),
    },
    DemoAccountSpec {
        connector: ConnectorKind::TradeRepublic,
        external_ref: "demo-krypto",
        name: "Krypto-Depot",
        kind: AccountKind::Brokerage,
        iban: "",
        account_number: "",
        opening_cents: 6_200_00,
        bank_name: "Musterbank",
        principal_cents: None,
    },
    // appended: the demo bookings address the accounts above by their index
    DemoAccountSpec {
        connector: ConnectorKind::TradeRepublic,
        external_ref: "demo-einzeltitel",
        name: "Einzeltitel-Depot",
        kind: AccountKind::Brokerage,
        iban: "",
        account_number: "",
        opening_cents: 16_590_00,
        bank_name: "Musterbank",
        principal_cents: None,
    },
    // PayPal wallet, Amazon orders, a second Giro and a second savings account (#556); appended for the same reason. They share the card
    // login: a login of their own would join "Alle abrufen" without a demo sync behind it.
    DemoAccountSpec {
        connector: ConnectorKind::AmericanExpress,
        external_ref: "demo-paypal",
        name: "PayPal",
        kind: AccountKind::PaymentService,
        iban: "",
        account_number: "",
        opening_cents: 187_45,
        bank_name: "PayPal",
        principal_cents: None,
    },
    DemoAccountSpec {
        connector: ConnectorKind::AmericanExpress,
        external_ref: "demo-amazon",
        name: "Amazon Bestellungen",
        kind: AccountKind::PaymentService,
        iban: "",
        account_number: "",
        opening_cents: -164_39,
        bank_name: "Amazon",
        principal_cents: None,
    },
    DemoAccountSpec {
        connector: ConnectorKind::VolksbankKasselGoettingen,
        external_ref: "demo-giro2",
        name: "Gemeinschaftskonto",
        kind: AccountKind::Checking,
        iban: "DE12500105170648489890",
        account_number: "0648489890",
        opening_cents: 1_820_30,
        bank_name: "Musterbank",
        principal_cents: None,
    },
    DemoAccountSpec {
        connector: ConnectorKind::VolksbankKasselGoettingen,
        external_ref: "demo-tagesgeld2",
        name: "Tagesgeld Familie",
        kind: AccountKind::Savings,
        iban: "DE27100777770209299700",
        account_number: "0209299700",
        opening_cents: 8_500_00,
        bank_name: "Musterbank",
        principal_cents: None,
    },
];

type Tree<'a> = &'a [(
    &'static str,
    &'static str,
    &'static str,
    &'a [(&'static str, &'static str)],
)];

fn categories(conn: &Connection) -> CoreResult<std::collections::HashMap<&'static str, i64>> {
    let tree: Tree = &[
        (
            "Einnahmen",
            "arrow.down.circle.fill",
            "#2E9D6A",
            &[
                ("Gehalt", "briefcase.fill"),
                ("Zinsen & Dividenden", "percent"),
                ("Erstattungen", "arrow.uturn.backward.circle"),
                ("Sonstige Einnahmen", "plus.circle"),
            ],
        ),
        (
            "Wohnen & Energie",
            "house.fill",
            "#B7782F",
            &[
                ("Miete", "building.2.fill"),
                ("Energie", "bolt.fill"),
                ("Internet & Telefon", "wifi"),
                ("Haushalt", "sofa.fill"),
            ],
        ),
        (
            "Lebensmittel",
            "cart.fill",
            "#B5474F",
            &[
                ("Supermarkt", "cart"),
                ("Bäcker & Metzger", "fork.knife.circle"),
                ("Lebensmittelrettung", "leaf.fill"),
                ("Restaurants & Cafés", "fork.knife"),
                ("Lieferdienste", "takeoutbag.and.cup.and.straw.fill"),
            ],
        ),
        (
            "Mobilität",
            "car.fill",
            "#3A6FB0",
            &[
                ("ÖPNV & Bahn", "tram.fill"),
                ("Tanken", "fuelpump.fill"),
                ("Fahrrad", "bicycle"),
                ("Reisen", "airplane"),
            ],
        ),
        (
            "Einkaufen",
            "bag.fill",
            "#7D5BA6",
            &[
                ("Online-Shopping", "shippingbox.fill"),
                ("Elektronik", "desktopcomputer"),
                ("Drogerie", "cross.vial.fill"),
            ],
        ),
        (
            "Freizeit",
            "sparkles",
            "#2F8F9D",
            &[
                ("Streaming & Musik", "play.tv.fill"),
                ("Sport & Fitness", "figure.run"),
                ("Spiele", "gamecontroller.fill"),
            ],
        ),
        (
            "Versicherungen",
            "shield.lefthalf.filled",
            "#4B5D73",
            &[
                ("Krankenversicherung", "cross.case.fill"),
                ("Haftpflicht & Hausrat", "shield.fill"),
                ("Mitgliedschaften", "person.badge.shield.checkmark.fill"),
            ],
        ),
        (
            "Gebühren & Bargeld",
            "eurosign.circle.fill",
            "#7A808A",
            &[
                ("Bankgebühren", "building.columns"),
                ("Bargeld", "banknote"),
            ],
        ),
        (
            "Kredite & Darlehen",
            "house.circle.fill",
            "#6B7280",
            &[("Darlehensraten", "house")],
        ),
        (
            "Sparen & Investieren",
            "chart.line.uptrend.xyaxis.circle.fill",
            "#1F7A5C",
            &[
                ("ETF-Sparpläne", "chart.line.uptrend.xyaxis"),
                ("Krypto", "bitcoinsign.circle"),
                ("Aktien", "chart.bar.fill"),
                ("Saveback & Round up", "gift.fill"),
            ],
        ),
        (
            "Umbuchungen",
            "arrow.left.arrow.right.circle.fill",
            "#9AA0A6",
            &[
                ("Kreditkartenabrechnung", "creditcard.fill"),
                ("Tagesgeld", "banknote.fill"),
                ("Eigene Konten", "arrow.left.arrow.right"),
            ],
        ),
    ];
    let mut map = std::collections::HashMap::new();
    for (i, (name, sym, color, kids)) in tree.iter().enumerate() {
        conn.execute(
            "INSERT INTO categories(parent_id, name, sf_symbol, color_hex, sort_order) VALUES (NULL, ?1, ?2, ?3, ?4)",
            params![name, sym, color, i as i32],
        )?;
        let pid = conn.last_insert_rowid();
        map.insert(*name, pid);
        for (j, (kid, ksym)) in kids.iter().enumerate() {
            conn.execute(
                "INSERT INTO categories(parent_id, name, sf_symbol, color_hex, sort_order) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![pid, kid, ksym, color, j as i32],
            )?;
            map.insert(*kid, conn.last_insert_rowid());
        }
    }
    conn.execute(
        "UPDATE categories SET budget_cents = 60000 WHERE name = 'Supermarkt'",
        [],
    )?;
    conn.execute(
        "UPDATE categories SET budget_cents = 15000 WHERE name = 'Restaurants & Cafés'",
        [],
    )?;
    Ok(map)
}

fn rules(conn: &Connection, cat: &std::collections::HashMap<&'static str, i64>) -> CoreResult<()> {
    // (name, category, field, kind, pattern, min, max)
    let rules: &[(&str, &str, &str, &str, &str, Option<i64>, Option<i64>)] = &[
        (
            "Saveback & Round up",
            "Saveback & Round up",
            "any",
            "keyword",
            "Saveback|Round up",
            None,
            None,
        ),
        (
            "Gehalt",
            "Gehalt",
            "any",
            "keyword",
            "Gehalt|Lohn",
            Some(1),
            None,
        ),
        (
            "Zinsen & Dividenden",
            "Zinsen & Dividenden",
            "any",
            "regex",
            r"\b(Zinsen|Dividende|Ertrag)\b",
            Some(1),
            None,
        ),
        (
            "Erstattungen",
            "Erstattungen",
            "any",
            "keyword",
            "Erstattung|Rückerstattung|Gutschrift Retoure",
            Some(1),
            None,
        ),
        (
            "Tagesgeld",
            "Tagesgeld",
            "purpose",
            "keyword",
            "Tagesgeld",
            None,
            None,
        ),
        (
            "Kreditkarte",
            "Kreditkartenabrechnung",
            "any",
            "keyword",
            "Kreditkartenabrechnung|Zahlung erhalten",
            None,
            None,
        ),
        (
            "Eigene Konten",
            "Eigene Konten",
            "any",
            "keyword",
            "Einzahlung|Aufladung|Eigenes Konto|Bankeinzug",
            None,
            None,
        ),
        (
            "ETF-Sparpläne",
            "ETF-Sparpläne",
            "counterparty",
            "regex",
            r"(?i)(MSCI|STOXX|FTSE|AI & Big Data|S&P 500)",
            None,
            Some(0),
        ),
        (
            "Krypto",
            "Krypto",
            "counterparty",
            "keyword",
            "Bitcoin|Ethereum",
            None,
            Some(0),
        ),
        (
            "Aktien-Sparplan",
            "Aktien",
            "counterparty",
            "keyword",
            "Bayer",
            None,
            Some(0),
        ),
        (
            "Darlehen",
            "Darlehensraten",
            "any",
            "keyword",
            "Darlehensrate|Tilgung",
            None,
            None,
        ),
        (
            "Supermärkte",
            "Supermarkt",
            "counterparty",
            "keyword",
            "REWE|EDEKA|ALDI|LIDL|Alnatura|Kaufland|Penny|Netto",
            None,
            Some(0),
        ),
        (
            "Bäcker & Metzger",
            "Bäcker & Metzger",
            "counterparty",
            "keyword",
            "Vinzenz Murr|Metzgerei|Bäckerei",
            None,
            Some(0),
        ),
        (
            "Lebensmittelrettung",
            "Lebensmittelrettung",
            "counterparty",
            "keyword",
            "Too Good To Go",
            None,
            Some(0),
        ),
        (
            "Restaurants",
            "Restaurants & Cafés",
            "any",
            "keyword",
            "Restaurant|Café|Pizzeria|Trattoria",
            None,
            Some(0),
        ),
        (
            "Lieferdienste",
            "Lieferdienste",
            "any",
            "keyword",
            "Lieferando|Wolt",
            None,
            Some(0),
        ),
        (
            "Energie",
            "Energie",
            "counterparty",
            "keyword",
            "EnBW Energie|Stadtwerke",
            None,
            Some(0),
        ),
        (
            "Telefon & Internet",
            "Internet & Telefon",
            "counterparty",
            "keyword",
            "Telekom|Vodafone|congstar",
            None,
            Some(0),
        ),
        (
            "Bahn & ÖPNV",
            "ÖPNV & Bahn",
            "any",
            "regex",
            r"(?i)^DB\s+(Fernverkehr|Vertrieb)|NVV|Deutschlandticket",
            None,
            Some(0),
        ),
        (
            "Tanken & Laden",
            "Tanken",
            "counterparty",
            "keyword",
            "Aral|Shell|Esso|EnBW|IONITY|Tesla",
            None,
            Some(0),
        ),
        (
            "Baumarkt & Möbel",
            "Haushalt",
            "counterparty",
            "keyword",
            "HORNBACH|OBI|BAUHAUS|IKEA|toom",
            None,
            Some(0),
        ),
        (
            "Fahrrad",
            "Fahrrad",
            "counterparty",
            "keyword",
            "Fixbike",
            None,
            Some(0),
        ),
        (
            "Reisen",
            "Reisen",
            "any",
            "keyword",
            "Hotel|Lufthansa|Booking.com|Airbnb|Sixt",
            None,
            Some(0),
        ),
        (
            "Online-Shops",
            "Online-Shopping",
            "any",
            "keyword",
            "Amazon|AMZN|Zalando|eBay",
            None,
            Some(0),
        ),
        (
            "Elektronik",
            "Elektronik",
            "counterparty",
            "keyword",
            "MediaMarkt|Cyberport|Apple Distribution",
            None,
            Some(0),
        ),
        (
            "Drogerie",
            "Drogerie",
            "counterparty",
            "keyword",
            "dm-drogerie|Rossmann|Apotheke",
            None,
            Some(0),
        ),
        (
            "Streaming",
            "Streaming & Musik",
            "any",
            "keyword",
            "Spotify|Netflix|APPLE.COM|Disney",
            None,
            Some(0),
        ),
        (
            "Fitness",
            "Sport & Fitness",
            "counterparty",
            "keyword",
            "FitX|Kletterhalle",
            None,
            Some(0),
        ),
        (
            "Spiele",
            "Spiele",
            "any",
            "keyword",
            "Steam|Nintendo",
            None,
            Some(0),
        ),
        (
            "Krankenversicherung",
            "Krankenversicherung",
            "purpose",
            "keyword",
            "Krankenversicherung",
            None,
            Some(0),
        ),
        (
            "Haftpflicht",
            "Haftpflicht & Hausrat",
            "purpose",
            "regex",
            r"(Haftpflicht|Hausrat)",
            None,
            Some(0),
        ),
        (
            "Miete",
            "Miete",
            "counterparty",
            "keyword",
            "Hausverwaltung",
            None,
            Some(0),
        ),
        (
            "Rundfunkbeitrag",
            "Haushalt",
            "counterparty",
            "keyword",
            "Beitragsservice",
            None,
            Some(0),
        ),
        (
            "Mitgliedschaften",
            "Mitgliedschaften",
            "counterparty",
            "keyword",
            "ADAC",
            None,
            Some(0),
        ),
        (
            "Kontoführung",
            "Bankgebühren",
            "any",
            "keyword",
            "Kontoführung|Entgelt",
            None,
            Some(0),
        ),
        (
            "Geldautomat",
            "Bargeld",
            "booking_text",
            "keyword",
            "Bargeldauszahlung|Geldautomat",
            None,
            None,
        ),
    ];
    for (i, (name, c, field, kind, pattern, min, max)) in rules.iter().enumerate() {
        conn.execute(
            "INSERT INTO rules(name, category_id, field, match_kind, pattern, min_amount_cents, max_amount_cents, priority, enabled)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1)",
            params![name, cat[c], field, kind, pattern, min, max, (i as i32 + 1) * 10],
        )?;
    }
    Ok(())
}

/// (account index, days ago, cents, counterparty, purpose, booking text) – exactly 68 invented bookings
/// (the car-loan rates of the manual demo loan are followed by PayPal, Amazon orders and second Giro/savings bookings).
const BOOKINGS: &[(usize, i64, i64, &str, &str, &str)] = &[
    (
        0,
        52,
        365000,
        "TechNova GmbH",
        "Gehalt September",
        "LOHN/GEHALT",
    ),
    (
        0,
        22,
        365000,
        "TechNova GmbH",
        "Gehalt Oktober",
        "LOHN/GEHALT",
    ),
    (
        0,
        38,
        41230,
        "Finanzamt Musterstadt",
        "Einkommensteuer Erstattung",
        "GUTSCHRIFT",
    ),
    (
        0,
        55,
        -115000,
        "Hausverwaltung Nord GmbH",
        "Miete und Nebenkosten Oktober",
        "DAUERAUFTRAG",
    ),
    (
        0,
        25,
        -115000,
        "Hausverwaltung Nord GmbH",
        "Miete und Nebenkosten November",
        "DAUERAUFTRAG",
    ),
    (
        0,
        50,
        -9500,
        "EnBW Energie Baden-Württemberg AG",
        "Abschlag Strom",
        "FOLGELASTSCHRIFT",
    ),
    (
        0,
        20,
        -9500,
        "EnBW Energie Baden-Württemberg AG",
        "Abschlag Strom",
        "FOLGELASTSCHRIFT",
    ),
    (
        0,
        30,
        -4495,
        "Telekom Deutschland GmbH",
        "Festnetz und Internet",
        "FOLGELASTSCHRIFT",
    ),
    (
        0,
        45,
        -1836,
        "ARD ZDF Deutschlandradio Beitragsservice",
        "Rundfunkbeitrag Quartal",
        "FOLGELASTSCHRIFT",
    ),
    (
        0,
        56,
        -95000,
        "Darlehensrate Zins & Tilgung Kto 4009988",
        "Darlehensrate Zins & Tilgung Kto 4009988",
        "LASTSCHRIFT",
    ),
    (
        0,
        26,
        -95000,
        "Darlehensrate Zins & Tilgung Kto 4009988",
        "Darlehensrate Zins & Tilgung Kto 4009988",
        "LASTSCHRIFT",
    ),
    (
        0,
        53,
        -50000,
        "iShares Core MSCI World ETF",
        "Sparplan",
        "SPARPLAN",
    ),
    (
        0,
        23,
        -50000,
        "iShares Core MSCI World ETF",
        "Sparplan",
        "SPARPLAN",
    ),
    (
        0,
        24,
        -25000,
        "Vanguard FTSE All-World ETF",
        "Sparplan",
        "SPARPLAN",
    ),
    (
        0,
        54,
        -30000,
        "Tagesgeld Übertrag",
        "Tagesgeld Übertrag",
        "UMBUCHUNG",
    ),
    (
        0,
        2,
        -4782,
        "REWE Markt GmbH",
        "Kartenzahlung girocard",
        "KARTENZAHLUNG",
    ),
    (
        0,
        17,
        -6315,
        "REWE Markt GmbH",
        "Kartenzahlung girocard",
        "KARTENZAHLUNG",
    ),
    (
        0,
        41,
        -3840,
        "REWE Markt GmbH",
        "Kartenzahlung girocard",
        "KARTENZAHLUNG",
    ),
    (
        0,
        6,
        -2977,
        "Lidl Dienstleistung GmbH",
        "Kartenzahlung girocard",
        "KARTENZAHLUNG",
    ),
    (
        0,
        33,
        -5108,
        "Lidl Dienstleistung GmbH",
        "Kartenzahlung girocard",
        "KARTENZAHLUNG",
    ),
    (
        0,
        11,
        -7264,
        "EDEKA Center",
        "Kartenzahlung girocard",
        "KARTENZAHLUNG",
    ),
    (
        0,
        9,
        -2345,
        "dm-drogerie markt",
        "Kartenzahlung girocard",
        "KARTENZAHLUNG",
    ),
    (
        0,
        36,
        -3190,
        "dm-drogerie markt",
        "Kartenzahlung girocard",
        "KARTENZAHLUNG",
    ),
    (
        0,
        15,
        -1429,
        "Rossmann",
        "Kartenzahlung girocard",
        "KARTENZAHLUNG",
    ),
    (
        0,
        4,
        -680,
        "Bäckerei Konditorei König",
        "Kartenzahlung girocard",
        "KARTENZAHLUNG",
    ),
    (
        0,
        28,
        -3456,
        "Alnatura Bio-Supermarkt",
        "Kartenzahlung girocard",
        "KARTENZAHLUNG",
    ),
    (
        0,
        13,
        -1875,
        "Apotheke am Markt",
        "Kartenzahlung girocard",
        "KARTENZAHLUNG",
    ),
    (
        0,
        51,
        -4900,
        "DB Vertrieb GmbH",
        "Deutschlandticket November",
        "FOLGELASTSCHRIFT",
    ),
    (
        0,
        21,
        -4900,
        "DB Vertrieb GmbH",
        "Deutschlandticket Dezember",
        "FOLGELASTSCHRIFT",
    ),
    (
        0,
        40,
        -7490,
        "DB Vertrieb GmbH",
        "ICE Ticket Kassel-Hamburg",
        "KARTENZAHLUNG",
    ),
    (
        0,
        8,
        -6843,
        "Shell Station",
        "Kartenzahlung girocard",
        "KARTENZAHLUNG",
    ),
    (
        0,
        34,
        -7210,
        "Aral AG",
        "Kartenzahlung girocard",
        "KARTENZAHLUNG",
    ),
    (
        0,
        19,
        -2136,
        "EnBW mobility+ Ladestrom",
        "Ladevorgang",
        "LASTSCHRIFT",
    ),
    (
        3,
        43,
        -18900,
        "Sixt GmbH & Co. Autovermietung",
        "Mietwagen Wochenende",
        "KARTENZAHLUNG",
    ),
    (
        3,
        3,
        -3999,
        "Amazon.de",
        "Bestellung 302-1234567-8901234",
        "KARTENZAHLUNG",
    ),
    (
        3,
        18,
        -2490,
        "Amazon.de",
        "Bestellung 302-2345678-9012345",
        "KARTENZAHLUNG",
    ),
    (
        3,
        37,
        -6149,
        "Amazon.de",
        "Bestellung 302-3456789-0123456",
        "KARTENZAHLUNG",
    ),
    (
        3,
        10,
        -299,
        "Apple Distribution International",
        "iCloud+ Speicher",
        "KARTENZAHLUNG",
    ),
    (
        3,
        42,
        -1299,
        "Apple Distribution International",
        "Apple Music",
        "KARTENZAHLUNG",
    ),
    (
        3,
        29,
        -12900,
        "IKEA Deutschland GmbH",
        "Kartenzahlung Regal",
        "KARTENZAHLUNG",
    ),
    (
        3,
        14,
        -1099,
        "Spotify AB",
        "Spotify Premium",
        "KARTENZAHLUNG",
    ),
    (
        3,
        16,
        -1399,
        "Netflix International B.V.",
        "Netflix Standard",
        "KARTENZAHLUNG",
    ),
    (
        0,
        48,
        -2780,
        "Allianz Versicherungs-AG",
        "Hausrat Beitrag",
        "FOLGELASTSCHRIFT",
    ),
    (
        0,
        31,
        6500,
        "Techniker Krankenkasse",
        "Bonusprogramm Erstattung",
        "GUTSCHRIFT",
    ),
    (
        0,
        46,
        -5400,
        "ADAC e.V.",
        "Mitgliedsbeitrag",
        "FOLGELASTSCHRIFT",
    ),
    (
        0,
        5,
        -3500,
        "Max Mustermann",
        "Danke für das Auslegen",
        "ÜBERWEISUNG",
    ),
    (0, 12, 2000, "Laura Schneider", "Kinokarten", "GUTSCHRIFT"),
    (
        0,
        27,
        -6000,
        "Lukas Weber",
        "Geburtstagsgeschenk Anteil",
        "ÜBERWEISUNG",
    ),
    (
        0,
        35,
        4250,
        "Hannah Berger",
        "Restaurant Anteil",
        "GUTSCHRIFT",
    ),
    (0, 1, -1500, "Tim Hoffmann", "Konzertticket", "ÜBERWEISUNG"),
    (
        0,
        54,
        -28900,
        "Autobank AG",
        "Rate Kreditvertrag 7731045 Autokredit",
        "LASTSCHRIFT",
    ),
    (
        0,
        24,
        -28900,
        "Autobank AG",
        "Rate Kreditvertrag 7731045 Autokredit",
        "LASTSCHRIFT",
    ),
    (
        7,
        4,
        -2499,
        "Spotify AB",
        "PayPal-Zahlung Spotify Premium",
        "ZAHLUNG",
    ),
    (
        7,
        9,
        -5990,
        "Zalando SE",
        "PayPal-Zahlung Bestellung 10458-2291",
        "ZAHLUNG",
    ),
    (
        7,
        15,
        12000,
        "Anna Becker",
        "Rückzahlung Konzertkarten",
        "ZAHLUNG ERHALTEN",
    ),
    (
        7,
        27,
        -1899,
        "eBay Kleinanzeigen",
        "PayPal-Zahlung Kamerazubehör",
        "ZAHLUNG",
    ),
    (
        7,
        41,
        -3450,
        "Steam Games S.a.r.l.",
        "PayPal-Zahlung Spielekauf",
        "ZAHLUNG",
    ),
    (
        7,
        60,
        25000,
        "Giro Musterbank",
        "Aufladung vom Girokonto",
        "GUTHABEN",
    ),
    (
        8,
        2,
        -4799,
        "Amazon",
        "Kopfhörer Bluetooth – Bestellung 303-4455667-1020304",
        "Amazon-Bestellung",
    ),
    (
        8,
        10,
        1599,
        "Amazon",
        "Erstattung für Bestellung 303-5566778-2131415",
        "Amazon-Erstattung",
    ),
    (
        8,
        14,
        -1599,
        "Amazon",
        "2× USB-C Ladekabel – Bestellung 303-5566778-2131415",
        "Amazon-Bestellung",
    ),
    (
        8,
        33,
        -8990,
        "Amazon",
        "Kaffeebohnen 1 kg; Entkalker – Bestellung 303-6677889-3242526",
        "Amazon-Bestellung",
    ),
    (
        8,
        45,
        -2650,
        "Amazon",
        "Buch: Rust in Action – Bestellung 303-7788990-4353637",
        "Amazon-Bestellung",
    ),
    (
        9,
        20,
        280000,
        "Erika Mustermann",
        "Gehalt Anteil Oktober",
        "DAUERAUFTRAG",
    ),
    (
        9,
        8,
        -45000,
        "Hausverwaltung Nord",
        "Nebenkosten Abschlag",
        "DAUERAUFTRAG",
    ),
    (
        9,
        3,
        -12740,
        "REWE Markt GmbH",
        "Wocheneinkauf",
        "KARTENZAHLUNG",
    ),
    (10, 30, 1250, "Musterbank", "Zinsgutschrift Q3", "ZINSEN"),
    (
        10,
        12,
        50000,
        "Gemeinschaftskonto",
        "Sparrate Urlaub",
        "UEBERTRAG",
    ),
];

struct Tx {
    account: &'static str,
    date: NaiveDate,
    cents: i64,
    counterparty: String,
    purpose: String,
    booking_text: String,
}

fn generate(today: NaiveDate) -> Vec<Tx> {
    BOOKINGS
        .iter()
        .map(|(acc, ago, cents, cp, purpose, bt)| Tx {
            account: DEMO_ACCOUNTS[*acc].external_ref,
            date: today - Duration::days(*ago),
            cents: *cents,
            counterparty: cp.to_string(),
            purpose: purpose.to_string(),
            booking_text: bt.to_string(),
        })
        .collect()
}

#[uniffi::export]
impl Vault {
    /// For a brand-new personal vault: the German category tree and the
    /// starter rules (no accounts, no bookings). Returns the number of
    /// categories created; no-op if categories exist or ever existed (deleted
    /// starter categories must not come back and spread to other devices).
    pub fn seed_starter_categories(&self) -> Result<u32, crate::error::CoreError> {
        let db = self.db()?;
        // includes soft-deleted rows
        let n: i64 = db.query_row("SELECT count(*) FROM categories", [], |r| r.get(0))?;
        if n > 0 {
            return Ok(0);
        }
        let tx = db.unchecked_transaction()?;
        let cats = categories(&tx)?;
        rules(&tx, &cats)?;
        crate::sync_merge::fill_missing_uuids(&tx)?;
        tx.commit()?;
        Ok(cats.len() as u32)
    }

    /// Number of accounts (0 = nothing synced yet).
    pub fn account_count(&self) -> Result<u32, crate::error::CoreError> {
        let db = self.db()?;
        Ok(db.query_row(
            "SELECT count(*) FROM accounts WHERE deleted_ms IS NULL",
            [],
            |r| r.get::<_, i64>(0),
        )? as u32)
    }

    /// Whether the vault ever held an account. Counts every row, so a vault
    /// whose accounts are all deleted still syncs once deletions keep rows.
    pub fn has_account_history(&self) -> Result<bool, crate::error::CoreError> {
        let db = self.db()?;
        Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM accounts)", [], |r| r.get(0))?)
    }

    /// Fills an empty vault with fictitious demo data for all 7 banks
    /// (accounts incl. loans, ~6 months of bookings, categories, rules).
    /// Returns the number of bookings created. No-op if data exists.
    pub fn seed_demo_data(&self) -> Result<u32, crate::error::CoreError> {
        if !self.is_empty()? {
            return Ok(0);
        }
        let db = self.db()?;
        let tx = db.unchecked_transaction()?;
        let cats = categories(&tx)?;
        rules(&tx, &cats)?;
        crate::sync_merge::fill_missing_uuids(&tx)?;
        let mut conn_ids = std::collections::HashMap::new();
        let logins = [
            (ConnectorKind::VolksbankKasselGoettingen, "demo-musterbank"),
            (ConnectorKind::TradeRepublic, "demo-depot"),
            (ConnectorKind::AmericanExpress, "demo-visa"),
        ];
        for (kind, login) in logins {
            let d = descriptor(kind);
            tx.execute(
                "INSERT INTO bank_connections(connector, display_name, login_name, bank_code, sf_symbol, last_sync_unix)
                 VALUES (?1, ?2, ?3, ?4, ?5, strftime('%s','now') - 3600)",
                params![kind.as_str(), d.name, login, d.bank_code, d.sf_symbol],
            )?;
            conn_ids.insert(kind, tx.last_insert_rowid());
        }
        // A few fetch-log rows so the Abruf-Protokoll window is not empty in the demo (#238).
        {
            let now_ms = chrono::Utc::now().timestamp_millis();
            let hour = 3_600_000_i64;
            let rows: [(ConnectorKind, &str, i64, &str, &str, &str, i64); 6] = [
                (
                    ConnectorKind::VolksbankKasselGoettingen,
                    "demo-musterbank",
                    1,
                    "manual",
                    "ok",
                    "",
                    12,
                ),
                (
                    ConnectorKind::TradeRepublic,
                    "demo-depot",
                    3,
                    "automatic",
                    "ok",
                    "",
                    0,
                ),
                (
                    ConnectorKind::AmericanExpress,
                    "demo-visa",
                    6,
                    "manual",
                    "error",
                    "Zeitüberschreitung beim Verbindungsaufbau zur Bank.",
                    0,
                ),
                (
                    ConnectorKind::VolksbankKasselGoettingen,
                    "demo-musterbank",
                    27,
                    "automatic",
                    "cancelled",
                    "Automatischer Abruf übersprungen: Die Bank verlangt eine neue Freigabe.",
                    0,
                ),
                (
                    ConnectorKind::TradeRepublic,
                    "demo-depot",
                    50,
                    "manual",
                    "error",
                    "Anmeldung abgelehnt (HTTP 401). Bitte Zugangsdaten prüfen.",
                    0,
                ),
                (
                    ConnectorKind::VolksbankKasselGoettingen,
                    "demo-musterbank",
                    74,
                    "manual",
                    "ok",
                    "",
                    31,
                ),
            ];
            for (kind, _, hours_ago, source, outcome, message, imported) in rows {
                let d = descriptor(kind);
                tx.execute(
                    "INSERT INTO sync_log(ts_ms, connection_id, connection_name, source, outcome, message, imported)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![now_ms - hours_ago * hour, conn_ids[&kind], d.name, source, outcome, message, imported],
                )?;
            }
        }
        let today = Local::now().date_naive();
        let txs = generate(today);
        for g in [
            "Giro & Tagesgeld",
            "Kreditkarten",
            "Depots & ETFs",
            "Darlehen & Kredite",
        ] {
            let (_, sym, order) = [
                AccountKind::Checking,
                AccountKind::CreditCard,
                AccountKind::Brokerage,
                AccountKind::Loan,
            ]
            .into_iter()
            .map(group_for)
            .find(|x| x.0 == g)
            .unwrap();
            Vault::ensure_group(&tx, g, sym, order)?;
        }
        let mut acc_ids = std::collections::HashMap::new();
        for (i, spec) in DEMO_ACCOUNTS.iter().enumerate() {
            let (group, sym, order) = group_for(spec.kind);
            let gid = Vault::ensure_group(&tx, group, sym, order)?;
            let balance = spec.opening_cents;
            let bank = if spec.bank_name.is_empty() {
                descriptor(spec.connector).name
            } else {
                spec.bank_name.to_string()
            };
            tx.execute(
                "INSERT INTO accounts(connection_id, group_id, external_ref, name, bank_name, owner, iban, account_number, currency, kind,
                                      balance_cents, balance_date, sf_symbol, sort_order, initial_balance_cents)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?14, ?6, ?7, 'EUR', ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    conn_ids[&spec.connector],
                    gid,
                    spec.external_ref,
                    spec.name,
                    bank,
                    spec.iban,
                    spec.account_number,
                    spec.kind.as_str(),
                    balance,
                    today.format("%Y-%m-%d").to_string(),
                    spec.kind.sf_symbol(),
                    i as i32,
                    spec.principal_cents,
                    // a second owner, so the wealth view has people to tell apart (#280, #351)
                    if matches!(spec.external_ref, "demo-einzeltitel" | "demo-krypto") {
                        "Anton Mustermann"
                    } else {
                        "Erika Mustermann"
                    }
                ],
            )?;
            acc_ids.insert(spec.external_ref, tx.last_insert_rowid());
        }
        // Scheduled transfers for the "Geplant" page (#240): fictional, relative to today.
        for (n, (days, name, iban, cents, purpose)) in [
            (
                6,
                "Hausverwaltung Nord",
                "DE02120300000000202051",
                85_000,
                "Miete Dezember",
            ),
            (
                12,
                "Stadtwerke München",
                "DE89370400440532013000",
                8_500,
                "Abschlag Strom Kd-Nr 482910",
            ),
            (
                23,
                "Finanzamt Musterstadt",
                "DE75512108001245126199",
                62_000,
                "Einkommensteuer Vorauszahlung Q4",
            ),
            (
                41,
                "Anna Becker",
                "DE44500105175407324931",
                15_000,
                "Anteil Weihnachtsgeschenk",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            tx.execute(
                "INSERT INTO planned_transfers(account_id, end_to_end_id, execution_date, recipient_name,
                        recipient_iban, amount_cents, purpose, created_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)",
                params![
                    acc_ids["demo-giro"],
                    format!("DEMOPLAN{n}"),
                    (today + chrono::Duration::days(days)).format("%Y-%m-%d").to_string(),
                    name,
                    iban,
                    cents,
                    purpose
                ],
            )?;
        }
        // A submitted direct debit collection (#143): two member fees due soon.
        for (n, (name, iban, purpose)) in [
            ("Mia Beispiel", "DE89370400440532013000", "Mitgliedsbeitrag"),
            ("Jan Muster", "DE02120300000000202051", "Mitgliedsbeitrag"),
        ]
        .into_iter()
        .enumerate()
        {
            tx.execute(
                "INSERT INTO planned_transfers(account_id, end_to_end_id, execution_date, recipient_name,
                        recipient_iban, amount_cents, purpose, created_ms, kind)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1250, ?6, 0, 'debit')",
                params![
                    acc_ids["demo-giro"],
                    format!("DEMODEBIT{n}"),
                    (today + chrono::Duration::days(9)).format("%Y-%m-%d").to_string(),
                    name,
                    iban,
                    purpose
                ],
            )?;
        }
        // Standing orders as the bank reports them (#239): rent monthly, a savings
        // transfer quarterly and a weekly payment. The monthly ones are changeable and
        // deletable (#496), the weekly one is read-only.
        for (name, iban, cents, purpose, unit, interval, day, last) in [
            (
                "Hausverwaltung Nord",
                "DE02120300000000202051",
                85_000,
                "Miete",
                "M",
                1,
                1,
                None,
            ),
            (
                "Sparverein Nord",
                "DE89370400440532013000",
                30_000,
                "Quartalssparen",
                "M",
                3,
                15,
                None,
            ),
            (
                "Fitnessstudio Mitte",
                "DE44500105175407324931",
                1_250,
                "Beitrag Kursgebühr",
                "W",
                1,
                5,
                Some("2027-06-30"),
            ),
        ] {
            tx.execute(
                "INSERT INTO standing_orders(account_id, key, order_id, recipient_name, recipient_iban,
                        amount_cents, purpose, first_date, last_date, time_unit, interval, exec_day,
                        can_change, can_delete, fetched_ms)
                 VALUES (?1, ?2, ?2, ?3, ?4, ?5, ?6, '2025-01-01', ?7, ?8, ?9, ?10, ?11, ?11, 0)",
                params![
                    acc_ids["demo-giro"],
                    format!("DEMO-{purpose}"),
                    name,
                    iban,
                    cents,
                    purpose,
                    last,
                    unit,
                    interval,
                    day,
                    // Rent and savings can be changed (as with a bank that allows it); the weekly one cannot.
                    i64::from(unit == "M"),
                ],
            )?;
        }
        // Direct debit demo (#143): creditor ID and a few member mandates.
        tx.execute(
            "INSERT INTO creditor_ids(account_id, creditor_id, updated_ms) VALUES (?1, 'DE98ZZZ09999999999', 0)",
            params![acc_ids["demo-giro"]],
        )?;
        for (name, iban, reference, signed, scheme, cents, purpose, active, last) in [
            (
                "Mia Beispiel",
                "DE89370400440532013000",
                "M-2024-001",
                "2024-03-01",
                "CORE",
                1_250,
                "Mitgliedsbeitrag",
                1,
                Some("2026-10-01"),
            ),
            (
                "Jan Muster",
                "DE02120300000000202051",
                "M-2025-014",
                "2025-01-15",
                "CORE",
                1_250,
                "Mitgliedsbeitrag",
                1,
                None,
            ),
            (
                "Muster Bau GmbH",
                "DE44500105175407324931",
                "M-2025-020",
                "2025-06-02",
                "B2B",
                25_000,
                "Wartungsvertrag",
                1,
                Some("2026-10-15"),
            ),
            (
                "Eva Probe",
                "DE89370400440532013000",
                "M-2023-007",
                "2023-09-10",
                "CORE",
                1_250,
                "Mitgliedsbeitrag",
                0,
                Some("2025-12-01"),
            ),
        ] {
            tx.execute(
                "INSERT INTO mandates(account_id, debtor_name, debtor_iban, reference, signed_date, scheme,
                        default_amount_cents, default_purpose, active, last_collected, created_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0)",
                params![
                    acc_ids["demo-giro"],
                    name,
                    iban,
                    reference,
                    signed,
                    scheme,
                    cents,
                    purpose,
                    active,
                    last
                ],
            )?;
        }
        // Depot positions: the two savings plans and a money market ETF, summing to the balance
        // of the ETF depot (31.380,00 EUR), and single securities in a depot of their own (16.590,00 EUR): five
        // stocks, a federal bond and a gold ETC. So the "Positionen" cards are not empty in the
        // demo and the wealth view has several asset classes and instruments to tell apart (#351).
        for (isin, wkn, name, quantity, current, purchase, prev_day) in [
            (
                "IE00B4L5Y983",
                "A0RPWH",
                "iShares Core MSCI World UCITS ETF",
                160.0_f64,
                104_25_i64,
                92_40_i64,
                103_60_i64,
            ),
            (
                "IE00BK5BQT80",
                "A2PKXG",
                "Vanguard FTSE All-World UCITS ETF",
                110.0,
                107_00,
                98_10,
                107_45,
            ),
            (
                "LU0290358497",
                "DBX0AN",
                "Xtrackers II EUR Overnight Rate Swap UCITS ETF",
                20.0,
                146_50,
                141_20,
                146_48,
            ),
            (
                "DE0007236101",
                "723610",
                "Siemens AG",
                15.0,
                180_00,
                148_00,
                181_30,
            ),
            (
                "US0378331005",
                "865985",
                "Apple Inc.",
                12.0,
                210_00,
                165_00,
                208_40,
            ),
            (
                "DE0007164600",
                "716460",
                "SAP SE",
                10.0,
                240_00,
                131_00,
                238_20,
            ),
            (
                "US5949181045",
                "870747",
                "Microsoft Corp.",
                6.0,
                400_00,
                310_00,
                397_50,
            ),
            (
                "DE0008404005",
                "840400",
                "Allianz SE",
                8.0,
                285_00,
                232_50,
                286_10,
            ),
            (
                "DE0001102580",
                "110258",
                "Bundesrepublik Deutschland Anleihe 2032",
                20.0,
                97_50,
                99_20,
                97_35,
            ),
            (
                "DE000A0S9GB0",
                "A0S9GB",
                "Xetra-Gold",
                30.0,
                78_00,
                52_40,
                77_60,
            ),
        ] {
            let value = (quantity * current as f64).round() as i64;
            let day = ((current - prev_day) as f64 * quantity).round() as i64;
            let gain = ((current - purchase) as f64 * quantity).round() as i64;
            tx.execute(
                "INSERT INTO holdings(account_id, isin, wkn, name, quantity, current_price_cents,
                        purchase_price_cents, value_cents, prev_day_price_cents, prev_day_value_cents,
                        profit_loss_purchase_cents, profit_loss_purchase_rel,
                        profit_loss_prev_day_cents, profit_loss_prev_day_rel)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    acc_ids[if name.contains("ETF") {
                        "demo-depot"
                    } else {
                        "demo-einzeltitel"
                    }],
                    isin,
                    wkn,
                    name,
                    quantity,
                    current,
                    purchase,
                    value,
                    prev_day,
                    value - day,
                    gain,
                    (current - purchase) as f64 / purchase as f64 * 100.0,
                    day,
                    (current - prev_day) as f64 / prev_day as f64 * 100.0
                ],
            )?;
        }
        // The demo broker reports the type of its stocks, as a connector with explicit labels
        // does; the ETFs, the bond and the gold ETC are left to the local rules.
        tx.execute(
            "INSERT INTO holding_classifications(holding_id, asset_class, instrument)
             SELECT id, 'equity', 'single' FROM holdings
             WHERE account_id = ?1 AND isin NOT IN ('DE0001102580', 'DE000A0S9GB0')",
            [acc_ids["demo-einzeltitel"]],
        )?;
        // Crypto depot: no ISIN, the ticker stands in the WKN column; BTC 0,05 × 88.000 € and
        // ETH 0,8 × 2.250 € add up to the account balance (6.200,00 EUR).
        for (wkn, name, quantity, current, purchase, prev_day) in [
            (
                "BTC",
                "Bitcoin",
                0.05_f64,
                88_000_00_i64,
                61_000_00_i64,
                87_100_00_i64,
            ),
            ("ETH", "Ethereum", 0.8, 2_250_00, 2_480_00, 2_270_00),
        ] {
            let value = (quantity * current as f64).round() as i64;
            let day = ((current - prev_day) as f64 * quantity).round() as i64;
            let gain = ((current - purchase) as f64 * quantity).round() as i64;
            tx.execute(
                "INSERT INTO holdings(account_id, isin, wkn, name, quantity, current_price_cents,
                        purchase_price_cents, value_cents, prev_day_price_cents, prev_day_value_cents,
                        profit_loss_purchase_cents, profit_loss_purchase_rel,
                        profit_loss_prev_day_cents, profit_loss_prev_day_rel)
                 VALUES (?1, '', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    acc_ids["demo-krypto"],
                    wkn,
                    name,
                    quantity,
                    current,
                    purchase,
                    value,
                    prev_day,
                    value - day,
                    gain,
                    (current - purchase) as f64 / purchase as f64 * 100.0,
                    day,
                    (current - prev_day) as f64 / prev_day as f64 * 100.0
                ],
            )?;
        }
        // The fixture explicitly models direct BTC/ETH custody; do not infer this from
        // an arbitrary WKN in real accounts.
        tx.execute("INSERT OR REPLACE INTO holding_classifications(holding_id,direct_crypto) SELECT id,1 FROM holdings WHERE account_id=?1", [acc_ids["demo-krypto"]])?;
        // Securities transactions of the depot (#314): the two savings plans of the last
        // twelve months, two distributions and one custody fee.
        {
            use crate::models::{SecurityDirection, SecurityTransaction};
            let depot = acc_ids["demo-depot"];
            let mut n = 0;
            for months_ago in 0..12_i64 {
                let day = today - Duration::days(months_ago * 30 + 3);
                for (isin, name, price) in [
                    (
                        "IE00B4L5Y983",
                        "iShares Core MSCI World UCITS ETF",
                        98_00_i64,
                    ),
                    ("IE00BK5BQT80", "Vanguard FTSE All-World UCITS ETF", 102_00),
                ] {
                    n += 1;
                    let value = 250_00_i64;
                    crate::security_transactions::upsert(
                        &tx,
                        depot,
                        &SecurityTransaction {
                            external_id: format!("demo-st-{n}"),
                            booking_date: day.format("%Y-%m-%d").to_string(),
                            isin: isin.into(),
                            name: name.into(),
                            direction: SecurityDirection::Buy,
                            quantity: value as f64 / price as f64,
                            price_cents: price,
                            currency: "EUR".into(),
                            value_cents: value,
                            booking_text: "Sparplan".into(),
                            ..Default::default()
                        },
                    )?;
                }
            }
            for (months_ago, direction, value, text) in [
                (
                    2_i64,
                    SecurityDirection::Dividend,
                    38_40_i64,
                    "Ausschüttung",
                ),
                (8, SecurityDirection::Dividend, 41_10, "Ausschüttung"),
                (5, SecurityDirection::Fee, 12_50, "Depotentgelt"),
            ] {
                n += 1;
                crate::security_transactions::upsert(
                    &tx,
                    depot,
                    &SecurityTransaction {
                        external_id: format!("demo-st-{n}"),
                        booking_date: (today - Duration::days(months_ago * 30 + 10))
                            .format("%Y-%m-%d")
                            .to_string(),
                        isin: "IE00B4L5Y983".into(),
                        name: "iShares Core MSCI World UCITS ETF".into(),
                        direction,
                        currency: "EUR".into(),
                        value_cents: value,
                        booking_text: text.into(),
                        ..Default::default()
                    },
                )?;
            }
        }
        // More securities activities for the demo (#314): the two depots show buys with fees,
        // sells, distributions with withholding tax, own fee/tax lines, a foreign-currency
        // distribution and an other-kind line. Three world-depot savings-plan buys carry the
        // amount and day of the Giro's "Sparplan" bookings, which they are linked to below.
        {
            use crate::models::{SecurityDirection as D, SecurityTransaction};
            const ISHARES: (&str, &str, &str) = (
                "IE00B4L5Y983",
                "A0RPWH",
                "iShares Core MSCI World UCITS ETF",
            );
            const VANGUARD: (&str, &str, &str) = (
                "IE00BK5BQT80",
                "A2PKXG",
                "Vanguard FTSE All-World UCITS ETF",
            );
            const XTRACKERS: (&str, &str, &str) = (
                "LU0290358497",
                "DBX0AN",
                "Xtrackers II EUR Overnight Rate Swap UCITS ETF",
            );
            const SIEMENS: (&str, &str, &str) = ("DE0007236101", "723610", "Siemens AG");
            const APPLE: (&str, &str, &str) = ("US0378331005", "865985", "Apple Inc.");
            const SAP: (&str, &str, &str) = ("DE0007164600", "716460", "SAP SE");
            const MICROSOFT: (&str, &str, &str) = ("US5949181045", "870747", "Microsoft Corp.");
            const ALLIANZ: (&str, &str, &str) = ("DE0008404005", "840400", "Allianz SE");
            const BUND: (&str, &str, &str) = (
                "DE0001102580",
                "110258",
                "Bundesrepublik Deutschland Anleihe 2032",
            );
            const GOLD: (&str, &str, &str) = ("DE000A0S9GB0", "A0S9GB", "Xetra-Gold");
            struct Row {
                account: &'static str,
                days_ago: i64,
                settle_after: i64,
                direction: D,
                security: (&'static str, &'static str, &'static str),
                quantity: f64,
                price: i64,
                currency: &'static str,
                value: i64,
                fees: i64,
                taxes: i64,
                accrued: i64,
                text: &'static str,
            }
            let row = |account,
                       days_ago,
                       settle_after,
                       direction,
                       security,
                       quantity,
                       price,
                       value,
                       fees,
                       taxes,
                       accrued,
                       text| Row {
                account,
                days_ago,
                settle_after,
                direction,
                security,
                quantity,
                price,
                currency: "EUR",
                value,
                fees,
                taxes,
                accrued,
                text,
            };
            let w = "demo-depot";
            let e = "demo-einzeltitel";
            let mut rows = vec![
                // savings-plan buys that match the Giro's bookings (linked to them below)
                row(
                    w,
                    53,
                    0,
                    D::Buy,
                    ISHARES,
                    5.4,
                    92_59,
                    500_00,
                    0,
                    0,
                    0,
                    "Sparplan",
                ),
                row(
                    w,
                    23,
                    0,
                    D::Buy,
                    ISHARES,
                    4.8,
                    104_17,
                    500_00,
                    0,
                    0,
                    0,
                    "Sparplan",
                ),
                row(
                    w,
                    24,
                    0,
                    D::Buy,
                    VANGUARD,
                    2.3,
                    108_70,
                    250_00,
                    0,
                    0,
                    0,
                    "Sparplan",
                ),
                // world depot: a buy with a fee, a sale with fee and tax, a distribution with
                // withholding tax, the annual advance lump-sum tax and an unlinked fee line
                row(
                    w,
                    100,
                    2,
                    D::Buy,
                    XTRACKERS,
                    10.0,
                    145_00,
                    1_450_00,
                    4_90,
                    0,
                    0,
                    "Kauf",
                ),
                row(
                    w,
                    40,
                    2,
                    D::Sell,
                    XTRACKERS,
                    5.0,
                    146_00,
                    730_00,
                    1_50,
                    0,
                    0,
                    "Verkauf",
                ),
                row(
                    w,
                    62,
                    0,
                    D::Dividend,
                    VANGUARD,
                    0.0,
                    0,
                    62_00,
                    0,
                    16_35,
                    0,
                    "Ausschüttung",
                ),
                row(
                    w,
                    120,
                    0,
                    D::Tax,
                    ISHARES,
                    0.0,
                    0,
                    3_20,
                    0,
                    0,
                    0,
                    "Vorabpauschale",
                ),
                row(
                    w,
                    95,
                    0,
                    D::Fee,
                    ISHARES,
                    0.0,
                    0,
                    0,
                    2_50,
                    0,
                    0,
                    "Fremdkostenpauschale",
                ),
                // single-stock depot: initial purchases at the demo cost prices
                row(
                    e,
                    340,
                    2,
                    D::Buy,
                    SIEMENS,
                    15.0,
                    148_00,
                    2_220_00,
                    4_90,
                    0,
                    0,
                    "Kauf",
                ),
                row(
                    e,
                    300,
                    2,
                    D::Buy,
                    APPLE,
                    12.0,
                    165_00,
                    1_980_00,
                    4_90,
                    0,
                    0,
                    "Kauf",
                ),
                row(
                    e,
                    280,
                    2,
                    D::Buy,
                    SAP,
                    10.0,
                    131_00,
                    1_310_00,
                    4_90,
                    0,
                    0,
                    "Kauf",
                ),
                row(
                    e,
                    200,
                    2,
                    D::Buy,
                    MICROSOFT,
                    6.0,
                    310_00,
                    1_860_00,
                    4_90,
                    0,
                    0,
                    "Kauf",
                ),
                row(
                    e,
                    150,
                    2,
                    D::Buy,
                    ALLIANZ,
                    8.0,
                    232_50,
                    1_860_00,
                    4_90,
                    0,
                    0,
                    "Kauf",
                ),
                row(
                    e,
                    120,
                    2,
                    D::Buy,
                    BUND,
                    20.0,
                    99_20,
                    1_984_00,
                    4_90,
                    0,
                    18_40,
                    "Kauf",
                ),
                row(
                    e,
                    90,
                    2,
                    D::Buy,
                    GOLD,
                    30.0,
                    52_40,
                    1_572_00,
                    4_90,
                    0,
                    0,
                    "Kauf",
                ),
                // a partial sale with its own tax line, distributions, a custody fee
                row(
                    e,
                    45,
                    2,
                    D::Sell,
                    APPLE,
                    3.0,
                    205_00,
                    615_00,
                    4_90,
                    0,
                    0,
                    "Verkauf",
                ),
                row(
                    e,
                    45,
                    0,
                    D::Tax,
                    APPLE,
                    0.0,
                    0,
                    36_50,
                    0,
                    0,
                    0,
                    "Kapitalertragsteuer",
                ),
                row(
                    e,
                    255,
                    0,
                    D::Dividend,
                    SIEMENS,
                    15.0,
                    4_70,
                    70_50,
                    0,
                    18_59,
                    0,
                    "Dividende",
                ),
                row(
                    e,
                    190,
                    0,
                    D::Dividend,
                    SAP,
                    10.0,
                    2_35,
                    23_50,
                    0,
                    6_20,
                    0,
                    "Dividende",
                ),
                row(
                    e,
                    175,
                    0,
                    D::Dividend,
                    ALLIANZ,
                    8.0,
                    13_80,
                    110_40,
                    0,
                    29_12,
                    0,
                    "Dividende",
                ),
                row(
                    e,
                    60,
                    0,
                    D::Fee,
                    SIEMENS,
                    0.0,
                    0,
                    12_50,
                    0,
                    0,
                    0,
                    "Depotentgelt",
                ),
                row(
                    e,
                    20,
                    0,
                    D::Other,
                    MICROSOFT,
                    0.0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    "Kapitalmaßnahme",
                ),
            ];
            // a foreign-currency distribution: shown, but never summed into the euro figures
            let mut usd = row(
                e,
                70,
                0,
                D::Dividend,
                MICROSOFT,
                6.0,
                83,
                4_98,
                0,
                0,
                0,
                "Dividende",
            );
            usd.currency = "USD";
            rows.push(usd);
            let mut n = 1000;
            for r in rows {
                n += 1;
                let day = |d: i64| (today - Duration::days(d)).format("%Y-%m-%d").to_string();
                crate::security_transactions::upsert(
                    &tx,
                    acc_ids[r.account],
                    &SecurityTransaction {
                        external_id: format!("demo-st-{n}"),
                        booking_date: day(r.days_ago),
                        settlement_date: if r.settle_after > 0 {
                            day(r.days_ago - r.settle_after)
                        } else {
                            String::new()
                        },
                        isin: r.security.0.into(),
                        wkn: r.security.1.into(),
                        name: r.security.2.into(),
                        direction: r.direction,
                        quantity: r.quantity,
                        price_cents: r.price,
                        currency: r.currency.into(),
                        value_cents: r.value,
                        fees_cents: r.fees,
                        taxes_cents: r.taxes,
                        accrued_interest_cents: r.accrued,
                        fx_rate: (r.currency == "USD").then_some(1.08),
                        booking_text: r.text.into(),
                        ..Default::default()
                    },
                )?;
            }
        }
        // Valuation history of the three depots (`depot_values`), deterministic. Each depot shows a
        // case the chart has to handle:
        // - Welt-Depot: a year of fetched values, no gaps.
        // - Einzeltitel-Depot: connected only 120 days ago, so the sum of the depots is partial
        //   before that ("2 von 3 Depots") and the line breaks where it joins.
        // - Krypto-Depot: gaps of one to three days (the last value carries on), and an older
        //   stretch from an import; the fetched values win where both exist.
        {
            let now = crate::user_groups::now_ms();
            for (key, start, vol, weekends, seed) in [
                ("demo-depot", 0.55_f64, 0.0065_f64, false, 0x51F1_5EED_u64),
                ("demo-einzeltitel", 0.72, 0.011, false, 0xA11C_E5ED),
                ("demo-krypto", 0.60, 0.03, true, 0xC0FF_EE5D),
            ] {
                let id = acc_ids[key];
                let end: i64 = tx.query_row(
                    "SELECT balance_cents FROM accounts WHERE id = ?1",
                    [id],
                    |r| r.get(0),
                )?;
                let levels = demo_depot_levels(seed, start, vol, weekends, today);
                let value_at =
                    |ago: i64| -> i64 { (end as f64 * levels[ago as usize]).round() as i64 };
                let day = |ago: i64| (today - Duration::days(ago)).format("%Y-%m-%d").to_string();
                for ago in (0..=365_i64).rev() {
                    match key {
                        "demo-einzeltitel" if ago > 120 => continue,
                        // gaps: 2 days out of every 9, plus a single day now and then
                        "demo-krypto"
                            if ago > 0
                                && ago < 365
                                && (ago % 9 == 4 || ago % 9 == 5 || ago % 31 == 7) =>
                        {
                            continue
                        }
                        _ => {}
                    }
                    if key == "demo-krypto" && ago > 200 {
                        // imported history; deliberately a little off where it overlaps the fetches
                        crate::depot_values::upsert_from(
                            &tx,
                            id,
                            &day(ago),
                            value_at(ago),
                            now,
                            "import",
                            "observed",
                        )?;
                        continue;
                    }
                    crate::depot_values::upsert(&tx, id, &day(ago), value_at(ago), now)?;
                }
                if key == "demo-krypto" {
                    // overlap: import values 2 % higher for the days 200..=190 that fetches also cover
                    for ago in 190..=200_i64 {
                        crate::depot_values::upsert_from(
                            &tx,
                            id,
                            &day(ago),
                            (value_at(ago) as f64 * 1.02).round() as i64,
                            now,
                            "import",
                            "observed",
                        )?;
                    }
                }
            }
        }
        // manual loan without a bank account (#36): Stand two months ago, rates from the Giro
        // (linked by `apply_rules_inner` below)
        let car = crate::loans::create_in(
            &tx,
            &crate::loans::LoanSetup {
                name: "Autokredit".into(),
                original_cents: Some(15_000_00),
                anchor_date: (today - Duration::days(60)).format("%Y-%m-%d").to_string(),
                anchor_balance_cents: 9_400_00,
                interest_rate_ppm: Some(49_900),
                match_expr: "Autobank".into(),
            },
        )?;
        acc_ids.insert("demo-autokredit", car);
        // Sachwerte (#87): gold and a car; gold prices come from the API
        let sachwerte = crate::assets::seed_demo(&tx, today)?;
        acc_ids.insert("demo-sachwerte", sachwerte);
        // user groups for the "Nach Gruppe" sidebar view
        for (name, sym, refs) in [
            (
                "Alltag",
                "person",
                &[
                    "demo-giro",
                    "demo-visa",
                    "demo-paypal",
                    "demo-amazon",
                    "demo-giro2",
                    "demo-autokredit",
                ][..],
            ),
            (
                "Sparen & Vorsorge",
                "chart.pie",
                &[
                    "demo-tagesgeld",
                    "demo-tagesgeld2",
                    "demo-depot",
                    "demo-einzeltitel",
                    "demo-krypto",
                    "demo-sachwerte",
                ][..],
            ),
            ("Haus", "house", &["demo-darlehen"][..]),
        ] {
            let (uuid, _) = crate::user_groups::find_or_create(&tx, crate::i18n::tr(name), sym)?;
            for r in refs {
                crate::user_groups::assign(&tx, acc_ids[r], Some(&uuid), 0)?;
            }
        }
        let mut n = 0u32;
        {
            let mut ins = tx.prepare(
                "INSERT INTO transactions(account_id, external_id, booking_date, value_date, amount_cents, currency, counterparty,
                                          counterparty_iban, purpose, booking_text, checked)
                 VALUES (?1, ?2, ?3, ?3, ?4, 'EUR', ?5, ?6, ?7, ?8, ?9)",
            )?;
            for (i, t) in txs.iter().enumerate() {
                let date = t.date.format("%Y-%m-%d").to_string();
                let checked = (today - t.date).num_days() > 10;
                ins.execute(params![
                    acc_ids[t.account],
                    format!("demo-{i}"),
                    date,
                    t.cents,
                    t.counterparty,
                    "",
                    t.purpose,
                    t.booking_text,
                    checked as i64
                ])?;
                n += 1;
            }
            // A returned direct debit (#143): Jan Muster's account was closed. Matches
            // his mandate, so the mandate list and inspector show the return hint.
            tx.execute(
                "INSERT INTO transactions(account_id, external_id, booking_date, value_date, amount_cents, currency, counterparty,
                                          counterparty_iban, purpose, booking_text, checked, mandate_id, end_to_end_id)
                 VALUES (?1, 'demo-return', ?2, ?2, -1250, 'EUR', 'Jan Muster', 'DE02120300000000202051',
                         'Rücklastschrift AC04 Mitgliedsbeitrag', 'RÜCKLASTSCHRIFT', 0, 'M-2025-014', 'M-2025-014-20261001')",
                params![
                    acc_ids["demo-giro"],
                    (today - Duration::days(5)).format("%Y-%m-%d").to_string()
                ],
            )?;
            n += 1;
        }
        // Startsaldo (#212): the card reports no balance; it is computed from the
        // bookings, starting with the statement balance at the start of the
        // month three months ago, calibrated to today's balance of the spec.
        let visa = acc_ids["demo-visa"];
        tx.execute(
            "UPDATE accounts SET balance_derived = 1 WHERE id = ?1",
            [visa],
        )?;
        let since = (today - Duration::days(90)).with_day(1).unwrap_or(today);
        crate::opening_balance::store(&tx, visa, Some(0), &since.format("%Y-%m-%d").to_string())?;
        let card = DEMO_ACCOUNTS
            .iter()
            .find(|s| s.external_ref == "demo-visa")
            .map_or(0, |s| s.opening_cents);
        crate::opening_balance::calibrate_in(&tx, visa, card)?;
        // The three savings-plan buys above meet the Giro's "Sparplan" bookings (same amount and
        // day). The depot sits on its own login, which the automatic matching does not cross, so
        // the demo sets the link the way a connector with a cash reference does.
        tx.execute(
            "UPDATE security_transactions SET cash_transaction_id = (
                 SELECT t.id FROM transactions t
                  WHERE t.account_id = ?1 AND t.booking_date = security_transactions.booking_date
                    AND t.amount_cents = -(security_transactions.value_cents + security_transactions.fees_cents)
                    AND t.purpose = 'Sparplan' LIMIT 1)
             WHERE account_id = ?2 AND external_id IN ('demo-st-1001', 'demo-st-1002', 'demo-st-1003')",
            params![acc_ids["demo-giro"], acc_ids["demo-depot"]],
        )?;
        Vault::apply_rules_inner(&tx, true)?;
        demo_documents(&tx, today)?;
        demo_tags(&tx)?;
        tx.commit()?;
        Ok(n)
    }
}

/// A deterministic, natural-looking path of a depot's value over the last 365 days: `levels[ago]` is
/// the value `ago` days back relative to today (`levels[0]` is 1.0, so the history ends exactly at
/// the depot's balance). Daily moves are random with a volatility that changes every few weeks,
/// now and then a sharp drop that recovers a little, a drift towards today's value (`start` is the
/// share of today's value a year back), and standing values on weekends unless `weekends`.
fn demo_depot_levels(
    seed: u64,
    start: f64,
    vol: f64,
    weekends: bool,
    today: NaiveDate,
) -> Vec<f64> {
    use chrono::Weekday;
    let mut state = seed;
    let mut next = move || -> f64 {
        // splitmix64: small, deterministic, good enough for demo data
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64
    };
    const DAYS: usize = 365;
    let trades = |ago: usize| {
        weekends
            || !matches!(
                (today - Duration::days(ago as i64)).weekday(),
                Weekday::Sat | Weekday::Sun
            )
    };
    let active = (0..=DAYS).filter(|&a| trades(a)).count().max(1);
    let drift = -start.ln() / active as f64;
    // log value from a year back (index DAYS) to today (index 0), before scaling
    let mut w = vec![0.0_f64; DAYS + 1];
    let (mut regime, mut regime_left, mut rebound) = (1.0_f64, 0_u32, 0_u32);
    for ago in (0..DAYS).rev() {
        let mut r = 0.0;
        if trades(ago) {
            if regime_left == 0 {
                regime = 0.55 + 1.5 * next();
                regime_left = 15 + (next() * 40.0) as u32;
            }
            regime_left -= 1;
            let gauss = (next() + next() + next() + next() - 2.0) * 3.0_f64.sqrt();
            r = drift + regime * vol * gauss;
            if next() < 0.007 {
                r -= (3.0 + 4.0 * next()) * vol;
                rebound = 6;
            } else if rebound > 0 {
                r += 0.5 * vol;
                rebound -= 1;
            }
        }
        w[ago] = w[ago + 1] + r;
    }
    let today_log = w[0];
    w.iter().map(|x| (x - today_log).exp()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merchants::FlowKind;
    use crate::models::BookingFilter;

    #[test]
    fn demo_documents_cover_accounts_origins_and_links_with_readable_pdfs() {
        let dir = crate::test_support::TempDir::new("demo-documents");
        let v = Vault::open(dir.join("v.db").to_string_lossy().into(), vec![7u8; 32]).unwrap();
        v.seed_demo_data().unwrap();
        let docs = v
            .list_all_documents(crate::documents::DocumentFilter::default())
            .unwrap();
        assert_eq!(docs.len(), 9);
        let accounts: std::collections::HashSet<_> = docs.iter().map(|d| d.account_id).collect();
        assert_eq!(accounts.len(), 3);
        for bank in [false, true] {
            for linked in [false, true] {
                assert!(docs
                    .iter()
                    .any(|d| !d.source_url.is_empty() == bank && d.booking_id.is_some() == linked));
            }
        }
        let today = Local::now().date_naive();
        for d in &docs {
            let date = NaiveDate::parse_from_str(&d.doc_date, "%Y-%m-%d").unwrap();
            assert!((0..=30).contains(&(today - date).num_days()));
            let bytes = v.get_document_bytes(d.id).unwrap();
            assert!(bytes.starts_with(b"%PDF"));
            let text = crate::documents::extract_pdf_text(&bytes);
            assert!(text.contains(&d.account_name) && text.contains(&d.doc_date));
            if let Some(booking) = d.booking_id {
                let account: i64 = v
                    .db()
                    .unwrap()
                    .query_row(
                        "SELECT account_id FROM transactions WHERE id = ?1",
                        [booking],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(d.account_id, Some(account));
            }
        }
        // Reopening / seeding an existing demo must not duplicate uploads.
        v.seed_demo_data().unwrap();
        assert_eq!(
            v.list_all_documents(crate::documents::DocumentFilter::default())
                .unwrap()
                .len(),
            9
        );
    }

    #[test]
    fn direction_filter_splits_income_and_expenses() {
        use crate::models::BookingDirection;
        let dir_dir = crate::test_support::TempDir::new("girocrab-test");
        let dir = dir_dir.join("v.db");
        let v = Vault::open(dir.to_string_lossy().to_string(), vec![7u8; 32]).unwrap();
        v.seed_demo_data().unwrap();
        let page = |direction| {
            v.list_transactions(BookingFilter {
                direction,
                ..Default::default()
            })
            .unwrap()
        };
        let (all, income, expense) = (
            page(BookingDirection::All),
            page(BookingDirection::Income),
            page(BookingDirection::Expense),
        );
        assert!(income.total_count > 0 && expense.total_count > 0);
        assert!(income.transactions.iter().all(|t| t.amount_cents > 0));
        assert!(expense.transactions.iter().all(|t| t.amount_cents < 0));
        // zero-amount bookings belong to neither side
        let zero = all
            .transactions
            .iter()
            .filter(|t| t.amount_cents == 0)
            .count() as u32;
        assert_eq!(
            income.total_count + expense.total_count + zero,
            all.total_count
        );
        // page totals follow the filter
        assert_eq!(income.income_cents, all.income_cents);
        assert_eq!(income.expense_cents, 0);
        assert_eq!(expense.expense_cents, all.expense_cents);
        assert_eq!(expense.income_cents, 0);
        // the CSV export uses the same filter
        let csv = v
            .export_bookings_csv(BookingFilter {
                direction: BookingDirection::Income,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(csv.lines().count() as u32, income.total_count + 1);
    }

    #[test]
    fn demo_depot_has_positions_matching_its_balance() {
        let dir_dir = crate::test_support::TempDir::new("girocrab-test");
        let dir = dir_dir.join("v.db");
        let v = Vault::open(dir.to_string_lossy().to_string(), vec![7u8; 32]).unwrap();
        v.seed_demo_data().unwrap();
        let depot = v
            .list_accounts()
            .unwrap()
            .into_iter()
            .find(|a| a.name == "Welt-Depot ETF")
            .unwrap();
        let holdings = v.list_holdings(depot.id).unwrap();
        assert_eq!(holdings.len(), 3);
        let sum: i64 = holdings.iter().map(|h| h.value_cents).sum();
        assert_eq!(sum, depot.balance_cents);
        // the ETFs by rule, the single stocks of the second depot as its broker reports them (#351)
        use crate::asset_classification::{AssetClass, ClassificationSource, InstrumentKind};
        let etfs = v.classified_assets(depot.id).unwrap();
        assert!(etfs
            .iter()
            .all(|a| a.classification.instrument == InstrumentKind::Etf));
        // same wrapper, different exposure: two equity ETFs and one money market ETF
        let classes = |class: AssetClass| {
            etfs.iter()
                .filter(|a| a.classification.asset_class == class)
                .count()
        };
        assert_eq!(classes(AssetClass::Equity), 2);
        assert_eq!(classes(AssetClass::Cash), 1);
        let stocks = v
            .list_accounts()
            .unwrap()
            .into_iter()
            .find(|a| a.name == "Einzeltitel-Depot")
            .unwrap();
        let singles = v.classified_assets(stocks.id).unwrap();
        assert_eq!(singles.len(), 7);
        let sum: i64 = singles.iter().map(|a| a.holding.value_cents).sum();
        assert_eq!(sum, stocks.balance_cents);
        assert!(singles
            .iter()
            .all(|a| a.classification.security_key.is_some()));
        let of = |class: AssetClass, instrument: InstrumentKind, source: ClassificationSource| {
            singles
                .iter()
                .filter(|a| {
                    a.classification.asset_class == class
                        && a.classification.instrument == instrument
                        && a.classification.asset_class_source == source
                })
                .count()
        };
        use ClassificationSource::{Import, Rule};
        assert_eq!(
            of(AssetClass::Equity, InstrumentKind::SingleSecurity, Import),
            5
        );
        assert_eq!(
            of(AssetClass::Bond, InstrumentKind::SingleSecurity, Rule),
            1
        );
        assert_eq!(of(AssetClass::Commodity, InstrumentKind::EtcEtn, Rule), 1);
    }

    #[test]
    fn category_totals_follow_account_and_status_filters() {
        let dir_dir = crate::test_support::TempDir::new("girocrab-test");
        let dir = dir_dir.join("v.db");
        let v = Vault::open(dir.to_string_lossy().to_string(), vec![7u8; 32]).unwrap();
        v.seed_demo_data().unwrap();
        let sum = |cats: &[crate::Category]| -> (i64, u32) {
            cats.iter().filter(|c| c.depth == 0).fold((0, 0), |a, c| {
                (a.0 + c.total_cents, a.1 + c.transaction_count)
            })
        };
        let all = v.list_categories(None, None).unwrap();
        // no filter = same as the plain listing
        let same = v
            .list_categories_filtered(None, None, vec![], None, crate::BookingDirection::All)
            .unwrap();
        assert_eq!(sum(&all), sum(&same));
        // account filter: the parts add up to the whole
        let accounts = v.list_accounts().unwrap();
        let (mut total, mut count) = (0, 0);
        for a in &accounts {
            let part = v
                .list_categories_filtered(
                    None,
                    None,
                    vec![a.id],
                    None,
                    crate::BookingDirection::All,
                )
                .unwrap();
            let (t, n) = sum(&part);
            total += t;
            count += n;
        }
        // uncategorised bookings are not part of the tree, so the parts never exceed the whole
        assert_eq!((total, count), sum(&all));
        // status filter: checked + new = everything
        let (t1, n1) = sum(&v
            .list_categories_filtered(None, None, vec![], Some(true), crate::BookingDirection::All)
            .unwrap());
        let (t0, n0) = sum(&v
            .list_categories_filtered(
                None,
                None,
                vec![],
                Some(false),
                crate::BookingDirection::All,
            )
            .unwrap());
        assert_eq!((t1 + t0, n1 + n0), sum(&all));
        // direction filter: income + expenses = everything (no zero-amount bookings in the demo)
        let inc = sum(&v
            .list_categories_filtered(None, None, vec![], None, crate::BookingDirection::Income)
            .unwrap());
        let exp = sum(&v
            .list_categories_filtered(None, None, vec![], None, crate::BookingDirection::Expense)
            .unwrap());
        assert!(inc.0 > 0 && exp.0 < 0);
        assert_eq!((inc.0 + exp.0, inc.1 + exp.1), sum(&all));
        // marking everything checked moves all bookings to the "checked" side
        let ids: Vec<i64> = v
            .list_transactions(BookingFilter {
                limit: 1000,
                ..Default::default()
            })
            .unwrap()
            .transactions
            .iter()
            .map(|b| b.id)
            .collect();
        v.set_checked_for_ids(ids, true).unwrap();
        let (_, only_new) = sum(&v
            .list_categories_filtered(
                None,
                None,
                vec![],
                Some(false),
                crate::BookingDirection::All,
            )
            .unwrap());
        assert_eq!(only_new, 0);
        assert_eq!(
            sum(&v
                .list_categories_filtered(
                    None,
                    None,
                    vec![],
                    Some(true),
                    crate::BookingDirection::All
                )
                .unwrap()),
            sum(&all)
        );
    }

    #[test]
    fn demo_seed_tags_bookings_across_categories() {
        let dir = crate::test_support::TempDir::new("girocrab-test");
        let v = Vault::open(
            dir.join("v.db").to_string_lossy().to_string(),
            vec![7u8; 32],
        )
        .unwrap();
        v.seed_demo_data().unwrap();
        let tags: Vec<(String, u32)> = v
            .list_tags()
            .unwrap()
            .into_iter()
            .map(|t| (t.name, t.booking_count))
            .collect();
        let names: Vec<&str> = tags.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names.len(), DEMO_TAGS.len());
        assert_eq!(names[..3], ["Auto", "Urlaub", "Familie"]);
        // every tag with patterns finds bookings; the last two stay unused
        for ((name, n), (_, _, cps, purposes)) in tags.iter().zip(DEMO_TAGS.iter()) {
            assert_eq!(
                *n > 0,
                !cps.is_empty() || !purposes.is_empty(),
                "{name}: {n}"
            );
        }
        // only the rental car carries two tags
        let all = v
            .list_transactions(Default::default())
            .unwrap()
            .transactions;
        let doubles: Vec<_> = all.iter().filter(|b| b.tags.len() > 1).collect();
        assert!(
            doubles.iter().all(|b| b.counterparty.starts_with("Sixt")),
            "{doubles:?}"
        );
        // the rental car carries two tags
        let both = v
            .list_transactions(crate::models::BookingFilter {
                tag_ids: v.list_tags().unwrap()[..2].iter().map(|t| t.id).collect(),
                ..Default::default()
            })
            .unwrap()
            .transactions;
        assert!(both.iter().any(|b| b.counterparty.starts_with("Sixt")));
        assert!(both.iter().all(|b| b.tags.len() >= 2));
    }

    #[test]
    fn demo_depots_have_activities_and_linked_savings_plan_buys() {
        use crate::models::SecurityDirection as D;
        let dir_dir = crate::test_support::TempDir::new("girocrab-test");
        let v = Vault::open(
            dir_dir.join("v.db").to_string_lossy().to_string(),
            vec![7u8; 32],
        )
        .unwrap();
        v.seed_demo_data().unwrap();
        let accounts = v.list_accounts().unwrap();
        let id = |name: &str| accounts.iter().find(|a| a.name == name).unwrap().id;
        let world = v.list_security_transactions(id("Welt-Depot ETF")).unwrap();
        let singles = v
            .list_security_transactions(id("Einzeltitel-Depot"))
            .unwrap();
        for list in [&world, &singles] {
            for d in [D::Buy, D::Sell, D::Dividend, D::Fee, D::Tax] {
                assert!(list.iter().any(|t| t.direction == d), "{d:?}");
            }
        }
        assert!(singles.iter().any(|t| t.currency == "USD"));
        assert!(singles.iter().any(|t| t.direction == D::Other));
        // exactly the three matching savings-plan buys point to a Giro booking
        assert_eq!(
            world
                .iter()
                .filter(|t| t.cash_transaction_id.is_some())
                .count(),
            3
        );
        assert!(singles.iter().all(|t| t.cash_transaction_id.is_none()));
    }

    #[test]
    fn the_demo_value_paths_end_at_today_and_look_like_markets() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 17).unwrap(); // a Tuesday
        let calm = demo_depot_levels(7, 0.55, 0.0065, false, today);
        assert_eq!(calm.len(), 366);
        assert_eq!(calm[0], 1.0, "ends exactly at today's value");
        assert_eq!(
            calm,
            demo_depot_levels(7, 0.55, 0.0065, false, today),
            "deterministic"
        );
        // standing values on weekends (13 Nov 2026 is a Friday: 14th and 15th repeat it)
        assert_eq!(calm[3], calm[4]);
        assert_eq!(calm[2], calm[3]);
        // no market on weekends is a stretch of equal values; crypto trades every day
        let crypto = demo_depot_levels(9, 0.6, 0.03, true, today);
        assert_ne!(crypto[3], crypto[4]);
        // it moves: many changes of direction, no jump of a fifth in one day, a plausible start
        for path in [&calm, &crypto] {
            let moves: Vec<f64> = path.windows(2).map(|w| w[0].ln() - w[1].ln()).collect();
            let turns = moves.windows(2).filter(|m| m[0] * m[1] < 0.0).count();
            assert!(turns > 80, "{turns} turns");
            assert!(moves.iter().all(|m| m.abs() < 0.2));
            assert!(path[365] > 0.2 && path[365] < 1.5, "start {}", path[365]);
        }
    }

    #[test]
    fn demo_depots_show_partial_coverage_gaps_and_an_import_stretch() {
        let dir_dir = crate::test_support::TempDir::new("girocrab-test");
        let v = Vault::open(
            dir_dir.join("v.db").to_string_lossy().to_string(),
            vec![7u8; 32],
        )
        .unwrap();
        v.seed_demo_data().unwrap();
        let id = |name: &str| {
            v.list_accounts()
                .unwrap()
                .into_iter()
                .find(|a| a.name == name)
                .unwrap()
                .id
        };
        let (welt, einzel, krypto) = (
            id("Welt-Depot ETF"),
            id("Einzeltitel-Depot"),
            id("Krypto-Depot"),
        );
        // all three: the last value is today's, the history starts with the first known day
        let all = v.depot_value_history(vec![], 400).unwrap();
        assert_eq!(
            all.first().unwrap().covered_depots,
            2,
            "partial before the third joins"
        );
        assert_eq!(all.last().unwrap().covered_depots, 3);
        assert!(all.iter().all(|p| p.total_depots == 3));
        // the Einzeltitel-Depot joins 120 days ago: coverage rises by one exactly there
        let joins = all
            .windows(2)
            .filter(|w| w[1].covered_depots > w[0].covered_depots)
            .count();
        assert_eq!(joins, 1);
        let alone = v.depot_value_history(vec![einzel], 400).unwrap();
        assert_eq!(alone.len(), 121);
        // each depot ends at its balance
        for (name, depot) in [("Welt", welt), ("Einzeltitel", einzel), ("Krypto", krypto)] {
            let acc = v
                .list_accounts()
                .unwrap()
                .into_iter()
                .find(|a| a.id == depot)
                .unwrap();
            let h = v.depot_value_history(vec![depot], 400).unwrap();
            assert_eq!(h.last().unwrap().value_cents, acc.balance_cents, "{name}");
        }
        // the Welt-Depot has a value for every day of the year, the Krypto-Depot has gaps in its
        // fetched stretch (the history still carries the last value on)
        assert_eq!(v.depot_value_history(vec![welt], 400).unwrap().len(), 366);
        let db = v.db().unwrap();
        let fetched: i64 = db
            .query_row(
                "SELECT count(*) FROM depot_values WHERE account_id = ?1 AND source = 'fetch'",
                [krypto],
                |r| r.get(0),
            )
            .unwrap();
        let imported: i64 = db
            .query_row(
                "SELECT count(*) FROM depot_values WHERE account_id = ?1 AND source = 'import'",
                [krypto],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            fetched > 100 && fetched < 201,
            "gaps in the fetched stretch: {fetched}"
        );
        assert!(imported > 100, "an imported stretch exists: {imported}");
    }

    #[test]
    fn demo_seed_has_thirteen_accounts_and_exactly_sixty_nine_bookings() {
        let dir_dir = crate::test_support::TempDir::new("girocrab-test");
        let dir = dir_dir.join("v.db");
        let v = Vault::open(dir.to_string_lossy().to_string(), vec![7u8; 32]).unwrap();
        assert_eq!(v.seed_demo_data().unwrap(), 69);
        assert_eq!(v.account_count().unwrap(), 13);
        let car = v.list_manual_loans().unwrap();
        assert_eq!(car.len(), 1);
        assert_eq!(car[0].status.payment_count, 2);
        assert!(car[0].status.remaining_cents < 9_400_00 - 2 * 250_00);
        let accounts = v.list_accounts().unwrap();
        let loan = accounts
            .iter()
            .find(|a| a.name == "Immobilien-Darlehen")
            .unwrap();
        assert_eq!(loan.balance_cents, -182_500_00);
        assert_eq!(loan.initial_balance_cents, Some(250_000_00));
        let giro = accounts
            .iter()
            .find(|a| a.name == "Musterbank Girokonto")
            .unwrap();
        assert_eq!(giro.balance_cents, 4_280_50);
        assert!(!giro.balance_derived);
        // #212: the card's balance is computed from a Startsaldo plus bookings
        let visa = accounts
            .iter()
            .find(|a| a.name == "Visa Gold Kreditkarte")
            .unwrap();
        assert!(visa.balance_derived);
        assert_eq!(visa.balance_cents, -642_80);
        assert!(visa.opening_balance_date.ends_with("-01"));
        assert!(visa.opening_balance_cents.is_some());
        // Loan view also shows the Giro's instalments (linked via the account number).
        let page = v
            .list_transactions(BookingFilter {
                account_ids: vec![loan.id],
                limit: 100,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(page.total_count, 2);
        let cats = v.list_categories(None, None).unwrap();
        assert!(cats
            .iter()
            .any(|c| c.name == "Supermarkt" && c.transaction_count > 0));
        let page = v
            .list_transactions(BookingFilter {
                search: Some("Lidl".into()),
                limit: 50,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(page.total_count, 2);
        assert!(page
            .transactions
            .iter()
            .all(|t| t.category_name == "Supermarkt" && t.flow == FlowKind::Consumption));
        // Private persons get a monogram, never a logo.
        let p = v
            .list_transactions(BookingFilter {
                search: Some("Laura Schneider".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(p.transactions[0].merchant.monogram, "LS");
        // A fresh personal vault gets categories + rules, but no bookings.
        let p2_dir = crate::test_support::TempDir::new("girocrab-test-own");
        let p2 = p2_dir.join("v.db");
        let own = Vault::open(p2.to_string_lossy().to_string(), vec![9u8; 32]).unwrap();
        assert!(own.seed_starter_categories().unwrap() > 30);
        assert_eq!(own.seed_starter_categories().unwrap(), 0);
        assert_eq!(own.account_count().unwrap(), 0);
        assert!(!own.has_account_history().unwrap());
        assert!(v.has_account_history().unwrap());
        let _ = std::fs::remove_file(&p2);
        // wrong key is rejected
        drop(v);
        let wrong = Vault::open(dir.to_string_lossy().to_string(), vec![8u8; 32]);
        assert!(matches!(wrong, Err(crate::error::CoreError::WrongKey)));
        let _ = std::fs::remove_file(&dir);
    }

    #[test]
    fn default_names_are_shown_in_english_but_stored_in_german() {
        use crate::i18n::{with_language, Lang};
        let dir_dir = crate::test_support::TempDir::new("girocrab-i18n");
        let dir = dir_dir.join("v.db");
        let v = Vault::open(dir.to_string_lossy().to_string(), vec![5u8; 32]).unwrap();
        v.seed_demo_data().unwrap();
        let groups_en = with_language(Lang::En, || v.list_account_groups().unwrap());
        assert!(groups_en.iter().any(|g| g.name == "Checking & savings"));
        assert!(groups_en.iter().any(|g| g.name == "Loans"));
        let de = v.list_categories(None, None).unwrap();
        let en = with_language(Lang::En, || v.list_categories(None, None).unwrap());
        // Every starter category has an English name.
        for (d, e) in de.iter().zip(&en) {
            assert_eq!(d.id, e.id);
            assert_ne!(d.name, e.name, "{} is not translated", d.name);
        }
        let sm = en.iter().find(|c| c.name == "Supermarket").unwrap();
        assert_eq!(sm.path, "Food › Supermarket");
        // Classification still sees the stored German names.
        let filter = || BookingFilter {
            limit: 100,
            ..Default::default()
        };
        let page_de = v.list_transactions(filter()).unwrap();
        let page_en = with_language(Lang::En, || v.list_transactions(filter()).unwrap());
        let flows = |p: &crate::models::BookingPage| {
            p.transactions.iter().map(|t| t.flow).collect::<Vec<_>>()
        };
        assert_eq!(flows(&page_de), flows(&page_en));
        assert!(page_en.transactions.iter().any(|t| t.category_path
            == "Saving & investing › ETF savings plans"
            && t.flow == FlowKind::Investment));
        // Saving the unchanged English display name keeps the German default;
        // a real rename is stored as written.
        let sm_de = de.iter().find(|c| c.name == "Supermarkt").unwrap();
        with_language(Lang::En, || {
            v.update_category(
                sm_de.id,
                "Supermarket".into(),
                sm_de.parent_id,
                sm_de.sf_symbol.clone(),
                sm_de.color_hex.clone(),
            )
            .unwrap()
        });
        let after = v.list_categories(None, None).unwrap();
        assert!(after
            .iter()
            .any(|c| c.id == sm_de.id && c.name == "Supermarkt"));
        with_language(Lang::En, || {
            v.update_category(
                sm_de.id,
                "Groceries".into(),
                sm_de.parent_id,
                sm_de.sf_symbol.clone(),
                sm_de.color_hex.clone(),
            )
            .unwrap()
        });
        let after = v.list_categories(None, None).unwrap();
        assert!(after
            .iter()
            .any(|c| c.id == sm_de.id && c.name == "Groceries"));
        // Settings: `group` stays a stable id, `group_title` is localised.
        let s = with_language(Lang::En, || v.list_settings().unwrap());
        let days = s.iter().find(|e| e.key == "sync.days_back").unwrap();
        assert_eq!(days.group, "Allgemein");
        assert_eq!(days.group_title, "General");
        assert_eq!(days.label, "Days on first sync");
        drop(v);
        let _ = std::fs::remove_file(&dir);
    }
}
