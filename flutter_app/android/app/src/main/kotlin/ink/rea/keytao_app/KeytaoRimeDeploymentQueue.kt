package ink.rea.keytao_app

// The service main handler owns this queue; a completed batch never absorbs newer requests.
internal class KeytaoRimeDeploymentQueue<T> {
    private var current = emptyList<T>()
    private val pending = mutableListOf<T>()

    val running: Boolean get() = current.isNotEmpty()
    val requests: List<T> get() = current
    val rerun: Boolean get() = pending.isNotEmpty()

    fun enqueue(request: T): Boolean {
        if (running) {
            pending.add(request)
            return false
        }
        current = listOf(request)
        return true
    }

    fun complete(): List<T> {
        val completed = current
        current = pending.toList()
        pending.clear()
        return completed
    }
}
