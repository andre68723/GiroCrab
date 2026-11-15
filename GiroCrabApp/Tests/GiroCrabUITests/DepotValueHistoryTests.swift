import Testing
@testable import GiroCrabUI
import GiroCrabCore

@Suite struct DepotValueHistoryTests {
    private static func point(_ date: String, _ cents: Int64, covered: UInt32 = 1, total: UInt32 = 1) -> DepotValuePoint {
        DepotValuePoint(date: date, valueCents: cents, coveredDepots: covered, totalDepots: total)
    }

    private static let all = [point("2026-03-01", 100), point("2026-03-05", 120), point("2026-03-09", 90), point("2026-03-12", 130)]

    @Test func anOpenPeriodKeepsEveryPointInOrder() {
        let shown = DepotValueHistory.points(Self.all, from: nil, to: nil)
        #expect(shown.map(\.date) == ["2026-03-01", "2026-03-05", "2026-03-09", "2026-03-12"])
        #expect(shown.map(\.valueCents) == [100, 120, 90, 130])
    }

    @Test func thePeriodCutsAtBothEndsInclusively() {
        let shown = DepotValueHistory.points(Self.all, from: "2026-03-05", to: "2026-03-09")
        #expect(shown.map(\.date) == ["2026-03-05", "2026-03-09"])
    }

    @Test func aPeriodBeforeTheFirstValueShowsNothingInsteadOfInventingOne() {
        #expect(DepotValueHistory.points(Self.all, from: "2026-02-01", to: "2026-02-28").isEmpty)
        #expect(DepotValueHistory.points([], from: nil, to: nil).isEmpty)
    }

    @Test func aHalfOpenPeriodWorks() {
        #expect(DepotValueHistory.points(Self.all, from: "2026-03-09", to: nil).count == 2)
        #expect(DepotValueHistory.points(Self.all, from: nil, to: "2026-03-01").count == 1)
    }

    @Test func theLineBreaksWhereAnotherDepotJoinsAndOnlyTheEarlierStretchIsPartial() {
        let points = [
            Self.point("2026-03-01", 100, covered: 1, total: 2), Self.point("2026-03-02", 110, covered: 1, total: 2),
            Self.point("2026-03-03", 250, covered: 2, total: 2), Self.point("2026-03-04", 255, covered: 2, total: 2),
        ]
        let segments = DepotValueHistory.segments(points)
        #expect(segments.count == 2)
        #expect(segments.map(\.isPartial) == [true, false])
        #expect(segments[0].points.map(\.date) == ["2026-03-01", "2026-03-02"])
        #expect(segments[1].points.map(\.date) == ["2026-03-03", "2026-03-04"])
    }

    @Test func aFullyCoveredLineIsOneSolidSegmentWithoutNote() {
        let segments = DepotValueHistory.segments(Self.all)
        #expect(segments.count == 1)
        #expect(segments[0].isPartial == false)
        #expect(DepotValueHistory.coverageNote(Self.all) == nil)
    }

    @Test func theNoteNamesTheLowestCoverageOfThePeriod() {
        let points = [
            Self.point("2026-03-01", 10, covered: 1, total: 3), Self.point("2026-03-02", 20, covered: 2, total: 3),
            Self.point("2026-03-03", 30, covered: 3, total: 3),
        ]
        let note = DepotValueHistory.coverageNote(points)
        #expect(note?.contains("1") == true && note?.contains("3") == true)
        // the partial days cut away: no note
        #expect(DepotValueHistory.coverageNote(DepotValueHistory.points(points, from: "2026-03-03", to: nil)) == nil)
    }

    @Test func theValueOnADayIsTheLastPointUpToIt() {
        #expect(DepotValueHistory.value(on: "2026-03-07", in: Self.all)?.valueCents == 120)
        #expect(DepotValueHistory.value(on: "2026-03-09", in: Self.all)?.valueCents == 90)
        #expect(DepotValueHistory.value(on: "2026-03-30", in: Self.all)?.valueCents == 130)
        #expect(DepotValueHistory.value(on: "2026-02-28", in: Self.all) == nil, "nothing before the first point")
    }

    @Test func theChangeIsSinceTheFirstPointAndOnlyWithTheSameCoverage() {
        let first = Self.point("2026-03-01", 10_000, covered: 1, total: 2)
        let same = Self.point("2026-03-02", 10_500, covered: 1, total: 2)
        #expect(DepotValueHistory.change(of: same, since: first) == .init(cents: 500, percent: 5))
        let joined = Self.point("2026-03-03", 25_000, covered: 2, total: 2)
        #expect(DepotValueHistory.change(of: joined, since: first) == nil, "a depot joining is no gain")
        let zero = Self.point("2026-03-01", 0, covered: 1, total: 1)
        #expect(DepotValueHistory.change(of: Self.point("2026-03-02", 5, covered: 1, total: 1), since: zero)?.percent == nil)
    }

    @Test func theStartNoteAppearsOnlyWhenTheFirstValueIsClearlyAfterThePeriodStart() {
        #expect(DepotValueHistory.startNote(Self.all, from: nil) == nil)
        #expect(DepotValueHistory.startNote(Self.all, from: "2026-03-01") == nil)
        #expect(DepotValueHistory.startNote(Self.all, from: "2026-02-27") == nil, "a gap of a few days is no news")
        let note = DepotValueHistory.startNote(Self.all, from: "2025-12-01")
        #expect(note != nil)
        #expect(DepotValueHistory.startNote([], from: "2025-12-01") == nil)
    }

    @Test func theDaysBeforeTheFirstHoldingAreCutAwayForOtherAssets() {
        let balances = [("2026-01-01", 0), ("2026-01-02", 0), ("2026-01-03", 500), ("2026-01-04", 0), ("2026-01-05", 700)]
            .map { DailyBalance(date: $0.0, balanceCents: Int64($0.1)) }
        let points = DepotValueHistory.assetPoints(balances)
        #expect(points.map(\.date) == ["2026-01-03", "2026-01-04", "2026-01-05"], "a later 0 (everything sold) stays")
        #expect(points.allSatisfy { $0.coveredDepots == 1 && $0.totalDepots == 1 })
        #expect(DepotValueHistory.assetPoints([]).isEmpty)
        #expect(DepotValueHistory.assetPoints(balances.prefix(2).map { $0 }).isEmpty, "never held anything")
    }

    @Test func equalValuesInARowCollapseAndTheEndsStay() {
        let points = [Self.point("2026-03-01", 10), Self.point("2026-03-02", 10), Self.point("2026-03-03", 10),
                      Self.point("2026-03-04", 20), Self.point("2026-03-05", 20), Self.point("2026-03-06", 10)]
        let drawn = DepotValueHistory.drawn(points)
        #expect(drawn.map(\.date) == ["2026-03-01", "2026-03-04", "2026-03-06"])
        // another coverage with the same value is a different stretch
        let joined = [Self.point("2026-03-01", 10, covered: 1, total: 2), Self.point("2026-03-02", 10, covered: 2, total: 2)]
        #expect(DepotValueHistory.drawn(joined).count == 2)
    }

    @Test func aLongSeriesIsThinnedAndKeepsItsFirstAndLastPoint() {
        let long = (0..<3_000).map { Self.point("d\($0)", Int64($0 + 1)) }
        let drawn = DepotValueHistory.drawn(long, maxPoints: 500)
        #expect(drawn.count <= 520)
        #expect(drawn.first?.date == "d0")
        #expect(drawn.last?.date == "d2999")
        #expect(drawn.map(\.date) == drawn.map(\.date).sorted { Int($0.dropFirst())! < Int($1.dropFirst())! })
    }
}
