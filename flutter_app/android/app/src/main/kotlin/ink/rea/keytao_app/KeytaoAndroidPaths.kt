package ink.rea.keytao_app

import android.Manifest
import android.content.Context
import android.content.pm.ApplicationInfo
import android.content.pm.PackageManager
import android.os.Build
import android.os.Environment
import android.os.Process
import android.os.UserManager
import java.io.File
import org.json.JSONObject

enum class StorageStatus { READY, NOT_UNLOCKED_OR_NOT_MOUNTED, PERMISSION_MISSING }

/**
 * Where the IME keeps schemas, deployment output and its YAML configuration.
 *
 * The app, `:ime` and `:rime_deployer` independently resolve the single editable
 * root: Environment.getExternalStorageDirectory()/keytao (/sdcard/keytao).
 * Unlock, mounted storage, file access and a readable/writable directory are
 * required. Only success is cached; failures retry after 500 ms. There is no
 * private root, fallback, mirroring or migration. Slow calls never run under
 * the paths monitor.
 */
object KeytaoAndroidPaths {
    private const val rootDirectoryName = "keytao"
    private const val reloadStampFileName = "keytao-ime.reload"
    private const val deployingSentinelFileName = ".keytao-deploying"
    private const val deployedMarkerFileName = ".keytao-deployed"
    private const val failedRootRetryNs = 500_000_000L

    @Volatile
    private var cachedRoot: File? = null
    private var lastFailedRootAttemptNs: Long? = null
    private var storageNotReadyStartedNs: Long? = null
    private var storageNotReadyLogged = false
    @Volatile
    internal var lastStatus = StorageStatus.NOT_UNLOCKED_OR_NOT_MOUNTED
        private set
    @Volatile
    internal var testRootOverride: File? = null
        private set

    /** Throw at command boundaries; background IME consumers use [userRootOrNull]. */
    fun userRoot(context: Context): File = userRootOrNull(context)
        ?: throw IllegalStateException(when (lastStatus) {
            StorageStatus.PERMISSION_MISSING -> "需要文件访问权限"
            else -> "存储尚未就绪"
        })

    fun requireRoot(context: Context): File {
        check(hasStorageAccess(context)) { "需要文件访问权限" }
        return userRoot(context)
    }

    @Suppress("DEPRECATION")
    internal fun sharedRoot(): File = File(Environment.getExternalStorageDirectory(), rootDirectoryName)

