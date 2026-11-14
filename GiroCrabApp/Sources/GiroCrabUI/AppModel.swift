import Foundation
import SwiftUI
import Observation
import UniformTypeIdentifiers
import ImageIO
import GiroCrabCore

@MainActor
@Observable
public final class AppModel {
    enum VaultState: Equatable {
        /// First start: the demo is being prepared; a loading view shows meanwhile.
        case starting
        case locked
        case unlocking
        case open
        case failed(String)
    }

    let options: LaunchOptions
    private(set) var vault: Vault?
    /// Manual loans (GitHub #36), see ManualLoansUI.swift.
    @ObservationIgnored let loanStore = ManualLoanStore()
    var vaultState: VaultState = .locked
    /// The vault is open and the lock screen shows the crab's unlock hop for a moment.
    var unlockCelebrating = false

    // Data
    var groups: [AccountGroup] = [] {
        // Dock badge: unchecked bookings of the own vault; locking (ownVault
        // is gone before the lists are emptied) keeps the last known count.
        didSet {
            if dockBadge.record(newCount, ownVaultOpen: holdsRealData) { dockBadge.show() }
        }
    }
    @ObservationIgnored private let dockBadge = DockBadge()
    /// "Nach Gruppe" sidebar view (user groups, then "Ohne Gruppe").
    var userGroups: [UserGroup] = []
    /// Lookups for booking rows (see `rebuildAccountIndex`).
    var accountsByID: [Int64: Account] = [:]
    var accountMarkers: [Int64: AccountMarker] = [:]
    var categories: [GiroCrabCore.Category] = []
    var categoryTree: [CategoryNode] = []
    var bookings: [Booking] = [] { didSet { rebuildBookingIndex() } }
    /// Derived from `bookings` once per reload instead of on every view access.
    var dayGroups: [DayGroup] = [] { didSet { streamItems = StreamItem.flatten(dayGroups) } }
    /// `dayGroups` flattened into headers and rows for the lazy booking stream.
    private(set) var streamItems: [StreamItem] = []
    @ObservationIgnored var bookingIndexByID: [Booking.ID: Int] = [:]
    var page: BookingPage?
    /// Flows of the selected account/group in the selected period; nil = not loaded yet.
    var selectionFlow: (consumption: Int64, invested: Int64, income: Int64)?
    var dashboard: InvestmentDashboard?
    var merchantStats: MerchantStats?
    var rules: [Rule] = []
    var history: [DailyBalance] = []
    var connections: [BankConnection] = []
    /// Login with accounts that another login already delivers: asks which to fetch (#346).
    var accountSelectionConnection: BankConnection?
    var settings: [SettingEntry] = []
    /// "off" (default), "duckduckgo" or "google" – normalised by the core
    /// (legacy "1"/"true" from the old toggle count as "google").
    var logoProvider = "off"
    var onlineLogos: Bool { logoProvider != "off" }
    /// Not observed: signets read it in their body (`cachedLogo`), and filling it
    /// must not re-render every signet on screen.
    @ObservationIgnored var logoCache: [String: (image: Image, pixels: Int)] = [:]

    // UI state
    var sidebarSelection: SidebarItem? = .allAccounts { didSet { listLimit = Self.defaultListLimit; reloadSelection(periodChanged: false) } }
    var scope: BookingScope = .all {
        didSet {
            guard scope != oldValue else { return }
            listLimit = Self.defaultListLimit
            if scope == .investments, dashboardStale { loadDashboard() }
            // A selection still loading in the background has to pick up the new scope.
            if selectionLoadTask != nil { startSelectionLoad() } else { reloadScopeBookings() }
        }
    }
    var period: Period = .all { didSet { periodDidChange() } }
    /// Start/end of the `.custom` period (ISO, inclusive); remembered across launches.
    var customPeriodFrom: String = UserDefaults.standard.string(forKey: "period.customFrom") ?? AppModel.defaultCustomPeriod.from
    var customPeriodTo: String = UserDefaults.standard.string(forKey: "period.customTo") ?? AppModel.defaultCustomPeriod.to
    /// Shows the start/end date dialog of the period filter.
    var showCustomPeriod = false

    /// Sidebar selection and period as set by UI controls: the new data loads in
    /// the background, so the control responds at once. Setting `sidebarSelection`
    /// or `period` directly loads synchronously (the data is there on return).
    var interactiveSidebarSelection: SidebarItem? {
        get { sidebarSelection }
        set { loadsSelectionInBackground = true; sidebarSelection = newValue; loadsSelectionInBackground = false }
    }
    var interactivePeriod: Period {
        get { period }
        set { loadsSelectionInBackground = true; period = newValue; loadsSelectionInBackground = false }
    }
    /// Latest salary of the selected accounts; start of the `.sinceSalary` period.
    var salaryCycle: SalaryCycle?
    /// Recurring series of the selected accounts (GitHub #35).
    var recurring: [RecurringSeries] = [] {
        didSet { recurringByKey = Dictionary(grouping: recurring, by: \.baseKey) }
    }
    /// `recurring` by `baseKey`: every booking row asks `recurringSeries(for:)`,
    /// a lookup instead of a scan over all series.
    var recurringByKey: [String: [RecurringSeries]] = [:]
    /// Recurring bookings of the current month: booked vs. still due.
    var recurringMonth: RecurringMonthOverview?
    var showRecurringSheet = false
    var showAddBank = false
    /// Series hidden with "Nicht wiederkehrend" (can be shown again).
    var recurringHidden: [IgnoredRecurring] = []
    /// Booking whose custom recurrence dates are being edited.
    var customRecurringBooking: Booking?
    /// New / checked only; filtered in the core like the direction filter, so
    /// the count, the pages and the printout follow.
    var statusFilter: StatusFilter = .all {
        didSet {
            guard statusFilter != oldValue else { return }
            listLimit = Self.defaultListLimit
            reloadBookings()
        }
    }

