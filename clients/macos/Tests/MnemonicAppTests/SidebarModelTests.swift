import MnemonicShared
import XCTest
@testable import MnemonicApp

final class SidebarModelTests: XCTestCase {
    func testGroupsAndEntriesAreTheOwnersLayout() {
        let groups = SidebarModel.groups()
        XCTAssertEqual(groups.map(\.label), ["NOW", "MEMORY", "EXPLORE"])
        XCTAssertEqual(groups.map { group in group.entries.map(\.title) }, [
            ["Projects", "Timeline"], ["Memories", "Search", "Entities", "Memory Map"],
            ["Project Graph", "Overview", "Manage"]
        ])
    }

    func testEveryBuiltSidebarRouteIsAnEntryExactlyOnceAndNoGroupIsEmpty() {
        let groups = SidebarModel.groups()
        let entries = groups.flatMap(\.entries)
        XCTAssertTrue(groups.allSatisfy { group in !group.entries.isEmpty })
        XCTAssertEqual(Set(entries), Set(AppRoute.allCases))
        XCTAssertEqual(entries.count, AppRoute.allCases.count)
        XCTAssertEqual(Set(entries).count, entries.count)
        XCTAssertTrue(entries.allSatisfy { route in route == route.sidebarRoute })
    }

    func testDaemonHasSixDistinctShortEnglishStates() {
        let states: [LocalAPIFailure?] = [nil, .off, .unreachable, .unauthorised, .earlierDaemon, .undecodable]
        let words = states.map { failure in SidebarModel.daemonText(failure) }
        XCTAssertEqual(words, ["daemon running", "local API is off", "daemon unavailable",
                               "access unavailable", "daemon update needed", "answer unreadable"])
        XCTAssertEqual(Set(words).count, states.count)
    }
}
