import Foundation
import SwiftUI
import GiroCrabCore

extension AppModel {
    // MARK: - Sync (two-phase)

    /// Starts a sync (or the login it needs) once the banking policy is fresh
    /// and allows this login; otherwise explains why not.
    func requestSync(_ connection: BankConnection) {
        withFreshBankingPolicy { [self] in
            if let gate = bankingGate(for: connection) {
                statementsAccountID = nil
                if syncBatchTotal > 1, !gate.all {
                    // A lock published while "Alle abrufen" runs: skip this
                    // login, go on with the rest, name it in the closing notice.
                    syncBatchSkipped.append(String(localized: "\(connection.displayName) (Sicherheitssperre)"))
                    syncNextInQueue()
                } else {
                    resetSyncBatch()
                    showError(gate.title, gate.detail)
                }
                return
            }
            requestSyncChecked(connection)
        }
    }

    private func requestSyncChecked(_ connection: BankConnection) {
        // Amex / Amazon Visa: embedded web-portal login (no API keys needed).
        if usesPortal(connection) {
            webLoginConnection = connection
            return
        }
        // Trade Republic: AWS WAF blocks raw logins – without a stored
        // session the in-app browser login is the default flow.
        let magicDemo = Self.isMagicDemoLogin(connection.loginName)
        if !isDemo, !magicDemo, connection.connector == .tradeRepublic, !(connectionStatus(connection.id)?.hasSession ?? false) {
            webLoginConnection = connection
            return
        }
        if !isDemo, let st = connectionStatus(connection.id), (st.hasPin || (connection.connector == .tradeRepublic && st.hasSession)),
           missingSettings(for: connection).isEmpty {
            startSync(connection, secret: "")
            return
        }
        if !isDemo {
            let missing = missingSettings(for: connection)
            if !missing.isEmpty {
                setupRequest = SetupRequest(connection: connection, keys: missing)
                return
            }
        }
        let needsSecret = !(bankCatalog().first { $0.kind == connection.connector }?.secretLabel.hasPrefix("(") ?? false)
        if isDemo || !needsSecret {
            startSync(connection, secret: isDemo ? "demo" : "")
        } else {
            pendingLogin = PendingLogin(connection: connection)
        }
    }

    func requestSyncAll() {
        // portal connections need an interactive login – not part of "sync all"
        // logins whose accounts are all archived are not asked at all
        requestSyncBatch { [self] in connections.filter { !usesPortal($0) && !$0.allAccountsArchived } }
    }

    /// "Konten abrufen" in a group's menu: the logins behind the group's
    /// accounts, subgroups included, each once – as "Alle abrufen" would.
    func groupSyncConnections(_ group: UserGroup) -> [BankConnection] {
        let ids = Set(userGroupSubtree(group.uuid).flatMap(\.accounts)
            .filter { !$0.isArchived }.compactMap(\.connectionId))
        return connections.filter { ids.contains($0.id) && !usesPortal($0) }
    }

    func requestSyncGroup(_ group: UserGroup) {
        requestSyncBatch { [self] in groupSyncConnections(group) }
    }

    /// What a pull-to-refresh starts besides the iCloud merge.
    enum RefreshSyncAction: Equatable {
        case none
        case connection(Int64)
        case group(String)
        case all
    }

    /// Context-aware bank sync of the current sidebar selection. Import-only
    /// and portal accounts (interactive login) need no bank sync: the iCloud
    /// merge and reload cover them.
    func refreshSyncAction() -> RefreshSyncAction {
        switch sidebarSelection {
        case .account(let id):
            guard let cid = allAccounts.first(where: { $0.id == id })?.connectionId,
                  let c = connections.first(where: { $0.id == cid }),
                  !usesPortal(c), !c.allAccountsArchived else { return .none }
            return .connection(cid)
        case .userGroup(let uuid) where !uuid.isEmpty:
            return .group(uuid)
        case .documents:
            return .none
        default:
            return .all
        }
    }