    /// Label of the header filter menu: "Status: Alle" or the active filters.
    var filterTitle: String {
        guard directionFilter != .all else { return String(localized: "Status: \(statusFilter.title)") }
        let kind = directionFilter == .income ? String(localized: "Einnahmen") : String(localized: "Ausgaben")
        return statusFilter == .all ? kind : "\(statusFilter.title) · \(kind)"
    }

    /// Filled icon while any filter is active.
    var filterSymbol: String {
        statusFilter == .all && directionFilter == .all
            ? "line.3.horizontal.decrease" : "line.3.horizontal.decrease.circle.fill"
    }
    /// Income / expenses only (GitHub #53); filtered in the core, so totals and CSV follow.
    var directionFilter: BookingDirection = .all {
        didSet {
            guard directionFilter != oldValue else { return }
            listLimit = Self.defaultListLimit
            reloadBookings()
        }
    }
    var searchText: String = "" { didSet { scheduleSearch() } }
    var selectedBookingID: Booking.ID? {
        didSet {
            if stickyBooking?.id != selectedBookingID { stickyBooking = nil }
            Perf.signposter.emitEvent("selectBooking")
            // A plain selection (click, arrow keys, jumps) replaces a multiple one.
            if let id = selectedBookingID {
                if !selectedBookingIDs.contains(id) { selectedBookingIDs = [id] }
            } else if !selectedBookingIDs.isEmpty {
                selectedBookingIDs = []
            }
            // Cached per merchant; reloadMerchantStats() would drop the cache on every click.
            loadMerchantStats()
            if selectedBookingID != oldValue { scheduleAutoReview() }
        }
    }
    var autoReviewDelay: AutoReviewDelay = .stored {
        didSet { UserDefaults.standard.set(autoReviewDelay.rawValue, forKey: AutoReviewDelay.defaultsKey) }
    }
    @ObservationIgnored var autoReviewTask: Task<Void, Never>?

    /// All selected bookings (⌘-click adds/removes, ⇧-click selects a range,
    /// ⌘A all visible). `selectedBookingID` is the one shown in the inspector.
    var selectedBookingIDs: Set<Booking.ID> = []

    /// A booking that left the filtered list because the user just edited it
    /// (e.g. "kein Gehalt" in the salary period). It stays selected and visible
    /// in the inspector until the user selects something else, like in Mail.
    var stickyBooking: Booking?
    /// iPhone: set by a tap on a booking row, presents the detail sheet.
    var showBookingSheet = false
    var collapsedGroups: Set<Int64> = Set(UserDefaults.standard.array(forKey: "sidebar.collapsedKindGroups") as? [Int64] ?? []) {
        didSet { UserDefaults.standard.set(Array(collapsedGroups), forKey: "sidebar.collapsedKindGroups") }
    }
    var collapsedUserGroups: Set<String> = Set(UserDefaults.standard.stringArray(forKey: "sidebar.collapsedUserGroups") ?? []) {
        didSet { UserDefaults.standard.set(Array(collapsedUserGroups), forKey: "sidebar.collapsedUserGroups") }
    }
    /// Archived accounts stay in their group; this hides their sidebar rows.
    var showArchivedAccounts: Bool = UserDefaults.standard.object(forKey: "sidebar.showArchived") as? Bool ?? true {
        didSet { UserDefaults.standard.set(showArchivedAccounts, forKey: "sidebar.showArchived") }
    }
    /// Explicit per-device choice; `nil` = "Nach Gruppe" once groups exist.
    var accountViewChoice: AccountSidebarMode? = UserDefaults.standard.string(forKey: "sidebar.accountView").flatMap(AccountSidebarMode.init) {
        didSet { UserDefaults.standard.set(accountViewChoice?.rawValue, forKey: "sidebar.accountView") }
    }
    /// Settings tab on screen (set to `.groups` by "Gruppen bearbeiten …").
    var settingsTab: SettingsTab = .general
    /// macOS: the main window opens Settings when this is set (see `openSettings(_:)`).
    var settingsRequested = false
    /// "Gruppen bearbeiten …": the account-group editor sheet (list, details, accounts).
    var showGroupManager = false
    /// (demo, `--demo-settings banks:edit|banks:add`) sheet the bank settings open on appear.
    var demoSettingsSheet: String?
    /// The one group dialog on screen (new / rename / delete / move account).
    var groupDialog: GroupDialog?
    /// The account whose Startsaldo sheet is open (#212).
    var openingBalanceRequest: OpeningBalanceRequest?
    /// The account / group whose "Symbol & Farbe" popover is open.
    var iconColorTarget: IconColorTarget?
    /// The "new category" name prompt on screen (sidebar, menu bar, ⌘K).
    var newCategoryRequest: NewCategoryRequest?
    /// "Belege & PostBox" filtered to one account (#172); nil = all accounts.
    var documentsAccountFilter: Int64?
    var expandedCategories: Set<Int64> = []
    var showRuleEditor = false {
        didSet { if showRuleEditor, rulesStale { reloadRules() } }
    }
    /// Rule match counts / investment dashboard are out of date (category change);
    /// reloaded when the rule editor opens or the Investments scope is shown.
    @ObservationIgnored var rulesStale = false
    @ObservationIgnored var dashboardStale = false
    var ruleDraft: Rule?
    var showSettings = false
    var showCommandPalette = false
    /// The balance chart has keyboard focus (Return there jumps to its day,
    /// instead of editing the selected booking's note).
    @ObservationIgnored var chartHasKeyboardFocus = false
    /// Run after the command palette sheet has been dismissed (opens the next sheet).
    @ObservationIgnored var afterPaletteDismiss: (@MainActor () -> Void)?
    var showCategorizer = false
    /// Booking whose inspector note field should take focus (double-click / Return in the list).
    /// Carries the id because the NoteCard is recreated per booking and claims it on appear.
    var pendingNoteFocus: Booking.ID?
    /// Bumped to hand keyboard focus back to the booking list (e.g. Esc in the note field).
    var focusListToken = 0
    var showMoneyMoneyImport = false
    /// (demo, `--demo-sheet moneymoney`) canned preview so the sheet can be shown without a file.
    var demoImportPreview: MoneyMoneyImportPreview?
    /// (demo, `--demo-sheet moneymoney-db`) canned database preview with invented data.
    var demoDbPreview: MoneyMoneyDbPreview?
    /// (demo, `--demo-sheet add-bank[:search:<text> | :bank:<name>[:setup]]`)
    /// opens "Bank hinzufügen" on that step, for PR screenshots.
    var demoAddBank: String?
    var pendingLogin: PendingLogin?
    var setupRequest: SetupRequest?
    /// Trade Republic in-app browser login (WKWebView).
    /// Not opened while the banking policy blocks this login (security lock).
    var webLoginConnection: BankConnection? {
        didSet {
            if let c = webLoginConnection, let gate = bankingGate(for: c) {
                webLoginConnection = nil
                showError(gate.title, gate.detail)
            }
        }
    }
    /// Connector key per login (`fints:<BLZ>`, …) for the banking policy.
    var connectionKeys: [Int64: String] = [:]
    /// Showing the throw-away demo vault (also when the own vault is still empty).
    var demoActive = false
    /// Where the own Enable Banking application is shared between devices.
    @ObservationIgnored var enableBankingKeyStore: EnableBankingKeyStore = KeychainEnableBankingKeyStore()
    /// Taken over from another device; shown once unless the user turned it off.
    var enableBankingAdoptNotice: EnableBankingAdoptNotice?
    /// The personal vault while demo data is shown (nil if never unlocked).
    private(set) var ownVault: Vault?
    var activeChallenge: ActiveChallenge?
    var payeeCheck: PendingPayeeCheck?
    var syncingConnectionID: Int64?
    var toast: String?
    /// Text of the error alert. Clearing it also clears title and retry.
    var errorMessage: String? {
        didSet { if errorMessage == nil { errorTitle = nil; errorRetry = nil; errorAlternatives = [] } }
    }
    /// Alert title naming what failed (HIG: specific, not "something went wrong").
    var errorTitle: String?
    /// Optional "Erneut versuchen" action of the error alert.
    @ObservationIgnored var errorRetry: (@MainActor () -> Void)?
    /// Extra actions of the error alert after the bank rejected the FinTS registration in use:
    /// replace the own one, or switch to the other one.
    @ObservationIgnored var errorAlternatives: [(title: String, action: @MainActor () -> Void)] = []

