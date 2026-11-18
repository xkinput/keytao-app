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
print("chord key policy, release synthesis and Caps Lock policy tests passed")
