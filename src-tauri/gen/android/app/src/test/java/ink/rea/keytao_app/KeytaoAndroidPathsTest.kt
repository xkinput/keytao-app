package ink.rea.keytao_app

import java.nio.file.Files
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class KeytaoAndroidPathsTest {
    @Test
    fun `scheme requires source and a fresh manual deployment`() {
        val root = Files.createTempDirectory("keytao-android-schema-state").toFile()
        try {
            val build = root.resolve("build").apply { mkdirs() }
            build.resolve("keydo.schema.yaml").writeText("schema: {}\n")

            assertFalse(KeytaoAndroidPaths.hasInstalledSchema(root))
            assertFalse(KeytaoAndroidPaths.hasDeployedSchema(root))

            root.resolve("default.custom.yaml").writeText(
                "patch:\n  schema_list:\n    - schema: keydo\n",
            )
            root.resolve("keydo.schema.yaml").writeText("schema: {}\n")

            assertTrue(KeytaoAndroidPaths.hasInstalledSchema(root))
            assertTrue(KeytaoAndroidPaths.hasDeployedSchema(root))
            assertTrue(KeytaoAndroidPaths.invalidateDeployment(root))
            assertTrue(KeytaoAndroidPaths.hasInstalledSchema(root))
            assertFalse(KeytaoAndroidPaths.hasDeployedSchema(root))
        } finally {
            root.deleteRecursively()
        }
    }
}
