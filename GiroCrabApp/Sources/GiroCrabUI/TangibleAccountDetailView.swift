import SwiftUI
import GiroCrabCore

/// A Sachwerte account (#87) as a view of its own: held items and sold ones, a value chart of the
/// account or of the selected item, and the actions to add, revalue and sell. Items, valuations and
/// price sources come from the Sachwerte store (`model.assetStore`); nothing here is a securities
/// position or a booking. The page frame is shared with the securities depot.
struct TangibleAccountDetailView: View {
    @Environment(AppModel.self) private var model
    let account: Account

    @State private var query = ""
    @State private var classified: [ClassifiedAsset] = []
    /// A failed classification query is shown (with a retry), not swallowed: without it the
    /// classifications, their menus and the search by classification would be silently incomplete.
    @State private var classificationError: String?
    @State private var classificationLoaded = false
    /// Yesterday's value of one piece per automatically priced item: the day change of the figure tiles.
    @State private var previousUnits: [Int64: Int64] = [:]
    @State private var reloadToken = 0
    @State private var compositionMode = DepotCompositionMode.assetClass
    @State private var compositionFilter: Set<String>?
    @State private var editingClassification: ClassificationEditRequest?
    @State private var deleting: AssetItem?

    /// The selected item, if there is one: its value and history instead of the account's.
    private var item: AssetItem? { model.selectedAssetItem(in: account.id) }

