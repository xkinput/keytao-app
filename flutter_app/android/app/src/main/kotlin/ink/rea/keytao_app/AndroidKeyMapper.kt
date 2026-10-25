package ink.rea.keytao_app

import android.view.KeyEvent

data class RimeKey(
    val keyCode: Int,
    val modifiers: Int,
)

object AndroidKeyMapper {
    const val RIME_MOD_SHIFT = 0x0001
    const val RIME_MOD_LOCK = 0x0002
    const val RIME_MOD_CONTROL = 0x0004
    const val RIME_MOD_ALT = 0x0008
    const val RIME_MOD_SUPER = 1 shl 26
    const val RIME_RELEASE_MASK = 1 shl 30

    const val XK_SPACE = 0x0020
    const val XK_BACK_SPACE = 0xff08
    const val XK_TAB = 0xff09
    const val XK_RETURN = 0xff0d
    const val XK_ESCAPE = 0xff1b
    const val XK_HOME = 0xff50
    const val XK_LEFT = 0xff51
    const val XK_UP = 0xff52
    const val XK_RIGHT = 0xff53
    const val XK_DOWN = 0xff54
    const val XK_PAGE_UP = 0xff55
    const val XK_PAGE_DOWN = 0xff56
    const val XK_END = 0xff57
    const val XK_F4 = 0xffc1
    const val XK_DELETE = 0xffff
    const val XK_SHIFT_L = 0xffe1
    const val XK_SHIFT_R = 0xffe2

    fun fromAndroidKeyEvent(event: KeyEvent): RimeKey? {
        val special = when (event.keyCode) {
            KeyEvent.KEYCODE_DEL -> XK_BACK_SPACE
            KeyEvent.KEYCODE_FORWARD_DEL -> XK_DELETE
            KeyEvent.KEYCODE_TAB -> XK_TAB
            KeyEvent.KEYCODE_ENTER, KeyEvent.KEYCODE_NUMPAD_ENTER -> XK_RETURN
            KeyEvent.KEYCODE_ESCAPE -> XK_ESCAPE
            KeyEvent.KEYCODE_MOVE_HOME -> XK_HOME
            KeyEvent.KEYCODE_DPAD_LEFT -> XK_LEFT
            KeyEvent.KEYCODE_DPAD_UP -> XK_UP
            KeyEvent.KEYCODE_DPAD_RIGHT -> XK_RIGHT
            KeyEvent.KEYCODE_DPAD_DOWN -> XK_DOWN
            KeyEvent.KEYCODE_PAGE_UP -> XK_PAGE_UP
            KeyEvent.KEYCODE_PAGE_DOWN -> XK_PAGE_DOWN
            KeyEvent.KEYCODE_MOVE_END -> XK_END
            KeyEvent.KEYCODE_F4 -> XK_F4
            else -> null
        }
        if (special != null) return RimeKey(special, modifiers(event))

        return fromUnicodeCodePoint(event.getUnicodeChar(event.metaState), modifiers(event)) {
            KeytaoNativeBridge.textToKeysym(it)
        }
    }

    /**
     * Soft keyboard text to the keysym Rime expects.
     *
     * keytao-core owns the mapping (Latin-1 passes through, everything else uses
     * X11's `0x01000000 | codepoint` form) so a layout key such as `（` (U+FF08)
     * can never arrive as `XK_BackSpace`. A null result means "do not hand this
     * to Rime, commit it directly".
     */
    fun fromText(value: String): RimeKey? {
        if (value.codePointCount(0, value.length) != 1) return null
        return fromUnicodeCodePoint(value.codePointAt(0), 0) {
            KeytaoNativeBridge.textToKeysym(it)
        }
    }

    /** Share the native Unicode-to-keysym mapping for hardware and soft keys. */
    internal fun fromUnicodeCodePoint(
        codePoint: Int,
        modifiers: Int,
        textToKeysym: (String) -> Int,
    ): RimeKey? {
        // Android marks dead accents with COMBINING_ACCENT in the sign bit.
        // Neither these nor invalid Unicode scalars are standalone text keys.
        if (
            !Character.isValidCodePoint(codePoint) ||
            codePoint in 0xd800..0xdfff || codePoint == 0
        ) return null
        val keysym = textToKeysym(String(Character.toChars(codePoint)))
        if (keysym == 0) return null
        return RimeKey(keysym, modifiers)
    }

    fun permitsSoloShift(event: KeyEvent): Boolean = permitsSoloShift(modifiers(event))

    internal fun permitsSoloShift(modifiers: Int): Boolean {
        return modifiers and (RIME_MOD_CONTROL or RIME_MOD_ALT or RIME_MOD_SUPER) == 0
    }

    private fun modifiers(event: KeyEvent): Int {
        return modifiers(
            shift = event.isShiftPressed,
            control = event.isCtrlPressed,
            alt = event.isAltPressed,
            meta = event.isMetaPressed,
            capsLock = event.isCapsLockOn,
        )
    }

    internal fun modifiers(
        shift: Boolean,
        control: Boolean,
        alt: Boolean,
        meta: Boolean,
        capsLock: Boolean,
    ): Int {
        var mask = 0
        if (shift) mask = mask or RIME_MOD_SHIFT
        if (capsLock) mask = mask or RIME_MOD_LOCK
        if (control) mask = mask or RIME_MOD_CONTROL
        if (alt) mask = mask or RIME_MOD_ALT
        if (meta) mask = mask or RIME_MOD_SUPER
        return mask
    }
}
