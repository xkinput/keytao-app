package ink.rea.keytao_app

import org.junit.Assert.assertEquals
import org.junit.Test

class PerPointerLongPressDispatcherTest {
    // Container-only coverage: these tests do not exercise KeytaoKeyboardView.onTouchEvent integration.
    @Test
    fun `second pointer owns an independent long press timer`() {
        val scheduled = linkedMapOf<Runnable, Long>()
        val output = mutableListOf<String>()
        val dispatcher = PerPointerLongPressDispatcher<String>(
            postDelayed = { runnable, delay -> scheduled[runnable] = delay },
            removeCallbacks = { runnable -> scheduled.remove(runnable) },
        )

        dispatcher.begin(pointerId = 1, state = "A", delayMs = TEST_LONG_PRESS_DELAY_MS) { output += it }
        dispatcher.begin(pointerId = 2, state = "B", delayMs = TEST_LONG_PRESS_DELAY_MS) { output += it }

        val secondTimer = scheduled.keys.last()
        scheduled.remove(secondTimer)
        secondTimer.run()

        assertEquals(listOf("B"), output)
        assertEquals(1, scheduled.size)
        assertEquals(setOf("A", "B"), dispatcher.values.toSet())
    }

    @Test
    fun `tapping second pointer leaves held first pointer long press pending`() {
        val scheduled = linkedMapOf<Runnable, Long>()
        val output = mutableListOf<String>()
        val dispatcher = PerPointerLongPressDispatcher<String>(
            postDelayed = { runnable, delay -> scheduled[runnable] = delay },
            removeCallbacks = { runnable -> scheduled.remove(runnable) },
        )

        dispatcher.begin(pointerId = 1, state = "A", delayMs = TEST_LONG_PRESS_DELAY_MS) { output += it }
        dispatcher.begin(pointerId = 2, state = "B", delayMs = TEST_LONG_PRESS_DELAY_MS) { output += it }
        assertEquals("B", dispatcher.finish(pointerId = 2))
        val firstTimer = scheduled.keys.single()
        scheduled.remove(firstTimer)
        firstTimer.run()

        assertEquals(listOf("A"), output)
        assertEquals(0, scheduled.size)
        assertEquals(listOf("A"), dispatcher.values)
    }

    private companion object {
        const val TEST_LONG_PRESS_DELAY_MS = 300L
    }
}
