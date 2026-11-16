package ink.rea.keytao_app

import org.junit.Assert.assertEquals
import org.junit.Test

class KeytaoEmojiSearchTest {
    @Test
    fun `exact keywords outrank prefix then contains even when less recent`() {
        val index = linkedMapOf(
            "🙂" to listOf("开心微笑"),
            "😄" to listOf("微笑的脸"),
            "😊" to listOf("微笑"),
        )

        assertEquals(listOf("😊", "😄", "🙂"), rankEmojiSearch(index, "微笑", listOf("🙂", "😄")))
    }

    @Test
    fun `recent order boosts equal tiers and duplicate recents do not override first use`() {
        val index = linkedMapOf("😄" to listOf("笑"), "😁" to listOf("笑"), "😂" to listOf("笑"))

        assertEquals(listOf("😂", "😁", "😄"), rankEmojiSearch(index, "笑", listOf("😂", "😁", "😂")))
    }

    @Test
    fun `bundled tts names and case insensitive keywords participate in substring search`() {
        val index = linkedMapOf(
            "😂" to listOf("喜极而泣", "LOL"),
            "🛜" to listOf("无线", "Wi-Fi", "网络"),
            "🥲" to listOf("含泪的笑脸", "眼泪"),
        )

        assertEquals(listOf("😂"), rankEmojiSearch(index, " loL ", emptyList()))
        assertEquals(listOf("😂"), rankEmojiSearch(index, "极而", emptyList()))
        assertEquals(listOf("🛜"), rankEmojiSearch(index, "wi-f", emptyList()))
        assertEquals(emptyList<String>(), rankEmojiSearch(index, "不存在", listOf("😂")))
    }

    @Test
    fun `empty query shows unique recents and limits apply to every path`() {
        val index = linkedMapOf("😄" to listOf("笑"), "😁" to listOf("笑"))

        assertEquals(listOf("😁", "😄"), rankEmojiSearch(index, " ", listOf("😁", "😁", "😄")))
        assertEquals(listOf("😄"), rankEmojiSearch(index, "笑", emptyList(), limit = 1))
        assertEquals(emptyList<String>(), rankEmojiSearch(index, "笑", emptyList(), limit = -1))
        assertEquals(emptyList<String>(), rankEmojiSearch(index, "", listOf("😄"), limit = 0))
    }
}
