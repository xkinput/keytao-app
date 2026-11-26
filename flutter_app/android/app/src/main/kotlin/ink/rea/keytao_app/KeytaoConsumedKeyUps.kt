package ink.rea.keytao_app

internal class KeytaoConsumedKeyUps {
    private val keys = mutableSetOf<Pair<Int, Int>>()

    fun recordDown(deviceId: Int, keyCode: Int) {
        keys.add(deviceId to keyCode)
    }

    fun removeDown(deviceId: Int, keyCode: Int): Boolean = keys.remove(deviceId to keyCode)

    fun clear() = keys.clear()
}
