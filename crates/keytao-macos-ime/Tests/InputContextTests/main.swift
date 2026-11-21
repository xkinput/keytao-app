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

let notFoundRange = NSRange(location: NSNotFound, length: 0)
for embedded in [true, false] {
    for hidePanel in [false, true] {
        // Opening F4 menu (empty preedit, non-empty candidates) must install an
        // invisible marked range so AppKit/Chromium/Electron/WebKit suppress
        // default Up/Down/Return actions in the foreground app.
        let openMenu = KeyTaoMarkedTextPolicy.decide(
            preedit: "",
            commitPreview: "",
            hasComposition: true,
            isPackageSwitcher: true,
            embeddedComposition: embedded,
            hideCandidatePanel: hidePanel,
            hadMarkedText: false,
            selectedRange: NSRange(location: 12, length: 0)
        )
        expect(
            openMenu,
            .placeholder(replacementRange: notFoundRange),
            "F4 menu installs placeholder marked text (embedded=\(embedded), hide=\(hidePanel))"
        )

        // Navigating Up/Down keeps the placeholder marked range active.
        let navigateMenu = KeyTaoMarkedTextPolicy.decide(
            preedit: "",
            commitPreview: "",
            hasComposition: true,
            isPackageSwitcher: true,
            embeddedComposition: embedded,
            hideCandidatePanel: hidePanel,
            hadMarkedText: true
        )
        expect(
            navigateMenu,
            .placeholder(replacementRange: notFoundRange),
            "F4 menu Up/Down retains placeholder marked text (embedded=\(embedded), hide=\(hidePanel))"
        )

        // Pressing Return/Escape closes the menu and clears the placeholder.
        let closeMenu = KeyTaoMarkedTextPolicy.decide(
            preedit: "",
            commitPreview: "",
            hasComposition: false,
            isPackageSwitcher: false,
            embeddedComposition: embedded,
            hideCandidatePanel: hidePanel,
            hadMarkedText: true
        )
        expect(
            closeMenu,
            .clear,
            "closing F4 menu clears placeholder marked text (embedded=\(embedded), hide=\(hidePanel))"
        )
    }
}

// Opening a candidate-only F4 menu over a non-empty text selection collapses
// after the selection instead of replacing and erasing the selected text.
expect(
    KeyTaoMarkedTextPolicy.decide(
        preedit: "",
        commitPreview: "",
        hasComposition: true,
        isPackageSwitcher: true,
        embeddedComposition: true,
        hideCandidatePanel: false,
        hadMarkedText: false,
        selectedRange: NSRange(location: 4, length: 5)
    ),
    .placeholder(replacementRange: NSRange(location: 9, length: 0)),
    "F4 menu preserves an active document selection"
)

// Normal inline composition still uses inline text and commitPreview when hidden.
expect(
    KeyTaoMarkedTextPolicy.decide(
        preedit: "ni",
        commitPreview: "你",
        hasComposition: true,
        isPackageSwitcher: false,
        embeddedComposition: true,
        hideCandidatePanel: true,
        hadMarkedText: false
    ),
    .inline(text: "你", hiddenStyle: true),
    "hidden candidate panel still embeds commitPreview for normal typing"
)

if failures.isEmpty {
    print("input context tests passed")
} else {
    for failure in failures {
        FileHandle.standardError.write(Data("FAIL \(failure)\n".utf8))
    }
    exit(1)
}
