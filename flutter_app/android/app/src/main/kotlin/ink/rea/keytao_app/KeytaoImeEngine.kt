package ink.rea.keytao_app

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.os.Build
import android.util.Log
import java.io.File
import java.io.RandomAccessFile
import java.util.concurrent.Executors

/** Why the IME cannot compose right now, or [Readiness.READY] when it can. */
enum class Readiness {
    STORAGE_NOT_READY,
    PERMISSION_MISSING,
    UNWRITABLE,
    NOT_INSTALLED,
    NOT_DEPLOYED,
    DEPLOYING,
    ENGINE_DISABLED,
    NATIVE_UNAVAILABLE,
    READY,
}

data class KeytaoRimeOptionsState(
    val schemas: List<KeytaoRimeSchema>,
    val currentSchema: KeytaoRimeSchema?,
    val englishSchemaId: String?,
    val switches: List<KeytaoRimeSchemaSwitch>,
    val options: Map<String, Boolean>,
    val packages: List<KeytaoRimeSwitcherPackage> = emptyList(),
) {
    companion object {
        val EMPTY = KeytaoRimeOptionsState(emptyList(), null, null, emptyList(), emptyMap())
    }
}

class KeytaoImeEngine(context: Context) {
    private val appContext = context.applicationContext
    private val bundledVersion by lazy { bundledVersion(appContext) }

