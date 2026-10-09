import Foundation
import MnemonicShared

extension MnemonicService {
    func httpObject(_ path: String) -> [String: Any]? {
        guard let reply = httpReply(path),
              (200..<300).contains(reply.status),
              let json = try? JSONSerialization.jsonObject(with: reply.body) as? [String: Any]
        else { return nil }
        return json
    }

    /// The route's answer as it came: nil when the API is off, the token
    /// is absent or nothing answered in time; else the status and the
    /// body, whatever the status. The card of memory today reads the
    /// status itself, so that a daemon without the route (404) is told
    /// apart from a daemon without the API.
    func httpReply(_ path: String) -> (status: Int, body: Data)? {
        if let reply = environment.http { return reply(path) }
        guard let token = readHTTPToken() else { return nil }
        let base = MnemonicProfile.current.endpoint

        let encodedPath = path.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? path
        guard let url = URL(string: encodedPath, relativeTo: base) else { return nil }

        var req = URLRequest(url: url, timeoutInterval: 1.5)
        req.setValue(token, forHTTPHeaderField: "x-mnemonic-token")

        let sem = DispatchSemaphore(value: 0)
        var out: (status: Int, body: Data)?
        Self.session.dataTask(with: req) { data, resp, _ in
            defer { sem.signal() }
            guard let http = resp as? HTTPURLResponse else { return }
            out = (status: http.statusCode, body: data ?? Data())
        }.resume()
        _ = sem.wait(timeout: .now() + 2.0)
        return out
    }

    /// One session for every request, configured by the profile: an
    /// isolated client keeps no response on disk.
    private static let session = URLSession(
        configuration: MnemonicProfile.current.sessionConfiguration())

    private func readHTTPToken() -> String? {
        // Only the daemon-written auth.token of this profile, and in an
        // isolated profile only when the file lies inside it. (Dropped a
        // legacy dashboard.token fallback: the daemon never writes it, so
        // honouring it would let any other process plant a token the widget
        // would trust.)
        MnemonicProfile.current.token()
    }
}
