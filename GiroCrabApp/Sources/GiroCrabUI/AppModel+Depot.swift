import Foundation
import GiroCrabCore

extension AppModel {
    /// THE decision whether an account is a securities depot, a Sachwerte account or neither. Routing,
    /// toolbar, shortcuts and the wealth view all ask here (no name heuristics): both depot kinds are
    /// `brokerage` accounts, Sachwerte are the ones the asset store knows.
    func depotKind(of accountId: Int64) -> DepotKind? {
        guard let a = account(for: accountId), a.kind == .brokerage else { return nil }
        return isTangible(accountId) ? .tangible : .securities
    }

    /// How the sidebar selection is presented: bookings, a securities depot or a Sachwerte account.
    var presentation: AccountPresentation {
        guard case .account(let id)? = sidebarSelection else { return .bookings }
        switch depotKind(of: id) {
        case .securities: return .securities(id)
        case .tangible: return .tangible(id)
        case nil: return .bookings
        }
    }

    /// The selected securities depot (never a Sachwerte account).
    var selectedSecuritiesDepot: Account? {
        guard case .securities(let id) = presentation else { return nil }
        return account(for: id)
    }

    /// The selected Sachwerte account.
    var selectedTangibleAccount: Account? {
        guard case .tangible(let id) = presentation else { return nil }
        return account(for: id)
    }

    /// The depot view has no salary cycle: "since last salary" falls back to the whole period there
    /// instead of silently using a calendar month.
    var depotPeriodRange: (String?, String?) { period == .sinceSalary ? (nil, nil) : periodRange }
    var depotPeriodTitle: String { period == .sinceSalary ? Period.all.title : periodTitle }

    /// The selection's list, chart and sums are those of the current selection (no load pending).
    var selectionSettled: Bool {
        _ = selectionLoadStamp // observed: the task is not, and a fast load never changes `isLoadingSelection`
        return selectionLoadTask == nil && !isLoadingSelection
    }

    /// ⌘1…⌘3 / palette: the booking scopes, the depot tabs while a securities depot is shown, and
    /// nothing for Sachwerte (the hidden booking scope must not change unnoticed).
    func applyScopeShortcut(index: Int, scope newScope: BookingScope) {
        switch presentation {
        case .securities: if let tab = DepotTab.allCases[safe: index] { depotTab = tab }
        case .tangible: break
        case .bookings: scope = newScope
        }
    }
}

private extension Array {
    subscript(safe i: Int) -> Element? { indices.contains(i) ? self[i] : nil }
}
