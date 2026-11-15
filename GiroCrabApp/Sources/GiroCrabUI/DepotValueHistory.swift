import SwiftUI
import GiroCrabCore

/// A stretch of the depot value line with the same coverage: a new stretch starts where another depot
/// joins (or a gap in coverage ends), so the line breaks there and the jump is not drawn as growth.
struct DepotValueSegment: Equatable {
    var points: [DepotValuePoint]
    /// Not every depot had a value: the line is dashed and the sum is a partial one.
    var isPartial: Bool { points.first.map { $0.coveredDepots < $0.totalDepots } ?? false }
}

/// The value history of securities depots for the chart (`Vault.depotValueHistory`).
/// Pure: the period filter, the segments and the coverage note live here, so they are tested without a view.
enum DepotValueHistory {
    /// How far back the view asks the core; the period then cuts what it shows.
    static let lookbackDays: UInt32 = 3650

    /// The points inside `from...to` (ISO dates, nil = open), oldest first.
    /// The core sends `YYYY-MM-DD`, so the comparison is a plain string comparison.
    static func points(_ all: [DepotValuePoint], from: String?, to: String?) -> [DepotValuePoint] {
        all.filter { point in
            (from.map { point.date >= $0 } ?? true) && (to.map { point.date <= $0 } ?? true)
        }
    }

    /// Splits the points wherever the number of covered depots changes.
    static func segments(_ points: [DepotValuePoint]) -> [DepotValueSegment] {
        var result: [DepotValueSegment] = []
        for point in points {
            if var last = result.last, last.points.last?.coveredDepots == point.coveredDepots,
               last.points.last?.totalDepots == point.totalDepots {
                last.points.append(point)
                result[result.count - 1] = last
            } else {
                result.append(DepotValueSegment(points: [point]))
            }
        }
        return result
    }

    /// Value series of other assets (Sachwerte) as chart points: full coverage, the leading days on which
    /// nothing was held yet (value 0) cut away, so the axis fits what is known.
    static func assetPoints(_ balances: [DailyBalance]) -> [DepotValuePoint] {
        Array(balances.drop(while: { $0.balanceCents == 0 }))
            .map { DepotValuePoint(date: $0.date, valueCents: $0.balanceCents, coveredDepots: 1, totalDepots: 1) }
    }

    /// What is drawn: equal values in a row collapse into the first of them (lossless for a step line),
    /// and a series still longer than `maxPoints` keeps the last point of each stretch (the first and
    /// the last point always stay).
    static func drawn(_ points: [DepotValuePoint], maxPoints: Int = 900) -> [DepotValuePoint] {
        var collapsed: [DepotValuePoint] = []
        for point in points {
            if let last = collapsed.last, last.valueCents == point.valueCents,
               last.coveredDepots == point.coveredDepots, last.totalDepots == point.totalDepots { continue }
            collapsed.append(point)
        }
        guard collapsed.count > maxPoints, maxPoints > 2 else { return collapsed }
        let step = Int((Double(collapsed.count) / Double(maxPoints)).rounded(.up))
        var thinned = [collapsed[0]]
        var i = step
        while i < collapsed.count - 1 { thinned.append(collapsed[i]); i += step }
        thinned.append(collapsed[collapsed.count - 1])
        return thinned
    }

    /// The one note under every depot value chart (depot view and Vermögen).
    static let footnote: LocalizedStringKey = "Wertänderung, keine Rendite: Käufe, Verkäufe und Übertragungen verändern den Wert."

    /// The value that holds on `day` (ISO): the last point up to it (the points are oldest first).
    static func value(on day: String, in points: [DepotValuePoint]) -> DepotValuePoint? {
        points.last { $0.date <= day }
    }

    struct Change: Equatable {
        let cents: Int64
        /// nil when the start value is not positive.
        let percent: Double?
    }

    /// The change since the first point of the period. Only between days with the same coverage: when
    /// another depot has joined, the difference is that depot's value, not a gain.
    static func change(of point: DepotValuePoint, since first: DepotValuePoint) -> Change? {
        guard point.coveredDepots == first.coveredDepots, point.totalDepots == first.totalDepots else { return nil }
        let cents = point.valueCents - first.valueCents
        return Change(cents: cents, percent: first.valueCents > 0 ? Double(cents) / Double(first.valueCents) * 100 : nil)
    }

    /// "Werte ab 19.07.2026" when the first value is clearly later than the start of the shown period
    /// (the left of the chart is empty then: before that day the value is unknown).
    static func startNote(_ points: [DepotValuePoint], from: String?) -> String? {
        guard let from, let first = points.first,
              let periodStart = DateText.parseISODay(from), let firstDay = DateText.parseISODay(first.date),
              firstDay.timeIntervalSince(periodStart) > 3 * 86_400 else { return nil }
        return String(localized: "Werte ab \(DateText.medium(first.date))")
    }

    /// "Teilsumme · 2 von 3 Depots" when some day of the shown period is not fully covered
    /// (the lowest coverage in it), else nil.
    static func coverageNote(_ points: [DepotValuePoint]) -> String? {
        let partial = points.filter { $0.coveredDepots < $0.totalDepots }
        guard let worst = partial.min(by: { $0.coveredDepots < $1.coveredDepots }) else { return nil }
        return String(localized: "Teilsumme · \(Int(worst.coveredDepots)) von \(Int(worst.totalDepots)) Depots")
    }
}
