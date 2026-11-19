package ink.rea.keytao_app

internal class KeytaoVoiceInput {
    enum class State { IDLE, RECORDING, TRANSCRIBING }

    private var generation = 0L
    private var editor: Any? = null
    private var partialText = ""
    var state = State.IDLE
        private set

    fun start(editor: Any? = null): Long {
        this.editor = editor
        partialText = ""
        state = State.RECORDING
        return ++generation
    }

    fun transcribe() {
        if (state == State.RECORDING) state = State.TRANSCRIBING
    }

    fun partial(token: Long, editor: Any?, text: String?, unstableText: String? = null): String? {
        var shown: String? = null
        runIfCurrent(token) {
            val combined = text.orEmpty() + unstableText.orEmpty()
            if (this.editor === editor && combined.isNotEmpty()) {
                partialText = combined
                shown = combined
            }
        }
        return shown
    }

    fun finish(token: Long, editor: Any?, text: String?): String? {
        var committed: String? = null
        runIfCurrent(token) {
            if (this.editor === editor) {
                committed = text?.takeIf { it.isNotEmpty() } ?: partialText.takeIf { it.isNotEmpty() }
                cancel()
            }
        }
        return committed
    }

    fun cancel(editor: Any? = null): Boolean {
        val clear = partialText.isNotEmpty() && this.editor === editor
        generation++
        state = State.IDLE
        this.editor = null
        partialText = ""
        return clear
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
