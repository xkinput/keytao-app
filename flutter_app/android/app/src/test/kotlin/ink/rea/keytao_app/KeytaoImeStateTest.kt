package ink.rea.keytao_app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class KeytaoImeStateTest {
    @Test
    fun `parse state keeps schema name and the current page`() {
        val state = KeytaoImeState.fromJson(
            """
            {
              "preedit": "",
              "cursor": 0,
              "selStart": 0,
              "selEnd": 0,
              "candidates": [{ "text": "键道6", "comment": null }],
              "highlightedCandidateIndex": 0,
              "pageSize": 6,
              "page": 0,
              "isLastPage": false,
              "committed": "",
              "selectKeys": "1234567890",
              "asciiMode": false,
              "schemaName": "键道6",
              "accepted": true,
              "candidatePanel": {
                "preedit": null,
                "candidates": [
                  { "index": 0, "label": "1.", "text": "键道6", "comment": null, "selected": true }
                ],
                "navigation": { "canGoPrevious": false, "canGoNext": true }
              },
              "modeHint": { "asciiMode": false, "text": "中" }
            }
            """.trimIndent()
        )!!

        assertEquals("键道6", state.schemaName)
        assertEquals(6, state.pageSize)
        assertEquals(1, state.candidates.size)
        assertEquals("键道6", state.candidates[0].text)
        assertTrue(state.candidatePanel.navigation.canGoNext)
    }

    @Test
    fun `the caret and the active segment survive the json round trip`() {
        val state = KeytaoImeState.fromJson(
            """
            {
              "preedit": "你ui",
              "cursor": 1,
              "selStart": 1,
              "selEnd": 3
            }
            """.trimIndent()
        )!!

        assertEquals(1, state.cursor)
        assertEquals(1, state.selStart)
        assertEquals(3, state.selEnd)
    }

    @Test
    fun `the full candidate list is never inferred from the current page`() {
        val state = KeytaoImeState.fromJson(
            """
            {
              "candidates": [{ "text": "是", "comment": null }],
              "candidatePanel": {
                "candidates": [
                  { "index": 0, "label": "1.", "text": "是", "selected": true }
                ]
              }
            }
            """.trimIndent()
        )!!

        assertEquals(1, state.candidates.size)
        assertEquals(1, state.candidatePanel.candidates.size)
    }
}
