//! Records and enums shared with Swift. Dates are ISO `YYYY-MM-DD` strings,
//! money is always `i64` cents.

/// The supported institutions. The connector implementation is picked
/// from this value; endpoints/BLZ are listed in `bank_catalog()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ConnectorKind {
    /// Volksbank Kassel Göttingen, FinTS 3.0, BLZ 52090000
    VolksbankKasselGoettingen,
    /// Any other FinTS 3.0 PIN/TAN bank. The BLZ lives in the connection's
    /// connector key `fints:<blz>`, its URL in the setting `fints.url.<blz>`.
    FinTsGeneric,
    /// 1822direkt, FinTS 3.0, BLZ 50050201
    Direkt1822,
    /// comdirect official REST API
    Comdirect,
    /// PayPal Transaction Search REST API
    PayPal,
    /// Trade Republic WebSocket API (+ 2FA code)
    TradeRepublic,
    /// American Express via Enable Banking PSD2 aggregation
    AmericanExpress,
    /// Amazon Visa via Enable Banking PSD2 aggregation
    AmazonVisa,
    /// bunq public API with the user's own API key
    Bunq,
    /// Any other bank via Enable Banking PSD2 aggregation (N26, Revolut, …).
    /// The bank lives in the connection's connector key
    /// `enablebanking:<country>:<ASPSP name>`.
    EnableBanking,
    /// Scalable Capital Broker / Overnight / Fixed-Term via the GraphQL API
    /// of the official CLI (OAuth device flow, DPoP-bound tokens).
    ScalableCapital,
    /// Amazon customer account (order history) via embedded web login
    AmazonOrders,
    /// FNZ / ebase / finvesto portal: account/depot values and mailbox PDFs.
    Fnz,
    /// Interactive Brokers via the Flex Web Service (token + query id).
    Ibkr,
    /// quirion portal API, authenticated inside the embedded browser.
    Quirion,
    /// easybank (formerly Barclays Deutschland) credit cards and statements, see #209.
    Easybank,
    /// Alte Leipziger (Bauspar) via the fin4u customer portal API
    AlteLeipziger,
    /// Volkswagen Bank online banking (photoTAN / pushTAN) via the web portal.
    VolkswagenBank,
    /// Uberspace hosting accounts (balance and bookings per Asteroid) via the web dashboard.
    Uberspace,
    /// Bausparkasse Schwäbisch Hall (Bausparverträge, postbox PDFs) via the
    /// "Mein Konto" portal, authenticated inside the embedded browser (#290).
    SchwaebischHall,
}