    /// Pull-to-refresh: merges the iCloud snapshots first (bookings synced on
    /// the Mac), then syncs what the current selection shows. Returns when the
    /// work is done, so the native spinner stays up. While a sync runs it only
    /// waits for it instead of starting a second one.
    func refreshCurrentView() async {
        if let v = ownVault, CloudSyncManager.shared.isEnabled {
            await CloudSyncManager.shared.sync(vault: v)
            reloadAll()
        }
        guard syncingConnectionID == nil else {
            await awaitSyncSettled()
            return
        }
        let action = refreshSyncAction()
        guard action != .none else { return }
        if let hook = refreshSyncOverride { hook(action); return }
        switch action {
        case .none: return
        case .connection(let id):
            guard let c = connections.first(where: { $0.id == id }) else { return }
            requestSync(c)
        case .group(let uuid):
            guard let g = userGroups.first(where: { $0.uuid == uuid }) else { return }
            requestSyncGroup(g)
        case .all:
            requestSyncAll()
        }
        await awaitSyncSettled()
    }

    /// Waits until the running sync (batch included) is over, or stops at a
    /// PIN / TAN / setup prompt, which only the user can answer.
    private func awaitSyncSettled() async {
        var waited = 0
        // the banking policy check may delay the start a moment
        try? await Task.sleep(for: .milliseconds(150))
        while !Task.isCancelled, waited < 600 {
            let prompting = pendingLogin != nil || setupRequest != nil || webLoginConnection != nil || activeChallenge != nil
            if prompting || (syncingConnectionID == nil && syncQueue.isEmpty) { return }
            try? await Task.sleep(for: .milliseconds(200))
            waited += 1
        }
    }

    /// Runs `logins` one after another; read once the banking policy is fresh.
    private func requestSyncBatch(_ logins: @escaping @MainActor () -> [BankConnection]) {
        guard syncingConnectionID == nil else { return }
        withFreshBankingPolicy { [self] in
            guard syncingConnectionID == nil else { return }
            let batch = logins()
            // Logins the banking policy blocks are skipped and named at the end.
            let blocked = batch.filter { bankingGate(for: $0) != nil }
            let allowed = batch.filter { bankingGate(for: $0) == nil }
            guard let head = allowed.first else {
                if let gate = blocked.first.flatMap({ bankingGate(for: $0) }) {
                    showError(gate.title, gate.detail)
                }
                return
            }
            syncQueue = Array(allowed.dropFirst())
            syncBatchTotal = allowed.count
            syncBatchSkipped = blocked.map { String(localized: "\($0.displayName) (Sicherheitssperre)") }
            requestSyncChecked(head)
        }
    }

    /// Position in a running "Alle abrufen" over several logins, e.g. (2, 5).
    var syncBatchProgress: (current: Int, total: Int)? {
        guard syncBatchTotal > 1 else { return nil }
        return (syncBatchTotal - syncQueue.count, syncBatchTotal)
    }

    /// More logins wait after the current one: cancelling a PIN or TAN
    /// prompt then skips this login instead of stopping the whole batch.
    var canSkipInSyncBatch: Bool { !syncQueue.isEmpty }

    /// Cancel button of the TAN sheet. In a batch it drops this login and goes
    /// on with the next one (on the last one it ends the batch with its notice).
    func skipCurrentSync() {
        guard syncBatchTotal > 1, activeTransfer == nil else { cancelSync(); return }
        if let name = activeChallenge?.connection.displayName
            ?? connections.first(where: { $0.id == syncingConnectionID })?.displayName {
            syncBatchSkipped.append(name)
        }
        pollTask?.cancel()
        let s = activeSession
        activeSession = nil
        activeChallenge = nil
        statementsAccountID = nil
        syncGeneration &+= 1
        let generation = syncGeneration
        Task {
            // end the bank dialog first: the next login may be the same bank.
            // syncingConnectionID stays set until then, so neither a scheduled
            // sync nor a new "Alle abrufen" can start in between.
            await s?.cancel()
            // "Alle abbrechen", a lock or a new sync meanwhile own the state now:
            // never end a session this skip did not start.
            guard self.syncGeneration == generation else { return }
            self.syncingConnectionID = nil
            self.syncNextInQueue()
        }
    }

