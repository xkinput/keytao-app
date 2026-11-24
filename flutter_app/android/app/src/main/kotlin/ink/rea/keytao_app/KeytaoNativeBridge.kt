package ink.rea.keytao_app

import android.util.Log
import org.json.JSONArray
import org.json.JSONObject

data class ProtectedFileInstall(
    val isProtected: Boolean,
    val write: Boolean,
    val backupName: String?,
    val log: String?,
)

object KeytaoNativeBridge {
    private const val libraryName = "keytao_app_bridge"

    val loaded: Boolean = runCatching {
        System.loadLibrary(libraryName)
        true
    }.onFailure {
        // JNI is unavailable here; never include loader messages or filesystem paths.
        runCatching { Log.e("KeytaoNative", "{\"cat\":\"error\",\"ev\":\"native_lib_missing\"}") }
    }.getOrDefault(false)

    fun configureLibjd(dataDir: String) {
        if (loaded) nativeConfigureLibjd(dataDir)
    }

    private external fun nativeConfigureLibjd(dataDir: String)

    inline fun rtLog(
        level: Int,
        cat: String,
        ev: String,
        durMs: Double = Double.NaN,
        noinline build: (JSONObject.() -> Unit)? = null,
    ) {
        if (!loaded || !runCatching { nativeLogEnabled(level) }.getOrDefault(false)) return
        val kv = runCatching { build?.let { JSONObject().apply(it).toString() } }.getOrNull()
        runCatching { nativeLogEvent(level, cat, ev, durMs, kv) }
    }

    fun resolveThemeJson(
        defaultThemePath: String?,
        userThemePath: String?,
        systemColorScheme: String?,
    ): String? {
        if (!loaded) return null
        return runCatching { nativeResolveThemeJson(defaultThemePath, userThemePath, systemColorScheme) }
            .getOrNull()
            ?.takeIf { it.isNotBlank() }
    }

    fun writeThemeUi(path: String, colorScheme: String, accentHex: String?): Boolean {
        if (!loaded) return false
        return runCatching { nativeWriteThemeUi(path, colorScheme, accentHex) }.getOrDefault(false)
    }

    fun writeThemeAppearance(
        path: String,
        colorScheme: String,
        accentHex: String?,
        cornerRadius: Float,
        backgroundHex: String?,
    ): Boolean {
        if (!loaded) return false
        return runCatching {
            nativeWriteThemeAppearance(path, colorScheme, accentHex, cornerRadius, backgroundHex)
        }.getOrDefault(false)
    }

    fun defaultKeyboardYaml(): String? {
        if (!loaded) return null
        return runCatching { nativeDefaultKeyboardYaml() }
            .getOrNull()
            ?.takeIf { it.isNotBlank() }
    }

    fun prepareRimeLuaInstall(
        localContent: String?,
        packageContent: String?,
        packageLuaFilenames: Set<String>,
    ): RimeLuaMergeResult? {
        check(loaded) { "KeyTao native bridge is unavailable" }
        val json = runCatching {
            nativePrepareRimeLuaInstall(
                localContent,
                packageContent,
                JSONArray(packageLuaFilenames.toList()).toString(),
            )
        }.getOrElse { throw IllegalStateException("Failed to prepare rime.lua installation", it) }
            ?: error("Failed to prepare rime.lua installation")
        val result = JSONObject(json)
        if (result.isNull("mergedContent")) return null
        val renames = result.getJSONArray("renames")
        return RimeLuaMergeResult(
            result.getString("mergedContent"),
            (0 until renames.length()).map { index ->
                val pair = renames.getJSONArray(index)
                pair.getString(0) to pair.getString(1)
            },
        )
    }

    fun mergeDefaultCustom(existing: String?, packageContent: String): MergeResult {
        check(loaded) { "KeyTao native bridge is unavailable" }
        val json = runCatching { nativeMergeDefaultCustom(existing, packageContent) }
            .getOrElse { throw IllegalStateException("Failed to merge scheme configuration", it) }
            ?: error("Failed to merge scheme configuration")
        val result = JSONObject(json)
        val userSchemas = result.getJSONArray("userSchemas")
        return MergeResult(
            result.getString("mergedContent"),
            (0 until userSchemas.length()).map { userSchemas.getString(it) },
        )
    }