impl ConnectorKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConnectorKind::VolksbankKasselGoettingen => "fints:52090000",
            ConnectorKind::Direkt1822 => "fints:50050201",
            ConnectorKind::FinTsGeneric => "fints",
            ConnectorKind::Comdirect => "comdirect",
            ConnectorKind::PayPal => "paypal",
            ConnectorKind::TradeRepublic => "traderepublic",
            ConnectorKind::AmericanExpress => "enablebanking:amex",
            ConnectorKind::AmazonVisa => "enablebanking:amazonvisa",
            ConnectorKind::Bunq => "bunq",
            ConnectorKind::EnableBanking => "enablebanking",
            ConnectorKind::ScalableCapital => "scalable",
            ConnectorKind::AmazonOrders => "amazon-orders",
            ConnectorKind::Fnz => "fnz",
            ConnectorKind::Ibkr => "ibkr",
            ConnectorKind::Quirion => "quirion",
            ConnectorKind::Easybank => "easybank",
            ConnectorKind::AlteLeipziger => "fin4u",
            ConnectorKind::VolkswagenBank => "volkswagenbank",
            ConnectorKind::Uberspace => "uberspace",
            ConnectorKind::SchwaebischHall => "schwaebischhall",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        if let Some(k) = Self::all().into_iter().find(|k| k.as_str() == s) {
            return Some(k);
        }
        if Self::fints_blz(s).is_some() {
            return Some(ConnectorKind::FinTsGeneric);
        }
        Self::enable_banking_target(s).map(|_| ConnectorKind::EnableBanking)
    }
    /// `(country, ASPSP name)` of an `enablebanking:<CC>:<name>` connector key.
    pub fn enable_banking_target(key: &str) -> Option<(&str, &str)> {
        let rest = key.strip_prefix("enablebanking:")?;
        let (cc, name) = rest.split_once(':')?;
        (cc.len() == 2 && cc.bytes().all(|b| b.is_ascii_uppercase()) && !name.trim().is_empty())
            .then_some((cc, name))
    }
    /// Connector key stored in the DB for a generic Enable Banking bank.
    pub fn enable_banking_key(country: &str, aspsp: &str) -> String {
        format!("enablebanking:{}:{}", country.to_uppercase(), aspsp.trim())
    }
    /// BLZ of a `fints:<8 digits>` connector key (any FinTS bank).
    pub fn fints_blz(key: &str) -> Option<&str> {
        let rest = key.strip_prefix("fints:")?;
        let blz = rest.strip_suffix(":rah10").unwrap_or(rest);
        (blz.len() == 8 && blz.bytes().all(|b| b.is_ascii_digit())).then_some(blz)
    }
    /// Connector key stored in the DB for a generic FinTS bank.
    pub fn fints_key(blz: &str) -> String {
        format!("fints:{blz}")
    }
    pub fn all() -> Vec<Self> {
        vec![
            ConnectorKind::VolksbankKasselGoettingen,
            ConnectorKind::Direkt1822,
            ConnectorKind::FinTsGeneric,
            ConnectorKind::Comdirect,
            ConnectorKind::PayPal,
            ConnectorKind::TradeRepublic,
            ConnectorKind::AmericanExpress,
            ConnectorKind::AmazonVisa,
            ConnectorKind::Bunq,
            ConnectorKind::EnableBanking,
            ConnectorKind::ScalableCapital,
            ConnectorKind::AmazonOrders,
            ConnectorKind::Fnz,
            ConnectorKind::Ibkr,
            ConnectorKind::Quirion,
            ConnectorKind::Easybank,
            ConnectorKind::AlteLeipziger,
            ConnectorKind::VolkswagenBank,
            ConnectorKind::Uberspace,
            ConnectorKind::SchwaebischHall,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum AccountKind {
    Checking,
    Savings,
    CreditCard,
    PaymentService,
    Brokerage,
    Cash,
    /// Planned loan / mortgage (Darlehen) – shown neutral, not alarming.
    Loan,
}

impl AccountKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            AccountKind::Checking => "checking",
            AccountKind::Savings => "savings",
            AccountKind::CreditCard => "credit_card",
            AccountKind::PaymentService => "payment_service",
            AccountKind::Brokerage => "brokerage",
            AccountKind::Cash => "cash",
            AccountKind::Loan => "loan",
        }
    }
    pub fn from_str(s: &str) -> Self {
        match s {
            "savings" => AccountKind::Savings,
            "credit_card" => AccountKind::CreditCard,
            "payment_service" => AccountKind::PaymentService,
            "brokerage" => AccountKind::Brokerage,
            "cash" => AccountKind::Cash,
            "loan" => AccountKind::Loan,
            _ => AccountKind::Checking,
        }
    }
    /// SF Symbol used instead of any bank logo.
    pub fn sf_symbol(&self) -> &'static str {
        match self {
            AccountKind::Checking => "building.columns",
            AccountKind::Savings => "banknote",
            AccountKind::CreditCard => "creditcard",
            AccountKind::PaymentService => "envelope.circle",
            AccountKind::Brokerage => "chart.line.uptrend.xyaxis",
            AccountKind::Cash => "eurosign.circle",
            AccountKind::Loan => "house",
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BankConnection {
    pub id: i64,
    pub connector: ConnectorKind,
    pub display_name: String,
    pub login_name: String,
    pub bank_code: String,
    pub sf_symbol: String,
    pub last_sync_unix: Option<i64>,
    /// Every account of this login is archived: "sync all" skips it.
    pub all_accounts_archived: bool,
}

/// One securities position of a depot account.
#[derive(Debug, Clone, PartialEq, Default, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Holding {
    pub isin: String,
    pub wkn: String,
    pub name: String,
    pub quantity: f64,
    pub current_price_cents: i64,
    pub purchase_price_cents: i64,
    pub value_cents: i64,
    /// Previous-day price / value, when the bank reports them.
    #[uniffi(default = None)]
    pub prev_day_price_cents: Option<i64>,
    #[uniffi(default = None)]
    pub prev_day_value_cents: Option<i64>,
    /// Profit/loss against the purchase price (absolute in cents, relative in %).
    #[uniffi(default = None)]
    pub profit_loss_purchase_cents: Option<i64>,
    #[uniffi(default = None)]
    pub profit_loss_purchase_rel: Option<f64>,
    /// Profit/loss against the previous day.
    #[uniffi(default = None)]
    pub profit_loss_prev_day_cents: Option<i64>,
    #[uniffi(default = None)]
    pub profit_loss_prev_day_rel: Option<f64>,
    /// Custody type as the bank names it (e.g. "Girosammelverwahrung"), "" = unknown.
    #[uniffi(default = "")]
    pub custody_type: String,
}

