import AppKit
import Carbon
import Foundation

func forwards(
    _ keyval: UInt32 = 0x61,
    chordComposer: Bool = true,
    asciiMode: Bool = false,
    modifiers: UInt32 = 0
) -> Bool {
    KeyTaoChordKeyPolicy.shouldForwardRelease(
        chordComposer: chordComposer, asciiMode: asciiMode,
        keyval: keyval, modifiers: modifiers
    )
}

assert(forwards(), "chord schemas forward letter releases")
assert(!forwards(chordComposer: false), "ordinary schemas keep ignoring letter releases")
assert(!forwards(asciiMode: true), "ASCII mode preserves host key releases")
assert(forwards(0x20), "space can be a chording key")
assert(forwards(0x7e, modifiers: 0x0001 | 0x0002), "Shift and Caps Lock preserve printable chords")
for modifiers: UInt32 in [0x0004, 0x0008, 1 << 26, 0x0004 | 0x0008 | (1 << 26)] {
    assert(forwards(modifiers: modifiers), "shortcut modifiers must not leave chording keys pressed")
    assert(!forwards(chordComposer: false, modifiers: modifiers), "ordinary schema shortcuts keep their releases")
    assert(!forwards(asciiMode: true, modifiers: modifiers), "ASCII mode shortcuts keep their releases")
}
for keyval: UInt32 in [0, 0x1f, 0x7f, 0xff0d, 0xffe1] {
    assert(!forwards(keyval), "nonprintable releases keep their existing paths")
}
var releases = KeyTaoChordReleaseTracker()
func press(
    _ keyCode: UInt16, _ keyval: UInt32, at now: TimeInterval = 0,
    chordComposer: Bool = true, asciiMode: Bool = false,
    accepted: Bool = true, isPressed: Bool = true
) {
    releases.record(
        keyCode: keyCode, keyval: keyval, chordComposer: chordComposer,
        asciiMode: asciiMode, accepted: accepted, now: now, isPressed: isPressed
    )
}

press(2, 0x64)
press(40, 0x6b)
assert(releases.poll(now: 0.008, keyState: { _ in true }).isEmpty)
assert(releases.poll(now: 0.016, keyState: { $0 != 2 }) == [0x64])
assert(!releases.isEmpty, "the chord waits for the last physical key")
assert(releases.poll(now: 0.024, keyState: { _ in false }) == [0x6b])
assert(releases.isEmpty, "missing IMK keyUp events must not leave a chord pressed")

press(2, 0x44)
press(2, 0x64, at: 0.010)
assert(releases.takeRelease(keyCode: 2) == 0x44, "release uses the original press keysym despite modifiers/repeat")
assert(releases.isEmpty, "native keyUp removes the pending synthesized release")

press(40, 0x6b)
press(2, 0x64, at: 0.040)
assert(releases.poll(now: 0.119, keyState: { _ in nil }).isEmpty)
assert(releases.poll(now: 0.121, keyState: { _ in nil }) == [0x6b, 0x64], "unreadable state falls back in press order after the last press")
assert(releases.isEmpty)

press(2, 0x64, isPressed: false)
assert(releases.poll(now: 0.008, keyState: { _ in false }).isEmpty, "an unreadable all-up state must not split a chord immediately")
assert(releases.poll(now: 0.081, keyState: { _ in false }) == [0x64])
press(2, 0x64, isPressed: false)
assert(releases.poll(now: 0.008, keyState: { _ in true }).isEmpty)
assert(releases.poll(now: 0.016, keyState: { _ in false }) == [0x64], "polling can establish a readable pressed state")

press(2, 0x64)
assert(releases.poll(now: 1, keyState: { _ in true }).isEmpty, "readable held keys must not time out")
releases.clear()
assert(releases.isEmpty, "focus loss, commit, cancel and reload discard pending releases")
assert(releases.poll(now: 2, keyState: { _ in false }).isEmpty)

press(2, 0x64, chordComposer: false)
press(2, 0x64, asciiMode: true)
press(2, 0x64, accepted: false)
press(36, 0xff0d)
assert(releases.isEmpty, "ordinary schemas, ASCII, rejected and nonprintable presses stay unchanged")

let capsOn = KeyTaoModifierKeyPolicy.capsLockEvent(previousModifiers: 0, currentModifiers: 0x0002)
assert(capsOn?.keyval == 0xffe5 && capsOn?.modifiers == 0, "Caps Lock on sends the pre-toggle unlocked state")
let capsOff = KeyTaoModifierKeyPolicy.capsLockEvent(previousModifiers: 0x0002, currentModifiers: 0)
assert(capsOff?.keyval == 0xffe5 && capsOff?.modifiers == 0x0002, "Caps Lock off sends the pre-toggle locked state")
for otherModifiers: UInt32 in [0x0001, 0x0004, 0x0008, 1 << 26, 0x0001 | 0x0004 | 0x0008 | (1 << 26)] {
    let on = KeyTaoModifierKeyPolicy.capsLockEvent(
        previousModifiers: otherModifiers, currentModifiers: otherModifiers | 0x0002
    )
    assert(on?.modifiers == otherModifiers, "Caps Lock preserves other modifiers")
    let off = KeyTaoModifierKeyPolicy.capsLockEvent(
        previousModifiers: otherModifiers | 0x0002, currentModifiers: otherModifiers
    )
    assert(off?.modifiers == otherModifiers | 0x0002, "Caps Lock off preserves other modifiers")
}
assert(KeyTaoModifierKeyPolicy.capsLockEvent(previousModifiers: 0, currentModifiers: 0) == nil)
assert(KeyTaoModifierKeyPolicy.capsLockEvent(previousModifiers: 0x0002, currentModifiers: 0x0003) == nil,
       "unrelated modifier changes must not toggle Caps Lock again")
