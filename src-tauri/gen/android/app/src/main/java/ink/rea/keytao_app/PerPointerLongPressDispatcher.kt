package ink.rea.keytao_app

internal class PerPointerLongPressDispatcher<State>(
    private val postDelayed: (Runnable, Long) -> Unit,
    private val removeCallbacks: (Runnable) -> Unit,
) {
    private val active = linkedMapOf<Int, State>()
    private val pending = mutableMapOf<Int, Runnable>()

    fun isNotEmpty(): Boolean = active.isNotEmpty()

    val values: List<State>
        get() = active.values.toList()

    operator fun get(pointerId: Int): State? = active[pointerId]

    fun begin(
        pointerId: Int,
        state: State,
        delayMs: Long? = null,
        onLongPress: ((State) -> Unit)? = null,
    ) {
        cancelLongPress(pointerId)
        active[pointerId] = state
        if (delayMs == null || onLongPress == null) return

        lateinit var runnable: Runnable
        runnable = Runnable {
            if (pending[pointerId] !== runnable) return@Runnable
            pending.remove(pointerId)
            active[pointerId]?.let(onLongPress)
        }
        pending[pointerId] = runnable
        postDelayed(runnable, delayMs)
    }

    fun finish(pointerId: Int): State? {
        cancelLongPress(pointerId)
        return active.remove(pointerId)
    }

    fun cancelLongPress(pointerId: Int) {
        pending.remove(pointerId)?.let(removeCallbacks)
    }

    fun cancelAllLongPress() {
        pending.values.toList().forEach(removeCallbacks)
        pending.clear()
    }

    fun clear() {
        cancelAllLongPress()
        active.clear()
    }
}
