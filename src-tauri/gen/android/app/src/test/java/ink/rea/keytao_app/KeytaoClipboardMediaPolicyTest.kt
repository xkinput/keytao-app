package ink.rea.keytao_app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class KeytaoClipboardMediaPolicyTest {
    @Test
    fun `eviction removes oldest entries for count and byte caps`() {
        val items = mutableListOf(3L, 4L, 2L, 1L)
        assertEquals(listOf(1L, 2L), evictClipboardMedia(items, 3, 7L) { it })
        assertEquals(listOf(3L, 4L), items)
        assertEquals(listOf(4L, 3L), evictClipboardMedia(items, 0, 0L) { it })
        assertTrue(items.isEmpty())
    }

    @Test
    fun `mime acceptance supports exact type family and universal wildcards`() {
        assertTrue(mimeAccepted("image/png", arrayOf("image/png")))
        assertTrue(mimeAccepted("image/png", arrayOf("text/plain", "image/*")))
        assertTrue(mimeAccepted("application/pdf", arrayOf("*/*")))
        assertFalse(mimeAccepted("image/png", arrayOf("image/jpeg", "video/*")))
        assertFalse(mimeAccepted("image/png", emptyArray()))
        assertFalse(mimeAccepted("imageevil/png", arrayOf("image/*")))
    }
}
