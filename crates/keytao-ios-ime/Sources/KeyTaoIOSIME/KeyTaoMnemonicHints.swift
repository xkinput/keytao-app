import Foundation

/// Keyboard mnemonics are declared per keyboard profile inside the keyboard
/// configuration (`key.mnemonic`) rather than hardcoded per schema ID.
enum KeyTaoMnemonicHints {
    static func forKey(_ key: KeyTaoKeySpec) -> KeyTaoKeyMnemonic? {
        guard let mnemonic = key.mnemonic, mnemonic.isNonEmpty else {
            return nil
        }
        return mnemonic
    }
}
