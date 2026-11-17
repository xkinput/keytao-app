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
print("chord key policy tests passed")