assert(KeyTaoModifierKeyPolicy.activationFlags([.capsLock, .option], systemCapsLock: false) == .option)
assert(KeyTaoModifierKeyPolicy.activationFlags(.option, systemCapsLock: true) == [.capsLock, .option])

var capsFlags: NSEvent.ModifierFlags = []
for _ in 0..<2 {
    guard let restored = KeyTaoModifierKeyPolicy.inputModeCapsLockFlags(
        previousFlags: capsFlags, currentFlags: .capsLock
    ) else { fatalError("each standalone Caps Lock press switches the input mode") }
    assert(restored.isEmpty, "a restored lock allows another press without an intervening flagsChanged event")
    capsFlags = restored
    assert(KeyTaoModifierKeyPolicy.inputModeCapsLockFlags(previousFlags: capsFlags, currentFlags: []) == nil)
}
assert(KeyTaoModifierKeyPolicy.inputModeCapsLockFlags(previousFlags: .capsLock, currentFlags: []) == nil,
       "releasing a lock already active when KeyTao gained focus must preserve the hardware unlock")
assert(KeyTaoModifierKeyPolicy.inputModeCapsLockFlags(previousFlags: .capsLock, currentFlags: .capsLock) == nil,
       "duplicate flags with the hardware lock latched must preserve the normal Caps Lock policy")
for modifier: NSEvent.ModifierFlags in [.shift, .control, .option, .command, .function] {
    assert(KeyTaoModifierKeyPolicy.inputModeCapsLockFlags(
        previousFlags: modifier, currentFlags: [.capsLock, modifier]
    ) == nil, "combined Caps Lock presses must preserve the normal Caps Lock policy")
}

var tap = KeyTaoModifierTap()
func flags(
    _ keyCode: Int, _ previous: NSEvent.ModifierFlags, _ current: NSEvent.ModifierFlags,
    time: TimeInterval = 0
) -> UInt32? {
    tap.update(keyCode: UInt16(keyCode), previousFlags: previous, currentFlags: current, time: time)
}

let modifierKeys: [(Int, NSEvent.ModifierFlags, UInt32)] = [
    (kVK_Shift, .shift, 0xffe1),
    (kVK_RightShift, .shift, 0xffe2),
    (kVK_Control, .control, 0xffe3),
    (kVK_RightControl, .control, 0xffe4),
    (kVK_Option, .option, 0xffe9),
    (kVK_RightOption, .option, 0xffea),
]
for (keyCode, flag, keyval) in modifierKeys {
    assert(flags(keyCode, .capsLock, [.capsLock, flag]) == nil, "press only arms a modifier tap")
    assert(flags(keyCode, [.capsLock, flag], .capsLock, time: 0.1) == keyval,
           "release preserves the left or right key identity even with Caps Lock on")
    assert(flags(keyCode, flag, [], time: 0.2) == nil, "a release can switch only once")
}
assert(flags(kVK_Control, [], .control) == nil)
tap.clear()
assert(flags(kVK_Control, .control, [], time: 0.1) == nil, "typing or changing focus cancels the tap")
assert(flags(kVK_Option, [], .option) == nil)
assert(flags(kVK_Option, .option, [], time: 0.5) == nil, "a held modifier is not a tap")
assert(flags(kVK_Shift, [], .shift) == nil)
assert(flags(kVK_RightShift, .shift, .shift) == nil)
assert(flags(kVK_Shift, .shift, .shift) == nil)
assert(flags(kVK_RightShift, .shift, []) == nil, "overlapping left and right presses cannot form a solo tap")
assert(flags(kVK_Control, [], .control) == nil)
assert(flags(kVK_Command, .control, [.command, .control]) == nil)
assert(flags(kVK_Command, [.command, .control], .control) == nil)
assert(flags(kVK_Control, .control, []) == nil, "system Command chords cancel a modifier tap")
assert(flags(kVK_Option, .function, [.function, .option]) == nil)
assert(flags(kVK_Option, [.function, .option], .function) == nil, "Fn chords cannot switch modes")
assert(flags(kVK_Command, [], .command) == nil)
assert(flags(kVK_Command, .command, []) == nil, "Command is reserved for macOS")
print("chord key policy, release synthesis, Caps Lock and modifier tap tests passed")