/// What a securities transaction was (#314).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, uniffi::Enum)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SecurityDirection {
    Buy,
    Sell,
    Dividend,
    Fee,
    Tax,
    #[default]
    Other,
}

impl SecurityDirection {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Buy => "buy",
            Self::Sell => "sell",
            Self::Dividend => "dividend",
            Self::Fee => "fee",
            Self::Tax => "tax",
            Self::Other => "other",
        }
    }

    pub fn from_db(s: &str) -> Self {
        match s {
            "buy" => Self::Buy,
            "sell" => Self::Sell,
            "dividend" => Self::Dividend,
            "fee" => Self::Fee,
            "tax" => Self::Tax,
            _ => Self::Other,
        }
    }
}

/// One securities transaction of a depot (#314): not a cash booking, so no
/// category and no cash flow. Cents are in `currency`; `value_cents` is the
/// gross amount (quantity × price) without fees, taxes and accrued interest.
#[derive(Debug, Clone, PartialEq, Default, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SecurityTransaction {
    pub id: i64,
    pub account_id: i64,
    pub external_id: String,
    pub booking_date: String,
    pub settlement_date: String,
    pub isin: String,
    pub wkn: String,
    pub name: String,
    pub direction: SecurityDirection,
    pub quantity: f64,
    pub price_cents: i64,
    pub currency: String,
    pub value_cents: i64,
    pub fees_cents: i64,
    pub taxes_cents: i64,
    pub accrued_interest_cents: i64,
    pub fx_rate: Option<f64>,
    pub booking_text: String,
    /// The matching cash booking on a settlement account, when found.
    pub cash_transaction_id: Option<i64>,
}

#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Account {
    pub id: i64,
    pub connection_id: Option<i64>,
    pub connector: Option<ConnectorKind>,
    pub group_id: i64,
    pub name: String,
    pub bank_name: String,
    pub owner: String,
    pub iban: String,
    pub account_number: String,
    pub currency: String,
    pub kind: AccountKind,
    pub balance_cents: i64,
    pub balance_date: String,
    pub sf_symbol: String,
    pub sort_order: i32,
    pub transaction_count: u32,
    pub uncategorized_count: u32,
    /// Bookings not yet marked as checked ("new").
    pub new_count: u32,
    /// Original loan amount ("Ursprünglicher Kreditbetrag", positive cents).
    /// Auto-derived from statement history, editable by the user.
    pub initial_balance_cents: Option<i64>,
    /// User-defined group ("Nach Gruppe" view); `None` = "Ohne Gruppe".
    pub user_group_uuid: Option<String>,
    /// Counts towards the overall total (false if its user group, or a group
    /// that one is in, is excluded).
    pub in_total: bool,
    /// Marker colour (`#RRGGBB`): the user's choice or a default derived
    /// from the account's sync identity (same on every device).
    pub color_hex: String,
    /// When the account was archived ("Das Konto wurde aufgelöst"); `None` =
    /// active. Archived accounts keep bookings, categories and their group,
    /// but are skipped by syncs and rule runs and leave the overall total.
    pub archived_unix: Option<i64>,
    /// The bank / PSD2 consent no longer grants access (e.g. not included in a
    /// renewed Enable Banking consent). Kept with its bookings, not synced.
    pub access_revoked: bool,
    /// Opening balance ("Startsaldo", #212): the balance at the start of
    /// `opening_balance_date` (`""` = before the first booking). Only used for
    /// accounts with `balance_derived`.
    pub opening_balance_cents: Option<i64>,
    pub opening_balance_date: String,
    /// The balance is computed from the bookings (imports, connectors without
    /// a reported balance), not delivered by the bank; depots and loans never.
    pub balance_derived: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AccountGroup {
    pub id: i64,
    pub name: String,
    pub sf_symbol: String,
    pub sort_order: i32,
    /// Sum of all EUR account balances in this group.
    pub balance_cents: i64,
    pub accounts: Vec<Account>,
}

