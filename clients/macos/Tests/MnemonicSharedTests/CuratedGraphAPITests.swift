import Foundation
import XCTest
@testable import MnemonicShared

final class CuratedGraphAPITests: XCTestCase {
    func fixture() throws -> Data {
        let url = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().appendingPathComponent("Fixtures/curated-graph.json")
        return try Data(contentsOf: url)
    }
    func endpoint(_ port: Int = 3737) -> URL {
        var parts = URLComponents()
        parts.scheme = "http"
        parts.host = LocalEndpoint.loopbackHost
        parts.port = port
        return parts.url!
    }
    func api(_ server: LocalTransport, tokens: TimelineAPITokens = TimelineAPITokens()) -> LocalAPI {
        LocalAPI(endpoint: endpoint(), enabled: { true }, token: { tokens.next() }, transport: server)
    }
    func response(_ status: Int, _ body: Data = Data(), tag: String? = nil) -> Result<LocalResponse, Error> {
        .success(LocalResponse(status: status, tag: tag, body: body))
    }
    func json(_ object: Any) throws -> Data { try JSONSerialization.data(withJSONObject: object) }

    func testOverviewAndExpansionETagsAreIndependentAndOnlyEightExpansionsRemain() async throws {
        let body = try fixture()
        var script = [response(200, body, tag: "overview")]
        for index in 1...9 { script.append(response(200, body, tag: "expanded-\(index)")) }
        script += [response(304), response(304), response(304)]
        let server = TimelineAPITransport(script), local = api(server)
        _ = await local.curatedGraph()
        for index in 1...9 { _ = await local.curatedGraph(expand: "project:\(index)") }
        let overview = await local.curatedGraph()
        let retained = await local.curatedGraph(expand: "project:9")
        let evicted = await local.curatedGraph(expand: "project:1")
        let expected = try JSONDecoder().decode(CuratedGraphSnapshot.self, from: body)
        XCTAssertEqual(overview, .success(expected))
        XCTAssertEqual(retained, .success(expected))
        XCTAssertEqual(evicted, .failure(.local(.undecodable)))
        let requests = await server.allRequests()
        XCTAssertEqual(requests[10].value(forHTTPHeaderField: "If-None-Match"), "overview")
        XCTAssertEqual(requests[11].value(forHTTPHeaderField: "If-None-Match"), "expanded-9")
        XCTAssertNil(requests[12].value(forHTTPHeaderField: "If-None-Match"))
    }

    func testUncached304AndFailuresClearOnlyTheirMatchingScope() async throws {
        let body = try fixture()
        let server = TimelineAPITransport([
            response(304), response(200, body, tag: "overview"),
            response(200, body, tag: "expanded"), response(500), response(304), response(304),
        ])
        let local = api(server)
        let uncached = await local.curatedGraph()
        XCTAssertEqual(uncached, .failure(.local(.undecodable)))
        _ = await local.curatedGraph()
        _ = await local.curatedGraph(expand: "project:1")
        _ = await local.curatedGraph(expand: "project:1")
        let overview = await local.curatedGraph()
        let expansion = await local.curatedGraph(expand: "project:1")
        XCTAssertEqual(overview, .success(try JSONDecoder().decode(CuratedGraphSnapshot.self, from: body)))
        XCTAssertEqual(expansion, .failure(.local(.undecodable)))
        let requests = await server.allRequests()
        XCTAssertEqual(requests[4].value(forHTTPHeaderField: "If-None-Match"), "overview")
        XCTAssertNil(requests[5].value(forHTTPHeaderField: "If-None-Match"))
    }

