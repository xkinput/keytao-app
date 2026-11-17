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