/// A user-defined sidebar group ("Nach Gruppe" view).
#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UserGroup {
    /// Stable across devices. Empty for the "Ohne Gruppe" entry.
    pub uuid: String,
    pub name: String,
    pub sf_symbol: String,
    /// The group's colour (`#RRGGBB`); empty = none chosen.
    pub color_hex: String,
    pub sort_order: i32,
    /// The group's own switch: its accounts count towards the overall total.
    pub include_in_total: bool,
    /// Effective: false if this group or a group it is in is excluded.
    pub in_total: bool,
    /// The trailing "Ohne Gruppe" entry (accounts without a user group).
    pub is_ungrouped: bool,
    /// The group this one is a subgroup of; `None` = top level.
    pub parent_uuid: Option<String>,
    /// Nesting level (0 = top level). The list is a flattened tree: a
    /// group's subgroups follow it directly.
    pub depth: u32,
    /// Sum of all EUR account balances in this group and its subgroups.
    pub balance_cents: i64,
    /// The group's own accounts (without those of its subgroups).
    pub accounts: Vec<Account>,
}

#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Category {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub sf_symbol: String,
    pub color_hex: String,
    pub sort_order: i32,
    /// 0 for top-level categories.
    pub depth: u32,
    /// "Food › Groceries"
    pub path: String,
    /// Sum of transactions directly in this category (within the period).
    pub own_total_cents: i64,
    /// Sum including all sub-categories.
    pub total_cents: i64,
    pub transaction_count: u32,
    pub budget_cents: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CategorySource {
    None,
    Rule,
    Manual,
}

#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Booking {
    pub id: i64,
    pub account_id: i64,
    pub account_name: String,
    pub booking_date: String,
    pub value_date: String,
    pub amount_cents: i64,
    pub currency: String,
    pub counterparty: String,
    pub counterparty_iban: String,
    pub purpose: String,
    pub booking_text: String,
    pub category_id: Option<i64>,
    pub category_name: String,
    pub category_path: String,
    pub category_color_hex: String,
    pub category_symbol: String,
    pub category_source: CategorySource,
    pub rule_id: Option<i64>,
    pub checked: bool,
    /// Cleaned merchant (logo engine).
    pub merchant: crate::merchants::MerchantInfo,
    /// Flat copies for convenience in the UI layer.
    pub merchant_name: String,
    /// None for private persons and unknown merchants (no logo).
    pub merchant_domain: Option<String>,
    /// simple-icons SVG path ("" = none) – render in a 24×24 viewBox.
    pub merchant_svg_path: Option<String>,
    pub merchant_hex_color: String,
    /// "Kartenzahlung", "Lastschrift", "Sparplan", …
    pub payment_method: String,
    pub flow: crate::merchants::FlowKind,
    /// Purpose without SEPA keys / terminal codes.
    pub clean_purpose: String,
    pub account_kind: AccountKind,
    /// Attached PDFs / receipts (paperclip badge).
    pub document_count: u32,
    /// Free-text user note (imported from MoneyMoney or typed in the detail view).
    pub note: String,
    /// SEPA purpose code from the bank (e.g. `SALA`), "" = unknown.
    pub purpose_code: String,
    /// German Geschäftsvorfallcode from the bank (e.g. `153`), "" = unknown.
    pub gvc: String,
    /// User override: `Some(true)` = salary, `Some(false)` = never salary, `None` = automatic.
    pub salary_flag: Option<bool>,
    /// Detected (or marked) salary / pension inflow.
    pub is_salary: bool,
    /// Series key for recurring bookings (see `list_recurring`); "" = none.
    pub recurrence_key: String,
    /// Not booked yet (card hold, pending transfer): the bank still reports it
    /// as open, so it can change or disappear. The list shows it dashed.
    pub is_pending: bool,
    /// Product ids (Amazon ASIN) of an order's items, comma separated, one
    /// entry per item in purpose order ("" entry = unknown); "" = none (#354).
    pub item_refs: String,
    #[uniffi(default = "")]
    pub end_to_end_id: String,
    #[uniffi(default = "")]
    pub mandate_id: String,
    #[uniffi(default = "")]
    pub creditor_id: String,
    #[uniffi(default = "")]
    pub external_id: String,
    /// Tags of the booking (#228), in tag list order.
    #[uniffi(default)]
    #[cfg_attr(feature = "serde", serde(default))]
    pub tags: Vec<crate::tags::BookingTag>,
}

