package ink.rea.keytao_app

import org.junit.Assert.assertEquals
import org.junit.Test
import java.io.File
import kotlin.io.path.createTempDirectory

class RimeSchemaNameResolverTest {
    @Test
    fun `parse schema name with inline comments`() {
        val metadata = RimeSchemaNameResolver.parseSchemaMetadata(
            """
                schema:
                  schema_id: keydo # identifier
                  name: 键道·我流 # display
            """.trimIndent()
        )

        assertEquals("keydo", metadata.id)
        assertEquals("键道·我流", metadata.name)
    }

    @Test
    fun `parse schema name with bom and quotes`() {
        val metadata = RimeSchemaNameResolver.parseSchemaMetadata(
            "\uFEFFschema:\n  schema_id: \"txjx\"\n  name: '天行键'\n"
        )

        assertEquals("txjx", metadata.id)
        assertEquals("天行键", metadata.name)
    }

    @Test
    fun `resolve display name from installed schema file`() {
        val dir = createTempDirectory(prefix = "keytao-schema-resolver-").toFile()
        try {
            File(dir, "xmjd6.schema.yaml").writeText(
                """
                    schema:
                      schema_id: xmjd6
                      name: 星猫键道
                """.trimIndent()
            )

            assertEquals("星猫键道", RimeSchemaNameResolver.resolveDisplayName(dir, null, "xmjd6"))
            assertEquals("unknown", RimeSchemaNameResolver.resolveDisplayName(dir, null, "unknown"))
        } finally {
            dir.deleteRecursively()
        }
    }

    @Test
    fun `prefer deployed schema name after custom patches`() {
        val dir = createTempDirectory(prefix = "keytao-schema-resolver-").toFile()
        try {
            File(dir, "xmjd6.schema.yaml").writeText(
                "schema:\n  schema_id: xmjd6\n  name: 星猫键道\n"
            )
            File(dir, "build").mkdirs()
            File(dir, "build/xmjd6.schema.yaml").writeText(
                "schema:\n  schema_id: xmjd6\n  name: 🌟🐈\n"
            )

            assertEquals("🌟🐈", RimeSchemaNameResolver.resolveDisplayName(dir, null, "xmjd6"))
        } finally {
            dir.deleteRecursively()
        }
    }
}