    fun mergeLocalDefaultCustom(existing: String?, packageContent: String): MergeResult {
        check(loaded) { "KeyTao native bridge is unavailable" }
        val json = runCatching { nativeMergeLocalDefaultCustom(existing, packageContent) }
            .getOrElse { throw IllegalStateException("Failed to merge local scheme configuration", it) }
            ?: error("Failed to merge local scheme configuration")
        val result = JSONObject(json)
        val userSchemas = result.getJSONArray("userSchemas")
        return MergeResult(
            result.getString("mergedContent"),
            (0 until userSchemas.length()).map { userSchemas.getString(it) },
        )
    }

    fun prepareProtectedFileInstall(relative: String, existing: ByteArray?): ProtectedFileInstall {
        check(loaded) { "KeyTao native bridge is unavailable" }
        val json = runCatching { nativePrepareProtectedFileInstall(relative, existing) }
            .getOrElse { throw IllegalStateException("Failed to prepare protected file installation", it) }
            ?: error("Failed to prepare protected file installation")
        val result = JSONObject(json)
        return ProtectedFileInstall(
            result.getBoolean("protected"),
            result.getBoolean("write"),
            if (result.isNull("backup_name")) null else result.getString("backup_name"),
            if (result.isNull("log")) null else result.getString("log"),
        )
    }

    fun resolveKeyboardJson(
        defaultKeyboardPath: String?,
        userKeyboardPath: String?,
    ): String? {
        if (!loaded) return null
        return runCatching { nativeResolveKeyboardJson(defaultKeyboardPath, userKeyboardPath) }
            .getOrNull()
            ?.takeIf { it.isNotBlank() }
    }

    fun engineAvailable(): Boolean {
        if (!loaded) return false
        return runCatching { nativeEngineAvailable() }.getOrDefault(false)
    }

    fun hasDeployedSchema(userDir: String): Boolean {
        if (!loaded) return false
        return runCatching { nativeHasDeployedSchema(userDir) }.getOrDefault(false)
    }

    fun isolatedSchemaState(userDir: String): Readiness? {
        if (!loaded) return null
        return when (runCatching { nativeIsolatedSchemaState(userDir) }.getOrNull()) {
            0 -> Readiness.NOT_INSTALLED
            1 -> Readiness.NOT_DEPLOYED
            2 -> Readiness.READY
            else -> null
        }
    }

    fun deploymentInProgress(userDir: String): Boolean =
        loaded && runCatching { nativeDeploymentInProgress(userDir) }.getOrDefault(false)

    fun deploymentBlocksInput(userDir: String): Boolean =
        loaded && runCatching { nativeDeploymentBlocksInput(userDir) }.getOrDefault(false)

    fun deploymentHint(userDir: String): String? =
        if (loaded) runCatching { nativeDeploymentHint(userDir) }.getOrNull() else null

    fun deploymentProgressMessage(userDir: String): String? =
        if (loaded) runCatching { nativeDeploymentProgressMessage(userDir) }.getOrNull() else null

    fun deploymentKeyPressed(userDir: String): String? =
        if (loaded) runCatching { nativeDeploymentKeyPressed(userDir) }.getOrNull() else null

    fun resetDeploymentInput() {
        if (loaded) runCatching { nativeResetDeploymentInput() }
    }

    fun retainSession(session: Long): Long =
        if (loaded && session != 0L) runCatching { nativeRetainSession(session) }.getOrDefault(0L) else 0L

    fun prewarm(session: Long): Boolean =
        loaded && runCatching { nativePrewarm(session) }.getOrDefault(false)

    internal fun completeDeployment(userDir: String): Boolean {
        check(KeytaoAndroidPaths.testRootOverride?.absolutePath == userDir) {
            "Complete deployment fixture requires a test root"
        }
        if (!loaded) return false
        return runCatching { nativeCompleteDeployment(userDir) }.getOrDefault(false)
    }

    fun init(userDir: String, sharedDir: String?, deploy: Boolean): Boolean {
        if (!loaded) return false
        return runCatching { nativeInit(userDir, sharedDir, deploy) }.getOrDefault(false).also {
            KeytaoRuntimeLog.nativeInitialized()
        }
    }

    fun reinitialize(userDir: String, sharedDir: String?): Boolean {
        if (!loaded) return false
        return runCatching { nativeReinitialize(userDir, sharedDir) }.getOrDefault(false).also {
            KeytaoRuntimeLog.nativeInitialized()
        }
    }

