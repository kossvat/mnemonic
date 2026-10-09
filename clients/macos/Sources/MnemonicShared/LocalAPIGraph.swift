import Foundation

extension LocalAPI {
    public func curatedGraph(expand: String? = nil)
        async -> Result<CuratedGraphSnapshot, CuratedGraphFailure>
    {
        guard expand == nil || Self.graphKey(expand!) else { return .failure(.local(.undecodable)) }
        let version = graphEndpointVersion
        let ticket = graphCache.begin(expand)
        let query = expand.map { key in [URLQueryItem(name: "expand", value: key)] } ?? []
        let reply = await graphResponse(path: "/api/graph/curated", tag: ticket.previous?.tag, query: query)
        guard version == graphEndpointVersion, graphCache.isCurrent(ticket) else {
            return .failure(.local(.unreachable))
        }
        let result: Result<CuratedGraphSnapshot, CuratedGraphFailure>
        var entry: CuratedGraphCache.Entry?
        switch reply {
        case .failure(let failure): result = .failure(.local(failure))
        case .success(let answer):
            if answer.status == 304, let previous = ticket.previous, previous.tag != nil {
                entry = previous
                result = .success(previous.snapshot)
            } else if answer.status == 200,
                      let snapshot = try? JSONDecoder().decode(CuratedGraphSnapshot.self, from: answer.body) {
                entry = .init(snapshot: snapshot, tag: answer.tag)
                result = .success(snapshot)
            } else { result = .failure(Self.graphFailure(answer.status, overview: expand == nil)) }
        }
        graphCache.finish(ticket, entry: entry)
        return result
    }

    public func graphEdge(source: String, target: String, kind: CuratedGraphLinkKind,
                          offset: Int = 0, limit: Int = 10)
        async -> Result<CuratedGraphEvidencePage, CuratedGraphFailure>
    {
        guard Self.graphKey(source), Self.graphKey(target), Self.validPage(offset, limit) else {
            return .failure(.local(.undecodable))
        }
        let answer = await graphDecoded(CuratedGraphEvidencePage.self, path: "/api/graph/curated/edge",
            query: [URLQueryItem(name: "source", value: source), URLQueryItem(name: "target", value: target),
                    URLQueryItem(name: "kind", value: kind.queryValue)] + Self.pageQuery(offset, limit))
        if case .success(let page) = answer {
            guard page.offset == offset, page.limit == limit, page.memoryIDs.count <= limit,
                  Set(page.memoryIDs).count == page.memoryIDs.count,
                  page.memoryIDs.allSatisfy({ id in Self.opaque(id) != nil }),
                  Self.validNext(page.nextOffset, offset, page.memoryIDs.count) else {
                return .failure(.local(.undecodable))
            }
        }
        return answer
    }

    public func graphArchives(archived: Bool, offset: Int = 0, limit: Int = 20)
        async -> Result<CuratedGraphArchivePage, CuratedGraphFailure>
    {
        guard Self.validPage(offset, limit) else { return .failure(.local(.undecodable)) }
        let answer = await graphDecoded(CuratedGraphArchivePage.self, path: "/api/graph/curated/archives",
            query: [URLQueryItem(name: "archived", value: String(archived))] + Self.pageQuery(offset, limit))
        if case .success(let page) = answer {
            guard page.offset == offset, page.limit == limit, page.total >= 0,
                  page.entities.count <= limit, Set(page.entities.map(\.id)).count == page.entities.count,
                  Self.validNext(page.nextOffset, offset, page.entities.count),
                  page.entities.allSatisfy({ row in
                      Self.opaque(row.id, maxBytes: 192) != nil && row.memoryCount >= 0
                          && (row.archivedAt != nil) == archived
                  }) else { return .failure(.local(.undecodable)) }
        }
        return answer
    }

    public func graphMemory(id: String) async -> Result<CuratedGraphMemory, CuratedGraphFailure> {
        guard let encoded = Self.opaque(id) else { return .failure(.local(.undecodable)) }
        let answer = await graphDecoded(CuratedGraphMemory.self, path: "/api/memories/\(encoded)")
        if case .success(let memory) = answer,
           memory.id != id || !memory.importance.isFinite { return .failure(.local(.undecodable)) }
        return answer
    }

    public func setEntityArchived(id: String, archived: Bool) async -> Result<Void, CuratedGraphFailure> {
        guard let encoded = Self.opaque(id, maxBytes: 192),
              let body = try? JSONEncoder().encode(EntityArchiveBody(archived: archived)) else {
            return .failure(.local(.undecodable))
        }
        let result = await graphDecoded(EntityArchiveReply.self, path: "/api/entities/\(encoded)/archive",
                                        method: "POST", body: body)
        switch result {
        case .success(let reply) where reply.id == id && reply.archived == archived: return .success(())
        case .success: return .failure(.local(.undecodable))
        case .failure(let failure): return .failure(failure)
        }
    }

