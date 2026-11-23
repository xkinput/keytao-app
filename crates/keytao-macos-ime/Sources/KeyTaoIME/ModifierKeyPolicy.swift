import AppKit
import Carbon

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
        let previous = previousFlags.intersection(Self.modifiers)
        let current = currentFlags.intersection(Self.modifiers)
        // Some IMKit clients report a zero key code on Shift release.
        let effectiveKeyCode = keyCode == 0 && previous == .shift && current.isEmpty
            ? pressed?.keyCode ?? keyCode : keyCode
        let (modifier, keyval): (NSEvent.ModifierFlags, UInt32)
        switch Int(effectiveKeyCode) {
        case kVK_Shift: (modifier, keyval) = (.shift, 0xffe1)
        case kVK_RightShift: (modifier, keyval) = (.shift, 0xffe2)
        case kVK_Control: (modifier, keyval) = (.control, 0xffe3)
        case kVK_RightControl: (modifier, keyval) = (.control, 0xffe4)
        case kVK_Option: (modifier, keyval) = (.option, 0xffe9)
        case kVK_RightOption: (modifier, keyval) = (.option, 0xffea)
        default:
            clear()
            return nil
        }
        if previous == modifier && current == modifier && pressed?.keyCode == effectiveKeyCode {
            return nil
        }
        let press = pressed
        clear()
        if previous.isEmpty && current == modifier {
            pressed = (effectiveKeyCode, time)
            return nil
        }
        guard previous == modifier, current.isEmpty, let press, press.keyCode == effectiveKeyCode,
              time >= press.time, time - press.time < 0.5 else { return nil }
        return keyval
    }
}
