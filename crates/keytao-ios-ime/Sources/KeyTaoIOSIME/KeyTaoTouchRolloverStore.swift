import Foundation

struct KeyTaoTouchRolloverStateMachine<State> {
    private var storage: [ObjectIdentifier: State] = [:]

    var isEmpty: Bool { storage.isEmpty }
    var values: [State] { Array(storage.values) }

    subscript(identifier: ObjectIdentifier) -> State? {
        get { storage[identifier] }
        set { storage[identifier] = newValue }
    }

    mutating func begin(_ state: State, for identifier: ObjectIdentifier) {
        storage[identifier] = state
    }

    mutating func move(_ identifier: ObjectIdentifier, update: (inout State) -> Void) {
        guard var state = storage[identifier] else {
            return
        }
        update(&state)
        storage[identifier] = state
    }

    mutating func finish<Output>(
        _ identifier: ObjectIdentifier,
        resolving output: (State) -> Output?
    ) -> [Output] {
        guard let state = storage.removeValue(forKey: identifier), let emitted = output(state) else {
            return []
        }
        return [emitted]
    }

    mutating func cancel(_ identifier: ObjectIdentifier) -> State? {
        storage.removeValue(forKey: identifier)
    }

    mutating func removeAll() -> [State] {
        let removed = values
        storage.removeAll()
        return removed
    }
}

struct KeyTaoChordKeyEvent: Equatable {
    let key: UInt32
    let released: Bool

    var modifiers: UInt32 { released ? 1 << 30 : 0 }
}

struct KeyTaoChordTouchStateMachine<Identifier: Hashable> {
    private var heldKeys: [Identifier: UInt32] = [:]

    var identifiers: [Identifier] { Array(heldKeys.keys) }

    func contains(_ identifier: Identifier) -> Bool { heldKeys[identifier] != nil }

    mutating func begin(
        _ identifier: Identifier,
        text: String?,
        enabled: Bool
    ) -> KeyTaoChordKeyEvent? {
        guard enabled, heldKeys[identifier] == nil,
              let text, text.utf8.count == 1, let key = text.utf8.first, (0x20...0x7e).contains(key) else {
            return nil
        }
        heldKeys[identifier] = UInt32(key)
        return KeyTaoChordKeyEvent(key: UInt32(key), released: false)
    }

    mutating func end(_ identifier: Identifier) -> KeyTaoChordKeyEvent? {
        heldKeys.removeValue(forKey: identifier).map { KeyTaoChordKeyEvent(key: $0, released: true) }
    }

    mutating func releaseAll() -> [KeyTaoChordKeyEvent] {
        let events = heldKeys.values.map { KeyTaoChordKeyEvent(key: $0, released: true) }
        heldKeys.removeAll()
        return events
    }
}
