package ink.rea.keytao_app

import android.content.res.Configuration
import org.junit.Assert.assertEquals
import org.junit.Test

class KeytaoInputViewDecisionTest {
    @Test
    fun `visible hardware keyboard gets a strip when Android hides soft keys`() {
        assertEquals(
            KeytaoInputViewDecision(show = true, stripOnly = true),
            decideInputView(false, Configuration.KEYBOARD_QWERTY, Configuration.HARDKEYBOARDHIDDEN_NO),
        )
    }

    @Test
    fun `Android soft keyboard preference keeps the full keyboard`() {
        assertEquals(
            KeytaoInputViewDecision(show = true, stripOnly = false),
            decideInputView(true, Configuration.KEYBOARD_QWERTY, Configuration.HARDKEYBOARDHIDDEN_NO),
        )
    }

    @Test
    fun `missing or hidden hardware keyboard keeps the full keyboard`() {
        val full = KeytaoInputViewDecision(show = true, stripOnly = false)
        assertEquals(full, decideInputView(false, Configuration.KEYBOARD_NOKEYS, Configuration.HARDKEYBOARDHIDDEN_NO))
        assertEquals(full, decideInputView(false, Configuration.KEYBOARD_QWERTY, Configuration.HARDKEYBOARDHIDDEN_YES))
    }
}
