import SwiftUI
import QuickLook
import GiroCrabCore

/// The app's scenes, shared by the macOS executable and the Xcode
/// multiplatform target.
public struct GiroCrabScenes: Scene {
    @State private var model: AppModel
    #if os(macOS)
    @Environment(\.openWindow) private var openWindow
    @AppStorage(ViewLayout.inspectorKey) private var showInspector = true
    @AppStorage(ViewLayout.statisticsKey) private var showStatistics = true
    #endif

    public init(options: LaunchOptions = .current) {
        _model = State(initialValue: Self.prepare(options: options))
    }

    /// Creates the app's one model. Call it from the `App`'s `init`, not from its
    /// `body`: SwiftUI re-creates this struct on every evaluation of the app body,
    /// and a model built there (a) is built again each time – `AppModel.init`
    /// opens the vault and loads everything on the main thread – and (b) makes
    /// every property the init reads a dependency of the app body, so the
    /// model's own async loads re-evaluate the body and build the next model.
    /// A trace showed 10–13 such throw-away models per second.
    @discardableResult
    public static func prepare(options: LaunchOptions = .current) -> AppModel {
        if let model = sharedModel { return model }
        let model = AppModel(options: options)
        sharedModel = model
        GiroCrabTips.configure(options)
        return model
    }

    /// The one model of the app process; see `prepare`.
    @MainActor private static var sharedModel: AppModel?

