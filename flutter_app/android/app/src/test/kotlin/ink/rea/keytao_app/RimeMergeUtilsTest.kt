package ink.rea.keytao_app

import org.junit.Assert.*
import org.junit.Test

class RimeMergeUtilsTest {
    @Test
    fun `add-on schema extends the staged deployment budget`() {
        assertEquals(
            180_000L,
            KeytaoRimeDeployClient.timeoutMsForSchemas(listOf("keytao", "keytao-dz")),
        )
        assertEquals(
            300_000L,
            KeytaoRimeDeployClient.timeoutMsForSchemas(listOf("keytao", "easy_en")),
        )
        assertEquals(
            300_000L,
            KeytaoRimeDeployClient.timeoutMsForSchemas(listOf("keytao", "wanxiang")),
        )
        assertEquals(
            300_000L,
            KeytaoRimeDeployClient.timeoutMsForSchemas(listOf("keytao", "easy_en", "wanxiang")),
        )
    }

    @Test
    fun `addon recognition does not claim similarly named user schemas`() {
        assertTrue(isAddonSchema("wanxiang"))
        assertTrue(isAddonSchema("easy_en"))
        assertFalse(isAddonSchema("wanxiang_custom"))
        assertFalse(isAddonSchema("wanxiang_pro"))
        assertFalse(isAddonSchema("easy_en_personal"))
    }

    // ── parseSchemas ──────────────────────────────────────────────────────────

    @Test
    fun `parseSchemas extracts schema list`() {
        val content = "patch:\n  schema_list:\n    - schema: keytao_b\n    - schema: keytao_bg\n"
        assertEquals(listOf("keytao_b", "keytao_bg"), parseSchemas(content))
    }

    @Test
    fun `parseSchemas stops at non-schema line`() {
        val content = "patch:\n  schema_list:\n    - schema: foo\n  other_key: val\n"
        assertEquals(listOf("foo"), parseSchemas(content))
    }
}
