import XCTest
import GiroCrabCore
@testable import GiroCrabUI

/// A selection made in the UI loads in the background; everything else stays synchronous.
@MainActor
final class SelectionLoadTests: XCTestCase {
    private var model: AppModel!

    override func setUp() async throws {
        model = AppModel(options: LaunchOptions(arguments: ["--demo-seed"], environment: [:]))
        // placeholder at once, so the loading state can be checked deterministically
        model.selectionPlaceholderDelay = .zero
        XCTAssertEqual(model.vaultState, .open)
        XCTAssertNil(model.errorMessage)
    }

    override func tearDown() async throws { model = nil }

    private func expectedIDs() throws -> [Int64] {
        try XCTUnwrap(model.vault).listTransactions(filter: model.currentFilter).transactions.map(\.id)
    }

    private func awaitBookings(_ expected: [Int64]) async throws {
        for _ in 0..<300 {
            if model.bookings.map(\.id) == expected { return }
            try await Task.sleep(for: .milliseconds(10))
        }
        XCTFail("The background selection load did not publish its bookings")
    }

    /// The state a synchronous change to the same selection produces.
    private func assertMatchesSynchronousLoad(file: StaticString = #filePath, line: UInt = #line) throws {
        let bookings = model.bookings.map(\.id), flow = model.selectionFlow, history = model.history.map(\.balanceCents)
        let recurring = model.recurring.map(\.baseKey), categories = model.categories.map(\.totalCents)
        let selection = model.sidebarSelection, period = model.period
        model.sidebarSelection = selection
        model.period = period
        XCTAssertEqual(model.bookings.map(\.id), bookings, file: file, line: line)
        XCTAssertEqual(model.selectionFlow?.consumption, flow?.consumption, file: file, line: line)
        XCTAssertEqual(model.selectionFlow?.invested, flow?.invested, file: file, line: line)
        XCTAssertEqual(model.selectionFlow?.income, flow?.income, file: file, line: line)
        XCTAssertEqual(model.history.map(\.balanceCents), history, file: file, line: line)
        XCTAssertEqual(model.recurring.map(\.baseKey), recurring, file: file, line: line)
        XCTAssertEqual(model.categories.map(\.totalCents), categories, file: file, line: line)
    }

    /// A load that ends before the placeholder shows never changes `isLoadingSelection`; a view that
    /// asked `selectionSettled` (the Sachwerte chart) must still be told that the load is over.
    func testSettledStateIsObservableWhenAFastLoadEndsWithoutAPlaceholder() async throws {
        model.selectionPlaceholderDelay = .seconds(60)
        let account = try XCTUnwrap(model.allAccounts.first { $0.kind == .checking })
        model.interactiveSidebarSelection = .account(account.id)
        XCTAssertFalse(model.selectionSettled, "the load is running")
        XCTAssertFalse(model.isLoadingSelection, "and no placeholder shows")
        let changed = expectation(description: "the settled state notifies its observers")
        withObservationTracking { _ = model.selectionSettled } onChange: { changed.fulfill() }
        await fulfillment(of: [changed], timeout: 5)
        XCTAssertTrue(model.selectionSettled)
    }

    func testInteractiveAccountSwitchLoadsInBackground() async throws {
        let account = try XCTUnwrap(model.allAccounts.first { $0.kind == .checking })
        XCTAssertFalse(model.bookings.isEmpty)

        model.interactiveSidebarSelection = .account(account.id)
        XCTAssertEqual(model.sidebarSelection, .account(account.id))
        XCTAssertTrue(model.isLoadingSelection)
        XCTAssertTrue(model.bookings.isEmpty, "nothing of the previous selection shows while loading")
        XCTAssertTrue(model.dayGroups.isEmpty)
        XCTAssertTrue(model.history.isEmpty)
        XCTAssertFalse(model.hasMoreBookings)

        let expected = try expectedIDs()
        XCTAssertFalse(expected.isEmpty)
        try await awaitBookings(expected)
        XCTAssertFalse(model.isLoadingSelection)
        XCTAssertTrue(model.bookings.allSatisfy { $0.accountId == account.id })
        XCTAssertNil(model.errorMessage)
        try assertMatchesSynchronousLoad()
    }

    /// Default grace period: a fast load swaps old for new data in one step,
    /// without the empty list / chart / "–" tiles in between (no flicker).
    func testFastLoadKeepsPreviousDataUntilItPublishes() async throws {
        model.selectionPlaceholderDelay = .seconds(30)
        let before = model.bookings.map(\.id)
        let history = model.history.map(\.balanceCents)
        XCTAssertFalse(before.isEmpty)
        let account = try XCTUnwrap(model.allAccounts.first { $0.kind == .checking })

        model.interactiveSidebarSelection = .account(account.id)
        XCTAssertFalse(model.isLoadingSelection)
        XCTAssertEqual(model.bookings.map(\.id), before)
        XCTAssertEqual(model.history.map(\.balanceCents), history)
        XCTAssertFalse(model.hasMoreBookings, "no paging into the previous selection while loading")

        let expected = try expectedIDs()
        var sawEmpty = false
        for _ in 0..<300 where model.bookings.map(\.id) != expected {
            sawEmpty = sawEmpty || model.bookings.isEmpty || model.isLoadingSelection
            try await Task.sleep(for: .milliseconds(5))
        }
        XCTAssertEqual(model.bookings.map(\.id), expected)
        XCTAssertFalse(sawEmpty)
        XCTAssertFalse(model.isLoadingSelection)
        try assertMatchesSynchronousLoad()
    }

    func testInteractivePeriodChangeLoadsInBackground() async throws {
        model.interactivePeriod = .sinceSalary
        try await Task.sleep(for: .milliseconds(300))
        XCTAssertEqual(model.bookings.map(\.id), try expectedIDs())
        XCTAssertNil(model.errorMessage)
        try assertMatchesSynchronousLoad()
    }

    func testRapidSwitchesKeepTheLatestSelection() async throws {
        let accounts = model.allAccounts.prefix(3)
        for a in accounts {
            model.interactiveSidebarSelection = .account(a.id)
            await Task.yield()
        }
        let expected = try expectedIDs()
        try await awaitBookings(expected)
        try await Task.sleep(for: .milliseconds(150))
        XCTAssertEqual(model.bookings.map(\.id), expected)
        XCTAssertEqual(model.page?.transactions.map(\.id), expected)
    }

    func testSynchronousReloadSupersedesPendingLoad() async throws {
        let account = try XCTUnwrap(model.allAccounts.first { $0.kind == .checking })
        model.interactiveSidebarSelection = .account(account.id)
        // e.g. an edit right after the click: the data must be current on return
        model.reloadBookings()
        let expected = try expectedIDs()
        XCTAssertEqual(model.bookings.map(\.id), expected)
        XCTAssertFalse(model.isLoadingSelection)
        try await Task.sleep(for: .milliseconds(150))
        XCTAssertEqual(model.bookings.map(\.id), expected)
        try assertMatchesSynchronousLoad()
    }

    func testScopeChangeDuringPendingLoadIsPickedUp() async throws {
        let account = try XCTUnwrap(model.allAccounts.first { $0.kind == .checking })
        model.interactiveSidebarSelection = .account(account.id)
        model.scope = .everyday
        let expected = try expectedIDs()
        try await awaitBookings(expected)
        try await Task.sleep(for: .milliseconds(150))
        XCTAssertEqual(model.bookings.map(\.id), expected)
    }
}