    public var body: some Scene {
        WindowGroup("GiroCrab", id: "main") {
            GiroCrabRootView()
                .environment(model)
                .modifier(AutoLockOnBackground(model: model))
                #if os(macOS)
                // Keep the minimum independent of the split columns' ideal heights.
                .frame(minHeight: 480, maxHeight: .infinity)
                #endif
        }
        #if os(macOS)
        // Fits a 13" MacBook Air (1470 × 956 pt) with room for the Dock.
        .defaultSize(width: 1280, height: 820)
        .windowResizability(.contentMinSize)
        .commands {
            // Ablage › Neu: only commands that create something.
            CommandGroup(replacing: .newItem) {
                Button("Neue Überweisung …") { model.openTransfer() }
                    .keyboardShortcut("n", modifiers: .command)
                    .disabled(!model.canTransfer)
                Button("Neue Kontogruppe …") { model.groupDialog = .create() }
                    .keyboardShortcut("n", modifiers: [.command, .shift])
                    .disabled(!model.isUnlocked)
                Button("Bank hinzufügen …") { model.showAddBank = true }
                    .disabled(!model.isUnlocked)
                Button("Neues Darlehen …") { model.openLoanEditor(.create) }
                    .disabled(!model.isUnlocked)
                Button("Neue Hauptkategorie …") { model.newCategoryRequest = NewCategoryRequest(parent: nil) }
                    .disabled(!model.isUnlocked)
            }
            // Ablage › Import/Export (every toolbar command is also in the menu bar).
            CommandGroup(replacing: .importExport) {
                Button("Aus MoneyMoney importieren …") { model.openMoneyMoneyImport() }
                    .disabled(!model.isUnlocked)
                Button("Buchungen als CSV exportieren …") { model.exportFilteredBookingsCSV() }
                    .disabled(!model.isUnlocked)
            }
            CommandGroup(replacing: .printItem) {
                Button("Papierformat …") { PageSetup.run() }
                    .keyboardShortcut("p", modifiers: [.command, .shift])
                Button("Drucken …") { model.showPrintSheet = true }
                    .keyboardShortcut("p", modifiers: .command)
                    .disabled(!model.isUnlocked)
            }
            // "Bank hinzufügen …" lives in Ablage with the other "create" commands
            // (HIG: one command, one menu – Help search otherwise lists it twice).
            CommandMenu("Konten") {
                Button("Alle Konten abrufen") { model.requestSyncAll() }
                    .keyboardShortcut("r", modifiers: .command)
                    .disabled(!model.isUnlocked || model.bankingBlocksAll)
                Divider()
                Button("Regeln …") { model.openRuleEditor() }
                    .keyboardShortcut("k", modifiers: [.command, .option])
                    .disabled(!model.isUnlocked)
                Button("Wiederkehrende Buchungen …") { model.showRecurringSheet = true }
                    .disabled(!model.isUnlocked)
            }
            CommandMenu("Buchung") {
                // In the list, the single keys C and R do the same (kept for speed).
                Button("Kategorisieren …") { if model.selectedBooking != nil { model.showCategorizer = true } }
                    .keyboardShortcut("c", modifiers: [.command, .control])
                    .disabled(model.selectedBooking == nil)
                Button("Regel für diesen Händler erstellen …") { model.openRuleEditor(from: model.selectedBooking) }
                    .keyboardShortcut("r", modifiers: [.command, .control])
                    .disabled(model.selectedBooking == nil)
                // A checkmark item like "Als Gehalt / Rente zählen"; ⇧⌘U as in Mail
                Toggle("Geprüft", isOn: Binding(
                    get: { model.selectedBooking?.checked ?? false },
                    set: { on in if let b = model.selectedBooking { model.setChecked(on, for: b) } }
                ))
                .keyboardShortcut("u", modifiers: [.command, .shift])
                .disabled(model.selectedBooking == nil)
                // ⌥⇧⌘U as in Mail ("Alle Nachrichten als gelesen markieren"), next to ⇧⌘U
                Button("Alle als gelesen markieren") { model.markAllChecked() }
                    .keyboardShortcut("u", modifiers: [.command, .shift, .option])
                    .disabled(!model.canMarkAllChecked)
                Button("Notiz bearbeiten") { if let b = model.selectedBooking { model.editNote(for: b) } }
                    .disabled(model.selectedBooking == nil)
                // like the Finder (⌘Y); Space in the list does the same
                Button("Beleg in Quick Look anzeigen") {
                    if let b = model.selectedBooking, let d = model.documents(for: b).first { model.quickLook(d) }
                }
                .keyboardShortcut("y", modifiers: .command)
                .disabled(model.selectedBooking.map { model.documents(for: $0).isEmpty } ?? true)
                Divider()
                RecurringMenu(model: model, booking: model.selectedBooking)
                    .disabled(model.selectedBooking == nil)
                Toggle("Als Gehalt / Rente zählen", isOn: Binding(
                    get: { model.selectedBooking?.isSalary ?? false },
                    set: { on in if let b = model.selectedBooking { model.setSalaryFlag(b, on) } }
                ))
                .disabled((model.selectedBooking?.amountCents ?? 0) <= 0)
                Divider()
                // ⌘↑/⌘↓ stay with text fields (start/end of text); ↑/↓ work in the list.
                Button("Nächste Buchung") { model.moveSelection(1) }
                    .keyboardShortcut(.downArrow, modifiers: [.command, .control])
                Button("Vorherige Buchung") { model.moveSelection(-1) }
                    .keyboardShortcut(.upArrow, modifiers: [.command, .control])
                Divider()
                ForEach(Array(BookingScope.ordered.enumerated()), id: \.offset) { i, s in
                    Button(s.title) { model.scope = s }
                        .keyboardShortcut(KeyEquivalent(Character(String(i + 1))), modifiers: .command)
                }
                Divider()
                LoanAssignMenu(model: model, booking: model.selectedBooking)
            }
            CommandGroup(after: .appInfo) {
                if AppUpdates.shared.isAvailable {
                    // Standard place: right below "About", own group (HIG).
                    Button("Nach Updates suchen …") { AppUpdates.shared.checkForUpdates() }
                        .disabled(!AppUpdates.shared.canCheckForUpdates)
                    Divider()
                }
                Button("Tresor sperren") { model.lock() }
                    .keyboardShortcut("l", modifiers: [.command, .control])
                    .disabled(!model.hasOwnVault)
            }
            // `Group`: a CommandsBuilder takes at most 10 children (older Xcode in CI).
            Group {
                // Hilfe › Mitteilungen …: ellipsis because a window opens.
                // Replaces the default Help menu ("Help isn't available for …").
                CommandGroup(replacing: .help) {
                    Button("GiroCrab-Hilfe") { openWindow(id: "help") }
                        .keyboardShortcut("?", modifiers: .command)
                    Divider()
                    Button("Mitteilungen …") { openWindow(id: "announcements") }
                    Divider()
                    Link("Webseite & Dokumentation (girocrab.de)", destination: HelpLinks.website)
                    Link("Fehler melden / Issue erstellen (GitHub)", destination: HelpLinks.issues)
                    Link("Community & Diskussionen (GitHub)", destination: HelpLinks.discussions)
                }
                // Bearbeiten › Suchen …: focuses the toolbar search field.
                CommandGroup(after: .pasteboard) {
                    Divider()
                    Button("Suchen …") { ToolbarSearch.focus() }
                        .keyboardShortcut("f", modifiers: .command)
                        .disabled(!model.isUnlocked)
                }
            }
            SidebarCommands()
            ToolbarCommands()
            CommandGroup(after: .sidebar) {
                Button(showInspector ? "Informationen ausblenden" : "Informationen einblenden") { showInspector.toggle() }
                    .keyboardShortcut("i", modifiers: [.command, .option])
                    .disabled(!model.isUnlocked)
                Toggle("Auswertungen anzeigen", isOn: $showStatistics)
                    .disabled(!model.isUnlocked)
                Divider()
                Button("Befehlspalette …") { model.showCommandPalette = true }
                    .keyboardShortcut("k", modifiers: .command)
                    .disabled(!model.isUnlocked)
                Divider()
                Picker("Konten anzeigen", selection: Binding(get: { model.accountView }, set: { model.accountView = $0 })) {
                    Text("Nach Gruppe").tag(AccountSidebarMode.byGroup)
                    Text("Nach Kontoart").tag(AccountSidebarMode.byKind)
                }
                .disabled(!model.isUnlocked)
                Button("Kontogruppen bearbeiten …") { model.openGroupManager() }
                    .disabled(!model.isUnlocked)
                Divider()
                Button(model.demoActive ? "Eigenen Tresor anzeigen" : "Demo-Daten anzeigen") {
                    if model.demoActive { model.showOwnVault() } else { model.showDemo() }
                }
                .keyboardShortcut("d", modifiers: [.command, .shift])
                .disabled(!model.isUnlocked)
            }
        }
        #endif

        #if os(macOS)
        Window("Mitteilungen", id: "announcements") {
            AnnouncementsListView()
        }
        .defaultSize(width: 460, height: 360)
        Window("Abruf-Protokoll", id: "sync-log") {
            SyncLogView()
                .environment(model)
        }
        .defaultSize(width: 900, height: 520)

        Window("GiroCrab-Hilfe", id: "help") {
            NavigationStack { HelpView() }
        }
        .defaultSize(width: 640, height: 720)
        Settings {
            // HIG: the Settings window fits each tab (SettingsView sizes them).
            SettingsView()
                .environment(model)
        }
        #endif
    }
}

