package ink.rea.keytao_app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.assertThrows
import org.junit.Test
import java.io.ByteArrayInputStream

class KeytaoClipboardHistoryStateTest {
    @Test
    fun `configured limits share text and media capacity without counting pinned items`() {
        for (limit in listOf(10, 200)) {
            val state = ClipboardHistoryState(historyLimit = limit)
            state.remember(text("pinned text", 0).copy(pinned = true))
            state.remember(media("pinned media", 1).copy(pinned = true))
            repeat(limit + 3) { index ->
                state.remember(if (index % 2 == 0) text("item-$index", index + 2L) else media("item-$index", index + 2L))
            }

            assertEquals(limit, state.limit)
            assertEquals(2, state.items.count { it.pinned })
            assertEquals(limit, state.items.count { !it.pinned })
            assertEquals(
                ((limit + 2) downTo 3).map { "item-$it" },
                state.items.filterNot { it.pinned }.map { it.key },
            )
            val restored = ClipboardHistoryState(historyLimit = limit).apply { restore(state.encode()) }
            assertEquals(state.items, restored.items)
        }
    }

    @Test
    fun `restore uses the current configured limit and preserves all pins`() {
        val original = ClipboardHistoryState(historyLimit = 200)
        original.remember(text("pinned text", 0).copy(pinned = true))
        original.remember(media("pinned media", 1).copy(pinned = true))
        repeat(20) { index -> original.remember(text("item-$index", index + 2L)) }
        val restored = ClipboardHistoryState(historyLimit = 10)

        restored.restore(original.encode())

        assertEquals(original.items.filter { it.pinned }, restored.items.filter { it.pinned })
        assertEquals((19 downTo 10).map { "item-$it" }, restored.items.filterNot { it.pinned }.map { it.key })
    }

    @Test
    fun `reducing the limit immediately evicts oldest unpinned while preserving more pins than the limit`() {
        val state = ClipboardHistoryState(historyLimit = 200)
        repeat(12) { index -> state.remember(text("pin-$index", index.toLong()).copy(pinned = true)) }
        repeat(25) { index -> state.remember(text("item-$index", index + 20L)) }
        val pinned = state.items.filter { it.pinned }

        state.updateLimit(10)

        assertEquals(10, state.limit)
        assertEquals(pinned, state.items.filter { it.pinned })
        assertEquals((24 downTo 15).map { "item-$it" }, state.items.filterNot { it.pinned }.map { it.key })
    }

    @Test
    fun `unpinning an old item at capacity evicts it as the oldest unpinned item`() {
        val state = ClipboardHistoryState(historyLimit = 10)
        state.remember(text("old pin", 0).copy(pinned = true))
        repeat(10) { index -> state.remember(text("item-$index", index + 1L)) }

        assertTrue(state.togglePin("old pin"))

        assertEquals(10, state.items.size)
        assertFalse(state.items.any { it.key == "old pin" })
        assertEquals((9 downTo 0).map { "item-$it" }, state.items.map { it.key })
    }

    @Test
    fun `constructor and updates clamp limits to the supported range`() {
        assertEquals(50, ClipboardHistoryState().limit)
        assertEquals(10, ClipboardHistoryState(historyLimit = Int.MIN_VALUE).limit)
        val state = ClipboardHistoryState(historyLimit = Int.MAX_VALUE)
        assertEquals(200, state.limit)

        state.updateLimit(9)
        assertEquals(10, state.limit)
        state.updateLimit(201)
        assertEquals(200, state.limit)
    }

    @Test
    fun `pinned items survive combined unpinned eviction and clear all`() {
        val state = ClipboardHistoryState()
        state.remember(text("pinned", 1))
        state.togglePin("pinned")
        repeat(55) { index -> state.remember(text("item-$index", index + 2L)) }
        state.remember(media("media:photo", 100))

        assertEquals(51, state.items.size)
        assertEquals("pinned", state.items.first().key)
        assertEquals(50, state.items.count { !it.pinned })
        assertTrue(state.items.none { it.key == "item-5" })
        assertEquals("media:photo", state.items[1].key)

        state.clearUnpinned()
        assertEquals(listOf("pinned"), state.items.map { it.key })
        state.togglePin("pinned")
        state.clearUnpinned()
        assertTrue(state.items.isEmpty())
    }

    @Test
    fun `recapturing text preserves pin while updating recency without duplication`() {
        val state = ClipboardHistoryState()
        state.remember(text("same", 10))
        state.togglePin("same")
        state.remember(text("same", 20))

        assertEquals(listOf(text("same", 20).copy(pinned = true)), state.items)
        state.remove("same")
        assertTrue(state.items.isEmpty())
    }

    @Test
    fun `text keys with media prefix cannot toggle or delete a media record`() {
        val state = ClipboardHistoryState()
        state.remember(text("media:photo", 10))
        state.remember(media("media:photo", 20))
        state.togglePin("media:photo", media = true)

        assertTrue(state.isPinned("media:photo", media = true))
        assertFalse(state.isPinned("media:photo"))
        state.remove("media:photo")
        assertEquals(listOf(media("media:photo", 20).copy(pinned = true)), state.items)
    }

    @Test
    fun `JSON restart restores text media metadata pins and order`() {
        val original = ClipboardHistoryState()
        original.remember(text("quoted \"text\"\nnext", 10))
        original.remember(media("media:photo", 20))
        original.togglePin("quoted \"text\"\nnext")
        val restored = ClipboardHistoryState()

        restored.restore(original.encode())

        assertEquals(original.items, restored.items)
        restored.clearUnpinned()
        assertEquals(1, restored.items.size)
        assertTrue(restored.items.single().pinned)
    }

