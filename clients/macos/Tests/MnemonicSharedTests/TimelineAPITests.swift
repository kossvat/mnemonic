import Foundation
import XCTest
@testable import MnemonicShared

final class TimelineAPITests: XCTestCase {
    private func fixture(_ name: String) throws -> Data {
        let path = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().appendingPathComponent("Fixtures/\(name).json")
        return try Data(contentsOf: path)
    }

    private func response(_ status: Int, _ body: Data = Data(), tag: String? = nil)
        -> Result<LocalResponse, Error>
    {
        .success(LocalResponse(status: status, tag: tag, body: body))
    }

    private func endpoint(_ port: Int = 3737) -> URL {
        var parts = URLComponents()
        parts.scheme = "http"
        parts.host = LocalEndpoint.loopbackHost
        parts.port = port
        return parts.url!
    }

    private func api(_ transport: LocalTransport, tokens: TimelineAPITokens = TimelineAPITokens(),
                     enabled: Bool = true, locked: Bool = false) -> LocalAPI {
        LocalAPI(endpoint: endpoint(), enabled: { enabled }, token: { tokens.next() },
                 transport: transport, endpointLocked: locked)
    }

    private func pin() -> AttributionCorrection {
        AttributionCorrection(stream: "fixture-stream", project: nil)
    }

