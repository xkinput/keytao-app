import Foundation
#if canImport(CoreGraphics)
import CoreGraphics
#endif

// Pure touch-policy coverage; UIKit dispatch is typechecked separately.
var failures: [String] = []
var assertions = 0

func expect(_ condition: Bool, _ message: String, line: Int = #line) {
    assertions += 1
    if !condition {
        failures.append("line \(line): \(message)")
    }
}

let firstTouch = NSObject()
let secondTouch = NSObject()
let firstIdentifier = ObjectIdentifier(firstTouch)
let secondIdentifier = ObjectIdentifier(secondTouch)
struct TestTouchState {
    var key: String
    var currentLocation: Int
}

var activeTouches = KeyTaoTouchRolloverStateMachine<TestTouchState>()
var output: [String] = []

activeTouches.begin(TestTouchState(key: "A", currentLocation: 0), for: firstIdentifier)
activeTouches.begin(TestTouchState(key: "B", currentLocation: 0), for: secondIdentifier)
activeTouches.move(secondIdentifier) { $0.currentLocation = 1 }
output += activeTouches.finish(firstIdentifier) { $0.key }
output += activeTouches.finish(secondIdentifier) { $0.key }

expect(output == ["A", "B"], "A down, B down, A up, B up must commit A then B")
expect(activeTouches.isEmpty, "all ended touches must be removed independently")

func chordEvents(_ sequence: [(Int, String?)]) -> [KeyTaoChordKeyEvent] {
    var touches = KeyTaoChordTouchStateMachine<Int>()
    return sequence.compactMap { identifier, text in
        if let text {
            return touches.begin(identifier, text: text, enabled: true)
        }
        return touches.end(identifier)
    }
}

func chordEvent(_ key: UInt32, released: Bool = false) -> KeyTaoChordKeyEvent {
    KeyTaoChordKeyEvent(key: key, released: released)
}

expect(
    chordEvents([(0, "d"), (1, "k"), (0, nil), (1, nil)]) == [
        chordEvent(100), chordEvent(107), chordEvent(100, released: true), chordEvent(107, released: true),
    ],
    "overlapping pointers must send both presses before either release"
)
expect(
    chordEvents([(0, "d"), (1, "f"), (1, nil), (0, nil)]) == [
        chordEvent(100), chordEvent(102), chordEvent(102, released: true), chordEvent(100, released: true),
    ],
    "release order must follow the fingers, not press order"
)
expect(
    chordEvents([(0, "d"), (1, "j"), (2, "k"), (0, nil), (1, nil), (2, nil)]) == [
        chordEvent(100), chordEvent(106), chordEvent(107), chordEvent(100, released: true),
        chordEvent(106, released: true), chordEvent(107, released: true),
    ],
    "three simultaneous fingers must press every key before releasing"
)
expect(
    chordEvents([(0, "d"), (1, "j"), (0, nil), (2, "k"), (1, nil), (2, nil)]) == [
        chordEvent(100), chordEvent(106), chordEvent(100, released: true), chordEvent(107),
        chordEvent(106, released: true), chordEvent(107, released: true),
    ],
    "three-key rollover must retain j while d lifts and k presses"
)

var chordTouches = KeyTaoChordTouchStateMachine<Int>()
expect(chordTouches.begin(0, text: "d", enabled: true) == chordEvent(100), "touch down must press immediately")
expect(chordTouches.contains(0), "held chord touches must bypass move and long-press handling")
expect(chordTouches.end(0) == chordEvent(100, released: true), "cancel must release the original key")
expect(chordTouches.end(0) == nil, "cancel followed by touch up must not release twice")
expect(!chordTouches.contains(0), "cancel must remove the held touch")

_ = chordTouches.begin(0, text: "d", enabled: true)
_ = chordTouches.begin(1, text: "k", enabled: true)
expect(Set(chordTouches.identifiers) == [0, 1], "context resets must identify only held chord touches")
expect(chordTouches.end(0) == chordEvent(100, released: true), "one lift must leave the other pointer held")
expect(chordTouches.releaseAll() == [chordEvent(107, released: true)], "hide must release remaining pointers")
expect(chordTouches.releaseAll().isEmpty, "hide must drain held keys before callbacks can reenter")
expect(chordTouches.end(1) == nil, "touch up after hide must not repeat a release")

