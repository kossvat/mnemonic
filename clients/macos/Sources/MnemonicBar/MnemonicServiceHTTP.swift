import Foundation

extension MnemonicService {
    func httpObject(_ path: String) -> [String: Any]? {
        guard let token = readHTTPToken(),
              let base = URL(string: "http://127.0.0.1:3737")
        else { return nil }

        let encodedPath = path.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? path
        guard let url = URL(string: encodedPath, relativeTo: base) else { return nil }

        var req = URLRequest(url: url, timeoutInterval: 1.5)
        req.setValue(token, forHTTPHeaderField: "x-mnemonic-token")

        let sem = DispatchSemaphore(value: 0)
        var out: [String: Any]?
        URLSession.shared.dataTask(with: req) { data, resp, _ in
            defer { sem.signal() }
            guard let http = resp as? HTTPURLResponse,
                  (200..<300).contains(http.statusCode),
                  let data,
                  let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
            else { return }
            out = json
        }.resume()
        _ = sem.wait(timeout: .now() + 2.0)
        return out
    }

    private func readHTTPToken() -> String? {
        let home = FileManager.default.homeDirectoryForCurrentUser
        // Only the daemon-written auth.token. (Dropped a legacy dashboard.token
        // fallback: the daemon never writes it, so honouring it would let any
        // other process plant a token the widget would trust.)
        let paths = [
            home.appendingPathComponent(".mnemonic/auth.token"),
        ]
        for p in paths where FileManager.default.fileExists(atPath: p.path) {
            if let s = try? String(contentsOf: p, encoding: .utf8) {
                let token = s.trimmingCharacters(in: .whitespacesAndNewlines)
                if !token.isEmpty { return token }
            }
        }
        return nil
    }
}