    /** A failed resolution is retried; only the shared external root is cached. */
    val userDir: File? get() = KeytaoAndroidPaths.userRootOrNull(appContext)
    private var warmupComplete = false
    @Volatile private var algorithmEnabled = true
    private var suspendedRoot: String? = null
    @Volatile private var session: Long = 0L
    @Volatile private var closed = false
    private val deploymentHintLock = Any()
    @Volatile var deploymentHint: String? = null
        private set
    private var lastState = KeytaoImeState.empty()
    private var lastDisplaySchemaName = ""
    private val schemaDisplayNames = mutableMapOf<String, String>()
    private var displaySchemaId: String? = null
    private var rawDisplaySchemaName = ""
    private var sharedDataDir: File? = null
    private var reloadStampSignature: String? = null
    private var inputPolicyComposing = true
    private var inputPolicyLearning = true
    private val schemaNameDurations = KeytaoDurationHistogram()
    private data class SchemaLogoKey(val schemaId: String, val modifiedAt: Long, val generation: Long)
    private var schemaLogoGeneration = 0L
    private var schemaLogoKey: SchemaLogoKey? = null
    private var schemaLogoRequest: Pair<String, Long>? = null
    private val schemaLogoExecutor = Executors.newSingleThreadExecutor { runnable ->
        Thread(runnable, "KeyTao-Schema-Logo")
    }
    @Volatile var schemaLogo: Bitmap? = null
        private set
    @Volatile var onSchemaLogoChanged: (() -> Unit)? = null
    @Volatile var onReloadStarted: (() -> ((Long) -> Unit))? = null

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
        backgroundExecutor.execute { warmUpIfStorageReady() }
    }

    /** Callers hold the engine monitor, normally on the engine thread; defer until the real root resolves. */
    @Synchronized
    private fun warmUpIfStorageReady() {
        if (warmupComplete) return
        userDir ?: return
        KeytaoNativeBridge.configureLibjd(appContext.dataDir.absolutePath)
        val started = System.nanoTime()
        val bundledDataOk = runCatching { ensureBundledSharedData(appContext) }.isSuccess
        warmupComplete = bundledDataOk
        // Materialise keyboard.yaml and warm the configuration and theme caches
        // off the main thread, including when storage first appears on a retry.
        val defaultsOk = runCatching { KeytaoAndroidImeConfig.ensureDefaults(appContext) }.isSuccess
        val configOk = runCatching { KeytaoAndroidImeConfig.load(appContext) }.isSuccess
        val themeOk = runCatching { KeytaoThemeResolver.resolve(appContext) }.isSuccess
        KeytaoRuntimeLog.event("rime", "engine_warmup", KeytaoRuntimeLog.elapsedMs(started)) {
            put("bundled_data_ok", bundledDataOk)
            put("defaults_ok", defaultsOk)
            put("config_ok", configOk)
            put("theme_ok", themeOk)
        }
    }

    fun runInBackground(task: () -> Unit) {
        runCatching { backgroundExecutor.execute(task) }
    }

    @Synchronized
    fun ensureReady(): Boolean {
        val root = userDir ?: return false
        if (closed || suspendIfDisabled(root)) return false
        if (nativeReady && session != 0L) return true
        warmUpIfStorageReady()
        if (!warmupComplete) return false
        if (!hasInstalledSchema()) return false
        if (!hasDeployedSchema()) return false
        return initializeRuntime(deploy = false)
    }

    /**
     * Probe the data directory and bring librime up if it is usable. Blocking:
     * call it from [runInBackground], never from a lifecycle callback.
     */
    fun refreshReadiness(): Readiness {
        val root = userDir ?: run {
            schemaLogoKey = null
            schemaLogoRequest = null
            schemaLogo = null
            onSchemaLogoChanged?.invoke()
            return when (KeytaoAndroidPaths.lastStatus) {
                StorageStatus.PERMISSION_MISSING -> Readiness.PERMISSION_MISSING
                else -> Readiness.STORAGE_NOT_READY
            }
        }
        if (suspendIfDisabled(root)) return Readiness.ENGINE_DISABLED
        refreshDeploymentHint()
        warmUpIfStorageReady()
        if (KeytaoAndroidPaths.isDeploymentInProgress(root)) {
            return when {
                deploymentBlocksInput() -> Readiness.DEPLOYING
                nativeReady && session != 0L -> Readiness.READY
                else -> Readiness.NATIVE_UNAVAILABLE
            }
        }
        if (!KeytaoAndroidPaths.isWritable(root)) return Readiness.UNWRITABLE
        val isolated = KeytaoNativeBridge.isolatedSchemaState(root.absolutePath)
        if (isolated == Readiness.NOT_INSTALLED) {
            if (session != 0L) reload()
            return Readiness.NOT_INSTALLED
        }
        if (isolated == null && !KeytaoAndroidPaths.hasInstalledSchema(root)) {
            if (session != 0L) reload()
            return Readiness.NOT_INSTALLED
        }
        if (!warmupComplete) return Readiness.NOT_DEPLOYED
        val pendingBuild = isolated == Readiness.NOT_DEPLOYED ||
            (isolated == null && !hasDeployedSchema())
        // Core retains a usable session during deployment, and handles an
        // explicit engine switch even when the destination has no build.
        if (!pendingBuild || (nativeReady && session != 0L)) {
            reloadIfNeeded()
        }
        refreshDeploymentHint()
        return when {
            nativeReady && session != 0L -> Readiness.READY
            pendingBuild && deploymentBlocksInput() -> Readiness.DEPLOYING
            pendingBuild -> Readiness.NOT_DEPLOYED
            else -> Readiness.NATIVE_UNAVAILABLE
        }
    }

    @Synchronized
    private fun suspendIfDisabled(root: File): Boolean {
        val path = root.absolutePath
        algorithmEnabled = KeytaoNativeBridge.engineEnabled(path)
        if (algorithmEnabled) {
            suspendedRoot = null
            return false
        }
        if (suspendedRoot == path) return true
        nativeReady = false
        KeytaoNativeBridge.destroySession(session)
        session = 0L
        lastState = KeytaoImeState.empty(asciiMode = true)
        invalidateSchemaNameCache()
        schemaLogoGeneration++
        schemaLogoKey = null
        schemaLogoRequest = null
        schemaLogo = null
        onSchemaLogoChanged?.invoke()
        resetDeploymentInput()
        if (KeytaoNativeBridge.suspendEngine(path)) suspendedRoot = path
        return true
    }

    fun deploymentBlocksInput(): Boolean =
        algorithmEnabled && userDir?.let { KeytaoNativeBridge.deploymentBlocksInput(it.absolutePath) } == true

    fun refreshDeploymentHint() {
        synchronized(deploymentHintLock) {
            deploymentHint = if (algorithmEnabled) {
                userDir?.let { KeytaoNativeBridge.deploymentHint(it.absolutePath) }
            } else null
        }
    }

    fun schemeOperationProgress(): String? =
        userDir?.let { KeytaoNativeBridge.schemeOperationProgress(it.absolutePath) }

    fun deploymentKeyPressed() {
        synchronized(deploymentHintLock) {
            if (algorithmEnabled) {
                deploymentHint = userDir?.let { KeytaoNativeBridge.deploymentKeyPressed(it.absolutePath) }
            }
        }
    }

    fun resetDeploymentInput() {
        synchronized(deploymentHintLock) {
            KeytaoNativeBridge.resetDeploymentInput()
            deploymentHint = null
        }
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
    fun drainPendingState(expectedSession: Long): KeytaoImeState? {
        if (closed || session != expectedSession) return null
        val state = KeytaoNativeBridge.drainPendingState(session)
            ?.let { stableSchemaState(it) }
            ?: lastState.withoutTransientCommit()
        lastState = state.withoutTransientCommit()
        return state
    }

    @Synchronized
    fun processKey(keyCode: Int, modifiers: Int): KeytaoImeState {
        if (modifiers and AndroidKeyMapper.RIME_RELEASE_MASK == 0) deploymentKeyPressed()
        if (!nativeReady || session == 0L) return KeytaoImeState.empty(asciiMode = lastState.asciiMode)
        val state = KeytaoNativeBridge.processKey(session, keyCode, modifiers)
            ?.let { stableSchemaState(it) }
            ?: return KeytaoImeState.empty(asciiMode = lastState.asciiMode)
        lastState = state.withoutTransientCommit()
        return state
    }

    /** The single Enter implementation: Rime first, core falls back to the raw input. */
    @Synchronized
    fun processEnter(): KeytaoImeState {
        deploymentKeyPressed()
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
    fun deleteCandidate(index: Int): Pair<KeytaoImeState, Boolean> {
        val state = KeytaoNativeBridge.deleteCandidate(session, index)
            ?.let { stableSchemaState(it) }
            ?: return lastState.withoutTransientCommit() to false
        val deleted = state.accepted
        lastState = state.withoutTransientCommit()
        return state to deleted
    }

    @Synchronized
    fun candidateIsUserPhrase(index: Int): Boolean {
        return KeytaoNativeBridge.candidateIsUserPhrase(session, index)
    }

    @Synchronized
    fun allCandidates(limit: Int): List<KeytaoCandidate> {
        if (!nativeReady || session == 0L) return emptyList()
        return KeytaoNativeBridge.allCandidates(session, limit)
    }

    @Synchronized
    fun listSchemas(): List<KeytaoRimeSchema> {
        if (!nativeReady || session == 0L) return emptyList()
        return KeytaoNativeBridge.listSchemas(session)
    }

    fun listSwitcherPackages(): List<KeytaoRimeSwitcherPackage> {
        val activeSession = session
        if (!nativeReady || activeSession == 0L) return emptyList()
        return KeytaoNativeBridge.listSwitcherPackages(activeSession)
    }

    @Synchronized
    fun schemaSwitches(): List<KeytaoRimeSchemaSwitch> {
        if (!nativeReady || session == 0L) return emptyList()
        return KeytaoNativeBridge.schemaSwitches(session)
    }

    @Synchronized
    fun currentSchema(): KeytaoRimeSchema? {
        if (!nativeReady || session == 0L) return null
        return KeytaoNativeBridge.currentSchema(session)
    }

    @Synchronized
    fun selectSchema(schemaId: String): KeytaoImeState? {
        val started = System.nanoTime()
        var success = false
        try {
            invalidateSchemaNameCache()
            val state = KeytaoNativeBridge.selectSchema(session, schemaId)
                ?.let { stableSchemaState(it) }
                ?: return null
            lastState = state.withoutTransientCommit()
            success = true
            return state
        } finally {
            val ok = success
            KeytaoRuntimeLog.event("rime", "schema_switch", KeytaoRuntimeLog.elapsedMs(started)) { put("success", ok) }
        }
    }

    @Synchronized
    fun selectPackageSchema(packageKey: String, schemaId: String): KeytaoImeState? {
        if (!nativeReady || session == 0L) return null
        invalidateSchemaNameCache()
        val state = KeytaoNativeBridge.selectPackageSchema(session, packageKey, schemaId)
            ?.let(::stableSchemaState)
            ?: return null
        lastState = state.withoutTransientCommit()
        return state
    }

    @Synchronized
    fun getOption(optionName: String): Boolean {
        return KeytaoNativeBridge.getOption(session, optionName)
    }

    @Synchronized
    fun setOption(optionName: String, enabled: Boolean): KeytaoImeState? {
        val state = KeytaoNativeBridge.setOption(session, optionName, enabled)
            ?.let { stableSchemaState(it) }
            ?: return null
        lastState = state.withoutTransientCommit()
        return state
    }

    @Synchronized
    fun changePage(backward: Boolean): KeytaoImeState {
        val state = KeytaoNativeBridge.changePage(session, backward)
            ?.let { stableSchemaState(it) }
            ?: return KeytaoImeState.empty(asciiMode = lastState.asciiMode)
        lastState = state.withoutTransientCommit()
        return state
    }

    fun reload(): Boolean {
        val snapshot = synchronized(this) {
            val root = userDir ?: return false
            if (closed || suspendIfDisabled(root) || KeytaoAndroidPaths.isDeploymentInProgress(root)) return false
            val isolated = KeytaoNativeBridge.isolatedSchemaState(root.absolutePath)
            if (isolated == Readiness.NOT_INSTALLED) {
                return initializeRuntime(deploy = false, reinitialize = true)
            }
            if (!nativeReady || session == 0L || (isolated == null && !KeytaoAndroidPaths.hasInstalledSchema(root))) {
                return initializeRuntime(deploy = false, reinitialize = true)
            }
            val retained = KeytaoNativeBridge.retainSession(session)
            if (retained == 0L) return false
            Triple(session, retained, root)
        }
        val completed = onReloadStarted?.invoke()
        try {
            // Retain the session under the monitor, then let keys wait in core during prewarm.
            val requestedSignature = reloadStampSignature()
            val ok = KeytaoNativeBridge.reinitialize(snapshot.third.absolutePath, sharedDataDir?.absolutePath)
            val available = ok && KeytaoAndroidPaths.hasDeployedSchema(snapshot.third)
            if (available) KeytaoNativeBridge.prewarm(snapshot.second)
            synchronized(this) {
                if (closed || session != snapshot.first) return false
                if (ok) {
                    if (!available) {
                        nativeReady = false
                        KeytaoNativeBridge.destroySession(session)
                        session = 0L
                        lastState = KeytaoImeState.empty(asciiMode = lastState.asciiMode)
                    }
                    invalidateSchemaNameCache()
                    schemaLogoGeneration++
                    schemaLogoKey = null
                    schemaLogoRequest = null
                    reloadStampSignature = requestedSignature
                }
                refreshDeploymentHint()
            }
            if (available) completed?.invoke(snapshot.first)
            return ok
        } finally {
            KeytaoNativeBridge.destroySession(snapshot.second)
        }
    }

    /** Blocking: reloading finalizes librime, so keep it off the main thread. */
    fun reloadIfNeeded(): Boolean {
        if (!nativeReady || session == 0L) return ensureReady()
        val signature = reloadStampSignature() ?: return false
        if (signature == reloadStampSignature) return false
        return reload()
    }

    fun hasInstalledSchema(): Boolean {
        val root = userDir ?: return false
        val isolated = KeytaoNativeBridge.isolatedSchemaState(root.absolutePath)
        if (isolated != null) return isolated != Readiness.NOT_INSTALLED
        return KeytaoAndroidPaths.hasInstalledSchema(root)
    }

    fun hasDeployedSchema(): Boolean = userDir?.let(KeytaoAndroidPaths::hasDeployedSchema) ?: false

    @Synchronized
    internal fun deployNow(): Boolean {
        check(KeytaoAndroidPaths.testRootOverride != null) { "Deploy fixture requires a test root" }
        if (!hasInstalledSchema()) return false
        return initializeRuntime(deploy = true)
    }

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
        closed = true
        nativeReady = false
        KeytaoNativeBridge.destroySession(session)
        session = 0L
        backgroundExecutor.shutdownNow()
        schemaLogoExecutor.shutdownNow()
    }

    private fun initializeRuntime(deploy: Boolean, reinitialize: Boolean = false): Boolean {
        nativeReady = false
        warmUpIfStorageReady()
        if (!warmupComplete) return false
        val root = userDir ?: return false
        val started = System.nanoTime()
        var success = false
        try {
            invalidateSchemaNameCache()
            schemaLogoGeneration++
            schemaLogoKey = null
            schemaLogoRequest = null
            if (!hasInstalledSchema()) {
                KeytaoNativeBridge.destroySession(session)
                session = 0L
                lastState = KeytaoImeState.empty(asciiMode = lastState.asciiMode)
                if (reinitialize) {
                    KeytaoNativeBridge.reinitialize(root.absolutePath, sharedDataDir?.absolutePath)
                }
                return false
            }
            if (!deploy && !hasDeployedSchema()) return false
            ensureBundledSharedData(appContext)
            val sharedDir = findSharedDataDir() ?: return false
            sharedDataDir = sharedDir
            if (session != 0L) {
                KeytaoNativeBridge.destroySession(session)
                session = 0L
            }
            val requestedSignature = reloadStampSignature()
            val initialized = if (reinitialize) {
                KeytaoNativeBridge.reinitialize(root.absolutePath, sharedDir.absolutePath)
            } else {
                KeytaoNativeBridge.init(root.absolutePath, sharedDir.absolutePath, deploy)
            }
            if (!KeytaoNativeBridge.engineAvailable() || !initialized) {
                lastState = lastState.withoutTransientCommit()
                return false
            }
            if (!deploy && suspendIfDisabled(root)) return false
            val sessionStarted = System.nanoTime()
            session = KeytaoNativeBridge.createSession()
            val sessionCreated = session != 0L
            KeytaoRuntimeLog.event("rime", "create_session", KeytaoRuntimeLog.elapsedMs(sessionStarted)) {
                put("success", sessionCreated)
            }
            if (session != 0L && (!inputPolicyComposing || !inputPolicyLearning)) {
                KeytaoNativeBridge.setInputPolicy(session, inputPolicyComposing, inputPolicyLearning)
            }
            lastState = KeytaoNativeBridge.sessionState(session)
                ?.let { stableSchemaState(it) }
                ?: KeytaoImeState.empty()
            reloadStampSignature = requestedSignature
            refreshDeploymentHint()
            nativeReady = session != 0L
            success = nativeReady
            return nativeReady
        } finally {
            val ok = success
            KeytaoRuntimeLog.event("rime", "engine_init", KeytaoRuntimeLog.elapsedMs(started)) {
                put("deploy", deploy)
                put("reinitialize", reinitialize)
                put("success", ok)
            }
        }
    }

    private fun stableSchemaState(state: KeytaoImeState): KeytaoImeState {
        val root = userDir ?: return state
        val name = state.schemaName.trim()
        if (name.isNotEmpty() && !name.startsWith(".") && state.schemaId.isNotEmpty() && !state.schemaId.startsWith(".")) {
            if (displaySchemaId == null || displaySchemaId != state.schemaId || rawDisplaySchemaName != name) {
                displaySchemaId = state.schemaId
                rawDisplaySchemaName = name
            }
            val displayName = schemaDisplayNames.getOrPut(displaySchemaId!!) {
                val started = schemaNameDurations.start()
                try {
                    RimeSchemaNameResolver.resolveDisplayName(root, sharedDataDir, name)
                } finally {
                    schemaNameDurations.finish(started)
                }
            }
            lastDisplaySchemaName = displayName
            requestSchemaLogo(displaySchemaId!!)
            return state.copy(schemaName = displayName)
        }
        return if (lastDisplaySchemaName.isNotEmpty()) {
            state.copy(schemaName = lastDisplaySchemaName)
        } else {
            state
        }
    }

    private fun requestSchemaLogo(schemaId: String) {
        if (schemaId.isEmpty() || schemaId == "." || schemaId == ".." ||
            schemaId.any { it == '/' || it == '\\' || it == '\u0000' }) {
            schemaLogoRequest = null
            schemaLogoKey = null
            schemaLogo = null
            onSchemaLogoChanged?.invoke()
            return
        }
        val request = schemaId to schemaLogoGeneration
        if (schemaLogoRequest == request) return
        schemaLogoRequest = request
        schemaLogoExecutor.execute {
            var key: SchemaLogoKey? = null
            val bitmap = runCatching {
                val root = userDir ?: return@runCatching null
                val file = File(File(root, ".keytao-logo"), "$schemaId.png")
                key = SchemaLogoKey(schemaId, file.lastModified(), request.second)
                synchronized(this) {
                    if (schemaLogoKey == key) {
                        if (schemaLogoRequest == request) schemaLogoRequest = null
                        return@execute
                    }
                }
                if (!file.isFile || file.length() > 1024 * 1024) return@runCatching null
                val png = file.inputStream().use { input ->
                    val buffer = ByteArray(1024 * 1024 + 1)
                    var count = 0
                    while (count < buffer.size) {
                        val read = input.read(buffer, count, buffer.size - count)
                        if (read < 0) break
                        count += read
                    }
                    if (count > 1024 * 1024) return@runCatching null
                    buffer.copyOf(count)
                }
                val signature = byteArrayOf(-119, 80, 78, 71, 13, 10, 26, 10)
                if (png.size < signature.size || !png.copyOfRange(0, signature.size).contentEquals(signature)) {
                    return@runCatching null
                }
                val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
                BitmapFactory.decodeByteArray(png, 0, png.size, bounds)
                if (bounds.outWidth !in 1..512 || bounds.outHeight !in 1..512) return@runCatching null
                BitmapFactory.decodeByteArray(png, 0, png.size)
            }.getOrNull()
            synchronized(this) {
                if (schemaLogoRequest == request) {
                    schemaLogoRequest = null
                    schemaLogoKey = key
                    schemaLogo = bitmap
                    onSchemaLogoChanged?.invoke()
                }
            }
        }
    }

    @Synchronized
    private fun invalidateSchemaNameCache() {
        schemaDisplayNames.clear()
        displaySchemaId = null
        rawDisplaySchemaName = ""
    }

    fun flushRuntimeHistograms() {
        schemaNameDurations.drain("rime", "schema_name_resolve")
    }

    private fun findSharedDataDir(): File? {
        val root = userDir ?: return null
        return File(root, "rime-data").takeIf { File(it, "default.yaml").isFile }
    }

    private fun ensureBundledSharedData(context: Context) {
        val root = userDir ?: error("KeyTao storage is unavailable")
        prepareBundledSharedData(context, root, bundledVersion)
    }

    /**
     * keytao-core owns the reload signature format; computing our own here is how
     * the IME and the app used to disagree about whether a deployment happened.
     */
    fun reloadStampSignature(): String? {
        val root = userDir ?: return null
        return KeytaoNativeBridge.reloadStampSignature(root.absolutePath)
    }

    companion object {
        private const val bundledRimeDataAssetPath = "keytao-rime-data"

        private fun bundledVersion(context: Context): String {
            @Suppress("DEPRECATION")
            val info = context.packageManager.getPackageInfo(context.packageName, 0)
            @Suppress("DEPRECATION")
            val versionCode = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                info.longVersionCode
            } else {
                info.versionCode.toLong()
            }
            return "$versionCode/${info.versionName.orEmpty()}"
        }

        internal fun prepareBundledSharedData(
            context: Context,
            root: File,
            version: String = bundledVersion(context),
        ) {
            val target = File(root, "rime-data")
            refreshBundledSharedData(target, version) { staging ->
                check(context.assets.list(bundledRimeDataAssetPath)?.contains("default.yaml") == true) {
                    "Missing bundled RIME shared data"
                }
                copyAssetTree(context, bundledRimeDataAssetPath, staging)
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

        @Synchronized
        internal fun refreshBundledSharedData(target: File, version: String, copy: (File) -> Unit) {
            val parent = target.absoluteFile.parentFile ?: error("Missing bundled RIME parent directory")
            check(parent.mkdirs() || parent.isDirectory) { "Failed to create bundled RIME parent directory" }
            RandomAccessFile(File(parent, ".${target.name}.lock"), "rw").use { lockFile ->
                val lock = lockFile.channel.lock()
                try {
                    val staging = File(parent, ".${target.name}.staging")
                    val previous = File(parent, ".${target.name}.previous")
                    if (!target.exists() && previous.exists()) {
                        check(previous.renameTo(target)) { "Failed to restore bundled RIME shared data" }
                    }
                    val stamp = File(target, ".bundled-version")
                    if (File(target, "default.yaml").isFile &&
                        runCatching { stamp.readText() }.getOrNull() == version) {
                        for (leftover in listOf(staging, previous)) {
                            if (!runCatching { leftover.deleteRecursively() }.getOrDefault(false)) {
                                Log.w("KeytaoImeEngine", "Failed to remove bundled RIME leftover: ${leftover.name}")
                            }
                        }
                        return
                    }
                    check(staging.deleteRecursively()) { "Failed to remove staged bundled RIME shared data" }
                    check(staging.mkdirs()) { "Failed to create bundled RIME staging directory" }
                    try {
                        copy(staging)
                        check(File(staging, "default.yaml").isFile) { "Missing rime-data/default.yaml" }
                        check(previous.deleteRecursively()) { "Failed to remove old bundled RIME shared data" }
                        val hadTarget = target.exists()
                        check(!hadTarget || target.renameTo(previous)) { "Failed to move old bundled RIME shared data" }
                        if (!staging.renameTo(target)) {
                            check(!hadTarget || previous.renameTo(target)) { "Failed to restore bundled RIME shared data" }
                            error("Failed to activate bundled RIME shared data")
                        }
                        check(previous.deleteRecursively()) { "Failed to remove old bundled RIME shared data" }
                        stamp.writeText(version)
                    } finally {
                        staging.deleteRecursively()
                    }
                } finally {
                    lock.release()
                }
            }
        }

    }
}