public struct GiroCrabRootView: View {
    @Environment(AppModel.self) private var model
    /// Language question at start-up (#93, macOS only; iOS has a per-app
    /// language setting of its own). Skipped for developer/screenshot launches.
    @State private var languageQuestion: LanguageQuestion = {
        #if os(macOS)
        let o = LaunchOptions.current
        let developerLaunch = o.demoSeed || o.freshVault || o.screenshotPath != nil
        guard LanguagePreference.isAvailable, !developerLaunch || o.simulateSystemLanguage != nil else { return .none }
        return LanguagePreference.current.startupQuestion()
        #else
        return .none
        #endif
    }()
    @Environment(\.scenePhase) private var scenePhase
    /// iOS cold start: keep the animated crab visible for ~1 s, even when the vault
    /// is ready earlier. Only the view is held back; `bootstrap()` runs immediately.
    #if os(iOS)
    @State private var splashHold = true
    #else
    @State private var splashHold = false
    #endif

    public init() {}

    private var splash: some View {
        ZStack {
            Theme.canvas.ignoresSafeArea()
            StartLogo()
            ProgressView().offset(y: 96)
        }
    }

    public var body: some View {
        Group {
            if splashHold && model.vaultState != .starting {
                splash.transition(.opacity)
            } else {
            switch model.vaultState {
            case .starting where languageQuestion == .chooseLanguage:
                LanguagePromptView { languageQuestion = .none }
            case .starting:
                // Same background + centred logo as the system launch screen (LaunchInfo.plist),
                // so there is no visual jump between the two.
                splash
                    .onAppear { model.bootstrap() }
            case .open:
                MainSplitView()
                    .onAppear { StartupClock.mark("MainSplitView appear") }
                    // Asked only once the vault is open, so it never competes with Touch ID.
                    .modifier(SystemLanguageChangedAlert(newLanguage: Binding(
                        get: { if case .systemChanged(let to) = languageQuestion { to } else { nil } },
                        set: { if $0 == nil { languageQuestion = .none } }
                    )))
            case .locked, .unlocking:
                LockView()
            case .failed(let message):
                LockView(error: message)
            }
            }
        }
        .animation(.easeOut(duration: 0.35), value: splashHold)
        .task {
            guard splashHold else { return }
            try? await Task.sleep(for: .seconds(1.0))
            splashHold = false
        }
        .onChange(of: scenePhase) { _, phase in CloudSyncManager.shared.sceneChanged(phase) }
    }
}

