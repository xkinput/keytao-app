package ink.rea.keytao_app

import android.view.KeyEvent

data class RimeKey(
    val keyCode: Int,
    val modifiers: Int,
)

object AndroidKeyMapper {
    const val RIME_MOD_SHIFT = 0x0001
    const val RIME_MOD_CONTROL = 0x0004
    const val RIME_MOD_ALT = 0x0008
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

        val unicode = event.getUnicodeChar(event.metaState)
        if (unicode <= 0) return null
        return RimeKey(unicode, modifiers(event))
    }

    fun fromText(value: String): RimeKey? {
        if (value.codePointCount(0, value.length) != 1) return null
        return RimeKey(value.codePointAt(0), 0)
    }

    private fun modifiers(event: KeyEvent): Int {
        var mask = 0
        if (event.isShiftPressed) mask = mask or RIME_MOD_SHIFT
        if (event.isCtrlPressed) mask = mask or RIME_MOD_CONTROL
        if (event.isAltPressed) mask = mask or RIME_MOD_ALT
        return mask
    }
}
