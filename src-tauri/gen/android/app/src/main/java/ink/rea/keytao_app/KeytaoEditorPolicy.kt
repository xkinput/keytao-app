package ink.rea.keytao_app

import android.text.InputType
import android.view.inputmethod.EditorInfo

internal enum class EnterDecisionType {
    CONFIRM_COMPOSITION,
    INSERT_NEWLINE,
    PERFORM_ACTION,
    SEND_ENTER_KEY,
}

internal data class EnterDecision(
    val type: EnterDecisionType,
    val actionId: Int = EditorInfo.IME_ACTION_UNSPECIFIED,
)

internal data class TextUnitRange(
    val start: Int,
    val endExclusive: Int,
)

/**
 * What the editor told us it does not want us to do with the text it receives.
 *
 * `password` comes from the inputType variations and requires direct input.
 * `noLearning` comes from `IME_FLAG_NO_PERSONALIZED_LEARNING` (incognito), and
 * `noSuggestions` from `TYPE_TEXT_FLAG_NO_SUGGESTIONS`; those two disable the
 * learning policy without disabling composition. Incognito additionally gates
 * clipboard history. This is safe for the bundled schemas because all of them
 * set `enable_user_dict:false`.
 */
internal data class InputPrivacyMode(
    val password: Boolean = false,
    val noLearning: Boolean = false,
    val noSuggestions: Boolean = false,
) {
    /**
     * Whether keys may build a Rime composition at all.
     *
     * Password variations are the only editors that refuse composition.
     * Incognito and no-suggestion fields keep normal Chinese input while
     * disabling learning; incognito also disables clipboard memory. Core cannot
     * enforce learning off while composing for a third-party schema with a user
     * dictionary; the bundled schemas disable their user dictionaries.
     */
    val allowsComposing: Boolean get() = !password

    /** Whether Rime may record what was typed into the user dictionary. */
    val allowsLearning: Boolean get() = !password && !noLearning && !noSuggestions

    /** Whether clipboard contents may be remembered or previewed on the keyboard. */
    val allowsClipboard: Boolean get() = !password && !noLearning

    /** Whether committed text may be kept in memory for backspace restore. */
    val allowsTextRecall: Boolean get() = !password
}

internal object KeytaoEditorPolicy {
    /** `EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING`, inlined for minSdk 24. */
    private const val imeFlagNoPersonalizedLearning = 0x1000000

    /** Caption for an Enter key that carries no editor action. */
    private const val plainEnterLabel = "↵"

    fun resolvePrivacyMode(inputType: Int, imeOptions: Int): InputPrivacyMode {
        return InputPrivacyMode(
            password = isPasswordEditor(inputType),
            noLearning = imeOptions and imeFlagNoPersonalizedLearning != 0,
            noSuggestions = inputType and InputType.TYPE_MASK_CLASS == InputType.TYPE_CLASS_TEXT &&
                inputType and InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS != 0,
        )
    }

    fun isPasswordEditor(inputType: Int): Boolean {
        val variation = inputType and InputType.TYPE_MASK_VARIATION
        return when (inputType and InputType.TYPE_MASK_CLASS) {
            InputType.TYPE_CLASS_TEXT -> variation == InputType.TYPE_TEXT_VARIATION_PASSWORD ||
                variation == InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD ||
                variation == InputType.TYPE_TEXT_VARIATION_WEB_PASSWORD

            InputType.TYPE_CLASS_NUMBER -> variation == InputType.TYPE_NUMBER_VARIATION_PASSWORD
            else -> false
        }
    }

    /**
     * The layer an editor asks for through inputType, or null when it wants the
     * ordinary text keyboard. Numeric editors filter out anything Rime would
     * produce, so they must start on (and stay on) the digits layer.
     */
    fun resolveInitialLayer(inputType: Int): String? {
        return when (inputType and InputType.TYPE_MASK_CLASS) {
            InputType.TYPE_CLASS_NUMBER,
            InputType.TYPE_CLASS_PHONE,
            InputType.TYPE_CLASS_DATETIME,
            -> "numbers"

            else -> null
        }
    }

    /** Numeric editors drop non-digit text, so composition must not run there. */
    fun isDirectInputEditor(inputType: Int): Boolean = resolveInitialLayer(inputType) != null