/// Locks the personal vault again after the app has been in the background
/// for a while, so the Face ID / passcode prompt returns on the next start.
struct AutoLockOnBackground: ViewModifier {
    let model: AppModel
    @Environment(\.scenePhase) private var phase
    @State private var backgroundedAt: Date?
    private let timeout: TimeInterval = 60

    func body(content: Content) -> some View {
        #if os(iOS)
        content.onChange(of: phase) { _, new in
            switch new {
            case .background:
                backgroundedAt = Date()
            case .active:
                if let t = backgroundedAt, Date().timeIntervalSince(t) >= timeout, model.holdsRealData { model.lock() }
                backgroundedAt = nil
            default: break
            }
        }
        #else
        content
        #endif
    }
}

struct LockView: View {
    @Environment(AppModel.self) private var model
    var error: String?
    /// The unlock has been waiting for a while: most likely a Touch ID or
    /// Keychain prompt the user hasn't noticed (behind the window, other screen).
    @State private var waitingLong = false

    private var waitingHint: LocalizedStringKey {
        #if os(iOS)
        "Bitte bestätigen Sie die Abfrage auf Ihrem Gerät (Face ID oder Code)."
        #else
        "Bitte bestätigen Sie die Abfrage von macOS (Touch ID oder Schlüsselbund). Sie kann hinter diesem Fenster oder auf einem anderen Bildschirm liegen."
        #endif
    }

    private var crabPhase: AnimatedCrabLogoView.Phase {
        if model.unlockCelebrating { return .unlocked }
        if error != nil { return .failed }
        if case .unlocking = model.vaultState { return .unlocking }
        return .idle
    }

    var body: some View {
        VStack(spacing: 18) {
            AnimatedCrabLogoView(size: 120, phase: crabPhase)
            Text("GiroCrab ist gesperrt")
                .font(.title.bold())
            Text("Ihre Buchungen liegen in einem verschlüsselten Tresor.\nDer Schlüssel verlässt nie den Schlüsselbund dieses Geräts.")
                .multilineTextAlignment(.center)
                .foregroundStyle(Theme.secondary)
            Button {
                model.unlock()
            } label: {
                Label("Mit \(VaultKeyManager.shared.biometryName) entsperren", systemImage: VaultKeyManager.shared.biometrySymbol)
                    .padding(.horizontal, 8)
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.large)
            .disabled(model.vaultState == .unlocking)
            if model.vaultState == .unlocking {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    #if os(iOS)
                    Text("Warten auf \(VaultKeyManager.shared.biometryName) …").foregroundStyle(Theme.secondary)
                    #else
                    Text("Warte auf macOS …").foregroundStyle(Theme.secondary)
                    #endif
                }
                if waitingLong {
                    Label(waitingHint, systemImage: "exclamationmark.bubble")
                        .font(.callout)
                        .foregroundStyle(Theme.warning)
                        .multilineTextAlignment(.center)
                        .frame(maxWidth: 460)
                }
            }
            if let error {
                Text(error).font(.callout).foregroundStyle(.red).multilineTextAlignment(.center)
            }
            if model.lockPreview && !AppModel.vaultMayHoldRealData && model.vaultState != .unlocking {
                Button("Zurück zur Demo") { model.endLockPreview() }
                    .buttonStyle(.borderless)
            }
        }
        .padding(40)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.canvas)
        .task {
            if model.vaultState == .locked && error == nil && !model.demoDismissedLock { model.unlock() }
        }
        .task(id: model.vaultState == .unlocking) {
            waitingLong = false
            guard model.vaultState == .unlocking else { return }
            try? await Task.sleep(for: .seconds(5))
            if !Task.isCancelled && model.vaultState == .unlocking { waitingLong = true }
        }
    }
}

