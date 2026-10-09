import Foundation
import MnemonicShared

actor TimelineAPITransport: LocalTransport {
    struct Refused: Error {}
    private var script: [Result<LocalResponse, Error>]
    private var held = false
    private var requests: [URLRequest] = []
    private var notices: [Int: @Sendable () -> Void] = [:]
    private var replies: [Int: (CheckedContinuation<LocalResponse, Error>, Result<LocalResponse, Error>)] = [:]

    init(_ script: [Result<LocalResponse, Error>]) { self.script = script }
    func hold() { held = true }
    func allRequests() -> [URLRequest] { requests }

    func requested(_ count: Int, _ notify: @escaping @Sendable () -> Void) {
        if requests.count >= count { notify() } else { notices[count] = notify }
    }

    func send(_ request: URLRequest) async throws -> LocalResponse {
        requests.append(request)
        let number = requests.count
        let answer: Result<LocalResponse, Error> = script.isEmpty ? .failure(Refused()) : script.removeFirst()
        let notify = notices.removeValue(forKey: number)
        guard held else { notify?(); return try answer.get() }
        return try await withCheckedThrowingContinuation { continuation in
            replies[number] = (continuation, answer)
            notify?()
        }
    }

    func finish(_ number: Int) {
        guard let (reply, answer) = replies.removeValue(forKey: number) else { return }
        reply.resume(with: answer)
    }
}

final class TimelineAPITokens: @unchecked Sendable {
    private let lock = NSLock()
    private var reads = 0

    func next() -> String {
        lock.withLock {
            reads += 1
            return "fixture-token-\(reads)"
        }
    }

    var count: Int { lock.withLock { reads } }
}