    func testEncodedQueriesOpaquePathsAndArchiveBodiesUseExistingLocalTransport() async throws {
        let memoryID = String(repeating: "m", count: 210) + "/part?fragment#name%"
        let entityID = "entity/part?fragment#name%"
        let memory: [String: Any] = ["id": memoryID, "title": "Fixture", "content": "Evidence",
            "timestamp": "2026-09-30T12:00:00Z", "memory_type": "fact", "tags": [], "importance": 0.7]
        let server = TimelineAPITransport([
            response(200, try json(["memory_ids": [memoryID], "offset": 0, "limit": 10, "next_offset": NSNull()])),
            response(200, try json(memory)),
            response(200, try json(["entities": [], "offset": 0, "limit": 20, "total": 0, "next_offset": NSNull()])),
            response(200, try json(["id": entityID, "archived": true])),
            response(200, try json(["key": "harbor:api", "state": "active"])),
        ])
        let local = api(server)
        let edge = await local.graphEdge(source: "project:1", target: "unit:1:change-part", kind: .contains)
        XCTAssertEqual(try edge.get().memoryIDs, [memoryID])
        let memoryAnswer = await local.graphMemory(id: memoryID)
        XCTAssertEqual(try memoryAnswer.get().id, memoryID)
        _ = await local.graphArchives(archived: false)
        _ = await local.setEntityArchived(id: entityID, archived: true)
        _ = await local.setProjectArchived(key: "harbor:api", archived: false)
        let requests = await server.allRequests()
        let edgeURL = try XCTUnwrap(requests[0].url)
        let query = try XCTUnwrap(URLComponents(url: edgeURL, resolvingAgainstBaseURL: false)).queryItems
        XCTAssertTrue(query?.contains(URLQueryItem(name: "target", value: "unit:1:change-part")) == true)
        let memoryURL = try XCTUnwrap(requests[1].url)
        let parts = try XCTUnwrap(URLComponents(url: memoryURL, resolvingAgainstBaseURL: false))
        XCTAssertTrue(parts.percentEncodedPath.contains("%2Fpart%3Ffragment%23name%25"))
        XCTAssertNil(parts.query)
        XCTAssertNil(parts.fragment)
        let archiveURL = try XCTUnwrap(requests[3].url)
        XCTAssertTrue(URLComponents(url: archiveURL, resolvingAgainstBaseURL: false)!.percentEncodedPath
            .contains("entity%2Fpart%3Ffragment%23name%25/archive"))
        XCTAssertEqual(requests[3].httpMethod, "POST")
        XCTAssertEqual(try JSONSerialization.jsonObject(with: XCTUnwrap(requests[3].httpBody)) as? [String: Bool],
                       ["archived": true])
        XCTAssertEqual(requests[4].url?.path, "/api/projects/harbor:api/unarchive")
        XCTAssertNil(requests[4].httpBody)
        for request in requests {
            XCTAssertNil(request.value(forHTTPHeaderField: "If-None-Match"))
            XCTAssertEqual(request.timeoutInterval, 5)
            XCTAssertEqual(request.cachePolicy, .reloadIgnoringLocalCacheData)
        }
    }

    func testInvalidIdentityPagesAndMismatchedMemoryNeverBecomeUsableReplies() async throws {
        let server = TimelineAPITransport([]), local = api(server)
        for key in ["project:0", "project:01", "unit:1:", "entity:..", "ring:tech", "project:1?other=2"] {
            let answer = await local.curatedGraph(expand: key)
            XCTAssertEqual(answer, .failure(.local(.undecodable)))
        }
        let badPage = await local.graphArchives(archived: true, offset: -1)
        let hugePage = await local.graphEdge(source: "project:1", target: "project:2", kind: .relates, limit: 101)
        let badMemory = await local.graphMemory(id: "..")
        let badArchive = await local.setEntityArchived(id: String(repeating: "a", count: 193), archived: true)
        XCTAssertEqual(badPage, .failure(.local(.undecodable)))
        XCTAssertEqual(hugePage, .failure(.local(.undecodable)))
        XCTAssertEqual(badMemory, .failure(.local(.undecodable)))
        if case .success = badArchive { XCTFail("Accepted an invalid archive id") }
        let requests = await server.allRequests()
        XCTAssertTrue(requests.isEmpty)
    }

