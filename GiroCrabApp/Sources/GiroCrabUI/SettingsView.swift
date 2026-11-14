import SwiftUI
import GiroCrabCore
#if os(macOS)
import AppKit
#endif

/// Einstellungen: Bankzugänge, eigene FinTS-Produkt-ID und API-Schlüssel
/// (im verschlüsselten SQLCipher-Tresor), Händler-Logos, Sicherheit.
struct SettingsView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        TabView(selection: $model.settingsTab) {
            GeneralSettings()
                .settingsPane(height: 600)   // "Sprache" … "Händler-Logos" + "Demo": scrolls
                .tabItem { Label("Allgemein", systemImage: "gearshape") }
                .tag(SettingsTab.general)
            BankConnectionsSettings()
                .settingsPane(height: 600)
                .disabled(!model.isUnlocked)
                .tabItem { Label("Banken", systemImage: "building.columns") }
                .tag(SettingsTab.banks)
            #if os(macOS)
            DocumentFolderSettings()
                .settingsPane(height: 420)
                .disabled(!model.isUnlocked)
                .tabItem { Label("Dokumente", systemImage: "doc.on.doc") }
                .tag(SettingsTab.documents)
            #endif
            SecuritySettings()
                .settingsPane(height: 760)   // incl. "Sicherheitssperre für Bankverbindungen"
                .disabled(!model.isUnlocked)
                .tabItem { Label("Sicherheit", systemImage: "lock.shield") }
                .tag(SettingsTab.security)
            #if os(macOS)
            // Own pane like macOS "Softwareupdate"; works without an open vault.
            // Only in the Developer-ID build (Sparkle registered).
            if AppUpdates.shared.isAvailable {
                UpdateSettings()
                    .settingsPane(height: 360)
                    .tabItem { Label("Updates", systemImage: "arrow.down.circle") }
                    .tag(SettingsTab.updates)
            }
            #endif
        }
        #if os(iOS)
        .navigationTitle("Einstellungen")
        .navigationBarTitleDisplayMode(.inline)
        #endif
    }
}

/// Allgemein: app language (#93). Usable without an open vault.
struct GeneralSettings: View {
    @Environment(AppModel.self) private var model
    @State private var showHelp = false
    @AppStorage(ViewLayout.statisticsKey) private var showStatistics = true

