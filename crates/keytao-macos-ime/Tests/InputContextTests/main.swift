import Foundation

var failures: [String] = []

func expect<T: Equatable>(_ actual: T, _ expected: T, _ message: String) {
    if actual != expected {
        failures.append("\(message): expected \(expected), got \(actual)")
    }
}

let lifetime = KeyTaoInputContextGeneration()
var effects: [String] = []
let first = lifetime.current
if lifetime.perform(first, { effects.append("commit") }) {
    effects.append("preedit")
}
expect(effects, ["commit", "preedit"], "a stable host callback preserves commit/preedit order")

effects = []
let beforeCancel = lifetime.current
if lifetime.perform(beforeCancel, {
    effects.append("host callback")
    lifetime.invalidate()
}) {
    effects.append("stale preedit")
}
expect(effects, ["host callback"], "a reentrant cancel prevents the outer apply from restoring preedit")

let beforeFocusChange = lifetime.current
lifetime.invalidate()
expect(
    lifetime.perform(beforeFocusChange, { effects.append("old candidate click") }),
    false,
    "a callback captured in the old context is ignored before reaching the host"
)
expect(effects, ["host callback"], "the old candidate callback has no side effects")

effects = []
let outer = lifetime.current
if lifetime.perform(outer, {
    effects.append("old host")
    let current = lifetime.invalidate()
    _ = lifetime.perform(current, { effects.append("new context preedit") })
}) {
    effects.append("old context panel")
}
expect(
    effects,
    ["old host", "new context preedit"],
    "a reentrant new context survives while the obsolete outer panel update is stopped"
)

if failures.isEmpty {
    print("input context tests passed")
} else {
    for failure in failures {
        FileHandle.standardError.write(Data("FAIL \(failure)\n".utf8))
    }
    exit(1)
}
