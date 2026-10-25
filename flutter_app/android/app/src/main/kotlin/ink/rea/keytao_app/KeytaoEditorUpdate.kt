package ink.rea.keytao_app

/** Engine work that must finish before an editor may send its first key. */
internal interface KeytaoEditorEngineUpdate {
    val ready: Boolean
    fun reloadIfNeeded()
    fun applyPolicy()
    fun clearComposition()
    fun restoreMode()
    fun state(): KeytaoImeState
}

internal object KeytaoEditorUpdate {
    fun run(
        engine: KeytaoEditorEngineUpdate,
        reloadIfNeeded: Boolean,
        clearComposition: Boolean,
    ): KeytaoImeState? {
        if (reloadIfNeeded) engine.reloadIfNeeded()
        engine.applyPolicy()
        if (clearComposition && engine.ready) engine.clearComposition()
        engine.restoreMode()
        return if (engine.ready) engine.state().withoutTransientCommit() else null
    }
}
