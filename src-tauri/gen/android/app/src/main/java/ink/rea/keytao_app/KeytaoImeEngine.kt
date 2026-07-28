package ink.rea.keytao_app

import android.content.Context
import java.io.File
import java.util.concurrent.Executors

/** Why the IME cannot compose right now, or [Readiness.READY] when it can. */
enum class Readiness {
    UNWRITABLE,
    NOT_INSTALLED,
    NOT_DEPLOYED,
    NATIVE_UNAVAILABLE,
    READY,
}

class KeytaoImeEngine(context: Context) {
    private val appContext = context.applicationContext

    /**
     * Lazy on purpose: resolving the root creates directories and runs the
     * one-shot migration out of shared storage, which must not happen on the IME
     * main thread during onCreate.
     */
    val userDir: File by lazy { KeytaoAndroidPaths.userRoot(appContext) }
    private var session: Long = 0L
    private var lastState = KeytaoImeState.empty()
    private var lastDisplaySchemaName = ""
    private var sharedDataDir: File? = null
    private var reloadStampSignature: String? = null
    private var inputPolicyComposing = true
    private var inputPolicyLearning = true

    /**
     * Everything that touches the filesystem or librime runs here: the IME main
     * thread only ever reads the snapshot these tasks publish.
     */
    private val backgroundExecutor = Executors.newSingleThreadExecutor { runnable ->
        Thread(runnable, "KeyTao-IME-Engine")
    }

    @Volatile
    var nativeReady: Boolean = false
        private set

    init {
        backgroundExecutor.execute {
            runCatching { KeytaoAndroidPaths.migrateLegacyRootIfNeeded(appContext) }
            runCatching { ensureBundledSharedData(appContext) }
            // Materialise keyboard.yaml and warm the configuration and theme
            // caches before the first input view exists, so showing the keyboard
            // never blocks on YAML parsing or on writing a default config.
            runCatching { KeytaoAndroidImeConfig.ensureDefaults(appContext) }
            runCatching { KeytaoAndroidImeConfig.load(appContext) }
            runCatching { KeytaoThemeResolver.resolve(appContext) }
        }
    }

    fun runInBackground(task: () -> Unit) {
        runCatching { backgroundExecutor.execute(task) }
    }

    @Synchronized
    fun ensureReady(): Boolean {
        if (nativeReady) return true
        if (!hasInstalledSchema()) return false
        if (!hasDeployedSchema()) return false
        return initializeRuntime(deploy = false)
    }

    /**
     * Probe the data directory and bring librime up if it is usable. Blocking:
     * call it from [runInBackground], never from a lifecycle callback.
     */
    @Synchronized
    fun refreshReadiness(): Readiness {
        if (!KeytaoAndroidPaths.isWritable(userDir)) return Readiness.UNWRITABLE
        if (!hasInstalledSchema()) return Readiness.NOT_INSTALLED
        if (!hasDeployedSchema()) return Readiness.NOT_DEPLOYED
        if (!nativeReady) ensureReady()
        return if (nativeReady) Readiness.READY else Readiness.NATIVE_UNAVAILABLE
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

    /** The single Enter implementation: Rime first, core falls back to the raw input. */
    @Synchronized
    fun processEnter(): KeytaoImeState {
        val state = KeytaoNativeBridge.processEnter(session)
            ?.let { stableSchemaState(it) }
            ?: return KeytaoImeState.empty(asciiMode = lastState.asciiMode)
        lastState = state.withoutTransientCommit()
        return state
    }

    @Synchronized
    fun shouldBypassKey(keyCode: Int, modifiers: Int): Boolean {
        return KeytaoNativeBridge.shouldBypassKey(session, keyCode, modifiers)
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
            reloadStampSignature = reloadStampSignature()
            lastState = KeytaoNativeBridge.sessionState(session)
                ?.let { stableSchemaState(it) }
                ?.withoutTransientCommit()
                ?: lastState.withoutTransientCommit()
        }
        return ok
    }

    /** Blocking: reloading finalizes librime, so keep it off the main thread. */
    @Synchronized
    fun reloadIfNeeded(): Boolean {
        if (!nativeReady) return ensureReady()
        val signature = reloadStampSignature() ?: return false
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

    fun isUserDataWritable(): Boolean = KeytaoAndroidPaths.isWritable(userDir)

    @Synchronized
    fun reset(): KeytaoImeState {
        val state = KeytaoNativeBridge.reset(session)
            ?.let { stableSchemaState(it) }
            ?: KeytaoImeState.empty(asciiMode = lastState.asciiMode)
        lastState = state.withoutTransientCommit()
        return state
    }

    /** Drop the composition without committing it (focus lost, caret moved away). */
    @Synchronized
    fun clearComposition(): KeytaoImeState {
        val state = KeytaoNativeBridge.clearComposition(session)
            ?.let { stableSchemaState(it) }
            ?: KeytaoImeState.empty(asciiMode = lastState.asciiMode)
        lastState = state.withoutTransientCommit()
        return state
    }

    /** Commit what Rime currently holds (focus moving to another editor). */
    @Synchronized
    fun commitComposition(): KeytaoImeState {
        val state = KeytaoNativeBridge.commitComposition(session)
            ?.let { stableSchemaState(it) }
            ?: KeytaoImeState.empty(asciiMode = lastState.asciiMode)
        lastState = state.withoutTransientCommit()
        return state
    }

    /**
     * Password and no-personalized-learning editors run with composing off, so
     * keys never reach librime and cannot end up in the user dictionary.
     */
    @Synchronized
    fun setInputPolicy(composing: Boolean, learning: Boolean): KeytaoImeState? {
        if (inputPolicyComposing == composing && inputPolicyLearning == learning) return null
        inputPolicyComposing = composing
        inputPolicyLearning = learning
        val state = KeytaoNativeBridge.setInputPolicy(session, composing, learning)
            ?.let { stableSchemaState(it) }
            ?: return null
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
        backgroundExecutor.shutdownNow()
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
        if (session != 0L && (!inputPolicyComposing || !inputPolicyLearning)) {
            KeytaoNativeBridge.setInputPolicy(session, inputPolicyComposing, inputPolicyLearning)
        }
        lastState = KeytaoNativeBridge.sessionState(session)
            ?.let { stableSchemaState(it) }
            ?: KeytaoImeState.empty()
        reloadStampSignature = reloadStampSignature()
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
            KeytaoAndroidPaths.rimeDataDir(context),
            File(userDir, "shared"),
            File(context.filesDir, "rime-data"),
            File(context.noBackupFilesDir, "keytao/rime-data"),
        ).firstOrNull { File(it, "default.yaml").isFile }
    }

    private fun ensureBundledSharedData(context: Context) {
        val target = KeytaoAndroidPaths.rimeDataDir(context)
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

    /**
     * keytao-core owns the reload signature format; computing our own here is how
     * the IME and the app used to disagree about whether a deployment happened.
     */
    private fun reloadStampSignature(): String? {
        return KeytaoNativeBridge.reloadStampSignature(userDir.absolutePath)
    }

    companion object {
        private const val bundledRimeDataAssetPath = "keytao-rime-data"
    }
}