_ = chordTouches.begin(0, text: "d", enabled: true)
_ = chordTouches.begin(1, text: "k", enabled: true)
expect(
    chordTouches.releaseAll().sorted { $0.key < $1.key } == [chordEvent(100, released: true), chordEvent(107, released: true)],
    "hide must release every still-held key"
)
expect(chordTouches.begin(0, text: "d", enabled: false) == nil, "non-chord and ASCII modes must retain tap behavior")
expect(chordTouches.end(0) == nil, "ordinary tap must not synthesize a release")
for text in ["", "dk", "中", "é", "\n"] {
    expect(chordTouches.begin(0, text: text, enabled: true) == nil, "only one ASCII character can start a chord touch")
}
_ = chordTouches.begin(0, text: "d", enabled: true)
expect(chordTouches.begin(0, text: "k", enabled: true) == nil, "a tracked pointer cannot be pressed twice")
expect(chordTouches.begin(1, text: "k", enabled: false) == nil, "a later mode change must block new chord touches")
expect(chordTouches.end(0) == chordEvent(100, released: true), "mode changes must still release the originally pressed key")
expect(chordEvent(100).modifiers == 0, "press modifiers must be unchanged")
expect(chordEvent(100, released: true).modifiers == 1 << 30, "release modifiers must preserve RIME_RELEASE_MASK")

