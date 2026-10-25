package ink.rea.keytao_app

/**
 * The librime operations the ordering rules in [KeytaoRimeInput] need, kept as
 * an interface so those rules can be exercised without an InputMethodService.
 */
internal interface KeytaoRimeKeySink {
    /** Whether the editor still holds a composing region we put there. */
    val hostComposing: Boolean

    /** Pure mapping check; must not send a key or change the Rime session. */
    fun canProcessText(text: String): Boolean

    /** Keysym lookup plus `processKey`, or null when processing is unavailable. */
    fun processText(text: String): KeytaoImeState?

    /** Push a result to the editor: its commit first, then its preedit. */
    fun applyState(state: KeytaoImeState)

    /** Drop whatever librime still holds. */
    fun resetEngine()

    /** Send literal text straight to the editor. */
    fun commitDirect(text: String)
}

internal object KeytaoRimeInput {
    /** Rime only switches on release after it has observed the Shift press. */
    fun processSoloShift(
        keysym: Int,
        processKey: (Int, Int) -> KeytaoImeState,
        applyPress: (KeytaoImeState) -> Unit,
    ): KeytaoImeState {
        applyPress(processKey(keysym, 0))
        return processKey(keysym, AndroidKeyMapper.RIME_RELEASE_MASK)
    }

    /**
     * Flush what a declined key already produced before the key itself falls
     * through to the host.
     *
     * `ascii_composer`, and any composition that auto-commits on the current
     * key, commit first and reject the key afterwards. Handing the key to the
     * host without applying that result would drop the committed text and leave
     * a stale composing region behind. Mirrors iOS `applyRejectedKeyFallback`.
     */
    fun applyRejectedResult(sink: KeytaoRimeKeySink, result: KeytaoImeState) {
        if (result.committed.isEmpty() && !result.hasComposition && !sink.hostComposing) return
        sink.applyState(result)
    }

    /**
     * Feed a multi-character Rime code to librime one key at a time, applying
     * every result in order.
     *
     * Keeping only the last result loses any commit an earlier key produced —
     * a code that auto-commits halfway through would have that text overwritten
     * by the state the final key returns. Once a prefix has been consumed, a
     * fallback may insert only the remaining code points, never that prefix again.
     */
    fun feedSequence(sink: KeytaoRimeKeySink, sequence: String, fallbackText: String) {
        val characters = sequence.codePoints().toArray().map { String(Character.toChars(it)) }
        // Discover an unmappable suffix before a prefix can produce commits or
        // a composing region. Falling back halfway through would either replay
        // a committed prefix or replace a still-uncommitted one in the editor.
        if (characters.any { !sink.canProcessText(it) }) {
            sink.resetEngine()
            sink.commitDirect(fallbackText)
            return
        }
        fun remainingText(consumed: Int): String = if (consumed == 0) {
            fallbackText
        } else {
            sequence.substring(sequence.offsetByCodePoints(0, consumed))
        }
        for ((consumed, character) in characters.withIndex()) {
            val result = sink.processText(character)
            if (result == null) {
                sink.resetEngine()
                sink.commitDirect(remainingText(consumed))
                return
            }
            if (!result.accepted && !result.hasComposition) {
                applyRejectedResult(sink, result)
                sink.resetEngine()
                sink.commitDirect(remainingText(consumed))
                return
            }
            sink.applyState(result)
        }
    }
}