    // MARK: FinTS product registration (GiroCrab's own vs. the user's)
    /// This build carries GiroCrab's own registration (release builds only).
    let fintsBuiltinAvailable = fintsBuiltinProductIdAvailable()
    /// Stored choice: "builtin", "own" or "" (not decided yet).
    var fintsProductIDMode = ""
    /// Registration that would be sent now: "builtin", "own" or "" (none).
    var fintsProductIDSource = ""
    /// Existing user with their own ID after an upgrade: ask once which to use.
    var offerProductIDChoice = false
    /// Settings should put the cursor into the field for the own registration.
    var focusOwnProductID = false

    /// Error alert with a specific title and, where it helps, a retry action.
    func showError(_ title: String, _ message: String, retry: (@MainActor () -> Void)? = nil) {
        errorMessage = message
        errorTitle = title
        errorRetry = retry
    }

    var activeSession: SyncSession?
    var activeTransfer: TransferSession?
    var showTransferSheet = false
    /// Bumped whenever a sync wrote a row to the Abrufprotokoll (#238); the log window reloads on it.
    var syncLogVersion = 0
    var csvExport = CSVFileDocument(data: Data())
    var showCSVExporter = false
    /// Print dialog (GitHub #53).
    var showPrintSheet = false
    var transferDraft = TransferDraft()
    var transferError: String?
    /// Instant was not possible and nothing was sent: why (shown in the
    /// transfer sheet with a "send as regular transfer" button).
    var transferInstantDeclined: String?
    /// The order reached the bank but its result is unclear.
    var transferUnknown: TransferUnknownNotice?
    /// The user cancelled, but the bank did not confirm it (re-check or retry).
    var transferCancelNotice: UnconfirmedTransferCancel?
    var splitBooking: Booking?
    var transferBusy = false
    var syncQueue: [BankConnection] = []
    /// "Alle abrufen": number of logins in the running batch (0 = single sync).
    var syncBatchTotal = 0
    /// Logins the user skipped in the running batch, named when it ends.
    var syncBatchSkipped: [String] = []
    /// Bumped by every sync start, cancel, skip and lock: a deferred step of
    /// an older one (e.g. the end of "Überspringen") checks it before acting.
    @ObservationIgnored var syncGeneration = 0
    /// Security lock for bank connections; tests swap in their own instance.
    @ObservationIgnored var bankingPolicy: BankingPolicy = .shared
    /// Scheduled PSD2 sync loop (see PSD2Consent.swift).
    @ObservationIgnored var autoSyncTask: Task<Void, Never>?
    /// The running sync was started by the schedule: no login prompts, no error dialogs.
    var quietSyncConnectionID: Int64?
    var pollTask: Task<Void, Never>?
    var searchTask: Task<Void, Never>?
    var scopeLoadTask: Task<Void, Never>?
    @ObservationIgnored var selectionLoadTask: Task<Void, Never>?
    /// A selection load has run longer than `selectionPlaceholderDelay`: list and chart are
    /// empty and show the loading placeholder until it publishes.
    var isLoadingSelection = false
    /// Grace period before a running selection load clears the previous data. Loads that
    /// finish sooner (most) swap list, chart and tiles in one step: no empty flash.
    /// Slower ones never leave the old figures under the new title for longer than this.
    @ObservationIgnored var selectionPlaceholderDelay: Duration = .milliseconds(200)
    @ObservationIgnored var selectionPlaceholderTask: Task<Void, Never>?
    @ObservationIgnored var loadsSelectionInBackground = false
    @ObservationIgnored var merchantStatsTask: Task<Void, Never>?
    @ObservationIgnored var merchantStatsCache: [String: MerchantStats] = [:]
    /// Merchant whose history `merchantStats` currently shows.
    @ObservationIgnored var merchantStatsKey: String?

