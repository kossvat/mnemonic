import Foundation
import MnemonicShared
import XCTest

/// The same bytes must be emitted by the daemon's guarded curated route test.
final class CuratedGraphTests: XCTestCase {
    private func fixture() throws -> Data {
        let url = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("Fixtures/curated-graph.json")
        return try Data(contentsOf: url)
    }

    private func object() throws -> [String: Any] {
        try XCTUnwrap(JSONSerialization.jsonObject(with: fixture()) as? [String: Any])
    }

    private func decode(_ object: [String: Any]) throws -> CuratedGraphSnapshot {
        try JSONDecoder().decode(CuratedGraphSnapshot.self,
            from: JSONSerialization.data(withJSONObject: object))
    }

    private func replacing(_ group: String, field: Int, with value: Any) throws -> [String: Any] {
        var object = try object()
        var rows = try XCTUnwrap(object[group] as? [[Any]])
        rows[0][field] = value
        object[group] = rows
        return object
    }

    private func roundTrip(_ graph: CuratedGraphSnapshot) throws {
        let bytes = try JSONEncoder().encode(graph)
        XCTAssertEqual(try JSONDecoder().decode(CuratedGraphSnapshot.self, from: bytes), graph)
    }

    func testFixtureDecodesAllKindsLinksShelvesAndCoordinates() throws {
        let graph = try JSONDecoder().decode(CuratedGraphSnapshot.self, from: fixture())
        XCTAssertEqual(graph.schemaVersion, 1)
        XCTAssertEqual(graph.nodes.map(\.kind), [.project, .project, .part, .unit, .technology, .person])
        XCTAssertEqual(graph.nodes.map(\.id), ["project:1", "project:3", "project:2",
            "unit:1:LP-12", "entity:rust", "entity:mira"])
        XCTAssertEqual(graph.nodes.map(\.x), [0, 37, 75, 89, 5000, 6500])
        XCTAssertEqual(graph.nodes.map(\.y), [0, -423, -94, -81, 0, 0])
        XCTAssertEqual(graph.nodes[0].label, "Harbor")
        XCTAssertEqual(graph.nodes[0].projectSlug, "harbor")
        XCTAssertEqual(graph.nodes[2].projectSlug, "harbor:api")
        XCTAssertEqual(graph.nodes[0].state, .active)
        XCTAssertEqual(graph.nodes[1].state, .quiet)
        XCTAssertEqual(graph.nodes[3].state, .stale)
        XCTAssertTrue(graph.nodes[3].isClosed)
        XCTAssertNil(graph.nodes[3].lastActivity)
        XCTAssertNil(graph.nodes[3].inactiveDays)
        XCTAssertNil(graph.nodes[3].projectSlug)
        XCTAssertFalse(graph.nodes[0].isClosed)
        XCTAssertEqual(graph.nodes[0].lastActivity, 1_799_913_600)
        XCTAssertEqual(graph.nodes[1].inactiveDays, 10)
        XCTAssertEqual(graph.links.map(\.kind), [.contains, .contains, .uses, .involves, .relates])
        XCTAssertEqual(graph.links.map(\.memoryCount), [0, 0, 3, 2, 2])
        let related = graph.links[4]
        XCTAssertEqual(graph.nodes[related.sourceIndex].key, "project:1")
        XCTAssertEqual(graph.nodes[related.targetIndex].key, "project:3")
        XCTAssertEqual(graph.shelf.map(\.reason), [.staleProject, .outsideRegistry])
        XCTAssertEqual(graph.shelf[0].id, "project:4")
        XCTAssertEqual(graph.shelf[0].label, "Ledger")
        XCTAssertEqual(graph.shelf[0].memoryCount, 4)
        XCTAssertEqual(graph.shelf[0].lastActivity, 1_796_112_000)
        XCTAssertEqual(graph.shelf[0].inactiveDays, 45)
        XCTAssertEqual(graph.shelf[0].projectSlug, "ledger")
        XCTAssertEqual(graph.shelf[1].id, "entity:phantom")
        XCTAssertEqual(graph.shelf[1].memoryCount, 2)
        XCTAssertNil(graph.shelf[1].lastActivity)
        XCTAssertNil(graph.shelf[1].inactiveDays)
        XCTAssertNil(graph.shelf[1].projectSlug)
        XCTAssertEqual(graph.omitted.nodeCount, 1)
        XCTAssertEqual(graph.omitted.linkCount, 0)
        XCTAssertEqual(graph.omitted.staleProjectCount, 0)
        XCTAssertEqual(graph.omitted.outsideRegistryCount, 0)
        try roundTrip(graph)
    }