    @Test
    fun `corrupt JSON falls back safely and invalid paths or record shapes are skipped`() {
        val state = ClipboardHistoryState()
        state.restore("not JSON")
        assertTrue(state.items.isEmpty())
        state.restore("""[
            {"key":"valid","text":"kept","timestamp":10},
            {"key":"bad","fileName":"../outside"},
            {"key":"bad-windows","fileName":"a\\b"},
            {"key":"mixed","text":"text","fileName":"photo.bin"},
            {"key":"empty","text":" "},
            {"text":"missing key"},
            5
        ]""")
        assertEquals(listOf(StoredClipboardItem("valid", 10, text = "kept")), state.items)
    }

    @Test
    fun `restore reapplies unpinned cap without evicting pinned entries`() {
        val state = ClipboardHistoryState()
        val encoded = (0..60).joinToString(prefix = "[", postfix = "]") { index ->
            """{"key":"$index","text":"$index","timestamp":$index,"pinned":${index == 0}}"""
        }

        state.restore(encoded)

        assertEquals(51, state.items.size)
        assertEquals("0", state.items.first().key)
        assertEquals("60", state.items[1].key)
        assertEquals("11", state.items.last().key)
    }

    @Test
    fun `persistence budget counts UTF8 and JSON escapes and evicts oldest unpinned first`() {
        val state = ClipboardHistoryState()
        state.remember(text("pinned", 1))
        state.togglePin("pinned")
        state.remember(text("old\n\"中😀", 2))
        state.remember(text("new\n\"中😀", 3))
        val expected = ClipboardHistoryState().apply {
            remember(text("pinned", 1).copy(pinned = true))
            remember(text("new\n\"中😀", 3))
        }
        val budget = expected.encode().toByteArray(Charsets.UTF_8).size
        val encoded = state.encode(budget)
        val restored = ClipboardHistoryState().apply { restore(encoded) }

        assertEquals(expected.items, restored.items)
        assertTrue(encoded.toByteArray(Charsets.UTF_8).size <= budget)
        assertEquals(3, state.items.size)
    }

    @Test
    fun `three long pinned texts and short history survive persistence without duplicate text`() {
        val state = ClipboardHistoryState()
        repeat(3) { index ->
            val value = "$index" + "中".repeat(19_999)
            state.remember(text(value, index + 1L))
            assertTrue(state.togglePin(value))
        }
        state.remember(text("short pin", 4))
        state.togglePin("short pin")
        state.remember(text("short history", 5))
        val encoded = state.encode(255 * 1024)
        val restored = ClipboardHistoryState().apply { restore(encoded) }

        assertFalse(encoded.contains("\"text\":"))
        assertEquals(state.items, restored.items)
    }

    @Test
    fun `oversized records do not discard later pinned or unpinned entries`() {
        for (pinned in listOf(false, true)) {
            val state = ClipboardHistoryState()
            state.remember(text("older pin", 0).copy(pinned = true))
            state.remember(text("older history", 1))
            state.remember(text("中".repeat(20_000), 2).copy(pinned = pinned))
            state.remember(text("newer", 3).copy(pinned = pinned))
            val expected = ClipboardHistoryState().apply {
                state.items.filterNot { it.key.length == 20_000 }.forEach { remember(it) }
            }
            val budget = expected.encode().toByteArray(Charsets.UTF_8).size
            val restored = ClipboardHistoryState().apply { restore(state.encode(budget)) }

            assertEquals(expected.items, restored.items)
            assertEquals(4, state.items.size)
            assertEquals("[]", state.encode(2))
        }
    }

    @Test
    fun `new pin respects exact persisted bytes while unpin remains allowed`() {
        val state = ClipboardHistoryState()
        val value = "quoted \"中😀\"\nnext"
        state.remember(text(value, 1))
        state.remember(media("media:photo", 2))
        val expected = ClipboardHistoryState().apply {
            remember(text(value, 1).copy(pinned = true))
            remember(media("media:photo", 2).copy(pinned = true))
        }
        val budget = expected.encode().toByteArray(Charsets.UTF_8).size
        assertTrue(state.togglePin(value, maxBytes = budget))
        val before = state.items

        assertFalse(state.togglePin("media:photo", media = true, maxBytes = budget - 1))
        assertEquals(before, state.items)
        assertTrue(state.togglePin("media:photo", media = true, maxBytes = budget))
        assertEquals(expected.items, state.items)
        val restored = ClipboardHistoryState().apply { restore(state.encode(budget)) }
        assertEquals(expected.items, restored.items)
        assertTrue(state.togglePin("media:photo", media = true, maxBytes = 2))
        assertFalse(state.isPinned("media:photo", media = true))
    }

    @Test
    fun `bounded media reads accept the exact limit and reject the first excess byte`() {
        val bytes = ByteArray(64) { it.toByte() }

        assertArrayEquals(bytes, ClipboardEncryption.readBounded(ByteArrayInputStream(bytes), 64))
        assertArrayEquals(byteArrayOf(), ClipboardEncryption.readBounded(ByteArrayInputStream(byteArrayOf()), 0))
        assertThrows(IllegalArgumentException::class.java) {
            ClipboardEncryption.readBounded(ByteArrayInputStream(bytes), 63)
        }
    }

    private fun text(value: String, timestamp: Long) = StoredClipboardItem(value, timestamp, text = value)

    private fun media(key: String, timestamp: Long) = StoredClipboardItem(
        key, timestamp, fileName = "photo.bin", mime = "image/png", name = "Photo",
        size = 123, width = 12, height = 34,
    )
}