/// Dry-run result of a MoneyMoney import (nothing stored).
#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MoneyMoneyImportPreview {
    pub total_rows: u32,
    pub new_bookings: u32,
    /// Rows already in the vault (identical re-import or FinTS overlap); they are
    /// skipped or only enriched with category / note.
    pub duplicates_skipped_or_enriched: u32,
    pub notes_count: u32,
    pub date_range_start: Option<String>,
    pub date_range_end: Option<String>,
    pub detected_categories: Vec<String>,
    pub sample_bookings: Vec<Booking>,
    pub suggested_account_name: String,
    /// Existing account the file most likely belongs to (statement IBAN,
    /// matching bookings or the file name), to preselect as target.
    pub suggested_account_id: Option<i64>,
}

#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MoneyMoneyImportResult {
    pub account_id: i64,
    pub account_created: bool,
    pub total_rows: u32,
    pub imported: u32,
    pub duplicates: u32,
    /// Existing rows that received a MoneyMoney category and/or note.
    pub enriched: u32,
    pub notes_imported: u32,
    pub categories_created: u32,
    /// Sum of imported bookings (= balance of a newly created archive account).
    pub balance_cents: i64,
}

/// Top segmented control: everything / everyday life / savings plans & perks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, uniffi::Enum)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum BookingScope {
    #[default]
    All,
    /// Alltag & Konsum: everything except investments and perks.
    Everyday,
    /// Sparpläne & Perks: investments, Saveback, Round up.
    Investments,
}

/// Income / expense filter of the booking list (GitHub #53).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, uniffi::Enum)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum BookingDirection {
    #[default]
    All,
    /// Inflows only (amount > 0).
    Income,
    /// Outflows only (amount < 0).
    Expense,
}

