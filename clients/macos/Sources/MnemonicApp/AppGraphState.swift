import Foundation
import MnemonicShared

struct GraphEdgeSelection: Equatable, Sendable {
    let sourceKey: String
    let targetKey: String
    let sourceLabel: String
    let targetLabel: String
    let kind: CuratedGraphLinkKind
    let memoryCount: UInt32
}

struct GraphEvidenceRow: Identifiable, Equatable, Sendable {
    let id: String
    let memory: CuratedGraphMemory?
    let failure: CuratedGraphFailure?
}

struct GraphArchiveListState: Equatable, Sendable {
    var entities: [CuratedGraphArchiveItem] = []
    var loading = false
    var failure: CuratedGraphFailure?
    var total: Int?
    var nextOffset: Int?
    var isLoaded = false
    var discardedCount = 0
}

enum GraphArchiveTarget: Equatable, Sendable {
    case entity(id: String, label: String)
    case project(key: String, label: String)

    var label: String {
        switch self {
        case .entity(_, let label), .project(_, let label): return label
        }
    }
}

struct GraphMutationRecovery: Equatable, Sendable {
    let target: GraphArchiveTarget
    let archived: Bool
    let confirmed: Bool
}

struct GraphRequests: Sendable {
    var snapshot: @Sendable (String?) async -> Result<CuratedGraphSnapshot, CuratedGraphFailure>
    var project: @Sendable (String) async -> Result<ProjectDetail, CuratedGraphFailure>
    var evidence: @Sendable (GraphEdgeSelection, Int, Int) async
        -> Result<CuratedGraphEvidencePage, CuratedGraphFailure>
    var memory: @Sendable (String) async -> Result<CuratedGraphMemory, CuratedGraphFailure>
    var archives: @Sendable (Bool, Int, Int) async -> Result<CuratedGraphArchivePage, CuratedGraphFailure>
    var mutate: @Sendable (GraphArchiveTarget, Bool) async -> Result<Void, CuratedGraphFailure>

    init(api: LocalAPI) {
        snapshot = { key in await api.curatedGraph(expand: key) }
        project = { key in
            let answer = await api.project(key: key)
            return answer.mapError { failure in failure == .earlierDaemon ? .notFound : .local(failure) }
        }
        evidence = { edge, offset, limit in
            await api.graphEdge(source: edge.sourceKey, target: edge.targetKey,
                                kind: edge.kind, offset: offset, limit: limit)
        }
        memory = { id in await api.graphMemory(id: id) }
        archives = { archived, offset, limit in
            await api.graphArchives(archived: archived, offset: offset, limit: limit)
        }
        mutate = { target, archived in
            switch target {
            case .entity(let id, _): return await api.setEntityArchived(id: id, archived: archived)
            case .project(let key, _): return await api.setProjectArchived(key: key, archived: archived)
            }
        }
    }

    init(
        snapshot: @escaping @Sendable (String?) async -> Result<CuratedGraphSnapshot, CuratedGraphFailure>,
        project: @escaping @Sendable (String) async -> Result<ProjectDetail, CuratedGraphFailure>,
        evidence: @escaping @Sendable (GraphEdgeSelection, Int, Int) async
            -> Result<CuratedGraphEvidencePage, CuratedGraphFailure>,
        memory: @escaping @Sendable (String) async -> Result<CuratedGraphMemory, CuratedGraphFailure>,
        archives: @escaping @Sendable (Bool, Int, Int) async -> Result<CuratedGraphArchivePage, CuratedGraphFailure>,
        mutate: @escaping @Sendable (GraphArchiveTarget, Bool) async -> Result<Void, CuratedGraphFailure>
    ) {
        self.snapshot = snapshot
        self.project = project
        self.evidence = evidence
        self.memory = memory
        self.archives = archives
        self.mutate = mutate
    }
}