    var body: some View {
        AssetAccountFrame(
            searchText: $query,
            searchPrompt: "Sachwert oder Klassifizierung",
            backgroundTap: { model.deselectAssetItem() },
            minContentHeight: 420,
            top: { EmptyView() },
            header: { header },
            metrics: { wide in
                if !heldItems.isEmpty {
                    DepotMetricGrid(tiles: DepotKPIs.tangibles(items: heldItems, previousUnits: previousUnits, currency: currency), wide: wide)
                }
            },
            analysis: {
                if classificationLoaded, classificationError == nil, heldAssets.contains(where: { $0.holding.valueCents > 0 }) {
                    AssetCompositionCard(assets: heldAssets, currency: currency, modes: [.position, .assetClass],
                                         mode: $compositionMode, filter: $compositionFilter, expandedKey: "tangibleAnalysisExpanded",
                                         displayKey: "tangibleCompositionDisplay",
                                         note: model.selectedAssetItem(in: account.id) != nil ? "Zusammensetzung des gesamten Kontos, nicht nur des gewählten Objekts" : nil)
                }
            },
            content: { wide in
                if let message = classificationError { classificationFailure(message) }
                TangibleAssetsList(account: account, query: query, keys: compositionFilter, classified: classified, wide: wide,
                                   editingClassification: $editingClassification, deleting: $deleting)
            }
        )
        .task(id: "\(account.id)-\(account.balanceCents)-\(account.balanceDate)-\(model.assetClassificationVersion)-\(model.vaultRevision)-\(reloadToken)") {
            await loadClassification()
        }
        .capturableSheet(item: $editingClassification) { request in AssetClassificationSheet(request: request).environment(model) }
        .onChange(of: account.id) { model.selectAssetItem(nil) }
        .confirmationDialog("„\(deleting?.setup.name ?? "")“ löschen?", isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }), titleVisibility: .visible) {
            Button("Löschen", role: .destructive) {
                if let item = deleting {
                    do { try model.changeAssets { try $0.deleteAssetItem(itemId: item.id) } } catch { model.errorMessage = describe(error) }
                }
                deleting = nil
            }
        } message: {
            Text("Der Sachwert und sein Verlauf werden entfernt.")
        }
    }

    private var currency: String { account.currency.isEmpty ? "EUR" : account.currency }

    private var heldItems: [AssetItem] { model.assetItems(account.id).filter { !$0.sold } }

    /// What is held now: sold items are not part of the current inventory.
    private var heldAssets: [ClassifiedAsset] {
        classified.filter { asset in asset.assetItemId.flatMap { model.assetItem($0) }.map { !$0.sold } ?? true }
    }

    private func loadClassification() async {
        let id = account.id
        do {
            // nil: locked or swapped meanwhile; a cancelled load never replaces newer data
            let yesterday = DateText.iso(Calendar.current.date(byAdding: .day, value: -1, to: Date()) ?? Date())
            let priced = model.assetItems(id).filter { !$0.sold && $0.setup.priceSource != "manual" }.map(\.id)
            guard let loaded = try await model.vaultOffMain({ vault in
                (try vault.classifiedAssets(accountId: id),
                 // a missing price leaves the day change unknown, it does not fail the load
                 Dictionary(uniqueKeysWithValues: priced.compactMap { item in
                     ((try? vault.assetUnitValueOn(itemId: item, date: yesterday)) ?? nil).map { (item, $0) }
                 }))
            }), !Task.isCancelled else { return }
            classified = loaded.0
            previousUnits = loaded.1
            classificationError = nil
            classificationLoaded = true
        } catch {
            guard !Task.isCancelled else { return }
            classificationError = describe(error)
            classificationLoaded = true
        }
    }

    private func classificationFailure(_ message: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Label("Klassifizierung konnte nicht geladen werden", systemImage: "exclamationmark.triangle")
                .appFont(.subheadline, weight: .semibold).foregroundStyle(Theme.warning)
            Text("Klassifizierungen fehlen: die Zusammensetzung, das Klassifizieren im Kontextmenü und die Suche nach Klassifizierung sind unvollständig.")
                .appFont(.caption).foregroundStyle(Theme.secondary)
            Text(message).appFont(.caption2).foregroundStyle(Theme.tertiary).lineLimit(3)
            Button("Erneut versuchen") { reloadToken += 1 }.buttonStyle(.borderless).appFont(.caption)
        }
        .padding(.horizontal, 16).padding(.vertical, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Theme.card))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Theme.warning.opacity(0.6), lineWidth: 1))
    }

    // MARK: header

    /// "Sachwerte · Auto (Kombi)" while an item is selected, else "Sachwerte · Gesamt".
    private var title: String {
        "\(model.title) · \(item?.setup.name ?? String(localized: "Gesamt"))"
    }

    private var header: some View {
        AssetHeaderCard(
            account: account,
            title: title,
            valueCents: item?.holding.valueCents ?? account.balanceCents,
            valueLabel: item.map { $0.sold ? String(localized: "Verkauft") : String(localized: "Wert heute") } ?? String(localized: "Wert heute"),
            subtitle: { subtitle },
            valueNote: { EmptyView() },
            accessory: { AssetValueChart(content: chartContent, currency: currency, footnote: DepotValueHistory.footnote) },
            actions: { actions }
        )
    }

    @ViewBuilder
    private var subtitle: some View {
        let line = [account.bankName, account.iban.isEmpty ? "" : formatIBAN(account.iban)].filter { !$0.isEmpty }.joined(separator: " · ")
        let note = model.accountNote(for: account.id)
        if !line.isEmpty {
            Text(line).sensitive().appFont(.caption).foregroundStyle(Theme.secondary)
        }
        if !note.isEmpty {
            Text(note).sensitive().appFont(.caption).foregroundStyle(Theme.secondary).lineLimit(2).help(note)
        }
        statusLine
        if !heldItems.isEmpty {
            Text("\(heldItems.count) Positionen").appFont(.caption).foregroundStyle(Theme.secondary).lineLimit(1)
        }
    }

    /// "Bewertung vom 16.11.2026 · Goldpreis vom 16.11.2026": when the value and the automatic price are from.
    @ViewBuilder
    private var statusLine: some View {
        let priceDate = heldItems.filter { $0.setup.priceSource != "manual" }.map(\.valueDate).filter { !$0.isEmpty }.max()
        // the value of the account is made of valuations of different days: say from which to which
        let valueDates = heldItems.map(\.valueDate).filter { !$0.isEmpty }
        let (first, last) = (valueDates.min(), valueDates.max())
        let valuation: String? = switch (first, last) {
        case let (f?, l?) where f != l: String(localized: "Bewertungen vom \(DateText.medium(f)) bis \(DateText.medium(l))")
        case let (_, l?): String(localized: "Bewertung vom \(DateText.medium(l))")
        default: account.balanceDate.isEmpty ? nil : String(localized: "Bewertung vom \(DateText.medium(account.balanceDate))")
        }
        // the price statement is one phrase: it wraps as a whole, never inside "Goldpreis"
        let parts = [valuation, priceDate.map { String(localized: "Goldpreis vom \(DateText.medium($0))").replacingOccurrences(of: " ", with: "\u{00A0}") }].compactMap { $0 }
        if !parts.isEmpty {
            // a date stays with its "vom"/"bis" (no-break space): the line wraps between the statements, not inside a date
            Text(parts.map { $0.replacingOccurrences(of: "vom ", with: "vom\u{00A0}").replacingOccurrences(of: "bis ", with: "bis\u{00A0}") }
                .joined(separator: " · "))
                .appFont(.caption).foregroundStyle(Theme.secondary).lineLimit(4)
        }
    }

    /// The selected item's history (cut to the selected period), or the account's. The account's
    /// series comes with the selection load: while that is pending it still belongs to the previous
    /// account, so nothing is drawn yet.
    private var chartContent: AssetValueChart.Content {
        let (from, to) = model.periodRange
        let history: [DailyBalance]
        if item != nil {
            history = model.assetStore.selectedHistory.filter { p in
                (from.map { p.date >= $0 } ?? true) && (to.map { p.date <= $0 } ?? true)
            }
        } else if model.selectionSettled {
            history = model.history
        } else {
            return .loading
        }
        // the same chart as the depots: the days before the first holding are no values
        return .depot(DepotValueHistory.assetPoints(history), from: from, to: to)
    }

    @ViewBuilder
    private var actions: some View {
        if model.assetItems(account.id).contains(where: { $0.setup.priceSource != "manual" }) {
            Button {
                model.refreshAssetPrices(userInitiated: true)
            } label: {
                if model.assetStore.refreshing {
                    ProgressView().controlSize(.small)
                } else {
                    Label("Goldpreis aktualisieren", systemImage: "arrow.clockwise")
                }
            }
            .buttonStyle(.borderless)
            .help("Goldpreis jetzt abrufen")
        }
        Button {
            model.openAssetEditor(accountId: account.id)
        } label: {
            Label("Sachwert hinzufügen", systemImage: "plus")
        }
        .buttonStyle(.borderless)
    }
}

