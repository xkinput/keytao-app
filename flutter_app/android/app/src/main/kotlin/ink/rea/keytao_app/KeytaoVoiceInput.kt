package ink.rea.keytao_app

internal class KeytaoVoiceInput {
    private var generation = 0L
    var listening = false
        private set

    fun start(): Long {
        listening = true
        return ++generation
    }

    fun cancel() {
        generation++
        listening = false
    }

    fun runIfCurrent(token: Long, action: () -> Unit) {
        if (listening && token == generation) action()
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