    func testNotFoundScopeAndUnavailableRemainDistinctAndMutation401RetriesOnce() async throws {
        let tokens = TimelineAPITokens()
        let server = TimelineAPITransport([
            response(404), response(404), response(503), response(401), response(200,
                try json(["id": "fixture", "archived": false])), response(500),
        ])
        let local = api(server, tokens: tokens)
        let overview = await local.curatedGraph()
        let expanded = await local.curatedGraph(expand: "project:1")
        let unavailable = await local.curatedGraph()
        let restored = await local.setEntityArchived(id: "fixture", archived: false)
        let uncertain = await local.setEntityArchived(id: "fixture", archived: true)
        XCTAssertEqual(overview, .failure(.local(.earlierDaemon)))
        XCTAssertEqual(expanded, .failure(.notFound))
        XCTAssertEqual(unavailable, .failure(.unavailable))
        if case .failure = restored { XCTFail("Authenticated restore failed") }
        if case .success = uncertain { XCTFail("Unconfirmed write became success") }
        let requests = await server.allRequests()
        XCTAssertEqual(requests.count, 6)
        XCTAssertEqual(requests[3].httpBody, requests[4].httpBody)
        XCTAssertEqual(requests[3].httpMethod, requests[4].httpMethod)
        XCTAssertEqual(tokens.count, 2)
    }

    func testEndpointChangeRevokesHeldResponseAndClearsOverviewAndExpansionTags() async throws {
        for status in [200, 401] {
            let body = try fixture()
            let server = TimelineAPITransport([response(200, body, tag: "overview"),
                response(200, body, tag: "expanded"), response(status, body, tag: "late"),
                response(200, body, tag: "new"), response(200, body)])
            let local = api(server)
            _ = await local.curatedGraph()
            _ = await local.curatedGraph(expand: "project:1")
            await server.hold()
            let arrived = expectation(description: "Old endpoint graph held")
            await server.requested(3) { arrived.fulfill() }
            let reading = Task { await local.curatedGraph() }
            await fulfillment(of: [arrived], timeout: 5)
            try await local.setEndpoint(endpoint(4444).absoluteString)
            await server.finish(3)
            let old = await reading.value
            XCTAssertEqual(old, .failure(.local(.unreachable)))
            for index in 4...5 {
                let next = expectation(description: "New endpoint graph held")
                await server.requested(index) { next.fulfill() }
                let reading = Task { await local.curatedGraph(expand: index == 4 ? nil : "project:1") }
                await fulfillment(of: [next], timeout: 5)
                await server.finish(index)
                _ = await reading.value
            }
            let requests = await server.allRequests()
            XCTAssertEqual(requests.count, 5)
            XCTAssertNil(requests[3].value(forHTTPHeaderField: "If-None-Match"))
            XCTAssertNil(requests[4].value(forHTTPHeaderField: "If-None-Match"))
            XCTAssertEqual(requests[3].url?.port, 4444)
        }
    }

    func testLateSameScopeResponseCannotOverwriteTheNewerETag() async throws {
        let body = try fixture()
        let server = TimelineAPITransport([response(200, body, tag: "older"),
            response(200, body, tag: "newer"), response(304)])
        await server.hold()
        let local = api(server)
        let first = expectation(description: "First graph read held")
        await server.requested(1) { first.fulfill() }
        let old = Task { await local.curatedGraph() }
        await fulfillment(of: [first], timeout: 5)
        let second = expectation(description: "New graph read held")
        await server.requested(2) { second.fulfill() }
        let new = Task { await local.curatedGraph() }
        await fulfillment(of: [second], timeout: 5)
        await server.finish(2)
        _ = await new.value
        await server.finish(1)
        let oldResult = await old.value
        XCTAssertEqual(oldResult, .failure(.local(.unreachable)))
        let third = expectation(description: "Conditional graph read held")
        await server.requested(3) { third.fulfill() }
        let current = Task { await local.curatedGraph() }
        await fulfillment(of: [third], timeout: 5)
        await server.finish(3)
        _ = await current.value
        let requests = await server.allRequests()
        XCTAssertEqual(requests[2].value(forHTTPHeaderField: "If-None-Match"), "newer")
    }