    public func setProjectArchived(key: String, archived: Bool) async -> Result<Void, CuratedGraphFailure> {
        guard let encoded = ProjectKey.encoded(key) else { return .failure(.local(.undecodable)) }
        let action = archived ? "archive" : "unarchive"
        let result = await graphDecoded(ProjectArchiveReply.self, path: "/api/projects/\(encoded)/\(action)",
                                        method: "POST")
        switch result {
        case .success(let reply) where !reply.key.isEmpty && reply.state == (archived ? "archived" : "active"):
            return .success(())
        case .success: return .failure(.local(.undecodable))
        case .failure(let failure): return .failure(failure)
        }
    }

    private func graphDecoded<Value: Decodable & Sendable>(
        _ type: Value.Type, path: String, query: [URLQueryItem] = [], method: String = "GET", body: Data? = nil
    ) async -> Result<Value, CuratedGraphFailure> {
        let version = graphEndpointVersion
        let answer = await graphResponse(path: path, query: query, method: method, body: body)
        guard version == graphEndpointVersion else { return .failure(.local(.unreachable)) }
        switch answer {
        case .failure(let failure): return .failure(.local(failure))
        case .success(let reply):
            guard reply.status == 200 else { return .failure(Self.graphFailure(reply.status)) }
            guard let value = try? JSONDecoder().decode(type, from: reply.body) else {
                return .failure(.local(.undecodable))
            }
            return .success(value)
        }
    }

    private static func graphFailure(_ status: Int, overview: Bool = false) -> CuratedGraphFailure {
        switch status {
        case 401: return .local(.unauthorised)
        case 404: return overview ? .local(.earlierDaemon) : .notFound
        case 503: return .unavailable
        default: return .local(.undecodable)
        }
    }

    private static func pageQuery(_ offset: Int, _ limit: Int) -> [URLQueryItem] {
        [URLQueryItem(name: "offset", value: String(offset)), URLQueryItem(name: "limit", value: String(limit))]
    }
    private static func validPage(_ offset: Int, _ limit: Int) -> Bool {
        offset >= 0 && offset <= Int.max - 100 && (1...100).contains(limit)
    }
    private static func validNext(_ next: Int?, _ offset: Int, _ count: Int) -> Bool {
        next == nil || (count > 0 && next == offset + count)
    }
    private static func opaque(_ value: String, maxBytes: Int? = nil) -> String? {
        guard !value.isEmpty, value != ".", value != "..",
              maxBytes.map({ maximum in value.utf8.count <= maximum }) ?? true,
              value.unicodeScalars.allSatisfy({ scalar in !CharacterSet.controlCharacters.contains(scalar) })
        else { return nil }
        return value.addingPercentEncoding(withAllowedCharacters:
            CharacterSet(charactersIn: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~"))
    }
    private static func graphKey(_ key: String) -> Bool {
        guard key.utf8.count <= 192,
              key.utf8.allSatisfy({ byte in
                  (48...57).contains(byte) || (65...90).contains(byte) || (97...122).contains(byte)
                      || [45, 46, 58, 95].contains(byte)
              }) else { return false }
        let parts = key.split(separator: ":", omittingEmptySubsequences: false)
        guard parts.count >= 2 else { return false }
        if parts[0] == "project" { return parts.count == 2 && positive(parts[1]) }
        if parts[0] == "entity" { return parts.dropFirst().allSatisfy { atom in validAtom(atom) } }
        guard parts[0] == "unit", positive(parts[1]) else { return false }
        if parts.count == 4, parts[2] == "h" {
            return parts[3].utf8.count == 40 && parts[3].utf8.allSatisfy { byte in
                (48...57).contains(byte) || (97...102).contains(byte)
            }
        }
        return parts.count == 3 && parts[2].utf8.count <= 160 && validAtom(parts[2])
    }
    private static func positive(_ value: Substring) -> Bool {
        guard let parsed = Int64(value), parsed > 0 else { return false }
        return String(parsed) == value
    }
    private static func validAtom(_ atom: Substring) -> Bool {
        !atom.isEmpty && atom != "." && atom != ".."
    }
    private struct EntityArchiveBody: Encodable { let archived: Bool }
    private struct EntityArchiveReply: Decodable, Sendable { let id: String; let archived: Bool }
    private struct ProjectArchiveReply: Decodable, Sendable { let key: String; let state: String }
}
