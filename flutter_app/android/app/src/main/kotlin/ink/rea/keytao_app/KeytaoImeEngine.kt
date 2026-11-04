package ink.rea.keytao_app

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import java.io.File
import java.util.concurrent.Executors

/** Why the IME cannot compose right now, or [Readiness.READY] when it can. */
enum class Readiness {
    STORAGE_NOT_READY,
    PERMISSION_MISSING,
    UNWRITABLE,
    NOT_INSTALLED,
    NOT_DEPLOYED,
    NATIVE_UNAVAILABLE,
    READY,
}

data class KeytaoRimeOptionsState(
    val schemas: List<KeytaoRimeSchema>,
    val currentSchema: KeytaoRimeSchema?,
    val englishSchemaId: String?,
    val switches: List<KeytaoRimeSchemaSwitch>,
    val options: Map<String, Boolean>,
) {
    companion object {
        val EMPTY = KeytaoRimeOptionsState(emptyList(), null, null, emptyList(), emptyMap())
    }
}

class KeytaoImeEngine(context: Context) {
    private val appContext = context.applicationContext

    /** A failed resolution is retried; only the shared external root is cached. */
    val userDir: File? get() = KeytaoAndroidPaths.userRootOrNull(appContext)
    private var warmupComplete = false
    private var session: Long = 0L
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
        warmupComplete = true
        val started = System.nanoTime()
        val bundledDataOk = runCatching { ensureBundledSharedData(appContext) }.isSuccess
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
        if (nativeReady) return true
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
    @Synchronized
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
        warmUpIfStorageReady()
        if (!KeytaoAndroidPaths.isWritable(root)) return Readiness.UNWRITABLE
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

    fun hasInstalledSchema(): Boolean = userDir?.let(KeytaoAndroidPaths::hasInstalledSchema) ?: false

    fun hasDeployedSchema(): Boolean = userDir?.let(KeytaoAndroidPaths::hasDeployedSchema) ?: false

    @Synchronized
    fun deployNow(): Boolean {
        if (!hasInstalledSchema()) return false
        return initializeRuntime(deploy = true)
    }

    fun deployStep(schemaId: String?): KeytaoRimeDeployStepResult {
        val root = userDir
        if (root == null || !KeytaoAndroidPaths.hasInstalledSchema(root)) {
            return KeytaoRimeDeployStepResult(error = "No KeyTao schema is installed")
        }
        invalidateSchemaNameCache()
        try {
            ensureBundledSharedData(appContext)
            val sharedDir = findSharedDataDir()
                ?: return KeytaoRimeDeployStepResult(error = "Missing rime-data/default.yaml")
            return KeytaoNativeBridge.deployStep(
                root.absolutePath,
                sharedDir.absolutePath,
                schemaId,
            )
        } finally {
            // Deployment does not hold the engine monitor; discard anything
            // resolved concurrently while the compiled files were changing.
            invalidateSchemaNameCache()
        }
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
        KeytaoNativeBridge.destroySession(session)
        session = 0L
        backgroundExecutor.shutdownNow()
        schemaLogoExecutor.shutdownNow()
    }

    private fun initializeRuntime(deploy: Boolean, reinitialize: Boolean = false): Boolean {
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
            if (!hasInstalledSchema()) return false
            if (!deploy && !hasDeployedSchema()) return false
            ensureBundledSharedData(appContext)
            val sharedDir = findSharedDataDir() ?: return false
            sharedDataDir = sharedDir
            if (session != 0L) {
                KeytaoNativeBridge.destroySession(session)
                session = 0L
            }
            val initialized = if (reinitialize) {
                KeytaoNativeBridge.reinitialize(root.absolutePath, sharedDir.absolutePath)
            } else {
                KeytaoNativeBridge.init(root.absolutePath, sharedDir.absolutePath, deploy)
            }
            nativeReady = KeytaoNativeBridge.engineAvailable() && initialized
            if (!nativeReady) {
                lastState = lastState.withoutTransientCommit()
                return false
            }
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
            reloadStampSignature = reloadStampSignature()
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
        val root = userDir ?: return
        val target = File(root, "rime-data")
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
        val root = userDir ?: return null
        return KeytaoNativeBridge.reloadStampSignature(root.absolutePath)
    }

    companion object {
        private const val bundledRimeDataAssetPath = "keytao-rime-data"
    }
}
