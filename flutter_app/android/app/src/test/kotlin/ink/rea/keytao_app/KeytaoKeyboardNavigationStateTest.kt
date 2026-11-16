package ink.rea.keytao_app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class KeytaoKeyboardNavigationStateTest {
    @Test
    fun `only entering a direct input page or panel requires committing composition`() {
        val state = KeyboardNavigationState()
        val targets = listOf(
            Triple("numbers", null, true),
            Triple("symbols_math", null, true),
            Triple("symbols_emoji_face", null, true),
            Triple("letters", KeyboardPanel.CLIPBOARD, true),
            Triple("letters", KeyboardPanel.PHRASES, true),
            Triple("letters", null, false),
            Triple("editor", null, false),
            Triple("letters", KeyboardPanel.RIME, false),
            Triple("letters", KeyboardPanel.SETTINGS, false),
            Triple("letters", KeyboardPanel.EXPANDED_CANDIDATES, false),
        )

        for ((layer, panel, expected) in targets) {
            assertEquals(expected, state.shouldCommitBeforeNavigation(layer, panel))
            assertEquals(false, state.shouldCommitBeforeNavigation(layer, panel, reset = true))
        }
        state.navigate("numbers")
        assertEquals(false, state.shouldCommitBeforeNavigation("numbers", null))
        state.navigate(panel = KeyboardPanel.CLIPBOARD)
        assertEquals(false, state.shouldCommitBeforeNavigation("numbers", KeyboardPanel.CLIPBOARD))
        assertEquals(true, state.shouldCommitBeforeNavigation("numbers", KeyboardPanel.PHRASES))
    }

    @Test
    fun `number symbol and emoji pages share one origin`() {
        val state = KeyboardNavigationState()

        for (layer in listOf("numbers", "symbols_math", "symbols_emoji_animals", "numbers")) {
            state.navigate(layer)
            assertEquals("letters", state.returnLayer)
            assertEquals("letters", state.resolveLayer("back"))
        }
        state.navigate(state.resolveLayer("back"))

        assertEquals("letters", state.layer)
        assertEquals("letters", state.returnLayer)
    }

    @Test
    fun `back to editor consumes its single origin without building a stack`() {
        val state = KeyboardNavigationState()
        state.navigate("symbols_math")
        state.navigate("editor")
        assertEquals("symbols_math", state.returnLayer)

        for (layer in listOf("numbers", "symbols_en", "symbols_emoji_face")) {
            state.navigate(layer)
            assertEquals("editor", state.returnLayer)
        }
        state.navigate(state.resolveLayer("back"))

        assertEquals("editor", state.layer)
        assertEquals("letters", state.returnLayer)
        state.navigate(state.resolveLayer("back"))
        assertEquals("letters", state.layer)
    }

    @Test
    fun `closing every overlay preserves its page origin and one shot shift`() {
        val state = KeyboardNavigationState()
        state.navigate("editor")
        state.navigate("numbers")
        state.shift = KeyboardShiftState.ONCE

        for (panel in KeyboardPanel.values()) {
            state.navigate(panel = panel)
            assertEquals(panel, state.panel)
            state.navigate()

            assertNull(state.panel)
            assertEquals("numbers", state.layer)
            assertEquals("editor", state.returnLayer)
            assertEquals(KeyboardShiftState.ONCE, state.shift)
        }
    }

    @Test
    fun `new field resets pages panels origin and both shift states`() {
        for (shift in listOf(KeyboardShiftState.ONCE, KeyboardShiftState.LOCKED)) {
            for (forcedLayer in listOf("letters", "numbers")) {
                val state = KeyboardNavigationState()
                state.navigate("editor")
                state.navigate("symbols_math", panel = KeyboardPanel.CLIPBOARD)
                state.shift = shift

                state.navigate(layer = forcedLayer, reset = true)

                assertEquals(forcedLayer, state.layer)
                assertNull(state.panel)
                assertEquals("letters", state.returnLayer)
                assertEquals(KeyboardShiftState.OFF, state.shift)
                assertEquals("symbols_math", state.resolveLayer("symbols:last"))
            }
        }
    }

    @Test
    fun `page switches retain caps lock but clear one shot shift`() {
        val state = KeyboardNavigationState()
        state.shift = KeyboardShiftState.LOCKED

        for (layer in listOf("editor", "numbers", "symbols_math", "symbols_emoji_face", "letters")) {
            state.navigate(layer)
            assertEquals(KeyboardShiftState.LOCKED, state.shift)
        }
        state.shift = KeyboardShiftState.ONCE
        state.navigate("numbers")

        assertEquals(KeyboardShiftState.OFF, state.shift)
    }

    @Test
    fun `last category targets remember symbol and emoji categories independently`() {
        val state = KeyboardNavigationState()
        assertEquals("symbols", state.resolveLayer("symbols:last"))
        assertEquals("symbols_emoji_face", state.resolveLayer("emoji:last"))

        state.navigate("symbols_math")
        state.navigate("symbols_emoji_animals")
        state.navigate("letters")

        assertEquals("symbols_math", state.resolveLayer("symbols:last"))
        assertEquals("symbols_emoji_animals", state.resolveLayer("emoji:last"))
        assertEquals("letters", state.resolveLayer(null))
        assertEquals("letters", state.resolveLayer(""))
    }

    @Test
    fun `symbol auto return is opt in and never applies to emoji or numbers`() {
        val state = KeyboardNavigationState()
        for (layer in listOf("symbols", "symbols_math")) {
            state.navigate(layer)
            assertNull(state.layerAfterDirectCommit(symbolAutoReturn = false))
            assertEquals("letters", state.layerAfterDirectCommit(symbolAutoReturn = true))
        }
        for (layer in listOf("symbols_emoji_face", "letters", "numbers", "editor")) {
            state.navigate(layer)
            assertNull(state.layerAfterDirectCommit(symbolAutoReturn = true))
        }
        state.navigate("symbols_math")
        for (panel in listOf(KeyboardPanel.CLIPBOARD, KeyboardPanel.PHRASES)) {
            state.navigate(panel = panel)
            assertNull(state.layerAfterDirectCommit(symbolAutoReturn = true))
            state.navigate()
            assertEquals("letters", state.layerAfterDirectCommit(symbolAutoReturn = true))
        }
    }

    @Test
    fun `orientation recreation restores all navigation state without resetting it`() {
        val previous = KeyboardNavigationState()
        previous.navigate("symbols_math")
        previous.navigate("symbols_emoji_animals")
        previous.navigate("editor")
        previous.navigate("numbers", panel = KeyboardPanel.PHRASES)
        previous.shift = KeyboardShiftState.LOCKED
        val restored = KeyboardNavigationState()

        restored.restoreFrom(previous)

        assertEquals("numbers", restored.layer)
        assertEquals(KeyboardPanel.PHRASES, restored.panel)
        assertEquals("editor", restored.returnLayer)
        assertEquals("symbols_math", restored.lastSymbolCategory)
        assertEquals("symbols_emoji_animals", restored.lastEmojiCategory)
        assertEquals(KeyboardShiftState.LOCKED, restored.shift)
    }
}
