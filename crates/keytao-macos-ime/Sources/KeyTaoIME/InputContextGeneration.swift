#if canImport(CoreGraphics)
import CoreGraphics
#else
import Foundation
#endif

/// IMKit client calls can synchronously cancel, reload or switch the active
/// context. A callback may finish, but its caller must not resume stale work.
final class KeyTaoInputContextGeneration {
    private(set) var current: UInt64 = 0
    private var lastCursorRect: CGRect?

    @discardableResult
    func invalidate() -> UInt64 {
        current &+= 1
        clearCursorRect()
        return current
    }

    func clearCursorRect() {
        lastCursorRect = nil
    }

    func prepareForCommit(hasComposition: Bool) {
        if !hasComposition { clearCursorRect() }
    }

    func cursorRect(isComposing: Bool, refreshAnchor: Bool = false, query: () -> CGRect?) -> CGRect {
        let generation = current
        let rect = query()
        guard isCurrent(generation) else { return .zero }
        guard isComposing else { return rect ?? .zero }
        if let rect, refreshAnchor || lastCursorRect == nil { lastCursorRect = rect }
        return rect ?? lastCursorRect ?? .zero
    }

    func isCurrent(_ token: UInt64) -> Bool {
        token == current
    }

    @discardableResult
    func perform(_ token: UInt64, _ body: () -> Void) -> Bool {
        guard isCurrent(token) else { return false }
        body()
        return isCurrent(token)
    }
}