    /// "Alle abbrechen": ends the running sync and closes whichever prompt of
    /// the batch is open (PIN, setup, browser login, TAN).
    func cancelAllSync() {
        setupRequest = nil
        webLoginConnection = nil   // in a batch only the Trade Republic login
        cancelSync()
    }

    /// "Abbrechen" / "Überspringen" in the PIN sheet.
    func cancelPendingLogin() {
        let c = pendingLogin?.connection
        pendingLogin = nil
        skipPrompt(for: c)
    }

    /// "Abbrechen" / "Überspringen" in the setup sheet (missing settings).
    func cancelSetupRequest() {
        let c = setupRequest?.connection
        setupRequest = nil
        skipPrompt(for: c)
    }

    /// "Abbrechen" / "Überspringen" in the Trade Republic browser login.
    func cancelWebLogin() {
        let c = webLoginConnection
        webLoginConnection = nil
        skipPrompt(for: c)
    }

    /// A prompt before the bank dialog (PIN, setup, browser login) was
    /// cancelled: in a batch go on with the next login, otherwise end it –
    /// a stale queue would otherwise resume after a later single sync.
    private func skipPrompt(for connection: BankConnection?) {
        // A cancelled "Kontoauszüge abrufen" must never carry over to the next
        // sync – it would send that login's PIN for another bank's account.
        statementsAccountID = nil
        guard syncBatchTotal > 1 else { resetSyncBatch(); return }
        if let name = connection?.displayName { syncBatchSkipped.append(name) }
        syncNextInQueue()
    }

    func resetSyncBatch() {
        syncQueue = []
        syncBatchTotal = 0
        syncBatchSkipped = []
    }

    /// Toast suffix: new bookings the bank delivered for archived accounts (not stored).
    static func skippedArchivedNote(_ s: SyncSummary) -> String {
        s.transactionsSkippedArchived == 0 ? "" : " · " + String(localized: "\(s.transactionsSkippedArchived) Buchungen archivierter Konten übersprungen")
    }

    /// Locale of the UI language (Settings › Allgemein › Sprache), which can
    /// differ from the system's – e.g. "und" vs. "and" in a list. Builds
    /// without the translations (`swift run`) fall back to the system locale.
    private static var uiLocale: Locale {
        LanguagePreference.isAvailable ? Locale(identifier: LanguagePreference.runningLanguage) : .current
    }

    private func syncNextInQueue() {
        guard syncQueue.isEmpty else {
            requestSync(syncQueue.removeFirst())
            return
        }
        // batch done: name what was skipped, after the last login's own notice
        if !syncBatchSkipped.isEmpty {
            let note = String(localized: "Übersprungen: \(syncBatchSkipped.formatted(.list(type: .and).locale(Self.uiLocale)))")
            toast = toast.map { $0 + " · " + note } ?? note
            // the notice disappears after a few seconds: VoiceOver reads it out
            AccessibilityNotification.Announcement(note).post()
        }
        resetSyncBatch()
    }

    func startSync(_ connection: BankConnection, secret: String) {
        guard let vault else { return }
        if usesPortal(connection) { webLoginConnection = connection; return }
        pendingLogin = nil
        // Statements only for an account of exactly this login: a stale value
        // would hand this login's PIN to another bank.
        if let accountID = statementsAccountID,
           allAccounts.first(where: { $0.id == accountID })?.connectionId != connection.id
            || quietSyncConnectionID == connection.id {
            statementsAccountID = nil
        }
        let statementsFor = statementsAccountID
        syncingConnectionID = connection.id
        syncGeneration &+= 1
        Task {
            var started: SyncSession?
            do {
                let session: SyncSession
                if let accountID = statementsFor {
                    session = try vault.fetchAccountStatements(accountId: accountID, secret: secret, demo: isDemo)
                } else {
                    session = try vault.beginSync(connectionId: connection.id, secret: secret, demo: isDemo)
                }
                started = session
                self.activeSession = session
                let step = try await session.start()
                guard self.activeSession === session else { return }  // skipped / cancelled meanwhile
                self.handle(step, connection: connection)
            } catch {
                if let started, self.activeSession !== started { return }
                // Kept for the "use GiroCrab's registration" retry of a rejected product ID,
                // so the login is not asked again; finishSync takes it and clears it.
                if statementsFor == nil { self.failedSyncSecret = secret }
                self.finishSync(error: error)
            }
        }
    }

