package ink.rea.keytao_app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class KeytaoEditorUpdateTest {
    @Test
    fun `a hardware editor reloads before restoring its policy composition and mode`() {
        val engine = RecordingEngine()
        val result = KeytaoEditorUpdate.run(engine, reloadIfNeeded = true, clearComposition = true)

        assertEquals(listOf("reload", "policy", "clear", "mode", "state"), engine.calls)
        assertEquals("current preedit", result!!.preedit)
        assertEquals("", result.committed)
        assertFalse(result.accepted)
    }

    @Test
    fun `a repeated view update keeps its composition while rechecking reload`() {
        val engine = RecordingEngine()
        KeytaoEditorUpdate.run(engine, reloadIfNeeded = true, clearComposition = false)
        assertEquals(listOf("reload", "policy", "mode", "state"), engine.calls)
    }

    @Test
    fun `failed reload policy or mode work cannot publish a ready state`() {
        for ((step, expected) in listOf(
            "reload" to listOf("reload"),
            "policy" to listOf("reload", "policy"),
            "mode" to listOf("reload", "policy", "clear", "mode"),
        )) {
            val engine = RecordingEngine().apply { failAt = step }
            val update = runCatching { KeytaoEditorUpdate.run(engine, true, true) }
            assertTrue(update.isFailure)
            assertEquals(expected, engine.calls)
            assertFalse(engine.calls.contains("state"))
        }
    }

    @Test
    fun `an unavailable engine never reads or clears a nonexistent session`() {
        val engine = RecordingEngine().apply { ready = false }
        assertNull(KeytaoEditorUpdate.run(engine, reloadIfNeeded = true, clearComposition = true))
        assertEquals(listOf("reload", "policy", "mode"), engine.calls)
    }

    private class RecordingEngine : KeytaoEditorEngineUpdate {
        override var ready = true
        val calls = mutableListOf<String>()
        var failAt: String? = null
        private fun step(name: String) {
            calls.add(name)
            check(failAt != name) { "Transient $name failure" }
        }
        override fun reloadIfNeeded() = step("reload")
        override fun applyPolicy() = step("policy")
        override fun clearComposition() = step("clear")
        override fun restoreMode() = step("mode")
        override fun state(): KeytaoImeState {
            step("state")
            return KeytaoImeState(preedit = "current preedit", committed = "old commit", accepted = true)
        }
    }
}