#[derive(Debug, Clone, Default, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BookingFilter {
    /// Empty = all accounts.
    pub account_ids: Vec<i64>,
    pub category_id: Option<i64>,
    pub include_subcategories: bool,
    pub uncategorized_only: bool,
    pub search: Option<String>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub limit: u32,
    pub offset: u32,
    pub scope: BookingScope,
    /// Only bookings of this merchant key.
    pub merchant_key: Option<String>,
    /// Income / expenses only; `All` keeps both.
    pub direction: BookingDirection,
    /// Only checked (`true`) or only new (`false`) bookings; `None` keeps both.
    pub checked: Option<bool>,
    /// Extend the page to the end of its last day, so a day is never split
    /// between two pages (the list shows a sum per day).
    pub whole_days: bool,
    /// Only bookings carrying every one of these tags (#228); empty = any.
    #[uniffi(default)]
    #[cfg_attr(feature = "serde", serde(default))]
    pub tag_ids: Vec<i64>,
    /// Only bookings without any tag.
    #[uniffi(default)]
    #[cfg_attr(feature = "serde", serde(default))]
    pub untagged_only: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BookingPage {
    pub transactions: Vec<Booking>,
    pub total_count: u32,
    /// Sum of `FlowKind::Income` bookings (= `flow.income_cents`).
    pub income_cents: i64,
    /// Net sum of all other non-transfer bookings (negative = outflow).
    pub expense_cents: i64,
    /// Everyday spending only (negative), never includes ETF plans or perks.
    pub consumption_cents: i64,
    /// Invested via savings plans / purchases (negative).
    pub investment_cents: i64,
    /// Saveback + Round up movements (net).
    pub perk_cents: i64,
    /// KPI flows of all matching bookings (not only the loaded page).
    pub flow: SelectionFlow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RuleField {
    /// Counterparty, purpose and booking text.
    Any,
    Counterparty,
    Purpose,
    BookingText,
    CounterpartyIban,
}

impl RuleField {
    pub fn as_str(&self) -> &'static str {
        match self {
            RuleField::Any => "any",
            RuleField::Counterparty => "counterparty",
            RuleField::Purpose => "purpose",
            RuleField::BookingText => "booking_text",
            RuleField::CounterpartyIban => "iban",
        }
    }
    pub fn from_str(s: &str) -> Self {
        match s {
            "counterparty" => RuleField::Counterparty,
            "purpose" => RuleField::Purpose,
            "booking_text" => RuleField::BookingText,
            "iban" => RuleField::CounterpartyIban,
            _ => RuleField::Any,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum MatchKind {
    /// Pipe-separated keywords, any of them matches ("REWE|EDEKA|ALDI").
    Keyword,
    /// A full regular expression.
    Regex,
    /// A search expression: AND/OR/NOT, "phrases", wildcards, /regex/,
    /// field prefixes and amount comparisons (see `search_expr`).
    Expression,
}

#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Rule {
    /// 0 when creating a new rule.
    pub id: i64,
    pub name: String,
    pub category_id: i64,
    pub field: RuleField,
    pub match_kind: MatchKind,
    pub pattern: String,
    pub case_sensitive: bool,
    pub min_amount_cents: Option<i64>,
    pub max_amount_cents: Option<i64>,
    /// Accounts the rule is restricted to; empty = all accounts. Stored in the
    /// existing `rules.account_id` column (single id as integer, several as
    /// comma-separated text), so no schema step is needed.
    pub account_ids: Vec<i64>,
    /// Lower number = evaluated first.
    pub priority: i32,
    pub enabled: bool,
    /// Filled when listing: how many transactions this rule categorised.
    pub match_count: u32,
}

#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SettingEntry {
    pub key: String,
    pub label: String,
    pub help: String,
    /// Stable (German) section id; use `group_title` for display.
    pub group: String,
    /// Localised section title.
    pub group_title: String,
    /// Empty when `is_secret` (use `get_setting` to reveal).
    pub value: String,
    pub is_secret: bool,
    pub has_value: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PeriodSummary {
    pub income_cents: i64,
    pub expense_cents: i64,
    pub balance_cents: i64,
}

#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DailyBalance {
    pub date: String,
    pub balance_cents: i64,
}

/// One day of the value history of securities depots (`Vault::depot_value_history`): the sum of
/// the depots that had a value that day. `covered_depots` of `total_depots` says how complete
/// the sum is; a depot adds nothing before its first known value (it is not counted as 0).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DepotValuePoint {
    pub date: String,
    pub value_cents: i64,
    pub covered_depots: u32,
    pub total_depots: u32,
}

/// Sum of one category (with subcategories) over one calendar day, month or year (`period` = `yyyy-MM-dd`, `yyyy-MM` or `yyyy`).
#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CategoryPeriodSum {
    pub period: String,
    pub cents: i64,
}

#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MerchantStats {
    pub key: String,
    pub name: String,
    pub count: u32,
    /// Net sum of all bookings at this merchant.
    pub total_cents: i64,
    pub average_cents: i64,
    pub first_date: String,
    pub last_date: String,
}

/// Merchant history of a selected booking: all accounts and the booking's own
/// account, computed in one pass (#591).
#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MerchantHistory {
    pub all: MerchantStats,
    pub account: MerchantStats,
}

/// KPI tiles: flows of a selection (accounts + period), all positive.
#[derive(Debug, Clone, Default, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SelectionFlow {
    /// Everyday spending.
    pub consumption_cents: i64,
    /// Savings plans / purchases plus perk outflows (Round up).
    pub invested_cents: i64,
    pub income_cents: i64,
}

impl SelectionFlow {
    /// `amount_cents` is the booking's share in the selection: its full amount,
    /// or with a category filter only the split parts of that category.
    pub(crate) fn add(&mut self, flow: crate::merchants::FlowKind, amount_cents: i64) {
        use crate::merchants::FlowKind;
        match flow {
            FlowKind::Consumption => self.consumption_cents -= amount_cents,
            FlowKind::Investment => self.invested_cents -= amount_cents,
            FlowKind::Perk if amount_cents < 0 => self.invested_cents -= amount_cents,
            _ => {}
        }
        if kpi_side(flow, amount_cents) == KpiSide::Income {
            self.income_cents += amount_cents;
        }
    }
}