    func testHashedUnitKeysReachExpansionAndEvidenceRoutesUnchanged() async throws {
        let digest = "0123456789abcdef0123456789abcdef01234567"
        let key = "unit:1:h:" + digest
        let body = Data(String(decoding: try fixture(), as: UTF8.self)
            .replacingOccurrences(of: "unit:1:LP-12", with: key).utf8)
        let server = TimelineAPITransport([
            response(200, body, tag: "hashed-unit"),
            response(200, try json(["memory_ids": ["fixture-evidence"], "offset": 0,
                                   "limit": 10, "next_offset": NSNull()])),
            response(304),
        ])
        let local = api(server)
        let expanded = await local.curatedGraph(expand: key)
        let edge = await local.graphEdge(source: "project:1", target: key, kind: .contains)
        let cached = await local.curatedGraph(expand: key)
        let snapshot = try expanded.get()
        XCTAssertTrue(snapshot.nodes.contains { node in node.key == key })
        XCTAssertEqual(try edge.get().memoryIDs, ["fixture-evidence"])
        XCTAssertEqual(cached, .success(snapshot))
        let requests = await server.allRequests()
        XCTAssertEqual(requests.count, 3)
        XCTAssertEqual(requests[0].url?.path, "/api/graph/curated")
        XCTAssertEqual(requests[1].url?.path, "/api/graph/curated/edge")
        let expansion = try XCTUnwrap(URLComponents(url: XCTUnwrap(requests[0].url), resolvingAgainstBaseURL: false))
        let evidence = try XCTUnwrap(URLComponents(url: XCTUnwrap(requests[1].url), resolvingAgainstBaseURL: false))
        XCTAssertEqual(expansion.queryItems, [URLQueryItem(name: "expand", value: key)])
        XCTAssertTrue(evidence.queryItems?.contains(URLQueryItem(name: "target", value: key)) == true)
        XCTAssertEqual(requests[2].value(forHTTPHeaderField: "If-None-Match"), "hashed-unit")
    }

    func testMalformedHashesReservedFormsAndDirectSeparatorsNeverSend() async {
        let server = TimelineAPITransport([]), tokens = TimelineAPITokens()
        let local = api(server, tokens: tokens)
        let digest = String(repeating: "a", count: 40)
        var keys = ["unit:1:h:", "unit:1:h:" + String(repeating: "a", count: 39),
                    "unit:1:h:" + String(repeating: "a", count: 41),
                    "unit:1:h:" + String(repeating: "a", count: 64),
                    "unit:1:h:" + String(repeating: "A", count: 40),
                    "unit:1:h:" + String(repeating: "g", count: 40),
                    "unit:1:H:" + digest, "unit:1:h:" + digest + ":part",
                    "unit:01:h:" + digest, "unit:0:h:" + digest, "unit:1:h::" + digest]
        keys += ["unit:1:change/part", "unit:1:change:part", "unit:1:.", "unit:1:..",
                 "unit:1:" + String(repeating: "x", count: 161), "entity:person/fixture"]
        for key in keys {
            let expanded = await local.curatedGraph(expand: key)
            let edge = await local.graphEdge(source: key, target: "project:1", kind: .contains)
            XCTAssertEqual(expanded, .failure(.local(.undecodable)), key)
            XCTAssertEqual(edge, .failure(.local(.undecodable)), key)
        }
        let requests = await server.allRequests()
        XCTAssertTrue(requests.isEmpty)
        XCTAssertEqual(tokens.count, 0)
    }

}
