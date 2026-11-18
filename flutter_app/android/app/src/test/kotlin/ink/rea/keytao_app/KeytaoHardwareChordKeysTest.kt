package ink.rea.keytao_app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class KeytaoHardwareChordKeysTest {
    @Test
    fun `rejected and modifier held presses leave key up to the host`() {
        val keys = KeytaoHardwareChordKeys()
        for (modifiers in listOf(0, AndroidKeyMapper.RIME_MOD_CONTROL, AndroidKeyMapper.RIME_MOD_ALT)) {
            keys.recordPress(31, RimeKey('c'.code, modifiers), false, true, false)
            keys.passToHost(31)
            assertEquals(KeytaoHardwareChordKeys.Release(null, true), keys.release(31))
        }
        for (modifiers in listOf(
            AndroidKeyMapper.RIME_MOD_SHIFT,
            AndroidKeyMapper.RIME_MOD_LOCK,
            AndroidKeyMapper.RIME_MOD_CONTROL,
            AndroidKeyMapper.RIME_MOD_ALT,
            AndroidKeyMapper.RIME_MOD_SUPER,
        )) {
            keys.recordPress(52, RimeKey('x'.code, modifiers), true, true, false)
            assertNull(keys.release(52))
        }
    }

    @Test
    fun `only accepted chord presses release their saved keysym exactly once`() {
        val keys = KeytaoHardwareChordKeys()
        keys.recordPress(32, RimeKey('d'.code, 0), true, true, false)
        keys.recordPress(39, RimeKey('k'.code, 0), true, true, false)
        assertEquals(KeytaoHardwareChordKeys.Release(RimeKey('d'.code, AndroidKeyMapper.RIME_RELEASE_MASK), false), keys.release(32))
        assertNull(keys.release(32))
        assertEquals(listOf(RimeKey('k'.code, AndroidKeyMapper.RIME_RELEASE_MASK)), keys.releaseAll())
        assertTrue(keys.releaseAll().isEmpty())
        assertNull(keys.release(39))
    }

    @Test
    fun `normal ascii and non printable accepted keys never enter the chord path`() {
        val keys = KeytaoHardwareChordKeys()
        keys.recordPress(32, RimeKey('d'.code, 0), true, false, false)
        keys.recordPress(32, RimeKey('d'.code, 0), true, true, true)
        keys.recordPress(66, RimeKey(AndroidKeyMapper.XK_RETURN, 0), true, true, false)
        assertTrue(keys.releaseAll().isEmpty())
    }

    @Test
    fun `a rejected repeat still releases the original chord and passes key up to the host`() {
        val keys = KeytaoHardwareChordKeys()
        keys.recordPress(32, RimeKey('d'.code, 0), true, true, false)
        keys.recordPress(32, RimeKey('D'.code, AndroidKeyMapper.RIME_MOD_CONTROL), false, true, false)
        keys.passToHost(32)
        assertEquals(
            KeytaoHardwareChordKeys.Release(RimeKey('d'.code, AndroidKeyMapper.RIME_RELEASE_MASK), true),
            keys.release(32),
        )
        assertNull(keys.release(32))
        keys.passToHost(31)
        keys.recordPress(39, RimeKey('k'.code, 0), true, true, false)
        assertEquals(listOf(RimeKey('k'.code, AndroidKeyMapper.RIME_RELEASE_MASK)), keys.releaseAll())
        assertEquals(KeytaoHardwareChordKeys.Release(null, true), keys.release(31))
        assertNull(keys.release(39))
    }
}