/// Side of a booking in the income / expense KPIs. The one definition used by
/// `BookingPage`, `SelectionFlow` and the period comparison (`cashflow_by_period`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KpiSide {
    /// `FlowKind::Income` (salary, other real inflows).
    Income,
    /// Money spent: every consumption booking, netted (a refund is a positive
    /// `Consumption` and reduces the expenses), plus the *outflows* of
    /// investments (savings plans, purchases, Round up) and loans (instalments).
    Expense,
    /// Transfers between own accounts / card settlements, and inflows that are
    /// neither income nor a refund: sale proceeds, Saveback, loan payouts.
    /// They must not reduce the expenses (a 20 000 € share sale is no refund).
    Neutral,
}

pub(crate) fn kpi_side(flow: crate::merchants::FlowKind, amount_cents: i64) -> KpiSide {
    use crate::merchants::FlowKind;
    match flow {
        FlowKind::Income => KpiSide::Income,
        FlowKind::Transfer => KpiSide::Neutral,
        FlowKind::Consumption => KpiSide::Expense,
        FlowKind::Investment | FlowKind::Perk | FlowKind::Loan if amount_cents < 0 => {
            KpiSide::Expense
        }
        FlowKind::Investment | FlowKind::Perk | FlowKind::Loan => KpiSide::Neutral,
    }
}

#[cfg(test)]
mod kpi_tests {
    use super::*;
    use crate::merchants::FlowKind;

    #[test]
    fn only_refunds_reduce_expenses() {
        assert_eq!(kpi_side(FlowKind::Consumption, -5_000), KpiSide::Expense);
        assert_eq!(kpi_side(FlowKind::Consumption, 1_000), KpiSide::Expense); // refund
        assert_eq!(kpi_side(FlowKind::Investment, -10_000), KpiSide::Expense); // ETF plan
        assert_eq!(kpi_side(FlowKind::Investment, 2_000_000), KpiSide::Neutral); // sale
        assert_eq!(kpi_side(FlowKind::Loan, -80_000), KpiSide::Expense); // instalment
        assert_eq!(kpi_side(FlowKind::Loan, 5_000_000), KpiSide::Neutral); // payout
        assert_eq!(kpi_side(FlowKind::Perk, 300), KpiSide::Neutral); // Saveback
        assert_eq!(kpi_side(FlowKind::Income, 300_000), KpiSide::Income);
        assert_eq!(kpi_side(FlowKind::Transfer, -50_000), KpiSide::Neutral);
    }
}

#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PlanExecution {
    /// "2026-11"
    pub month: String,
    /// Invested amount in that month (positive cents, 0 = not executed).
    pub amount_cents: i64,
}

#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SavingsPlan {
    pub merchant: crate::merchants::MerchantInfo,
    pub monthly_cents: i64,
    pub day_of_month: u32,
    pub last_date: String,
    pub active: bool,
    pub total_invested_cents: i64,
    pub executions: Vec<PlanExecution>,
}

#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MonthFlow {
    pub month: String,
    pub consumption_cents: i64,
    pub investment_cents: i64,
    pub income_cents: i64,
}

#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct InvestmentDashboard {
    pub months: Vec<String>,
    pub plans: Vec<SavingsPlan>,
    /// Sum of all active plans per month (positive cents).
    pub total_monthly_cents: i64,
    pub saveback_earned_cents: i64,
    pub saveback_count: u32,
    pub roundup_invested_cents: i64,
    pub roundup_count: u32,
    pub perks_this_month_cents: i64,
    pub monthly: Vec<MonthFlow>,
}

/// A single SEPA credit transfer ("Überweisung") to be sent via FinTS.
#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SepaTransferRequest {
    pub account_id: i64,
    pub recipient_name: String,
    pub recipient_iban: String,
    pub recipient_bic: Option<String>,
    /// Must be > 0.
    pub amount_cents: i64,
    /// Verwendungszweck, max. 140 characters.
    pub purpose: String,
    /// SEPA Instant Payment ("Echtzeitüberweisung", HKIPZ). FinTS only.
    pub instant: bool,
    /// Optional future execution date `YYYY-MM-DD` (Terminüberweisung, HKCSE). FinTS only.
    pub execution_date: Option<String>,
}

/// One payment of a batch transfer.
#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BatchPaymentInput {
    pub name: String,
    pub iban: String,
    pub bic: Option<String>,
    /// Must be > 0.
    pub amount_cents: i64,
    /// Verwendungszweck, max. 140 characters.
    pub purpose: String,
}

