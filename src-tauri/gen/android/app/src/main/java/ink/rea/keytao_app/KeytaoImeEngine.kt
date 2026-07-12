package ink.rea.keytao_app

import android.content.Context
import android.os.SystemClock
import java.io.File

class KeytaoImeEngine(context: Context) {
    private val appContext = context.applicationContext
    val userDir: File = KeytaoAndroidPaths.userRoot()
    private val reloadStamp = KeytaoAndroidPaths.reloadStampFile()
    private var session: Long = 0L
    private var lastState = KeytaoImeState.empty()
    private var lastDisplaySchemaName = ""
    private var sharedDataDir: File? = null
    private var reloadStampSignature: String? = fileSignature(reloadStamp)
    private var writableCache: Boolean? = null
    private var writableCacheCheckedAtMs = 0L

    @Volatile
    var nativeReady: Boolean = false
        private set

    init {
        ensureBundledSharedData(appContext)
    }

    @Synchronized
    fun ensureReady(): Boolean {
        if (nativeReady) return true
        if (!hasInstalledSchema()) return false
        if (!hasDeployedSchema()) return false
        return initializeRuntime(deploy = false)
    }

    @Synchronized
    fun state(): KeytaoImeState {
        lastState = KeytaoNativeBridge.sessionState(session)
            ?.let { stableSchemaState(it) }
            ?.withoutTransientCommit()
            ?: lastState.withoutTransientCommit()
        return lastState
    }

    @Synchronized
    fun processKey(keyCode: Int, modifiers: Int): KeytaoImeState {
        val state = KeytaoNativeBridge.processKey(session, keyCode, modifiers)
            ?.let { stableSchemaState(it) }
            ?: return KeytaoImeState.empty(asciiMode = lastState.asciiMode)
        lastState = state.withoutTransientCommit()
        return state
    }

    @Synchronized
    fun selectCandidate(index: Int): KeytaoImeState {
        val state = KeytaoNativeBridge.selectCandidate(session, index)
            ?.let { stableSchemaState(it) }
            ?: return KeytaoImeState.empty(asciiMode = lastState.asciiMode)
        lastState = state.withoutTransientCommit()
        return state
    }

    @Synchronized
    fun selectCandidateGlobal(index: Int): KeytaoImeState {
        val state = KeytaoNativeBridge.selectCandidateGlobal(session, index)
            ?.let { stableSchemaState(it) }
            ?: return KeytaoImeState.empty(asciiMode = lastState.asciiMode)
        lastState = state.withoutTransientCommit()
        return state
    }

    @Synchronized
    fun allCandidates(limit: Int): List<KeytaoCandidate> {
        if (!nativeReady || session == 0L) return emptyList()
        return KeytaoNativeBridge.allCandidates(session, limit)
    }

    @Synchronized
    fun changePage(backward: Boolean): KeytaoImeState {
        val state = KeytaoNativeBridge.changePage(session, backward)
            ?.let { stableSchemaState(it) }
            ?: return KeytaoImeState.empty(asciiMode = lastState.asciiMode)
        lastState = state.withoutTransientCommit()
        return state
    }

    @Synchronized
    fun reload(): Boolean {
        val ok = initializeRuntime(deploy = false, reinitialize = true)
        if (ok) {
            reloadStampSignature = fileSignature(reloadStamp)
            lastState = KeytaoNativeBridge.sessionState(session)
                ?.let { stableSchemaState(it) }
                ?.withoutTransientCommit()
                ?: lastState.withoutTransientCommit()
        }
        return ok
    }

    @Synchronized
    fun reloadIfNeeded(): Boolean {
        if (!nativeReady) return ensureReady()
        val signature = fileSignature(reloadStamp) ?: return false
        if (signature == reloadStampSignature) return false
        return reload()
    }

    fun hasInstalledSchema(): Boolean = KeytaoAndroidPaths.hasInstalledSchema(userDir)

    fun hasDeployedSchema(): Boolean = KeytaoAndroidPaths.hasDeployedSchema(userDir)

    @Synchronized
    fun deployNow(): Boolean {
        if (!hasInstalledSchema()) return false
        return initializeRuntime(deploy = true)
    }

    fun deployStep(schemaId: String?): KeytaoRimeDeployStepResult {
        if (!hasInstalledSchema()) {
            return KeytaoRimeDeployStepResult(error = "No KeyTao schema is installed")
        }
        ensureBundledSharedData(appContext)
        val sharedDir = findSharedDataDir(appContext)
        return KeytaoNativeBridge.deployStep(
            userDir.absolutePath,
            sharedDir?.absolutePath,
            schemaId,
        )
    }

