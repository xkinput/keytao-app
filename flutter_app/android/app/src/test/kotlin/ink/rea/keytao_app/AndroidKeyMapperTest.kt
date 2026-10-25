package ink.rea.keytao_app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class AndroidKeyMapperTest {
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
        assertTrue(AndroidKeyMapper.permitsSoloShift(0))
        assertTrue(AndroidKeyMapper.permitsSoloShift(AndroidKeyMapper.RIME_MOD_SHIFT))
        assertTrue(AndroidKeyMapper.permitsSoloShift(AndroidKeyMapper.RIME_MOD_SHIFT or AndroidKeyMapper.RIME_MOD_LOCK))
        for (modifier in listOf(
            AndroidKeyMapper.RIME_MOD_CONTROL,
            AndroidKeyMapper.RIME_MOD_ALT,
            AndroidKeyMapper.RIME_MOD_SUPER,
        )) {
            assertFalse(AndroidKeyMapper.permitsSoloShift(modifier))
            assertFalse(AndroidKeyMapper.permitsSoloShift(modifier or AndroidKeyMapper.RIME_MOD_SHIFT))
        }
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
}
