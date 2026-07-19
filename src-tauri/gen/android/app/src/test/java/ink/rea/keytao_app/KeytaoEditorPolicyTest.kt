package ink.rea.keytao_app

import android.text.InputType
import android.view.inputmethod.EditorInfo
import org.junit.Assert.assertEquals
import org.junit.Test

class KeytaoEditorPolicyTest {
    @Test
    fun `composition confirmation wins over fixed newline`() {
        val decision = resolve(
            hasComposition = true,
            forceNewline = true,
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_MULTI_LINE,
            imeOptions = EditorInfo.IME_ACTION_NONE,
        )

        assertEquals(EnterDecisionType.CONFIRM_COMPOSITION, decision.type)
    }

    @Test
    fun `multiline editor inserts newline even when it advertises send`() {
        val decision = resolve(
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_MULTI_LINE,
            imeOptions = EditorInfo.IME_ACTION_SEND,
        )

        assertEquals(EnterDecisionType.INSERT_NEWLINE, decision.type)
    }

    @Test
    fun `single line editor performs its action`() {
        val decision = resolve(
            inputType = InputType.TYPE_CLASS_TEXT,
            imeOptions = EditorInfo.IME_ACTION_SEND,
        )

        assertEquals(EnterDecisionType.PERFORM_ACTION, decision.type)
        assertEquals(EditorInfo.IME_ACTION_SEND, decision.actionId)
    }

    @Test
    fun `ime multiline display flag does not make a single line editor multiline`() {
        val decision = resolve(
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_IME_MULTI_LINE,
            imeOptions = EditorInfo.IME_ACTION_DONE,
        )

        assertEquals(EnterDecisionType.PERFORM_ACTION, decision.type)
        assertEquals(EditorInfo.IME_ACTION_DONE, decision.actionId)
    }

    @Test
    fun `fixed newline inserts newline without composition`() {
        val decision = resolve(
            forceNewline = true,
            inputType = InputType.TYPE_CLASS_TEXT,
            imeOptions = EditorInfo.IME_ACTION_SEND,
        )

        assertEquals(EnterDecisionType.INSERT_NEWLINE, decision.type)
    }

    @Test
    fun `no enter action flag sends enter key to the host`() {
        val decision = resolve(
            inputType = InputType.TYPE_CLASS_TEXT,
            imeOptions = EditorInfo.IME_ACTION_SEND or EditorInfo.IME_FLAG_NO_ENTER_ACTION,
        )

        assertEquals(EnterDecisionType.SEND_ENTER_KEY, decision.type)
    }

    @Test
    fun `custom action label uses its action id`() {
        val decision = resolve(
            inputType = InputType.TYPE_CLASS_TEXT,
            imeOptions = EditorInfo.IME_ACTION_UNSPECIFIED,
            actionId = 42,
            hasActionLabel = true,
        )

        assertEquals(EnterDecisionType.PERFORM_ACTION, decision.type)
        assertEquals(42, decision.actionId)
    }

    @Test
    fun `styled replacement span merges adjacent grapheme ranges`() {
        val ranges = KeytaoEditorPolicy.mergeAtomicTextRanges(
            graphemeRanges = listOf(
                TextUnitRange(0, 1),
                TextUnitRange(1, 2),
                TextUnitRange(2, 3),
                TextUnitRange(3, 4),
            ),
            styledRanges = listOf(TextUnitRange(0, 3)),
        )

        assertEquals(
            listOf(TextUnitRange(0, 3), TextUnitRange(3, 4)),
            ranges,
        )
    }

    @Test
    fun `separate styled ranges stay as separate deletion units`() {
        val ranges = KeytaoEditorPolicy.mergeAtomicTextRanges(
            graphemeRanges = listOf(
                TextUnitRange(0, 1),
                TextUnitRange(1, 2),
                TextUnitRange(2, 3),
                TextUnitRange(3, 4),
            ),
            styledRanges = listOf(TextUnitRange(0, 2), TextUnitRange(2, 4)),
        )

        assertEquals(
            listOf(TextUnitRange(0, 2), TextUnitRange(2, 4)),
            ranges,
        )
    }

    private fun resolve(
        hasComposition: Boolean = false,
        forceNewline: Boolean = false,
        inputType: Int,
        imeOptions: Int,
        actionId: Int = EditorInfo.IME_ACTION_UNSPECIFIED,
        hasActionLabel: Boolean = false,
    ): EnterDecision = KeytaoEditorPolicy.resolveEnterDecision(
        hasComposition = hasComposition,
        forceNewline = forceNewline,
        inputType = inputType,
        imeOptions = imeOptions,
        actionId = actionId,
        hasActionLabel = hasActionLabel,
    )
}
