package ink.rea.keytao_app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class KeytaoChordTouchesTest {
    @Test
    fun `two overlapping pointers press before either releases`() {
        val touches = KeytaoChordTouches()
        val events = listOf(
            touches.begin(1, KeyCommand.input("d"), true, false),
            touches.begin(2, KeyCommand.rimeInput("k"), true, false),
            touches.finish(1),
            touches.finish(2),
        )
        assertEquals(listOf(down("d"), down("k"), up("d"), up("k")), events)
        assertTrue(touches.releaseAll().isEmpty())
        assertEquals(listOf(down("d"), down("f"), up("f"), up("d")), listOf(
            touches.begin(1, KeyCommand.input("d"), true, false),
            touches.begin(2, KeyCommand.input("f"), true, false),
            touches.finish(2),
            touches.finish(1),
        ))
    }

    @Test
    fun `three pointers preserve an overlapping chord when the first finger lifts`() {
        val touches = KeytaoChordTouches()
        val events = listOf(
            touches.begin(1, KeyCommand.input("d"), true, false),
            touches.begin(2, KeyCommand.input("j"), true, false),
            touches.finish(1),
            touches.begin(3, KeyCommand.input("k"), true, false),
            touches.finish(2),
            touches.finish(3),
        )
        assertEquals(listOf(down("d"), down("j"), up("d"), down("k"), up("j"), up("k")), events)
    }

    @Test
    fun `cancel releases the original key exactly once`() {
        val touches = KeytaoChordTouches()
        assertEquals(down("d"), touches.begin(1, KeyCommand.input("d"), true, false))
        assertTrue(touches.contains(1))
        assertEquals(up("d"), touches.finish(1))
        assertFalse(touches.contains(1))
        assertNull(touches.finish(1))
        assertTrue(touches.releaseAll().isEmpty())
    }

    @Test
    fun `hide or cancellation releases all held keys before callbacks can reenter`() {
        val touches = KeytaoChordTouches()
        touches.begin(1, KeyCommand.input("d"), true, false)
        touches.begin(2, KeyCommand.input("f"), true, false)
        assertEquals(listOf(up("d"), up("f")), touches.releaseAll())
        assertTrue(touches.releaseAll().isEmpty())
        assertNull(touches.finish(1))
        assertNull(touches.finish(2))
    }

    @Test
    fun `input restart removes only chord pointers and preserves ordinary rollover and long press`() {
        val scheduled = mutableSetOf<Runnable>()
        val active = PerPointerLongPressDispatcher<String>(
            postDelayed = { runnable, _ -> scheduled.add(runnable) },
            removeCallbacks = { runnable -> scheduled.remove(runnable) },
        )
        val touches = KeytaoChordTouches()
        active.begin(1, "a")
        active.begin(2, "b", 300) {}
        assertEquals("a", active.finish(1))
        assertTrue(touches.releaseAll { active.finish(it) }.isEmpty())
        assertEquals(listOf("b"), active.values)
        assertEquals(1, scheduled.size)
        active.begin(3, "d")
        active.begin(4, "k")
        touches.begin(3, KeyCommand.input("d"), true, false)
        touches.begin(4, KeyCommand.input("k"), true, false)
        assertEquals(listOf(up("d"), up("k")), touches.releaseAll { pointerId ->
            assertFalse(touches.contains(pointerId))
            active.finish(pointerId)
        })
        assertNull(active.finish(3))
        assertNull(active.finish(4))
        assertEquals("b", active.finish(2))
        assertTrue(scheduled.isEmpty())
    }

    @Test
    fun `mode changes cannot suppress releases for keys already pressed`() {
        val touches = KeytaoChordTouches()
        touches.begin(1, KeyCommand.input("d"), true, false)
        assertNull(touches.begin(2, KeyCommand.input("k"), false, true))
        assertEquals(up("d"), touches.finish(1))
    }

    @Test
    fun `non chord ascii and non character commands keep the normal tap path`() {
        val touches = KeytaoChordTouches()
        assertNull(touches.begin(1, KeyCommand.input("d"), false, false))
        assertNull(touches.begin(1, KeyCommand.input("d"), true, true))
        for (command in listOf(
            KeyCommand.directInput("d"),
            KeyCommand(KeyCommandTypes.SPACE, " "),
            KeyCommand(KeyCommandTypes.BACKSPACE),
            KeyCommand.input("dk"),
            KeyCommand.rimeInput("你"),
            KeyCommand.input("\n"),
            KeyCommand.input("é"),
        )) {
            assertNull(touches.begin(1, command, true, false))
        }
        assertFalse(touches.contains(1))
        assertNull(touches.finish(1))
        assertTrue(touches.releaseAll().isEmpty())
    }

    @Test
    fun `a lone key leaves no held state for the next chord`() {
        val touches = KeytaoChordTouches()
        assertEquals(down("d"), touches.begin(1, KeyCommand.input("d"), true, false))
        assertNull(touches.begin(1, KeyCommand.input("k"), true, false))
        assertEquals(up("d"), touches.finish(1))
        assertEquals(down("k"), touches.begin(1, KeyCommand.input("k"), true, false))
        assertEquals(listOf(up("k")), touches.releaseAll())
    }

    @Test
    fun `shifted input and rime input keep uppercase normal taps and consume one shot shift`() {
        for (command in listOf(KeyCommand.input("d"), KeyCommand.rimeInput("d", "d"))) {
            val touches = KeytaoChordTouches()
            val shifted = command.applyingShift(shiftActive = true, chordComposer = true)
            assertEquals("D", shifted.value)
            assertEquals(command.fallbackValue?.uppercase(), shifted.fallbackValue)
            assertTrue(shifted.consumesOneShotShift(chordComposer = true))
            assertNull(touches.begin(1, shifted, true, false, shiftActive = true))
            assertNull(touches.finish(1))
            assertEquals(down("d"), touches.begin(2, command, true, false))
            assertEquals(up("d"), touches.finish(2))
        }
        val rimeInput = KeyCommand.rimeInput("d", "d")
        assertEquals(rimeInput, rimeInput.applyingShift(shiftActive = true, chordComposer = false))
        assertFalse(rimeInput.consumesOneShotShift(chordComposer = false))
        assertEquals(KeyCommand.input("d"), KeyCommand.input("d").applyingShift(false, true))
    }

    private fun down(text: String) = KeytaoChordTouches.Event(text, released = false)
    private fun up(text: String) = KeytaoChordTouches.Event(text, released = true)
}
