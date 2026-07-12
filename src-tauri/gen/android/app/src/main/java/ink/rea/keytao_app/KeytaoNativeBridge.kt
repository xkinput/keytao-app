package ink.rea.keytao_app

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

    fun selectCandidate(session: Long, index: Int): KeytaoImeState? {
        if (!loaded || session == 0L) return null
        return KeytaoImeState.fromJson(
            runCatching { nativeSelectCandidate(session, index) }.getOrNull()
        )
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

    external fun nativeSelectCandidate(session: Long, index: Int): String?

    external fun nativeSelectCandidateGlobal(session: Long, index: Int): String?

    external fun nativeAllCandidates(session: Long, limit: Int): String?

    external fun nativeChangePage(session: Long, backward: Boolean): String?

    external fun nativeReset(session: Long): String?

    external fun nativeGetAsciiMode(session: Long): Boolean

    external fun nativeSetAsciiMode(session: Long, enabled: Boolean): String?
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