struct MainSplitView: View {
    @Environment(AppModel.self) private var model
    @AppStorage(EnableBankingKeySync.noticeSuppressedKey) private var enableBankingNoticeSuppressed = false
    #if os(macOS)
    @State private var columns: NavigationSplitViewVisibility = .all
    @AppStorage(ViewLayout.inspectorKey) private var showInspector = true
    @Environment(\.openSettings) private var openSettings
    #else
    @State private var columns: NavigationSplitViewVisibility = .automatic
    #endif

    private var sidebar: some View {
        SidebarView()
            .frame(minHeight: 0, maxHeight: .infinity)
            .macColumnWidth(min: 240, ideal: 260, max: 480)
    }

    private var bookingColumn: some View {
        Group {
            if model.sidebarSelection == .documents {
                DocumentsView()
            } else {
                BookingStreamView()
            }
        }
        #if os(macOS)
        // The customizable toolbar belongs to the column, not to the view in
        // it: its items must stay when Belege replaces the booking list
        // (AppKit crashes on customizable items that come and go).
        .modifier(MainToolbar())
        // Window title for both views here too, so the toolbar keeps the same
        // compact style (and height) when switching to Belege.
        .modifier(WindowTitle(title: model.title))
        #endif
        .frame(minHeight: 0, maxHeight: .infinity)
    }

    #if os(macOS)
    /// macOS: the detail column is a system inspector (HIG "Inspectors"), toggled
    /// from the toolbar and View menu; the state is remembered (GitHub #234).
    private var splitView: some View {
        NavigationSplitView(columnVisibility: $columns) {
            sidebar
        } detail: {
            // min widths: 240 (sidebar) + 560 + 280 (inspector) keep the minimum window size
            bookingColumn
                .macColumnWidth(min: 560, ideal: 700)
        }
        .navigationSplitViewStyle(.balanced)
        .inspector(isPresented: $showInspector) {
            InspectorView()
                .inspectorColumnWidth(min: 280, ideal: 320, max: 380)
        }
    }
    #else
    private var splitView: some View {
        NavigationSplitView(columnVisibility: $columns) {
            sidebar
        } content: {
            bookingColumn
        } detail: {
            InspectorView()
                .frame(minHeight: 0, maxHeight: .infinity)
        }
        .navigationSplitViewStyle(.balanced)
    }
    #endif

