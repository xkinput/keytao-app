package ink.rea.keytao_app

import org.json.JSONArray
import org.json.JSONObject

object KeytaoNativeBridge {
    private const val libraryName = "keytao_app_lib"

    val loaded: Boolean = runCatching {
        System.loadLibrary(libraryName)
        true
    }.getOrDefault(false)

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

    fun defaultKeyboardYaml(): String? {
        if (!loaded) return null
        return runCatching { nativeDefaultKeyboardYaml() }
            .getOrNull()
            ?.takeIf { it.isNotBlank() }
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

    fun deployStep(userDir: String, sharedDir: String?, schemaId: String?): KeytaoRimeDeployStepResult {
        if (!loaded) return KeytaoRimeDeployStepResult(error = "KeyTao native bridge is unavailable")
        val json = runCatching { nativeDeployStep(userDir, sharedDir, schemaId) }.getOrNull()
        return KeytaoRimeDeployStepResult.fromJson(json)
    }

    fun init(userDir: String, sharedDir: String?, deploy: Boolean): Boolean {
        if (!loaded) return false
        return runCatching { nativeInit(userDir, sharedDir, deploy) }.getOrDefault(false)
    }

    fun reinitialize(userDir: String, sharedDir: String?): Boolean {
        if (!loaded) return false
        return runCatching { nativeReinitialize(userDir, sharedDir) }.getOrDefault(false)
    }

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

    fun highlightCandidate(session: Long, index: Int): KeytaoImeState? {
        if (!loaded || session == 0L) return null
        return KeytaoImeState.fromJson(
            runCatching { nativeHighlightCandidate(session, index) }.getOrNull()
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

    fun isEnterKey(keyValue: Int): Boolean {
        if (!loaded) return false
        return runCatching { nativeIsEnterKey(keyValue) }.getOrDefault(false)
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

    external fun nativeResolveThemeJson(
        defaultThemePath: String?,
        userThemePath: String?,
        systemColorScheme: String?,
    ): String

    external fun nativeDefaultKeyboardYaml(): String

    external fun nativeResolveKeyboardJson(
        defaultKeyboardPath: String?,
        userKeyboardPath: String?,
    ): String

    external fun nativeEngineAvailable(): Boolean

    external fun nativeDeployStep(userDir: String, sharedDir: String?, schemaId: String?): String

    external fun nativeInit(userDir: String, sharedDir: String?, deploy: Boolean): Boolean

    external fun nativeReinitialize(userDir: String, sharedDir: String?): Boolean

    external fun nativeCreateSession(): Long

    external fun nativeDestroySession(session: Long)

    external fun nativeSessionState(session: Long): String?

    external fun nativeProcessKey(session: Long, keyValue: Int, modifiers: Int): String?

    external fun nativeProcessEnter(session: Long): String?

    external fun nativeSelectCandidate(session: Long, index: Int): String?

    external fun nativeHighlightCandidate(session: Long, index: Int): String?

    external fun nativeDeleteCandidate(session: Long, index: Int): String?

    external fun nativeCandidateIsUserPhrase(session: Long, index: Int): Boolean

    external fun nativeSelectCandidateGlobal(session: Long, index: Int): String?

    external fun nativeAllCandidates(session: Long, limit: Int): String?

    external fun nativeListSchemas(session: Long): String?

    external fun nativeSchemaSwitches(session: Long): String?

    external fun nativeCurrentSchema(session: Long): String?

    external fun nativeSelectSchema(session: Long, schemaId: String): String?

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

    external fun nativeIsEnterKey(keyValue: Int): Boolean

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
            return runCatching {
                val array = JSONArray(json)
                buildList {
                    for (index in 0 until array.length()) {
                        array.optJSONObject(index)?.let(::fromJsonObject)?.let(::add)
                    }
                }
            }.getOrDefault(emptyList())
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

data class KeytaoRimeDeployStepResult(
    val success: Boolean = false,
    val schemas: List<String> = emptyList(),
    val error: String = "",
) {
    companion object {
        fun fromJson(json: String?): KeytaoRimeDeployStepResult {
            if (json.isNullOrBlank()) {
                return KeytaoRimeDeployStepResult(error = "Android RIME deployment returned no result")
            }
            return runCatching {
                val root = JSONObject(json)
                val values = root.optJSONArray("schemas")
                val schemas = buildList {
                    if (values != null) {
                        for (index in 0 until values.length()) {
                            values.optString(index).trim().takeIf(String::isNotEmpty)?.let(::add)
                        }
                    }
                }
                KeytaoRimeDeployStepResult(
                    success = root.optBoolean("success", false),
                    schemas = schemas,
                    error = root.optString("error"),
                )
            }.getOrElse { error ->
                KeytaoRimeDeployStepResult(
                    error = error.message ?: "Invalid Android RIME deployment result",
                )
            }
        }
    }
}