    func handle(_ step: SyncStep, connection: BankConnection) {
        // A scheduled sync never asks for a login: the bank wants a new
        // consent, so stop quietly – the connection view shows it.
        if quietSyncConnectionID == connection.id {
            switch step {
            case .needsChallenge, .stillWaiting:
                logSync(connectionID: connection.id, source: .automatic, outcome: .cancelled,
                        message: String(localized: "Automatischer Abruf übersprungen: Die Bank verlangt eine neue Freigabe."))
                quietSyncConnectionID = nil
                let s = activeSession
                activeSession = nil
                syncingConnectionID = nil
                Task { await s?.cancel() }
                checkConsentExpiry()
                return
            case .finished:
                break
            }
        }
        switch step {
        case .needsChallenge(let c):
            activeChallenge = ActiveChallenge(connection: connection, challenge: c, waiting: false)
            if c.kind == .decoupledPush { schedulePoll(connection: connection, interval: c.pollIntervalSeconds) }
        case .stillWaiting(let c):
            if activeChallenge == nil {
                activeChallenge = ActiveChallenge(connection: connection, challenge: c, waiting: true)
            } else {
                activeChallenge?.waiting = true
                activeChallenge?.busy = false
            }
            schedulePoll(connection: connection, interval: c.pollIntervalSeconds)
        case .finished(let s):
            logSync(connectionID: connection.id, source: syncLogSource(for: connection.id), outcome: .ok,
                    imported: Int(s.transactionsImported))
            activeChallenge = nil
            syncingConnectionID = nil
            activeSession = nil
            if let accountID = statementsAccountID {
                statementsAccountID = nil
                documentsVersion += 1
                let n = max(0, accountDocuments(accountID).count - statementsCountBefore)
                toast = String(localized: "\(connection.displayName): \(n) neue Kontoauszüge")
            } else {
                let quiet = quietSyncConnectionID == connection.id
                // scheduled syncs only speak up when something changed
                if !quiet || s.transactionsImported > 0 || !s.newAccounts.isEmpty || !s.revokedAccounts.isEmpty {
                    toast = syncNotice(connection, s) + Self.skippedArchivedNote(s)
                }
            }
            quietSyncConnectionID = nil
            reloadAll()
            checkPendingAccountSelection()
            syncNextInQueue()
        }
    }

