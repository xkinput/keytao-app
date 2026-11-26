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
    const val XK_CAPS_LOCK = 0xffe5
    const val PREVIOUS_CANDIDATE = 0x02000000
    const val NEXT_CANDIDATE = 0x02000001
    const val PREVIOUS_PAGE = 0x02000002
    const val NEXT_PAGE = 0x02000003
    const val OPEN_PACKAGE_MENU = 0x02000004

    fun fromAndroidKeyEvent(event: KeyEvent): RimeKey? {
        val keyCode = event.keyCode
        if (keyCode == KeyEvent.KEYCODE_LANGUAGE_SWITCH ||
            (keyCode == KeyEvent.KEYCODE_CAPS_LOCK && event.repeatCount != 0)
        ) return null
        val metaState = event.metaState
        val codePoint = event.getUnicodeChar(metaState)
        var modifiers = modifiers(event)
        if (modifiers and RIME_MOD_ALT != 0) {
            val codePointWithoutAlt = event.getUnicodeChar(metaState and KeyEvent.META_ALT_MASK.inv())
            modifiers = modifiersForUnicode(modifiers, codePoint, codePointWithoutAlt)
        }
        specialHardwareKeysym(keyCode)?.let { return RimeKey(it, modifiers) }
        return fromHardwareUnicode(keyCode, codePoint, modifiers) {
            KeytaoNativeBridge.textToKeysym(it)
        }
    }

    internal fun modifiersForUnicode(modifiers: Int, codePoint: Int, codePointWithoutAlt: Int): Int =
        if (codePoint != 0 && codePoint != codePointWithoutAlt) modifiers and RIME_MOD_ALT.inv() else modifiers

    internal fun consumedAltMetaStates(altPressed: Boolean, rimeModifiers: Int): Int =
        if (altPressed && rimeModifiers and RIME_MOD_ALT == 0) KeyEvent.META_ALT_MASK else 0

    internal fun specialHardwareKeysym(keyCode: Int): Int? = when (keyCode) {
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
        KeyEvent.KEYCODE_CAPS_LOCK -> XK_CAPS_LOCK
        else -> null
    }

    internal fun fromHardwareUnicode(
        keyCode: Int,
        codePoint: Int,
        modifiers: Int,
        textToKeysym: (String) -> Int,
    ): RimeKey? {
        if (keyCode == KeyEvent.KEYCODE_SPACE && codePoint == 0) return RimeKey(XK_SPACE, modifiers)
        val key = fromUnicodeCodePoint(codePoint, modifiers, textToKeysym)
        if (key == null && keyCode == KeyEvent.KEYCODE_GRAVE &&
            modifiers and RIME_MOD_LOCK.inv() == RIME_MOD_CONTROL
        ) return RimeKey(OPEN_PACKAGE_MENU, 0)
        return key
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

    internal fun modifierTapKeysym(keyCode: Int, metaState: Int = 0): Int? {
        val (keysym, ownModifier) = when (keyCode) {
            KeyEvent.KEYCODE_SHIFT_LEFT -> XK_SHIFT_L to (KeyEvent.META_SHIFT_ON or KeyEvent.META_SHIFT_LEFT_ON)
            KeyEvent.KEYCODE_SHIFT_RIGHT -> XK_SHIFT_R to (KeyEvent.META_SHIFT_ON or KeyEvent.META_SHIFT_RIGHT_ON)
            KeyEvent.KEYCODE_CTRL_LEFT -> 0xffe3 to (KeyEvent.META_CTRL_ON or KeyEvent.META_CTRL_LEFT_ON)
            KeyEvent.KEYCODE_CTRL_RIGHT -> 0xffe4 to (KeyEvent.META_CTRL_ON or KeyEvent.META_CTRL_RIGHT_ON)
            KeyEvent.KEYCODE_ALT_LEFT -> 0xffe9 to (KeyEvent.META_ALT_ON or KeyEvent.META_ALT_LEFT_ON)
            KeyEvent.KEYCODE_ALT_RIGHT -> 0xffea to (KeyEvent.META_ALT_ON or KeyEvent.META_ALT_RIGHT_ON)
            else -> return null
        }
        val modifiers = KeyEvent.META_SHIFT_MASK or KeyEvent.META_CTRL_MASK or
            KeyEvent.META_ALT_MASK or KeyEvent.META_META_MASK or KeyEvent.META_FUNCTION_ON or KeyEvent.META_SYM_ON
        return keysym.takeIf { metaState and modifiers and ownModifier.inv() == 0 }
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

    internal fun candidateArrowKey(
        keyCode: Int,
        modifiers: Int,
        panelVisible: Boolean,
        hasCandidates: Boolean,
        isPackageSwitcher: Boolean = false,
    ): Int {
        if (!panelVisible || !hasCandidates || modifiers and RIME_MOD_LOCK.inv() != 0) {
            return keyCode
        }
        if (isPackageSwitcher) {
            return when (keyCode) {
                XK_UP, XK_LEFT -> PREVIOUS_CANDIDATE
                XK_DOWN, XK_RIGHT -> NEXT_CANDIDATE
                else -> keyCode
            }
        }
        return when (keyCode) {
            XK_LEFT -> PREVIOUS_CANDIDATE
            XK_RIGHT -> NEXT_CANDIDATE
            XK_UP -> PREVIOUS_PAGE
            XK_DOWN -> NEXT_PAGE
            else -> keyCode
        }
    }

    internal fun shouldScheduleReloadStampTimer(
        serviceDestroyed: Boolean,
        inputViewShown: Boolean,
        hardwareKeyboardActive: Boolean,
        readiness: Readiness?,
        retryCount: Int,
    ): Boolean {
        if (serviceDestroyed) return false
        if (inputViewShown) return true
        if (!hardwareKeyboardActive) return false
        return when (readiness) {
            Readiness.DEPLOYING -> true
            Readiness.READY, Readiness.ENGINE_DISABLED, Readiness.PERMISSION_MISSING,
            Readiness.UNWRITABLE, Readiness.NOT_INSTALLED -> false
            else -> retryCount < 30
        }
    }
}
