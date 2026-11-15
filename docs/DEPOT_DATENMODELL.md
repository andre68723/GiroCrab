# Datenmodell der Wertpapierdepots

Stand: Schema 26. Regeln für Schemaänderungen: `docs/ARCHITECTURE.md`.

## Überblick

```
accounts (kind = 'brokerage')
  ├── holdings                 aktueller Bestand, bei jedem Abruf ersetzt
  │     └── holding_classifications   Klasse/Instrument je Position (Schritt 16)
  ├── security_transactions    Käufe, Verkäufe, Ausschüttungen, Gebühren, Steuern
  └── depot_values             Wert des Depots je Tag und Quelle

security_classifications       Nutzerwahl je Wertpapier (security_key), unabhängig vom Konto
```

Sachwerte (`kind` mit `assets::is_tangible`) haben ein eigenes Modell: `asset_items`, `asset_quantities`,
`asset_values`, `asset_prices`, `asset_price_fetch` (Schritt 13, `girocrab-core/src/assets.rs`).

## Tabellen

### `holdings` (Basis, Schritt 14 ergänzt Spalten)
Positionen eines Depots **zum Zeitpunkt des letzten Abrufs**.
- Schlüssel: `id`; `account_id` verweist auf `accounts` (`ON DELETE CASCADE`).
- Kennung: `isin`, `wkn` (leer = unbekannt), `name`.
- Zahlen: `quantity` (REAL), `current_price_cents`, `purchase_price_cents`, `value_cents` (alle in Cent der
  Kontowährung). Ab Schritt 14 zusätzlich `prev_day_price_cents`, `prev_day_value_cents`,
  `profit_loss_purchase_cents`/`_rel`, `profit_loss_prev_day_cents`/`_rel` (jeweils NULL = nicht geliefert) und
  `custody_type`.
- **Wird bei jedem Abruf vollständig ersetzt** (`connectors/persist.rs`, ebenso beim Import). Es gibt
  keine Historie, und die Zeilen-ID ist nicht stabil. Beziehungen auf eine Position laufen deshalb über ISIN oder
  `security_key`, nicht über `holdings.id`.
- Gehört nicht zum Tresor-Abgleich (`vault/merge.rs` übernimmt sie nicht); jedes Gerät holt sie selbst.

### `security_transactions` (Schritt 14)
Umsätze eines Depots, getrennt von den Geldbuchungen (`transactions`).
- Eindeutig je Depot: `UNIQUE(account_id, external_id)`.
- `booking_date`, `settlement_date`, `isin`, `wkn`, `name`.
- `direction`: `buy | sell | dividend | fee | tax | other`.
- `quantity`, `price_cents`, `currency`, `value_cents`, `fees_cents`, `taxes_cents`, `accrued_interest_cents`,
  `fx_rate` (NULL = keine Fremdwährung), `booking_text`.
- `cash_transaction_id` verweist (`ON DELETE SET NULL`) auf die zugehörige Geldbuchung (`link_cash_bookings`).
- Abgleich: `changed_ms` (neuester Stand gewinnt), `deleted_ms` als weiches Löschen. Indizes auf
  `(account_id, booking_date, deleted_ms)` und `(isin, deleted_ms)`.
- Aus ihr lässt sich die **Menge zu einem Tag** zurückrechnen (Käufe/Verkäufe), soweit der Umsatzverlauf reicht.

### Klassifizierung (Schritt 16)
`security_classifications(security_key, asset_class_override, instrument_override, …_ms)` hält die Nutzerwahl
je Wertpapier, `holding_classifications(holding_id, asset_class, instrument, direct_crypto)` das berechnete Ergebnis
je Position. Details: `docs/asset-classification.md`.
`security_key` ist der **gemeinsame Schlüssel eines Wertpapiers** und wird für alles verwendet, was sich auf ein
Wertpapier statt auf eine Position bezieht.

### Depotwert aus dem Konto
`accounts.balance_cents` und `balance_date` tragen den vom Anbieter gemeldeten Depotwert und sein Stichtag. Das ist der
aktuelle Gesamtwert des Kontos. Die historischen Depotgesamtwerte stehen getrennt davon in `depot_values`.

## Wer schreibt und wer liest

| Daten | Schreibt | Liest |
|---|---|---|
| `holdings` | `connectors/persist.rs` (`store_fetch`), Datenbank-Importer (`importers/`) | Depotansicht, Vermögen, Klassifizierung |
| `security_transactions` | `connectors/persist.rs` (`security_transactions::upsert` beim Abruf); ein Import schreibt keine Umsätze | Aktivitäten im Depot, Kennzahlen |
| `security_classifications` | `set_asset_class_override`, `set_instrument_override` (Kontextmenü) | Zusammensetzung (`classified_assets`) |

## Wertverlauf eines Depots (`depot_values`, Schritt 26)

**Umfang:** Gespeichert werden ausschließlich Zeitreihen der **Depotgesamtwerte**, keine historischen Kurse,
Bestände oder Marktwerte einzelner Wertpapierpositionen. `holdings` bleibt der aktuelle Bestand; eine Zeitreihe
einer Einzelposition lässt sich aus `depot_values` nicht wiederherstellen. Ein späterer Import kann fertige
Depotgesamtwerte übernehmen; Zeitreihen einzelner Positionen werden in diesem Modell nicht übernommen.

