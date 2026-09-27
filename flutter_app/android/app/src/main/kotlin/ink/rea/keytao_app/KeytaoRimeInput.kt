package ink.rea.keytao_app

/**
 * The librime operations the ordering rules in [KeytaoRimeInput] need, kept as
 * an interface so those rules can be exercised without an InputMethodService.
 */
internal interface KeytaoRimeKeySink {
    /** Whether the editor still holds a composing region we put there. */
    val hostComposing: Boolean

    /** Keysym lookup plus `processKey`, or null when the text maps to no keysym. */
    fun processText(text: String): KeytaoImeState?

    /** Push a result to the editor: its commit first, then its preedit. */
    fun applyState(state: KeytaoImeState)

    /** Drop whatever librime still holds. */
    fun resetEngine()

    /** Send literal text straight to the editor. */
    fun commitDirect(text: String)
}

internal object KeytaoRimeInput {
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
        if (result.committed.isEmpty() && !sink.hostComposing) return
        sink.applyState(result)
    }

    /**
     * Feed a multi-character Rime code to librime one key at a time, applying
     * every result in order.
     *
     * Keeping only the last result loses any commit an earlier key produced —
     * a code that auto-commits halfway through would have that text overwritten
     * by the state the final key returns.
     */
    fun feedSequence(sink: KeytaoRimeKeySink, sequence: String, fallbackText: String) {
        for (codePoint in sequence.codePoints().toArray()) {
            val result = sink.processText(String(Character.toChars(codePoint)))
            if (result == null) {
                sink.resetEngine()
                sink.commitDirect(fallbackText)
                return
            }
            if (!result.accepted && !result.hasComposition) {
                applyRejectedResult(sink, result)
                sink.resetEngine()
                sink.commitDirect(fallbackText)
                return
            }
            sink.applyState(result)
        }
    }
}
