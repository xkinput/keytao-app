package ink.rea.keytao_app

internal class KeytaoVoiceInput {
    enum class State { IDLE, RECORDING, TRANSCRIBING }

    private var generation = 0L
    var state = State.IDLE
        private set

    fun start(): Long {
        state = State.RECORDING
        return ++generation
    }

    fun transcribe() {
        if (state == State.RECORDING) state = State.TRANSCRIBING
    }

    fun cancel() {
        generation++
        state = State.IDLE
    }

    fun runIfCurrent(token: Long, action: () -> Unit) {
        if (state != State.IDLE && token == generation) action()
    }

    companion object {
        fun shouldMarkPermissionPermanentlyDenied(
            denied: Boolean,
            rationaleBeforeRequest: Boolean?,
            rationaleAfterRequest: Boolean,
        ): Boolean {
            return denied && rationaleBeforeRequest == true && !rationaleAfterRequest
        }

        fun isOffered(
            recognitionAvailable: Boolean,
            privacyAllowed: Boolean,
            hasComposition: Boolean,
            hasConfiguredAction: Boolean,
            permanentlyDenied: Boolean,
        ): Boolean {
            return recognitionAvailable && privacyAllowed && !hasComposition &&
                !hasConfiguredAction && !permanentlyDenied
        }
    }
}