/// The held Sachwerte as positions of the shared table: built from the items themselves, so the table
/// never waits for the classification (which is attached where it has loaded).
enum TangiblePositions {
    static func assets(items: [AssetItem], classified: [ClassifiedAsset]) -> [ClassifiedAsset] {
        items.filter { !$0.sold }.map { item in
            var holding = item.holding
            holding.name = item.setup.name
            let classification = classified.first { $0.assetItemId == item.id }?.classification
                ?? AssetClassification(securityKey: nil, assetClass: .unclassified, assetClassSource: .unknown,
                                       instrument: .unclassified, instrumentSource: .unknown)
            return ClassifiedAsset(holding: holding, classification: classification, assetItemId: item.id)
        }
    }

    /// Gain of an item against its purchase data (exact total paid while nothing changed).
    static func gain(_ asset: ClassifiedAsset, items: [AssetItem]) -> HoldingFigures.Gain? {
        guard let id = asset.assetItemId, let item = items.first(where: { $0.id == id }),
              let g = WealthView.tangibleGain(item) else { return nil }
        return HoldingFigures.Gain(cents: g.cents, costCents: asset.holding.valueCents - g.cents, reported: false)
    }

    /// One piece of a manually valued item (house, car) has no market price: the price column would
    /// only repeat the value, so it stays empty.
    static func hidesPrice(_ asset: ClassifiedAsset, items: [AssetItem]) -> Bool {
        guard let id = asset.assetItemId, let item = items.first(where: { $0.id == id }) else { return false }
        return item.setup.priceSource == "manual" && item.holding.quantity == 1
    }

