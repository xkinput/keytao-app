package ink.rea.keytao_app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class KeytaoVoiceInputTest {
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
