package ink.rea.keytao_app

import android.view.KeyEvent
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class AndroidKeyMapperTest {
    @Test
    fun `hardware Caps Lock reaches Rime while system language switching stays with Android`() {
        assertEquals(0xffe5, AndroidKeyMapper.specialHardwareKeysym(KeyEvent.KEYCODE_CAPS_LOCK))
        assertNull(AndroidKeyMapper.specialHardwareKeysym(KeyEvent.KEYCODE_LANGUAGE_SWITCH))
    }

    @Test
    fun `hardware Unicode uses the shared keysym conversion instead of special key values`() {
        for ((text, keysym) in listOf(
            "A" to 0x0041,
            "é" to 0x00e9,
            "（" to 0x0100ff08,
            "键" to 0x0100952e,
            "🙂" to 0x0101f642,
        )) {
            val seen = mutableListOf<String>()
            val result = AndroidKeyMapper.fromUnicodeCodePoint(
                text.codePointAt(0),
                AndroidKeyMapper.RIME_MOD_SHIFT,
            ) {
                seen.add(it)
                keysym
            }
            assertEquals(listOf(text), seen)
            assertEquals(RimeKey(keysym, AndroidKeyMapper.RIME_MOD_SHIFT), result)
            assertTrue(result!!.keyCode != AndroidKeyMapper.XK_BACK_SPACE)
        }
    }

    @Test
    fun `number row and keypad digit characters reach the shared conversion intact`() {
        for (digit in '0'..'9') {
            var conversionCalls = 0
            val result = AndroidKeyMapper.fromUnicodeCodePoint(digit.code, 0) {
                conversionCalls++
                assertEquals(digit.toString(), it)
                digit.code
            }
            assertEquals(RimeKey(digit.code, 0), result)
            assertEquals(1, conversionCalls)
        }
    }

    @Test
    fun `dead accents and invalid Unicode scalars are left to the host`() {
        for (codePoint in listOf(0, -1, Int.MIN_VALUE or 0x00b4, 0xd800, 0xdfff, 0x110000)) {
            var converted = false
            val result = AndroidKeyMapper.fromUnicodeCodePoint(codePoint, 0) {
                converted = true
                1
            }
            assertNull(result)
            assertFalse(converted)
        }
        assertNull(AndroidKeyMapper.fromUnicodeCodePoint('\n'.code, 0) { 0 })
    }

    @Test
    fun `only Shift without control alt or meta may toggle the input mode`() {
        val shift = KeyEvent.KEYCODE_SHIFT_LEFT
        val shiftOn = KeyEvent.META_SHIFT_LEFT_ON or KeyEvent.META_SHIFT_ON
        assertEquals(AndroidKeyMapper.XK_SHIFT_L, AndroidKeyMapper.modifierTapKeysym(shift))
        assertEquals(AndroidKeyMapper.XK_SHIFT_L, AndroidKeyMapper.modifierTapKeysym(shift, shiftOn))
        assertEquals(AndroidKeyMapper.XK_SHIFT_L, AndroidKeyMapper.modifierTapKeysym(shift, shiftOn or KeyEvent.META_CAPS_LOCK_ON))
        for (modifier in listOf(
            KeyEvent.META_CTRL_ON,
            KeyEvent.META_ALT_ON,
            KeyEvent.META_META_ON,
            KeyEvent.META_SHIFT_RIGHT_ON,
        )) {
            assertNull(AndroidKeyMapper.modifierTapKeysym(shift, modifier))
            assertNull(AndroidKeyMapper.modifierTapKeysym(shift, modifier or shiftOn))
        }
    }

    @Test
    fun `standalone Control and Alt use the physical side and leave AltGr chords alone`() {
        assertEquals(0xffe3, AndroidKeyMapper.modifierTapKeysym(KeyEvent.KEYCODE_CTRL_LEFT))
        assertEquals(0xffe4, AndroidKeyMapper.modifierTapKeysym(KeyEvent.KEYCODE_CTRL_RIGHT, KeyEvent.META_CTRL_RIGHT_ON))
        assertEquals(0xffe9, AndroidKeyMapper.modifierTapKeysym(KeyEvent.KEYCODE_ALT_LEFT))
        val altRight = KeyEvent.KEYCODE_ALT_RIGHT
        assertEquals(0xffea, AndroidKeyMapper.modifierTapKeysym(altRight, KeyEvent.META_ALT_RIGHT_ON))
        assertNull(AndroidKeyMapper.modifierTapKeysym(altRight, KeyEvent.META_ALT_RIGHT_ON or KeyEvent.META_CTRL_ON))
        assertNull(AndroidKeyMapper.modifierTapKeysym(KeyEvent.KEYCODE_LANGUAGE_SWITCH))
    }

    @Test
    fun `modified Space falls back to its keysym only when the keymap gives no character`() {
        val space = KeyEvent.KEYCODE_SPACE
        val ctrl = AndroidKeyMapper.RIME_MOD_CONTROL
        val alt = AndroidKeyMapper.RIME_MOD_ALT
        assertEquals(RimeKey(AndroidKeyMapper.XK_SPACE, ctrl), AndroidKeyMapper.fromHardwareUnicode(space, 0, ctrl) { 0 })
        val noBreakSpace = '\u00a0'.code
        assertEquals(RimeKey(noBreakSpace, alt), AndroidKeyMapper.fromHardwareUnicode(space, noBreakSpace, alt) { it.single().code })
    }

    @Test
    fun `Alt-produced digits and punctuation drop Alt while preserving other modifiers`() {
        val alt = AndroidKeyMapper.RIME_MOD_ALT
        val shift = AndroidKeyMapper.RIME_MOD_SHIFT
        assertEquals(0, AndroidKeyMapper.modifiersForUnicode(alt, '1'.code, 'w'.code))
        assertEquals(shift, AndroidKeyMapper.modifiersForUnicode(alt or shift, '！'.code, 'Q'.code))
        assertEquals(0, AndroidKeyMapper.modifiersForUnicode(alt, '!'.code, 0))
    }

    @Test
    fun `Alt remains when it produces the same character or no character`() {
        val alt = AndroidKeyMapper.RIME_MOD_ALT
        assertEquals(alt, AndroidKeyMapper.modifiersForUnicode(alt, 'w'.code, 'w'.code))
        assertEquals(alt, AndroidKeyMapper.modifiersForUnicode(alt, 0, 'w'.code))
        assertEquals(alt, AndroidKeyMapper.modifiersForUnicode(alt, 0, 0))
    }

    @Test
    fun `consumed Alt-produced characters clear only the editor Alt bits`() {
        val alt = AndroidKeyMapper.RIME_MOD_ALT
        val shift = AndroidKeyMapper.RIME_MOD_SHIFT
        val stripped = AndroidKeyMapper.modifiersForUnicode(alt or shift, '!'.code, 'Q'.code)
        assertEquals(KeyEvent.META_ALT_MASK, AndroidKeyMapper.consumedAltMetaStates(true, stripped))
        assertEquals(0, AndroidKeyMapper.consumedAltMetaStates(true, alt or shift))
        assertEquals(0, AndroidKeyMapper.consumedAltMetaStates(false, shift))
    }

    @Test
    fun `hardware modifiers retain CapsLock and Meta independently of Shift`() {
        for (shift in listOf(false, true)) {
            for (capsLock in listOf(false, true)) {
                val mask = AndroidKeyMapper.modifiers(
                    shift = shift,
                    capsLock = capsLock,
                    control = false,
                    alt = false,
                    meta = false,
                )
                assertEquals(shift, mask and AndroidKeyMapper.RIME_MOD_SHIFT != 0)
                assertEquals(capsLock, mask and AndroidKeyMapper.RIME_MOD_LOCK != 0)
            }
        }
        assertEquals(
            AndroidKeyMapper.RIME_MOD_CONTROL or AndroidKeyMapper.RIME_MOD_ALT or AndroidKeyMapper.RIME_MOD_SUPER,
            AndroidKeyMapper.modifiers(shift = false, capsLock = false, control = true, alt = true, meta = true),
        )
    }

    @Test
    fun `visible horizontal candidate bar maps arrow keys and hardware ready state stops idle polling`() {
        assertEquals(
            AndroidKeyMapper.PREVIOUS_CANDIDATE,
            AndroidKeyMapper.candidateArrowKey(AndroidKeyMapper.XK_LEFT, 0, panelVisible = true, hasCandidates = true),
        )
        assertEquals(
            AndroidKeyMapper.NEXT_CANDIDATE,
            AndroidKeyMapper.candidateArrowKey(AndroidKeyMapper.XK_RIGHT, AndroidKeyMapper.RIME_MOD_LOCK, panelVisible = true, hasCandidates = true),
        )
        assertEquals(
            AndroidKeyMapper.PREVIOUS_PAGE,
            AndroidKeyMapper.candidateArrowKey(AndroidKeyMapper.XK_UP, 0, panelVisible = true, hasCandidates = true),
        )
        assertEquals(
            AndroidKeyMapper.NEXT_PAGE,
            AndroidKeyMapper.candidateArrowKey(AndroidKeyMapper.XK_DOWN, 0, panelVisible = true, hasCandidates = true),
        )
        assertEquals(
            AndroidKeyMapper.XK_LEFT,
            AndroidKeyMapper.candidateArrowKey(AndroidKeyMapper.XK_LEFT, 0, panelVisible = false, hasCandidates = true),
        )
        assertEquals(
            AndroidKeyMapper.PREVIOUS_CANDIDATE,
            AndroidKeyMapper.candidateArrowKey(AndroidKeyMapper.XK_UP, 0, panelVisible = true, hasCandidates = true, isPackageSwitcher = true),
        )
        assertEquals(
            AndroidKeyMapper.NEXT_CANDIDATE,
            AndroidKeyMapper.candidateArrowKey(AndroidKeyMapper.XK_DOWN, 0, panelVisible = true, hasCandidates = true, isPackageSwitcher = true),
        )

        assertFalse(
            AndroidKeyMapper.shouldScheduleReloadStampTimer(
                serviceDestroyed = false,
                inputViewShown = false,
                hardwareKeyboardActive = true,
                readiness = Readiness.READY,
                retryCount = 0,
            ),
        )
        assertTrue(
            AndroidKeyMapper.shouldScheduleReloadStampTimer(
                serviceDestroyed = false,
                inputViewShown = false,
                hardwareKeyboardActive = true,
                readiness = Readiness.DEPLOYING,
                retryCount = 0,
            ),
        )
        assertFalse(
            AndroidKeyMapper.shouldScheduleReloadStampTimer(
                serviceDestroyed = false,
                inputViewShown = false,
                hardwareKeyboardActive = true,
                readiness = Readiness.ENGINE_DISABLED,
                retryCount = 0,
            ),
        )
        assertTrue(
            AndroidKeyMapper.shouldScheduleReloadStampTimer(
                serviceDestroyed = false,
                inputViewShown = true,
                hardwareKeyboardActive = false,
                readiness = Readiness.READY,
                retryCount = 0,
            ),
        )
    }
}
