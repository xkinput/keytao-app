package ink.rea.keytao_app

import android.content.Context
import android.os.Environment
import java.io.File
import java.nio.file.Files

/**
 * Where the IME keeps schemas, deployment output and its YAML configuration.
 *
 * The app process and the `:ime` process share a UID, so app-specific storage is
 * enough for both and no storage permission is involved. Everything lives under
 * `getExternalFilesDir(null)/keytao`, visible to the user for hand-editing
 * theme.yaml. The IME waits for this directory after unlock, without a fallback.
 */
object KeytaoAndroidPaths {
    private const val rootDirectoryName = "keytao"
    private const val reloadStampFileName = "keytao-ime.reload"
    private const val legacyMigrationMarkerName = ".keytao-migrated-from-shared-storage"
    private const val failedRootRetryNs = 500_000_000L

    @Volatile
    private var cachedRoot: File? = null
    private var lastFailedRootAttemptNs: Long? = null
    private var storageNotReadyStartedNs: Long? = null
    private var storageNotReadyLogged = false

    /** App-process compatibility only; the `:ime` process must use [userRootOrNull]. */
    fun userRoot(context: Context): File {
        // ponytail: Devices without shared storage need a persisted root choice if one shows up.
        return (userRootOrNull(context) ?: File(context.applicationContext.filesDir, rootDirectoryName))
            .apply { mkdirs() }
    }

    fun userRootOrNull(context: Context): File? {
        cachedRoot?.let { return it }
        resolveUserRoot { context.applicationContext.getExternalFilesDir(null) }
        synchronized(this) {
            val root = cachedRoot
            if (!storageNotReadyLogged && storageNotReadyStartedNs != null) {
                storageNotReadyLogged = true
                KeytaoRuntimeLog.event("lifecycle", "storage_not_ready")
            }
            if (root != null) {
                storageNotReadyStartedNs?.let { started ->
                    KeytaoRuntimeLog.event("lifecycle", "storage_ready", KeytaoRuntimeLog.elapsedMs(started))
                }
                storageNotReadyStartedNs = null
                lastFailedRootAttemptNs = null
            }
            return root
        }
    }

    fun isUserRootResolved(): Boolean = cachedRoot != null

    internal fun resolveUserRoot(
        nowNs: () -> Long = System::nanoTime,
        externalFilesDir: () -> File?,
    ): File? {
        cachedRoot?.let { return it }
        synchronized(this) {
            cachedRoot?.let { return it }
            lastFailedRootAttemptNs?.let { failedAt ->
                if (nowNs() - failedAt < failedRootRetryNs) return null
            }
        }
        // StorageManager may block here; never hold the paths monitor across it.
        val external = runCatching { externalFilesDir() }.getOrNull()
        synchronized(this) {
            cachedRoot?.let { return it }
            if (external == null) {
                lastFailedRootAttemptNs = nowNs()
                if (storageNotReadyStartedNs == null) storageNotReadyStartedNs = lastFailedRootAttemptNs
                return null
            }
            val root = File(external, rootDirectoryName)
            cachedRoot = root
            return root
        }
    }

    @Synchronized
    internal fun resetUserRootCacheForTests() {
        cachedRoot = null
        lastFailedRootAttemptNs = null
        storageNotReadyStartedNs = null
        storageNotReadyLogged = false
    }

    internal fun isStaleInternalRoot(dir: File, resolvedRoot: File): Boolean = runCatching {
        dir.isDirectory && dir.canonicalPath != resolvedRoot.canonicalPath &&
            !File(dir, "default.custom.yaml").exists() &&
            !File(dir, "default-custom.yaml").exists() &&
            dir.walkTopDown().onFail { _, error -> throw error }.none {
                Files.isSymbolicLink(it.toPath()) ||
                    it.name.endsWith(".userdb") || it.name.endsWith(".userdb.txt")
            }
    }.getOrDefault(false)

    fun themeFile(context: Context): File = File(userRoot(context), "theme.yaml")

    fun themeFileOrNull(context: Context): File? = userRootOrNull(context)?.let { File(it, "theme.yaml") }

    fun keyboardFile(context: Context): File = File(userRoot(context), "keyboard.yaml")

    fun keyboardFileOrNull(context: Context): File? = userRootOrNull(context)?.let { File(it, "keyboard.yaml") }

    fun imeConfigFile(context: Context): File = File(userRoot(context), "android_ime.json")

    fun imeConfigFileOrNull(context: Context): File? = userRootOrNull(context)?.let { File(it, "android_ime.json") }

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

    /**
     * Pull an install made by an older build out of shared storage exactly once.
     *
     * Best effort by design: without MANAGE_EXTERNAL_STORAGE the old directory is
     * usually unreadable, in which case the user simply reinstalls the schema
     * from the app. Blocking — call it from a background thread.
     */
    fun migrateLegacyRootIfNeeded(context: Context) {
        val root = userRootOrNull(context) ?: return
        migrateLegacyRoot(root)
    }

    private fun migrateLegacyRoot(root: File) {
        root.mkdirs()
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