    fun hasStorageAccess(context: Context): Boolean = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
        Environment.isExternalStorageManager()
    } else {
        context.checkSelfPermission(Manifest.permission.WRITE_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
    }

    internal fun isUnlockedAndMounted(context: Context): Boolean =
        (Build.VERSION.SDK_INT < Build.VERSION_CODES.N ||
            context.getSystemService(UserManager::class.java)?.isUserUnlocked == true) &&
            Environment.getExternalStorageState() == Environment.MEDIA_MOUNTED

    fun status(context: Context): StorageStatus {
        userRootOrNull(context)
        return lastStatus
    }

    fun userRootOrNull(context: Context): File? {
        cachedRoot?.let { return it }
        val appContext = context.applicationContext
        val override = testRootOverride
        resolveUserRoot(
            unlockedAndMounted = { override != null || isUnlockedAndMounted(appContext) },
            hasAccess = { override != null || hasStorageAccess(appContext) },
            rootProvider = { override ?: sharedRoot() },
        )
        val events = mutableListOf<Pair<String, Double>>()
        val result = synchronized(this) {
            val root = cachedRoot
            if (!storageNotReadyLogged && storageNotReadyStartedNs != null) {
                storageNotReadyLogged = true
                events.add("storage_not_ready" to Double.NaN)
            }
            if (root != null) {
                storageNotReadyStartedNs?.let { started ->
                    events.add("storage_ready" to KeytaoRuntimeLog.elapsedMs(started))
                }
                storageNotReadyStartedNs = null
                lastFailedRootAttemptNs = null
            }
            root
        }
        events.forEach { (event, elapsed) -> KeytaoRuntimeLog.event("lifecycle", event, elapsed) }
        result?.let { restoreLegacyLocalManifest(appContext.dataDir, it) }
        return result
    }

    fun isUserRootResolved(): Boolean = cachedRoot != null

    @Synchronized
    internal fun retryResolution() { lastFailedRootAttemptNs = null }

    internal fun resolveUserRoot(
        nowNs: () -> Long = System::nanoTime,
        unlockedAndMounted: () -> Boolean = { true },
        hasAccess: () -> Boolean = { true },
        usable: (File) -> Boolean = ::isWritable,
        rootProvider: () -> File?,
    ): File? {
        cachedRoot?.let { return it }
        synchronized(this) {
            cachedRoot?.let { return it }
            lastFailedRootAttemptNs?.let { failedAt ->
                if (nowNs() - failedAt < failedRootRetryNs) return null
            }
        }
        // StorageManager may block here; never hold the paths monitor across it.
        var status = StorageStatus.NOT_UNLOCKED_OR_NOT_MOUNTED
        val root = runCatching {
            when {
                !unlockedAndMounted() -> null
                !hasAccess() -> { status = StorageStatus.PERMISSION_MISSING; null }
                else -> rootProvider()?.takeIf(usable)
            }
        }.getOrNull()
        synchronized(this) {
            cachedRoot?.let { return it }
            if (root == null) {
                lastStatus = status
                lastFailedRootAttemptNs = nowNs()
                if (storageNotReadyStartedNs == null) storageNotReadyStartedNs = lastFailedRootAttemptNs
                return null
            }
            cachedRoot = root
            lastStatus = StorageStatus.READY
            return root
        }
    }

    @Synchronized
    internal fun resetUserRootCacheForTests() {
        cachedRoot = null
        lastFailedRootAttemptNs = null
        storageNotReadyStartedNs = null
        storageNotReadyLogged = false
        lastStatus = StorageStatus.NOT_UNLOCKED_OR_NOT_MOUNTED
    }

    /** Debug instrumentation only; also validated in the remote deploy process. */
    internal fun setUserRootForTests(context: Context, root: File?) {
        check(context.applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0)
        if (root != null) {
            val allowed = File(context.cacheDir, "keytao-storage-tests").canonicalFile
            require(root.canonicalFile == allowed) { "Test root must be isolated in the app cache" }
        }
        testRootOverride = root
        resetUserRootCacheForTests()
    }

    fun themeFileOrNull(context: Context): File? = userRootOrNull(context)?.let { File(it, "theme.yaml") }

    fun keyboardFileOrNull(context: Context): File? = userRootOrNull(context)?.let { File(it, "keyboard.yaml") }

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

    fun hasDeployedSchema(root: File): Boolean =
        KeytaoNativeBridge.hasDeployedSchema(root.absolutePath)

    fun beginDeployment(root: File) {
        val build = File(root, "build")
        check(build.isDirectory || build.mkdirs()) { "Failed to create Android RIME build directory" }
        File(build, deployingSentinelFileName).writeText("${System.currentTimeMillis()}/${Process.myPid()}")
        val marker = File(build, deployedMarkerFileName)
        check(!marker.exists() || marker.delete()) { "Failed to remove Android RIME deployment marker" }
    }

    fun invalidateDeployment(root: File): Boolean = runCatching {
        beginDeployment(root)
        val build = File(root, "build")
        val children = build.listFiles() ?: return@runCatching false
        var cleared = true
        for (child in children) {
            if (child.name != deployingSentinelFileName && !child.deleteRecursively()) cleared = false
        }
        cleared
    }.getOrDefault(false)

    internal fun configuredSchemas(root: File): List<String> {
        val config = sequenceOf("default.custom.yaml", "default-custom.yaml")
            .map { File(root, it) }
            .firstOrNull { it.isFile }
            ?: return emptyList()
        val localSchemas = runCatching {
            val manifest = JSONObject(File(root, ".keytao-active-scheme.json").readText())
            if (!manifest.getString("scheme_key").startsWith("local-") ||
                File(manifest.getString("root")) != root.canonicalFile) {
                emptySet()
            } else {
                manifest.getJSONObject("manifest").keys().asSequence()
                    .filter { it.endsWith(".schema.yaml") }
                    .map { it.removeSuffix(".schema.yaml") }
                    .filter(::isSafeSchemaId)
                    .toSet()
            }
        }.getOrDefault(emptySet())
        return runCatching {
            parseSchemas(config.readText()).filter {
                isSafeSchemaId(it) && (isManagedSchema(it) || it in localSchemas)
            }
        }
            .getOrDefault(emptyList())
    }

    private fun restoreLegacyLocalManifest(dataDir: File, root: File) {
        val marker = File(root, ".keytao-active-scheme.json")
        if (marker.exists()) return
        runCatching {
            val state = JSONObject(File(dataDir, "app-state.json").readText())
            val key = state.getString("active_scheme")
            if (!key.startsWith("local-") || !state.getJSONObject("installed_packages").has(key)) return
            val manifest = JSONObject(File(dataDir, "scheme-packages/active-manifest.json").readText())
            if (File(manifest.getString("root")) != root.canonicalFile) return
            if (manifest.has("scheme_key") && manifest.getString("scheme_key") != key) return
            manifest.put("scheme_key", key)
            val temporary = File.createTempFile(".active-", ".json", root)
            try {
                temporary.writeText(manifest.toString())
                if (!marker.exists()) check(temporary.renameTo(marker))
            } finally {
                temporary.delete()
            }
        }
    }

    fun isWritable(root: File): Boolean {
        return try {
            if (!root.exists() && !root.mkdirs()) {
                return false
            }
            if (!root.isDirectory || !root.canRead() || !root.canWrite()) return false
            val probe = File.createTempFile(".keytao-write-", ".tmp", root)
            try {
                probe.writeText("ok")
                probe.readText() == "ok"
            } finally {
                check(probe.delete())
            }
        } catch (_: Throwable) {
            false
        }
    }

}
