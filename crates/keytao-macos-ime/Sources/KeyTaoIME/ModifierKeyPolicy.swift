import AppKit
import Carbon

enum KeyTaoModifierKeyPolicy {
    static func activationFlags(_ flags: NSEvent.ModifierFlags, systemCapsLock: Bool) -> NSEvent.ModifierFlags {
        let flags = flags.intersection(.deviceIndependentFlagsMask).subtracting(.capsLock)
        return systemCapsLock ? flags.union(.capsLock) : flags
    }

    static func capsLockEvent(
        previousModifiers: UInt32, currentModifiers: UInt32
    ) -> (keyval: UInt32, modifiers: UInt32)? {
        let lock: UInt32 = 0x0002
        guard (previousModifiers ^ currentModifiers) & lock != 0 else { return nil }
        // Rime expects the lock state before the Caps Lock toggle.
        return (0xffe5, currentModifiers ^ lock)
    }
}

struct KeyTaoModifierTap {
    private static let modifiers: NSEvent.ModifierFlags = [.shift, .control, .option, .command, .function]
    private var pressed: (keyCode: UInt16, time: TimeInterval)?

    mutating func clear() {
        pressed = nil
    }

    mutating func update(
        keyCode: UInt16,
        previousFlags: NSEvent.ModifierFlags,
        currentFlags: NSEvent.ModifierFlags,
        time: TimeInterval
    ) -> UInt32? {
        let press = pressed
        clear()
        let (modifier, keyval): (NSEvent.ModifierFlags, UInt32)
        switch Int(keyCode) {
        case kVK_Shift: (modifier, keyval) = (.shift, 0xffe1)
        case kVK_RightShift: (modifier, keyval) = (.shift, 0xffe2)
        case kVK_Control: (modifier, keyval) = (.control, 0xffe3)
        case kVK_RightControl: (modifier, keyval) = (.control, 0xffe4)
        case kVK_Option: (modifier, keyval) = (.option, 0xffe9)
        case kVK_RightOption: (modifier, keyval) = (.option, 0xffea)
        default: return nil
        }
        let previous = previousFlags.intersection(Self.modifiers)
        let current = currentFlags.intersection(Self.modifiers)
        if previous.isEmpty && current == modifier {
            pressed = (keyCode, time)
            return nil
        }
        guard previous == modifier, current.isEmpty, let press, press.keyCode == keyCode,
              time >= press.time, time - press.time < 0.5 else { return nil }
        return keyval
    }
}
