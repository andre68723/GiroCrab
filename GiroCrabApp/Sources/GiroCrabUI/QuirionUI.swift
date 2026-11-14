import SwiftUI
import WebKit
import GiroCrabCore

/// Authorization stays in the page's JavaScript closure, never in Swift, logs
/// or the vault. Installed before the portal's first authenticated request.
enum QuirionScripts {
    static let hook = #"""
    (() => {
      if (location.origin !== 'https://banking.quirion.de' || window.__gcQuirion) return;
      const origin = 'https://api.it-aws-prod.quirion.de';
      const prefix = '/legacy/v1/';
      const nativeFetch = window.fetch;
      let authorization = null;
      const trusted = url => {
        try { const u = new URL(url, location.href);
          return u.origin === origin && u.pathname.startsWith(prefix);
        } catch { return false; }
      };
      const remember = (url, headers) => {
        if (!trusted(url)) return;
        const value = new Headers(headers || {}).get('authorization');
        if (value) authorization = value;
      };
      window.fetch = function(input, init) {
        const url = input instanceof Request ? input.url : String(input);
        const headers = init?.headers || (input instanceof Request ? input.headers : {});
        const response = nativeFetch.apply(this, arguments);
        if (trusted(url)) response.then(r => { if (r.ok) remember(url, headers); }).catch(() => {});
        return response;
      };
      const open = XMLHttpRequest.prototype.open;
      const setHeader = XMLHttpRequest.prototype.setRequestHeader;
      const requests = new WeakMap();
      XMLHttpRequest.prototype.open = function(method, url) {
        const request = {url: String(url), headers: {}};
        requests.set(this, request);
        this.addEventListener('load', () => {
          if (this.status >= 200 && this.status < 300) remember(request.url, request.headers);
        }, {once:true});
        return open.apply(this, arguments);
      };
      XMLHttpRequest.prototype.setRequestHeader = function(name, value) {
        const request = requests.get(this);
        if (request && trusted(request.url)) request.headers[name] = value;
        return setHeader.apply(this, arguments);
      };
      const request = async (path, body) => {
        if (location.origin !== 'https://banking.quirion.de' || !authorization)
          throw new Error('quirion: Bitte erneut anmelden.');
        if (!path.startsWith(prefix) || path.includes('://')) throw new Error('quirion: Ungültiger Abruf.');
        const controller = new AbortController();
        const timeout = setTimeout(() => controller.abort(), 30000);
        try {
          const r = await nativeFetch.call(window, origin + path, {
            method: body === undefined ? 'GET' : 'POST', credentials: 'omit', redirect: 'error',
            headers: {'Authorization':authorization, 'Accept':'application/json', 'Content-Type':'application/json'},
            ...(body === undefined ? {} : {body:JSON.stringify(body)}), signal:controller.signal
          });
          if (!r.ok) {
            if (r.status === 401 || r.status === 403) authorization = null;
            throw new Error('quirion: Abruf fehlgeschlagen (HTTP ' + r.status + '). Bitte erneut anmelden.');
          }
          return r;
        } finally { clearTimeout(timeout); }
      };
      const json = async (path, body) => (await request(path, body)).json();
      const identifier = n => {
        if (!Number.isSafeInteger(n) || n <= 0) throw new Error('quirion: Ungültige Konto-ID.');
        return String(n);
      };
      const fields = (object, keys) => Object.fromEntries(keys.filter(k => object?.[k] !== undefined).map(k => [k, object[k]]));
      const cashKeys = ['accountHolder','iban','sortKey','currency','exchangeRate','balance'];
      const txKeys = ['amount','currency','status','bookingDate','valutaDate','type','verwendungszweck','Verwendungszweck',
        'creditorAccountHolder','creditorIban','debtorAccountHolder','debtorIban'];
      const history = async (businessPartnerId, ipsId, start) => {
        const timestamp = Date.parse(start);
        if (!Number.isFinite(timestamp)) throw new Error('quirion: Unbekannter Beginn der Umsatzhistorie.');
        const range = Math.max(3000, Math.ceil((Date.now() - timestamp) / 86400000) + 366);
        const body = {businessPartnerId, range, status:'CLOSED', type:'ALL'};
        if (ipsId !== undefined) body.ipsId = ipsId;
        const tx = await json(prefix + 'transactions', body);
        if (!Array.isArray(tx)) throw new Error('quirion: Ungültige Umsatzhistorie.');
        return tx.map(t => fields(t, txKeys));
      };
      window.__gcQuirion = {
        ready: () => !!authorization,
        collect: async () => {
          const user = await json(prefix + 'user');
          if (!Array.isArray(user.businessPartner)) throw new Error('quirion: Keine Konten erkannt.');
          const partners = [];
          for (const bp of user.businessPartner.filter(b => b.status === 'OPEN')) {
            const bpId = identifier(bp.businessPartnerId);
            const base = prefix + 'business-partners/' + bpId;
            const b = await json(base + '?clearing=true&products=true&history=false&historyYear=false&reference=false&users=false&wphg=false');
            if (b.businessPartnerId !== bp.businessPartnerId || !Array.isArray(b.products)) throw new Error('quirion: Unvollständige Kontoübersicht.');
            const partner = {businessPartnerId:bp.businessPartnerId, name:bp.name, balanceDate:bp.balanceDate,
              clearing:b.clearing ? fields(b.clearing, cashKeys) : null, products:[]};
            if (partner.clearing) partner.clearingTransactions = await history(bp.businessPartnerId, undefined, bp.createdAt);
            for (const p of b.products.filter(p => p.status === 'OPEN')) {
              const productId = identifier(p.ipsId);
              const d = await json(base + '/products/' + productId + '?condition=false&debit=false&depotitems=true&liquid=true&history=false&historyYear=false&savingsplans=false&transactions=false&wphg=false');
              if (!d.liquid || !Array.isArray(d.depotItems)) throw new Error('quirion: Unvollständiges Depot.');
              partner.products.push({
                summary:fields(p,['ipsId','businessPartnerId','name','status','depotBalance','balance','balanceDate']),
                detail:{liquid:fields(d.liquid,cashKeys), depotItems:d.depotItems.map(i => fields(i,
                  ['isin','name','shortName','quantity','value','aktuellerKurs','einstandskurs','exchangeRate']))},
                transactions:await history(bp.businessPartnerId, p.ipsId, p.createdAt)
              });
            }
            partners.push(partner);
          }
          return JSON.stringify({version:1,partners});
        },
        documents: async () => {
          const data = await json(prefix + 'postbox?status=ALL&from=2000-01-01&to=' + new Date().toISOString().slice(0,10));
          if (!Array.isArray(data.documents)) throw new Error('quirion: Ungültige Postbox.');
          return JSON.stringify(data.documents.map(d => fields(d,['id','businessPartnerId','displayName','createdAt'])));
        },
        pdf: async id => {
          if (typeof id !== 'string' || !id) throw new Error('quirion: Ungültige Dokument-ID.');
          let encoded = await (await request(prefix + 'postbox/download', {doc:id})).text();
          if (encoded.startsWith('"')) encoded = JSON.parse(encoded);
          if (!atob(encoded).startsWith('%PDF-')) throw new Error('quirion: Der Download ist kein PDF.');
          return encoded;
        }
      };
    })();
    """#

    static let probe = #"""
    location.origin === 'https://banking.quirion.de' && !!window.__gcQuirion?.ready()
    """#
}

struct QuirionWebSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let connection: BankConnection
    @State private var browser = PortalBrowser(url: URL(string: "https://banking.quirion.de/login/")!,
        domains: PortalDomains(kind: .quirion), captureTraffic: false, userScript: QuirionScripts.hook)
    @State private var uiDelegate = AmazonUIDelegate()
    @State private var ready = false
    @State private var running = false
    @State private var includeDocuments = true
    @State private var progress = ""
    @State private var notice: String?
    @State private var task: Task<Void, Never>?

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                VStack(alignment: .leading, spacing: 3) {
                    Text("\(connection.displayName) – quirion").font(.headline)
                    if running { Text(progress).font(.caption) }
                    else if let notice { Text(notice).font(.caption).foregroundStyle(.orange) }
                    else { Text(ready ? String(localized: "Angemeldet – Depot, Konten und Historie können abgerufen werden.") : String(localized: "Melden Sie sich bei quirion an und bestätigen Sie gegebenenfalls die Anmeldung.")).font(.caption) }
                }
                Spacer()
                if running || browser.loading { ProgressView().controlSize(.small) }
            }.padding(14)
            Divider()
            PortalWebView(browser: browser).macFrame(minWidth: 760, minHeight: 620)
            Divider()
            HStack {
                Button("Schließen", role: .cancel) { task?.cancel(); close() }.keyboardShortcut(.cancelAction)
                Toggle("PDF-Postbox inkl. Archiv", isOn: $includeDocuments).disabled(running)
                Spacer()
                Button("Depot und Konten abrufen") { start() }.buttonStyle(.borderedProminent).disabled(!ready || running)
            }.padding(12)
        }
        .webPageDialogs(uiDelegate)
        .task {
            browser.webView.uiDelegate = uiDelegate
            while !Task.isCancelled {
                let authenticated = (try? await browser.webView.evaluateJavaScript(QuirionScripts.probe)) as? Bool ?? false
                let ok = browser.onPortal && !browser.onLoginPage && authenticated
                if ready != ok { ready = ok }
                try? await Task.sleep(nanoseconds: 1_000_000_000)
            }
        }
        .onDisappear { task?.cancel(); browser.teardown() }
        .interactiveDismissDisabled(running)
    }

    private func run(_ script: String, arguments: [String: Any] = [:]) async throws -> String {
        try Task.checkCancellation()
        let guardScript = "if (location.origin !== 'https://banking.quirion.de' || !window.__gcQuirion) throw new Error('quirion: Bitte erneut anmelden.');\n"
        let result = try await browser.webView.callAsyncJavaScript(guardScript + script, arguments: arguments, in: nil, contentWorld: .page)
        try Task.checkCancellation()
        guard let string = result as? String else { throw QuirionError.unreadable }
        return string
    }

    private func start() {
        running = true
        notice = nil
        task = Task {
            guard let pin = model.pinVault() else { running = false; return }
            let connectionId = connection.id
            do {
                progress = String(localized: "Depotpositionen und Umsatzhistorie abrufen …")
                let snapshot = try await run("return await window.__gcQuirion.collect();")
                let summary = try await pin.run { try $0.importQuirionSnapshot(connectionId: connectionId, snapshotJson: snapshot) }
                var downloaded = 0
                var failed = 0
                if includeDocuments {
                    progress = String(localized: "Postbox und Archiv auflisten …")
                    let list = try await run("return await window.__gcQuirion.documents();")
                    let pending = try await pin.run { try $0.pendingQuirionDocuments(connectionId: connectionId, documentsJson: list) }
                    var streak = QuirionFailureStreak()
                    for (index, metadata) in pending.enumerated() {
                        try Task.checkCancellation()
                        progress = String(localized: "PDF \(index + 1) von \(pending.count) …")
                        do {
                            guard let data = metadata.data(using: .utf8),
                                  let doc = try JSONSerialization.jsonObject(with: data) as? [String: Any],
                                  let id = doc["id"] as? String else { throw QuirionError.unreadable }
                            let encoded = try await run("return await window.__gcQuirion.pdf(id);", arguments: ["id":id])
                            guard let bytes = Data(base64Encoded: encoded) else { throw QuirionError.unreadable }
                            if try await pin.run({ try $0.importQuirionDocument(connectionId: connectionId, documentJson: metadata, bytes: bytes) }) { downloaded += 1 }
                            streak.succeeded()
                        } catch is CancellationError { throw CancellationError() }
                        catch {
                            failed += 1
                            if streak.failed() { failed += pending.count - index - 1; break }
                        }
                    }
                }
                let message = String(localized: "quirion: \(summary.accountsUpdated) Konten/Depots aktualisiert, \(summary.transactionsImported) neue Buchungen, \(downloaded) neue PDFs.")
                model.toast = message
                if failed == 0 { close() }
                else { notice = message + " " + String(localized: "\(failed) PDFs nicht abrufbar. Ein erneuter Abruf lädt nur fehlende Dokumente.") }
            } catch is CancellationError {
            } catch { notice = describe(error) }
            await pin.checkpoint()
            running = false
            model.documentsVersion += 1
            model.reloadAll()
            // accounts another login already delivers wait for a choice (#346)
            model.checkPendingAccountSelection()
        }
    }

    private func close() { model.webLoginConnection = nil; dismiss() }
}

/// Aborts the PDF loop after repeated failures in a row (expired session, portal change).
struct QuirionFailureStreak {
    static let limit = 3
    private var count = 0
    mutating func succeeded() { count = 0 }
    /// Returns true once the limit of consecutive failures is reached.
    mutating func failed() -> Bool { count += 1; return count >= Self.limit }
}

private enum QuirionError: LocalizedError {
    case unreadable
    var errorDescription: String? { String(localized: "quirion: Die Portalantwort konnte nicht gelesen werden.") }
}
