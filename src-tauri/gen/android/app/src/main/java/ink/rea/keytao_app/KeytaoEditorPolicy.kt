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

internal object KeytaoEditorPolicy {
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

    private fun isMultilineTextEditor(inputType: Int): Boolean {
        return inputType and InputType.TYPE_MASK_CLASS == InputType.TYPE_CLASS_TEXT &&
            inputType and InputType.TYPE_TEXT_FLAG_MULTI_LINE != 0
    }

    private fun rangesOverlap(left: TextUnitRange, right: TextUnitRange): Boolean {
        return left.start < right.endExclusive && right.start < left.endExclusive
    }
}