    /**
     * What the Enter key should read, following `EditorInfo.actionLabel` first and
     * the `IME_ACTION_*` code second.
     *
     * Null means "keep whatever the layout says", which is only right when the
     * key really does insert a line break. Everywhere else the caption has to
     * come from the editor, otherwise a layout that hard-codes e.g. `发送` on its
     * digits layer would claim an action the editor never asked for.
     */
    fun resolveEnterLabel(
        inputType: Int,
        imeOptions: Int,
        actionLabel: CharSequence?,
        forceNewline: Boolean,
    ): String? {
        if (forceNewline || isMultilineTextEditor(inputType)) return null
        if (imeOptions and EditorInfo.IME_FLAG_NO_ENTER_ACTION != 0) return plainEnterLabel
        actionLabel?.toString()?.trim()?.takeIf { it.isNotEmpty() }?.let { return it }
        return when (imeOptions and EditorInfo.IME_MASK_ACTION) {
            EditorInfo.IME_ACTION_GO -> "前往"
            EditorInfo.IME_ACTION_SEARCH -> "搜索"
            EditorInfo.IME_ACTION_SEND -> "发送"
            EditorInfo.IME_ACTION_NEXT -> "下一项"
            EditorInfo.IME_ACTION_DONE -> "完成"
            EditorInfo.IME_ACTION_PREVIOUS -> "上一项"
            else -> plainEnterLabel
        }
    }

    fun resolveEnterDecision(
        hasComposition: Boolean,
        forceNewline: Boolean,
        inputType: Int,
        imeOptions: Int,
        actionId: Int,
        hasActionLabel: Boolean,
    ): EnterDecision {
        if (hasComposition) {
            return EnterDecision(EnterDecisionType.CONFIRM_COMPOSITION)
        }
        if (forceNewline || isMultilineTextEditor(inputType)) {
            return EnterDecision(EnterDecisionType.INSERT_NEWLINE)
        }
        if (
            inputType and InputType.TYPE_MASK_CLASS == InputType.TYPE_NULL ||
            imeOptions and EditorInfo.IME_FLAG_NO_ENTER_ACTION != 0
        ) {
            return EnterDecision(EnterDecisionType.SEND_ENTER_KEY)
        }
        if (hasActionLabel && actionId != EditorInfo.IME_ACTION_UNSPECIFIED) {
            return EnterDecision(EnterDecisionType.PERFORM_ACTION, actionId)
        }
        val action = imeOptions and EditorInfo.IME_MASK_ACTION
        return when (action) {
            EditorInfo.IME_ACTION_NONE,
            EditorInfo.IME_ACTION_UNSPECIFIED,
            -> EnterDecision(EnterDecisionType.SEND_ENTER_KEY)

            else -> EnterDecision(EnterDecisionType.PERFORM_ACTION, action)
        }
    }

    /**
     * Where the caret belongs inside the preedit, in Unicode scalar offsets, or
     * null when it already sits at the end — which is where
     * `setComposingText(text, 1)` leaves it, so nothing has to be done.
     *
     * `selStart`/`selEnd` are librime's active segment, not a caret: for an
     * ordinary unconverted composition they span the whole preedit. They only
     * move the caret when the schema put it outside them, which is how a
     * schema signals "the caret is not at the end".
     */
    fun resolveComposingCaret(
        preeditCharCount: Int,
        cursor: Int,
        selStart: Int,
        selEnd: Int,
    ): Int? {
        if (preeditCharCount <= 0) return null
        val caret = when {
            cursor in 0..preeditCharCount -> cursor
            selStart in 0..preeditCharCount && selStart == selEnd -> selStart
            else -> return null
        }
        if (caret >= preeditCharCount) return null
        return caret
    }

    fun mergeAtomicTextRanges(
        graphemeRanges: List<TextUnitRange>,
        styledRanges: List<TextUnitRange>,
    ): List<TextUnitRange> {
        if (graphemeRanges.isEmpty() || styledRanges.isEmpty()) return graphemeRanges

        val textStart = graphemeRanges.first().start
        val textEnd = graphemeRanges.last().endExclusive
        val merged = graphemeRanges.toMutableList()
        styledRanges
            .asSequence()
            .map {
                TextUnitRange(
                    start = it.start.coerceIn(textStart, textEnd),
                    endExclusive = it.endExclusive.coerceIn(textStart, textEnd),
                )
            }
            .filter { it.start < it.endExclusive }
            .sortedBy(TextUnitRange::start)
            .forEach { styledRange ->
                val firstIndex = merged.indexOfFirst { range -> rangesOverlap(range, styledRange) }
                if (firstIndex < 0) return@forEach
                val lastIndex = merged.indexOfLast { range -> rangesOverlap(range, styledRange) }
                val combined = TextUnitRange(
                    start = minOf(merged[firstIndex].start, styledRange.start),
                    endExclusive = maxOf(merged[lastIndex].endExclusive, styledRange.endExclusive),
                )
                repeat(lastIndex - firstIndex + 1) {
                    merged.removeAt(firstIndex)
                }
                merged.add(firstIndex, combined)
            }
        return merged
    }

    fun isMultilineTextEditor(inputType: Int): Boolean {
        return inputType and InputType.TYPE_MASK_CLASS == InputType.TYPE_CLASS_TEXT &&
            inputType and InputType.TYPE_TEXT_FLAG_MULTI_LINE != 0
    }

    private fun rangesOverlap(left: TextUnitRange, right: TextUnitRange): Boolean {
        return left.start < right.endExclusive && right.start < left.endExclusive
    }
}
