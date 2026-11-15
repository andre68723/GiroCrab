import SwiftUI
import GiroCrabCore

/// Everything the depot view loads for one depot in one pass.
struct DepotData {
    let accountId: Int64
    /// Vault generation the data came from: data of another vault is never shown.
    let revision: Int
    let assets: [ClassifiedAsset]
    let transactions: [SecurityTransaction]
    /// The depot's value history (all of it; the period cuts it for the chart).
    let valuePoints: [DepotValuePoint]
    var holdings: [Holding] { assets.map(\.holding) }
}

enum DepotLoadState {
    case loading
    case failed(String)
    case loaded(DepotData)
}

enum DepotFormat {
    static func percent(_ value: Double, signed: Bool = false, digits: Int = 2) -> String {
        let f = value.formatted(.number.precision(.fractionLength(digits)))
        return (signed && value > 0 ? "+" : "") + f.replacingOccurrences(of: "-", with: "−") + " %"
    }

    static func tint(_ cents: Int64) -> Color { cents > 0 ? Theme.inflow : (cents < 0 ? Theme.negative : Theme.text) }

    static func coverage(_ f: DepotFigure) -> String? {
        f.isPartial ? String(localized: "für \(f.covered) von \(f.total) Positionen") : nil
    }

    static func relative(_ unix: Int64) -> String {
        let f = RelativeDateTimeFormatter()
        f.locale = .autoupdatingCurrent
        f.unitsStyle = .full
        return f.localizedString(for: Date(timeIntervalSince1970: TimeInterval(unix)), relativeTo: Date())
    }
}

/// Toolbar switch (macOS) / header switch (iOS) of the depot view.
struct DepotTabPicker: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        Picker("Ansicht", selection: $model.depotTab) {
            ForEach(DepotTab.allCases) { Text($0.title).tag($0) }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .fixedSize()
        .help("Positionen oder Wertpapieraktivitäten (⌘1, ⌘2)")
    }
}