    var body: some View {
        @Bindable var model = model
        splitView
        .quickLookPreview($model.quickLookURL)
        .accountGroupDialogs($model.groupDialog)
        .newCategoryDialog()
        .manualLoanSheets(model)
        .openingBalanceSheet(model)
        #if os(macOS)
        // AppModel.openSettings(_:), e.g. the merchant-logo tip → Settings › Händler-Logos
        .onChange(of: model.settingsRequested) { _, requested in
            guard requested else { return }
            model.settingsRequested = false
            openSettings()
        }
        #endif
        .sheet(isPresented: $model.showGroupManager) {
            GroupManagerSheet()
                .environment(model)
        }
        .sheet(isPresented: $model.showRuleEditor) {
            RuleEditorView(initialDraft: model.ruleDraft)
                .environment(model)
        }
        .sheet(isPresented: $model.showCommandPalette, onDismiss: {
            // Follow-up sheet/popover chosen in the palette: opened once the
            // palette is really gone, instead of after a guessed delay.
            let next = model.afterPaletteDismiss
            model.afterPaletteDismiss = nil
            next?()
        }) {
            CommandPaletteView()
                .environment(model)
        }
        .sheet(item: $model.accountSelectionConnection) { c in
            AccountSelectionSheet(connection: c)
                .environment(model)
        }
        .sheet(isPresented: $model.showAddBank) {
            AddBankSheet()
                .environment(model)
        }
        .sheet(isPresented: $model.showMoneyMoneyImport) {
            MoneyMoneyImportSheet()
                .environment(model)
        }
        .sheet(isPresented: $model.showRecurringSheet) {
            RecurringSheet()
                .environment(model)
        }
        .sheet(item: $model.customRecurringBooking) { b in
            CustomDatesSheet(initialDates: model.recurringSeries(for: b).flatMap { $0.interval == .custom ? $0.customDates : nil } ?? []) { dates in
                model.markRecurring(b, .custom, customDates: dates)
            }
            .environment(model)
        }
        .sheet(isPresented: $model.showCategorizer) {
            CategorizerSheet()
                .environment(model)
        }
        .sheet(isPresented: $model.showTransferSheet) {
            TransferSheetView()
                .environment(model)
                .interactiveDismissDisabled(model.transferBusy)
        }
        .sheet(isPresented: Binding(get: { model.splitBooking != nil }, set: { if !$0 { model.splitBooking = nil } })) {
            if let b = model.splitBooking { SplitSheet(booking: b).environment(model) }
        }
        .sheet(item: $model.payeeCheck) { _ in
            PayeeCheckSheet()
                .environment(model)
                .interactiveDismissDisabled()
        }
        .sheet(item: $model.activeChallenge) { _ in
            TwoFactorSheet()
                .environment(model)
                .interactiveDismissDisabled()
        }
        .sheet(item: $model.webLoginConnection) { c in
            Group {
                if c.connector == .tradeRepublic { WebLoginSheet(connection: c) }
                else if c.connector == .amazonOrders { AmazonWebSheet(connection: c) }
                else if c.connector == .fnz { FnzWebSheet(connection: c) }
                else if c.connector == .quirion { QuirionWebSheet(connection: c) }
                else { PortalWebSheet(connection: c) }
            }
            .environment(model)
        }
        .sheet(item: $model.setupRequest) { req in
            BankSetupSheet(request: req)
                .environment(model)
        }
        .sheet(item: $model.pendingLogin) { login in
            PinSheet(connection: login.connection)
                .environment(model)
        }
        #if os(iOS)
        .sheet(isPresented: $model.showSettings) {
            NavigationStack { SettingsView() }
                .environment(model)
        }
        #endif
        .alert("Status der Überweisung unklar", isPresented: Binding(get: { model.transferUnknown != nil }, set: { if !$0 { model.transferUnknown = nil } })) {
            Button("Verstanden", role: .cancel) { model.transferUnknown = nil }
        } message: {
            if let u = model.transferUnknown {
                Text(String(format: String(localized: "Der Auftrag hat die Bank erreicht, aber die Antwort ist nicht eindeutig. Die Überweisung wurde möglicherweise bereits ausgeführt.\n\nBitte prüfen Sie zuerst Ihr Konto (Umsätze oder Online-Banking), bevor Sie sie erneut senden. In GiroCrab ist sie als „Status unklar“ vorgemerkt.\n\nReferenz (EndToEnd-ID): %@\nBank: %@"), u.endToEndId, u.reason))
            }
        }
        .alert("Stornierung nicht bestätigt", isPresented: Binding(get: { model.transferCancelNotice != nil }, set: { if !$0 { model.transferCancelNotice = nil } })) {
            Button("Status erneut prüfen") { model.recheckTransferCancel() }
            Button("Schließen", role: .cancel) { model.transferCancelNotice = nil }
        } message: {
            Text(model.transferCancelNotice?.message ?? "")
        }
        .alert(model.errorTitle ?? String(localized: "Etwas ist schiefgelaufen"),
               isPresented: Binding(get: { model.errorMessage != nil }, set: { if !$0 { model.errorMessage = nil } })) {
            if let retry = model.errorRetry {
                Button("Erneut versuchen") { model.errorMessage = nil; retry() }
            }
            ForEach(Array(model.errorAlternatives.enumerated()), id: \.offset) { _, alt in
                Button(alt.title) { model.errorMessage = nil; alt.action() }
            }
            Button("OK", role: .cancel) { model.errorMessage = nil }
        } message: {
            Text(model.errorMessage ?? "")
        }
        // After the upgrade: GiroCrab's own FinTS registration or keep the user's (asked once).
        .alert("Welche FinTS-Registrierung soll GiroCrab verwenden?",
               isPresented: Binding(get: { model.offerProductIDChoice }, set: { _ in })) {
            Button("GiroCrab-Registrierung verwenden (empfohlen)") { model.setFintsProductIDMode("builtin") }
                .keyboardShortcut(.defaultAction)
            Button("Meine eigene behalten") { model.setFintsProductIDMode("own") }
        } message: {
            Text("GiroCrab hat jetzt eine eigene FinTS-Produktregistrierung. Sie müssen dafür nichts mehr tun. Ihre eigene Registrierung bleibt gespeichert, und Sie können jederzeit in den Einstellungen wechseln.")
        }
        // own host view: the "don't show again" toggle must not reach other dialogs
        .background {
            Color.clear
                .alert("Enable-Banking-Anwendung synchronisiert",
                       isPresented: Binding(get: { model.enableBankingAdoptNotice != nil },
                                            set: { if !$0 { model.enableBankingAdoptNotice = nil } }),
                       presenting: model.enableBankingAdoptNotice) { _ in
                    Button("OK") {}
                } message: { notice in
                    Text(notice.message)
                }
                .dialogSuppressionToggle(isSuppressed: $enableBankingNoticeSuppressed)
        }
        .overlay(alignment: .bottom) {
            if let toast = model.toast {
                Label(toast, systemImage: "checkmark.circle.fill")
                    .padding(.horizontal, 16).padding(.vertical, 10)
                    .background(.regularMaterial, in: Capsule())
                    .shadow(color: .black.opacity(0.08), radius: 8, y: 2)
                    .padding(.bottom, 22)
                    .transition(Motion.slide(.bottom))
                    // id: a changed or extended notice gets its full time again
                    .task(id: toast) {
                        try? await Task.sleep(nanoseconds: 3_500_000_000)
                        // replaced meanwhile: the new notice's own task takes over
                        guard !Task.isCancelled else { return }
                        withAnimation { model.toast = nil }
                    }
            }
        }
        .animation(Motion.animation(.spring(duration: 0.3)), value: model.toast)
    }
}

