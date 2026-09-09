package ink.rea.keytao_app

import android.app.ActivityManager
import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Intent
import android.content.res.Configuration
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Color
import android.graphics.Region
import android.graphics.drawable.ColorDrawable
import android.inputmethodservice.InputMethodService
import android.icu.text.BreakIterator
import android.os.Build
import android.os.Debug
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.provider.OpenableColumns
import android.text.InputType
import android.text.Spanned
import android.text.style.ReplacementSpan
import android.util.Log
import android.view.KeyCharacterMap
import android.view.KeyEvent
import android.view.View
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.ExtractedTextRequest
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager
import android.view.ViewGroup
import android.widget.FrameLayout
import android.webkit.MimeTypeMap
import androidx.core.content.FileProvider
import androidx.core.view.inputmethod.EditorInfoCompat
import androidx.core.view.inputmethod.InputConnectionCompat
import androidx.core.view.inputmethod.InputContentInfoCompat
import java.io.File
import java.io.InputStream
import java.util.Locale
import java.util.UUID
import java.util.concurrent.Executors
import kotlin.math.max
import kotlin.math.roundToInt

class KeytaoInputMethodService : InputMethodService(), KeytaoKeyboardView.Listener {
    private var createdAtMs = 0L
    private var startupLatencyLogged = false
    private val inputCounts = KeytaoInputCounts()
    private val applyStateDurations = KeytaoDurationHistogram()
    private lateinit var engine: KeytaoImeEngine
    private val mainHandler = Handler(Looper.getMainLooper())
    private val candidateExecutor = Executors.newSingleThreadExecutor()
    private val clipboardHistory = mutableListOf<String>()
    private val clipboardHistoryTimestamps = mutableMapOf<String, Long>()
    private data class ClipboardMedia(
        val id: String,
        val file: File,
        val mime: String,
        val name: String,
        val size: Long,
        val width: Int,
        val height: Int,
        val ts: Long,
        val thumb: Bitmap?,
    )
    private val clipboardMedia = mutableListOf<ClipboardMedia>()
    private data class ClipboardMediaSnapshot(val uri: String, val timestamp: Long)
    private var lastSeenClipboardMedia: ClipboardMediaSnapshot? = null
    @Volatile private var clipboardMediaGeneration = 0L
    private val clipboardMediaFileLock = Any()
    private val clipboardMediaStreams = mutableMapOf<String, InputStream>()
    private data class ClipboardSnapshot(val text: String, val timestamp: Long)
    private var clipboardSuppression: ClipboardSnapshot? = null
    private var lastOfferedClip: ClipboardSuggestionOffer? = null
    private val clipboardListener = ClipboardManager.OnPrimaryClipChangedListener {
        // A clipboard change is a new user/system event even when it writes the
        // same text again, so a deletion suppression cannot survive it.
        clipboardSuppression = null
        lastSeenClipboardMedia = null
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
    private var englishSchemaId: String? = null
    private var currentRimeSchemaId: String? = null
    private var lastChineseSchemaId: String? = null
    private var chineseSwitchSnapshot: Map<String, Boolean> = emptyMap()

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
    private var backspaceGestureRestoreStart = 0
    private data class BackspaceSelectionSession(
        val anchor: Int,
        val beforeUnits: List<String>,
        var selectedText: String = "",
        var selectionApplied: Boolean = false,
    )
    private var backspaceSelectionSession: BackspaceSelectionSession? = null
    private val recentCommittedUnits = mutableListOf<String>()
    private val doubleSpacePeriodTracker = DoubleSpacePeriodTracker()
    private var lastCommittedText: String? = null
    private var restoreAllOnNextDirectionalRestore = false
    private var privacyMode = InputPrivacyMode()
    private var directInputEditor = false
    private var clipboardListenerRegistered = false
    private var availabilityRefreshPending = false
    private var lastReadiness: Readiness? = null
    private var editorUpdatePending = false
    private var editorUpdateGeneration = 0L
    private var rimeOptionsGeneration = 0L
    private var systemBottomInsetDp = -1
    private val graphemeIterator: BreakIterator by lazy {
        BreakIterator.getCharacterInstance(Locale.ROOT)
    }

    override fun onCreate() {
        createdAtMs = SystemClock.elapsedRealtime()
        super.onCreate()
        KeytaoRuntimeLog.event("lifecycle", "ime_create")
        engine = KeytaoImeEngine(applicationContext)
        clipboardManager = getSystemService(ClipboardManager::class.java)
        wipeClipboardMedia()
        scheduleAvailabilityRefresh()
    }

    override fun onDestroy() {
        editorUpdateGeneration++
        rimeOptionsGeneration++
        unregisterClipboardListener()
        wipeClipboardMedia()
        if (::engine.isInitialized) {
            engine.close()
        }
        candidateExecutor.shutdownNow()
        super.onDestroy()
        KeytaoRuntimeLog.flush(200)
    }

    override fun onCreateInputView(): View {
        val started = System.nanoTime()
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
        KeytaoRuntimeLog.event("lifecycle", "ime_create_input_view", KeytaoRuntimeLog.elapsedMs(started))
        return host
    }

    override fun onComputeInsets(outInsets: Insets) {
        val mode = keyboardLayoutState.mode
        if (mode == KeyboardLayoutMode.FULL) {
            super.onComputeInsets(outInsets)
            return
        }
        if (mode == KeyboardLayoutMode.ONE_HANDED) super.onComputeInsets(outInsets)
        val host = keyboardHost ?: return
        if (!host.populateFloatingTouchableRegion(floatingTouchableRegion)) {
            if (mode == KeyboardLayoutMode.FLOATING) {
                outInsets.contentTopInsets = host.height
                outInsets.visibleTopInsets = host.height
            }
            host.requestInsetsAfterLayout()
            return
        }

        host.getLocationInWindow(keyboardHostLocation)
        floatingTouchableRegion.translate(keyboardHostLocation[0], keyboardHostLocation[1])

        if (mode == KeyboardLayoutMode.FLOATING) {
            val hostBottom = keyboardHostLocation[1] + host.height
            outInsets.contentTopInsets = hostBottom
            outInsets.visibleTopInsets = hostBottom
        }
        outInsets.touchableInsets = Insets.TOUCHABLE_INSETS_REGION
        outInsets.touchableRegion.set(floatingTouchableRegion)
    }

    override fun onStartInput(attribute: EditorInfo?, restarting: Boolean) {
        val started = System.nanoTime()
        super.onStartInput(attribute, restarting)
        backspaceSelectionSession = null
        // doStartInput() skips doFinishInput() when restarting, so the editor can
        // still hold a composing region from the previous round.
        if (restarting) {
            currentInputConnection?.finishComposingText()
        }
        applyEditorInfo(attribute, clearComposition = true)
        currentState = KeytaoImeState.empty(asciiMode = currentState.asciiMode)
        composing = false
        selectionModeActive = false
        backspaceRestoreStack.clear()
        restoreAllOnNextDirectionalRestore = false
        recentCommittedUnits.clear()
        doubleSpacePeriodTracker.reset()
        lastCommittedText = null
        keyboardView?.updateState(currentState)
        scheduleAvailabilityRefresh()
        logStartInput("input", attribute, restarting, started)
    }

    override fun onStartInputView(info: EditorInfo?, restarting: Boolean) {
        val started = System.nanoTime()
        super.onStartInputView(info, restarting)
        keyboardView?.beginRenderSession()
        applyEditorInfo(info, reloadIfNeeded = true)
        registerClipboardListener()
        offerCurrentClipboardSuggestionOnShow()
        keyboardView?.updateTheme(KeytaoThemeResolver.resolve(this))
        applyKeyboardPresentation(KeytaoAndroidImeConfig.load(this))
        keyboardView?.updateInputMethodSwitching(canOfferNextInputMethod())
        applyAvailability()
        keyboardView?.updateState(currentState)
        scheduleAvailabilityRefresh()
        logStartInput("input_view", info, restarting, started)
        logMemorySnapshot("start_input_view")
        if (!startupLatencyLogged) {
            startupLatencyLogged = true
            KeytaoRuntimeLog.event("lifecycle", "startup_latency", (SystemClock.elapsedRealtime() - createdAtMs).toDouble())
        }
    }

    override fun onFinishInputView(finishingInput: Boolean) {
        val started = System.nanoTime()
        unregisterClipboardListener()
        keyboardView?.clearRecentClipboardSuggestion()
        keyboardView?.resetClipboardClearConfirmation()
        super.onFinishInputView(finishingInput)
        flushSessionHistograms()
        logMemorySnapshot("finish_input_view")
        KeytaoRuntimeLog.event("lifecycle", "ime_finish_input", KeytaoRuntimeLog.elapsedMs(started)) {
            put("phase", "input_view")
            put("finishing_input", finishingInput)
        }
    }

    override fun onWindowShown() {
        val started = System.nanoTime()
        super.onWindowShown()
        val mode = keyboardLayoutState.mode.name.lowercase(Locale.ROOT)
        val hostHeight = keyboardHost?.height ?: 0
        val childHeight = keyboardView?.height ?: 0
        val diagnostics = keyboardView?.renderDiagnostics() ?: mapOf(
            "view_w" to 0,
            "view_h" to 0,
            "key_rects" to 0,
            "toolbar_rects" to 0,
            "layer" to "unknown",
            "schema_ready" to false,
            "panel_expanded" to false,
        )
        KeytaoRuntimeLog.event("lifecycle", "window_shown", KeytaoRuntimeLog.elapsedMs(started)) {
            put("mode", mode)
            put("host_h", hostHeight)
            put("child_h", childHeight)
            diagnostics.forEach { (key, value) -> put(key, value) }
        }
    }

    override fun onWindowHidden() {
        val started = System.nanoTime()
        super.onWindowHidden()
        KeytaoRuntimeLog.event("lifecycle", "window_hidden", KeytaoRuntimeLog.elapsedMs(started))
    }

    override fun onTrimMemory(level: Int) {
        super.onTrimMemory(level)
        releaseClipboardMediaThumbnails()
        KeytaoRuntimeLog.event("memory", "mem_trim") { put("level", level) }
        logMemorySnapshot("trim_memory")
    }

    override fun onLowMemory() {
        super.onLowMemory()
        releaseClipboardMediaThumbnails()
        // Snapshots belong only to start/finish input view and onTrimMemory.
        KeytaoRuntimeLog.event("memory", "mem_low")
    }

    private fun logStartInput(phase: String, info: EditorInfo?, restarting: Boolean, started: Long) {
        val pkg = info?.packageName
        val secure = !privacyMode.allowsComposing
        KeytaoRuntimeLog.event("lifecycle", "ime_start_input", KeytaoRuntimeLog.elapsedMs(started)) {
            put("phase", phase)
            pkg?.let { put("pkg", it) }
            put("restarting", restarting)
            put("secure", secure)
        }
    }

    private fun logMemorySnapshot(moment: String) {
        // Defer only scalar metadata when the native logger has not initialized yet.
        KeytaoRuntimeLog.refreshEnabled()
        if (!KeytaoRuntimeLog.collecting) return
        val nativeBytes = Debug.getNativeHeapAllocatedSize()
        val runtime = Runtime.getRuntime()
        val javaBytes = runtime.totalMemory() - runtime.freeMemory()
        val memory = ActivityManager.MemoryInfo()
        val manager = getSystemService(ActivityManager::class.java)
        val available = runCatching { manager?.getMemoryInfo(memory); manager != null }.getOrDefault(false)
        val availableBytes = memory.availMem
        val totalBytes = memory.totalMem
        val lowMemory = memory.lowMemory
        KeytaoRuntimeLog.event("memory", "snapshot") {
            put("moment", moment)
            put("native_heap_mb", nativeBytes / 1_048_576.0)
            put("java_heap_mb", javaBytes / 1_048_576.0)
            if (available) {
                put("available_mb", availableBytes / 1_048_576.0)
                put("total_mb", totalBytes / 1_048_576.0)
                put("low_memory", lowMemory)
            }
        }
    }

    private fun flushSessionHistograms() {
        inputCounts.drain()
        applyStateDurations.drain("input", "apply_state")
        keyboardView?.flushRuntimeHistograms()
        if (::engine.isInitialized) {
            // The histogram has its own short lock; this never takes the engine monitor.
            engine.flushRuntimeHistograms()
        }
    }

    /**
     * Apply what the editor declared about itself: privacy contract first, then
     * the keyboard shape (layer and Enter label) it asked for.
     */
    private fun applyEditorInfo(
        info: EditorInfo?,
        clearComposition: Boolean = false,
        reloadIfNeeded: Boolean = false,
    ) {
        val inputType = info?.inputType ?: InputType.TYPE_NULL
        val imeOptions = info?.imeOptions ?: EditorInfo.IME_ACTION_NONE
        val nextPrivacy = KeytaoEditorPolicy.resolvePrivacyMode(inputType, imeOptions)
        directInputEditor = KeytaoEditorPolicy.isDirectInputEditor(inputType)
        if (nextPrivacy != privacyMode) {
            privacyMode = nextPrivacy
            if (!nextPrivacy.allowsClipboard) {
                clipboardHistory.clear()
                clipboardHistoryTimestamps.clear()
                wipeClipboardMedia()
                clipboardSuppression = null
                keyboardView?.clearRecentClipboardSuggestion()
            }
            if (!nextPrivacy.allowsTextRecall) {
                cancelBackspaceSelection()
                backspaceRestoreStack.clear()
                recentCommittedUnits.clear()
                lastCommittedText = null
                restoreAllOnNextDirectionalRestore = false
            }
        }
        val bypass = !privacyMode.allowsComposing || directInputEditor
        val targetAsciiMode = if (bypass && !bypassAsciiActive) {
            bypassAsciiActive = true
            asciiModeBeforeBypass = currentState.asciiMode
            true
        } else if (!bypass && bypassAsciiActive) {
            bypassAsciiActive = false
            asciiModeBeforeBypass
        } else {
            if (bypass) true else currentState.asciiMode
        }
        currentState = currentState.copy(asciiMode = targetAsciiMode)
        scheduleEditorUpdate(nextPrivacy, targetAsciiMode, clearComposition, reloadIfNeeded)
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
        keyboardView?.updateEmojiHistoryLearningAllowed(privacyMode.allowsLearning)
        keyboardView?.updateTextRecallAllowed(privacyMode.allowsTextRecall)
        Log.d(
            "KeytaoIme",
            "editor pkg=${currentInputEditorInfo?.packageName} " +
                "inputType=0x${inputType.toString(16)} " +
                "imeOptions=0x${imeOptions.toString(16)} " +
                "composing=${privacyMode.allowsComposing} " +
                "direct=$directInputEditor ready=$inputAvailable",
        )
    }

    private fun scheduleEditorUpdate(
        policy: InputPrivacyMode,
        targetAsciiMode: Boolean,
        clearComposition: Boolean,
        reloadIfNeeded: Boolean,
    ) {
        if (!::engine.isInitialized) return
        val generation = ++editorUpdateGeneration
        rimeOptionsGeneration++
        editorUpdatePending = true
        inputAvailable = false
        unavailableMessage = preparingMessage
        applyAvailability()
        // Show callbacks never take the engine monitor, even for no-op setters.
        // Keep input gated until the current editor's privacy policy is installed.
        engine.runInBackground {
            val result = runCatching {
                if (reloadIfNeeded) engine.reloadIfNeeded()
                engine.setInputPolicy(policy.allowsComposing, policy.allowsLearning)
                if (clearComposition && engine.nativeReady) engine.clearComposition()
                engine.setAsciiMode(targetAsciiMode)
                if (engine.nativeReady) engine.state().withoutTransientCommit() else null
            }
            val state = result.getOrNull()
            mainHandler.post {
                if (generation != editorUpdateGeneration) return@post
                if (state?.hasComposition == true && !currentState.hasComposition) {
                    // The editor may move its caret while show work is pending.
                    // Clear the stale engine composition before releasing the gate.
                    scheduleEditorUpdate(policy, targetAsciiMode, clearComposition = true, reloadIfNeeded = false)
                    return@post
                }
                editorUpdatePending = false
                state?.let {
                    currentState = it
                    composing = it.hasComposition
                    keyboardView?.updateState(it)
                }
                lastReadiness?.let(::applyReadiness)
            }
            if (result.isFailure) KeytaoRuntimeLog.event("error", "editor_update_failed")
        }
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
        keyboardView?.updateSettingsConfig(config)
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
        return config.effectiveKeyboardHeightDp + config.candidateBarHeightDp + androidBottomInsetDp(config)
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
        val started = System.nanoTime()
        editorUpdateGeneration++
        rimeOptionsGeneration++
        editorUpdatePending = false
        // finishComposingText() already puts the composing text into the editor,
        // so Rime only needs its own composition discarded — committing here too
        // would duplicate the text.
        cancelBackspaceSelection()
        currentInputConnection?.finishComposingText()
        composing = false
        engine.runInBackground {
            if (engine.nativeReady) engine.clearComposition()
        }
        inputAvailable = false
        currentState = KeytaoImeState.empty(asciiMode = currentState.asciiMode)
        keyboardView?.updateState(currentState)
        super.onFinishInput()
        flushSessionHistograms()
        KeytaoRuntimeLog.event("lifecycle", "ime_finish_input", KeytaoRuntimeLog.elapsedMs(started)) {
            put("phase", "input")
        }
    }

    override fun onEvaluateFullscreenMode(): Boolean = false

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
        inputCounts.record("key_down")
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
        inputCounts.record("key_up")
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
            if (usesEnglishSchema()) {
                handleMode(null)
            } else {
                applyState(engine.setAsciiMode(!currentState.asciiMode))
            }
        }
        return true
    }

    override fun onKeyCommand(command: KeyCommand) {
        inputCounts.recordCommand(command.type)
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
                command.fallbackValue,
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

    override fun onCandidateIsUserPhrase(index: Int): Boolean {
        return inputAvailable && engine.candidateIsUserPhrase(index)
    }

    override fun onDeleteCandidate(index: Int): Boolean {
        if (!inputAvailable) {
            showUnavailableMessage()
            return false
        }
        val (state, deleted) = engine.deleteCandidate(index)
        applyState(state)
        keyboardView?.showMessage(if (deleted) "已删除用户词" else "系统词不可删除")
        return deleted
    }

    override fun onDismissKeyboard() {
        requestHideSelf(0)
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

    override fun onRequestClipboardHistory(callback: (List<ClipboardEntry>) -> Unit) {
        if (!privacyMode.allowsClipboard) {
            callback(emptyList())
            return
        }
        rememberCurrentClipboard(suggest = false)
        callback(clipboardEntries())
    }

    override fun onDeleteClipboardEntry(key: String, media: Boolean) {
        if (!privacyMode.allowsClipboard) return
        if (media) {
            val item = clipboardMedia.firstOrNull { "media:${it.id}" == key } ?: return
            clipboardMedia.remove(item)
            item.file.delete()
            logClipboardMedia()
            return
        }
        // History is de-duplicated on insert, so text identity is stable while
        // the asynchronously rendered panel index may already be stale.
        clipboardHistory.remove(key)
        clipboardHistoryTimestamps.remove(key)
        currentClipboardSnapshot()?.takeIf { it.text == key }?.let { clipboardSuppression = it }
        keyboardView?.clearRecentClipboardSuggestion()
    }

    override fun onClearClipboardHistory() {
        if (!privacyMode.allowsClipboard) return
        val clip = runCatching { clipboardManager?.primaryClip }.getOrNull()
        clipboardHistory.clear()
        clipboardHistoryTimestamps.clear()
        wipeClipboardMedia()
        // Only an explicit clear suppresses re-capturing the current media clip.
        lastSeenClipboardMedia = clip?.takeIf { it.itemCount > 0 }?.let {
            val uri = it.getItemAt(0).uri ?: return@let null
            val timestamp = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) it.description.timestamp else 0L
            ClipboardMediaSnapshot(uri.toString(), timestamp)
        }
        clipboardSuppression = currentClipboardSnapshot()
        keyboardView?.clearRecentClipboardSuggestion()
    }

    override fun onCommitClipboardMedia(key: String) {
        if (!privacyMode.allowsClipboard) return
        val item = clipboardMedia.firstOrNull { "media:${it.id}" == key } ?: return
        val uri = runCatching {
            FileProvider.getUriForFile(this, "$packageName.fileprovider", item.file)
        }.getOrNull() ?: return
        val editor = currentInputEditorInfo
        val connection = currentInputConnection
        if (editor != null && connection != null && Build.VERSION.SDK_INT >= Build.VERSION_CODES.N_MR1 &&
            mimeAccepted(item.mime, EditorInfoCompat.getContentMimeTypes(editor))
        ) {
            val info = InputContentInfoCompat(uri, ClipDescription(item.name, arrayOf(item.mime)), null)
            val committed = runCatching {
                InputConnectionCompat.commitContent(
                    connection, editor, info,
                    InputConnectionCompat.INPUT_CONTENT_GRANT_READ_URI_PERMISSION, null,
                )
            }.getOrDefault(false)
            if (committed) return
        }
        val copied = runCatching {
            val manager = clipboardManager ?: return@runCatching false
            manager.setPrimaryClip(ClipData.newUri(contentResolver, item.name, uri))
            true
        }.getOrDefault(false)
        keyboardView?.showMessage(
            if (copied) "已复制到系统剪贴板，请在输入框长按粘贴" else "复制到系统剪贴板失败",
        )
    }

    override fun onToolbarCustomization(order: List<String>, pinnedCount: Int) {
        if (!KeytaoAndroidImeConfig.persistToolbarCustomization(this, order, pinnedCount)) {
            keyboardView?.showMessage("工具栏顺序保存失败")
        }
    }

    override fun onSettingPreview(key: String, value: String) {
        val next = applySettingToConfig(baseKeyboardConfig ?: KeytaoAndroidImeConfig.load(this), key, value)
            ?: return
        baseKeyboardConfig = next
        applyKeyboardPresentation(next, keyboardLayoutState)
    }

    override fun onSettingChanged(key: String, value: String) {
        if (key == "accentColor" || key == "colorScheme") {
            persistThemeSetting(key, value)
            return
        }
        if (key == "reset") {
            val patch = defaultPanelSettingsPatch()
            var next = baseKeyboardConfig ?: KeytaoAndroidImeConfig.load(this)
            patch.forEach { (settingKey, settingValue) ->
                next = applySettingToConfig(next, settingKey, settingValue.toString()) ?: next
            }
            baseKeyboardConfig = next
            applyKeyboardPresentation(next, keyboardLayoutState)
            val configWritten = KeytaoAndroidImeConfig.persistSettings(this, patch)
            val themeWritten = KeytaoNativeBridge.writeThemeUi(
                KeytaoAndroidPaths.themeFile(this).absolutePath,
                "auto",
                "",
            )
            KeytaoThemeResolver.invalidate()
            keyboardView?.updateTheme(KeytaoThemeResolver.resolve(this))
            if (!configWritten || !themeWritten) keyboardView?.showMessage("键盘内设置恢复失败")
            return
        }
        val patch = settingPatch(key, value) ?: return
        if (!KeytaoAndroidImeConfig.persistSettings(this, mapOf(key to patch))) {
            keyboardView?.showMessage("设置保存失败")
        }
    }

    private fun persistThemeSetting(key: String, value: String) {
        val currentScheme = keyboardView?.currentThemeColorScheme() ?: "auto"
        val colorScheme = if (key == "colorScheme") value else currentScheme
        val accent = if (key == "accentColor") value else null
        if (!KeytaoNativeBridge.writeThemeUi(
                KeytaoAndroidPaths.themeFile(this).absolutePath,
                colorScheme,
                accent,
            )) {
            keyboardView?.showMessage("主题设置保存失败")
            return
        }
        KeytaoThemeResolver.invalidate()
        keyboardView?.updateTheme(KeytaoThemeResolver.resolve(this))
    }

    private fun settingPatch(key: String, value: String): Any? = when (key) {
        "keyboardHeightDp", "candidateBarHeightDp", "haptics.intensity", "keySoundVolume" ->
            value.toFloatOrNull()?.roundToInt()
        "candidateFontScale" -> value.toFloatOrNull()
        "keyHintVisible", "numberRowEnabled", "haptics.enabled", "keySoundEnabled", "keyPreviewEnabled" ->
            value.toBooleanStrictOrNull()
        else -> null
    }

    private fun applySettingToConfig(
        current: KeytaoAndroidImeConfig,
        key: String,
        value: String,
    ): KeytaoAndroidImeConfig? {
        return when (key) {
            "keyboardHeightDp" -> current.copy(keyboardHeightDp = value.toFloatOrNull()?.roundToInt()?.coerceIn(160, 420) ?: return null)
            "candidateBarHeightDp" -> current.copy(candidateBarHeightDp = value.toFloatOrNull()?.roundToInt()?.coerceIn(36, 96) ?: return null)
            "candidateFontScale" -> current.copy(candidateFontScale = value.toFloatOrNull()?.coerceIn(0.8f, 1.4f) ?: return null)
            "keyHintVisible" -> current.copy(keyHintVisible = value.toBooleanStrictOrNull() ?: return null)
            "numberRowEnabled" -> current.copy(numberRowEnabled = value.toBooleanStrictOrNull() ?: return null)
            "haptics.enabled" -> current.copy(hapticsEnabled = value.toBooleanStrictOrNull() ?: return null)
            "haptics.intensity" -> current.copy(hapticIntensity = value.toFloatOrNull()?.roundToInt()?.coerceIn(1, 100) ?: return null)
            "keySoundEnabled" -> current.copy(keySoundEnabled = value.toBooleanStrictOrNull() ?: return null)
            "keySoundVolume" -> current.copy(keySoundVolume = value.toFloatOrNull()?.roundToInt()?.coerceIn(0, 100) ?: return null)
            "keyPreviewEnabled" -> current.copy(keyPreviewEnabled = value.toBooleanStrictOrNull() ?: return null)
            else -> null
        }
    }

    private fun defaultPanelSettingsPatch(): Map<String, Any> = linkedMapOf(
        "candidateFontScale" to 1.0f,
        "keyHintVisible" to true,
        "keyboardHeightDp" to 266,
        "candidateBarHeightDp" to 52,
        "numberRowEnabled" to false,
        "haptics.enabled" to true,
        "haptics.intensity" to 42,
        "keySoundEnabled" to true,
        "keySoundVolume" to 100,
        "keyPreviewEnabled" to true,
    )

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
            "begin" -> backspaceGestureRestoreStart = backspaceRestoreStack.size
            "delete" -> deleteBeforeCursorForRestore(count)
            "deleteSegment" -> deleteTrailingSegmentBeforeCursorForRestore()
            "restore" -> restoreBackspaceText(count)
            "deleteAll" -> {
                deleteAllBeforeCursorForRestore()
                backspaceGestureRestoreStart = 0
            }
            "restoreAll" -> {
                restoreAllBackspaceText()
                backspaceGestureRestoreStart = 0
            }
            "restoreGesture" -> {
                val gestureCount = (backspaceRestoreStack.size - backspaceGestureRestoreStart).coerceAtLeast(0)
                if (gestureCount > 0) restoreBackspaceText(gestureCount)
            }
            "select" -> updateBackspaceSelection(count)
            "cancelSelection" -> cancelBackspaceSelection()
            "commitSelection" -> commitBackspaceSelection()
        }
        val preview = if (privacyMode.allowsTextRecall) {
            val start = backspaceGestureRestoreStart.coerceIn(0, backspaceRestoreStack.size)
            backspaceRestoreStack.subList(start, backspaceRestoreStack.size).asReversed().joinToString("")
        } else {
            ""
        }
        if (action !in setOf("select", "cancelSelection", "commitSelection")) {
            keyboardView?.showBackspaceDeletionPreview(preview)
        }
    }

    private fun updateBackspaceSelection(count: Int) {
        if (!privacyMode.allowsTextRecall) return
        resetCompositionBeforeBackspaceSelection()
        val connection = currentInputConnection ?: return
        val session = backspaceSelectionSession ?: run {
            val extracted = connection.getExtractedText(ExtractedTextRequest(), 0) ?: return
            val anchor = max(extracted.selectionStart, extracted.selectionEnd)
            val before = connection.getTextBeforeCursor(
                backspaceContextLimit,
                InputConnection.GET_TEXT_WITH_STYLES,
            )?.toString().orEmpty()
            BackspaceSelectionSession(anchor = anchor, beforeUnits = textUnits(before)).also {
                backspaceSelectionSession = it
            }
        }
        val selectionLength = trailingDeletionSegmentsLength(
            session.beforeUnits.joinToString(""),
            count.coerceIn(1, maxBackspaceGestureBatchCount),
        )
        val selectedUnits = session.beforeUnits.takeLast(selectionLength)
        val selectedText = selectedUnits.joinToString("")
        val selectionStart = session.anchor - selectedText.length
        session.selectionApplied = selectedText.isNotEmpty() &&
            selectionStart >= 0 && connection.setSelection(selectionStart, session.anchor)
        session.selectedText = selectedText
        selectionModeActive = session.selectionApplied
        keyboardView?.showBackspaceDeletionPreview(selectedText, pendingSelection = true)
    }

    private fun cancelBackspaceSelection() {
        val session = backspaceSelectionSession ?: run {
            keyboardView?.showBackspaceDeletionPreview("")
            return
        }
        if (session.selectionApplied) {
            currentInputConnection?.setSelection(session.anchor, session.anchor)
        }
        backspaceSelectionSession = null
        selectionModeActive = false
        keyboardView?.showBackspaceDeletionPreview("")
    }

    private fun commitBackspaceSelection() {
        val session = backspaceSelectionSession ?: return
        val selectedText = session.selectedText
        backspaceSelectionSession = null
        if (selectedText.isEmpty()) {
            selectionModeActive = false
            keyboardView?.showBackspaceDeletionPreview("")
            return
        }
        val connection = currentInputConnection ?: return
        backspaceRestoreStack.clear()
        backspaceGestureRestoreStart = 0
        if (session.selectionApplied) {
            if (!deleteSelectionForRestore(connection)) {
                connection.setSelection(session.anchor, session.anchor)
                deleteBeforeCursorForRestore(textUnits(selectedText).size)
            }
        } else {
            deleteBeforeCursorForRestore(textUnits(selectedText).size)
        }
        keyboardView?.showBackspaceDeletionPreview(selectedText)
    }

    private fun resetCompositionBeforeBackspaceSelection() {
        if (!currentState.hasComposition && !composing) return
        clearCompositionBeforeEdit()
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
            doubleSpacePeriodTracker.reset()
            applyState(engine.processKey(AndroidKeyMapper.XK_SPACE, 0))
        } else {
            val config = keyboardView?.currentConfig()
            val contextBefore = currentInputConnection
                ?.getTextBeforeCursor(64, 0)
                ?.toString()
                .orEmpty()
            val replace = doubleSpacePeriodTracker.shouldReplaceSpace(
                nowMs = SystemClock.uptimeMillis(),
                contextBefore = contextBefore,
                enabled = config?.doubleSpacePeriodEnabled ?: true,
                hasComposition = false,
            )
            if (replace) {
                deleteOneBeforeCursorForRestore(resetComposition = false)
                commitDirect(if (isEnglishMode()) ". " else "。")
            } else {
                commitDirect(" ")
            }
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
        val hadComposition = composing || currentState.hasComposition
        backspaceRestoreStack.clear()
        restoreAllOnNextDirectionalRestore = false
        connection.beginBatchEdit()
        connection.commitText("\n", 1)
        rememberCommittedText("\n")
        composing = false
        selectionModeActive = false
        connection.endBatchEdit()
        currentState = if (editorUpdatePending || !engine.nativeReady) {
            currentState.withoutTransientCommit()
        } else if (hadComposition) {
            engine.reset().withoutTransientCommit()
        } else {
            engine.state().withoutTransientCommit()
        }
        keyboardView?.updateState(currentState)
    }

    private fun sendEnterKey() {
        sendKeyStroke(KeyEvent.KEYCODE_ENTER)
    }

    private fun handleMode(value: String?) {
        val decision = decideLanguageMode(
            englishMode = configuredEnglishMode(),
            englishSchemaId = englishSchemaId,
            value = value,
            currentSchemaId = currentRimeSchemaId,
            asciiMode = currentState.asciiMode,
        )
        if (!decision.usesEnglishSchema) {
            applyState(engine.setAsciiMode(decision.targetEnglish))
            return
        }

        val englishId = englishSchemaId ?: return
        if ((currentRimeSchemaId == englishId) == decision.targetEnglish) return
        val schemas = engine.listSchemas()
        val targetSchemaId = if (decision.targetEnglish) {
            englishId
        } else {
            lastChineseSchemaId?.takeIf { remembered ->
                schemas.any { it.id == remembered && it.id != englishId }
            } ?: schemas.firstOrNull { it.id != englishId }?.id
        }
        if (targetSchemaId == null) {
            keyboardView?.showMessage("没有可用的中文输入方案")
            refreshRimeOptions()
            return
        }

        if (currentState.hasComposition || composing) {
            applyState(engine.clearComposition())
        }
        if (decision.targetEnglish) {
            engine.currentSchema()?.takeIf { it.id != englishId }?.let { current ->
                lastChineseSchemaId = current.id
                chineseSwitchSnapshot = snapshotChineseSwitchOptions(
                    engine.schemaSwitches().flatMap { it.optionNames } + "ascii_punct",
                    engine::getOption,
                )
            }
        }

        val selectedState = engine.selectSchema(targetSchemaId)
        if (selectedState == null) {
            keyboardView?.showMessage("无法切换输入方案")
            refreshRimeOptions()
            return
        }
        var resultingState: KeytaoImeState = selectedState
        val failedOptions = mutableListOf<String>()
        if (!decision.targetEnglish) {
            for ((optionName, enabled) in chineseSwitchSnapshot) {
                val restoredState = engine.setOption(optionName, enabled)
                if (restoredState == null) {
                    failedOptions += optionName
                    continue
                }
                resultingState = restoredState
            }
        }
        applyState(resultingState)
        if (failedOptions.isNotEmpty()) {
            keyboardView?.showMessage("无法恢复 Rime 选项：${failedOptions.joinToString("、")}")
        }
        refreshRimeOptions()
    }

    private fun openRimeMenu() {
        refreshRimeOptions()
    }

    private fun selectRimeSchema(schemaId: String) {
        if (schemaId.isBlank()) return
        val schemaSwitchingEnabled = usesEnglishSchema()
        if (schemaSwitchingEnabled && schemaId == englishSchemaId) {
            handleMode("ascii")
            return
        }
        if (schemaSwitchingEnabled && currentRimeSchemaId == englishSchemaId) {
            lastChineseSchemaId = schemaId
            handleMode("chinese")
            return
        }
        val state = engine.selectSchema(schemaId)
        if (state == null) {
            keyboardView?.showMessage("无法切换输入方案")
            refreshRimeOptions()
            return
        }
        applyState(state)
        refreshRimeOptions()
    }

    private fun setRimeOption(optionName: String, action: String?) {
        if (optionName.isBlank() || action == null) return
        val enabled = action.toBooleanStrictOrNull()
        val state = if (enabled != null) {
            if (optionName == "ascii_mode" && !usesEnglishSchema()) {
                engine.setAsciiMode(enabled)
            } else {
                engine.setOption(optionName, enabled)
            }
        } else if (action.startsWith(rimeOptionChoicePrefix)) {
            val previous = action.removePrefix(rimeOptionChoicePrefix)
            if (previous.isNotEmpty() && previous != optionName) {
                engine.setOption(previous, false) ?: run {
                    keyboardView?.showMessage("无法更新 Rime 选项")
                    return
                }
            }
            engine.setOption(optionName, true)
        } else {
            return
        }
        if (state == null) {
            keyboardView?.showMessage("无法更新 Rime 选项")
            return
        }
        applyState(state)
        refreshRimeOptions()
    }

    private fun refreshRimeOptions(inBackground: Boolean = false) {
        val generation = ++rimeOptionsGeneration
        val refresh = {
            val schemas = engine.listSchemas()
            val currentSchema = engine.currentSchema()
            val resolvedEnglishSchemaId = resolveEnglishSchemaId(schemas.map { it.id to it.name })
            val switches = engine.schemaSwitches().toMutableList().apply {
                if (none { it.name == "ascii_punct" }) {
                    add(
                        KeytaoRimeSchemaSwitch(
                            name = "ascii_punct",
                            options = emptyList(),
                            states = listOf("中文标点", "英文标点"),
                            reset = null,
                        )
                    )
                }
            }
            val options = KeytaoRimeOptionsState(
                schemas = schemas,
                currentSchema = currentSchema,
                englishSchemaId = resolvedEnglishSchemaId,
                switches = switches,
                options = switches
                    .flatMap { it.optionNames }
                    .distinct()
                    .associateWith(engine::getOption),
            )
            val publish = publish@{
                if (generation != rimeOptionsGeneration) return@publish
                englishSchemaId = resolvedEnglishSchemaId
                currentRimeSchemaId = currentSchema?.id
                if (currentSchema != null && currentSchema.id != englishSchemaId) {
                    lastChineseSchemaId = currentSchema.id
                }
                keyboardView?.updateRimeOptions(
                    options.copy(englishSchemaId = if (usesEnglishSchema()) englishSchemaId else null)
                )
            }
            if (inBackground) mainHandler.post { publish() } else publish()
        }
        // Preserve synchronous mode-switch bookkeeping for existing key commands.
        // Only show/readiness callers use the engine queue and main-thread post.
        if (inBackground) engine.runInBackground { refresh() } else refresh()
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
        inputCounts.record("clipboard_read")
        if (!privacyMode.allowsClipboard) return null
        val clip = clipboardManager?.primaryClip ?: return null
        if (clip.itemCount <= 0) return null
        if (isSensitiveClip(clip.description)) return null
        // Never coerce a media URI into a literal content:// text-history entry.
        clip.getItemAt(0).uri?.let { uri ->
            if (uri.authority == "$packageName.fileprovider") return null
            val mime = runCatching {
                contentResolver.getType(uri) ?: clip.description.getMimeType(0)
            }.getOrNull() ?: return null
            if (!mime.startsWith("text/")) return null
        }
        val text = clip.getItemAt(0)
            ?.coerceToText(this)
            ?.toString()
            ?.takeIf { it.isNotEmpty() }
            ?: return null
        val uri = clip.getItemAt(0).uri
        if (uri?.scheme == "content" && text == uri.toString()) return null
        val timestamp = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            clip.description?.timestamp ?: 0L
        } else {
            0L
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
        inputCounts.record("clipboard_write")
        // Copy/cut through KeyTao is an explicit new clipboard write.
        clipboardSuppression = null
        clipboardManager?.setPrimaryClip(ClipData.newPlainText("KeyTao", text))
        rememberClipboardText(text, suggest = false, timestamp = System.currentTimeMillis())
    }

    private fun rememberCurrentClipboard(suggest: Boolean) {
        if (!privacyMode.allowsClipboard) return
        val clip = runCatching { clipboardManager?.primaryClip }.getOrNull() ?: return
        if (captureClipboardMedia(clip)) return
        val snapshot = currentUnsuppressedClipboardSnapshot() ?: return
        val timestamp = snapshot.timestamp.takeIf { it > 0 }
            ?: if (suggest) System.currentTimeMillis() else 0L
        rememberClipboardText(snapshot.text, suggest, timestamp)
    }

    private fun clipboardDirectory(): File = File(KeytaoAndroidPaths.userRoot(this), "clipboard")

    private fun clipboardEntries(): List<ClipboardEntry> {
        if (!privacyMode.allowsClipboard) return emptyList()
        val text = clipboardHistory.map {
            (clipboardHistoryTimestamps[it] ?: 0L) to ClipboardEntry(key = it, text = it)
        }
        val media = clipboardMedia.map {
            it.ts to ClipboardEntry("media:${it.id}", mime = it.mime, name = it.name,
                size = it.size, width = it.width, height = it.height, thumb = it.thumb)
        }
        return (media + text).sortedByDescending { it.first }.map { it.second }
    }

    /** Open the provider stream during the focused IME's clipboard callback grant window. */
    private fun captureClipboardMedia(clip: ClipData): Boolean {
        if (!privacyMode.allowsClipboard || isSensitiveClip(clip.description)) return true
        if (clip.itemCount == 0) return false
        val uri = clip.getItemAt(0).uri ?: return false
        if (uri.authority == "$packageName.fileprovider") return true
        val mime = runCatching {
            contentResolver.getType(uri) ?: clip.description.getMimeType(0)
        }.getOrNull() ?: return false
        if (mime.startsWith("text/")) return false
        if (uri.scheme != "content") return true
        keyboardView?.clearRecentClipboardSuggestion()
        val timestamp = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) clip.description.timestamp else 0L
        val snapshot = ClipboardMediaSnapshot(uri.toString(), timestamp)
        if (snapshot == lastSeenClipboardMedia) return true
        val reservedBytes = (clipboardMediaStreams.size + 1L) * clipboardMediaMaxBytes
        if (clipboardMediaStreams.size >= clipboardMediaLimit || reservedBytes > clipboardMediaTotalBytes) return true
        val name = runCatching {
            contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
                if (cursor.moveToFirst()) cursor.getString(0) else null
            }
        }.getOrNull()?.take(160)?.takeIf { it.isNotBlank() } ?: "剪贴板文件"
        val stream = runCatching { contentResolver.openInputStream(uri) }.getOrNull() ?: return true
        // Reserve a full item budget for every opened stream, including jobs awaiting main completion.
        // This bounds backing files during capture as well as after insertion.
        evictClipboardMedia(clipboardMedia, clipboardMediaLimit - clipboardMediaStreams.size - 1,
            clipboardMediaTotalBytes - reservedBytes) { it.size }.forEach { it.file.delete() }
        keyboardView?.refreshClipboardItems(clipboardEntries())
        val id = UUID.randomUUID().toString()
        val generation = clipboardMediaGeneration
        val capturedAt = timestamp.takeIf { it > 0 } ?: System.currentTimeMillis()
        val extension = MimeTypeMap.getSingleton().getExtensionFromMimeType(mime)
            ?.takeIf { it.matches(Regex("[a-zA-Z0-9]{1,10}")) } ?: "bin"
        clipboardMediaStreams[id] = stream
        try {
            candidateExecutor.execute {
                val media = snapshotClipboardStream(stream, id, extension, mime, name, capturedAt, generation)
                mainHandler.post {
                    clipboardMediaStreams.remove(id)
                    if (generation != clipboardMediaGeneration || !privacyMode.allowsClipboard) {
                        media?.file?.delete()
                        return@post
                    }
                    if (media != null) {
                        clipboardMedia.add(0, media)
                        clipboardMedia.sortByDescending { it.ts }
                        evictClipboardMedia(clipboardMedia, clipboardMediaLimit, clipboardMediaTotalBytes) { it.size }
                            .forEach { it.file.delete() }
                        keyboardView?.refreshClipboardItems(clipboardEntries())
                    }
                    logClipboardMedia()
                }
            }
            lastSeenClipboardMedia = snapshot
        } catch (_: java.util.concurrent.RejectedExecutionException) {
            clipboardMediaStreams.remove(id)
            runCatching { stream.close() }
            if (lastSeenClipboardMedia == snapshot) lastSeenClipboardMedia = null
        }
        return true
    }

    private fun snapshotClipboardStream(
        stream: InputStream, id: String, extension: String, mime: String, name: String,
        timestamp: Long, generation: Long,
    ): ClipboardMedia? {
        val file = File(clipboardDirectory(), "$id.$extension")
        var retained = false
        try {
            stream.use { input ->
                // Only file creation is locked. A wipe can unlink an in-flight copy immediately.
                val output = synchronized(clipboardMediaFileLock) {
                    if (generation != clipboardMediaGeneration) return null
                    file.parentFile?.mkdirs()
                    file.outputStream()
                }
                var bytes = 0L
                output.use {
                    val buffer = ByteArray(16 * 1024)
                    while (true) {
                        if (generation != clipboardMediaGeneration || Thread.currentThread().isInterrupted) return null
                        val read = input.read(buffer)
                        if (read < 0) break
                        bytes += read
                        if (bytes > clipboardMediaMaxBytes) return null
                        it.write(buffer, 0, read)
                    }
                }
                if (bytes == 0L || generation != clipboardMediaGeneration) return null
                var width = 0
                var height = 0
                var thumb: Bitmap? = null
                if (mime.startsWith("image/")) {
                    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
                    BitmapFactory.decodeFile(file.path, bounds)
                    width = bounds.outWidth.coerceAtLeast(0)
                    height = bounds.outHeight.coerceAtLeast(0)
                    if (width > 0 && height > 0 && width.toLong() * height <= clipboardMaxPixels) {
                        var sample = 1
                        while ((maxOf(width, height) + sample - 1) / sample > clipboardThumbPx) sample *= 2
                        val options = BitmapFactory.Options().apply {
                            inSampleSize = sample
                            inPreferredConfig = Bitmap.Config.RGB_565
                        }
                        thumb = BitmapFactory.decodeFile(file.path, options)?.let { decoded ->
                            if (decoded.config == Bitmap.Config.RGB_565) decoded
                            else decoded.copy(Bitmap.Config.RGB_565, false).also { decoded.recycle() }
                        }
                    }
                }
                if (generation != clipboardMediaGeneration) return null
                retained = true
                return ClipboardMedia(id, file, mime, name, bytes, width, height, timestamp, thumb)
            }
        } catch (_: Exception) {
            return null
        } catch (_: OutOfMemoryError) {
            return null
        } finally {
            if (!retained) file.delete()
        }
    }

    private fun releaseClipboardMediaThumbnails() {
        clipboardMedia.replaceAll { it.copy(thumb = null) }
        keyboardView?.refreshClipboardItems(clipboardEntries())
    }

    private fun wipeClipboardMedia() {
        synchronized(clipboardMediaFileLock) {
            clipboardMediaGeneration++
            clipboardDirectory().deleteRecursively()
        }
        clipboardMediaStreams.values.forEach { runCatching { it.close() } }
        clipboardMediaStreams.clear()
        clipboardMedia.clear()
        lastSeenClipboardMedia = null
        keyboardView?.refreshClipboardItems(clipboardEntries())
        logClipboardMedia()
    }

    private fun logClipboardMedia() {
        val count = clipboardMedia.size
        val bytes = clipboardMedia.sumOf { it.size }
        KeytaoRuntimeLog.event("ui", "clipboard_media") {
            put("count", count)
            put("bytes", bytes)
        }
    }

    private fun offerCurrentClipboardSuggestionOnShow() {
        val snapshot = currentUnsuppressedClipboardSnapshot() ?: return
        rememberClipboardText(snapshot.text, suggest = false, timestamp = snapshot.timestamp)
        if (!shouldOfferClipboardSuggestion(
                text = snapshot.text,
                timestamp = snapshot.timestamp,
                now = System.currentTimeMillis(),
                lastOffered = lastOfferedClip,
                windowMs = KeytaoImeInteractionTuning.CLIPBOARD_SUGGESTION_WINDOW_MS,
            )
        ) {
            return
        }
        lastOfferedClip = ClipboardSuggestionOffer(snapshot.text, snapshot.timestamp)
        keyboardView?.showRecentClipboardSuggestion(snapshot.text)
    }

    private fun currentUnsuppressedClipboardSnapshot(): ClipboardSnapshot? {
        val snapshot = currentClipboardSnapshot()
        clipboardSuppression?.let { suppressed ->
            val sameClipboardWrite = snapshot?.text == suppressed.text &&
                (suppressed.timestamp == 0L || snapshot.timestamp == suppressed.timestamp)
            if (sameClipboardWrite) return null
            clipboardSuppression = null
        }
        return snapshot
    }

    private fun rememberClipboardText(text: String, suggest: Boolean, timestamp: Long = 0L) {
        if (text.isBlank() || !privacyMode.allowsClipboard) return
        val wasFirst = clipboardHistory.firstOrNull() == text
        clipboardHistory.remove(text)
        clipboardHistory.add(0, text)
        clipboardHistoryTimestamps[text] = timestamp.takeIf { it > 0 }
            ?: clipboardHistoryTimestamps[text] ?: System.currentTimeMillis()
        while (clipboardHistory.size > clipboardHistoryLimit) {
            clipboardHistoryTimestamps.remove(clipboardHistory.removeAt(clipboardHistory.lastIndex))
        }
        if (suggest && !wasFirst) {
            lastOfferedClip = ClipboardSuggestionOffer(text, timestamp)
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

    private fun applyReadiness(readiness: Readiness) {
        lastReadiness = readiness
        val message = when (readiness) {
            Readiness.UNWRITABLE -> "无法写入 KeyTao 数据目录，请重新安装 KeyTao"
            Readiness.NOT_INSTALLED -> defaultUnavailableMessage
            Readiness.NOT_DEPLOYED -> "请先在 KeyTao App 部署方案"
            Readiness.NATIVE_UNAVAILABLE -> "RIME 运行库未就绪，请重新安装 KeyTao"
            Readiness.READY -> ""
        }
        inputAvailable = message.isEmpty() && !editorUpdatePending
        unavailableMessage = if (message.isEmpty() && editorUpdatePending) {
            preparingMessage
        } else {
            message.ifEmpty { defaultUnavailableMessage }
        }
        keyboardView?.updateAvailability(inputAvailable, unavailableMessage)
        if (inputAvailable) refreshRimeOptions(inBackground = true)
    }

    private fun configuredEnglishMode(): String {
        return baseKeyboardConfig?.englishMode ?: keyboardView?.currentConfig()?.englishMode ?: "ascii"
    }

    private fun usesEnglishSchema(): Boolean {
        return configuredEnglishMode() == "schema" && englishSchemaId != null
    }

    private fun isEnglishMode(): Boolean {
        return if (usesEnglishSchema()) currentRimeSchemaId == englishSchemaId else currentState.asciiMode
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
        inputCounts.record("commit_direct")
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
        currentState = if (editorUpdatePending || !engine.nativeReady) {
            currentState.withoutTransientCommit()
        } else if (hadComposition) {
            engine.reset().withoutTransientCommit()
        } else {
            engine.state().withoutTransientCommit()
        }
        keyboardView?.updateState(currentState)
    }

    private fun applyState(state: KeytaoImeState) {
        val started = applyStateDurations.start()
        try {
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
        } finally {
            applyStateDurations.finish(started)
        }
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
        private const val clipboardMediaLimit = 12
        private const val clipboardMediaMaxBytes = 8L * 1024 * 1024
        private const val clipboardMediaTotalBytes = 48L * 1024 * 1024
        private const val clipboardThumbPx = 176
        private const val clipboardMaxPixels = 20_000_000L
        private const val expandedCandidateLimit = 96

        /** `EditorInfo.MEMORY_EFFICIENT_TEXT_LENGTH`: the budget AOSP recommends
         *  for surrounding-text requests, which cross a binder on every call. */
        private const val backspaceContextLimit = 2048
        private const val maxBackspaceGestureBatchCount = 96
        private const val recentCommittedUnitLimit = 2048
        private const val defaultAndroidBottomInsetDp = 48
        private const val rimeOptionChoicePrefix = "choice:"
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