    func testTimelineQueryAndMutationsKeepTheLiveETagIndependent() async throws {
        let timeline = try fixture("project-timeline")
        let decoded = try JSONDecoder().decode(ProjectTimeline.self, from: timeline)
        let server = TimelineAPITransport([
            response(200, try fixture("live"), tag: "live-tag"),
            response(200, timeline, tag: "timeline-tag"),
            response(200, Data("{\"id\":7}".utf8), tag: "correction-tag"),
            response(200, Data("{\"removed\":7}".utf8)), response(304),
        ])
        let local = api(server)
        _ = await local.live()
        let answer = await local.timeline(day: decoded.day)
        let added = await local.addAttributionOverride(pin())
        let removed = await local.removeAttributionOverride(id: 7)
        let unchanged = await local.live()
        XCTAssertEqual(answer, .success(decoded))
        XCTAssertEqual(added, .success(7))
        if case .failure(let failure) = removed { XCTFail("Unexpected removal failure: \(failure)") }
        XCTAssertEqual(unchanged, .unchanged)
        let requests = await server.allRequests()
        XCTAssertEqual(requests.map(\.httpMethod), ["GET", "GET", "POST", "DELETE", "GET"])
        let url = try XCTUnwrap(requests[1].url)
        let parts = try XCTUnwrap(URLComponents(url: url, resolvingAgainstBaseURL: false))
        XCTAssertEqual(parts.percentEncodedPath, "/api/projects/timeline")
        XCTAssertEqual(parts.queryItems, [URLQueryItem(name: "day", value: decoded.day)])
        XCTAssertNil(parts.fragment)
        XCTAssertEqual(requests[2].url?.path, "/api/attribution/overrides")
        XCTAssertEqual(requests[3].url?.path, "/api/attribution/overrides/7")
        XCTAssertEqual(requests[2].value(forHTTPHeaderField: "Content-Type"), "application/json")
        let body = try XCTUnwrap(requests[2].httpBody)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: body) as? [String: Any])
        XCTAssertTrue(object["project"] is NSNull)
        XCTAssertNil(requests[3].httpBody)
        for request in requests[1...3] {
            XCTAssertNil(request.value(forHTTPHeaderField: "If-None-Match"))
            XCTAssertEqual(request.timeoutInterval, 5)
            XCTAssertEqual(request.cachePolicy, .reloadIgnoringLocalCacheData)
        }
        XCTAssertEqual(requests[4].value(forHTTPHeaderField: "If-None-Match"), "live-tag")
    }

    func testInvalidDaysCorrectionsAndUndoIDsDoNotReadTokenOrSend() async {
        let server = TimelineAPITransport([]), tokens = TimelineAPITokens()
        let local = api(server, tokens: tokens)
        for day in ["2026-02-29", "2026-2-03", "2026-06-17?extra=1", "2026-06-17/part"] {
            let answer = await local.timeline(day: day)
            XCTAssertEqual(answer, .failure(.undecodable))
        }
        for correction in [AttributionCorrection(project: nil),
                           AttributionCorrection(stream: "\n", project: nil),
                           AttributionCorrection(stream: "fixture", project: "../project")] {
            let answer = await local.addAttributionOverride(correction)
            XCTAssertEqual(answer, .failure(.undecodable))
        }
        for id in [Int64(0), -1] {
            let answer = await local.removeAttributionOverride(id: id)
            guard case .failure(.undecodable) = answer else { XCTFail("Accepted invalid undo id"); continue }
        }
        let requests = await server.allRequests()
        XCTAssertTrue(requests.isEmpty)
        XCTAssertEqual(tokens.count, 0)
    }

    func testOffRoutesDoNotSendAndMismatchedTimelineDayDoesNotDecodeAsRequestedDay() async throws {
        let server = TimelineAPITransport([]), tokens = TimelineAPITokens()
        let disabled = api(server, tokens: tokens, enabled: false)
        let timeline = await disabled.timeline(day: "2026-09-29")
        let added = await disabled.addAttributionOverride(pin())
        let removed = await disabled.removeAttributionOverride(id: 7)
        XCTAssertEqual(timeline, .failure(.off))
        XCTAssertEqual(added, .failure(.off))
        guard case .failure(.off) = removed else { return XCTFail("Disabled deletion was accepted") }
        let requests = await server.allRequests()
        XCTAssertTrue(requests.isEmpty)
        XCTAssertEqual(tokens.count, 0)
        let wrong = TimelineAPITransport([response(200, try fixture("project-timeline"))])
        let answer = await api(wrong).timeline(day: "2026-09-30")
        XCTAssertEqual(answer, .failure(.undecodable))
    }

    func testMutation401ReloadsTokenOnceWithoutChangingMethodOrBody() async throws {
        for deleting in [false, true] {
            let body = Data((deleting ? "{\"removed\":7}" : "{\"id\":7}").utf8)
            let server = TimelineAPITransport([response(401), response(200, body)])
            let tokens = TimelineAPITokens(), local = api(server, tokens: tokens)
            if deleting { _ = await local.removeAttributionOverride(id: 7) }
            else {
                let answer = await local.addAttributionOverride(pin())
                XCTAssertEqual(answer, .success(7))
            }
            let requests = await server.allRequests()
            XCTAssertEqual(requests.count, 2)
            XCTAssertEqual(tokens.count, 2)
            XCTAssertEqual(requests[0].httpBody, requests[1].httpBody)
            XCTAssertEqual(requests[0].httpMethod, requests[1].httpMethod)
            XCTAssertEqual(requests[0].url, requests[1].url)
            XCTAssertEqual(requests[0].value(forHTTPHeaderField: LocalAPI.tokenHeader), "fixture-token-1")
            XCTAssertEqual(requests[1].value(forHTTPHeaderField: LocalAPI.tokenHeader), "fixture-token-2")
        }
    }

    func testFailuresNeverRetryAnUncertainMutationAndConfirmTheReturnedID() async {
        let cases: [(Result<LocalResponse, Error>, LocalAPIFailure)] = [
            (.failure(TimelineAPITransport.Refused()), .unreachable),
            (response(500), .undecodable), (response(302), .undecodable),
            (response(404), .earlierDaemon), (response(200, Data("{}".utf8)), .undecodable),
        ]
        for (reply, reason) in cases {
            let server = TimelineAPITransport([reply])
            let answer = await api(server).addAttributionOverride(pin())
            XCTAssertEqual(answer, .failure(reason))
            let requests = await server.allRequests()
            XCTAssertEqual(requests.count, 1)
        }
        let unauthorised = TimelineAPITransport([response(401), response(401), response(200)])
        let answer = await api(unauthorised).addAttributionOverride(pin())
        XCTAssertEqual(answer, .failure(.unauthorised))
        let attempts = await unauthorised.allRequests()
        XCTAssertEqual(attempts.count, 2)
        let invalid = TimelineAPITransport([
            response(200, Data("{\"id\":0}".utf8)), response(200, Data("{\"removed\":8}".utf8)),
        ])
        let local = api(invalid)
        let added = await local.addAttributionOverride(pin())
        XCTAssertEqual(added, .failure(.undecodable))
        let removed = await local.removeAttributionOverride(id: 7)
        guard case .failure(.undecodable) = removed else { return XCTFail("Accepted wrong removal id") }
    }

    func testEndpointChangeRejectsLateReadMutationAnd401WithoutResending() async throws {
        for kind in 0...2 {
            for status in [200, 401] {
                let body = kind == 0 ? try fixture("project-timeline")
                    : Data((kind == 1 ? "{\"id\":7}" : "{\"removed\":7}").utf8)
                let server = TimelineAPITransport([response(status, body)])
                await server.hold()
                let local = api(server)
                let correction = pin()
                let arrived = expectation(description: "Old endpoint request is held")
                await server.requested(1) { arrived.fulfill() }
                let asking = Task { () -> LocalAPIFailure? in
                    switch kind {
                    case 0:
                        if case .failure(let failure) = await local.timeline(day: "2026-09-29") { return failure }
                    case 1:
                        if case .failure(let failure) = await local.addAttributionOverride(correction) { return failure }
                    default:
                        if case .failure(let failure) = await local.removeAttributionOverride(id: 7) { return failure }
                    }
                    return nil
                }
                await fulfillment(of: [arrived], timeout: 5)
                try await local.setEndpoint(endpoint(4444).absoluteString)
                await server.finish(1)
                let failure = await asking.value
                XCTAssertEqual(failure, .unreachable)
                let requests = await server.allRequests()
                XCTAssertEqual(requests.count, 1)
            }
        }
    }

    func testFixedEndpointCannotMoveBeforeATimelineMutation() async throws {
        let server = TimelineAPITransport([response(200, Data("{\"id\":7}".utf8))])
        let local = api(server, locked: true)
        do {
            try await local.setEndpoint(endpoint(4444).absoluteString)
            XCTFail("Changed a fixed profile")
        } catch { XCTAssertEqual(error as? LocalEndpointFailure, .fixedProfile) }
        let added = await local.addAttributionOverride(pin())
        XCTAssertEqual(added, .success(7))
        let requests = await server.allRequests()
        XCTAssertEqual(requests.first?.url?.port, 3737)
    }
}