/// Batch transfer (Sammelüberweisung, HKCCM): one debit account, many payments.
#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BatchTransferRequest {
    pub account_id: i64,
    pub payments: Vec<BatchPaymentInput>,
    /// true = one booking for the sum, false = one booking per payment (if the
    /// bank allows it).
    pub batch_booking: bool,
    /// Optional future execution date `YYYY-MM-DD`.
    pub execution_date: Option<String>,
}

/// What to do with a standing order at the bank (#496).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum StandingOrderAction {
    /// New order (HKCDE).
    Create,
    /// Replace the data of the order `order_row_id` (HKCDN).
    Change,
    /// Delete the order `order_row_id` (HKCDL). Only the account and the id count.
    Delete,
}

/// Create, change or delete a standing order (Dauerauftrag, HKCDE/HKCDN/HKCDL).
/// For `Change` and `Delete` the order is the stored one (`order_row_id`, from
/// `list_standing_orders`); the bank's order id and, for a deletion, the data
/// to send come from the vault, not from the caller.
#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StandingOrderRequest {
    pub account_id: i64,
    pub action: StandingOrderAction,
    /// `standing_orders.id` of the order to change or delete.
    pub order_row_id: Option<i64>,
    pub recipient_name: String,
    pub recipient_iban: String,
    pub recipient_bic: Option<String>,
    /// Must be > 0.
    pub amount_cents: i64,
    /// Verwendungszweck, max. 140 characters.
    pub purpose: String,
    /// First execution `YYYY-MM-DD`, in the future.
    pub first_date: String,
    /// Last execution `YYYY-MM-DD`; `None` = until cancelled.
    pub last_date: Option<String>,
    /// `M` (months) or `W` (weeks).
    pub time_unit: String,
    /// Every n-th month / week (1–99).
    pub interval: u32,
    /// Day of the month (1–31, 31 = last day) or weekday (1 = Monday … 7).
    pub exec_day: u32,
}

/// What the bank allows when creating or changing a standing order, as far as its
/// parameters (BPD) say. An empty list means "no restriction" or "not known".
#[derive(Debug, Clone, Default, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StandingOrderLimits {
    /// The first execution must lie at least / at most this many days ahead (0 = no limit).
    pub min_days_ahead: u32,
    pub max_days_ahead: u32,
    /// Allowed intervals in months / weeks.
    pub months: Vec<u32>,
    pub weeks: Vec<u32>,
    /// Allowed days of the month (31 = last day) / weekdays (1 = Monday … 7).
    pub days_of_month: Vec<u32>,
    pub weekdays: Vec<u32>,
}

/// Recipient suggested from past bookings (auto-completion).
#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct KnownRecipient {
    pub name: String,
    pub iban: String,
    pub times_used: u32,
}

/// Budget progress of one category for one month.
#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CategoryBudgetStatus {
    pub category_id: i64,
    pub name: String,
    pub path: String,
    pub sf_symbol: String,
    pub color_hex: String,
    pub budget_cents: i64,
    pub spent_cents: i64,
    /// Negative when over budget.
    pub remaining_cents: i64,
    /// spent / budget * 100 (can exceed 100).
    pub percent: f64,
}

#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TransactionSplit {
    pub id: i64,
    pub transaction_id: i64,
    pub category_id: Option<i64>,
    pub amount_cents: i64,
    pub note: String,
}

#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TransactionSplitInput {
    pub category_id: Option<i64>,
    /// Same sign as the parent booking; all parts must add up to it.
    pub amount_cents: i64,
    pub note: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PeriodGranularity {
    Month,
    Quarter,
    Year,
}

/// Income vs. expenses of one period ("2025-03", "2025-Q1", "2025").
#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PeriodCashflow {
    pub key: String,
    pub income_cents: i64,
    /// Positive cents.
    pub expense_cents: i64,
}

#[derive(Debug, Clone, uniffi::Record)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TransferTemplate {
    /// 0 = not saved yet.
    pub id: i64,
    pub name: String,
    pub recipient_name: String,
    pub recipient_iban: String,
    pub recipient_bic: String,
    pub default_amount_cents: Option<i64>,
    pub default_purpose: String,
}
