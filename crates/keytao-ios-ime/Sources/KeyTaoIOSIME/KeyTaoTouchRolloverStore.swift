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