    fun engineEnabled(userDir: String): Boolean =
        !loaded || runCatching { nativeEngineEnabled(userDir) }.getOrDefault(true)

    fun suspendEngine(userDir: String): Boolean =
        loaded && runCatching { nativeSuspendEngine(userDir) }.getOrDefault(false)

    fun createSession(): Long {
        if (!loaded) return 0L
        return runCatching { nativeCreateSession() }.getOrDefault(0L)
    }

    fun destroySession(session: Long) {
        if (!loaded || session == 0L) return
        runCatching { nativeDestroySession(session) }
    }

    fun sessionState(session: Long): KeytaoImeState? {
        if (!loaded || session == 0L) return null
        return KeytaoImeState.fromJson(runCatching { nativeSessionState(session) }.getOrNull())
    }

    fun drainPendingState(session: Long): KeytaoImeState? {
        if (!loaded || session == 0L) return null
        return KeytaoImeState.fromJson(runCatching { nativeDrainPendingState(session) }.getOrNull())
    }

    fun processKey(session: Long, keyValue: Int, modifiers: Int): KeytaoImeState? {
        if (!loaded || session == 0L) return null
        return KeytaoImeState.fromJson(
            runCatching { nativeProcessKey(session, keyValue, modifiers) }.getOrNull()
        )
    }

    fun processEnter(session: Long): KeytaoImeState? {
        if (!loaded || session == 0L) return null
        return KeytaoImeState.fromJson(
            runCatching { nativeProcessEnter(session) }.getOrNull()
        )
    }

    fun selectCandidate(session: Long, index: Int): KeytaoImeState? {
        if (!loaded || session == 0L) return null
        return KeytaoImeState.fromJson(
            runCatching { nativeSelectCandidate(session, index) }.getOrNull()
        )
    }

    fun deleteCandidate(session: Long, index: Int): KeytaoImeState? {
        if (!loaded || session == 0L) return null
        return KeytaoImeState.fromJson(
            runCatching { nativeDeleteCandidate(session, index) }.getOrNull()
        )
    }

    fun candidateIsUserPhrase(session: Long, index: Int): Boolean {
        return loaded && session != 0L &&
            runCatching { nativeCandidateIsUserPhrase(session, index) }.getOrDefault(false)
    }

    fun selectCandidateGlobal(session: Long, index: Int): KeytaoImeState? {
        if (!loaded || session == 0L) return null
        return KeytaoImeState.fromJson(
            runCatching { nativeSelectCandidateGlobal(session, index) }.getOrNull()
        )
    }

    fun allCandidates(session: Long, limit: Int): List<KeytaoCandidate> {
        if (!loaded || session == 0L) return emptyList()
        return KeytaoImeState.parseCandidateArray(
            runCatching { nativeAllCandidates(session, limit.coerceAtLeast(0)) }.getOrNull()
        )
    }

    fun listSchemas(session: Long): List<KeytaoRimeSchema> {
        if (!loaded || session == 0L) return emptyList()
        return KeytaoRimeSchema.parseArray(runCatching { nativeListSchemas(session) }.getOrNull())
    }

    fun listSwitcherPackages(session: Long): List<KeytaoRimeSwitcherPackage> {
        if (!loaded || session == 0L) return emptyList()
        return KeytaoRimeSwitcherPackage.parseArray(
            runCatching { nativeListSwitcherPackages(session) }.getOrNull()
        )
    }

    fun schemaSwitches(session: Long): List<KeytaoRimeSchemaSwitch> {
        if (!loaded || session == 0L) return emptyList()
        return KeytaoRimeSchemaSwitch.parseArray(
            runCatching { nativeSchemaSwitches(session) }.getOrNull()
        )
    }

    fun currentSchema(session: Long): KeytaoRimeSchema? {
        if (!loaded || session == 0L) return null
        return KeytaoRimeSchema.fromJson(runCatching { nativeCurrentSchema(session) }.getOrNull())
    }

    fun selectSchema(session: Long, schemaId: String): KeytaoImeState? {
        if (!loaded || session == 0L || schemaId.isBlank()) return null
        return KeytaoImeState.fromJson(
            runCatching { nativeSelectSchema(session, schemaId) }.getOrNull()
        )
    }

