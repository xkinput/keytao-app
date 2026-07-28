// Checks the keytao-core-ffi contract the macOS frontend depends on, against a
// real librime and a real deployed user directory. Run it with ../smoke.sh.
//
// This is a manual tool, the Swift counterpart of keytao-core's
// examples/*_smoke.rs: it is not part of any build or CI path.

import Foundation
import CKeytaoCore

var failures = 0

func expect(_ condition: Bool, _ message: String) {
    if condition {
        print("ok   - \(message)")
    } else {
        print("FAIL - \(message)")
        failures += 1
    }
}

guard CommandLine.arguments.count > 2 else {
    print("usage: keytao-macos-smoke <user_data_dir> <shared_data_dir>")
    exit(2)
}
let userDir = CommandLine.arguments[1]
let sharedDir = CommandLine.arguments[2]

// Text to keysym is keytao-core's, so a full width parenthesis never lands in
// the X11 function key block.
let fullWidthParen = "（".withCString { keytao_text_to_keysym($0) }
expect(
    fullWidthParen == 0x0100_FF08,
    "（ maps to 0x0100FF08, not XK_BackSpace (got \(String(fullWidthParen, radix: 16)))"
)
expect("a".withCString { keytao_text_to_keysym($0) } == 0x61, "a maps to 0x61")
expect("ab".withCString { keytao_text_to_keysym($0) } == 0, "multi-character text has no keysym")

expect(keytao_key_policy_is_enter(0xff0d), "XK_Return is an enter key")
expect(keytao_key_policy_is_enter(0xff8d), "XK_KP_Enter is an enter key")
expect(!keytao_key_policy_is_enter(0x61), "a is not an enter key")

// ImeState offsets are Unicode scalars; IMKit counts UTF-16.
let surrogate = "a𝄞b"
expect(surrogate.keytaoUtf16Offset(fromCharacterOffset: 0) == 0, "char 0 -> utf16 0")
expect(surrogate.keytaoUtf16Offset(fromCharacterOffset: 1) == 1, "char 1 -> utf16 1")
expect(surrogate.keytaoUtf16Offset(fromCharacterOffset: 2) == 3, "char 2 -> utf16 3 (surrogate pair)")
expect(surrogate.keytaoUtf16Offset(fromCharacterOffset: 3) == 4, "char 3 -> utf16 4")

let stampPath = userDir.withCString { ptr -> String? in
    guard let raw = keytao_reload_stamp_path_at(ptr) else { return nil }
    defer { keytao_free_string(raw) }
    return String(cString: raw)
}
expect(
    stampPath == (userDir as NSString).appendingPathComponent("keytao-ime.reload"),
    "stamp path is <user_dir>/keytao-ime.reload (got \(stampPath ?? "nil"))"
)

keytao_set_ui_capabilities(true, true, true, true, true, false)
"light".withCString { keytao_set_system_color_scheme($0) }

guard keytao_init(userDir, sharedDir) else {
    print("FAIL - keytao_init(\(userDir), \(sharedDir))")
    exit(1)
}
guard let session = keytao_create_session() else {
    print("FAIL - keytao_create_session")
    exit(1)
}
defer { keytao_destroy_session(session) }

func process(_ keyval: UInt32) -> KeyTaoImeState? {
    KeyTaoImeState.consuming(keytao_session_process_key_json(session, keyval, 0))
}

func compose(_ code: String) -> KeyTaoImeState {
    var state = KeyTaoImeState.empty
    for scalar in code.unicodeScalars {
        guard let next = process(scalar.value) else {
            print("FAIL - process_key_json returned nil")
            exit(1)
        }
        state = next
    }
    return state
}

_ = KeyTaoImeState.consuming(keytao_session_clear_composition_json(session))
let state = compose("ui")

print("preedit=\(state.preedit) cursor=\(state.cursor) sel=\(state.selStart)..\(state.selEnd)")
print("orientation=\(state.candidatePanel.orientation) candidates=\(state.candidatePanel.candidates.count)")
for candidate in state.candidatePanel.candidates.prefix(3) {
    print("  [\(candidate.index)] label=\(candidate.label) selected=\(candidate.selected)")
}

expect(state.accepted, "librime accepted the keys")
expect(!state.preedit.isEmpty, "preedit is not empty")
expect(!state.candidatePanel.candidates.isEmpty, "the candidate panel model carries candidates")
expect(
    state.candidatePanel.orientation == .vertical,
    "declaring supports_vertical gives macOS a vertical panel"
)
expect(state.candidatePanel.candidates.first?.selected == true, "the first candidate is highlighted")
expect(state.candidatePanel.candidates.allSatisfy { !$0.label.isEmpty }, "every candidate has a label")
expect(
    !state.candidatePanel.navigation.canGoPrevious,
    "the first candidate page reports no previous page"
)
expect(!state.modeHint.text.isEmpty, "the mode hint carries text (\(state.modeHint.text))")
expect(state.hasComposition, "hasComposition follows preedit/candidates")
expect(state.cursor <= state.preedit.unicodeScalars.count, "cursor is a scalar offset inside preedit")
expect(state.selEnd <= state.preedit.unicodeScalars.count, "selEnd is a scalar offset inside preedit")

guard let entered = KeyTaoImeState.consuming(keytao_session_process_enter_json(session)) else {
    print("FAIL - process_enter_json returned nil")
    exit(1)
}
expect(!entered.committed.isEmpty, "Enter commits something (\(entered.committed))")
expect(!entered.hasComposition, "Enter ends the composition")

_ = compose("ui")
guard let cleared = KeyTaoImeState.consuming(keytao_session_clear_composition_json(session)) else {
    print("FAIL - clear_composition_json returned nil")
    exit(1)
}
expect(cleared.committed.isEmpty, "clear_composition commits nothing")
expect(!cleared.hasComposition, "clear_composition ends the composition")

_ = compose("ui")
guard let committed = KeyTaoImeState.consuming(keytao_session_commit_composition_json(session)) else {
    print("FAIL - commit_composition_json returned nil")
    exit(1)
}
expect(!committed.committed.isEmpty, "commit_composition hands text back (\(committed.committed))")
expect(!committed.hasComposition, "commit_composition ends the composition")

_ = compose("ui")
expect(!keytao_key_policy_should_bypass(session, 0xff50, 0), "Home is not bypassed while composing")
_ = KeyTaoImeState.consuming(keytao_session_clear_composition_json(session))
expect(keytao_key_policy_should_bypass(session, 0xff50, 0), "Home is bypassed with no composition")
expect(keytao_key_policy_should_bypass(session, 0x61, 1 << 26), "Super chords are bypassed")
expect(!keytao_key_policy_should_bypass(session, 0x61, 0x0004), "Ctrl chords reach librime")

print(failures == 0 ? "\nall macOS FFI smoke checks passed" : "\n\(failures) check(s) failed")
exit(failures == 0 ? 0 : 1)
