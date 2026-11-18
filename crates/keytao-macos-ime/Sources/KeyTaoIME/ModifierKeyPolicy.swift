enum KeyTaoModifierKeyPolicy {
    static func capsLockEvent(
        previousModifiers: UInt32, currentModifiers: UInt32
    ) -> (keyval: UInt32, modifiers: UInt32)? {
        let lock: UInt32 = 0x0002
        guard (previousModifiers ^ currentModifiers) & lock != 0 else { return nil }
        // Rime expects the lock state before the Caps Lock toggle.
        return (0xffe5, currentModifiers ^ lock)
    }
}
