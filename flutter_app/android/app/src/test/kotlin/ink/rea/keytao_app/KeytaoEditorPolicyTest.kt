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
    fun `only a complete non nested trailing bracket token delegates deletion`() {
        for ((text, expected) in listOf(
            "[微笑]" to TextUnitRange(0, 4),
            "[OK]" to TextUnitRange(0, 4),
            "abc[哭]" to TextUnitRange(3, 6),
            "[微笑][哭]" to TextUnitRange(4, 7),
            "[abcdefghijkl]" to TextUnitRange(0, 14),
        )) {
            assertEquals(text, expected, KeytaoEditorPolicy.trailingBracketTokenRange(text))
        }
        for (text in listOf(
            "", "[]", "[a[b]", "[超过十二个字符的很长很长的名字]",
            "[abcdefghijklm]", "[微笑]abc", "abc]", "[a\rb]", "[a\nb]",
        )) {
            assertNull(text, KeytaoEditorPolicy.trailingBracketTokenRange(text))
        }
    }

    @Test
    fun `host backspace recall requires a proven bounded backward cursor difference`() {
        assertEquals("[微笑]", KeytaoEditorPolicy.reconcileHostBackspace("abc[微笑]", 7, 3, "abc", "", 1))
        assertEquals("]", KeytaoEditorPolicy.reconcileHostBackspace("abc[微笑]", 7, 6, "abc[微笑", "", 1))
        assertEquals("[微笑]", KeytaoEditorPolicy.reconcileHostBackspace("x[微笑]", 100, 96, "prefix", "", 1))
        assertNull(KeytaoEditorPolicy.reconcileHostBackspace("[微笑]", 100, 95, "prefix", "", 1))
        assertNull(KeytaoEditorPolicy.reconcileHostBackspace("abc[微笑]", 7, 7, "abc[微笑]", "", 1))
        assertNull(KeytaoEditorPolicy.reconcileHostBackspace("abc[微笑]", 7, 8, "abc[微笑]x", "", 1))
        assertNull(KeytaoEditorPolicy.reconcileHostBackspace("[微笑]", 3, -1, "", "", 1))
        assertNull(KeytaoEditorPolicy.reconcileHostBackspace("", 7, 6, "prefix", "", 1))
    }

    @Test
    fun `ignored host delete followed by a cursor move must not create restore text`() {
        assertNull(KeytaoEditorPolicy.reconcileHostBackspace("hello[ok]", 9, 4, "hell", "o[ok]", 1))
        assertNull(KeytaoEditorPolicy.reconcileHostBackspace("hello[ok]", 9, 8, "hello[ok", "]", 1))
        assertNull(KeytaoEditorPolicy.reconcileHostBackspace("hello[ok]", 9, 4, "hell", "o[ok]tail", 1))
    }

    @Test
    fun `host deletion proof compares overlapping context windows`() {
        assertEquals("[ok]", KeytaoEditorPolicy.reconcileHostBackspace("lo[ok]", 100, 96, "hello", "tail", 1))
        assertEquals("[ok]", KeytaoEditorPolicy.reconcileHostBackspace("hello[ok]", 100, 96, "lo", "tail", 1))
    }

    @Test
    fun `host deletion proof rejects mismatched or stale context`() {
        assertNull(KeytaoEditorPolicy.reconcileHostBackspace("hello[ok]", 9, 5, "other", "", 1))
        assertNull(KeytaoEditorPolicy.reconcileHostBackspace("hello[ok]", 9, 5, "hello[ok]", "", 1))
        assertNull(KeytaoEditorPolicy.reconcileHostBackspace("hello[ok]", 9, 8, "hello[oX", "", 1))
    }

    @Test
    fun `host deletion proof requires readable context and overlap away from buffer start`() {
        assertNull(KeytaoEditorPolicy.reconcileHostBackspace("hello[ok]", 9, 5, null, "", 1))
        assertNull(KeytaoEditorPolicy.reconcileHostBackspace("hello[ok]", 9, 5, "hello", null, 1))
        assertNull(KeytaoEditorPolicy.reconcileHostBackspace("hello[ok]", 9, 5, "", "", 1))
        assertNull(KeytaoEditorPolicy.reconcileHostBackspace("[ok]", 100, 96, "hello", "", 1))
    }

    @Test
    fun `host can delete the whole token at buffer start`() {
        assertEquals("[ok]", KeytaoEditorPolicy.reconcileHostBackspace("[ok]", 4, 0, "", "", 1))
        assertEquals("[ok]", KeytaoEditorPolicy.reconcileHostBackspace("[ok]", 4, 0, "", "tail", 1))
        assertNull(KeytaoEditorPolicy.reconcileHostBackspace("[ok]", 4, 0, "", "[ok]", 1))
    }

    @Test
    fun `only one outstanding host delete is eligible for reconciliation`() {
        for (outstanding in listOf(-1, 0, 2, 3)) {
            assertNull(KeytaoEditorPolicy.reconcileHostBackspace("ab[c]", 5, 4, "ab[c", "", outstanding))
        }
        assertEquals("]", KeytaoEditorPolicy.reconcileHostBackspace("ab[c]", 5, 4, "ab[c", "", 1))
    }

    @Test
    fun `late repeat updates must not turn stale bracket snapshots into restore units`() {
        val restoreStack = mutableListOf<String>()
        // The second injected DEL invalidates recall for the whole outstanding
        // burst, including coalesced callbacks and its final delayed update.
        for ((oldCursor, newCursor, beforeCursor) in listOf(
            Triple(5, 4, "ab[c"), Triple(4, 3, "ab["), Triple(3, 2, "ab"),
        )) {
            KeytaoEditorPolicy.reconcileHostBackspace(
                "ab[c]", oldCursor, newCursor, beforeCursor, "", 2,
            )?.let(restoreStack::add)
        }
        assertTrue(restoreStack.isEmpty())
        assertEquals("ab", "ab" + restoreStack.asReversed().joinToString(""))
    }

    @Test
    fun `repeated following text and truncated cursor move evidence remain ambiguous`() {
        assertNull(KeytaoEditorPolicy.reconcileHostBackspace("hello[ok]", 9, 5, "hello", "[ok]", 1))
        assertNull(KeytaoEditorPolicy.reconcileHostBackspace("hello[ok]", 9, 4, "hell", "o[", 1))
    }

    @Test
    fun `host deletion proof uses UTF16 offsets with supplementary characters`() {
        assertEquals("[微笑]", KeytaoEditorPolicy.reconcileHostBackspace("😀[微笑]", 6, 2, "😀", "", 1))
        assertEquals("]", KeytaoEditorPolicy.reconcileHostBackspace("😀[微笑]", 6, 5, "😀[微笑", "", 1))
    }

    @Test
    fun `null input class sends a delete key even with input flags`() {
        for (inputType in listOf(InputType.TYPE_NULL, InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS)) {
            assertEquals(BackspaceDecision.SEND_DEL_KEY, backspace(inputType = inputType))
        }
    }

    @Test
    fun `unknown cursor sends a delete key in a text editor`() {
        assertEquals(BackspaceDecision.SEND_DEL_KEY, backspace(hasKnownCursor = false))
    }

    @Test
    fun `known cursor in a text editor keeps buffer deletion`() {
        assertEquals(BackspaceDecision.DELETE_BEFORE_CURSOR, backspace())
    }

    @Test
    fun `known selection in a text editor is deleted first`() {
        assertEquals(BackspaceDecision.DELETE_SELECTION, backspace(hasSelection = true))
    }

    @Test
    fun `bufferless editors send delete even if a selection is reported`() {
        assertEquals(
            BackspaceDecision.SEND_DEL_KEY,
            backspace(inputType = InputType.TYPE_NULL, hasSelection = true),
        )
        assertEquals(
            BackspaceDecision.SEND_DEL_KEY,
            backspace(hasKnownCursor = false, hasSelection = true),
        )
    }

    @Test
    fun `composition keeps backspace in the engine before any host decision`() {
        for (inputType in listOf(InputType.TYPE_NULL, InputType.TYPE_CLASS_TEXT)) {
            for (hasKnownCursor in listOf(false, true)) {
                for (hasSelection in listOf(false, true)) {
                    assertEquals(
                        BackspaceDecision.ENGINE,
                        backspace(
                            hasComposition = true,
                            inputType = inputType,
                            hasKnownCursor = hasKnownCursor,
                            hasSelection = hasSelection,
                        ),
                    )
                }
            }
        }
    }

    @Test
    fun `password editors follow cursor and selection availability`() {
        val inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
        assertEquals(BackspaceDecision.DELETE_BEFORE_CURSOR, backspace(inputType = inputType))
        assertEquals(
            BackspaceDecision.DELETE_SELECTION,
            backspace(inputType = inputType, hasSelection = true),
        )
        assertEquals(
            BackspaceDecision.SEND_DEL_KEY,
            backspace(inputType = inputType, hasKnownCursor = false),
        )
        assertFalse(KeytaoEditorPolicy.resolvePrivacyMode(inputType, 0).allowsTextRecall)
    }

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

    private fun backspace(
        hasComposition: Boolean = false,
        inputType: Int = InputType.TYPE_CLASS_TEXT,
        hasKnownCursor: Boolean = true,
        hasSelection: Boolean = false,
    ): BackspaceDecision = KeytaoEditorPolicy.resolveBackspaceDecision(
        hasComposition = hasComposition,
        inputType = inputType,
        hasKnownCursor = hasKnownCursor,
        hasSelection = hasSelection,
    )

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
