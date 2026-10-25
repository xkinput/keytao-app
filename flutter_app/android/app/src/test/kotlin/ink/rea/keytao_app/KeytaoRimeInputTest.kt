package ink.rea.keytao_app

import org.junit.Assert.assertEquals
import org.junit.Test

class KeytaoRimeInputTest {
    @Test
    fun `solo Shift sends a press before release and preserves an intermediate commit`() {
        val log = mutableListOf<String>()
        val released = accepted(committed = "remaining").copy(asciiMode = true)
        val result = KeytaoRimeInput.processSoloShift(
            AndroidKeyMapper.XK_SHIFT_L,
            processKey = { keysym, modifiers ->
                assertEquals(AndroidKeyMapper.XK_SHIFT_L, keysym)
                log.add("process:$modifiers")
                if (modifiers == 0) accepted(committed = "previous") else released
            },
            applyPress = { log.add("apply:${it.committed}") },
        )
        assertEquals(
            listOf("process:0", "apply:previous", "process:${AndroidKeyMapper.RIME_RELEASE_MASK}"),
            log,
        )
        assertEquals(released, result)
    }

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
                "commitDirect:b",
            ),
            sink.log,
        )
    }

    @Test
    fun `a code whose first key maps to no keysym falls back untouched`() {
        val sink = RecordingSink("☃" to null).apply { unmappableTexts.add("☃") }

        KeytaoRimeInput.feedSequence(sink, "☃", "☃")

        assertEquals(listOf("reset", "commitDirect:☃"), sink.log)
    }

    @Test
    fun `an unmappable suffix is found before a prefix can compose or commit`() {
        for (prefixResult in listOf(accepted(preedit = "a"), accepted(committed = "a"))) {
            val sink = RecordingSink("a" to prefixResult).apply { unmappableTexts.add("🦊") }

            KeytaoRimeInput.feedSequence(sink, "a🦊b", "custom fallback")

            assertEquals(listOf("a", "🦊"), sink.mappingChecks)
            assertEquals(listOf("reset", "commitDirect:custom fallback"), sink.log)
            assertEquals("custom fallback", sink.committed.toString())
        }
    }

    @Test
    fun `a rejected suffix never repeats an already committed prefix`() {
        val sink = RecordingSink(
            "a" to accepted(committed = "a"),
            "b" to rejected(),
        )

        KeytaoRimeInput.feedSequence(sink, "ab", "ab")

        assertEquals(
            listOf("process:a", "apply:a/", "process:b", "reset", "commitDirect:b"),
            sink.log,
        )
        assertEquals("ab", sink.committed.toString())
    }

    @Test
    fun `rejected and unmapped suffixes retain complete Unicode code points`() {
        for (result in listOf(null, rejected(committed = "旧"))) {
            val sink = RecordingSink(
                "🙂" to accepted(committed = "🙂"),
                "🦊" to result,
            )

            KeytaoRimeInput.feedSequence(sink, "🙂🦊b", "custom fallback")

            val expected = mutableListOf("process:🙂", "apply:🙂/", "process:🦊")
            if (result != null) expected.add("apply:旧/")
            expected.addAll(listOf("reset", "commitDirect:🦊b"))
            assertEquals(expected, sink.log)
            assertEquals(if (result == null) "🙂🦊b" else "🙂旧🦊b", sink.committed.toString())
        }
    }

    @Test
    fun `the first rejected key keeps the declared fallback after flushing an earlier commit`() {
        val sink = RecordingSink("a" to rejected(committed = "旧"))

        KeytaoRimeInput.feedSequence(sink, "ab", "、")

        assertEquals(listOf("process:a", "apply:旧/", "reset", "commitDirect:、"), sink.log)
        assertEquals("旧、", sink.committed.toString())
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
    fun `a rejected hardware key retains its composition before reaching the host`() {
        val sink = RecordingSink()

        KeytaoRimeInput.applyRejectedResult(sink, rejected(preedit = "aa"))

        assertEquals(listOf("apply:/aa"), sink.log)
        assertEquals(true, sink.hostComposing)
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
        val committed = StringBuilder()
        val mappingChecks = mutableListOf<String>()
        val unmappableTexts = mutableSetOf<String>()
        override var hostComposing: Boolean = false
        private val scriptedResults = scripted.toMap()

        override fun canProcessText(text: String): Boolean {
            mappingChecks.add(text)
            return text !in unmappableTexts
        }

        override fun processText(text: String): KeytaoImeState? {
            log.add("process:$text")
            return scriptedResults[text]
        }

        override fun applyState(state: KeytaoImeState) {
            log.add("apply:${state.committed}/${state.preedit}")
            committed.append(state.committed)
            hostComposing = state.preedit.isNotEmpty()
        }

        override fun resetEngine() {
            log.add("reset")
        }

        override fun commitDirect(text: String) {
            log.add("commitDirect:$text")
            committed.append(text)
            hostComposing = false
        }
    }
}
