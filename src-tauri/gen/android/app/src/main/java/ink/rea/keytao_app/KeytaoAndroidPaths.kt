package ink.rea.keytao_app

import android.os.Environment
import java.io.File

object KeytaoAndroidPaths {
    private const val rootDirectoryName = "keytao"
    private const val reloadStampFileName = "keytao-ime.reload"

    @Suppress("DEPRECATION")
    fun userRoot(): File = File(Environment.getExternalStorageDirectory(), rootDirectoryName).apply {
        mkdirs()
    }

    fun themeFile(): File = File(userRoot(), "theme.yaml")

    fun keyboardFile(): File = File(userRoot(), "keyboard.yaml")

    fun imeConfigFile(): File = File(userRoot(), "android_ime.json")

    fun reloadStampFile(): File = File(userRoot(), reloadStampFileName)

    fun rimeDataDir(): File = File(userRoot(), "rime-data")

    fun hasInstalledSchema(root: File = userRoot()): Boolean {
        val schemas = configuredSchemas(root)
        return schemas.isNotEmpty() && schemas.all { File(root, "$it.schema.yaml").isFile }
    }

    fun hasDeployedSchema(root: File = userRoot()): Boolean {
        val schemas = configuredSchemas(root)
        val build = File(root, "build")
        return schemas.isNotEmpty() &&
            schemas.all { File(root, "$it.schema.yaml").isFile } &&
            schemas.all { File(build, "$it.schema.yaml").isFile }
    }

    fun invalidateDeployment(root: File = userRoot()): Boolean {
        val build = File(root, "build")
        return !build.exists() || build.deleteRecursively()
    }

    private fun configuredSchemas(root: File): List<String> {
        val config = sequenceOf("default.custom.yaml", "default-custom.yaml")
            .map { File(root, it) }
            .firstOrNull { it.isFile }
            ?: return emptyList()
        return runCatching { parseSchemas(config.readText()).filter(::isManagedSchema) }
            .getOrDefault(emptyList())
    }

    fun isWritable(root: File = userRoot()): Boolean {
        return try {
            if (!root.exists() && !root.mkdirs()) {
                return false
            }
            val probe = File(root, ".keytao-write-test")
            probe.writeText("ok")
            probe.delete()
            true
        } catch (_: Throwable) {
            false
        }
    }
}
