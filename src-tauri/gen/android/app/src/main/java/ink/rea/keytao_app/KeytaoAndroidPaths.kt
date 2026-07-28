package ink.rea.keytao_app

import android.content.Context
import android.os.Environment
import java.io.File

/**
 * Where the IME keeps schemas, deployment output and its YAML configuration.
 *
 * The app process and the `:ime` process share a UID, so app-specific storage is
 * enough for both and no storage permission is involved. Everything lives under
 * `getExternalFilesDir(null)/keytao` when external storage is usable — that path
 * stays visible to the user for hand-editing theme.yaml — and falls back to
 * `filesDir/keytao` when it is not.
 */
object KeytaoAndroidPaths {
    private const val rootDirectoryName = "keytao"
    private const val reloadStampFileName = "keytao-ime.reload"
    private const val legacyMigrationMarkerName = ".keytao-migrated-from-shared-storage"

    @Volatile
    private var cachedRoot: File? = null

    fun userRoot(context: Context): File {
        cachedRoot?.let { return it }
        synchronized(this) {
            cachedRoot?.let { return it }
            val root = resolveRoot(context)
            root.mkdirs()
            cachedRoot = root
            return root
        }
    }

    fun themeFile(context: Context): File = File(userRoot(context), "theme.yaml")

    fun keyboardFile(context: Context): File = File(userRoot(context), "keyboard.yaml")

    fun imeConfigFile(context: Context): File = File(userRoot(context), "android_ime.json")

    /**
     * keytao-core decides where the reload signal lives; the local constant is
     * only the fallback for a process that could not load the native library.
     */
    fun reloadStampFile(context: Context): File {
        val root = userRoot(context)
        val nativePath = KeytaoNativeBridge.reloadStampPath(root.absolutePath)
        return if (nativePath != null) File(nativePath) else File(root, reloadStampFileName)
    }

    fun rimeDataDir(context: Context): File = File(userRoot(context), "rime-data")

    fun hasInstalledSchema(root: File): Boolean {
        val schemas = configuredSchemas(root)
        return schemas.isNotEmpty() && schemas.all { File(root, "$it.schema.yaml").isFile }
    }

    fun hasDeployedSchema(root: File): Boolean {
        val schemas = configuredSchemas(root)
        val build = File(root, "build")
        return schemas.isNotEmpty() &&
            schemas.all { File(root, "$it.schema.yaml").isFile } &&
            schemas.all { File(build, "$it.schema.yaml").isFile }
    }

    fun invalidateDeployment(root: File): Boolean {
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

    fun isWritable(root: File): Boolean {
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

    private fun resolveRoot(context: Context): File {
        val app = context.applicationContext
        val external = runCatching { app.getExternalFilesDir(null) }.getOrNull()
        if (external != null) {
            val state = runCatching { Environment.getExternalStorageState(external) }.getOrNull()
            if (state == Environment.MEDIA_MOUNTED) {
                return File(external, rootDirectoryName)
            }
        }
        return File(app.filesDir, rootDirectoryName)
    }

    /**
     * Pull an install made by an older build out of shared storage exactly once.
     *
     * Best effort by design: without MANAGE_EXTERNAL_STORAGE the old directory is
     * usually unreadable, in which case the user simply reinstalls the schema
     * from the app. Blocking — call it from a background thread.
     */
    fun migrateLegacyRootIfNeeded(context: Context) {
        migrateLegacyRoot(userRoot(context))
    }

    private fun migrateLegacyRoot(root: File) {
        val marker = File(root, legacyMigrationMarkerName)
        if (marker.exists()) return
        runCatching {
            @Suppress("DEPRECATION")
            val legacy = File(Environment.getExternalStorageDirectory(), rootDirectoryName)
            if (legacy.isDirectory && legacy.canRead() && legacy.absolutePath != root.absolutePath) {
                legacy.listFiles()?.forEach { child ->
                    val target = File(root, child.name)
                    if (!target.exists()) {
                        child.copyRecursively(target, overwrite = false)
                    }
                }
            }
        }
        runCatching { marker.writeText("1") }
    }
}