    // MARK: State used by AppModel+Loading.swift

    @ObservationIgnored var dashboardTask: Task<Void, Never>?
    /// How many bookings a full reload fetches: one page, or as many as are loaded.
    var listLimit: UInt32 = AppModel.defaultListLimit
    /// Size of the pages appended while scrolling (tests use small pages).
    var appendPageSize: UInt32 = AppModel.defaultListLimit
    /// The next page is being fetched in the background.
    var isLoadingMoreBookings = false
    @ObservationIgnored var loadMoreTask: Task<Void, Never>?
    /// Bumped by every full reload of the list; a page requested before is dropped.
    @ObservationIgnored var listGeneration = 0
    /// Day card the stream should scroll to (set by a click in the balance chart);
    /// a fresh token per click, so clicking the same day again scrolls again.
    var scrollToDay: DayJump?

    // MARK: State used by AppModel+Documents.swift

    /// Folder that receives a copy of every document (nil = off). Per Mac, not per vault.
    var documentMirrorPath: String? = UserDefaults.standard.string(forKey: "documents.mirrorDir")
    /// Result of the latest mirror run, for the settings.
    var documentMirrorStatus: String?
    var mirrorRunning = false
    var documentsVersion = 0
    /// Account whose statements (HKEKA) are being fetched by the running session.
    var statementsAccountID: Int64?
    var statementsCountBefore = 0
    /// Decrypted copy shown in QuickLook; removed again as soon as the
    /// preview closes or shows another document (and on lock / quit).
    var quickLookURL: URL? {
        didSet {
            if let old = oldValue, old != quickLookURL { try? FileManager.default.removeItem(at: old) }
        }
    }

    // MARK: State used by AppModel+Logos.swift

    /// Bumped whenever a domain override changes -> signets refetch.
    var logoVersion = 0
    var logoInflight: [String: Task<(image: Image, pixels: Int)?, Never>] = [:]

    // MARK: State used by AppModel+Editing.swift

    /// Category refresh in flight; only a newer category change or a vault
    /// switch cancels it, so the totals it brings can't get lost.
    @ObservationIgnored var categoryRefreshTask: Task<Void, Never>?

    // MARK: State used by AppModel+Connections.swift

    /// Last queued background settings write; the next one waits for it, so
    /// two quick edits reach the vault in the order they were made.
    @ObservationIgnored var settingsWriteTask: Task<Void, Never>?
    /// All banks Enable Banking offers (`GET /aspsps`); empty until loaded or without credentials.
    var livePsd2Banks: [Psd2Bank] = []
    /// Enable Banking connections the last MoneyMoney import set up, with the
    /// accounts to link to the user's application – shown in the Enable
    /// Banking setup so they can be linked while activating it.
    var psd2LinkHints: [MoneyMoneyDbPsd2Connection] = []

    // MARK: State used by AppModel+Sync.swift

    /// Hook for tests: runs the action's sync instead of the real one.
    @ObservationIgnored var refreshSyncOverride: ((RefreshSyncAction) -> Void)?
    var syncingConnectionIDBeforeError: Int64?
    /// PIN of the sync that just failed (memory only, cleared in `finishSync`).
    var failedSyncSecret: String?

    public init(options: LaunchOptions = .current) {
        self.options = options
        StartupClock.mark("AppModel.init begin")
        if options.freshVault || options.demoSeed {
            // Developer switches only (screenshots): synchronous, not the normal start path.
            GiroCrabCore.setLanguage(code: Bundle.main.preferredLocalizations.first ?? "de")
        }
        if options.freshVault {
            openFreshVault()
            return
        }
        if !options.demoSeed {
            // Normal start: zero I/O and no FFI here. The first frame shows the loading view;
            // everything else (language, vault check, demo seeding) runs in `bootstrap()`.
            vaultState = .starting
            StartupClock.mark("AppModel.init end")
            return
        }
        if options.demoSeed {
            openDemoVault()
            if options.demoSheet == "account-selection" || options.demoSettings == "banks:select" {
                // a second login reaching accounts the first one delivers (#346)
                _ = try? vault?.seedDemoAccountSelection()
                reloadAll()
            }
            switch options.initialView {
            case "alltag": scope = .everyday
            case "sparplaene", "sparpläne", "investments": scope = .investments
            case "belege", "documents": sidebarSelection = .documents
            default: break
            }
            if let q = options.selectAccount, let a = allAccounts.first(where: { $0.name.lowercased().contains(q) }) {
                sidebarSelection = .account(a.id)
                selectedBookingID = bookings.first?.id
            }
            switch options.demoFilter {
            case "income": directionFilter = .income
            case "expense", "expenses": directionFilter = .expense
            default: break
            }
            reloadBookings()
            if let q = options.pickBooking, let b = bookings.first(where: { $0.merchantName.lowercased().contains(q) || $0.counterparty.lowercased().contains(q) }) {
                selectedBookingID = b.id
            }
            if let sheet = options.demoSheet {
                Task { @MainActor [weak self] in
                    try? await Task.sleep(nanoseconds: 600_000_000)
                    self?.presentDemoSheet(sheet)
                }
            }
            if let settings = options.demoSettings {
                let parts = settings.split(separator: ":").map(String.init)
                settingsTab = SettingsTab(demoName: parts[0]) ?? .general
                demoSettingsSheet = parts.count > 1 ? parts[1] : nil
                let groups = parts[0] == "groups"   // the group editor is a sheet of the main window now
                Task { @MainActor [weak self] in
                    try? await Task.sleep(nanoseconds: 600_000_000)
                    if groups { self?.showGroupManager = true } else { self?.settingsRequested = true }
                }
            }
            if options.demoSync {
                Task { @MainActor [weak self] in
                    try? await Task.sleep(nanoseconds: 400_000_000)
                    self?.requestSyncAll()
                }
            }
        }
    }