    fun isUserDataWritable(forceRefresh: Boolean = false): Boolean {
        val now = SystemClock.uptimeMillis()
        writableCache?.let { cached ->
            if (!forceRefresh && now - writableCacheCheckedAtMs < writableCacheTtlMs) {
                return cached
            }
        }
        val writable = KeytaoAndroidPaths.isWritable(userDir)
        writableCache = writable
        writableCacheCheckedAtMs = now
        return writable
    }

    @Synchronized
    fun reset(): KeytaoImeState {
        val state = KeytaoNativeBridge.reset(session)
            ?.let { stableSchemaState(it) }
            ?: KeytaoImeState.empty(asciiMode = lastState.asciiMode)
        lastState = state.withoutTransientCommit()
        return state
    }

    @Synchronized
    fun setAsciiMode(enabled: Boolean): KeytaoImeState {
        val state = KeytaoNativeBridge.setAsciiMode(session, enabled)
            ?.let { stableSchemaState(it) }
            ?: KeytaoImeState.empty(asciiMode = enabled)
        lastState = state.withoutTransientCommit()
        return state
    }

    @Synchronized
    fun close() {
        KeytaoNativeBridge.destroySession(session)
        session = 0L
    }

    private fun initializeRuntime(deploy: Boolean, reinitialize: Boolean = false): Boolean {
        if (!hasInstalledSchema()) return false
        if (!deploy && !hasDeployedSchema()) return false
        ensureBundledSharedData(appContext)
        val sharedDir = findSharedDataDir(appContext)
        sharedDataDir = sharedDir
        if (session != 0L) {
            KeytaoNativeBridge.destroySession(session)
            session = 0L
        }
        val initialized = if (reinitialize) {
            KeytaoNativeBridge.reinitialize(userDir.absolutePath, sharedDir?.absolutePath)
        } else {
            KeytaoNativeBridge.init(userDir.absolutePath, sharedDir?.absolutePath, deploy)
        }
        nativeReady = KeytaoNativeBridge.engineAvailable() && initialized
        if (!nativeReady) {
            lastState = lastState.withoutTransientCommit()
            return false
        }
        session = KeytaoNativeBridge.createSession()
        lastState = KeytaoNativeBridge.sessionState(session)
            ?.let { stableSchemaState(it) }
            ?: KeytaoImeState.empty()
        reloadStampSignature = fileSignature(reloadStamp)
        nativeReady = session != 0L
        return nativeReady
    }

    private fun stableSchemaState(state: KeytaoImeState): KeytaoImeState {
        val name = state.schemaName.trim()
        if (name.isNotEmpty() && !name.startsWith(".")) {
            val displayName = RimeSchemaNameResolver.resolveDisplayName(userDir, sharedDataDir, name)
            lastDisplaySchemaName = displayName
            return state.copy(schemaName = displayName)
        }
        return if (lastDisplaySchemaName.isNotEmpty()) {
            state.copy(schemaName = lastDisplaySchemaName)
        } else {
            state
        }
    }

    private fun findSharedDataDir(context: Context): File? {
        return listOf(
            userDir,
            KeytaoAndroidPaths.rimeDataDir(),
            File(userDir, "shared"),
            File(context.filesDir, "rime-data"),
            File(context.noBackupFilesDir, "keytao/rime-data"),
        ).firstOrNull { File(it, "default.yaml").isFile }
    }

    private fun ensureBundledSharedData(context: Context) {
        val target = KeytaoAndroidPaths.rimeDataDir()
        val marker = File(target, "default.yaml")
        if (marker.isFile) return
        val children = runCatching {
            context.assets.list(bundledRimeDataAssetPath)
        }.getOrNull()
        if (children.isNullOrEmpty()) return
        runCatching {
            copyAssetTree(context, bundledRimeDataAssetPath, target)
        }
    }

    private fun copyAssetTree(context: Context, assetPath: String, target: File) {
        val children = context.assets.list(assetPath).orEmpty()
        if (children.isEmpty()) {
            target.parentFile?.mkdirs()
            context.assets.open(assetPath).use { input ->
                target.outputStream().use { output -> input.copyTo(output) }
            }
            return
        }

        target.mkdirs()
        for (child in children) {
            copyAssetTree(context, "$assetPath/$child", File(target, child))
        }
    }

    private fun fileSignature(file: File): String? {
        if (!file.isFile) return null
        return "${file.length()}:${file.lastModified()}"
    }

    companion object {
        private const val bundledRimeDataAssetPath = "keytao-rime-data"
        private const val writableCacheTtlMs = 2_000L
    }
}
