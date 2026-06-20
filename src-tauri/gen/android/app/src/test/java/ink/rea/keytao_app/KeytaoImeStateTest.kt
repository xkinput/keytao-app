package ink.rea.keytao_app

import org.junit.Assert.assertEquals
import org.junit.Test

class KeytaoImeStateTest {
    @Test
    fun `parse state keeps schema name and all candidates`() {
        val state = KeytaoImeState.fromJson(
            """
            {
              "preedit": "",
              "cursor": 0,
              "candidates": [{ "text": "键道6", "comment": null }],
              "allCandidates": [
                { "text": "键道6", "comment": null },
                { "text": "中 → 英", "comment": "" },
                { "text": "半角 → 全角", "comment": null }
              ],
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
        assertEquals(3, state.allCandidates.size)
        assertEquals(2, state.allCandidates[2].index)
        assertEquals("半角 → 全角", state.allCandidates[2].text)
    }

    @Test
    fun `parse state does not copy current page into all candidates`() {
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
        assertEquals(0, state.allCandidates.size)
    }
}