    fun selectPackageSchema(session: Long, packageKey: String, schemaId: String): KeytaoImeState? {
        if (!loaded || session == 0L || packageKey.isBlank() || schemaId.isBlank()) return null
        return KeytaoImeState.fromJson(
            runCatching { nativeSelectPackageSchema(session, packageKey, schemaId) }.getOrNull()
        )
    }

    fun getOption(session: Long, optionName: String): Boolean {
        if (!loaded || session == 0L || optionName.isBlank()) return false
        return runCatching { nativeGetOption(session, optionName) }.getOrDefault(false)
    }

    fun setOption(session: Long, optionName: String, enabled: Boolean): KeytaoImeState? {
        if (!loaded || session == 0L || optionName.isBlank()) return null
        return KeytaoImeState.fromJson(
            runCatching { nativeSetOption(session, optionName, enabled) }.getOrNull()
        )
    }

    fun changePage(session: Long, backward: Boolean): KeytaoImeState? {
        if (!loaded || session == 0L) return null
        return KeytaoImeState.fromJson(
            runCatching { nativeChangePage(session, backward) }.getOrNull()
        )
    }

    fun reset(session: Long): KeytaoImeState? {
        if (!loaded || session == 0L) return null
        return KeytaoImeState.fromJson(runCatching { nativeReset(session) }.getOrNull())
    }

    fun commitComposition(session: Long): KeytaoImeState? {
        if (!loaded || session == 0L) return null
        return KeytaoImeState.fromJson(runCatching { nativeCommitComposition(session) }.getOrNull())
    }

    fun clearComposition(session: Long): KeytaoImeState? {
        if (!loaded || session == 0L) return null
        return KeytaoImeState.fromJson(runCatching { nativeClearComposition(session) }.getOrNull())
    }

    /**
     * Sensitive editors turn composing off so keys never reach librime, which is
     * also what keeps them out of the user dictionary.
     */
    fun setInputPolicy(session: Long, composing: Boolean, learning: Boolean): KeytaoImeState? {
        if (!loaded || session == 0L) return null
        return KeytaoImeState.fromJson(
            runCatching { nativeSetInputPolicy(session, composing, learning) }.getOrNull()
        )
    }

    fun inputPolicyComposing(session: Long): Boolean {
        if (!loaded || session == 0L) return true
        return runCatching { nativeInputPolicyComposing(session) }.getOrDefault(true)
    }

    fun inputPolicyLearning(session: Long): Boolean {
        if (!loaded || session == 0L) return true
        return runCatching { nativeInputPolicyLearning(session) }.getOrDefault(true)
    }

    /**
     * The X11 keysym for a soft keyboard key, or 0 when the text has to be
     * committed straight to the editor instead of going through Rime.
     */
    fun textToKeysym(text: String): Int {
        if (!loaded) return 0
        return runCatching { nativeTextToKeysym(text) }.getOrDefault(0)
    }

    fun shouldBypassKey(session: Long, keyValue: Int, modifiers: Int): Boolean {
        if (!loaded || session == 0L) return false
        return runCatching { nativeShouldBypassKey(session, keyValue, modifiers) }.getOrDefault(false)
    }

    /** Turn a Unicode scalar offset into the UTF-16 offset `InputConnection` counts in. */
    fun utf16OffsetFromChars(text: String, charOffset: Int): Int {
        if (!loaded) return charOffset.coerceIn(0, text.length)
        return runCatching { nativeUtf16OffsetFromChars(text, charOffset) }
            .getOrDefault(charOffset.coerceIn(0, text.length))
    }

    /**
     * Signature of the reload signal keytao-core writes after a deployment, or
     * null when no reload has been requested yet.
     */
    fun reloadStampSignature(userDir: String): String? {
        if (!loaded) return null
        return runCatching { nativeReloadStampSignature(userDir) }
            .getOrNull()
            ?.takeIf { it.isNotBlank() }
    }

    fun reloadStampPath(userDir: String): String? {
        if (!loaded) return null
        return runCatching { nativeReloadStampPath(userDir) }
            .getOrNull()
            ?.takeIf { it.isNotBlank() }
    }

    fun asciiMode(session: Long): Boolean {
        if (!loaded || session == 0L) return false
        return runCatching { nativeGetAsciiMode(session) }.getOrDefault(false)
    }