/// The launch-screen logo (asset `LaunchLogo`, 120 pt, centred), continued as the animated crab.
/// Same 360-pt artwork grid and backdrop as the launch image, so the hand-off has no jump.
private struct StartLogo: View {
    var body: some View {
        // Square #263040 like the launch image (no squircle corners), crab on top.
        ZStack {
            Rectangle().fill(Color(red: 0x26 / 255, green: 0x30 / 255, blue: 0x40 / 255))
            AnimatedCrabLogoView(size: 120, phase: .idle, showBackgroundSquircle: false)
        }
        .frame(width: 120, height: 120)
    }
}

#if os(macOS)
import AppKit

/// Bearbeiten › Suchen … (⌘F): puts the cursor into the window's toolbar search
/// field (`.searchable(placement: .toolbar)`; `searchFocused` needs macOS 15).
enum ToolbarSearch {
    @MainActor
    static func focus() {
        let window = NSApp.keyWindow ?? NSApp.mainWindow ?? NSApp.windows.first { $0.toolbar != nil }
        guard let window, let item = window.toolbar?.items.compactMap({ $0 as? NSSearchToolbarItem }).first else {
            FileHandle.standardError.write("ToolbarSearch: no search field\n".data(using: .utf8)!)
            return
        }
        window.makeFirstResponder(item.searchField)
    }
}
#endif