    /// Screenshot helper: open a sheet pre-filled with fictional data.
    private func presentDemoSheet(_ name: String) {
        switch name {
        case "account-selection":
            // choice of accounts another login already delivers (#346)
            accountSelectionConnection = connections.last
        case "icon-color":
            // the "Symbol & Farbe" popover of the first account (#167)
            if let a = allAccounts.first { iconColorTarget = .account(a.id) }
        case "icon-color-category":
            // the "Symbol & Farbe" popover of the first main category (#168)
            if let c = categories.first(where: { $0.depth == 0 }) { iconColorTarget = .category(c.id) }
        case "category":
            // "Neue Unterkategorie" sheet of the first main category (#168)
            if let c = categories.first(where: { $0.depth == 0 }) {
                newCategoryRequest = NewCategoryRequest(parent: c.id)
            }
        case "print":
            // Print dialog (GitHub #53) on last month's demo bookings.
            period = .lastMonth
            showPrintSheet = true
        case "transfer", "transfer-confirm":
            if let a = transferAccounts.first { sidebarSelection = .account(a.id) }
            openTransfer()
            transferDraft.name = "Stadtwerke München"
            transferDraft.iban = TransferSheetView.groupedIBAN("DE89370400440532013000")
            transferDraft.amountText = Money.plain(8500)
            transferDraft.purpose = "Abschlag Strom Kd-Nr 482910"
        case "moneymoney":
            demoImportPreview = MoneyMoneyImportPreview(
                totalRows: 4820, newBookings: 4820, duplicatesSkippedOrEnriched: 0, notesCount: 312,
                dateRangeStart: "2012-01-03", dateRangeEnd: "2026-10-30",
                detectedCategories: ["Haushalt > Lebensmittel", "Gehalt", "Steuern", "Wohnen > Miete", "Mobilität > Tanken", "Freizeit > Restaurants", "Versicherungen", "Reisen"],
                sampleBookings: [], suggestedAccountName: "MoneyMoney-Archiv", suggestedAccountId: nil)
            showMoneyMoneyImport = true
        case "recurring":
            showRecurringSheet = true
        case "find":
            #if os(macOS)
            ToolbarSearch.focus() // Bearbeiten › Suchen … (⌘F)
            #endif
        case "rules":
            ruleDraft = nil
            showRuleEditor = true
        case "rule-edit":
            // a rule opened in the editor (#179 pattern field)
            ruleDraft = rules.first
            showRuleEditor = true
        case "palette":
            showCommandPalette = true
        case "categorizer":
            if selectedBookingID == nil { selectedBookingID = bookings.first?.id }
            showCategorizer = true
        case "error":
            if let c = connections.first {
                showError(String(localized: "Abruf von „\(c.displayName)“ fehlgeschlagen"),
                          String(localized: "Die Bank hat nicht geantwortet (Zeitüberschreitung).")) { [weak self] in self?.requestSync(c) }
            }
        case "payee-match", "payee-close", "payee-nomatch":
            guard let connection = connections.first else { break }
            let result: PayeeMatch = name == "payee-match" ? .match : name == "payee-close" ? .closeMatch : .noMatch
            payeeCheck = PendingPayeeCheck(connection: connection, check: PayeeCheck(
                result: result, enteredName: "Stadtwerke Muenchen", iban: "DE89370400440532013000",
                bankName: result == .closeMatch ? "Stadtwerke München GmbH" : nil, reason: nil,
                notice: result == .noMatch ? "Wenn Sie die Überweisung trotzdem freigeben, kann das Geld an einen anderen Empfänger gehen." : nil))
        case let n where n == "add-bank" || n.hasPrefix("add-bank:"):
            // the original spelling (LaunchOptions lowercases --demo-sheet): "Sparkasse Köln"
            let args = CommandLine.arguments
            let raw = args.firstIndex(of: "--demo-sheet").flatMap { $0 + 1 < args.count ? args[$0 + 1] : nil } ?? n
            demoAddBank = String(raw.dropFirst("add-bank".count)).trimmingCharacters(in: CharacterSet(charactersIn: ":"))
            showAddBank = true
        case "moneymoney-db":
            #if os(macOS)
            demoDbPreview = Self.demoMoneyMoneyDbPreview(existingGiro: allAccounts.first { $0.kind == .checking })
            #endif
            showMoneyMoneyImport = true
        default: break
        }
    }

    /// Opens the MoneyMoney import. An import belongs in the own vault, so the
    /// demo data an empty vault starts with is left first.
    func openMoneyMoneyImport() {
        if isDemo && !options.demoSeed { showOwnVault() }
        showMoneyMoneyImport = true
    }

    var isDemo: Bool { demoActive }

    /// Login `demo` (case-insensitive, PIN 12345) at any bank: the core simulates the
    /// bank without network access or FinTS product ID (Apple App Review).
    nonisolated static func isMagicDemoLogin(_ login: String) -> Bool {
        login.trimmingCharacters(in: .whitespaces).caseInsensitiveCompare("demo") == .orderedSame
    }

    /// Fixed catalog entry "Demo-Bank (Test & Vorschau)" in the bank search.
    func addDemoBank() {
        addFinTsConnection(name: "Demo-Bank", blz: "99999999", url: "https://demo.invalid/fints", login: "demo")
    }

    /// Loads (or reloads) the throw-away demo vault with fictitious data for all banks,
    /// including the simulated sync / TAN / transfer flows. Own data stays untouched.
    func loadDemoData() {
        openDemoVault()
    }
    var hasOwnVault: Bool { ownVault != nil }

    // MARK: - Vault lifecycle

    /// Called by the root view's first `onAppear`: decides lock vs. demo off the main thread.
    func bootstrap() {
        guard !bootstrapped, vaultState == .starting else { return }
        bootstrapped = true
        _ = Self.quickLookCleanupObserver
        StartupClock.mark("first frame (RootView.onAppear)")
        // last known count, before the vault is unlocked
        dockBadge.show()
        Task { @MainActor [weak self] in
            let mayHoldRealData = await Task.detached(priority: .userInitiated) { () -> Bool in
                // Core texts follow the language the app bundle resolved (German without a bundle).
                GiroCrabCore.setLanguage(code: Bundle.main.preferredLocalizations.first ?? "de")
                // Decrypted QuickLook copies a crash or force quit may have left behind.
                Self.removeQuickLookFiles()
                return Self.vaultMayHoldRealData
            }.value
            guard let self else { return }
            if mayHoldRealData {
                self.vaultState = .locked
            } else {
                await self.openDemoVaultAsync()
            }
        }
    }

