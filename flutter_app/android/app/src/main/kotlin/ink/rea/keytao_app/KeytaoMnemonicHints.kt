package ink.rea.keytao_app

/**
 * Keyboard mnemonics are now declared per keyboard profile inside the keyboard
 * configuration (key.mnemonic) rather than hardcoded per schema ID.
 */
internal object KeytaoMnemonicHints {
    fun forKey(key: KeySpec): KeyMnemonic? = key.mnemonic?.takeIf { it.isNonEmpty() }
}