    fun setAsciiMode(session: Long, enabled: Boolean): KeytaoImeState? {
        if (!loaded || session == 0L) return null
        return KeytaoImeState.fromJson(
            runCatching { nativeSetAsciiMode(session, enabled) }.getOrNull()
        )
    }

    external fun nativeLogEnabled(level: Int): Boolean

    external fun nativeLogEvent(level: Int, cat: String, ev: String, durMs: Double, kvJson: String?)

    external fun nativeLogKeystroke(ms: Double)

    external fun nativeLogDrainKeyStats()

    external fun nativeLogFlush(timeoutMs: Int)

    external fun nativeResolveThemeJson(
        defaultThemePath: String?,
        userThemePath: String?,
        systemColorScheme: String?,
    ): String

    external fun nativeWriteThemeUi(path: String, colorScheme: String, accentHex: String?): Boolean

    external fun nativeWriteThemeAppearance(
        path: String,
        colorScheme: String,
        accentHex: String?,
        cornerRadius: Float,
        backgroundHex: String?,
    ): Boolean

    external fun nativeDefaultKeyboardYaml(): String

    external fun nativePrepareRimeLuaInstall(
        localContent: String?,
        packageContent: String?,
        packageLuaFilenamesJson: String,
    ): String?

    external fun nativePrepareProtectedFileInstall(relative: String, existing: ByteArray?): String?

    external fun nativeMergeDefaultCustom(existing: String?, packageContent: String): String?

    external fun nativeMergeLocalDefaultCustom(existing: String?, packageContent: String): String?

    external fun nativeResolveKeyboardJson(
        defaultKeyboardPath: String?,
        userKeyboardPath: String?,
    ): String

    external fun nativeRenderSchemaLogos(userDir: String)

    external fun nativeEngineAvailable(): Boolean

    private external fun nativeEngineEnabled(userDir: String): Boolean

    private external fun nativeSuspendEngine(userDir: String): Boolean

    external fun nativeDeployPackages(userDir: String): String?

    external fun nativeHasDeployedSchema(userDir: String): Boolean

    external fun nativeIsolatedSchemaState(userDir: String): Int

    external fun nativeDeploymentInProgress(userDir: String): Boolean

    external fun nativeDeploymentBlocksInput(userDir: String): Boolean

    external fun nativeDeploymentHint(userDir: String): String?

    external fun nativeDeploymentProgressMessage(userDir: String): String?

    external fun nativeDeploymentKeyPressed(userDir: String): String?

    external fun nativeResetDeploymentInput()

    external fun nativeRetainSession(session: Long): Long

    external fun nativePrewarm(session: Long): Boolean

    external fun nativeCompleteDeployment(userDir: String): Boolean

    external fun nativeInit(userDir: String, sharedDir: String?, deploy: Boolean): Boolean

    external fun nativeReinitialize(userDir: String, sharedDir: String?): Boolean

    external fun nativeCreateSession(): Long

    external fun nativeDestroySession(session: Long)

    external fun nativeSessionState(session: Long): String?

    external fun nativeDrainPendingState(session: Long): String?

    external fun nativeProcessKey(session: Long, keyValue: Int, modifiers: Int): String?

    external fun nativeProcessEnter(session: Long): String?

    external fun nativeSelectCandidate(session: Long, index: Int): String?

    external fun nativeDeleteCandidate(session: Long, index: Int): String?

    external fun nativeCandidateIsUserPhrase(session: Long, index: Int): Boolean

    external fun nativeSelectCandidateGlobal(session: Long, index: Int): String?

    external fun nativeAllCandidates(session: Long, limit: Int): String?

    external fun nativeListSchemas(session: Long): String?

    external fun nativeListSwitcherPackages(session: Long): String?

    external fun nativeSchemaSwitches(session: Long): String?

    external fun nativeCurrentSchema(session: Long): String?

    external fun nativeSelectSchema(session: Long, schemaId: String): String?

    external fun nativeSelectPackageSchema(session: Long, packageKey: String, schemaId: String): String?

    external fun nativeGetOption(session: Long, optionName: String): Boolean

    external fun nativeSetOption(session: Long, optionName: String, enabled: Boolean): String?

    external fun nativeChangePage(session: Long, backward: Boolean): String?

    external fun nativeReset(session: Long): String?

    external fun nativeCommitComposition(session: Long): String?

    external fun nativeClearComposition(session: Long): String?

    external fun nativeSetInputPolicy(session: Long, composing: Boolean, learning: Boolean): String?

