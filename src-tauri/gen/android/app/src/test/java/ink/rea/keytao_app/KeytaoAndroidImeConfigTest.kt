package ink.rea.keytao_app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class KeytaoAndroidImeConfigTest {
    @Test
    fun `parse config keeps key hints and swipe page commands`() {
        val config = KeytaoAndroidImeConfig.parse(
            """
            {
              "keyboardHeightDp": 280,
              "candidateBarHeightDp": 58,
              "keyboardBottomInsetDp": 32,
              "swipeThresholdDp": 30,
              "rows": [
                [
                  {
                "label": "空格",
                "hint": "主题",
                "weight": 4.5,
                "style": "accent",
                "rimeValue": ";w",
                "action": { "type": "space" },
                "longPress": { "type": "directInput", "value": "！" },
                "asciiLongPress": { "type": "directInput", "value": "!" },
                "swipeUp": { "type": "openPage", "value": "theme" },
                "swipeDown": { "type": "keyboardPicker" }
              }
                ]
              ]
            }
            """.trimIndent()
        )

        val key = config.rows.single().single()
        assertEquals(32, config.keyboardBottomInsetDp)
        assertEquals("空格", key.label)
        assertEquals("主题", key.hint)
        assertEquals(4.5f, key.weight)
        assertEquals("accent", key.style)
        assertEquals(";w", key.rimeValue)
        assertEquals(KeyCommandTypes.SPACE, key.action.type)
        assertEquals(KeyCommandTypes.DIRECT_INPUT, key.longPress?.type)
        assertEquals("！", key.longPress?.value)
        assertEquals(KeyCommandTypes.DIRECT_INPUT, key.asciiLongPress?.type)
        assertEquals("!", key.asciiLongPress?.value)
        assertEquals(KeyCommandTypes.OPEN_PAGE, key.swipeUp?.type)
        assertEquals("theme", key.swipeUp?.value)
        assertEquals(KeyCommandTypes.KEYBOARD_PICKER, key.swipeDown?.type)
    }

    @Test
    fun `parse config falls back to input command from value`() {
        val config = KeytaoAndroidImeConfig.parse(
            """
            {
              "rows": [[{ "label": "a", "value": "a" }]]
            }
            """.trimIndent()
        )

        val key = config.rows.single().single()
        assertEquals(KeyCommandTypes.INPUT, key.action.type)
        assertEquals("a", key.action.value)
        assertNull(key.swipeUp)
    }

    @Test
    fun `parse config keeps long press command`() {
        val config = KeytaoAndroidImeConfig.parse(
            """
            {
              "rows": [[{
                "label": "中",
                "action": { "type": "mode" },
                "longPress": { "type": "openPage", "value": "settings" }
              }]]
            }
            """.trimIndent()
        )

        val key = config.rows.single().single()
        assertEquals(KeyCommandTypes.MODE, key.action.type)
        assertEquals(KeyCommandTypes.OPEN_PAGE, key.longPress?.type)
        assertEquals("settings", key.longPress?.value)
    }

    @Test
    fun `parse config keeps ascii variants and symbol rows`() {
        val config = KeytaoAndroidImeConfig.parse(
            """
            {
              "rows": [[{ "label": "，", "value": "，", "asciiLabel": ",", "asciiValue": "," }]],
              "symbolRows": [[{ "label": "【", "value": "【", "asciiLabel": "[", "asciiValue": "[" }]]
            }
            """.trimIndent()
        )

        val key = config.rows.single().single()
        assertEquals("，", key.label)
        assertEquals(",", key.asciiLabel)
        assertEquals(",", key.asciiValue)

        val symbolKey = config.symbolRows.single().single()
        assertEquals("【", symbolKey.label)
        assertEquals("[", symbolKey.asciiLabel)
        assertEquals("[", symbolKey.asciiValue)
    }

    @Test
    fun `parse config normalizes old punctuation and number symbol switch`() {
        val config = KeytaoAndroidImeConfig.parse(
            """
            {
              "rows": [[
                { "label": "，", "value": "，" },
                { "label": "。", "value": "。" }
              ]],
              "numberRows": [[
                { "label": "#+=", "value": "#+=" }
              ]]
            }
            """.trimIndent()
        )

        val comma = config.rows.single()[0]
        assertEquals(",", comma.asciiLabel)
        assertEquals(",", comma.asciiValue)

        val period = config.rows.single()[1]
        assertEquals(".", period.asciiLabel)
        assertEquals(".", period.asciiValue)

        val symbolSwitch = config.numberRows.single().single()
        assertEquals(KeyCommandTypes.KEYBOARD_MODE, symbolSwitch.action.type)
        assertEquals("symbols", symbolSwitch.action.value)
    }

    @Test
    fun `default m long press goes through rime input path`() {
        val config = KeytaoAndroidImeConfig.parse("""{ "rows": [] }""")
        val mKey = config.rows
            .flatten()
            .first { it.label == "m" }

        assertEquals("=", mKey.hint)
        assertEquals(KeyCommandTypes.INPUT, mKey.longPress?.type)
        assertEquals("=", mKey.longPress?.value)
    }
}