Ein Depot hat keine Buchungen, über die man den Wert zurückrechnen könnte, und die angebundenen Banken melden nur
den aktuellen Wert. Der Verlauf entsteht deshalb aus Punkten: je Abruf der gemeldete Depotwert mit seinem Stichtag, dazu
Werte aus einem Import. Eine Kursquelle gibt es nicht und ist nicht geplant (ein Verlauf aus Kursen bräuchte
Kursserien je Handelsplatz und Währung, die Menge je Tag und eine Weitergabe von Kennungen an Dritte).

```
depot_values(
  account_id  INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  value_date  TEXT    NOT NULL,                    -- YYYY-MM-DD
  source      TEXT    NOT NULL DEFAULT 'fetch',    -- fetch | import | prices
  quality     TEXT    NOT NULL DEFAULT 'observed', -- observed | reconstructed | estimated
  value_cents INTEGER NOT NULL,
  changed_ms  INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (account_id, value_date, source)
) WITHOUT ROWID;
```

**Quellen und Güte**
- `fetch`: der Depotwert, den die Bank bei einem Abruf meldet. `import`: Werte aus einer Datei oder Datenbank.
  `prices` und die Güten `reconstructed` / `estimated` sind für berechnete Werte reserviert; heute entsteht nur
  `observed`.
- Mehrere Quellen für denselben Tag liegen nebeneinander. Angezeigt wird je Tag die beste: erst nach Güte, dann
  `fetch` vor `import` vor `prices`.
- Abgleich zwischen Geräten: je (Konto, Tag, Quelle) gewinnt `changed_ms`. Die Zeilen verschwinden mit ihrem Konto.
  Ein weiches Löschen einer Quelle (damit andere Geräte einen entfernten Import nicht zurückbringen) ist noch offen
  und gehört zum Import.

**Regeln der Reihe** (`depot_values::series`, `Vault::depot_value_history`)
- Vor dem ersten Punkt eines Depots ist sein Wert **unbekannt**: nicht 0 und nicht der erste Wert.
- Ein **bestätigter Nullwert** (Depot vollständig verkauft) ist ein Punkt und zählt als abgedeckt. Eine 0 ohne
  Bestätigung (die Bank hat den Bestand nicht geliefert) hinterlässt keinen Punkt.
- Bekannte Werte gelten vorwärts weiter, es wird nicht interpoliert.
- Je Tag liefert die Abfrage `covered_depots` und `total_depots` (`DepotValuePoint`). Die Abdeckung wird berechnet,
  nicht gespeichert. Die Reihe beginnt am ersten Tag, an dem mindestens ein Depot einen Wert hat. Eine Summe über mehrere
  Depots ist eine Teilsumme, solange nicht alle abgedeckt sind („2 von 3 Depots“); wo ein weiteres Depot erstmals
  hinzukommt, darf der Sprung nicht als Wertzuwachs gezeigt werden.
- Der Wert ist der gemeldete Depotwert und **keine Rendite**: Käufe, Verkäufe und Übertragungen verändern ihn.

**Wer schreibt**
- `depot_values::record` beim Abruf (`connectors/persist.rs`): für Depots mit gelieferten Positionen (ein leerer
  Bestand bestätigt eine 0) und für Depots, deren Gesamtwert ohne Positionen kommt. Ist der Kontostand als unbekannt
  markiert, entsteht kein Punkt. Sachwerte haben ein eigenes Modell und bleiben außen vor.
- Import (geplant): Werte je Tag aus einer Datenbank oder einer CSV-Datei mit Datum und Wert, geschrieben als
  `import` / `observed`.
  - Ein Wert 0 ist ein bestätigter Nullwert, wenn davor positive Werte lagen; sonst ist er kein Punkt.
  - Ein Abruf-Wert desselben Tages geht vor. Wiederholtes Einlesen derselben Daten ändert nichts.
  - Beim CSV-Import: Spaltenzuordnung, Datums- und Zahlenformat, Vorschau mit Zeitraum und Zeilenzahl; fehlerhafte
    Zeilen werden gezählt und gemeldet, zukünftige Daten abgelehnt.
  - Werte in einer anderen Währung als der des Depots werden ausgewiesen und nicht eingerechnet.

**Wer liest:** `Vault::depot_value_history(account_ids, days)` für das Diagramm der Depotansicht
(`DepotDetailView`, zugeschnitten auf den Zeitraum durch `DepotValueHistory`). `balance_history` nimmt die Depotwerte
direkt, nicht als Differenz zum Kontostand.

**Demo** (`--demo-seed`): Das Welt-Depot hat ein Jahr lückenloser Abruf-Werte, das Einzeltitel-Depot beginnt erst vor
120 Tagen (Teilsumme in der Summe), das Krypto-Depot hat Lücken von ein bis drei Tagen und eine ältere importierte
Strecke, an deren Überlappung der Abruf gewinnt.
