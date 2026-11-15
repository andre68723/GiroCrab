# Architecture notes

## Schema & migrations

The vault is a SQLCipher database. Its schema is versioned in
`girocrab-core/src/schema.rs`: `PRAGMA user_version` counts the steps a vault has run,
and `migrate` runs the missing ones in a single transaction.

| Version | Step |
|---------|------|
| 1 | `BASELINE`: the complete schema of a new vault |
| 2–8 | empty – vaults from before #206 carry these numbers, reached through probes |
| 9 | `repair_to_v8`: the old probe-based setup, run once, so every vault ends up identical |
| 10 | opening balances (#212) |
| 11 | `sync_log`: the Abrufprotokoll (#238), local to the device |
| 12 | SEPA references of a booking (end-to-end id, mandate reference, creditor id) |
| 13 | tangible assets (`asset_items`, quantities, values, prices) |
| 14 | `security_transactions`: orders and distributions of depots, extra `holdings` columns |
| 15 | product ids of an order's items (#354) |
| 16 | asset classification (#351, `docs/asset-classification.md`) |
| 17 | planned transfers |
| 18 | standing orders |
| 19 | repair of step 15 for vaults of development builds |
| 20 | mandates and creditor ids |
| 21 | kind of a planned transfer (credit / debit) |
| 22 | return acknowledgement of a mandate |
| 23 | booking tags (#228) |
| 24 | a rule restricted to several accounts (#562) |
| 25 | index `transactions(amount, date)` |
| 26+ | future changes; the tables of securities depots are described in `docs/DEPOT_DATENMODELL.md` |

**Changing the schema**

1. Append a step to `STEPS` (SQL, or a hook for Rust logic such as backfills). Never edit or
   reorder an existing step: vaults in the wild have already run it.
2. Run `cargo run -q -p girocrab-core --example make_fixture_vault`. It writes
   `girocrab-core/tests/fixtures/vault-v{N}.girocrab`; commit it. Never regenerate or edit an
   older fixture – each stands for vaults written by that version.
3. `cargo test -q -p girocrab-core --test schema_fixtures` writes `fixtures/schema-v{N}.sql`
   (normalised `sqlite_master`) on first run. Review and commit it, too. Every fixture vault must
   open and end up with exactly the schema of a fresh vault.

**How a vault opens** (`Vault::migrate`)

1. Key check (`SELECT … FROM sqlite_master`); a wrong key is `CoreError::WrongKey`, which makes
   `Vault::open` try the next `cipher_compatibility` mode.
2. Downgrade guard: `user_version` above this build's `SCHEMA_VERSION` →
   `CoreError::SchemaTooNew`, before anything is written. The file stays untouched. The same
   applies to the peer file in an iCloud snapshot merge. Builds from before #206 have no guard.
3. `foreign_keys` / `journal_mode = WAL` (both no-ops inside a transaction).
4. All pending steps run in **one** transaction, which also sets `user_version`. If a step
   fails, nothing changes.

The snapshot export (`write_snapshot`) stamps `user_version = SCHEMA_VERSION` on the copy.

**Rules of thumb**

- Counts filter `deleted_ms IS NULL`: indexes used by count queries must cover that column.
- Index changes are a migration step, not a probe that runs on every open.
- Data repairs that must run on every open (e.g. `fill_missing_uuids`) are called from
  `Vault::open` directly, not from the schema steps.

## Vault modules

`Vault` is one object (one SQLCipher connection behind a mutex) with many feature modules. Each
module adds its own methods to it and reaches the database through `self.db()`.

- `vault.rs` only opens, keys, migrates and snapshots the vault (`open`, `rekey`,
  `write_snapshot`, `checkpoint`, `is_empty`) and holds the schema constants.
- Feature SQL lives in the module of its feature, never in `vault.rs`:

  | Module | Feature |
  |--------|---------|
  | `vault/accounts.rs` | bank logins, accounts, account groups, holdings, balance history |
  | `vault/bookings.rs` | booking list (filter, paging, enrichment), booking edits, CSV export |
  | `vault/categories.rs` | category tree |
  | `vault/rules.rs` | pattern and expression rules |
  | `vault/settings.rs` | settings and `KNOWN_SETTINGS` |
  | `vault/merge.rs` | merging a peer snapshot or another vault file |
  | `loans.rs`, `planning.rs`, `documents.rs`, `insights.rs`, `connectors/*.rs`, `importers/*.rs`, … | their feature |

- A feature module has at most one `#[uniffi::export] impl Vault` (the Swift API) and one plain
  `impl Vault` (helpers taking `&Connection`, for use inside other transactions). Helpers are
  private unless another module calls them; prefer `pub(super)` inside `vault/` over `pub(crate)`.
- Modules under `vault/` are children of `vault`, so they see its private fields and helpers
  (`kept_key`, `migrate`). Other modules use `Vault::db()` only.
- Tests sit in the module whose code they exercise.
- A new feature gets a new module. Don't add another section to an existing file.
