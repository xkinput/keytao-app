package ink.rea.keytao_app

object KeytaoNativeBridge {
    private const val libraryName = "keytao_app_lib"

    val loaded: Boolean = runCatching {
        System.loadLibrary(libraryName)
        true
    }.getOrDefault(false)

    fun resolveThemeJson(defaultThemePath: String?, userThemePath: String?): String? {
        if (!loaded) return null
        return runCatching { nativeResolveThemeJson(defaultThemePath, userThemePath) }
            .getOrNull()
            ?.takeIf { it.isNotBlank() }
    }

    fun engineAvailable(): Boolean {
        if (!loaded) return false
        return runCatching { nativeEngineAvailable() }.getOrDefault(false)
    }

    fun init(userDir: String, sharedDir: String?): Boolean {
        if (!loaded) return false
        return runCatching { nativeInit(userDir, sharedDir) }.getOrDefault(false)
    }

    fun reload(): Boolean {
        if (!loaded) return false
        return runCatching { nativeReload() }.getOrDefault(false)
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

    external fun nativeResolveThemeJson(defaultThemePath: String?, userThemePath: String?): String

    external fun nativeEngineAvailable(): Boolean

    external fun nativeInit(userDir: String, sharedDir: String?): Boolean

    external fun nativeReload(): Boolean

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
