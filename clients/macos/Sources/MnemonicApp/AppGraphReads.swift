import Foundation
import MnemonicShared

extension AppGraphController {
    func refresh() async {
        guard visible, !mutationInFlight, !running.contains(visit) else { return }
        let started = visit
        running.insert(started)
        clearSelection()
        loading = true
        changed()
        defer {
            running.remove(started)
            invalidated.remove(started)
            if visit == started { loading = false; changed() }
        }
        repeat {
            invalidated.remove(started)
            let focus = expandedKey
            var answer = await requests.snapshot(focus)
            guard current(started), !mutationInFlight else { return }
            if invalidated.contains(started) { continue }
            if case .failure(.notFound) = answer, focus != nil {
                expandedKey = nil
                clearSnapshot()
                clearSelection()
                changed()
                answer = await requests.snapshot(nil)
                guard current(started), !mutationInFlight else { return }
                if invalidated.contains(started) { continue }
            }
            switch answer {
            case .success(let value):
                snapshot = value
                failure = nil
                if let selected = selectedKey, !represented(selected) { clearSelection() }
            case .failure(let reason):
                clearSnapshot()
                failure = reason
                clearSelection()
            }
            changed()
        } while current(started) && invalidated.contains(started)
    }

    func expand(_ key: String?) async {
        guard visible, !mutationInFlight else { return }
        if let key, !represented(key) { return }
        visit &+= 1
        expandedKey = key
        clearSnapshot()
        failure = nil
        loading = false
        clearSelection()
        invalidateLists(clear: false)
        changed()
        await refresh()
    }

    func selectNode(_ key: String?) async {
        guard visible, !mutationInFlight else { return }
        clearSelection()
        if let key, represented(key) { selectedKey = key }
        changed()
        guard let key = selectedKey, let slug = projectSlug(key) else { return }
        let started = visit, selected = selection
        projectLoading = true
        changed()
        let answer = await requests.project(slug)
        guard selectionCurrent(started, selected), selectedKey == key else { return }
        projectLoading = false
        switch answer {
        case .success(let detail): project = detail; projectFailure = nil
        case .failure(let reason): project = nil; projectFailure = reason
        }
        changed()
    }

    func projectSlug(_ key: String) -> String? {
        snapshot?.nodes.first(where: { node in node.key == key })?.projectSlug
            ?? snapshot?.shelf.first(where: { item in item.key == key })?.projectSlug
    }

    func represented(_ key: String) -> Bool {
        snapshot?.nodes.contains(where: { node in node.key == key }) == true
            || snapshot?.shelf.contains(where: { item in item.key == key }) == true
            || outside.entities.contains(where: { item in "entity:" + item.id == key })
            || history.entities.contains(where: { item in "entity:" + item.id == key })
    }
}
