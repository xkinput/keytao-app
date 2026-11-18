package ink.rea.keytao_app

internal class KeytaoChordTouches {
    data class Event(val text: String, val released: Boolean)

    private val held = linkedMapOf<Int, String>()

    fun begin(
        pointerId: Int,
        command: KeyCommand,
        chordComposer: Boolean,
        asciiMode: Boolean,
        shiftActive: Boolean = false,
    ): Event? {
        val text = command.value ?: return null
        if (!chordComposer || asciiMode || shiftActive ||
            (command.type != KeyCommandTypes.INPUT && command.type != KeyCommandTypes.RIME_INPUT) ||
            text.length != 1 || text[0].code !in 0x20..0x7e || pointerId in held
        ) return null
        held[pointerId] = text
        return Event(text, released = false)
    }

    fun contains(pointerId: Int): Boolean = pointerId in held

    fun finish(pointerId: Int): Event? = held.remove(pointerId)?.let { Event(it, released = true) }

    fun releaseAll(removeTouch: (Int) -> Unit = {}): List<Event> {
        val touches = held.toList()
        held.clear()
        touches.forEach { (pointerId, _) -> removeTouch(pointerId) }
        return touches.map { (_, text) -> Event(text, released = true) }
    }
}

internal fun KeyCommand.consumesOneShotShift(chordComposer: Boolean): Boolean =
    (type == KeyCommandTypes.INPUT || (chordComposer && type == KeyCommandTypes.RIME_INPUT)) &&
        value?.let { it.length == 1 && it[0].isLetter() } == true

internal fun KeyCommand.applyingShift(shiftActive: Boolean, chordComposer: Boolean): KeyCommand {
    if (!shiftActive || !consumesOneShotShift(chordComposer)) return this
    return copy(
        value = value?.uppercase(),
        fallbackValue = if (chordComposer && type == KeyCommandTypes.RIME_INPUT &&
            fallbackValue?.let { it.length == 1 && it[0].isLetter() } == true
        ) fallbackValue.uppercase() else fallbackValue,
    )
}

internal class KeytaoHardwareChordKeys {
    data class Release(val key: RimeKey?, val passToHost: Boolean)

    private val acceptedPresses = linkedMapOf<Int, RimeKey>()
    private val passedPresses = mutableSetOf<Int>()

    fun recordPress(keyCode: Int, key: RimeKey, accepted: Boolean, chordComposer: Boolean, asciiMode: Boolean) {
        if (accepted && chordComposer && !asciiMode && key.keyCode in 0x20..0x7e && key.modifiers == 0) {
            acceptedPresses.putIfAbsent(keyCode, key)
        }
    }

    fun passToHost(keyCode: Int) {
        passedPresses.add(keyCode)
    }

    fun release(keyCode: Int): Release? {
        val key = acceptedPresses.remove(keyCode)?.let {
            it.copy(modifiers = it.modifiers or AndroidKeyMapper.RIME_RELEASE_MASK)
        }
        val passToHost = passedPresses.remove(keyCode)
        return if (key != null || passToHost) Release(key, passToHost) else null
    }

    fun releaseAll(): List<RimeKey> {
        val releases = acceptedPresses.values.map {
            it.copy(modifiers = it.modifiers or AndroidKeyMapper.RIME_RELEASE_MASK)
        }
        acceptedPresses.clear()
        return releases
    }
}