    external fun nativeInputPolicyComposing(session: Long): Boolean

    external fun nativeInputPolicyLearning(session: Long): Boolean

    external fun nativeGetAsciiMode(session: Long): Boolean

    external fun nativeSetAsciiMode(session: Long, enabled: Boolean): String?

    external fun nativeTextToKeysym(text: String): Int

    external fun nativeShouldBypassKey(session: Long, keyValue: Int, modifiers: Int): Boolean

    external fun nativeUtf16OffsetFromChars(text: String, charOffset: Int): Int

    external fun nativeReloadStampSignature(userDir: String): String?

    external fun nativeReloadStampPath(userDir: String): String
}

data class KeytaoRimeSchema(
    val id: String,
    val name: String,
) {
    companion object {
        fun fromJson(json: String?): KeytaoRimeSchema? {
            if (json.isNullOrBlank()) return null
            return runCatching { fromJsonObject(JSONObject(json)) }.getOrNull()
        }

        fun parseArray(json: String?): List<KeytaoRimeSchema> {
            if (json.isNullOrBlank()) return emptyList()
            return runCatching { parseArray(JSONArray(json)) }.getOrDefault(emptyList())
        }

        fun parseArray(array: JSONArray?): List<KeytaoRimeSchema> {
            if (array == null) return emptyList()
            return buildList {
                for (index in 0 until array.length()) {
                    array.optJSONObject(index)?.let(::fromJsonObject)?.let(::add)
                }
            }
        }

        private fun fromJsonObject(root: JSONObject): KeytaoRimeSchema? {
            val id = root.optString("id").trim()
            if (id.isEmpty()) return null
            return KeytaoRimeSchema(
                id = id,
                name = root.optString("name").trim().ifEmpty { id },
            )
        }
    }
}

data class KeytaoRimeSwitcherPackage(
    val key: String,
    val displayName: String,
    val active: Boolean,
    val ready: Boolean,
    val schemas: List<KeytaoRimeSchema>,
) {
    companion object {
        fun parseArray(json: String?): List<KeytaoRimeSwitcherPackage> {
            if (json.isNullOrBlank()) return emptyList()
            return runCatching {
                val array = JSONArray(json)
                buildList {
                    for (index in 0 until array.length()) {
                        val entry = array.optJSONObject(index) ?: continue
                        val key = entry.optString("package_key").trim()
                        if (key.isEmpty()) continue
                        add(KeytaoRimeSwitcherPackage(
                            key = key,
                            displayName = entry.optString("display_name").trim().ifEmpty { key },
                            active = entry.optBoolean("active"),
                            ready = entry.optBoolean("ready"),
                            schemas = KeytaoRimeSchema.parseArray(entry.optJSONArray("schemas")),
                        ))
                    }
                }
            }.getOrDefault(emptyList())
        }
    }
}

data class KeytaoRimeSchemaSwitch(
    val name: String?,
    val options: List<String>,
    val states: List<String>,
    val reset: Int?,
) {
    val optionNames: List<String>
        get() = options.ifEmpty { listOfNotNull(name) }

    companion object {
        fun parseArray(json: String?): List<KeytaoRimeSchemaSwitch> {
            if (json.isNullOrBlank()) return emptyList()
            return runCatching {
                val array = JSONArray(json)
                buildList {
                    for (index in 0 until array.length()) {
                        val root = array.optJSONObject(index) ?: continue
                        val name = if (root.has("name") && !root.isNull("name")) {
                            root.optString("name").trim().takeIf(String::isNotEmpty)
                        } else {
                            null
                        }
                        val options = root.stringList("options")
                        if (name == null && options.isEmpty()) continue
                        add(
                            KeytaoRimeSchemaSwitch(
                                name = name,
                                options = options,
                                states = root.stringList("states"),
                                reset = if (root.has("reset") && !root.isNull("reset")) {
                                    root.optInt("reset")
                                } else {
                                    null
                                },
                            )
                        )
                    }
                }
            }.getOrDefault(emptyList())
        }

        private fun JSONObject.stringList(name: String): List<String> {
            val values = optJSONArray(name) ?: return emptyList()
            return buildList {
                for (index in 0 until values.length()) {
                    values.optString(index).trim().takeIf(String::isNotEmpty)?.let(::add)
                }
            }
        }
    }
}
