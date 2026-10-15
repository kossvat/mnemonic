import AppKit
import MnemonicShared
import SwiftUI

/// What the board drew: the answer of the board and the live snapshot whose
/// cards it painted.
struct BoardPaint: Equatable {
    let board: ProjectBoard?
    let live: LiveSnapshot?
}

/// A point over one card. It draws in the pass of the window that draws the
/// card, again for each new board, snapshot or revision of the proof and when
/// the window shows after it was hidden, and reports the card with the revision
/// it drew for once that pass has been committed. It adds no timer, and no
/// observer that stays: the one a paint adds fires once and is gone.
struct BoardPaintProbe: NSViewRepresentable {
    let paint: BoardPaint
    let card: String
    let revision: UInt64
    let visible: Bool
    let onDraw: (BoardPaint, String, UInt64) -> Void

    func makeNSView(context: Context) -> BoardPaintProbeView {
        let view = BoardPaintProbeView()
        view.update(paint: paint, card: card, revision: revision, visible: visible, onDraw: onDraw)
        return view
    }

    func updateNSView(_ view: BoardPaintProbeView, context: Context) {
        view.update(paint: paint, card: card, revision: revision, visible: visible, onDraw: onDraw)
    }
}

@MainActor
final class BoardPaintProbeView: NSView {
    private struct Shown: Equatable {
        let paint: BoardPaint
        let card: String
        let revision: UInt64
    }

    private var shown: Shown?
    private(set) var visible = false
    /// How many times an update asked for a paint; tests read it.
    private(set) var paintRequests = 0
    private var onDraw: ((BoardPaint, String, UInt64) -> Void)?

    override var isOpaque: Bool { false }

    func update(paint: BoardPaint, card: String, revision: UInt64, visible now: Bool,
                onDraw: @escaping (BoardPaint, String, UInt64) -> Void) {
        self.onDraw = onDraw
        let reopened = now && !visible
        visible = now
        let next = Shown(paint: paint, card: card, revision: revision)
        guard shown != next || reopened else { return }
        shown = next
        paintRequests += 1
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        guard visible, let shown, let onDraw else { return }
        // Core Animation commits a pass in an observer of the main run loop.
        // This one has the last order of all, so it runs after that commit,
        // in this pass or the next, and removes itself once it has run. A paint
        // made inside that commit misses the pass it is in, so the loop is woken
        // to turn once more instead of sleeping until some other input.
        let observer = CFRunLoopObserverCreateWithHandler(
            kCFAllocatorDefault, CFRunLoopActivity.beforeWaiting.rawValue | CFRunLoopActivity.exit.rawValue,
            false, CFIndex.max
        ) { _, _ in
            MainActor.assumeIsolated { onDraw(shown.paint, shown.card, shown.revision) }
        }
        CFRunLoopAddObserver(CFRunLoopGetMain(), observer, .commonModes)
        CFRunLoopWakeUp(CFRunLoopGetMain())
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if window != nil { needsDisplay = true }
    }
}
