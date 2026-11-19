package ink.rea.keytao_app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class KeytaoVoiceInputTest {
    @Test
    fun `partial replaces partial and ignores empty updates`() {
        val voice = KeytaoVoiceInput()
        val editor = Any()
        val token = voice.start(editor)
        assertEquals("hello", voice.partial(token, editor, "hello"))
        assertEquals("hello world", voice.partial(token, editor, "hello world"))
        assertNull(voice.partial(token, editor, ""))
        assertEquals("hello world", voice.finish(token, editor, ""))
    }

    @Test
    fun `unstable text is appended to the first recognition result`() {
        val voice = KeytaoVoiceInput()
        val editor = Any()
        val token = voice.start(editor)
        assertEquals("hello world", voice.partial(token, editor, "hello", " world"))
        assertEquals("world", voice.partial(token, editor, null, "world"))
    }

    @Test
    fun `final replaces partial and commits exactly once`() {
        val voice = KeytaoVoiceInput()
        val editor = Any()
        val token = voice.start(editor)
        voice.partial(token, editor, "hello")
        voice.transcribe()
        assertEquals("hello world", voice.finish(token, editor, "hello world"))
        assertEquals(KeytaoVoiceInput.State.IDLE, voice.state)
        assertNull(voice.finish(token, editor, "duplicate"))
        assertNull(voice.partial(token, editor, "late partial"))
        assertFalse(voice.cancel(editor))
    }

    @Test
    fun `empty or missing final commits the last partial`() {
        for (finalText in listOf("", null)) {
            val voice = KeytaoVoiceInput()
            val editor = Any()
            val token = voice.start(editor)
            voice.partial(token, editor, "last partial")
            assertEquals("last partial", voice.finish(token, editor, finalText))
            assertFalse(voice.cancel(editor))
        }
    }

    @Test
    fun `recognizers without partials commit only a nonempty final`() {
        val voice = KeytaoVoiceInput()
        val editor = Any()
        assertEquals("final only", voice.finish(voice.start(editor), editor, "final only"))
        assertNull(voice.finish(voice.start(editor), editor, ""))
        assertNull(voice.finish(voice.start(editor), editor, null))
        assertFalse(voice.cancel(editor))
    }

    @Test
    fun `cancel clears composing exactly once and rejects later results`() {
        val voice = KeytaoVoiceInput()
        val editor = Any()
        val token = voice.start(editor)
        voice.partial(token, editor, "unfinished")
        assertTrue(voice.cancel(editor))
        assertFalse(voice.cancel(editor))
        assertNull(voice.partial(token, editor, "late partial"))
        assertNull(voice.finish(token, editor, "late final"))
    }

    @Test
    fun `stale token cannot replace or commit the new session partial`() {
        val voice = KeytaoVoiceInput()
        val editor = Any()
        val staleToken = voice.start(editor)
        voice.partial(staleToken, editor, "old")
        assertTrue(voice.cancel(editor))
        val token = voice.start(editor)
        voice.partial(token, editor, "current")
        assertNull(voice.partial(staleToken, editor, "stale"))
        assertNull(voice.finish(staleToken, editor, "stale final"))
        assertEquals("current", voice.finish(token, editor, ""))
    }

    @Test
    fun `changed editor never receives partial final or composing clear`() {
        val voice = KeytaoVoiceInput()
        val editor = Any()
        val nextEditor = Any()
        val token = voice.start(editor)
        voice.partial(token, editor, "old editor")
        assertNull(voice.partial(token, nextEditor, "new editor"))
        assertFalse(voice.cancel(nextEditor))
        assertNull(voice.finish(token, nextEditor, "late final"))
        val nextToken = voice.start(editor)
        voice.partial(nextToken, editor, "old editor")
        assertNull(voice.finish(nextToken, nextEditor, "wrong editor"))
        assertTrue(voice.cancel(editor))
    }

    @Test
    fun `permission is permanently denied only after rationale becomes unavailable`() {
        assertTrue(KeytaoVoiceInput.shouldMarkPermissionPermanentlyDenied(true, true, false))
        assertFalse(KeytaoVoiceInput.shouldMarkPermissionPermanentlyDenied(true, false, false))
        assertFalse(KeytaoVoiceInput.shouldMarkPermissionPermanentlyDenied(true, true, true))
        assertFalse(KeytaoVoiceInput.shouldMarkPermissionPermanentlyDenied(true, null, false))
        assertFalse(KeytaoVoiceInput.shouldMarkPermissionPermanentlyDenied(false, true, false))
    }

    @Test
    fun `voice is offered only when every condition holds`() {
        for (available in listOf(false, true)) {
            for (privacyAllowed in listOf(false, true)) {
                for (composing in listOf(false, true)) {
                    for (configured in listOf(false, true)) {
                        for (denied in listOf(false, true)) {
                            assertEquals(
                                available && privacyAllowed && !composing && !configured && !denied,
                                KeytaoVoiceInput.isOffered(available, privacyAllowed, composing, configured, denied),
                            )
                        }
                    }
                }
            }
        }
    }

    @Test
    fun `voice moves from recording through transcribing to idle`() {
        val voice = KeytaoVoiceInput()
        assertEquals(KeytaoVoiceInput.State.IDLE, voice.state)
        voice.transcribe()
        assertEquals(KeytaoVoiceInput.State.IDLE, voice.state)
        voice.start()
        assertEquals(KeytaoVoiceInput.State.RECORDING, voice.state)
        voice.transcribe()
        assertEquals(KeytaoVoiceInput.State.TRANSCRIBING, voice.state)
        voice.cancel()
        assertEquals(KeytaoVoiceInput.State.IDLE, voice.state)
    }

    @Test
    fun `current callbacks can finish while transcribing`() {
        val voice = KeytaoVoiceInput()
        val committed = mutableListOf<String>()
        val token = voice.start()
        voice.runIfCurrent(token) { voice.transcribe() }
        assertEquals(KeytaoVoiceInput.State.TRANSCRIBING, voice.state)
        voice.runIfCurrent(token) {
            voice.cancel()
            committed.add("final result")
        }
        assertEquals(KeytaoVoiceInput.State.IDLE, voice.state)
        voice.runIfCurrent(token) { committed.add("duplicate result") }
        assertEquals(listOf("final result"), committed)
    }

    @Test
    fun `late callbacks cannot commit after cancellation or a new session`() {
        val voice = KeytaoVoiceInput()
        val committed = mutableListOf<String>()
        val oldToken = voice.start()
        assertEquals(KeytaoVoiceInput.State.RECORDING, voice.state)
        voice.cancel()
        voice.runIfCurrent(oldToken) {
            voice.transcribe()
            committed.add("late result")
        }
        assertEquals(KeytaoVoiceInput.State.IDLE, voice.state)
        val token = voice.start()
        voice.runIfCurrent(oldToken) {
            voice.transcribe()
            committed.add("previous field")
        }
        assertEquals(KeytaoVoiceInput.State.RECORDING, voice.state)
        voice.runIfCurrent(token) {
            voice.cancel()
            committed.add("final result")
        }
        voice.runIfCurrent(token) { committed.add("duplicate result") }
        assertEquals(listOf("final result"), committed)
    }
}