expect(KeyTaoImeState.empty.chordComposer == false, "default state must preserve ordinary tap behavior")
expect(KeyTaoImeState.decode(json: "{}")?.chordComposer == false, "older state JSON must default chord mode off")
expect(KeyTaoImeState.decode(json: #"{"chordComposer":true}"#)?.chordComposer == true, "state JSON must enable chord mode")
expect(KeyTaoImeState.decode(json: #"{"chordComposer":false}"#)?.chordComposer == false, "state JSON must disable chord mode")

struct TouchKeySpec {
    var command: KeyTaoKeyCommand
    var stack: [Int]? = nil
}
struct KeyRect {
    var spec: TouchKeySpec
    var rect = CGRect(x: 0, y: 0, width: 40, height: 40)
}
struct ActiveTouch {
    var key: KeyRect
    var keyIndex: Int
    var originKey: KeyRect
    var originKeyIndex: Int
    var touchStart: CGPoint
    var currentTouchPoint: CGPoint
    var touchStartUptime: TimeInterval
    var longPressConsumed: Bool
    var backspaceGestureUnits: Int
    var pressedStackIndex: Int?
    var cursorGesture: KeyTaoCursorGestureTracker?
    var backspaceGestureConsumed = false
}
enum TouchNavigationDestination { case reset(String?) }
final class TouchDelegate {
    var events: [KeyTaoChordKeyEvent] = []
    func keyboardViewWillBeginChord(_ view: KeyTaoIOSKeyboardView) -> Bool { true }
    func keyboardView(_ view: KeyTaoIOSKeyboardView, didTriggerChordKey event: KeyTaoChordKeyEvent) { events.append(event) }
    func keyboardView(_ view: KeyTaoIOSKeyboardView, didTrigger command: KeyTaoKeyCommand) {}
}
final class KeyTaoIOSKeyboardView {
    var state = KeyTaoImeState.empty
    var shiftState = KeyTaoShiftState.off
    var hostTraits = ChordGateHostTraits()
    var isEmojiSearching = false
    var activeTouches = KeyTaoTouchRolloverStateMachine<ActiveTouch>()
    var chordTouches = KeyTaoChordTouchStateMachine<ObjectIdentifier>()
    var pointerSlotsByTouchIdentifier: [ObjectIdentifier: Int] = [:]
    var candidateGestureTouchIdentifier: ObjectIdentifier?
    var longPressPending: Set<ObjectIdentifier> = []
    var delegate: TouchDelegate? = TouchDelegate()
    var pressedToolbar: Int?
    var hasActiveComposition = false
    func invalidateLayoutAndDisplay() {}
    func update(state: KeyTaoImeState) { self.state = state }
    func resolveCommand(_ key: TouchKeySpec, deltaY: CGFloat, rect: CGRect?, releaseY: CGFloat?) -> KeyTaoKeyCommand { applyShift(key.command) }
    func actionForMode(_ key: TouchKeySpec) -> KeyTaoKeyCommand { key.command }
    func isBackspaceKey(_ key: TouchKeySpec) -> Bool { false }
    func isSpaceKey(_ key: TouchKeySpec) -> Bool { false }
    func usesSelectionBackspaceGesture() -> Bool { false }
    func stackIndex(in rect: CGRect, count: Int, y: CGFloat) -> Int { 0 }
    func animateKeyPress(keyIndex: Int, pressed: Bool) {}
    func performPressFeedback() {}
    func backspaceGestureCommand(_ action: String) -> KeyTaoKeyCommand { .input(action) }
    func scheduleBackspaceRepeat(for identifier: ObjectIdentifier) {}
    func scheduleLongPressIfNeeded(for identifier: ObjectIdentifier) { longPressPending.insert(identifier) }
    func stopLongPressAndRepeat(for identifier: ObjectIdentifier) { longPressPending.remove(identifier) }
    func resetKeyboardScroll() {}
    func clearActiveTouches() {
        _ = activeTouches.removeAll()
        _ = chordTouches.releaseAll()
        pointerSlotsByTouchIdentifier.removeAll()
        candidateGestureTouchIdentifier = nil
        longPressPending.removeAll()
    }
    func lift(_ identifier: ObjectIdentifier) -> String? {
        guard let touch = activeTouches.finish(identifier, resolving: { $0 }).first else { return nil }
        if let event = chordTouches.end(identifier) {
            delegate?.keyboardView(self, didTriggerChordKey: event)
            return nil
        }
        let command = applyShift(touch.key.spec.command)
        clearOneShotShift(after: command)
        return command.value
    }
}
struct ContextEngine {
    var nativeReady = true
    func reset() -> KeyTaoImeState { .empty }
}
struct ContextResetTracker { mutating func reset() {} }
struct KeyTaoHostTraits {
    var forcedLayer: String? = nil
    init(proxy: Int) {}
}
final class ContextResetHarness {
    var keyboardView: KeyTaoIOSKeyboardView?
    var resettingInputContext = false
    var inputContextGeneration: UInt64 = 0
    var inputDocumentIdentifier: UUID?
    var queuedCommands: [Int] = []
    var backspaceRestoreStack: [Int] = []
    var doubleSpacePeriodTracker = ContextResetTracker()
    var lastCommittedText: String?
    var textDocumentProxy = 0
    var engine = ContextEngine()
    var currentState = KeyTaoImeState.empty
    func releaseMemoryIfNeeded() {}
    func currentDocumentIdentifier() -> UUID? { nil }
    func cancelBackspaceSelection() {}
    func resetClipboardState() {}
    func clearHostMarkedText() {}
}

for resetNavigation in [false, true] {
    let view = KeyTaoIOSKeyboardView()
    let reset = ContextResetHarness()
    reset.keyboardView = view
    view.beginKeyTouch(identifier: firstIdentifier, key: KeyRect(spec: TouchKeySpec(command: .input("a"))), keyIndex: 0, point: .zero)
    view.pointerSlotsByTouchIdentifier[firstIdentifier] = 0
    view.candidateGestureTouchIdentifier = secondIdentifier
    reset.resetInputContextCaches(resetNavigation: resetNavigation)
    expect(view.activeTouches[firstIdentifier] != nil, "host change must preserve a held ordinary key, including document changes")
    expect(view.longPressPending.contains(firstIdentifier), "host change must preserve the ordinary key's long-press timer")
    expect(view.pointerSlotsByTouchIdentifier[firstIdentifier] == 0, "host change must preserve ordinary bounce pointer slots")
    expect(view.candidateGestureTouchIdentifier == secondIdentifier, "host change must preserve candidate gesture ownership")
    expect(view.lift(firstIdentifier) == "a", "ordinary letter held across a host change must still commit on lift")
}

let resetChordView = KeyTaoIOSKeyboardView()
let resetChord = ContextResetHarness()
resetChord.keyboardView = resetChordView
resetChordView.state.chordComposer = true
resetChordView.beginKeyTouch(identifier: firstIdentifier, key: KeyRect(spec: TouchKeySpec(command: .input("d"))), keyIndex: 0, point: .zero)
resetChord.resetInputContextCaches(resetNavigation: true)
expect(resetChordView.delegate?.events == [chordEvent(100), chordEvent(100, released: true)], "context reset must still release every held chord key")
expect(resetChordView.lift(firstIdentifier) == nil, "context reset must not emit a cancelled chord as a tap")

for type in [KeyTaoCommandType.input, KeyTaoCommandType.rimeInput] {
    for shift in [KeyTaoShiftState.once, .locked] {
        let view = KeyTaoIOSKeyboardView()
        view.state.chordComposer = true
        view.shiftState = shift
        let key = KeyRect(spec: TouchKeySpec(command: KeyTaoKeyCommand(type: type, value: "d", fallbackValue: nil)))
        view.beginKeyTouch(identifier: firstIdentifier, key: key, keyIndex: 0, point: .zero)
        expect(view.delegate?.events.isEmpty == true, "shifted letters must use the normal path because chord_composer rejects uppercase")
        expect(view.lift(firstIdentifier) == "D", "shifted chord-schema letters must preserve uppercase normal input")
        expect(view.shiftState == (shift == .once ? .off : .locked), "one-shot shift must clear and caps lock must remain")
        if shift == .once {
            view.beginKeyTouch(identifier: secondIdentifier, key: key, keyIndex: 0, point: .zero)
            expect(view.delegate?.events == [chordEvent(100)], "the letter after one-shot shift must resume lowercase chord input")
            _ = view.lift(secondIdentifier)
            expect(view.delegate?.events.last == chordEvent(100, released: true), "the resumed lowercase chord must release normally")
        }
    }
}

let shiftFallbackView = KeyTaoIOSKeyboardView()
shiftFallbackView.state.chordComposer = true
shiftFallbackView.shiftState = .once
let rimeLetter = KeyTaoKeyCommand(type: KeyTaoCommandType.rimeInput, value: "d", fallbackValue: "d")
expect(shiftFallbackView.applyShift(rimeLetter).fallbackValue == "D", "rejected shifted Rime letters must preserve uppercase fallback")
let rimeMappedLetter = KeyTaoKeyCommand(type: KeyTaoCommandType.rimeInput, value: "d", fallbackValue: "中")
expect(shiftFallbackView.applyShift(rimeMappedLetter).fallbackValue == "中", "Shift must preserve a declared non-Latin fallback")
shiftFallbackView.state.chordComposer = false
expect(shiftFallbackView.applyShift(rimeLetter) == rimeLetter, "non-chord Rime-input Shift behavior must remain unchanged")
shiftFallbackView.clearOneShotShift(after: rimeLetter)
expect(shiftFallbackView.shiftState == .once, "non-chord Rime-input commands must retain the existing Shift policy")

struct ChordGateEngine { var nativeReady = true }
struct ChordGateHostTraits { var bypassesRime = false }
final class ChordGateHarness {
    var inputAvailable = true
    var engine = ChordGateEngine()
    var runtimeStarting = false
    var currentState = KeyTaoImeState.empty
    var hostTraits = ChordGateHostTraits()
    var onRefresh: () -> Void = {}
    func refreshInputDocumentIfNeeded() -> Bool {
        onRefresh()
        return false
    }
}

let gate = ChordGateHarness()
gate.currentState.chordComposer = true
_ = chordTouches.begin(0, text: "d", enabled: true)
gate.onRefresh = { _ = chordTouches.releaseAll() }
expect(gate.keyboardViewWillBeginChord(KeyTaoIOSKeyboardView()), "a refreshed ready document must accept its first down")
expect(!chordTouches.contains(0), "refresh must release the old document's held keys before the new down")
expect(chordTouches.begin(1, text: "k", enabled: true) == chordEvent(107), "the new document's first press must survive refresh")
expect(chordTouches.end(1) == chordEvent(107, released: true), "the new document's first press must still have a release")
gate.onRefresh = {}
gate.inputAvailable = false
expect(!gate.keyboardViewWillBeginChord(KeyTaoIOSKeyboardView()), "unavailable input must retain the ordinary tap recovery path")
gate.inputAvailable = true
gate.engine.nativeReady = false
expect(!gate.keyboardViewWillBeginChord(KeyTaoIOSKeyboardView()), "a reclaimed runtime must not receive a chord press")
gate.engine.nativeReady = true
gate.runtimeStarting = true
expect(!gate.keyboardViewWillBeginChord(KeyTaoIOSKeyboardView()), "startup must retain ordinary tap queuing")
gate.runtimeStarting = false
gate.currentState.asciiMode = true
expect(!gate.keyboardViewWillBeginChord(KeyTaoIOSKeyboardView()), "refreshed ASCII mode must retain ordinary taps")
gate.currentState.asciiMode = false
gate.currentState.chordComposer = false
expect(!gate.keyboardViewWillBeginChord(KeyTaoIOSKeyboardView()), "a refreshed non-chord schema must retain ordinary taps")
gate.currentState.chordComposer = true
gate.hostTraits.bypassesRime = true
expect(!gate.keyboardViewWillBeginChord(KeyTaoIOSKeyboardView()), "a refreshed direct-input host must retain ordinary taps")

if failures.isEmpty {
    print("touch rollover tests passed (\(assertions) assertions)")
} else {
    for failure in failures {
        FileHandle.standardError.write(Data("FAIL \(failure)\n".utf8))
    }
    exit(1)
}
