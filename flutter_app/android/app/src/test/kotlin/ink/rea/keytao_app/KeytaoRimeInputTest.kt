package ink.rea.keytao_app

import org.junit.Assert.assertEquals
import org.junit.Test

class KeytaoRimeInputTest {
    @Test
    fun `every result of a multi character code is applied in order`() {
        val sink = RecordingSink(
            "a" to accepted(preedit = "a"),
            "b" to accepted(committed = "啊", preedit = "b"),
            "c" to accepted(preedit = "bc"),
        )

        KeytaoRimeInput.feedSequence(sink, "abc", "abc")

        assertEquals(
            listOf(
                "process:a",
                "apply:/a",
                "process:b",
                "apply:啊/b",
                "process:c",
                "apply:/bc",
            ),
            sink.log,
        )
    }

    @Test
    fun `two commits inside one code both reach the editor`() {
        val sink = RecordingSink(
            "a" to accepted(committed = "啊", preedit = ""),
            "b" to accepted(committed = "吧", preedit = "b"),
        )

        KeytaoRimeInput.feedSequence(sink, "ab", "ab")

        assertEquals(
            listOf("process:a", "apply:啊/", "process:b", "apply:吧/b"),
            sink.log,
        )
    }

    @Test
    fun `a rejected key in the middle of a code flushes its commit before the fallback`() {
        val sink = RecordingSink(
            "a" to accepted(preedit = "a"),
            "b" to rejected(committed = "啊"),
        )

        KeytaoRimeInput.feedSequence(sink, "ab", "、")

        assertEquals(
            listOf(
                "process:a",
                "apply:/a",
                "process:b",
                "apply:啊/",
                "reset",
                "commitDirect:、",
            ),
            sink.log,
        )
    }

    @Test
    fun `a code whose first key maps to no keysym falls back untouched`() {
        val sink = RecordingSink("☃" to null)

        KeytaoRimeInput.feedSequence(sink, "☃", "☃")

        assertEquals(listOf("process:☃", "reset", "commitDirect:☃"), sink.log)
    }

    @Test
    fun `a rejected key hands over its commit before the host sees the key`() {
        val sink = RecordingSink()

        KeytaoRimeInput.applyRejectedResult(sink, rejected(committed = "啊"))

        assertEquals(listOf("apply:啊/"), sink.log)
    }

    @Test
    fun `a rejected key with nothing pending does not touch the editor`() {
        val sink = RecordingSink()

        KeytaoRimeInput.applyRejectedResult(sink, rejected())

        assertEquals(emptyList<String>(), sink.log)
    }

    @Test
    fun `a rejected key clears the composing region the editor still shows`() {
        val sink = RecordingSink().apply { hostComposing = true }

        KeytaoRimeInput.applyRejectedResult(sink, rejected())

        assertEquals(listOf("apply:/"), sink.log)
    }

    private fun accepted(committed: String = "", preedit: String = "") =
        KeytaoImeState(preedit = preedit, committed = committed, accepted = true)

    private fun rejected(committed: String = "", preedit: String = "") =
        KeytaoImeState(preedit = preedit, committed = committed, accepted = false)

    private class RecordingSink(
        vararg scripted: Pair<String, KeytaoImeState?>,
    ) : KeytaoRimeKeySink {
        val log = mutableListOf<String>()
        override var hostComposing: Boolean = false
        private val scriptedResults = scripted.toMap()

        override fun processText(text: String): KeytaoImeState? {
            log.add("process:$text")
            return scriptedResults[text]
        }

        override fun applyState(state: KeytaoImeState) {
            log.add("apply:${state.committed}/${state.preedit}")
            hostComposing = state.preedit.isNotEmpty()
        }

        override fun resetEngine() {
            log.add("reset")
        }

        override fun commitDirect(text: String) {
            log.add("commitDirect:$text")
            hostComposing = false
        }
    }
}
