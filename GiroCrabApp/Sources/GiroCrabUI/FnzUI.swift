import SwiftUI
import WebKit
import GiroCrabCore

/// FNZ's own authenticated portal calls.
enum FnzScripts {
    static let probe = #"""
    (() => {
      if (location.hostname !== 'portal.fnz.de' || !/\/EOY\/EL\//.test(location.pathname)
        || !/#\/eo\//.test(location.hash)) return false;
      const text = document.body?.innerText || '';
      // The compact layout hides "Abmelden" inside the hamburger menu.
      // Account/depot detail links also prove the overview has finished loading.
      const signedIn = /Logout|Abmelden/i.test(text)
        || !!document.querySelector('a[href*="/services/logout"]')
        || !!document.querySelector('a[href*="#/eo/bestand/"]');
      return signedIn && /Gesamtbestand|Postkorb|Depot/i.test(text);
    })()
    """#

    static let overview = #"""
    const r = await fetch(location.origin + location.pathname + 'eox/rest/depot/getDepotOverviewDataForEndCustomer', {
      credentials: 'include', headers: {'Accept':'application/json', 'X-EOX-Session':'',
        'X-EOX-CLIENT-VERSION':'26.4.01', 'X-Context-ID':crypto.randomUUID()}
    });
    if (!r.ok) throw new Error('FNZ: Kontoübersicht nicht abrufbar (HTTP ' + r.status + '). Bitte erneut anmelden.');
    const data = await r.json();
    if (!Array.isArray(data.ownerGroups)) throw new Error('FNZ: Keine Kontoübersicht erkannt. Bitte erneut anmelden.');
    return JSON.stringify(data);
    """#

    /// Read every available transaction page and the current investment-fund
    /// inventory. No order creation, execution or document status changes.
    static let snapshot = #"""
    const base = location.origin + location.pathname + 'eox/rest/';
    const headers = {'Accept':'application/json','content-type':'application/json',
      'X-EOX-Session':'','X-EOX-CLIENT-VERSION':'26.4.01'};
    const get = async (path, body) => {
      const r = await fetch(base + path, {method:body === undefined ? 'GET' : 'POST', credentials:'include',
        headers:{...headers,'X-Context-ID':crypto.randomUUID()},
        ...(body === undefined ? {} : {body:JSON.stringify(body)})});
      if (!r.ok) throw new Error('FNZ: Daten nicht abrufbar (HTTP ' + r.status + '). Bitte erneut anmelden.');
      const data = await r.json();
      if (!data || typeof data !== 'object') throw new Error('FNZ: Ungültige Portalantwort.');
      if (data.resourceIsUnavailable === true) throw new Error('FNZ: Die angeforderten Daten sind derzeit nicht verfügbar.');
      return data;
    };
    const hasId = value => typeof value === 'string' ? !!value.trim() :
      value && typeof value === 'object' && !Array.isArray(value) && Object.values(value).some(hasId);
    const stableId = (value, nested = false) => {
      if (!nested && !hasId(value)) throw new Error('FNZ: Umsatz ohne Kennung.');
      if (value === null) return 'null';
      if (typeof value === 'string') {
        return JSON.stringify(value);
      }
      if (typeof value !== 'object' || Array.isArray(value) || !Object.keys(value).length)
        throw new Error('FNZ: Ungültige Umsatzkennung.');
      return '{' + Object.keys(value).sort().map(k => JSON.stringify(k) + ':' +
        stableId(value[k], true)).join(',') + '}';
    };
    const pages = async (path, filter, arrayKey, totalKey, idKey) => {
      const all = [], seen = new Set();
      let total = null;
      for (let page = 0; page < 1000; page++) {
        const data = await get(path, {...filter,pagination:{page:String(page),pageSize:'100'}});
        const rows = data[arrayKey], count = data[totalKey];
        if (!Array.isArray(rows) || !Number.isSafeInteger(count) || count < 0 || count > 100000
          || (total !== null && total !== count)) throw new Error('FNZ: Unvollständige Umsatzliste. Bitte erneut abrufen.');
        total = count;
        for (const row of rows) {
          const id = stableId(row?.[idKey]);
          if (seen.has(id)) throw new Error('FNZ: Wiederholte Umsatzseite.');
          seen.add(id);
          all.push(row);
        }
        if (all.length === total) return {rows:all,metadata:data};
        if (!rows.length || all.length > total) throw new Error('FNZ: Unvollständige Umsatzliste.');
      }
      throw new Error('FNZ: Zu viele Umsatzseiten.');
    };
    const overview = await get('depot/getDepotOverviewDataForEndCustomer');
    if (!Array.isArray(overview.ownerGroups)) throw new Error('FNZ: Keine Kontoübersicht erkannt.');
    const now = new Date();
    const dateTo = new Date(now.getTime() - now.getTimezoneOffset() * 60000).toISOString().slice(0,10);
    const filter = {dateFrom:'2000-01-01',dateTo,showIngoingTransactions:true,showOutgoingTransactions:true,sortOrder:'BOOKING_DATE_DESC'};
    const details = [], refs = new Set();
    let totalsOnly = 0;
    for (const group of overview.ownerGroups) {
      for (const kind of ['investmentDepots','managedDepots','stockDepots','preciousMetalDepots','cashAccounts']) {
        if (!Array.isArray(group[kind])) throw new Error('FNZ: Unvollständige Kontoübersicht.');
        for (const row of group[kind]) {
          const account = row.depotOrCashAccount;
          if (account?.closedFlag === true) continue;
          const ref = account?.externalDepotNumber?.externalDepotNumber;
          const depotId = account?.depotOrCashAccountId?.depotId;
          if (typeof ref !== 'string' || !ref || typeof depotId?.id !== 'string' || !depotId.id)
            throw new Error('FNZ: Konto ohne eindeutige Kennung.');
          if (refs.has(ref)) continue;
          refs.add(ref);
          if (kind === 'cashAccounts') {
            const data = await pages('cash-account-transaction/getCashAccountTransactionOverviewForEndCustomer',
              {...filter,cashAccountId:ref,referenceFilter:''},'cashAccountTransactions','totalCashAccountTransactions','orderOrTransactionId');
            details.push({accountRef:ref,kind:'cash',cashAccountId:data.metadata.cashAccountId,iban:data.metadata.iban,
              transactions:data.rows});
          } else if (kind === 'investmentDepots') {
            const inventory = await get('depot-inventory/getEndCustomerDepotInventory/depotId/' + encodeURIComponent(depotId.id));
            const data = await pages('order-transaction/getOrderTransactionOverviewForEndCustomer',
              {...filter,depotId,showTransactionsOnDepotLevel:true,subDepotPos:null,showTaxTransactions:true,
               showOtherTransactions:true,includeCanceledTransactions:true},'orderTransactions','totalOrderTransactions','orderTransactionId');
            const historical = await pages('order-transaction/getMigratedAabTransactionOverview',{depotId},
              'migratedAabTransactions','totalTransactions','migratedAabTransactionId');
            details.push({accountRef:ref,kind:'investment',inventory,transactions:data.rows,historicalTransactions:historical.rows});
          } else {
            // Keep existing support for other FNZ depot families: their detail
            // endpoints have different schemas and are not yet live validated.
            details.push({accountRef:ref,kind:'totalsOnly'});
            totalsOnly++;
          }
        }
      }
    }
    return JSON.stringify({version:1,overview,details,totalsOnly});
    """#

    static let documents = #"""
    const base = location.origin + location.pathname;
    const dateTo = new Date().toISOString().slice(0, 19);
    const all = [];
    for (const archived of [false, true]) {
      let from = 0;
      const pageIds = new Set();
      for (;;) {
        const body = {postboxCategoryAndStateFilter:{dateFrom:'2000-01-01T00:00:00', dateTo,
          category:null, onlyUnread:false, onlyFavoured:false, archived}, postboxDepotFilter:null,
          indexFrom:String(from), indexTo:String(from + 100)};
        const r = await fetch(base + 'eox/rest/postbox/getCustomerPostboxForCustomer', {
          method:'POST', credentials:'include', headers:{'content-type':'application/json',
            'X-EOX-Session':'', 'X-EOX-CLIENT-VERSION':'26.4.01', 'X-Context-ID':crypto.randomUUID()},
          body:JSON.stringify(body)});
        if (!r.ok) throw new Error('FNZ: Postkorb nicht abrufbar (HTTP ' + r.status + ').');
        const j = await r.json();
        if (!Array.isArray(j.documents)) throw new Error('FNZ: Ungültige Dokumentenliste. Bitte erneut anmelden.');
        const docs = j.documents;
        if (docs.some(d => !d || typeof d.id !== 'string' || !d.id)) throw new Error('FNZ: Dokument ohne ID.');
        if (docs.length && docs.every(d => pageIds.has(d.id))) throw new Error('FNZ: Wiederholte Postkorbseite.');
        for (const d of docs) pageIds.add(d.id);
        all.push(...docs);
        from += 100;
        if (docs.length < 100 || (Number.isInteger(j.totalMessages) && j.totalMessages >= 0 && from >= j.totalMessages)) break;
        if (from >= 100000) throw new Error('FNZ: Zu viele Postkorbseiten.');
      }
    }
    const seen = new Set();
    return JSON.stringify(all.filter(d => !seen.has(d.id) && seen.add(d.id)));
    """#

    /// `id` is passed as an argument, never interpolated into JavaScript.
    static let pdf = #"""
    const prefix = location.pathname.split('EOY')[0];
    const r = await fetch(location.origin + prefix + 'p/postkorb.aspx?docId=' + encodeURIComponent(id)
      + '&download=true', {credentials:'include'});
    const ct = (r.headers.get('content-type') || '').split(';')[0].trim().toLowerCase();
    if (!r.ok || ct !== 'application/pdf') throw new Error('FNZ: PDF nicht abrufbar. Bitte Sitzung prüfen.');
    const buf = new Uint8Array(await r.arrayBuffer());
    if (String.fromCharCode(...buf.subarray(0,5)) !== '%PDF-') throw new Error('FNZ: Ungültiger PDF-Download.');
    let bin = '';
    for (let i=0; i<buf.length; i+=0x8000) bin += String.fromCharCode.apply(null, buf.subarray(i,i+0x8000));
    return btoa(bin);
    """#
}

struct FnzWebSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let connection: BankConnection
    @State private var browser = PortalBrowser(url: URL(string: "https://portal.fnz.de/")!,
                                                domains: PortalDomains(kind: .fnz), captureTraffic: false)
    @State private var uiDelegate = AmazonUIDelegate()
    @State private var ready = false
    @State private var running = false
    @State private var includeDocuments = true
    @State private var progress = ""
    @State private var notice: String?
    @State private var task: Task<Void, Never>?
    /// Accounts another login already delivers: asked before the documents are downloaded (#346).
    @State private var choosing = false
    @State private var askedInFlow = false

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                VStack(alignment: .leading, spacing: 3) {
                    Text("\(connection.displayName) – FNZ Bank").font(.headline)
                    if running { Text(progress).font(.caption) }
                    else if let notice { Text(notice).font(.caption).foregroundStyle(.orange) }
                    else if let blocked = browser.blockedHost {
                        Text("Unsichere Seite blockiert (\(blocked)) – nur HTTPS-Seiten werden geöffnet.").font(.caption).foregroundStyle(.orange)
                    }
                    else if ready { Text("Angemeldet – Konten, Fondspositionen, Umsätze und Postkorb können abgerufen werden.").font(.caption) }
                    else { Text("Melden Sie sich bei FNZ an und bestätigen Sie in FNZsecure.").font(.caption) }
                }
                Spacer()
                if running || browser.loading { ProgressView().controlSize(.small) }
            }.padding(14)
            Divider()
            PortalWebView(browser: browser).macFrame(minWidth: 760, minHeight: 620)
                .overlay {
                    if choosing {
                        AccountSelectionPanel(connection: connection) { choosing = false }
                            .background(.background)
                    }
                }
            Divider()
            HStack {
                Button("Schließen", role: .cancel) { task?.cancel(); close() }
                    .keyboardShortcut(.cancelAction)
                Toggle("PDF-Postkorb inkl. Archiv", isOn: $includeDocuments).disabled(running)
                Spacer()
                Button("Konten und Depot abrufen") { start() }
                    .buttonStyle(.borderedProminent).disabled(!ready || running)
            }.padding(12)
        }
        .webPageDialogs(uiDelegate)
        .task {
            browser.webView.uiDelegate = uiDelegate
            while !Task.isCancelled {
                let ok = (try? await browser.webView.evaluateJavaScript(FnzScripts.probe)) as? Bool ?? false
                if ready != ok { ready = ok }
                try? await Task.sleep(nanoseconds: 1_000_000_000)
            }
        }
        .onDisappear { task?.cancel(); browser.teardown() }
        .interactiveDismissDisabled(running)
    }

    private func close() {
        model.webLoginConnection = nil
        dismiss()
    }

    private func run(_ script: String, arguments: [String: Any] = [:]) async throws -> String {
        try Task.checkCancellation()
        let guardScript = #"""
        if (location.hostname !== 'portal.fnz.de' || !/\/EOY\/EL\//.test(location.pathname))
          throw new Error('FNZ: Bitte erneut anmelden.');
        """#
        let result = try await browser.webView.callAsyncJavaScript(guardScript + "\n" + script,
            arguments: arguments, in: nil, contentWorld: .page)
        try Task.checkCancellation()
        guard let result = result as? String else { throw FnzError.unreadable }
        return result
    }

    private func start() {
        running = true
        notice = nil
        task = Task {
            // Pinned: a lock or demo switch mid-run must not send later steps to another vault.
            guard let pin = model.pinVault() else { running = false; return }
            let connectionId = connection.id
            do {
                progress = String(localized: "Konten, Fondspositionen und Umsatzhistorie abrufen …")
                let snapshot = try await run(FnzScripts.snapshot)
                let summary = try await pin.run { try $0.importFnzSnapshot(connectionId: connectionId, snapshotJson: snapshot) }
                let totalsOnly = snapshot.data(using: .utf8).flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] }?["totalsOnly"] as? Int ?? 0
                var downloaded = 0
                var failed = 0
                // Held-back accounts: ask now, so their documents are fetched (or skipped) accordingly.
                let open = try await pin.run { try $0.pendingAccountSelectionConnections() }
                if open.contains(connectionId) {
                    askedInFlow = true
                    progress = String(localized: "Auswahl der Konten …")
                    choosing = true
                    while choosing { try await Task.sleep(nanoseconds: 200_000_000) }
                }
                if includeDocuments {
                    progress = String(localized: "Postkorb und Archiv auflisten …")
                    let list = try await run(FnzScripts.documents)
                    let pending = try await pin.run { try $0.pendingFnzDocuments(connectionId: connectionId, documentsJson: list) }
                    var consecutiveFailures = 0
                    for (i, metadata) in pending.enumerated() {
                        try Task.checkCancellation()
                        progress = String(localized: "PDF \(i + 1) von \(pending.count) …")
                        do {
                            guard let data = metadata.data(using: .utf8),
                                  let doc = try JSONSerialization.jsonObject(with: data) as? [String: Any],
                                  let id = doc["id"] as? String else { throw FnzError.unreadable }
                            let b64 = try await run(FnzScripts.pdf, arguments: ["id": id])
                            guard let bytes = Data(base64Encoded: b64) else { throw FnzError.unreadable }
                            if try await pin.run({ try $0.importFnzDocument(connectionId: connectionId, documentJson: metadata, bytes: bytes) }) {
                                downloaded += 1
                            }
                            consecutiveFailures = 0
                        } catch is CancellationError { throw CancellationError() }
                        catch {
                            failed += 1
                            consecutiveFailures += 1
                            // A lost session must not cause hundreds of failed requests.
                            if consecutiveFailures >= 3 {
                                failed += pending.count - i - 1
                                break
                            }
                        }
                    }
                }
                let message = String(localized: "FNZ: \(summary.accountsUpdated) Konten/Depots aktualisiert, \(summary.transactionsImported) neue Buchungen, \(downloaded) neue PDFs.")
                model.toast = message
                if failed == 0 && totalsOnly == 0 { close() }
                else {
                    notice = message
                    if failed > 0 { notice = (notice ?? "") + " " + String(localized: "\(failed) PDFs nicht abrufbar. Erfolgreiche Downloads bleiben gespeichert; ein erneuter Abruf lädt nur fehlende Dokumente.") }
                    if totalsOnly > 0 { notice = (notice ?? "") + " " + String(localized: "Für \(totalsOnly) Depots sind derzeit nur Gesamtwerte und PDFs verfügbar.") }
                }
            } catch is CancellationError {
            } catch {
                notice = describe(error)
            }
            // PDFs are committed individually; flush them on success, cancellation and
            // partial failure before a vault file copy – in the vault they were written to.
            await pin.checkpoint()
            running = false
            choosing = false
            model.documentsVersion += 1
            model.reloadAll()
            // Skipped ("Später") accounts were already offered in this run.
            if !askedInFlow { model.checkPendingAccountSelection() }
            askedInFlow = false
        }
    }
}

private enum FnzError: LocalizedError {
    case unreadable
    var errorDescription: String? { String(localized: "FNZ: Die Portalantwort konnte nicht gelesen werden.") }
}
