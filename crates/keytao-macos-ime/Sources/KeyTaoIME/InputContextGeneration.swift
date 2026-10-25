/// IMKit client calls can synchronously cancel, reload or switch the active
/// context. A callback may finish, but its caller must not resume stale work.
final class KeyTaoInputContextGeneration {
    private(set) var current: UInt64 = 0

    @discardableResult
    func invalidate() -> UInt64 {
        current &+= 1
        return current
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
