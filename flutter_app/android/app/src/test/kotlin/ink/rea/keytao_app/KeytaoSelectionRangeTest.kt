package ink.rea.keytao_app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class KeytaoSelectionRangeTest {
    @Test
    fun `word selection stops at punctuation and spaces and joins Latin digits underscore`() {
        assertEquals(-2 to 2, selectionRangeAroundCursor("前句，中文", "测试。下句"))
        assertEquals(-4 to 3, selectionRangeAroundCursor("hi foo_", "123 bye"))
        assertEquals(-4 to 0, selectionRangeAroundCursor("test", " next"))
        assertNull(selectionRangeAroundCursor(" ", "。"))
    }

    @Test
    fun `caret at a word boundary prefers the word on its right`() {
        assertEquals(0 to 4, selectionRangeAroundCursor("中文", "word!"))
        assertEquals(0 to 2, selectionRangeAroundCursor("word", "中文。"))
    }

    @Test
    fun `CJK selection caps code points at 32 while keeping both sides around the caret`() {
        assertEquals(-16 to 16, selectionRangeAroundCursor("字".repeat(40), "字".repeat(40)))
        assertEquals(-30 to 2, selectionRangeAroundCursor("字".repeat(40), "文字。"))
        assertEquals(0 to 32, selectionRangeAroundCursor("", "字".repeat(40)))
    }

    @Test
    fun `supplementary Han characters keep UTF16 offsets and count as one CJK character`() {
        val han = String(Character.toChars(0x20000))
        assertEquals(-2 to 2, selectionRangeAroundCursor(" $han", "$han。"))
        assertEquals(-32 to 32, selectionRangeAroundCursor(han.repeat(40), han.repeat(40)))
    }

    @Test
    fun `line selection excludes bounding newlines and supports document edges`() {
        assertEquals(-3 to 3, selectionRangeAroundCursor("old\nabc", "def\nnew", line = true))
        assertEquals(0 to 3, selectionRangeAroundCursor("old\n", "new", line = true))
        assertEquals(-3 to 0, selectionRangeAroundCursor("old", "", line = true))
        assertNull(selectionRangeAroundCursor("old\n", "\nnew", line = true))
    }
}
