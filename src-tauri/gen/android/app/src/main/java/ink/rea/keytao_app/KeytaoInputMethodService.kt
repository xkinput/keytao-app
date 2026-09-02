package ink.rea.keytao_app

import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Intent
import android.content.res.Configuration
import android.graphics.Color
import android.graphics.Region
import android.graphics.drawable.ColorDrawable
import android.inputmethodservice.InputMethodService
import android.icu.text.BreakIterator
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.text.InputType
import android.text.Spanned
import android.text.style.ReplacementSpan
import android.util.Log
import android.view.KeyCharacterMap
import android.view.KeyEvent
import android.view.View
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager
import android.view.ViewGroup
import android.widget.FrameLayout
import java.util.Locale
import java.util.concurrent.Executors
import kotlin.math.max
import kotlin.math.roundToInt

class KeytaoInputMethodService : InputMethodService(), KeytaoKeyboardView.Listener {
    private lateinit var engine: KeytaoImeEngine
    private val mainHandler = Handler(Looper.getMainLooper())
    private val candidateExecutor = Executors.newSingleThreadExecutor()
    private val clipboardHistory = mutableListOf<String>()
    private data class ClipboardSnapshot(val text: String, val timestamp: Long?)
    private var clipboardSuppression: ClipboardSnapshot? = null
    private val clipboardListener = ClipboardManager.OnPrimaryClipChangedListener {
        // A clipboard change is a new user/system event even when it writes the
        // same text again, so a deletion suppression cannot survive it.
        clipboardSuppression = null
        rememberCurrentClipboard(suggest = true)
    }
    private var clipboardManager: ClipboardManager? = null
    private var keyboardView: KeytaoKeyboardView? = null
    private var keyboardHost: KeytaoKeyboardHost? = null
    private val floatingTouchableRegion = Region()
    private val keyboardHostLocation = IntArray(2)
    private val keyboardLayoutStateStore by lazy { KeytaoKeyboardLayoutStateStore(applicationContext) }
    private var baseKeyboardConfig: KeytaoAndroidImeConfig? = null
    private var keyboardLayoutState = KeyboardLayoutState(
        mode = KeyboardLayoutMode.FULL,
        floatingScale = 1f,
    )
    private var presentationIsLandscape = false
    private var currentState = KeytaoImeState.empty()
    private var bypassAsciiActive = false
    private var asciiModeBeforeBypass = false

    /**
     * Where the editor put our composing region, in absolute UTF-16 offsets, or
     * -1 while we do not know. Seeded by `onUpdateSelection`, which is the only
     * place the framework reports it, and carried forward across the commits we
     * make ourselves.
     */
    private var composingRegionStart = -1
    private var composing = false
        set(value) {
            field = value
            // The tracked region only describes a live composition; keeping it
            // past the end of one would aim setSelection at stale offsets.
            if (!value) composingRegionStart = -1
        }
    private var selectionModeActive = false
    private var shiftPressedWithoutKey = false
    private var pendingShiftKeyCode = 0
    private var inputAvailable = false
    // Readiness is probed on a background thread, so the first frames of the
    // keyboard must say "still starting up" rather than accuse the user of not
    // having installed a schema.
    private var unavailableMessage = preparingMessage
    private val backspaceRestoreStack = mutableListOf<String>()
    private val recentCommittedUnits = mutableListOf<String>()
    private var lastCommittedText: String? = null
    private var restoreAllOnNextDirectionalRestore = false
    private var privacyMode = InputPrivacyMode()
    private var directInputEditor = false
    private var clipboardListenerRegistered = false
    private var availabilityRefreshPending = false
    private var systemBottomInsetDp = -1
    private val graphemeIterator: BreakIterator by lazy {
        BreakIterator.getCharacterInstance(Locale.ROOT)
    }

    override fun onCreate() {
        super.onCreate()
        engine = KeytaoImeEngine(applicationContext)
        clipboardManager = getSystemService(ClipboardManager::class.java)
        scheduleAvailabilityRefresh()
    }

    override fun onDestroy() {
        unregisterClipboardListener()
        if (::engine.isInitialized) {
            engine.close()
        }
        candidateExecutor.shutdownNow()
        super.onDestroy()
    }