    var body: some View {
        @Bindable var model = model
        Form {
            #if os(iOS)
            // The Mac has these in its menus (Hilfe › GiroCrab-Hilfe, Ansicht › Demo-Daten anzeigen).
            Section {
                Button { showHelp = true } label: {
                    Label("Hilfe & FAQ", systemImage: "questionmark.circle")
                }
            }
            #endif
            LanguageSettingsSection()
            AnnouncementsSettingsSection()
            Section("Ansicht") {
                Toggle("Auswertungen in der Übersicht anzeigen", isOn: $showStatistics)
                Text("Blendet Saldoverlauf, Cashflow und Budgets über der Buchungsliste aus. Auch im Menü Ansicht umschaltbar.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Section("Buchungen") {
                Picker("Als geprüft markieren", selection: $model.autoReviewDelay) {
                    ForEach(AutoReviewDelay.allCases) { Text($0.title).tag($0) }
                }
                Text("Eine ausgewählte Buchung gilt nach dieser Zeit als geprüft – wie eine gelesene E-Mail. Bei Mehrfachauswahl nie.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            BalanceChartSettingsSection()
            MerchantLogoSettingsSection()
            #if os(iOS)
            Section("Demo") {
                Button {
                    model.loadDemoData()
                    model.settingsRequested = false
                } label: {
                    Label("Demo-Tresor / Demo-Daten laden", systemImage: "play.circle")
                }
                Text("Lädt einen Wegwerf-Tresor mit fiktiven Konten bei sieben Banken. „Alle abrufen“, TAN-Dialoge und Überweisungen laufen simuliert, ohne Netzwerk. Ihre eigenen Daten bleiben unberührt.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            #endif
        }
        .formStyle(.grouped)
        #if os(iOS)
        .sheet(isPresented: $showHelp) { HelpSheet() }
        #endif
    }
}

/// Look of the balance history in the header (GitHub #203).
struct BalanceChartSettingsSection: View {
    @AppStorage(BalanceChartStyle.defaultsKey) private var style: BalanceChartStyle = .line
    @AppStorage(BalanceBarUnit.defaultsKey) private var barUnit: BalanceBarUnit = .automatic

    var body: some View {
        Section("Saldoverlauf") {
            Picker("Darstellung", selection: $style) {
                ForEach(BalanceChartStyle.allCases) { Text($0.title).tag($0) }
            }
            if style == .bars {
                Picker("Balken je", selection: $barUnit) {
                    ForEach(BalanceBarUnit.allCases) { Text($0.title).tag($0) }
                }
            }
            Text("Balken zeigen den durchschnittlichen Saldo je Monat (bei kurzen Zeiträumen je Tag, ab sechs Jahren je Jahr) und machen Trends und Jahre vergleichbar. Gestrichelte Linien markieren den Jahreswechsel. Auch per Rechtsklick auf den Saldoverlauf umschaltbar.")
                .font(.caption).foregroundStyle(.secondary)
        }
    }
}

/// Tabs of the settings window (macOS) / sheet (iOS).
enum SettingsTab: Hashable {
    case general, banks, documents, security, updates

    /// `--demo-settings <name>` (screenshots).
    init?(demoName: String) {
        switch demoName {
        case "general": self = .general
        case "banks": self = .banks
        case "credentials": self = .banks   // FinTS registration now lives under "Banken"
        case "logos": self = .general   // merchant logos are a section of "Allgemein"
        case "documents": self = .documents
        case "security": self = .security
        case "updates": self = .updates
        default: return nil
        }
    }
}

struct BankConnectionsSettings: View {
    @Environment(AppModel.self) private var model
    @State private var showAddBank = false
    @State private var editing: BankConnection?
    @State private var confirmRemove: BankConnection?

    var body: some View {
        Form {
            if model.demoActive {
                Section {
                    Label(model.hasOwnVault
                          ? "Sie sehen gerade Demo-Daten. Neue Bankzugänge werden in Ihrem eigenen Tresor angelegt – die App wechselt dabei automatisch dorthin."
                          : "Demo-Modus: Bankzugänge werden hier nur simuliert. Für echte Konten zuerst den eigenen Tresor öffnen.",
                          systemImage: "theatermasks")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                    if !model.hasOwnVault {
                        Button("Eigenen Tresor öffnen") { model.showOwnVault() }
                    }
                }
            }
            Section("Verbundene Banken") {
                if model.connections.isEmpty {
                    Text("Noch keine Bank hinzugefügt.").foregroundStyle(.secondary)
                }
                ForEach(model.connections) { c in
                    HStack(spacing: 10) {
                        RoundedRectangle(cornerRadius: 8, style: .continuous)
                            .fill(c.connector.tint.opacity(0.12))
                            .overlay(Text(c.connector.monogram).font(.caption.bold()).foregroundStyle(c.connector.tint))
                            .frame(width: 30, height: 30)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(c.displayName).fontWeight(.medium)
                            Text(protocolName(c.connector) + (c.loginName.isEmpty ? "" : " · \(c.loginName)"))
                                .font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        if let t = c.lastSyncUnix {
                            Text(Date(timeIntervalSince1970: TimeInterval(t)), style: .relative)
                                .font(.caption).foregroundStyle(.secondary)
                        }
                        Button { editing = c } label: { Image(systemName: "slider.horizontal.3") }
                            .buttonStyle(.borderless)
                            .help("Bearbeiten / Details")
                        Button { model.requestSync(c) } label: { Image(systemName: "arrow.triangle.2.circlepath") }
                            .buttonStyle(.borderless)
                            .help(model.bankingGate(for: c)?.message ?? String(localized: "Jetzt abrufen"))
                            .disabled(model.bankingGate(for: c) != nil)
                        Button(role: .destructive) { confirmRemove = c } label: { Label("Bankzugang entfernen …", systemImage: "trash") }
                            .labelStyle(.iconOnly)
                            .buttonStyle(.borderless)
                            .help("Bankzugang entfernen …")
                    }
                    .contentShape(Rectangle())
                    .onTapGesture { editing = c }
                }
            }
            Section("Bank hinzufügen") {
                Button { showAddBank = true } label: {
                    Label("Bank suchen und hinzufügen …", systemImage: "magnifyingglass")
                }
                Text("Suche nach Bankname, BLZ oder IBAN: FinTS-Banken, Banken mit eigener API (bunq, comdirect, Trade Republic, PayPal) und PSD2-Banken (N26, Revolut, C24 …).")
                    .font(.caption).foregroundStyle(.secondary)
            }
            fintsRegistrationSection
        }
        .formStyle(.grouped)
        .sheet(isPresented: $showAddBank) {
            AddBankSheet()
                .environment(model)
        }
        .sheet(item: $editing) { c in
            ConnectionEditSheet(connection: c)
                .environment(model)
        }
        .confirmRemoveConnection($confirmRemove)
        .task {
            // --demo-settings banks:edit / banks:add (screenshots)
            switch model.demoSettingsSheet {
            case "edit": editing = model.connections.first
            case "select": editing = model.connections.last
            case "add": showAddBank = true
            case "remove": confirmRemove = model.connections.first
            default: break
            }
            model.demoSettingsSheet = nil
        }
    }

    /// Which FinTS product registration is used for all FinTS banks. The server
    /// address and the keys of the other connectors are edited per bank
    /// (sheet "Bearbeiten / Details").
    @ViewBuilder private var fintsRegistrationSection: some View {
        Section {
            if model.fintsBuiltinAvailable {
                Picker("Verwenden", selection: Binding(
                    get: { model.fintsProductIDMode == "own" ? "own" : "builtin" },
                    set: { model.setFintsProductIDMode($0) }
                )) {
                    Text("GiroCrab-Registrierung (empfohlen)").tag("builtin")
                    Text("Eigene Registrierung").tag("own")
                }
                #if os(macOS)
                .pickerStyle(.radioGroup)
                #endif
            } else if AppUpdateChannel.current != .stable {
                Label("Dieser Beta-Build enthält die FinTS-Registrierung von GiroCrab nicht. Bitte melden Sie das den Entwicklern.", systemImage: "exclamationmark.triangle")
                    .font(.callout)
                    .foregroundStyle(Theme.warning)
            }
            // The own ID is entered right here, only when it is the one in use.
            if !model.fintsBuiltinAvailable || model.fintsProductIDMode == "own" {
                OwnProductIDField()
            }
        } header: {
            Text("FinTS-Registrierung")
        } footer: {
            Text(model.fintsBuiltinAvailable
                 ? "Das FinTS-Produkt von GiroCrab ist eingebaut. Eigene API-Anwendungen (zum Beispiel für comdirect oder PayPal) registrieren Sie selbst; die Werte liegen nur in Ihrem verschlüsselten Tresor."
                 : "Nichts davon wird mit dieser Version ausgeliefert. Registrieren Sie Ihr eigenes FinTS-Produkt und Ihre eigenen API-Anwendungen; die Werte liegen nur in Ihrem verschlüsselten Tresor.")
                .font(.footnote)
                .foregroundStyle(.secondary)
                .leadingFooter()
        }
    }

    private func protocolName(_ k: ConnectorKind) -> String {
        bankCatalog().first { $0.kind == k }?.protocol ?? ""
    }
}

/// Merchant logos from the internet (GitHub #33); needs an open vault.
struct MerchantLogoSettingsSection: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Section {
            Picker("Logos aus dem Internet laden", selection: Binding(
                get: { model.logoProvider },
                set: { model.logoProvider = $0; model.saveSettingInBackground("merchants.online_logos", $0) }
            )) {
                Text("Aus").tag("off")
                Text("DuckDuckGo").tag("duckduckgo")
                Text("Google").tag("google")
            }
            Button("Logo-Cache leeren") { model.clearLogoCache() }
        } header: {
            Text("Händler-Logos")
        } footer: {
            Text("Ohne Anbieter zeigt GiroCrab mitgelieferte Markenzeichen, sonst Monogramme und Kategorie-Symbole. Mit Anbieter werden Favicons geladen und verschlüsselt im Tresor zwischengespeichert. Übertragen wird nur die Händler-Domain (z. B. lidl.de) – keine Beträge, keine Kontodaten.")
                .font(.footnote)
                .foregroundStyle(.secondary)
                .leadingFooter()
        }
        .disabled(!model.isUnlocked)
    }
}

#if os(macOS)
/// Automatic copy of all documents (statements, receipts, PostBox) into a
/// folder, e.g. in iCloud Drive (GitHub #57).
struct DocumentFolderSettings: View {
    @Environment(AppModel.self) private var model
    @State private var choosing = false

    var body: some View {
        Form {
            Section("Dokumentenordner") {
                Toggle("Dokumente automatisch in einen Ordner sichern", isOn: Binding(
                    get: { model.documentMirrorPath != nil },
                    set: { on in
                        if !on { model.setDocumentMirror(nil) }
                        else if let url = AppModel.suggestedMirrorURL { model.setDocumentMirror(url) }
                        else { choosing = true }
                    }
                ))
                if let path = model.documentMirrorPath {
                    LabeledContent("Ordner") {
                        HStack {
                            Text(displayPath(path))
                                .lineLimit(1)
                                .truncationMode(.middle)
                                .help(path)
                            Button("Ändern …") { choosing = true }
                            Button("Im Finder zeigen") {
                                NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
                            }
                        }
                    }
                    LabeledContent("Status") {
                        HStack {
                            Text(model.documentMirrorStatus ?? "–").foregroundStyle(.secondary)
                            Button("Jetzt sichern") { model.mirrorDocumentsIfEnabled() }
                        }
                    }
                }
                Text("Kontoauszüge, Belege und PostBox-Dokumente werden nach jedem Sync als Kopie abgelegt: <Ordner>/<Konto>/<Jahr>/. Vorhandene Dateien werden nie überschrieben. Achtung: Im Ordner liegen die PDFs unverschlüsselt – in iCloud Drive also auch in Ihrer iCloud.")
                    .font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Section("Automatischer Abruf") {
                Text("Ob GiroCrab die PDF-Kontoauszüge eines FinTS-Kontos bei jedem Sync mit abruft, stellen Sie pro Konto unter „Dokumente / Kontoauszüge“ ein.")
                    .font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .formStyle(.grouped)
        .fileImporter(isPresented: $choosing, allowedContentTypes: [.folder]) { result in
            if case .success(let url) = result { model.setDocumentMirror(url) }
        }
    }

    /// "~/Library/Mobile Documents/com~apple~CloudDocs/GiroCrab" -> "iCloud Drive/GiroCrab".
    private func displayPath(_ path: String) -> String {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        let cloud = home + "/Library/Mobile Documents/com~apple~CloudDocs"
        if path.hasPrefix(cloud) { return "iCloud Drive" + path.dropFirst(cloud.count) }
        if path.hasPrefix(home) { return "~" + path.dropFirst(home.count) }
        return path
    }
}
#endif

struct SecuritySettings: View {
    @Environment(AppModel.self) private var model
    @State private var requireBiometrics = VaultKeyManager.shared.requireBiometrics
    /// Read once per vault: the core call takes the vault mutex, so not in `body`.
    @State private var vaultPath = "–"

    var body: some View {
        Form {
            Section("Tresor") {
                LabeledContent("Verschlüsselung", value: "SQLCipher 4 (AES-256)")
                LabeledContent("Hauptschlüssel", value: model.isDemo ? String(localized: "Demo-Schlüssel (Wegwerf)") : String(localized: "Schlüsselbund, geschützt durch \(VaultKeyManager.shared.biometryName)"))
                #if os(macOS)
                if !model.isDemo {
                    Toggle("Beim Entsperren \(VaultKeyManager.shared.biometryName) verlangen", isOn: $requireBiometrics)
                        .onChange(of: requireBiometrics) { _, on in
                            guard on != VaultKeyManager.shared.requireBiometrics else { return }
                            Task {
                                do { try await VaultKeyManager.shared.setRequireBiometrics(on) }
                                catch {
                                    requireBiometrics = VaultKeyManager.shared.requireBiometrics
                                    model.toast = error.localizedDescription
                                }
                            }
                        }
                }
                #endif
                LabeledContent("Datei", value: vaultPath)
                    .textSelection(.enabled)
                LabeledContent("Version", value: AppVersionDisplay.current(core: coreVersion()))
                    .textSelection(.enabled)
            }
            ICloudSyncSection()
            BankingPolicySection()
            Section("Datenschutz") {
                Text("GiroCrab verbindet sich direkt von diesem Gerät mit Ihren Banken. Es gibt keinen GiroCrab-Server und kein Tracking. Nur wenn Sie den iCloud-Sync oben einschalten, liegt der verschlüsselte Tresor in Ihrem iCloud Drive. PINs und TANs liegen nur für einen Abruf im Speicher und werden danach gelöscht.")
                    .font(.callout).foregroundStyle(.secondary)
            }
            if model.hasOwnVault && !model.isDemo {
                Section {
                    Button("Jetzt sperren", systemImage: "lock.fill") { model.lock() }
                }
            }
        }
        .formStyle(.grouped)
        .task(id: model.isDemo) { vaultPath = model.vaultFilePath ?? "–" }
    }
}

#if os(macOS)
/// Einstellungen › Updates (nur im Developer-ID-Build mit Sparkle; im App-Store-
/// oder Xcode-Build ist kein Updater registriert und der Tab fehlt). Bedienbar
/// auch ohne geöffneten Tresor.
///
/// Die Kanäle Beta und Nightly sind bewusst versteckt: Die Auswahl erscheint
/// erst, wenn bei geöffnetem Tab die Wahltaste (⌥) gedrückt wird, und bleibt
/// dann sichtbar, bis die Einstellungen geschlossen werden – oder immer, wenn
/// schon ein Vorabkanal aktiv ist, damit man ihn jederzeit verlassen kann. Der
/// Wechsel in einen weniger stabilen Kanal muss bestätigt werden; „Abbrechen“
/// ist dabei die Standardtaste (HIG: riskante Aktion nie als Default).
struct UpdateSettings: View {
    @Bindable private var updates = AppUpdates.shared
    @State private var channelRevealed = NSEvent.modifierFlags.contains(.option)
    @State private var flagsMonitor: Any?
    @State private var pendingChannel: AppUpdateChannel?

    private var showsChannelPicker: Bool {
        channelRevealed || updates.channel != .stable
    }

    var body: some View {
        Form {
            Section {
                Toggle("Automatisch nach Updates suchen", isOn: $updates.automaticallyChecksForUpdates)
                Toggle("Updates automatisch laden und installieren", isOn: $updates.automaticallyDownloadsUpdates)
                    .disabled(!updates.automaticallyChecksForUpdates || !updates.allowsAutomaticUpdates)
            }
            Section {
                if showsChannelPicker {
                    Picker(selection: Binding(
                        get: { updates.channel },
                        set: { new in
                            if new.risk > updates.channel.risk { pendingChannel = new } else { updates.setChannel(new) }
                        }
                    )) {
                        Text("Stabil").tag(AppUpdateChannel.stable)
                        Text("Beta").tag(AppUpdateChannel.beta)
                        Text("Nightly").tag(AppUpdateChannel.nightly)
                    } label: {
                        if updates.channel == .stable {
                            Text("Update-Kanal")
                        } else {
                            Label("Update-Kanal", systemImage: "exclamationmark.triangle.fill")
                                .foregroundStyle(.orange)
                        }
                    }
                }
                LabeledContent("Zuletzt gesucht") {
                    if let date = updates.lastUpdateCheckDate {
                        Text(date, format: .relative(presentation: .named))
                    } else {
                        Text("Noch nie")
                    }
                }
                Button("Nach Updates suchen …") { updates.checkForUpdates() }
                    .disabled(!updates.canCheckForUpdates)
            } footer: {
                Group { switch updates.channel {
                case .stable:
                    Text("Updates werden nur installiert, wenn sie mit dem GiroCrab-Schlüssel signiert und von Apple notarisiert sind.")
                case .beta:
                    Text("Beta-Versionen sind Vorabversionen zum Testen. Neuere stabile Versionen erhalten Sie weiterhin. Ein Wechsel zurück installiert keine ältere Version.")
                case .nightly:
                    Text("Nightly-Builds sind ungetestete Entwicklungsstände. Neuere Beta- und stabile Versionen erhalten Sie weiterhin. Ein Wechsel zurück installiert keine ältere Version.")
                } }
                .leadingFooter()
            }
        }
        .formStyle(.grouped)
        .alert(
            pendingChannel == .nightly ? "Nightly-Builds erhalten?" : "Beta-Versionen erhalten?",
            isPresented: Binding(get: { pendingChannel != nil }, set: { if !$0 { pendingChannel = nil } }),
            presenting: pendingChannel
        ) { channel in
            Button(channel == .nightly ? "Nightly-Builds aktivieren" : "Beta-Versionen aktivieren", role: .destructive) {
                updates.setChannel(channel)
            }
            Button("Abbrechen", role: .cancel) {}
                .keyboardShortcut(.defaultAction)
        } message: { channel in
            if channel == .nightly {
                Text("Nightly-Builds entstehen aus dem aktuellen Entwicklungsstand und sind nicht getestet. Sie können Fehler enthalten oder Ihre Daten so verändern, dass ältere Versionen sie nicht mehr lesen können. Legen Sie vorher eine Sicherung Ihres Tresors an.")
            } else {
                Text("Beta-Versionen sind Vorabversionen zum Testen. Sie können Fehler enthalten oder Ihre Daten so verändern, dass ältere Versionen sie nicht mehr lesen können. Legen Sie vorher eine Sicherung Ihres Tresors an.")
            }
        }
        .onAppear {
            flagsMonitor = NSEvent.addLocalMonitorForEvents(matching: .flagsChanged) { event in
                if event.modifierFlags.contains(.option) { channelRevealed = true }
                return event
            }
        }
        .onDisappear {
            if let flagsMonitor { NSEvent.removeMonitor(flagsMonitor) }
            flagsMonitor = nil
            channelRevealed = false
        }
    }
}
#endif

/// Kompakter iCloud-Drive-Sync (nur die verschlüsselte Tresordatei).
struct ICloudSyncSection: View {
    @Environment(AppModel.self) private var model
    @State private var sync = CloudSyncManager.shared
    @State private var picking = false
    @State private var keychainSync = VaultKeyManager.shared.syncViaICloudKeychain
    @State private var showKeySheet = false
    @State private var keyCopied = false

    var body: some View {
        Section("iCloud Sync") {
            Toggle("Tresor über iCloud synchronisieren", isOn: Bindable(sync).isEnabled)
                .onChange(of: sync.isEnabled) { _, on in
                    guard on else { return }
                    Task {
                        // Share the vault key via iCloud Keychain (not as a file in iCloud Drive).
                        if !keychainSync {
                            if (try? await VaultKeyManager.shared.setICloudKeychainSync(true)) != nil { keychainSync = true }
                        }
                        model.syncNow()
                    }
                }
            if sync.isEnabled {
                LabeledContent("Ordner", value: sync.folderName ?? "iCloud Drive / GiroCrab")
                // The app's own iCloud container is neither visible in Finder nor
                // chosen by the user: only a plain folder gets these buttons.
                if sync.location == .folder {
                    #if os(macOS)
                    Button("Im Finder zeigen", systemImage: "folder") { revealFolder() }
                    #else
                    Button("Ordner wählen …", systemImage: "folder") { picking = true }
                    #endif
                }
                Button(sync.isSyncing ? LocalizedStringKey("Synchronisiere …") : LocalizedStringKey("Jetzt synchronisieren"), systemImage: "arrow.triangle.2.circlepath") {
                    model.syncNow()
                }
                .disabled(sync.isSyncing)
                if sync.isSyncing { ProgressView().controlSize(.small) }
                if let d = sync.lastSync {
                    LabeledContent("Letzter Sync", value: d.formatted(date: .abbreviated, time: .shortened))
                }
                if let s = sync.lastStats { Text(s).font(.caption).foregroundStyle(.secondary) }
                if let e = sync.lastError { Text(e).font(.caption).foregroundStyle(.red) }
                Toggle("Tresor-Schlüssel über iCloud-Schlüsselbund teilen", isOn: $keychainSync)
                    .onChange(of: keychainSync) { _, on in
                        guard on != VaultKeyManager.shared.syncViaICloudKeychain else { return }
                        Task {
                            do { try await VaultKeyManager.shared.setICloudKeychainSync(on) }
                            catch { keychainSync = VaultKeyManager.shared.syncViaICloudKeychain; model.toast = error.localizedDescription }
                        }
                    }
                HStack {
                    Button(keyCopied ? "Schlüssel kopiert (60 s)" : "Tresor-Schlüssel kopieren", systemImage: "key") {
                        Task {
                            if let k = try? await VaultKeyManager.shared.exportKeyBase64() {
                                VaultKeyManager.copyToPasteboard(k)
                                keyCopied = true
                            }
                        }
                    }
                    Button("Schlüssel einfügen …", systemImage: "square.and.pencil") { showKeySheet = true }
                }
                if sync.keyMismatch {
                    Label("Der iCloud-Tresor nutzt einen anderen Schlüssel. Fügen Sie den Schlüssel von Ihrem anderen Gerät ein.", systemImage: "exclamationmark.triangle.fill")
                        .font(.caption).foregroundStyle(.orange)
                }
            }
            Text(sync.location == .container
                 ? LocalizedStringKey("Synchronisiert Ihren verschlüsselten Tresor über iCloud. Der Schlüssel liegt nie in iCloud Drive: Er wird über den iCloud-Schlüsselbund geteilt oder einmalig auf dem zweiten Gerät eingefügt.")
                 : LocalizedStringKey("Synchronisiert Ihren verschlüsselten Tresor über iCloud Drive (Ordner „GiroCrab“). Der Schlüssel liegt nie im Ordner: Er wird über den iCloud-Schlüsselbund geteilt oder einmalig auf dem zweiten Gerät eingefügt."))
                .font(.caption).foregroundStyle(.secondary)
        }
        .onChange(of: sync.keyMismatch) { _, bad in if bad { showKeySheet = true } }
        .sheet(isPresented: $showKeySheet) {
            KeyPasteSheet().environment(model)
        }
        #if os(iOS)
        .fileImporter(isPresented: $picking, allowedContentTypes: [.folder]) { r in
            if case .success(let url) = r {
                do { try sync.setFolder(url); model.syncNow() } catch { }
            }
        }
        #endif
    }

    #if os(macOS)
    private func revealFolder() {
        let cloudDocs = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Mobile Documents/com~apple~CloudDocs", isDirectory: true)
        let mm = cloudDocs.appendingPathComponent("moneymoney", isDirectory: true)
        let gc = cloudDocs.appendingPathComponent("GiroCrab", isDirectory: true)
        let url = SyncFolder.containsVault(mm) ? mm : gc
        if FileManager.default.fileExists(atPath: url.path) {
            NSWorkspace.shared.activateFileViewerSelecting([url])
        } else {
            NSWorkspace.shared.open(cloudDocs)
        }
    }
    #endif
}

/// Fallback when the iCloud snapshot uses another key: paste the vault key
/// copied from the first device ("Tresor-Schlüssel kopieren").
struct KeyPasteSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var text = ""

    private var valid: Bool { (try? VaultKeyManager.parseKey(text)) != nil }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Tresor-Schlüssel einfügen").font(.headline)
            Text("Kopieren Sie den Schlüssel auf Ihrem anderen Gerät (Einstellungen › iCloud Sync › „Tresor-Schlüssel kopieren“) und fügen Sie ihn hier ein. Er wird im Schlüsselbund gespeichert, nie in iCloud Drive.")
                .font(.callout).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            SecureField("Schlüssel (Base64 oder Hex)", text: $text)
                .textFieldStyle(.roundedBorder)
                .autocorrectionDisabled()
            HStack {
                Button("Abbrechen", role: .cancel) { dismiss() }.keyboardShortcut(.cancelAction)
                Spacer()
                Button("Übernehmen") {
                    model.adoptSyncKey(text)
                    text = ""
                    dismiss()
                }
                .keyboardShortcut(.defaultAction)
                .buttonStyle(.borderedProminent)
                .disabled(!valid)
            }
        }
        .padding(24)
        .macFrame(minWidth: 420)
    }
}

private extension View {
    /// macOS: each Settings tab has its own size and the window follows it
    /// (HIG: no large empty panes); iOS: the sheet decides.
    @ViewBuilder
    func settingsPane(height: CGFloat) -> some View {
        #if os(macOS)
        frame(width: 680, height: height)
        #else
        self
        #endif
    }
}


/// Einstellungen › Mitteilungen: one switch, on by default (quiet banner only).
struct AnnouncementsSettingsSection: View {
    @State private var center = AnnouncementCenter.shared

    var body: some View {
        Section("Mitteilungen") {
            Toggle("Mitteilungen des GiroCrab-Projekts anzeigen", isOn: $center.isEnabled)
            Text("Gelegentliche Neuigkeiten, unabhängig von Updates. GiroCrab lädt dazu eine signierte Textdatei von GitHub, ohne Kennung und ohne Tracking. Mitteilungen erscheinen nie als Fenster und lassen sich jederzeit ausblenden. Sicherheitswarnungen sind davon unabhängig.")
                .font(.callout).foregroundStyle(.secondary)
        }
    }
}

/// The user's own FinTS product registration. Applies like every other setting: on Return
/// and when the field is left, no extra button. The value stays hidden (it is not shared).
struct OwnProductIDField: View {
    @Environment(AppModel.self) private var model
    @State private var text = ""
    @FocusState private var focused: Bool

    private var hasValue: Bool { model.settingEntry("fints.product_id")?.hasValue == true }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text("Eigene Produktregistrierung")
                Spacer()
                SecureField("Eigene Produktregistrierung", text: $text,
                            prompt: Text(hasValue ? "Gespeichert – zum Ersetzen neu eingeben" : "Registrierungsnummer eintragen"))
                    .labelsHidden()
                    .frame(maxWidth: 340)
                    .multilineTextAlignment(.trailing)
                    .focused($focused)
                    .onSubmit(commit)
            }
            Text("Nur nötig, wenn Sie Ihre eigene Registrierung statt der von GiroCrab verwenden. Sie bleibt gespeichert, auch wenn Sie die von GiroCrab nutzen.")
                .font(.caption).foregroundStyle(.secondary)
            Link("Produktregistrierung bei der Deutschen Kreditwirtschaft beantragen",
                 destination: URL(string: "https://www.fints.org/de/hersteller/produktregistrierung")!)
                .font(.caption)
        }
        .onAppear(perform: focusIfAsked)
        .onChange(of: model.focusOwnProductID) { _, _ in focusIfAsked() }
        .onChange(of: focused) { _, isFocused in if !isFocused { commit() } }
        #if os(macOS)
        .onDisappear(perform: commit)
        #endif
    }

    /// An empty field changes nothing (the stored value is never shown or cleared by accident).
    private func commit() {
        let value = text.trimmingCharacters(in: .whitespaces)
        text = ""
        guard !value.isEmpty else { return }
        model.saveSettingInBackground("fints.product_id", value)
    }

    /// After a rejected registration the alert sends the user here.
    private func focusIfAsked() {
        guard model.focusOwnProductID else { return }
        model.focusOwnProductID = false
        focused = true
    }
}
