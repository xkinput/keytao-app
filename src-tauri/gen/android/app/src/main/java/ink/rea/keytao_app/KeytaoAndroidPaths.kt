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
        return File(root, "keytao.schema.yaml").isFile ||
            File(root, "default.custom.yaml").isFile ||
            File(root, "build/keytao.table.bin").isFile
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