    override fun onCreateInputView(): View {
        window?.window?.setBackgroundDrawable(ColorDrawable(Color.TRANSPARENT))
        val host = KeytaoKeyboardHost(this)
        host.listener = object : KeytaoKeyboardHost.Listener {
            override fun onLayoutStateChanged(state: KeyboardLayoutState, finished: Boolean) {
                handleKeyboardLayoutStateChanged(state, finished)
            }

            override fun onSystemBottomInsetChanged(insetPx: Int) {
                handleSystemBottomInsetChanged(insetPx)
            }
        }
        val view = KeytaoKeyboardView(this)
        view.listener = this
        host.addView(
            view,
            0,
            FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            ),
        )
        view.updateTheme(KeytaoThemeResolver.resolve(this))
        view.updateState(currentState)
        keyboardHost = host
        keyboardView = view
        applyKeyboardPresentation(KeytaoAndroidImeConfig.load(this))
        view.updateSystemBottomInsetDp(systemBottomInsetDp)
        view.updateInputMethodSwitching(canOfferNextInputMethod())
        applyAvailability()
        return host
    }

    override fun onComputeInsets(outInsets: Insets) {
        super.onComputeInsets(outInsets)
        val host = keyboardHost ?: return
        if (!host.populateFloatingTouchableRegion(floatingTouchableRegion)) return

        host.getLocationInWindow(keyboardHostLocation)
        floatingTouchableRegion.translate(keyboardHostLocation[0], keyboardHostLocation[1])

        val hostBottom = keyboardHostLocation[1] + host.height
        outInsets.contentTopInsets = hostBottom
        outInsets.visibleTopInsets = hostBottom
        outInsets.touchableInsets = Insets.TOUCHABLE_INSETS_REGION
        outInsets.touchableRegion.set(floatingTouchableRegion)
    }

    override fun onStartInput(attribute: EditorInfo?, restarting: Boolean) {
        super.onStartInput(attribute, restarting)
        // doStartInput() skips doFinishInput() when restarting, so the editor can
        // still hold a composing region from the previous round.
        if (restarting) {
            currentInputConnection?.finishComposingText()
        }
        applyEditorInfo(attribute)
        val ready = applyAvailability()
        currentState = if (ready) engine.clearComposition().withoutTransientCommit() else KeytaoImeState.empty()
        composing = false
        selectionModeActive = false
        backspaceRestoreStack.clear()
        restoreAllOnNextDirectionalRestore = false
        recentCommittedUnits.clear()
        lastCommittedText = null
        keyboardView?.updateState(currentState)
        scheduleAvailabilityRefresh()
    }

    override fun onStartInputView(info: EditorInfo?, restarting: Boolean) {
        super.onStartInputView(info, restarting)
        applyEditorInfo(info)
        registerClipboardListener()
        keyboardView?.updateTheme(KeytaoThemeResolver.resolve(this))
        applyKeyboardPresentation(KeytaoAndroidImeConfig.load(this))
        keyboardView?.updateInputMethodSwitching(canOfferNextInputMethod())
        applyAvailability()
        keyboardView?.updateState(currentState)
        scheduleReloadIfNeeded()
        scheduleAvailabilityRefresh()
    }

    override fun onFinishInputView(finishingInput: Boolean) {
        unregisterClipboardListener()
        keyboardView?.clearRecentClipboardSuggestion()
        keyboardView?.resetClipboardClearConfirmation()
        super.onFinishInputView(finishingInput)
    }

    /**
     * Apply what the editor declared about itself: privacy contract first, then
     * the keyboard shape (layer and Enter label) it asked for.
     */
    private fun applyEditorInfo(info: EditorInfo?) {
        val inputType = info?.inputType ?: InputType.TYPE_NULL
        val imeOptions = info?.imeOptions ?: EditorInfo.IME_ACTION_NONE
        val nextPrivacy = KeytaoEditorPolicy.resolvePrivacyMode(inputType, imeOptions)
        directInputEditor = KeytaoEditorPolicy.isDirectInputEditor(inputType)
        if (nextPrivacy != privacyMode) {
            privacyMode = nextPrivacy
            if (!nextPrivacy.allowsClipboard) {
                clipboardHistory.clear()
                clipboardSuppression = null
                keyboardView?.clearRecentClipboardSuggestion()
            }
            if (!nextPrivacy.allowsTextRecall) {
                backspaceRestoreStack.clear()
                recentCommittedUnits.clear()
                lastCommittedText = null
                restoreAllOnNextDirectionalRestore = false
            }
        }
        engine.setInputPolicy(
            composing = privacyMode.allowsComposing,
            learning = privacyMode.allowsLearning,
        )?.let { state ->
            currentState = state.withoutTransientCommit()
            composing = false
        }
        val bypass = !privacyMode.allowsComposing || directInputEditor
        if (bypass && !bypassAsciiActive) {
            bypassAsciiActive = true
            asciiModeBeforeBypass = currentState.asciiMode
            currentState = engine.setAsciiMode(true).withoutTransientCommit()
            keyboardView?.updateState(currentState)
        } else if (!bypass && bypassAsciiActive) {
            bypassAsciiActive = false
            currentState = engine.setAsciiMode(asciiModeBeforeBypass).withoutTransientCommit()
            keyboardView?.updateState(currentState)
        }
        val behavior = keyboardView?.currentConfig()?.enterKeyBehavior ?: EnterKeyBehaviors.SYSTEM
        keyboardView?.updateEditorPresentation(
            enterLabel = KeytaoEditorPolicy.resolveEnterLabel(
                inputType = inputType,
                imeOptions = imeOptions,
                actionLabel = info?.actionLabel,
                forceNewline = behavior == EnterKeyBehaviors.NEWLINE,
            ),
            requestedLayer = KeytaoEditorPolicy.resolveInitialLayer(inputType),
        )
        Log.d(
            "KeytaoIme",
            "editor pkg=${currentInputEditorInfo?.packageName} " +
                "inputType=0x${inputType.toString(16)} " +
                "imeOptions=0x${imeOptions.toString(16)} " +
                "composing=${privacyMode.allowsComposing} " +
                "direct=$directInputEditor ready=$inputAvailable",
        )
    }

    /**
     * The framework reports every caret move here, including the ones an app makes
     * behind our back. A composition that no longer covers the caret must be given
     * up, otherwise the next setComposingText rewrites text at the old position.
     */
    override fun onUpdateSelection(
        oldSelStart: Int,
        oldSelEnd: Int,
        newSelStart: Int,
        newSelEnd: Int,
        candidatesStart: Int,
        candidatesEnd: Int,
    ) {
        super.onUpdateSelection(
            oldSelStart,
            oldSelEnd,
            newSelStart,
            newSelEnd,
            candidatesStart,
            candidatesEnd,
        )
        selectionModeActive = newSelStart != newSelEnd
        composingRegionStart = if (candidatesStart >= 0 && candidatesEnd >= candidatesStart) {
            candidatesStart
        } else {
            -1
        }
        if (!composing && !currentState.hasComposition) return
        val insideComposition = candidatesStart >= 0 &&
            candidatesEnd >= candidatesStart &&
            newSelStart >= candidatesStart &&
            newSelEnd <= candidatesEnd
        if (insideComposition) return
        abandonCompositionAfterExternalEdit()
    }

    private fun abandonCompositionAfterExternalEdit() {
        currentInputConnection?.finishComposingText()
        composing = false
        backspaceRestoreStack.clear()
        restoreAllOnNextDirectionalRestore = false
        recentCommittedUnits.clear()
        currentState = if (inputAvailable) {
            engine.clearComposition().withoutTransientCommit()
        } else {
            KeytaoImeState.empty(asciiMode = currentState.asciiMode)
        }
        keyboardView?.updateState(currentState)
    }

    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        keyboardView?.updateTheme(KeytaoThemeResolver.resolve(this))
        applyKeyboardPresentation(KeytaoAndroidImeConfig.load(this))
    }

    private fun applyKeyboardPresentation(config: KeytaoAndroidImeConfig) {
        val isLandscape = resources.configuration.orientation == Configuration.ORIENTATION_LANDSCAPE
        baseKeyboardConfig = config
        presentationIsLandscape = isLandscape
        keyboardLayoutState = keyboardLayoutStateStore.load(isLandscape, config.floating.profile(isLandscape))
        applyKeyboardPresentation(config, keyboardLayoutState)
    }

    private fun applyKeyboardPresentation(
        config: KeytaoAndroidImeConfig,
        state: KeyboardLayoutState,
    ) {
        val normalized = state.normalized(
            allowOneHanded = !presentationIsLandscape,
            isLandscape = presentationIsLandscape,
        )
        keyboardLayoutState = normalized
        val presentedConfig = when (normalized.mode) {
            KeyboardLayoutMode.FULL -> config
            KeyboardLayoutMode.ONE_HANDED -> config.scaledForOneHanded(normalized.oneHandedScale)
            KeyboardLayoutMode.FLOATING -> config.scaledForFloating(
                FloatingKeyboardProfile(enabled = true, scale = normalized.floatingScale),
                isLandscape = presentationIsLandscape,
            )
        }
        val theme = KeytaoThemeResolver.resolve(this)
        keyboardHost?.updatePresentation(
            normalized,
            config.floating.marginDp,
            normalKeyboardHeightDp(config),
            androidBottomInsetDp(config).toFloat(),
            presentationIsLandscape,
            theme,
        )
        keyboardView?.updateLayoutPresentation(
            mode = normalized.mode,
            oneHandedSide = normalized.oneHandedSide,
            oneHandedAvailable = !presentationIsLandscape,
        )
        keyboardView?.updateConfig(presentedConfig)
    }

    private fun handleKeyboardLayoutStateChanged(next: KeyboardLayoutState, finished: Boolean) {
        val normalized = next.normalized(
            allowOneHanded = !presentationIsLandscape,
            isLandscape = presentationIsLandscape,
        )
        val presentationChanged = normalized.mode != keyboardLayoutState.mode ||
            kotlin.math.abs(normalized.activeScale - keyboardLayoutState.activeScale) >= 0.001f
        keyboardLayoutState = normalized
        if (presentationChanged) {
            baseKeyboardConfig?.let { applyKeyboardPresentation(it, normalized) }
        } else {
            keyboardView?.updateLayoutPresentation(
                mode = normalized.mode,
                oneHandedSide = normalized.oneHandedSide,
                oneHandedAvailable = !presentationIsLandscape,
            )
        }
        if (finished) {
            keyboardLayoutStateStore.save(presentationIsLandscape, normalized)
        }
    }

    private fun toggleFloatingKeyboard() {
        val config = baseKeyboardConfig ?: KeytaoAndroidImeConfig.load(this)
        val nextMode = if (keyboardLayoutState.mode == KeyboardLayoutMode.FLOATING) {
            KeyboardLayoutMode.FULL
        } else {
            KeyboardLayoutMode.FLOATING
        }
        val next = keyboardLayoutState.copy(mode = nextMode).normalized(
            allowOneHanded = !presentationIsLandscape,
            isLandscape = presentationIsLandscape,
        )
        keyboardLayoutState = next
        keyboardLayoutStateStore.save(presentationIsLandscape, next)
        applyKeyboardPresentation(config, next)
    }

    private fun toggleOneHandedKeyboard() {
        if (presentationIsLandscape) return
        val config = baseKeyboardConfig ?: KeytaoAndroidImeConfig.load(this)
        val nextMode = if (keyboardLayoutState.mode == KeyboardLayoutMode.ONE_HANDED) {
            KeyboardLayoutMode.FULL
        } else {
            KeyboardLayoutMode.ONE_HANDED
        }
        val next = keyboardLayoutState.copy(mode = nextMode).normalized()
        keyboardLayoutState = next
        keyboardLayoutStateStore.save(presentationIsLandscape, next)
        applyKeyboardPresentation(config, next)
    }

    private fun normalKeyboardHeightDp(config: KeytaoAndroidImeConfig): Float {
        return (config.keyboardHeightDp + config.candidateBarHeightDp + androidBottomInsetDp(config)).toFloat()
    }

    /**
     * The system inset is the floor; `keyboardBottomInsetDp` is only the user's
     * extra offset on top of it. Before the first WindowInsets pass we keep the
     * historical constant so the bottom row is never flush with the screen edge.
     */
    private fun androidBottomInsetDp(config: KeytaoAndroidImeConfig): Int {
        val system = if (systemBottomInsetDp >= 0) systemBottomInsetDp else defaultAndroidBottomInsetDp
        return max(system, config.keyboardBottomInsetDp)
    }

    override fun onFinishInput() {
        // finishComposingText() already puts the composing text into the editor,
        // so Rime only needs its own composition discarded — committing here too
        // would duplicate the text.
        currentInputConnection?.finishComposingText()
        composing = false
        currentState = if (inputAvailable) {
            engine.clearComposition().withoutTransientCommit()
        } else {
            KeytaoImeState.empty()
        }
        keyboardView?.updateState(currentState)
        super.onFinishInput()
    }

    override fun onEvaluateFullscreenMode(): Boolean = false

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
        if (isShiftKey(keyCode)) {
            shiftPressedWithoutKey = true
            pendingShiftKeyCode = keyCode
            return super.onKeyDown(keyCode, event)
        }
        if (shiftPressedWithoutKey) {
            shiftPressedWithoutKey = false
            pendingShiftKeyCode = 0
        }
        if (!inputAvailable) {
            return super.onKeyDown(keyCode, event)
        }
        val key = AndroidKeyMapper.fromAndroidKeyEvent(event) ?: return super.onKeyDown(keyCode, event)
        if (!privacyMode.allowsComposing || directInputEditor) {
            return super.onKeyDown(keyCode, event)
        }
        // ascii_mode deliberately plays no part here: English mode still needs
        // librime's ascii_composer to see the key. keytao-core owns the rule.
        if (engine.shouldBypassKey(key.keyCode, key.modifiers)) {
            return super.onKeyDown(keyCode, event)
        }
        val result = engine.processKey(key.keyCode, key.modifiers)
        if (!result.accepted && !result.hasComposition) {
            KeytaoRimeInput.applyRejectedResult(rimeKeySink, result)
            return super.onKeyDown(keyCode, event)
        }
        applyState(result)
        return true
    }

    override fun onKeyUp(keyCode: Int, event: KeyEvent): Boolean {
        if (!isShiftKey(keyCode)) return super.onKeyUp(keyCode, event)
        val shouldToggle = shiftPressedWithoutKey && pendingShiftKeyCode == keyCode
        shiftPressedWithoutKey = false
        pendingShiftKeyCode = 0
        if (!shouldToggle) return super.onKeyUp(keyCode, event)
        if (!inputAvailable) return super.onKeyUp(keyCode, event)

        val shiftKeysym = if (keyCode == KeyEvent.KEYCODE_SHIFT_RIGHT) {
            AndroidKeyMapper.XK_SHIFT_R
        } else {
            AndroidKeyMapper.XK_SHIFT_L
        }
        val result = engine.processKey(shiftKeysym, AndroidKeyMapper.RIME_RELEASE_MASK)
        if (result.accepted || result.hasComposition) {
            applyState(result)
        } else {
            KeytaoRimeInput.applyRejectedResult(rimeKeySink, result)
            applyState(engine.setAsciiMode(!currentState.asciiMode))
        }
        return true
    }

    override fun onKeyCommand(command: KeyCommand) {
        if (!inputAvailable && command.requiresInstalledSchema()) {
            showUnavailableMessage()
            return
        }
        when (command.type) {
            KeyCommandTypes.INPUT -> handleTextInput(command.value.orEmpty(), command.fallbackValue)
            KeyCommandTypes.DIRECT_INPUT -> commitDirect(command.value.orEmpty())
            KeyCommandTypes.RIME_INPUT -> handleRimeInput(command.value.orEmpty(), command.fallbackValue)
            KeyCommandTypes.BACKSPACE -> handleBackspace()
            KeyCommandTypes.BACKSPACE_GESTURE -> handleBackspaceGesture(command.value.orEmpty(), command.fallbackValue)
            KeyCommandTypes.ENTER -> handleEnter()
            KeyCommandTypes.SPACE -> handleSpace()
            KeyCommandTypes.SHIFT -> keyboardView?.toggleShift()
            KeyCommandTypes.MODE -> handleMode(command.value)
            KeyCommandTypes.OPEN_PAGE -> openAppPage(command.value)
            KeyCommandTypes.KEYBOARD_PICKER -> showKeyboardPicker()
            KeyCommandTypes.NEXT_INPUT_METHOD -> switchToNextKeyboard()
            KeyCommandTypes.KEYBOARD_MODE -> keyboardView?.setKeyboardLayer(command.value)
            KeyCommandTypes.NEXT_PAGE -> applyState(engine.changePage(backward = false))
            KeyCommandTypes.PREVIOUS_PAGE -> applyState(engine.changePage(backward = true))
            KeyCommandTypes.RESET -> applyState(engine.reset())
            KeyCommandTypes.RIME_MENU -> openRimeMenu()
            KeyCommandTypes.RIME_SCHEMA -> selectRimeSchema(command.value.orEmpty())
            KeyCommandTypes.RIME_OPTION -> setRimeOption(
                command.value.orEmpty(),
                command.fallbackValue?.toBooleanStrictOrNull(),
            )
            KeyCommandTypes.EDIT -> handleEditAction(command.value.orEmpty(), command.fallbackValue)
            KeyCommandTypes.ONE_HANDED -> toggleOneHandedKeyboard()
            KeyCommandTypes.FLOATING -> toggleFloatingKeyboard()
            KeyCommandTypes.PANEL -> Unit
        }
    }

    override fun onCandidate(index: Int, global: Boolean) {
        if (!inputAvailable) {
            showUnavailableMessage()
            return
        }
        applyState(
            if (global) {
                engine.selectCandidateGlobal(index)
            } else {
                engine.selectCandidate(index)
            }
        )
    }

    override fun onRequestExpandCandidates(callback: (List<KeytaoCandidate>) -> Unit) {
        if (!inputAvailable) {
            showUnavailableMessage()
            callback(emptyList())
            return
        }
        candidateExecutor.execute {
            val candidates = engine.allCandidates(expandedCandidateLimit)
            mainHandler.post {
                callback(candidates)
            }
        }
    }

    override fun onRequestClipboardHistory(callback: (List<String>) -> Unit) {
        if (!privacyMode.allowsClipboard) {
            callback(emptyList())
            return
        }
        rememberCurrentClipboard(suggest = false)
        callback(clipboardHistory.toList())
    }

    override fun onDeleteClipboardEntry(text: String) {
        if (!privacyMode.allowsClipboard) return
        // History is de-duplicated on insert, so text identity is stable while
        // the asynchronously rendered panel index may already be stale.
        clipboardHistory.remove(text)
        currentClipboardSnapshot()?.takeIf { it.text == text }?.let { clipboardSuppression = it }
        keyboardView?.clearRecentClipboardSuggestion()
    }

    override fun onClearClipboardHistory() {
        if (!privacyMode.allowsClipboard) return
        clipboardHistory.clear()
        clipboardSuppression = currentClipboardSnapshot()
        keyboardView?.clearRecentClipboardSuggestion()
    }

    /** Sensitive or digits-only editors never build a composition. */
    private fun bypassesComposition(): Boolean {
        return !inputAvailable || !privacyMode.allowsComposing || directInputEditor
    }

    private fun handleTextInput(text: String, fallbackValue: String? = null) {
        if (text.isEmpty()) return
        // ascii_mode is not a bypass reason: the keyboard already resolved the
        // English variant of the key, librime's ascii_composer still has to see
        // it, and a rejected key falls back to the editor below.
        val fallbackText = fallbackValue ?: text
        if (bypassesComposition()) {
            commitDirect(fallbackText)
            return
        }
        if (text.codePointCount(0, text.length) != 1) {
            commitDirect(fallbackText)
            return
        }

        val key = AndroidKeyMapper.fromText(text)
        if (key == null) {
            commitDirect(fallbackText)
            return
        }

        val result = engine.processKey(key.keyCode, key.modifiers)
        if (!result.accepted && !result.hasComposition) {
            // The key still has to reach the editor, but whatever Rime flushed
            // on its way to rejecting it goes in first.
            KeytaoRimeInput.applyRejectedResult(rimeKeySink, result)
            commitDirect(fallbackText)
        } else {
            applyState(result)
        }
    }

    private fun handleRimeInput(sequence: String, fallbackValue: String?) {
        if (sequence.isEmpty()) return
        val fallbackText = fallbackValue ?: sequence
        // Same rule as handleTextInput: ascii_mode is not a bypass reason. Rime's
        // ascii_composer rejects the code in English mode and the driver below
        // falls back to the literal text.
        if (bypassesComposition()) {
            commitDirect(fallbackText)
            return
        }
        KeytaoRimeInput.feedSequence(rimeKeySink, sequence, fallbackText)
    }

    private val rimeKeySink = object : KeytaoRimeKeySink {
        override val hostComposing: Boolean get() = composing

        override fun processText(text: String): KeytaoImeState? {
            val key = AndroidKeyMapper.fromText(text) ?: return null
            return engine.processKey(key.keyCode, key.modifiers)
        }

        override fun applyState(state: KeytaoImeState) {
            this@KeytaoInputMethodService.applyState(state)
        }

        override fun resetEngine() {
            engine.reset()
        }

        override fun commitDirect(text: String) {
            this@KeytaoInputMethodService.commitDirect(text)
        }
    }

    private fun handleBackspace() {
        if (!currentState.hasComposition && !composing) {
            deleteOneBeforeCursorForRestore()
            selectionModeActive = false
            return
        }
        val result = engine.processKey(AndroidKeyMapper.XK_BACK_SPACE, 0)
        if (result.accepted || result.hasComposition) {
            applyState(result)
        } else {
            // Same order as every other fallback: whatever Rime flushed before
            // declining the key goes in first, and the stale composing region is
            // cleared before the host is asked to delete anything.
            KeytaoRimeInput.applyRejectedResult(rimeKeySink, result)
            deleteOneBeforeCursorForRestore(resetComposition = false)
            composing = false
            currentState = engine.reset().withoutTransientCommit()
            keyboardView?.updateState(currentState)
        }
    }

    private fun handleBackspaceGesture(action: String, countValue: String?) {
        val count = countValue
            ?.toIntOrNull()
            ?.coerceIn(1, maxBackspaceGestureBatchCount)
            ?: 1
        when (action) {
            "delete" -> deleteBeforeCursorForRestore(count)
            "deleteSegment" -> deleteTrailingSegmentBeforeCursorForRestore()
            "restore" -> restoreBackspaceText(count)
            "deleteAll" -> deleteAllBeforeCursorForRestore()
            "restoreAll" -> restoreAllBackspaceText()
        }
    }

    private fun deleteTrailingSegmentBeforeCursorForRestore() {
        val beforeCursor = currentInputConnection
            ?.getTextBeforeCursor(backspaceContextLimit, InputConnection.GET_TEXT_WITH_STYLES)
            ?.toString()
            .orEmpty()
        deleteBeforeCursorForRestore(trailingDeletionSegmentLength(beforeCursor))
    }

    private fun deleteOneBeforeCursorForRestore(resetComposition: Boolean = true): Boolean {
        if (resetComposition) {
            return deleteBeforeCursorForRestore(1)
        }
        return deleteBeforeCursorForRestore(1, resetComposition = false)
    }

    /**
     * `deleteSurroundingText` deletes around the selection and leaves the selected
     * text in place, so a selection has to be removed with an empty commit first.
     */
    private fun deleteSelectionForRestore(connection: InputConnection): Boolean {
        val selected = runCatching { connection.getSelectedText(0) }.getOrNull()
        if (selected.isNullOrEmpty()) return false
        backspaceRestoreStack.clear()
        if (privacyMode.allowsTextRecall) {
            backspaceRestoreStack.addAll(textUnits(selected).asReversed())
        }
        restoreAllOnNextDirectionalRestore = false
        recentCommittedUnits.clear()
        connection.commitText("", 1)
        selectionModeActive = false
        return true
    }

    private fun deleteBeforeCursorForRestore(count: Int, resetComposition: Boolean = true): Boolean {
        val preeditUnits = if (resetComposition) textUnits(currentState.preedit) else emptyList()
        if (resetComposition) clearCompositionBeforeEdit()
        val connection = currentInputConnection ?: return false
        if (deleteSelectionForRestore(connection)) return true
        val unitCount = count.coerceAtLeast(1)
        restoreAllOnNextDirectionalRestore = false
        rememberCommittedUnits(preeditUnits)
        val beforeCursor = connection.getTextBeforeCursor(
            backspaceContextLimit,
            InputConnection.GET_TEXT_WITH_STYLES,
        ) ?: ""
        val deletedUnits = textUnits(beforeCursor).takeLast(unitCount)
        if (deletedUnits.isEmpty()) {
            backspaceRestoreStack.clear()
            deleteSurroundingCodePoints(connection, unitCount)
        } else {
            backspaceRestoreStack.addAll(deletedUnits.asReversed())
            discardRecentCommittedSuffix(deletedUnits)
            connection.deleteSurroundingText(deletedUnits.sumOf(String::length), 0)
        }
        selectionModeActive = false
        return true
    }

    private fun deleteAllBeforeCursorForRestore() {
        val preeditUnits = textUnits(currentState.preedit)
        clearCompositionBeforeEdit()
        val connection = currentInputConnection ?: return
        if (deleteSelectionForRestore(connection)) return
        backspaceRestoreStack.clear()
        rememberCommittedUnits(preeditUnits)
        val beforeCursor = connection.getTextBeforeCursor(
            backspaceContextLimit,
            InputConnection.GET_TEXT_WITH_STYLES,
        ) ?: ""
        val recoverableUnits = textUnits(beforeCursor)
        if (recoverableUnits.isEmpty()) {
            restoreAllOnNextDirectionalRestore = false
            deleteSurroundingCodePoints(connection, backspaceContextLimit)
        } else {
            backspaceRestoreStack.addAll(recoverableUnits.asReversed())
            recentCommittedUnits.clear()
            connection.deleteSurroundingText(beforeCursor.length, 0)
        }
        restoreAllOnNextDirectionalRestore = backspaceRestoreStack.isNotEmpty()
        selectionModeActive = false
    }

    private fun restoreOneBackspaceText(): Boolean {
        return restoreBackspaceText(1)
    }

    private fun restoreBackspaceText(count: Int): Boolean {
        if (restoreAllOnNextDirectionalRestore) {
            return restoreAllBackspaceText()
        }
        val connection = currentInputConnection ?: return false
        if (backspaceRestoreStack.isEmpty()) return false
        val restored = buildString {
            repeat(count.coerceAtLeast(1)) {
                if (backspaceRestoreStack.isEmpty()) return@repeat
                append(backspaceRestoreStack.removeAt(backspaceRestoreStack.lastIndex))
            }
        }
        if (restored.isEmpty()) return false
        connection.commitText(restored, 1)
        rememberCommittedText(restored)
        selectionModeActive = false
        return true
    }

    private fun restoreAllBackspaceText(): Boolean {
        val connection = currentInputConnection ?: return false
        if (backspaceRestoreStack.isEmpty()) return false
        restoreAllOnNextDirectionalRestore = false
        val restored = buildString {
            while (backspaceRestoreStack.isNotEmpty()) {
                append(backspaceRestoreStack.removeAt(backspaceRestoreStack.lastIndex))
            }
        }
        connection.commitText(restored, 1)
        rememberCommittedText(restored)
        selectionModeActive = false
        return true
    }

    private fun deleteSurroundingCodePoints(connection: android.view.inputmethod.InputConnection, count: Int): Boolean {
        return if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.N) {
            connection.deleteSurroundingTextInCodePoints(count.coerceAtLeast(1), 0)
        } else {
            connection.deleteSurroundingText(count.coerceAtLeast(1), 0)
        }
    }

    private fun discardRecentCommittedSuffix(deletedUnits: List<String>) {
        if (deletedUnits.isEmpty() || deletedUnits.size > recentCommittedUnits.size) return
        val fromIndex = recentCommittedUnits.size - deletedUnits.size
        if (recentCommittedUnits.subList(fromIndex, recentCommittedUnits.size) == deletedUnits) {
            recentCommittedUnits.subList(fromIndex, recentCommittedUnits.size).clear()
        }
    }

    private fun rememberCommittedText(text: String) {
        if (text.isEmpty()) return
        rememberCommittedUnits(textUnits(text))
    }

    private fun rememberCommittedUnits(units: List<String>) {
        if (units.isEmpty() || !privacyMode.allowsTextRecall) return
        recentCommittedUnits.addAll(units)
        while (recentCommittedUnits.size > recentCommittedUnitLimit) {
            recentCommittedUnits.removeAt(0)
        }
    }

    private fun textUnits(text: CharSequence): List<String> {
        if (text.isEmpty()) return emptyList()
        val plainText = text.toString()
        val iterator = graphemeIterator
        iterator.setText(plainText)
        val graphemeRanges = mutableListOf<TextUnitRange>()
        var start = iterator.first()
        var end = iterator.next()
        while (end != BreakIterator.DONE) {
            graphemeRanges.add(TextUnitRange(start, end))
            start = end
            end = iterator.next()
        }
        val replacementRanges = if (text is Spanned) {
            text.getSpans(0, text.length, ReplacementSpan::class.java)
                .map { span -> TextUnitRange(text.getSpanStart(span), text.getSpanEnd(span)) }
        } else {
            emptyList()
        }
        return KeytaoEditorPolicy.mergeAtomicTextRanges(graphemeRanges, replacementRanges)
            .map { range -> plainText.substring(range.start, range.endExclusive) }
    }

    private fun handleSpace() {
        if (currentState.hasComposition) {
            applyState(engine.processKey(AndroidKeyMapper.XK_SPACE, 0))
        } else {
            commitDirect(" ")
        }
    }

    private fun handleEnter() {
        val behavior = keyboardView?.currentConfig()?.enterKeyBehavior ?: EnterKeyBehaviors.SYSTEM
        val editorInfo = currentInputEditorInfo
        val decision = KeytaoEditorPolicy.resolveEnterDecision(
            hasComposition = composing || currentState.hasComposition,
            forceNewline = behavior == EnterKeyBehaviors.NEWLINE,
            inputType = editorInfo?.inputType ?: InputType.TYPE_NULL,
            imeOptions = editorInfo?.imeOptions ?: EditorInfo.IME_ACTION_NONE,
            actionId = editorInfo?.actionId ?: EditorInfo.IME_ACTION_UNSPECIFIED,
            hasActionLabel = !editorInfo?.actionLabel.isNullOrEmpty(),
        )
        when (decision.type) {
            // keytao-core owns the Enter contract: Return goes to Rime and core
            // falls back to committing the raw input when Rime declines it.
            EnterDecisionType.CONFIRM_COMPOSITION -> applyState(engine.processEnter())
            EnterDecisionType.INSERT_NEWLINE -> commitLineBreak()
            EnterDecisionType.PERFORM_ACTION -> {
                if (currentInputConnection?.performEditorAction(decision.actionId) != true) {
                    sendEnterKey()
                }
            }
            EnterDecisionType.SEND_ENTER_KEY -> sendEnterKey()
        }
    }

    private fun commitLineBreak() {
        val connection = currentInputConnection ?: return
        backspaceRestoreStack.clear()
        restoreAllOnNextDirectionalRestore = false
        connection.beginBatchEdit()
        connection.commitText("\n", 1)
        rememberCommittedText("\n")
        composing = false
        selectionModeActive = false
        connection.endBatchEdit()
        currentState = if (!engine.nativeReady) {
            currentState.withoutTransientCommit()
        } else {
            engine.reset().withoutTransientCommit()
        }
        keyboardView?.updateState(currentState)
    }

    private fun sendEnterKey() {
        sendKeyStroke(KeyEvent.KEYCODE_ENTER)
    }

    private fun handleMode(value: String?) {
        val target = when (value) {
            "ascii", "english", "en" -> true
            "chinese", "zh", "cn" -> false
            else -> !currentState.asciiMode
        }
        applyState(engine.setAsciiMode(target))
    }

    private fun openRimeMenu() {
        refreshRimeOptions()
    }

    private fun selectRimeSchema(schemaId: String) {
        if (schemaId.isBlank()) return
        val state = engine.selectSchema(schemaId)
        if (state == null) {
            keyboardView?.showMessage("无法切换输入方案")
            refreshRimeOptions()
            return
        }
        applyState(state)
        refreshRimeOptions()
    }

    private fun setRimeOption(optionName: String, enabled: Boolean?) {
        if (optionName.isBlank() || enabled == null) return
        val state = if (optionName == "ascii_mode") {
            engine.setAsciiMode(enabled)
        } else {
            engine.setOption(optionName, enabled)
        }
        if (state == null) {
            keyboardView?.showMessage("无法更新 Rime 选项")
            return
        }
        applyState(state)
        refreshRimeOptions()
    }

    private fun refreshRimeOptions() {
        keyboardView?.updateRimeOptions(
            KeytaoRimeOptionsState(
                schemas = engine.listSchemas(),
                currentSchema = engine.currentSchema(),
                options = rimeOptionNames.associateWith(engine::getOption),
            )
        )
    }

    private fun handleEditAction(action: String, value: String?) {
        when (action) {
            "copy" -> copySelection(cut = false)
            "cut" -> copySelection(cut = true)
            "paste" -> pasteClipboard()
            "tab" -> commitDirect("\t")
            "lineStart" -> moveToLineBoundary(start = true)
            "lineEnd" -> moveToLineBoundary(start = false)
            "cursorLeft" -> moveCursor(KeyEvent.KEYCODE_DPAD_LEFT)
            "cursorRight" -> moveCursor(KeyEvent.KEYCODE_DPAD_RIGHT)
            "cursorUp" -> moveCursor(KeyEvent.KEYCODE_DPAD_UP)
            "cursorDown" -> moveCursor(KeyEvent.KEYCODE_DPAD_DOWN)
            "undo" -> performUndoRedo(redo = false)
            "redo" -> performUndoRedo(redo = true)
            "forwardDelete" -> forwardDelete()
            "clearAll" -> clearAllText()
            "selectAll" -> selectAllText()
            "toggleSelection" -> toggleSelectionMode()
            "selectLeft" -> extendSelection(left = true)
            "selectRight" -> extendSelection(left = false)
            "repeatCommit" -> lastCommittedText?.let { commitDirect(it) }
            "pasteText" -> value?.takeIf { it.isNotEmpty() }?.let {
                keyboardView?.clearRecentClipboardSuggestion()
                commitDirect(it)
            }
        }
    }

    private fun copySelection(cut: Boolean) {
        clearCompositionBeforeEdit()
        val action = if (cut) android.R.id.cut else android.R.id.copy
        performContextAction(action) {
            sendKeyStroke(
                if (cut) KeyEvent.KEYCODE_X else KeyEvent.KEYCODE_C,
                KeyEvent.META_CTRL_ON or KeyEvent.META_CTRL_LEFT_ON,
            )
        }
        if (cut) selectionModeActive = false
    }

    private fun pasteClipboard() {
        clearCompositionBeforeEdit()
        keyboardView?.clearRecentClipboardSuggestion()
        performContextAction(android.R.id.paste) {
            currentClipboardText()?.let { commitDirect(it) }
        }
    }

    private fun selectAllText() {
        clearCompositionBeforeEdit()
        performContextAction(android.R.id.selectAll) {
            sendKeyStroke(KeyEvent.KEYCODE_A, KeyEvent.META_CTRL_ON or KeyEvent.META_CTRL_LEFT_ON)
        }
        selectionModeActive = true
    }

    private fun toggleSelectionMode() {
        clearCompositionBeforeEdit()
        selectionModeActive = !selectionModeActive
    }

    private fun extendSelection(left: Boolean) {
        clearCompositionBeforeEdit()
        sendKeyStroke(
            if (left) KeyEvent.KEYCODE_DPAD_LEFT else KeyEvent.KEYCODE_DPAD_RIGHT,
            KeyEvent.META_SHIFT_ON or KeyEvent.META_SHIFT_LEFT_ON,
        )
        selectionModeActive = true
    }

    private fun moveToLineBoundary(start: Boolean) {
        clearCompositionBeforeEdit()
        sendKeyStroke(if (start) KeyEvent.KEYCODE_MOVE_HOME else KeyEvent.KEYCODE_MOVE_END)
        selectionModeActive = false
    }

    private fun moveCursor(keyCode: Int) {
        clearCompositionBeforeEdit()
        sendKeyStroke(keyCode, 0)
        selectionModeActive = false
    }

    private fun forwardDelete() {
        clearCompositionBeforeEdit()
        sendKeyStroke(KeyEvent.KEYCODE_FORWARD_DEL)
        selectionModeActive = false
    }

    private fun performUndoRedo(redo: Boolean) {
        clearCompositionBeforeEdit()
        performContextAction(if (redo) android.R.id.redo else android.R.id.undo) {
            val metaState = KeyEvent.META_CTRL_ON or KeyEvent.META_CTRL_LEFT_ON or (
                if (redo) KeyEvent.META_SHIFT_ON or KeyEvent.META_SHIFT_LEFT_ON else 0
            )
            sendKeyStroke(KeyEvent.KEYCODE_Z, metaState)
        }
        selectionModeActive = false
    }

    private fun clearAllText() {
        clearCompositionBeforeEdit()
        val connection = currentInputConnection ?: return
        performContextAction(android.R.id.selectAll) {
            sendKeyStroke(KeyEvent.KEYCODE_A, KeyEvent.META_CTRL_ON or KeyEvent.META_CTRL_LEFT_ON)
        }
        val selected = runCatching { connection.getSelectedText(0) }.getOrNull()
        if (!selected.isNullOrEmpty()) {
            sendKeyStroke(KeyEvent.KEYCODE_DEL)
        }
        selectionModeActive = false
    }

    /**
     * FLAG_KEEP_TOUCH_MODE matters: ViewRootImpl consumes the first navigation or
     * Enter key that would take the host out of touch mode, so without it the
     * cursor-move and Enter keys we inject can silently do nothing.
     */
    private fun sendKeyStroke(keyCode: Int, metaState: Int = 0) {
        val connection = currentInputConnection ?: return
        val now = SystemClock.uptimeMillis()
        connection.sendKeyEvent(softKeyEvent(now, KeyEvent.ACTION_DOWN, keyCode, metaState))
        connection.sendKeyEvent(softKeyEvent(now, KeyEvent.ACTION_UP, keyCode, metaState))
    }

    private fun softKeyEvent(time: Long, action: Int, keyCode: Int, metaState: Int): KeyEvent {
        return KeyEvent(
            time,
            time,
            action,
            keyCode,
            0,
            metaState,
            KeyCharacterMap.VIRTUAL_KEYBOARD,
            0,
            KeyEvent.FLAG_SOFT_KEYBOARD or KeyEvent.FLAG_KEEP_TOUCH_MODE,
        )
    }

    private fun performContextAction(action: Int, fallback: () -> Unit = {}) {
        val performed = currentInputConnection?.performContextMenuAction(action) == true
        if (!performed) fallback()
    }

    private fun clearCompositionBeforeEdit() {
        if (!currentState.hasComposition && !composing) return
        currentInputConnection?.finishComposingText()
        composing = false
        currentState = engine.reset().withoutTransientCommit()
        keyboardView?.updateState(currentState)
    }

    /**
     * The clipboard is only watched while an input view is up, and only for as
     * long as the editor allows it — a password field must never see a paste chip.
     */
    private fun registerClipboardListener() {
        if (clipboardListenerRegistered || !privacyMode.allowsClipboard) return
        val manager = clipboardManager ?: return
        manager.addPrimaryClipChangedListener(clipboardListener)
        clipboardListenerRegistered = true
        rememberCurrentClipboard(suggest = false)
    }

    private fun unregisterClipboardListener() {
        if (!clipboardListenerRegistered) return
        clipboardManager?.removePrimaryClipChangedListener(clipboardListener)
        clipboardListenerRegistered = false
    }

    private fun currentClipboardText(): String? {
        return currentClipboardSnapshot()?.text
    }

    private fun currentClipboardSnapshot(): ClipboardSnapshot? {
        if (!privacyMode.allowsClipboard) return null
        val clip = clipboardManager?.primaryClip ?: return null
        if (clip.itemCount <= 0) return null
        if (isSensitiveClip(clip.description)) return null
        val text = clip.getItemAt(0)
            ?.coerceToText(this)
            ?.toString()
            ?.takeIf { it.isNotEmpty() }
            ?: return null
        val timestamp = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            clip.description?.timestamp
        } else {
            null
        }
        return ClipboardSnapshot(text, timestamp)
    }

    /**
     * `ClipDescription.EXTRA_IS_SENSITIVE` is how password managers ask every
     * clipboard consumer, the system preview included, not to show the content.
     */
    private fun isSensitiveClip(description: ClipDescription?): Boolean {
        if (description == null) return false
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return false
        return description.extras?.getBoolean(ClipDescription.EXTRA_IS_SENSITIVE) == true
    }

    private fun setClipboardText(text: String) {
        // Copy/cut through KeyTao is an explicit new clipboard write.
        clipboardSuppression = null
        clipboardManager?.setPrimaryClip(ClipData.newPlainText("KeyTao", text))
        rememberClipboardText(text, suggest = false)
    }

    private fun rememberCurrentClipboard(suggest: Boolean) {
        val snapshot = currentClipboardSnapshot()
        clipboardSuppression?.let { suppressed ->
            val sameClipboardWrite = snapshot?.text == suppressed.text &&
                (suppressed.timestamp == null || snapshot.timestamp == suppressed.timestamp)
            if (sameClipboardWrite) return
            clipboardSuppression = null
        }
        snapshot?.text?.let { rememberClipboardText(it, suggest) }
    }

    private fun rememberClipboardText(text: String, suggest: Boolean) {
        if (text.isBlank() || !privacyMode.allowsClipboard) return
        val wasFirst = clipboardHistory.firstOrNull() == text
        clipboardHistory.remove(text)
        clipboardHistory.add(0, text)
        while (clipboardHistory.size > clipboardHistoryLimit) {
            clipboardHistory.removeAt(clipboardHistory.lastIndex)
        }
        if (suggest && !wasFirst) {
            keyboardView?.showRecentClipboardSuggestion(text)
        }
    }

    /**
     * Push the last known readiness to the keyboard. Probing the data directory
     * and bringing librime up happens in [scheduleAvailabilityRefresh]; the
     * lifecycle callbacks only read this snapshot.
     */
    private fun applyAvailability(): Boolean {
        keyboardView?.updateAvailability(inputAvailable, unavailableMessage)
        return inputAvailable
    }

    private fun scheduleAvailabilityRefresh() {
        if (!::engine.isInitialized || availabilityRefreshPending) return
        availabilityRefreshPending = true
        engine.runInBackground {
            val readiness = engine.refreshReadiness()
            mainHandler.post {
                availabilityRefreshPending = false
                applyReadiness(readiness)
            }
        }
    }

    private fun scheduleReloadIfNeeded() {
        if (!::engine.isInitialized) return
        engine.runInBackground {
            if (!engine.reloadIfNeeded()) return@runInBackground
            val state = engine.state().withoutTransientCommit()
            mainHandler.post {
                if (!composing && !currentState.hasComposition) {
                    currentState = state
                    keyboardView?.updateState(currentState)
                }
            }
        }
    }

    private fun applyReadiness(readiness: Readiness) {
        val message = when (readiness) {
            Readiness.UNWRITABLE -> "无法写入 KeyTao 数据目录，请重新安装 KeyTao"
            Readiness.NOT_INSTALLED -> defaultUnavailableMessage
            Readiness.NOT_DEPLOYED -> "请先在 KeyTao App 部署方案"
            Readiness.NATIVE_UNAVAILABLE -> "RIME 运行库未就绪，请重新安装 KeyTao"
            Readiness.READY -> ""
        }
        inputAvailable = message.isEmpty()
        unavailableMessage = message.ifEmpty { defaultUnavailableMessage }
        keyboardView?.updateAvailability(inputAvailable, unavailableMessage)
    }

    private fun showUnavailableMessage() {
        scheduleAvailabilityRefresh()
        keyboardView?.showMessage(unavailableMessage)
    }

    /**
     * IME windows are not padded for the navigation bar or the gesture handle;
     * the real bottom inset has to come from WindowInsets instead of a constant.
     */
    private fun handleSystemBottomInsetChanged(insetPx: Int) {
        val density = resources.displayMetrics.density
        val insetDp = if (density > 0f) (insetPx / density).roundToInt() else 0
        val clamped = insetDp.coerceIn(0, 96)
        if (clamped == systemBottomInsetDp) return
        systemBottomInsetDp = clamped
        // The callback arrives inside the traversal that is about to measure us,
        // so re-laying out is deferred to the next frame.
        mainHandler.post {
            keyboardView?.updateSystemBottomInsetDp(clamped)
            baseKeyboardConfig?.let { applyKeyboardPresentation(it, keyboardLayoutState) }
        }
    }

    private fun commitDirect(text: String) {
        val connection = currentInputConnection ?: return
        val hadComposition = composing || currentState.hasComposition
        backspaceRestoreStack.clear()
        restoreAllOnNextDirectionalRestore = false
        connection.beginBatchEdit()
        connection.commitText(text, 1)
        rememberCommittedText(text)
        lastCommittedText = text.takeIf { it.isNotEmpty() && privacyMode.allowsTextRecall }
        composing = false
        selectionModeActive = false
        connection.endBatchEdit()
        currentState = if (!engine.nativeReady) {
            currentState.withoutTransientCommit()
        } else if (hadComposition) {
            engine.reset().withoutTransientCommit()
        } else {
            engine.state().withoutTransientCommit()
        }
        keyboardView?.updateState(currentState)
    }

    private fun applyState(state: KeytaoImeState) {
        val connection = currentInputConnection
        if (connection != null) {
            connection.beginBatchEdit()
            // commitText replaces the composing region, so the next composition
            // starts where the committed text ends.
            var regionStart = composingRegionStart
            if (state.committed.isNotEmpty()) {
                backspaceRestoreStack.clear()
                restoreAllOnNextDirectionalRestore = false
                connection.commitText(state.committed, 1)
                rememberCommittedText(state.committed)
                composing = false
                selectionModeActive = false
                if (regionStart >= 0) regionStart += state.committed.length
            }

            if (state.preedit.isNotEmpty()) {
                connection.setComposingText(state.preedit, 1)
                composing = true
                composingRegionStart = regionStart
                applyPreeditCaret(connection, state, regionStart)
            } else if (composing) {
                connection.commitText("", 1)
                composing = false
            }
            connection.endBatchEdit()
        }

        currentState = state.withoutTransientCommit()
        keyboardView?.updateState(currentState)
    }

    /**
     * `setComposingText` can only leave the caret before or after the preedit,
     * so a schema that moved it inside needs an explicit setSelection. Rime
     * counts Unicode scalars and `InputConnection` counts UTF-16 units, hence
     * the conversion through keytao-core.
     */
    private fun applyPreeditCaret(connection: InputConnection, state: KeytaoImeState, regionStart: Int) {
        if (regionStart < 0) return
        val preedit = state.preedit
        val caret = KeytaoEditorPolicy.resolveComposingCaret(
            preeditCharCount = preedit.codePointCount(0, preedit.length),
            cursor = state.cursor,
            selStart = state.selStart,
            selEnd = state.selEnd,
        ) ?: return
        val offset = regionStart + KeytaoNativeBridge.utf16OffsetFromChars(preedit, caret)
        connection.setSelection(offset, offset)
    }

    private fun openAppPage(page: String?) {
        currentInputConnection?.finishComposingText()
        composing = false
        requestHideSelf(0)
        val intent = Intent(this, MainActivity::class.java).apply {
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP)
            putExtra("keytao_page", page ?: "home")
        }
        startActivity(intent)
    }

    private fun showKeyboardPicker() {
        val manager = getSystemService(InputMethodManager::class.java)
        manager?.showInputMethodPicker()
    }

    /**
     * `supportsSwitchingToNextInputMethod="true"` is a promise to the framework
     * that the keyboard offers a way out; falling back to the picker keeps that
     * promise on devices where no next input method is available.
     */
    private fun switchToNextKeyboard() {
        val switched = runCatching {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                switchToNextInputMethod(false)
            } else {
                val token = window?.window?.attributes?.token
                val manager = getSystemService(InputMethodManager::class.java)
                @Suppress("DEPRECATION")
                token != null && manager != null && manager.switchToNextInputMethod(token, false)
            }
        }.getOrDefault(false)
        if (!switched) showKeyboardPicker()
    }

    private fun canOfferNextInputMethod(): Boolean {
        return runCatching {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                shouldOfferSwitchingToNextInputMethod()
            } else {
                val token = window?.window?.attributes?.token
                val manager = getSystemService(InputMethodManager::class.java)
                @Suppress("DEPRECATION")
                token != null && manager != null &&
                    manager.shouldOfferSwitchingToNextInputMethod(token)
            }
        }.getOrDefault(false)
    }

    private fun isShiftKey(keyCode: Int): Boolean {
        return keyCode == KeyEvent.KEYCODE_SHIFT_LEFT || keyCode == KeyEvent.KEYCODE_SHIFT_RIGHT
    }

    companion object {
        private const val defaultUnavailableMessage = "请先在 KeyTao App 安装键道方案"
        private const val preparingMessage = "正在准备 KeyTao 输入法"
        private const val clipboardHistoryLimit = 24
        private const val expandedCandidateLimit = 96

        /** `EditorInfo.MEMORY_EFFICIENT_TEXT_LENGTH`: the budget AOSP recommends
         *  for surrounding-text requests, which cross a binder on every call. */
        private const val backspaceContextLimit = 2048
        private const val maxBackspaceGestureBatchCount = 96
        private const val recentCommittedUnitLimit = 2048
        private const val defaultAndroidBottomInsetDp = 48
        private val rimeOptionNames = listOf(
            "ascii_mode",
            "ascii_punct",
            "full_shape",
            "simplification",
        )
    }

    private fun KeyCommand.requiresInstalledSchema(): Boolean {
        return when (type) {
            KeyCommandTypes.OPEN_PAGE,
            KeyCommandTypes.BACKSPACE,
            KeyCommandTypes.BACKSPACE_GESTURE,
            KeyCommandTypes.KEYBOARD_PICKER,
            KeyCommandTypes.NEXT_INPUT_METHOD,
            KeyCommandTypes.KEYBOARD_MODE,
            KeyCommandTypes.SHIFT,
            KeyCommandTypes.DIRECT_INPUT,
            KeyCommandTypes.INPUT,
            KeyCommandTypes.RIME_INPUT,
            KeyCommandTypes.SPACE,
            KeyCommandTypes.EDIT,
            KeyCommandTypes.ONE_HANDED,
            KeyCommandTypes.FLOATING,
            KeyCommandTypes.PANEL -> false
            else -> true
        }
    }
}
