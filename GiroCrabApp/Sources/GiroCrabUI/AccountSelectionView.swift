import SwiftUI
import GiroCrabCore

/// Which accounts of one login are fetched (#346). Accounts that are already
/// reachable through another login are held back until the user decides; skipped
/// accounts stay listed so the choice can be changed later.
struct AccountSelectionList: View {
    @Environment(AppModel.self) private var model
    let connection: BankConnection
    /// Called after a successful save (the sheet closes itself).
    var onSaved: () -> Void = {}
    var showsSaveButton = true
    var saveTitle: LocalizedStringKey = "Auswahl speichern"

    @State private var choices: [ConnectionAccountChoice] = []
    @State private var enabled: Set<String> = []
    @State private var loaded = false
    @State private var error: String?

    var body: some View {
        Group {
            if !loaded {
                ProgressView()
            } else if choices.isEmpty {
                Text("Noch keine Konten abgerufen.").foregroundStyle(.secondary)
            } else {
                ForEach(choices, id: \.accountRef) { c in
                    Toggle(isOn: Binding(
                        get: { enabled.contains(c.accountRef) },
                        set: { on in
                            if on { enabled.insert(c.accountRef) } else { enabled.remove(c.accountRef) }
                        })) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(c.name)
                            Text(detail(c)).font(.caption).foregroundStyle(.secondary)
                            if c.pending {
                                Label("Bereits über „\(c.existingConnection)“ eingebunden", systemImage: "exclamationmark.triangle")
                                    .font(.caption).foregroundStyle(Theme.warning)
                            }
                        }
                    }
                }
                Text("Abrufen: Konto, Umsätze und Dokumente werden über diesen Zugang importiert. Überspringen: nichts davon, bereits gespeicherte Daten bleiben erhalten.")
                    .font(.caption).foregroundStyle(.secondary)
                if let error { Text(error).font(.caption).foregroundStyle(Theme.negative) }
                if showsSaveButton {
                    Button(saveTitle) { save() }
                        .buttonStyle(.borderedProminent)
                }
            }
        }
        .task { await load() }
    }

    private func detail(_ c: ConnectionAccountChoice) -> String {
        [c.number, c.owner].filter { !$0.isEmpty }.joined(separator: " · ")
    }

    private func load() async {
        let id = connection.id
        if let rows = try? await model.vaultOffMain({ try $0.connectionAccountChoices(connectionId: id) }) {
            choices = rows
            // held-back accounts start as "Abrufen" off: the user has to opt in
            enabled = Set(rows.filter(\.enabled).map(\.accountRef))
        }
        loaded = true
    }

    func save() {
        let id = connection.id
        let refs = Array(enabled)
        Task {
            do {
                _ = try await model.requireVaultOffMain { try $0.setConnectionAccountChoices(connectionId: id, enabledRefs: refs) }
                model.reloadAll()
                onSaved()
            } catch {
                self.error = describe(error)
            }
        }
    }
}

/// The choice itself: title, explanation, the list and "Später". Used as a sheet and
/// inside a running portal fetch (before documents are downloaded).
struct AccountSelectionPanel: View {
    let connection: BankConnection
    var onDone: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 6) {
                Text("Konten auswählen – \(connection.displayName)").font(.title3.bold())
                Text("Einige Konten sind bereits über einen anderen Zugang eingebunden. Wählen Sie, welche über diesen Zugang abgerufen werden sollen. Ohne Auswahl werden sie übersprungen.")
                    .font(.callout).foregroundStyle(.secondary)
            }
            .padding(20)
            Divider()
            Form { AccountSelectionList(connection: connection, onSaved: onDone, saveTitle: "Übernehmen und fortfahren") }
                .formStyle(.grouped)
            Divider()
            HStack {
                Spacer()
                Button("Später") { onDone() }
            }
            .padding(14)
        }
    }
}

/// Shown after a fetch when an account was found that another login already delivers.
struct AccountSelectionSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let connection: BankConnection

    var body: some View {
        AccountSelectionPanel(connection: connection) {
            dismiss()
            model.checkPendingAccountSelection()
        }
        .frame(minWidth: 460, idealWidth: 520, minHeight: 560)
    }
}