    func schedulePoll(connection: BankConnection, interval: UInt32) {
        pollTask?.cancel()
        pollTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: UInt64(max(interval, 1)) * 1_000_000_000)
            guard !Task.isCancelled, let self else { return }
            if let transfer = self.activeTransfer {
                do {
                    let step = try await transfer.poll()
                    guard self.activeTransfer === transfer else { return }  // cancelled meanwhile
                    self.handleTransfer(step, connection: connection)
                } catch {
                    guard self.activeTransfer === transfer else { return }
                    self.finishTransfer(error: error)
                }
                return
            }
            guard let session = self.activeSession else { return }
            do {
                let step = try await session.poll()
                guard self.activeSession === session else { return }  // skipped / cancelled meanwhile
                self.handle(step, connection: connection)
            } catch {
                guard self.activeSession === session else { return }
                self.finishSync(error: error)
            }
        }
    }

    func submitCode(_ code: String) {
        if let transfer = activeTransfer, let ch = activeChallenge {
            activeChallenge?.busy = true
            activeChallenge?.error = nil
            Task {
                do {
                    let step = try await transfer.submitCode(code: code)
                    // cancelled while the TAN was on its way: the cancel notice owns the sheet
                    guard self.activeTransfer === transfer else { return }
                    self.handleTransfer(step, connection: ch.connection)
                } catch {
                    guard self.activeTransfer === transfer else { return }
                    self.finishTransfer(error: error)
                }
            }
            return
        }
        guard let session = activeSession, let ch = activeChallenge else { return }
        activeChallenge?.busy = true
        activeChallenge?.error = nil
        Task {
            do {
                let step = try await session.submitCode(code: code)
                // "Überspringen" while the code was on its way: drop the late answer
                guard self.activeSession === session else { return }
                self.handle(step, connection: ch.connection)
            } catch {
                guard self.activeSession === session else { return }
                self.finishSync(error: error)
            }
        }
    }

    func cancelSync() {
        pollTask?.cancel()
        if let t = activeTransfer {
            activeTransfer = nil
            activeChallenge = nil
            transferBusy = false
            transferError = nil
            // The bank has to confirm: a bunq draft stays approvable until it is rejected.
            cancelTransferSession(t, confirmedMessage: String(localized: "Überweisung abgebrochen – es wurde nichts gesendet, solange die TAN nicht bestätigt wurde."))
            return
        }
        let s = activeSession
        if s != nil, let id = syncingConnectionID {
            logSync(connectionID: id, source: syncLogSource(for: id), outcome: .cancelled,
                    message: String(localized: "Vom Benutzer abgebrochen"))
        }
        activeSession = nil
        activeChallenge = nil
        pendingLogin = nil
        syncingConnectionID = nil
        statementsAccountID = nil
        syncGeneration &+= 1
        resetSyncBatch()
        Task { await s?.cancel() }
    }

    func finishSync(error: Error) {
        let failedID = quietSyncConnectionID ?? syncingConnectionID
        logSync(connectionID: failedID, source: syncLogSource(for: failedID), outcome: .error, message: describe(error))
        let retrySecret = failedSyncSecret
        failedSyncSecret = nil
        if let quietID = quietSyncConnectionID {
            // scheduled sync: a notice instead of dialogs / login prompts
            quietSyncConnectionID = nil
            pollTask?.cancel()
            activeSession = nil
            activeChallenge = nil
            syncingConnectionID = nil
            let name = connections.first { $0.id == quietID }?.displayName ?? ""
            toast = String(localized: "\(name): automatischer Abruf fehlgeschlagen – \(describe(error))")
            return
        }
        statementsAccountID = nil
        syncingConnectionIDBeforeError = syncingConnectionID
        pollTask?.cancel()
        activeSession = nil
        activeChallenge = nil
        syncingConnectionID = nil
        resetSyncBatch()
        if let ce = error as? CoreError, case .BrowserLoginRequired(let m) = ce,
           let conn = connections.first(where: { $0.id == syncingConnectionIDBeforeError }) {
            toast = m
            webLoginConnection = conn
            return
        }
        if let ce = error as? CoreError, case .MissingSetting(let key) = ce,
           let conn = connections.first(where: { $0.id == syncingConnectionIDBeforeError }) {
            var keys = missingSettings(for: conn)
            if !keys.contains(key) { keys.insert(key, at: 0) }
            setupRequest = SetupRequest(connection: conn, keys: keys)
            return
        }
        if let conn = connections.first(where: { $0.id == syncingConnectionIDBeforeError }) {
            showError(String(localized: "Abruf von „\(conn.displayName)“ fehlgeschlagen"), describe(error)) { [weak self] in
                guard let self, let c = self.connections.first(where: { $0.id == conn.id }) else { return }
                self.requestSync(c)
            }
            errorAlternatives = productIDAlternatives(for: conn, error: error, secret: retrySecret)
        } else {
            showError(String(localized: "Abruf fehlgeschlagen"), describe(error))
        }
    }
}

extension AppModel {
    /// Opens the account choice for the first login whose new accounts overlap with
    /// another login (#346). No-op while a choice is already shown.
    func checkPendingAccountSelection() {
        guard accountSelectionConnection == nil else { return }
        Task { [weak self] in
            guard let self,
                  let ids = try? await self.vaultOffMain({ try $0.pendingAccountSelectionConnections() }),
                  let first = ids.first,
                  self.accountSelectionConnection == nil else { return }
            self.accountSelectionConnection = self.connections.first { $0.id == first }
        }
    }
}
