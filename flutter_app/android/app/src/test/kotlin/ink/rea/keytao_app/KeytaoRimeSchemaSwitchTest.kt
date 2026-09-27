package ink.rea.keytao_app

import org.junit.Assert.assertEquals
import org.junit.Test

class KeytaoRimeSchemaSwitchTest {
    @Test
    fun `parse boolean switch with states`() {
        val switches = KeytaoRimeSchemaSwitch.parseArray(
            """
            [
              {
                "name": "simplification",
                "states": ["漢字", "汉字"],
                "reset": 0
              }
            ]
            """.trimIndent()
        )

        assertEquals(
            listOf(
                KeytaoRimeSchemaSwitch(
                    name = "simplification",
                    options = emptyList(),
                    states = listOf("漢字", "汉字"),
                    reset = 0,
                )
            ),
            switches,
        )
    }

    @Test
    fun `parse single choice switch options`() {
        val switches = KeytaoRimeSchemaSwitch.parseArray(
            """
            [
              {
                "options": ["zh_tw", "zh_cn", "zh_hk"],
                "states": ["臺灣", "大陆", "香港"]
              }
            ]
            """.trimIndent()
        )

        assertEquals(
            listOf(
                KeytaoRimeSchemaSwitch(
                    name = null,
                    options = listOf("zh_tw", "zh_cn", "zh_hk"),
                    states = listOf("臺灣", "大陆", "香港"),
                    reset = null,
                )
            ),
            switches,
        )
    }

    @Test
    fun `drop switch missing both name and options`() {
        val switches = KeytaoRimeSchemaSwitch.parseArray(
            """
            [
              {"states": ["关", "开"]},
              {"name": "ascii_mode", "states": ["中", "英"]}
            ]
            """.trimIndent()
        )

        assertEquals(listOf("ascii_mode"), switches.map { it.name })
    }

    @Test
    fun `cycle single choice switch from missing first and last active indexes`() {
        assertEquals(2, nextRimeSwitchOptionIndex(activeIndex = -1, reset = 7, optionCount = 3))
        assertEquals(1, nextRimeSwitchOptionIndex(activeIndex = 0, reset = 0, optionCount = 3))
        assertEquals(0, nextRimeSwitchOptionIndex(activeIndex = 2, reset = 0, optionCount = 3))
    }
}
