import AppKit

/// Detached views discard display invalidation. An unshown window retains the
/// real AppKit dirty region without creating visible-window budget evidence.
@MainActor
func staticGraphTestWindow(containing view: NSView) -> NSWindow {
    _ = NSApplication.shared
    let window = NSWindow(contentRect: CGRect(origin: .zero, size: view.bounds.size),
                          styleMask: .borderless, backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = view
    window.layoutIfNeeded()
    return window
}

/// Drain AppKit and its backing layer through the real display mechanism.
/// The view dirty flag may be clear while the layer still has pending paint.
@MainActor
func staticGraphTestDisplay(_ view: NSView) {
    view.window?.layoutIfNeeded()
    view.layoutSubtreeIfNeeded()
    view.window?.displayIfNeeded()
    view.displayIfNeeded()
    view.layer?.displayIfNeeded()
    CATransaction.flush()
}
