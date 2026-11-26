package ink.rea.keytao_app

import android.content.res.Configuration

internal data class KeytaoInputViewDecision(val show: Boolean, val stripOnly: Boolean)

internal fun decideInputView(
    softKeys: Boolean,
    keyboard: Int,
    hardKeyboardHidden: Int,
): KeytaoInputViewDecision = KeytaoInputViewDecision(
    show = true,
    stripOnly = !softKeys && keyboard != Configuration.KEYBOARD_NOKEYS &&
        hardKeyboardHidden != Configuration.HARDKEYBOARDHIDDEN_YES,
)