    static func positions(items: [AssetItem], classified: [ClassifiedAsset]) -> [DepotPosition] {
        DepotPosition.make(assets(items: items, classified: classified), gain: { gain($0, items: items) },
                           hidesPrice: { hidesPrice($0, items: items) })
    }
}

/// Held and sold items of a Sachwerte account: the held ones in the shared positions table; the search filters by name and classification.
struct TangibleAssetsList: View {
    @Environment(AppModel.self) private var model
    @Environment(\.reportRedaction) private var redaction
    @AppStorage("showSoldAssets") private var showSold = false
    @State private var sort = DepotSort()
    let account: Account
    let query: String
    let keys: Set<String>?
    let classified: [ClassifiedAsset]
    let wide: Bool
    @Binding var editingClassification: ClassificationEditRequest?
    @Binding var deleting: AssetItem?

    var body: some View {
        let items = model.assetItems(account.id)
        let matching = items.filter { item in
            DepotMetrics.matches(query: query, fields: [item.setup.name, classified.first { $0.assetItemId == item.id }?.classification.compactTitle ?? ""])
        }
        // shares are relative to everything held, whatever the search shows
        let allPositions = TangiblePositions.positions(items: items, classified: classified)
        let heldPositions = DepotPosition.sorted(DepotPosition.filtered(
            allPositions.filter { p in matching.contains { $0.id == p.asset.assetItemId } }, keys: keys, inventory: allPositions), by: sort)
        let sold = matching.filter(\.sold).sorted { $0.setup.name.localizedStandardCompare($1.setup.name) == .orderedAscending }
        if items.isEmpty {
            ContentUnavailableView {
                Label("Noch keine Sachwerte", systemImage: "shippingbox")
            } description: {
                Text("Fügen Sie z. B. Goldmünzen, Barren oder ein Auto hinzu.")
            } actions: {
                Button("Sachwert hinzufügen …") { model.openAssetEditor(accountId: account.id) }
            }
            .padding(.top, 40)
        }
        if !items.isEmpty, matching.isEmpty {
            ContentUnavailableView(
                "Keine Treffer",
                systemImage: "magnifyingglass",
                description: Text("Kein Sachwert passt zu „\(redaction.masking(query))“.")
            )
            .padding(.top, 20)
        }
        if !heldPositions.isEmpty {
            DepotPositionsTable(
                positions: heldPositions, totalCount: allPositions.count, sort: $sort, wide: wide, currency: currency,
                onClassify: { editingClassification = $0 },
                copyAll: { copyToPasteboard(allPositions.map { HoldingFigures.clipboardLine($0.holding) }.joined(separator: "\n")) },
                notesCalculatedGain: false,
                selectedID: allPositions.first { $0.asset.assetItemId == model.assetStore.selected && model.assetStore.selected != nil }?.id,
                onSelect: { p in p.asset.assetItemId.map { model.selectAssetItem($0) } },
                onOpen: { p in
                    guard let id = p.asset.assetItemId else { return }
                    // the second click toggled the selection off again: keep the item selected
                    if model.assetStore.selected != id { model.selectAssetItem(id) }
                    model.openAssetEditor(accountId: account.id, itemId: id)
                },
                rowMenu: { p in
                    if let item = items.first(where: { $0.id == p.asset.assetItemId }) { itemMenu(item) }
                })
        }
        if !sold.isEmpty {
            PositionCard(items: showSold ? sold : []) {
                Button {
                    withAnimation(Motion.animation(.easeInOut(duration: 0.2))) { showSold.toggle() }
                } label: {
                    HStack {
                        Image(systemName: "chevron.right")
                            .appFont(.caption, weight: .semibold)
                            .rotationEffect(.degrees(showSold ? 90 : 0))
                        Text("Ehemalige Positionen (\(sold.count))").appFont(.subheadline, weight: .semibold)
                        Spacer()
                    }
                    .foregroundStyle(Theme.secondary)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityValue(showSold ? Text("Ausgeklappt") : Text("Eingeklappt"))
            } row: { assetRow($0) }
        }
    }

    private var currency: String { account.currency.isEmpty ? "EUR" : account.currency }

    /// The item's own actions, in the table's context menu.
    @ViewBuilder
    private func itemMenu(_ item: AssetItem) -> some View {
        Button("Sachwert bearbeiten …") { model.openAssetEditor(accountId: account.id, itemId: item.id) }
        // gold follows the price API; only manual items carry own values
        if item.setup.priceSource == "manual" {
            Button("Bewertungen bearbeiten …") { model.openAssetAction(.values, itemId: item.id) }
        }
        Button("Anzahl bearbeiten …") { model.openAssetAction(.quantities, itemId: item.id) }
        Button("Sachwert verkaufen …") { model.openAssetAction(.sell, itemId: item.id) }
        Divider()
        Button("Sachwert löschen …", role: .destructive) { deleting = item }
    }

    private func assetRow(_ item: AssetItem) -> some View {
        let selected = model.assetStore.selected == item.id
        return PositionRow(symbol: AssetSymbols.symbol(of: item),
                           name: item.setup.name,
                           detail: detail(item),
                           valueCents: item.holding.valueCents,
                           warning: item.missingPriceDate != nil,
                           selected: selected,
                           dimmed: item.sold,
                           nameRedaction: .personal,
                           classification: classified.first { $0.assetItemId == item.id }?.classification)
            .onTapGesture { model.selectAssetItem(item.id) }
            .simultaneousGesture(TapGesture(count: 2).onEnded {
                // the second click toggled the selection off again: keep the item selected
                if model.assetStore.selected != item.id { model.selectAssetItem(item.id) }
                model.openAssetEditor(accountId: account.id, itemId: item.id)
            })
            .accessibilityAddTraits(selected ? [.isButton, .isSelected] : .isButton)
            .contextMenu {
                if let c = classified.first(where: { $0.assetItemId == item.id })?.classification {
                    Button("Klassifizierung bearbeiten", systemImage: "pencil") {
                        editingClassification = ClassificationEditRequest(name: item.setup.name, classification: c)
                    }
                }
                Button("Sachwert bearbeiten …") { model.openAssetEditor(accountId: account.id, itemId: item.id) }
                // gold follows the price API; only manual items carry own values
                if item.setup.priceSource == "manual" {
                    Button("Bewertungen bearbeiten …") { model.openAssetAction(.values, itemId: item.id) }
                }
                Button("Anzahl bearbeiten …") { model.openAssetAction(.quantities, itemId: item.id) }
                if !item.sold {
                    Button("Sachwert verkaufen …") { model.openAssetAction(.sell, itemId: item.id) }
                }
                Divider()
                Button("Sachwert löschen …", role: .destructive) { deleting = item }
            }
    }

    private func detail(_ item: AssetItem) -> String {
        if item.sold, let sale = item.quantities.last {
            return String(localized: "verkauft am \(DateText.medium(sale.date))")
        }
        if let missing = item.missingPriceDate {
            return String(localized: "Kurs für den \(DateText.medium(missing)) fehlt")
        }
        let qty = "\(NumberText.format(item.holding.quantity)) × \(Money.format(item.holding.currentPriceCents))"
        guard !item.valueDate.isEmpty else { return qty + " · " + String(localized: "noch kein Kurs") }
        let source = item.setup.priceSource == "manual" ? String(localized: "Wert vom \(DateText.medium(item.valueDate))") : String(localized: "Kurs vom \(DateText.medium(item.valueDate))")
        return "\(qty) · \(source)"
    }

    private func cardHeader(_ title: LocalizedStringKey, total: Int64? = nil) -> some View {
        HStack {
            Text(title).appFont(.subheadline, weight: .semibold).foregroundStyle(Theme.text)
            Spacer()
            if let total {
                Text(Money.format(total)).sensitive(.amounts).appFont(.subheadline, monospacedDigit: true).foregroundStyle(Theme.secondary)
            }
        }
    }
}
