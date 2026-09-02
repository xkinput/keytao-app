package ink.rea.keytao_app

import android.text.InputType
import android.view.inputmethod.EditorInfo
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
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

    @Test
    fun `password editors lose composition, clipboard and learning`() {
        val mode = KeytaoEditorPolicy.resolvePrivacyMode(
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_WEB_PASSWORD,
            imeOptions = EditorInfo.IME_ACTION_DONE,
        )

        assertFalse(mode.allowsComposing)
        assertFalse(mode.allowsLearning)
        assertFalse(mode.allowsClipboard)
        assertFalse(mode.allowsTextRecall)
    }

    @Test
    fun `numeric password variant counts as a password editor`() {
        assertTrue(
            KeytaoEditorPolicy.isPasswordEditor(
                InputType.TYPE_CLASS_NUMBER or InputType.TYPE_NUMBER_VARIATION_PASSWORD
            )
        )
    }

    @Test
    fun `incognito editors keep composing without learning or clipboard access`() {
        val mode = KeytaoEditorPolicy.resolvePrivacyMode(
            inputType = InputType.TYPE_CLASS_TEXT,
            imeOptions = EditorInfo.IME_ACTION_SEND or EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING,
        )

        assertTrue(mode.allowsComposing)
        assertFalse(mode.allowsLearning)
        assertFalse(mode.allowsClipboard)
    }

    @Test
    fun `no suggestions editors keep composing without learning`() {
        val mode = KeytaoEditorPolicy.resolvePrivacyMode(
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS,
            imeOptions = EditorInfo.IME_ACTION_NONE,
        )

        assertTrue(mode.allowsComposing)
        assertFalse(mode.allowsLearning)
    }

    @Test
    fun `password with no suggestions still loses composing`() {
        val mode = KeytaoEditorPolicy.resolvePrivacyMode(
            inputType = InputType.TYPE_CLASS_TEXT or
                InputType.TYPE_TEXT_VARIATION_PASSWORD or
                InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS,
            imeOptions = EditorInfo.IME_ACTION_DONE,
        )

        assertFalse(mode.allowsComposing)
    }

    @Test
    fun `a no suggestions flag outside a text editor changes nothing`() {
        val mode = KeytaoEditorPolicy.resolvePrivacyMode(
            inputType = InputType.TYPE_CLASS_NUMBER or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS,
            imeOptions = EditorInfo.IME_ACTION_NONE,
        )

        assertTrue(mode.allowsComposing)
        assertTrue(mode.allowsLearning)
    }

    @Test
    fun `ordinary text editors keep every capability`() {
        val mode = KeytaoEditorPolicy.resolvePrivacyMode(
            inputType = InputType.TYPE_CLASS_TEXT,
            imeOptions = EditorInfo.IME_ACTION_SEARCH,
        )

        assertTrue(mode.allowsComposing)
        assertTrue(mode.allowsLearning)
        assertTrue(mode.allowsClipboard)
        assertTrue(mode.allowsTextRecall)
    }

    @Test
    fun `numeric editors ask for the digits layer and direct input`() {
        assertEquals("numbers", KeytaoEditorPolicy.resolveInitialLayer(InputType.TYPE_CLASS_PHONE))
        assertEquals("numbers", KeytaoEditorPolicy.resolveInitialLayer(InputType.TYPE_CLASS_NUMBER))
        assertTrue(KeytaoEditorPolicy.isDirectInputEditor(InputType.TYPE_CLASS_DATETIME))
        assertNull(KeytaoEditorPolicy.resolveInitialLayer(InputType.TYPE_CLASS_TEXT))
        assertFalse(KeytaoEditorPolicy.isDirectInputEditor(InputType.TYPE_CLASS_TEXT))
    }

    @Test
    fun `enter label follows the declared action`() {
        assertEquals(
            "搜索",
            KeytaoEditorPolicy.resolveEnterLabel(
                inputType = InputType.TYPE_CLASS_TEXT,
                imeOptions = EditorInfo.IME_ACTION_SEARCH,
                actionLabel = null,
                forceNewline = false,
            ),
        )
    }

    @Test
    fun `enter label prefers the editor supplied label`() {
        assertEquals(
            "去支付",
            KeytaoEditorPolicy.resolveEnterLabel(
                inputType = InputType.TYPE_CLASS_TEXT,
                imeOptions = EditorInfo.IME_ACTION_GO,
                actionLabel = " 去支付 ",
                forceNewline = false,
            ),
        )
    }

    @Test
    fun `multiline editors keep the layout label`() {
        assertNull(
            KeytaoEditorPolicy.resolveEnterLabel(
                inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_MULTI_LINE,
                imeOptions = EditorInfo.IME_ACTION_SEND,
                actionLabel = null,
                forceNewline = false,
            ),
        )
    }

    @Test
    fun `an editor without an action never inherits a layout action word`() {
        assertEquals(
            "↵",
            KeytaoEditorPolicy.resolveEnterLabel(
                inputType = InputType.TYPE_CLASS_NUMBER,
                imeOptions = EditorInfo.IME_ACTION_NONE,
                actionLabel = null,
                forceNewline = false,
            ),
        )
        assertEquals(
            "↵",
            KeytaoEditorPolicy.resolveEnterLabel(
                inputType = InputType.TYPE_CLASS_TEXT,
                imeOptions = EditorInfo.IME_ACTION_SEND or EditorInfo.IME_FLAG_NO_ENTER_ACTION,
                actionLabel = "发送",
                forceNewline = false,
            ),
        )
    }

    @Test
    fun `a caret at the end of the preedit needs no explicit selection`() {
        assertNull(
            KeytaoEditorPolicy.resolveComposingCaret(
                preeditCharCount = 3,
                cursor = 3,
                selStart = 0,
                selEnd = 3,
            )
        )
    }

    @Test
    fun `a caret inside the preedit is reported in scalar offsets`() {
        assertEquals(
            1,
            KeytaoEditorPolicy.resolveComposingCaret(
                preeditCharCount = 3,
                cursor = 1,
                selStart = 1,
                selEnd = 3,
            )
        )
    }

    @Test
    fun `an out of range cursor falls back to a collapsed selection`() {
        assertEquals(
            2,
            KeytaoEditorPolicy.resolveComposingCaret(
                preeditCharCount = 4,
                cursor = -1,
                selStart = 2,
                selEnd = 2,
            )
        )
        assertNull(
            KeytaoEditorPolicy.resolveComposingCaret(
                preeditCharCount = 4,
                cursor = 9,
                selStart = 0,
                selEnd = 4,
            )
        )
    }

    @Test
    fun `an empty preedit never asks for a selection`() {
        assertNull(
            KeytaoEditorPolicy.resolveComposingCaret(
                preeditCharCount = 0,
                cursor = 0,
                selStart = 0,
                selEnd = 0,
            )
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