    func testOwnerAndAgentHoursKeepTenthsZeroAndUnknownSeparate() throws {
        let graph = try JSONDecoder().decode(CuratedGraphSnapshot.self, from: fixture())
        XCTAssertEqual(graph.nodes[0].hours30Days.ownerTenths, 15)
        XCTAssertEqual(graph.nodes[0].hours30Days.agentTenths, 120)
        XCTAssertEqual(graph.nodes[0].hours30Days.ownerHours, 1.5)
        XCTAssertEqual(graph.nodes[0].hours30Days.agentHours, 12)
        XCTAssertEqual(graph.nodes[0].sizeBucket, 2)
        XCTAssertEqual(graph.nodes[1].hours30Days.ownerHours, 0)
        XCTAssertNil(graph.nodes[1].hours30Days.agentHours)
        XCTAssertNil(graph.nodes[2].hours30Days.ownerHours)
        XCTAssertEqual(graph.nodes[2].hours30Days.agentHours, 0)
        XCTAssertNil(graph.nodes[3].hours30Days.ownerHours)
        XCTAssertNil(graph.nodes[3].hours30Days.agentHours)
    }

    func testEncodePreservesCompactKeysTuplesAndExplicitNulls() throws {
        let graph = try JSONDecoder().decode(CuratedGraphSnapshot.self, from: fixture())
        let encoded = try JSONEncoder().encode(graph)
        let serialized = try XCTUnwrap(JSONSerialization.jsonObject(with: encoded) as? NSDictionary)
        XCTAssertEqual(serialized, try object() as NSDictionary)
        XCTAssertEqual(Set(serialized.allKeys.compactMap { key in key as? String }), ["v", "n", "e", "s", "o"])
    }

    func testOutOfRangeReferencesAreRejectedBeforeTheGraphIsExposed() throws {
        let original = try object()
        let count = try XCTUnwrap(original["n"] as? [[Any]]).count
        for field in [0, 1] {
            for invalid in [-1, count, 65_536] {
                XCTAssertThrowsError(try decode(replacing("e", field: field, with: invalid)))
            }
        }
        var empty = original
        empty["n"] = [] as [Any]
        XCTAssertThrowsError(try decode(empty))
    }

    func testUnsupportedSchemaAndUnknownNumericEnumsAreRejected() throws {
        var newer = try object()
        newer["v"] = 2
        XCTAssertThrowsError(try decode(newer))
        for (group, field) in [("n", 2), ("n", 8), ("e", 2), ("s", 2)] {
            XCTAssertThrowsError(try decode(replacing(group, field: field, with: 255)))
            XCTAssertThrowsError(try decode(replacing(group, field: field, with: "active")))
        }
        // Archived is a known state even though the current route omits such nodes.
        let archived = try decode(replacing("n", field: 8, with: 3))
        XCTAssertEqual(archived.nodes[0].state, .archived)
        try roundTrip(archived)
    }

    func testTupleLengthsAreExactIncludingTheOptionalTailFields() throws {
        for group in ["n", "e", "s"] {
            for add in [false, true] {
                var object = try object()
                var rows = try XCTUnwrap(object[group] as? [[Any]])
                if add { rows[0].append(NSNull()) } else { rows[0].removeLast() }
                object[group] = rows
                XCTAssertThrowsError(try decode(object), "Invalid \(group) tuple length")
            }
        }
        for hours in [[NSNull()] as [Any], [NSNull(), 0, 0] as [Any]] {
            XCTAssertThrowsError(try decode(replacing("n", field: 5, with: hours)))
        }
        for omitted in [[0, 0, 0], [0, 0, 0, 0, 0]] {
            var object = try object()
            object["o"] = omitted
            XCTAssertThrowsError(try decode(object))
        }
    }

    func testNumericBoundsAndDuplicateNodeIdentitiesAreRejected() throws {
        XCTAssertThrowsError(try decode(replacing("n", field: 3, with: 2_147_483_648)))
        XCTAssertThrowsError(try decode(replacing("n", field: 5, with: [-1, 0])))
        XCTAssertThrowsError(try decode(replacing("n", field: 7, with: -1)))
        XCTAssertThrowsError(try decode(replacing("n", field: 9, with: 5)))
        XCTAssertThrowsError(try decode(replacing("e", field: 3, with: -1)))
        XCTAssertThrowsError(try decode(replacing("s", field: 3, with: -1)))
        var object = try object()
        var nodes = try XCTUnwrap(object["n"] as? [[Any]])
        nodes[1][0] = nodes[0][0]
        object["n"] = nodes
        XCTAssertThrowsError(try decode(object))
    }

    func testEmptyGraphAndNonzeroOmissionCountsRoundTrip() throws {
        let graph = try decode(["v": 1, "n": [] as [Any], "e": [] as [Any],
            "s": [] as [Any], "o": [3, 4, 5, 6]])
        XCTAssertTrue(graph.nodes.isEmpty)
        XCTAssertTrue(graph.links.isEmpty)
        XCTAssertTrue(graph.shelf.isEmpty)
        XCTAssertEqual(graph.omitted.nodeCount, 3)
        XCTAssertEqual(graph.omitted.linkCount, 4)
        XCTAssertEqual(graph.omitted.staleProjectCount, 5)
        XCTAssertEqual(graph.omitted.outsideRegistryCount, 6)
        try roundTrip(graph)
    }
}