    @ObservationIgnored private var bootstrapped = false

    /// Create + seed the throw-away demo vault on a background thread; only the finished
    /// vault is handed back to the main actor.
    private func openDemoVaultAsync() async {
        let result: Result<Vault, Error> = await Task.detached(priority: .userInitiated) {
            Result { try Self.makeDemoVault() }
        }.value
        StartupClock.mark("demo vault opened+seeded (background)")
        switch result {
        case .success(let v): adoptDemoVault(v)
        case .failure(let error): vaultState = .failed(describe(error))
        }
        StartupClock.mark("demo data loaded & shown")
    }

    nonisolated private static func makeDemoVault() throws -> Vault {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("GiroCrab-Demo.girocrab")
        for suffix in ["", "-wal", "-shm"] {
            try? FileManager.default.removeItem(atPath: url.path + suffix)
        }
        // Throw-away demo key (the demo vault holds only fictitious data).
        let key = Data((0..<32).map { UInt8(($0 * 7 + 3) & 0xFF) })
        let v = try Vault.open(path: url.path, key: key)
        _ = try v.seedDemoData()
        return v
    }

    private func adoptDemoVault(_ v: Vault) {
        vault = v
        lockPreview = false
        demoActive = true
        vaultState = .open
        selectedBookingID = nil
        sidebarSelection = .allAccounts
        reloadAll()
        #if os(macOS)
        selectedBookingID = (bookings.first(where: { $0.flow == .consumption && $0.documentCount > 0 }) ?? bookings.first(where: { $0.flow == .consumption }))?.id
        #endif
    }

    private func openDemoVault() {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("GiroCrab-Demo.girocrab")
        for suffix in ["", "-wal", "-shm"] {
            try? FileManager.default.removeItem(atPath: url.path + suffix)
        }
        // Throw-away demo key (the demo vault holds only fictitious data).
        let key = Data((0..<32).map { UInt8(($0 * 7 + 3) & 0xFF) })
        do {
            let v = try Vault.open(path: url.path, key: key)
            _ = try v.seedDemoData()
            vault = v
            lockPreview = false
            demoActive = true
            vaultState = .open
            selectedBookingID = nil
            sidebarSelection = .allAccounts
            reloadAll()
            #if os(macOS)
            // iPhone: a preselected booking would push the detail page over the list.
            selectedBookingID = (bookings.first(where: { $0.flow == .consumption && $0.documentCount > 0 }) ?? bookings.first(where: { $0.flow == .consumption }))?.id
            #endif
        } catch {
            vaultState = .failed(describe(error))
        }
    }

    /// Set once the --fresh-vault file was wiped in this process.
    private static var freshVaultWiped = false

    /// Developer switch --fresh-vault: empty personal vault without Keychain.
    /// Wiped only once per launch: SwiftUI may build further `AppModel`s
    /// (`State(initialValue:)` runs on every `RootView` init), and wiping again
    /// would delete the file the live model is still writing to.
    private func openFreshVault() {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("GiroCrab-Fresh.girocrab")
        // --keep-fresh-vault reopens the file as it is (e.g. a restored copy).
        if !Self.freshVaultWiped, !options.keepFreshVault {
            Self.freshVaultWiped = true
            for suffix in ["", "-wal", "-shm"] {
                try? FileManager.default.removeItem(atPath: url.path + suffix)
            }
        }
        do {
            let v = try Vault.open(path: url.path, key: Data(repeating: 0x5A, count: 32))
            _ = try v.seedStarterCategories()
            ownVault = v
            useVault(v, demo: false)
            if let bank = options.tryAddBank,
               let kind = bankCatalog().first(where: { $0.name.lowercased().contains(bank) })?.kind {
                Task { @MainActor [weak self] in
                    try? await Task.sleep(nanoseconds: 500_000_000)
                    self?.addConnection(kind, login: "kunde-4711")
                }
            }
        } catch {
            vaultState = .failed(describe(error))
        }
    }

    /// Structural trace (no secrets) shared with the core's depot debug log.
    /// Off unless `GIROCRAB_DEPOT_DEBUG=1`; per-user temp dir, mode 0600,
    /// no symbolic links, 5 MB cap (same rules as `depot_debug.rs`).
    static let debugLogEnabled =
        ProcessInfo.processInfo.environment["GIROCRAB_DEPOT_DEBUG"]?
            .trimmingCharacters(in: .whitespaces) == "1"

    static func debugLog(_ msg: String) {
        guard debugLogEnabled else { return }
        let line = "\(Date()) \(msg)\n"
        guard let data = line.data(using: .utf8) else { return }
        let path = FileManager.default.temporaryDirectory
            .appendingPathComponent("girocrab-depot-debug.log").path
        let fd = Darwin.open(path, O_WRONLY | O_CREAT | O_APPEND | O_NOFOLLOW, 0o600)
        guard fd >= 0 else { return }
        defer { Darwin.close(fd) }
        var st = Darwin.stat()
        guard Darwin.fstat(fd, &st) == 0, st.st_uid == Darwin.getuid(), st.st_nlink == 1 else { return }
        if st.st_mode & 0o077 != 0 { Darwin.fchmod(fd, 0o600) }
        if st.st_size > 5 * 1024 * 1024 { Darwin.ftruncate(fd, 0) }
        _ = data.withUnsafeBytes { Darwin.write(fd, $0.baseAddress, $0.count) }
    }