/// A securities depot as a view of its own: value, four depot figures, positions or activities.
/// No cash flow, budgets, booking scopes or booking filters; the search and the sort are the view's own.
struct DepotDetailView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.reportRedaction) private var redaction
    let account: Account

    @State private var load: DepotLoadState = .loading
    @State private var reloadToken = 0
    @State private var query = ""
    @State private var sort = DepotSort()
    @State private var activityFilter = DepotActivityFilter.all
    @State private var compositionMode = DepotCompositionMode.position
    /// Positions behind the slice clicked in the composition card (nil: all).
    @State private var compositionFilter: Set<String>?
    @State private var editingClassification: ClassificationEditRequest?

    private var currency: String { account.currency.isEmpty ? "EUR" : account.currency }

    /// Data of this depot only: while another depot's data is still on its way, nothing is shown.
    private var data: DepotData? {
        if case .loaded(let d) = load, d.accountId == account.id { return d }
        return nil
    }

    private var syncStamp: Int64 {
        model.connections.first { $0.id == account.connectionId }?.lastSyncUnix ?? 0
    }

    /// Anything that can change what the depot shows reloads it: the account's value, a sync, a
    /// classification, the vault itself.
    private var loadKey: String {
        "\(account.id)-\(account.balanceCents)-\(account.balanceDate)-\(model.assetClassificationVersion)-\(syncStamp)-\(model.vaultRevision)-\(reloadToken)"
    }

    var body: some View {
        // The depot has its own search: it never reads or changes the booking list's search or filters.
        AssetAccountFrame(
            searchText: $query,
            searchPrompt: "Wertpapier, ISIN oder WKN",
            top: { tabPicker },
            header: { header },
            metrics: { wide in
                if let data { metrics(data, wide: wide) }
            },
            analysis: {
                if let data, model.depotTab == .positions, !data.assets.isEmpty { analysisSection(data) }
            },
            content: { wide in content(wide: wide) }
        )
        .capturableSheet(item: $editingClassification) { AssetClassificationSheet(request: $0).environment(model) }
        .task(id: loadKey) { await reload() }
    }

    /// iPhone: the tab switch sits in the page (macOS: in the toolbar).
    @ViewBuilder
    private var tabPicker: some View {
        #if os(iOS)
        DepotTabPicker().frame(maxWidth: .infinity)
        #endif
    }

    @ViewBuilder
    private func content(wide: Bool) -> some View {
        if case .failed(let message) = load {
            failure(message)
        } else if let data {
            switch model.depotTab {
            case .positions: positionsTab(data, wide: wide)
            case .activities: activitiesTab(data, wide: wide)
            }
        } else {
            ProgressView().controlSize(.small).frame(maxWidth: .infinity).padding(.top, 30)
        }
    }

    private func reload() async {
        let id = account.id, revision = model.vaultRevision
        if case .loaded(let d) = load, d.accountId == id, d.revision == revision {} else { load = .loading }
        do {
            let result = try await model.vaultOffMain { vault in
                (try vault.classifiedAssets(accountId: id), try vault.listSecurityTransactions(accountId: id),
                 try vault.depotValueHistory(accountIds: [id], days: DepotValueHistory.lookbackDays))
            }
            // nil: locked or swapped meanwhile; a cancelled load never replaces newer data
            guard !Task.isCancelled, let result else { return }
            load = .loaded(DepotData(accountId: id, revision: revision, assets: result.0, transactions: result.1, valuePoints: result.2))
        } catch {
            guard !Task.isCancelled else { return }
            load = .failed(describe(error))
        }
    }

    private func failure(_ message: String) -> some View {
        ContentUnavailableView {
            Label("Depot konnte nicht geladen werden", systemImage: "exclamationmark.triangle")
        } description: {
            Text(message)
        } actions: {
            Button("Erneut versuchen") { reloadToken += 1 }
        }
        .padding(.top, 30)
    }

    // MARK: header

    private var header: some View {
        let value = DepotValue.resolve(reportedCents: account.balanceCents, holdings: data?.holdings ?? [])
        return AssetHeaderCard(
            account: account,
            title: account.name,
            valueCents: value.cents,
            valueLabel: value.source == .positionSum ? String(localized: "Summe der Positionen") : String(localized: "Depotwert"),
            periodForDepot: true,
            valueHelp: "Depotwert laut Bank. Das Guthaben des Verrechnungskontos ist nicht enthalten.",
            subtitle: {
                if !account.bankName.isEmpty {
                    Text(account.bankName).appFont(.caption).foregroundStyle(Theme.secondary).lineLimit(1)
                }
                statusLine
                if !positionCountText.isEmpty {
                    Text(positionCountText).appFont(.caption).foregroundStyle(Theme.secondary).lineLimit(1)
                }
            },
            valueNote: {
                if let diff = value.differenceCents {
                    Text("Positionen: \(Money.format(value.positionSumCents, currency: currency)) (\(Money.signed(diff, currency: currency)) Differenz)")
                        .sensitive(.amounts)
                        .appFont(.caption2, monospacedDigit: true).foregroundStyle(Theme.warning)
                        .multilineTextAlignment(.trailing)
                        .help("Der gemeldete Depotwert und die Summe der Positionen weichen voneinander ab, z. B. durch unterschiedliche Bewertungszeitpunkte.")
                }
            },
            accessory: {
                AssetValueChart(content: valuationHistory, currency: currency,
                                footnote: DepotValueHistory.footnote)
            },
            actions: { EmptyView() }
        )
    }

    private var positionCountText: String {
        guard let data else { return "" }
        return String(localized: "\(data.assets.count) Positionen")
    }

    /// "Bewertung vom 14.11.2026 · Abruf vor 2 Stunden"; each part only when the data has it.
    @ViewBuilder
    private var statusLine: some View {
        let parts = [
            account.balanceDate.isEmpty ? nil : String(localized: "Bewertung vom \(DateText.medium(account.balanceDate))"),
            syncStamp > 0 ? String(localized: "Abruf \(DepotFormat.relative(syncStamp))") : nil,
        ].compactMap { $0 }
        Text(parts.isEmpty ? String(localized: "Noch kein Abruf") : parts.joined(separator: " · "))
            .appFont(.caption).foregroundStyle(Theme.secondary).lineLimit(2)
    }

    // MARK: four figures

    private func metrics(_ data: DepotData, wide: Bool) -> some View {
        let (from, to) = model.depotPeriodRange
        return DepotMetricGrid(
            tiles: DepotKPIs.securities(assets: data.assets, transactions: data.transactions, currency: currency,
                                        periodTitle: model.depotPeriodTitle, from: from, to: to),
            wide: wide)
    }

    // MARK: positions

    @ViewBuilder
    private func positionsTab(_ data: DepotData, wide: Bool) -> some View {
        let positions = DepotPosition.make(data.assets)
        let shown = DepotPosition.sorted(DepotPosition.filtered(DepotPosition.filtered(positions, query: query), keys: compositionFilter, inventory: positions), by: sort)
        if data.assets.isEmpty {
            ContentUnavailableView(
                "Keine Positionen",
                systemImage: "chart.line.uptrend.xyaxis",
                description: Text("Positionen kommen mit dem nächsten Abruf. Wertpapiergeschäfte stehen unter „Aktivitäten“, Geldbewegungen als Buchungen im Verrechnungskonto.")
            )
            .padding(.top, 30)
        } else {
            if shown.isEmpty {
                ContentUnavailableView(
                    "Keine Treffer",
                    systemImage: "magnifyingglass",
                    description: Text("Keine Position passt zu „\(redaction.masking(query))“.")
                )
                .padding(.top, 20)
            } else {
                DepotPositionsTable(positions: shown, totalCount: positions.count, sort: $sort, wide: wide,
                                    currency: currency, onClassify: { editingClassification = $0 },
                                    copyAll: { copyToPasteboard(positions.map { HoldingFigures.clipboardLine($0.holding) }.joined(separator: "\n")) })
            }
        }
    }

    // MARK: composition and history

    @ViewBuilder
    private func analysisSection(_ data: DepotData) -> some View {
        Text("Aktueller Bestand · unabhängig vom gewählten Zeitraum")
            .appFont(.caption).foregroundStyle(Theme.secondary)
            .padding(.horizontal, 4)
        analysis(data)
    }

    @ViewBuilder
    private func analysis(_ data: DepotData) -> some View {
        AssetCompositionCard(assets: data.assets, currency: currency, mode: $compositionMode, filter: $compositionFilter,
                             expandedKey: "depotAnalysisExpanded", displayKey: "depotCompositionDisplay")
    }

    /// The depot's valuation history for the header chart: the values the core holds (the bank's own
    /// reports at each fetch, imported values), cut to the selected period. Before its first value
    /// the depot has none: nothing is drawn there, and nothing is derived from bookings.
    private var valuationHistory: AssetValueChart.Content {
        guard let data else { return .loading }
        if data.valuePoints.isEmpty {
            return .unavailable(String(localized: "Noch kein Verlauf: der Wert wird ab dem ersten Abruf festgehalten"))
        }
        let (from, to) = model.depotPeriodRange
        return .depot(DepotValueHistory.points(data.valuePoints, from: from, to: to), from: from, to: to)
    }

    // MARK: activities

    @ViewBuilder
    private func activitiesTab(_ data: DepotData, wide: Bool) -> some View {
        let (from, to) = model.depotPeriodRange
        let shown = DepotActivities.filtered(data.transactions, accountId: account.id, filter: activityFilter,
                                             query: query, from: from, to: to)
        DepotActivitiesView(all: data.transactions, shown: shown, filter: $activityFilter, query: query,
                            periodTitle: model.depotPeriodTitle, wide: wide)
    }
}
