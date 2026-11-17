import Foundation

enum KeyTaoChordKeyPolicy {
    static func shouldForwardRelease(
        chordComposer: Bool,
        asciiMode: Bool,
        keyval: UInt32,
        modifiers _: UInt32
    ) -> Bool {
        return chordComposer && !asciiMode
            && (0x20...0x7e).contains(keyval)
    }
}

struct KeyTaoChordReleaseTracker {
    private struct Press {
        let keyCode: UInt16
        let keyval: UInt32
        var observedPressed: Bool
    }

    private var pending: [Press] = []
    private var lastPress: TimeInterval = 0

    var isEmpty: Bool { pending.isEmpty }

    mutating func record(
        keyCode: UInt16, keyval: UInt32, chordComposer: Bool,
        asciiMode: Bool, accepted: Bool, now: TimeInterval, isPressed: Bool
    ) {
        guard accepted, KeyTaoChordKeyPolicy.shouldForwardRelease(
            chordComposer: chordComposer, asciiMode: asciiMode,
            keyval: keyval, modifiers: 0
        ) else { return }
        lastPress = now
        guard !pending.contains(where: { $0.keyCode == keyCode }) else { return }
        pending.append(Press(keyCode: keyCode, keyval: keyval, observedPressed: isPressed))
    }

    mutating func takeRelease(keyCode: UInt16) -> UInt32? {
        guard let index = pending.firstIndex(where: { $0.keyCode == keyCode }) else { return nil }
        return pending.remove(at: index).keyval
    }

    mutating func poll(now: TimeInterval, keyState: (UInt16) -> Bool?) -> [UInt32] {
        var released: [UInt32] = []
        var held: [Press] = []
        for var press in pending {
            let pressed = keyState(press.keyCode)
            if pressed == true { press.observedPressed = true }
            // CGEventSource can report all-up when state is unavailable.
            if (pressed == false && press.observedPressed)
                || (pressed != true && now - lastPress >= 0.080) {
                released.append(press.keyval)
            } else {
                held.append(press)
            }
        }
        pending = held
        return released
    }

    mutating func clear() {
        pending.removeAll()
    }
}