    nonisolated static var vaultURL: URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
            .appendingPathComponent("GiroCrab", isDirectory: true)
        try? FileManager.default.createDirectory(at: base, withIntermediateDirectories: true)
        // Keychain bypass (dev): separate file, so the real vault is never opened with the throw-away key.
        return base.appendingPathComponent(VaultKeyManager.keychainBypassed ? "tresor-dev.girocrab" : "tresor.girocrab")
    }

    func unlock() {
        guard vaultState != .unlocking else { return }
        demoDismissedLock = false
        lockPreview = false
        vaultState = .unlocking
        Task {
            do {
                // A new key only while no vault file exists: with a file, a missing
                // key (e.g. iCloud Keychain not synced yet) is an error, never a
                // fresh key that would shadow the real one.
                let existingSize = (try? FileManager.default.attributesOfItem(atPath: Self.vaultURL.path)[.size] as? Int) ?? 0
                var key = try await VaultKeyManager.shared.unlock(allowCreate: existingSize == 0)
                defer { key.resetBytes(in: 0..<key.count) }
                let fileSize = (try? FileManager.default.attributesOfItem(atPath: Self.vaultURL.path)[.size] as? Int) ?? -1
                Self.debugLog("unlock: vault=\(Self.vaultURL.lastPathComponent) size=\(fileSize) key_created_new=\(VaultKeyManager.shared.lastUnlockCreatedKey)")
                let v = try Vault.open(path: Self.vaultURL.path, key: key)
                _ = try v.seedStarterCategories()
                self.ownVault = v
                // Let the crab's unlock hop play on the lock screen before the main view replaces it.
                if !Motion.reduced {
                    self.unlockCelebrating = true
                    try? await Task.sleep(for: .milliseconds(320))
                }
                defer { self.unlockCelebrating = false }
                if try v.accountCount() == 0 && self.connectionsExist(v) == false {
                    // Nothing synced yet: show demo bookings so the UI can be judged.
                    UserDefaults.standard.set(false, forKey: Self.realDataKey)
                    self.openDemoVault()
                    self.toast = String(localized: "Ihr Tresor ist noch leer – Sie sehen Demo-Daten")
                } else {
                    self.useVault(v, demo: false)
                }
                if CloudSyncManager.shared.isEnabled {
                    await self.performSyncOrAutoAdopt(vault: v)
                }
            } catch {
                if self.ownVault == nil && !Self.vaultMayHoldRealData {
                    // Cancelled before any real data exists: back to the demo, never a dead end.
                    self.openDemoVault()
                } else {
                    self.vaultState = .failed(describe(error))
                }
            }
        }
    }

    /// Merges the other devices' snapshots and writes this device's one.
    /// The open vault does this with its own key: no biometric prompt.
    func syncNow() {
        guard let v = ownVault else { return }
        Task { await self.performSyncOrAutoAdopt(vault: v) }
    }

    /// Secondary device: the user pasted the vault key of the iCloud snapshot.
    /// Merges the snapshot with it, then switches this device's vault and
    /// keychain to that key so both devices share one key from now on.
    func adoptSyncKey(_ text: String) {
        guard let v = ownVault else { return }
        Task {
            do {
                let pasted = try VaultKeyManager.parseKey(text)
                let current = try await VaultKeyManager.shared.unlock()
                let cloud = CloudSyncManager.shared
                await cloud.sync(vault: v, remoteKey: pasted, mergeOnly: true)
                guard cloud.lastError == nil else { self.toast = cloud.lastError; return }
                if pasted != current {
                    try v.rekey(newKey: pasted)
                    do {
                        // On failure importKey writes `current` back into the keychain.
                        try VaultKeyManager.shared.importKey(pasted, replacing: current)
                    } catch {
                        // keep vault and keychain consistent: both on `current` again
                        try? v.rekey(newKey: current)
                        throw error
                    }
                }
                await cloud.sync(vault: v)
                self.reloadAll()
                self.toast = String(localized: "Tresor-Schlüssel übernommen – iCloud Sync aktiv")
            } catch {
                self.toast = describe(error)
            }
        }
    }

    private func performSyncOrAutoAdopt(vault v: Vault) async {
        await CloudSyncManager.shared.sync(vault: v)
        CloudSyncManager.shared.startAutoSync(vault: v) { [weak self] in self?.reloadAll() }
        if ((try? v.accountCount()) ?? 0) > 0 || self.connectionsExist(v) {
            if self.demoActive {
                self.useVault(v, demo: false)
            } else {
                self.reloadAll()
            }
        } else {
            self.reloadAll()
        }
    }

    private func connectionsExist(_ v: Vault) -> Bool {
        ((try? v.listBankConnections()) ?? []).isEmpty == false
    }

    func useVault(_ v: Vault, demo: Bool) {
        vault = v
        demoActive = demo
        if !demo { UserDefaults.standard.set(true, forKey: Self.realDataKey) }
        vaultState = .open
        selectedBookingID = nil
        sidebarSelection = .allAccounts
        logoCache = [:]
        reloadAll()
        selectedBookingID = bookings.first?.id
        if demo { stopAutoSync() } else { startAutoSync() }
        if !demo { reconcileEnableBankingKey() }
    }

    /// Switch to the demo data (own vault stays unlocked in the background).
    func showDemo() {
        guard !demoActive else { return }
        openDemoVault()
    }

    /// Switch back to the personal vault (unlocks it first if needed).
    func showOwnVault() {
        if let ov = ownVault {
            useVault(ov, demo: false)
        } else {
            vaultState = .locked
            unlock()
        }
    }

    /// Set when the user left the demo to the lock screen: no automatic prompt then.
    var demoDismissedLock = false

    nonisolated private static let realDataKey = "girocrab.hasRealData"

    /// Real data may exist in the (encrypted, hence unreadable while locked) vault.
    /// Without a stored answer, an existing vault file counts as real data.
    nonisolated static var vaultMayHoldRealData: Bool {
        if let v = UserDefaults.standard.object(forKey: realDataKey) as? Bool { return v }
        return FileManager.default.fileExists(atPath: vaultURL.path)
    }

    /// The personal vault is what is shown (not the demo): must be locked in the background.
    var holdsRealData: Bool { ownVault != nil && !demoActive }

    /// Demo only: shows the lock screen once so the protection can be tried out.
    /// Unlocking afterwards opens the personal vault (or comes back to the demo while it is empty).
    var lockPreview = false
    func previewLock() {
        guard demoActive, ownVault == nil, !Self.vaultMayHoldRealData else { return }
        lockPreview = true
        demoDismissedLock = true
        vaultState = .locked
    }

    /// Leaves the lock-screen preview back to the demo (never offered with real data).
    func endLockPreview() {
        guard lockPreview, !Self.vaultMayHoldRealData else { return }
        lockPreview = false
        openDemoVault()
    }

    func lock() {
        guard holdsRealData else { return }
        demoDismissedLock = true
        scopeLoadTask?.cancel()
        categoryRefreshTask?.cancel()
        dashboardTask?.cancel()
        endSelectionLoad()
        dropPendingPage()
        merchantStatsTask?.cancel()
        merchantStatsCache = [:]
        searchTask?.cancel()
        loanStore.loadTask?.cancel()
        loanStore.loans = []; loanStore.linkByBooking = [:]
        stopAutoSync()
        endBankDialogsForLock()
        quickLookURL = nil
        Self.removeQuickLookFiles()
        CloudSyncManager.shared.stopAutoSync(flushing: true)
        ownVault = nil
        demoActive = false
        vault = nil
        groups = []; userGroups = []; accountsByID = [:]; accountMarkers = [:]; bookings = []; categories = []; categoryTree = []; rules = []; connections = []; settings = []
        page = nil; dashboard = nil; merchantStats = nil; logoCache = [:]
        vaultState = .locked
    }

    /// Locking ends every bank dialog: no session keeps writing into the vault,
    /// no PIN/TAN prompt comes back after unlocking, no PIN stays in memory.
    private func endBankDialogsForLock() {
        pollTask?.cancel()
        pollTask = nil
        let session = activeSession
        let transfer = activeTransfer
        activeSession = nil
        activeTransfer = nil
        activeChallenge = nil
        payeeCheck = nil
        pendingLogin = nil
        setupRequest = nil
        webLoginConnection = nil
        syncingConnectionID = nil
        quietSyncConnectionID = nil
        statementsAccountID = nil
        syncGeneration &+= 1
        resetSyncBatch()
        transferBusy = false
        showTransferSheet = false
        transferDraft.pin = ""
        Task {
            await session?.cancel()
            _ = await transfer?.cancel()  // result cannot be shown once locked
        }
    }

    // MARK: - Off-main vault access

    /// Runs a core call against the open vault off the main thread. Returns `nil` when the
    /// vault is locked or was swapped (lock, demo switch) while the read ran, so a
    /// caller never publishes data from a vault that is no longer current.
    /// Use this for every core call from UI code that may take longer than a frame.
    func vaultOffMain<T: Sendable>(_ work: @escaping @Sendable (Vault) throws -> T) async throws -> T? {
        guard let vault else { return nil }
        let result = try await Self.offMain { try work(vault) }
        guard self.vault === vault else { return nil }
        return result
    }

    /// For writes and imports: throws when no vault is open, but always returns the
    /// result of a call that ran – a committed write is never reported as failed just
    /// because the vault was swapped meanwhile. Multi-step jobs use `pinVault()`.
    func requireVaultOffMain<T: Sendable>(_ work: @escaping @Sendable (Vault) throws -> T) async throws -> T {
        guard let pin = pinVault() else { throw CoreError.Database(message: "vault locked") }
        return try await pin.run(work)
    }

    /// The open vault, bound for a job of several calls (FNZ import): every step and the
    /// final checkpoint hit the vault the job started on, even if the user locks or
    /// switches to the demo meanwhile. Check `isCurrent` before publishing results.
    func pinVault() -> VaultPin? {
        vault.map { VaultPin(vault: $0) }
    }

    /// Whether `pin` still refers to the open vault.
    func isCurrent(_ pin: VaultPin) -> Bool { vault === pin.vault }

    /// Whether a vault is open – what views check instead of touching `vault`.
    var isUnlocked: Bool { vault != nil }

    /// Validates Enable Banking credentials against the API (async core call).
    func checkEnableBankingApplication(applicationId: String, privateKeyPem: String, redirectUrl: String) async throws -> EnableBankingAppCheck {
        guard let vault else { throw CoreError.Database(message: "vault locked") }
        return try await vault.checkEnableBankingApplication(applicationId: applicationId, privateKeyPem: privateKeyPem, redirectUrl: redirectUrl)
    }

    /// Path of the open vault file (Einstellungen › Tresor).
    var vaultFilePath: String? { vault?.filePath() }

    /// Runs blocking core calls off the main thread; cancelling the caller cancels the work.
    /// File-private: UI code goes through `vaultOffMain` / `VaultPin`, which carry the vault.
    nonisolated static func offMain<T: Sendable>(_ work: @escaping @Sendable () throws -> T) async throws -> T {
        let job = Task.detached(priority: .userInitiated) {
            try Task.checkCancellation()
            return try work()
        }
        return try await withTaskCancellationHandler { try await job.value } onCancel: { job.cancel() }
    }

    // MARK: - Core-call helper

    /// Runs a synchronous core call on the main actor and shows its error, if any.
    func runCore(_ body: () throws -> Void) {
        do { try body() } catch { errorMessage = describe(error) }
    }
}

/// A vault bound to one job (see `AppModel.pinVault`). Views hold this instead of the
/// vault itself; every call runs off the main thread.
struct VaultPin: Sendable {
    fileprivate let vault: Vault

    func run<T: Sendable>(_ work: @escaping @Sendable (Vault) throws -> T) async throws -> T {
        let vault = vault
        return try await AppModel.offMain { try work(vault) }
    }

    /// Flushes the WAL into the vault file (before a file copy, after bulk imports).
    /// Not cancellable: it must also run after a cancelled job.
    func checkpoint() async {
        let vault = vault
        await Task.detached(priority: .userInitiated) { try? vault.checkpoint() }.value
    }
}
