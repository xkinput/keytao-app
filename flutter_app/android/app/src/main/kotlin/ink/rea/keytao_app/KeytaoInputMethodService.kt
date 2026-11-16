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
import android.os.Bundle
import android.os.Debug
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.provider.OpenableColumns
import android.speech.RecognitionListener
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer
import android.text.InputType
import android.text.Spanned
import android.text.style.ReplacementSpan
import android.util.Base64
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
import android.widget.Toast
import android.webkit.MimeTypeMap
import androidx.core.content.FileProvider
import androidx.core.view.inputmethod.EditorInfoCompat
import androidx.core.view.inputmethod.InputConnectionCompat
import androidx.core.view.inputmethod.InputContentInfoCompat
import java.io.File
import java.io.InputStream
import java.security.MessageDigest
import java.util.Locale
import java.util.UUID
import java.util.concurrent.Executors
import org.json.JSONArray
import org.json.JSONObject
import kotlin.math.max
import kotlin.math.roundToInt

class KeytaoInputMethodService : InputMethodService(), KeytaoKeyboardView.Listener {
    private var createdAtMs = 0L
    private var startupLatencyLogged = false
    private val inputCounts = KeytaoInputCounts()
    private val applyStateDurations = KeytaoDurationHistogram()
    private var pendingKeystrokeStartNs = 0L
    private lateinit var engine: KeytaoImeEngine
    private val mainHandler = Handler(Looper.getMainLooper())
    private val voiceInput = KeytaoVoiceInput()
    private var speechRecognizer: SpeechRecognizer? = null
    private var voiceRecognitionAvailable = false
    private val candidateExecutor = Executors.newSingleThreadExecutor()
    private var clipboardHistory = ClipboardHistoryState()
    private val clipboardPreferences by lazy { getSharedPreferences("keytao_ime_clipboard", MODE_PRIVATE) }
    private val clipboardExecutor = Executors.newSingleThreadExecutor()
    private var clipboardHistoryLoaded = false
    private data class ClipboardPersistence(
        val items: List<StoredClipboardItem>,
        val historyLimit: Int,
        val captureSuppressed: Boolean,
        val suppressedFingerprint: String?,
        val lastMediaFingerprint: String?,
    )
    private val clipboardPersistenceLock = Any()
    private var pendingClipboardPersistence: ClipboardPersistence? = null
    private var clipboardPersistenceRunning = false
    private var clipboardCaptureSuppressed = false
    private var clipboardSuppressionFingerprint: String? = null
    private var lastClipboardMediaFingerprint: String? = null
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
        if (!clipboardHistoryLoaded || !clipboardHistoryVisible) return@OnPrimaryClipChangedListener
        clipboardCaptureSuppressed = false
        // A clipboard change is a new user/system event even when it writes the
        // same text again, so a deletion suppression cannot survive it.
        clipboardSuppression = null
        clipboardSuppressionFingerprint = null
        lastSeenClipboardMedia = null
        lastClipboardMediaFingerprint = null
        persistClipboardHistory()
        rememberCurrentClipboard(suggest = true)
    }
    private var clipboardManager: ClipboardManager? = null
    private var keyboardView: KeytaoKeyboardView? = null
    private var keyboardHost: KeytaoKeyboardHost? = null
    private val floatingTouchableRegion = Region()
    private val keyboardHostLocation = IntArray(2)
    private val keyboardLayoutStateStore by lazy { KeytaoKeyboardLayoutStateStore(applicationContext) }
    private var baseKeyboardConfig: KeytaoAndroidImeConfig? = null
    private var clipboardConfigLoaded = false
    private val clipboardHistoryVisible: Boolean
        get() = clipboardConfigLoaded && shouldShowClipboardHistory(
            hideSensitive = baseKeyboardConfig?.clipboardHideSensitive ?: true,
            sensitiveField = !privacyMode.allowsClipboard,
        )
    private val clipboardRecordingAllowed: Boolean
        get() = clipboardConfigLoaded && clipboardCaptureAllowed && shouldRecordClipboard(
            historyEnabled = baseKeyboardConfig?.clipboardHistoryEnabled ?: true,
            hideSensitive = baseKeyboardConfig?.clipboardHideSensitive ?: true,
            sensitiveField = !privacyMode.allowsClipboard,
        )
    private val clipboardCaptureAllowed: Boolean
        get() = !clipboardCaptureSuppressed || baseKeyboardConfig?.clipboardHideSensitive == false
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
    private var selectionModeLocked = false
    private var selectionModeActive = false
        set(value) {
            field = value
            if (!value) selectionModeLocked = false
            keyboardView?.updateSelectionMode(value)
        }
    private var hasKnownCursor = false
    private var knownSelectionEnd = -1
    private var emojiSearchQuery: String? = null
    private var shiftPressedWithoutKey = false
    private var pendingShiftKeyCode = 0
    private var inputAvailable = false
    // Readiness is probed on a background thread, so the first frames of the
    // keyboard must say "still starting up" rather than accuse the user of not
    // having installed a schema.
    private var unavailableMessage = preparingMessage
    private val backspaceRestoreStack = mutableListOf<String>()
    private data class HostBackspaceSnapshot(val selectionStart: Int, val beforeCursor: String)
    private var pendingHostBackspace: HostBackspaceSnapshot? = null
    private var outstandingHostBackspaces = 0
    private var hostBackspaceGeneration = 0L
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
    private var presentationLoadedWithoutRoot = false
    private var inputViewShown = false
    private var navigationResetOnShow = true
    private var configurationChanging = false
    private var panelBackDownConsumed = false
    private var serviceDestroyed = false
    private var readinessRetryCount = 0
    private var readinessRetryDelayMs = 500L
    private var lastReloadStampSignature: String? = null
    private val readinessRetry = Runnable {
        if (inputViewShown && !serviceDestroyed) {
            scheduleAvailabilityRefresh(pollReloadStamp = true)
        }
    }
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
        restoreClipboardHistory()
        scheduleAvailabilityRefresh()
    }

    override fun onDestroy() {
        cancelVoiceInput()
        discardPendingHostBackspace(resetOutstanding = true)
        serviceDestroyed = true
        inputViewShown = false
        mainHandler.removeCallbacks(readinessRetry)
        editorUpdateGeneration++
        rimeOptionsGeneration++
        unregisterClipboardListener()
        cancelClipboardCaptures()
        if (::engine.isInitialized) {
            engine.close()
        }
        candidateExecutor.shutdownNow()
        clipboardExecutor.shutdown()
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
        markPresentationLoad()
        val view = KeytaoKeyboardView(this)
        view.listener = this
        engine.onSchemaLogoChanged = {
            mainHandler.post { keyboardView?.updateSchemaLogo(engine.schemaLogo) }
        }
        view.updateSchemaLogo(engine.schemaLogo)
        host.addView(
            view,
            0,
            FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            ),
        )
        view.updateTheme(resolvePresentationTheme())
        view.updateState(currentState)
        view.updateSelectionMode(selectionModeActive)
        keyboardHost = host
        keyboardView = view
        applyKeyboardPresentation(loadPresentationConfig())
        view.updateSystemBottomInsetDp(systemBottomInsetDp)
        view.updateInputMethodSwitching(canOfferNextInputMethod())
        applyAvailability()
        refreshVoiceInput()
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
        cancelVoiceInput()
        voiceRecognitionAvailable = runCatching { SpeechRecognizer.isRecognitionAvailable(this) }.getOrDefault(false)
        val started = System.nanoTime()
        super.onStartInput(attribute, restarting)
        if (restarting && configurationChanging) {
            applyEditorInfo(attribute, reloadIfNeeded = true)
            logStartInput("input", attribute, restarting, started)
            return
        }
        shiftPressedWithoutKey = false
        pendingShiftKeyCode = 0
        discardPendingHostBackspace(resetOutstanding = true)
        hasKnownCursor = (attribute?.initialSelStart ?: -1) >= 0 &&
            (attribute?.initialSelEnd ?: -1) >= 0
        knownSelectionEnd = attribute?.initialSelEnd ?: -1
        onEmojiSearchRequested(false)
        backspaceSelectionSession = null
        // doStartInput() skips doFinishInput() when restarting, so the editor can
        // still hold a composing region from the previous round.
        if (restarting) {
            currentInputConnection?.finishComposingText()
        }
        // Hardware keyboards can start an editor without showing an input view.
        // They need the same deployment/reload check as the soft keyboard path.
        currentState = KeytaoImeState.empty(asciiMode = currentState.asciiMode)
        composing = false
        applyEditorInfo(attribute, clearComposition = true, reloadIfNeeded = true)
        if (!restarting && !configurationChanging) {
            keyboardView?.resetNavigation()
            navigationResetOnShow = true
        }
        if (!configurationChanging && (!restarting || navigationResetOnShow)) {
            selectionModeActive = false
        }
        backspaceRestoreStack.clear()
        restoreAllOnNextDirectionalRestore = false
        recentCommittedUnits.clear()
        doubleSpacePeriodTracker.reset()
        lastCommittedText = null
        keyboardView?.updateState(currentState)
        refreshVoiceInput()
        scheduleAvailabilityRefresh()
        logStartInput("input", attribute, restarting, started)
    }

    override fun onStartInputView(info: EditorInfo?, restarting: Boolean) {
        val started = System.nanoTime()
        super.onStartInputView(info, restarting)
        inputViewShown = true
        readinessRetryCount = 0
        readinessRetryDelayMs = 500L
        mainHandler.removeCallbacks(readinessRetry)
        keyboardView?.beginRenderSession()
        applyEditorInfo(info, reloadIfNeeded = true)
        val resetNavigationForShow = !configurationChanging && (navigationResetOnShow || !restarting)
        if (resetNavigationForShow) {
            keyboardView?.resetNavigation()
            selectionModeActive = false
            navigationResetOnShow = false
        }
        keyboardView?.updateTheme(resolvePresentationTheme())
        applyKeyboardPresentation(loadPresentationConfig())
        registerClipboardListener()
        rememberCurrentClipboard(suggest = false)
        if (resetNavigationForShow) offerCurrentClipboardSuggestionOnShow()
        keyboardView?.updateInputMethodSwitching(canOfferNextInputMethod())
        applyAvailability()
        keyboardView?.updateState(currentState)
        refreshVoiceInput()
        if (!configurationChanging) scheduleAvailabilityRefresh()
        logStartInput("input_view", info, restarting, started)
        logMemorySnapshot("start_input_view")
        if (!startupLatencyLogged) {
            startupLatencyLogged = true
            KeytaoRuntimeLog.event("lifecycle", "startup_latency", (SystemClock.elapsedRealtime() - createdAtMs).toDouble())
        }
    }

    override fun onFinishInputView(finishingInput: Boolean) {
        val started = System.nanoTime()
        discardPendingHostBackspace(resetOutstanding = true)
        inputViewShown = false
        if (!configurationChanging) onEmojiSearchRequested(false)
        if (!configurationChanging) navigationResetOnShow = true
        cancelVoiceInput()
        mainHandler.removeCallbacks(readinessRetry)
        unregisterClipboardListener()
        if (!configurationChanging) {
            keyboardView?.clearRecentClipboardSuggestion()
            keyboardView?.resetClipboardClearConfirmation()
        }
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
        cancelVoiceInput()
        if (!configurationChanging) navigationResetOnShow = true
        panelBackDownConsumed = false
        val started = System.nanoTime()
        discardPendingHostBackspace(resetOutstanding = true)
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
            if (engine.nativeReady) {
                runCatching { KeytaoNativeBridge.nativeLogDrainKeyStats() }
            }
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
            cancelVoiceInput()
            privacyMode = nextPrivacy
            suppressSensitiveClipboardCapture()
            refreshVoiceInput()
            if (!clipboardHistoryVisible) {
                unregisterClipboardListener()
                cancelClipboardCaptures()
                keyboardView?.clearRecentClipboardSuggestion()
            } else if (inputViewShown) {
                registerClipboardListener()
            }
            keyboardView?.refreshClipboardItems(clipboardEntries())
            if (!nextPrivacy.allowsTextRecall) {
                discardPendingHostBackspace()
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
        if (!configurationChanging) {
            scheduleEditorUpdate(nextPrivacy, targetAsciiMode, clearComposition, reloadIfNeeded)
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
                KeytaoEditorUpdate.run(
                    object : KeytaoEditorEngineUpdate {
                        override val ready: Boolean get() = engine.nativeReady
                        override fun reloadIfNeeded() { engine.reloadIfNeeded() }
                        override fun applyPolicy() { engine.setInputPolicy(policy.allowsComposing, policy.allowsLearning) }
                        override fun clearComposition() { engine.clearComposition() }
                        override fun restoreMode() { engine.setAsciiMode(targetAsciiMode) }
                        override fun state() = engine.state()
                    },
                    reloadIfNeeded,
                    clearComposition,
                )
            }
            val state = result.getOrNull()
            val runtimeReady = engine.nativeReady
            mainHandler.post {
                if (generation != editorUpdateGeneration) return@post
                if (!runtimeReady && lastReadiness == Readiness.READY) {
                    lastReadiness = Readiness.NATIVE_UNAVAILABLE
                }
                if (state?.hasComposition == true && !currentState.hasComposition) {
                    // The editor may move its caret while show work is pending.
                    // Clear the stale engine composition before releasing the gate.
                    scheduleEditorUpdate(policy, targetAsciiMode, clearComposition = true, reloadIfNeeded = false)
                    return@post
                }
                // A failed policy/mode update has not opened this editor for
                // input. A stale READY snapshot must not release its gate.
                // The next input/view lifecycle request retries the update.
                editorUpdatePending = result.isFailure
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
        reconcilePendingHostBackspace(
            oldSelStart, oldSelEnd, newSelStart, newSelEnd,
            hasComposition = composing || currentState.hasComposition ||
                candidatesStart >= 0 || candidatesEnd >= 0,
        )
        // A valid report makes buffer editing available for this input session.
        if (newSelStart >= 0 && newSelEnd >= 0) hasKnownCursor = true
        knownSelectionEnd = newSelEnd
        if (emojiSearchQuery != null) return
        selectionModeActive = selectionModeLocked || newSelStart != newSelEnd
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
        val previousView = keyboardView
        val resetOnShow = navigationResetOnShow
        configurationChanging = true
        try {
            super.onConfigurationChanged(newConfig)
            if (previousView != null && keyboardView !== previousView) {
                keyboardView?.restoreNavigationFrom(previousView)
            }
        } finally {
            configurationChanging = false
            navigationResetOnShow = resetOnShow
        }
        keyboardView?.updateTheme(resolvePresentationTheme())
        applyKeyboardPresentation(loadPresentationConfig())
    }

    private fun markPresentationLoad() {
        // Main thread only. Check before loading: resolving the root is irreversible.
        if (!KeytaoAndroidPaths.isUserRootResolved()) presentationLoadedWithoutRoot = true
    }

    private fun loadPresentationConfig(): KeytaoAndroidImeConfig {
        markPresentationLoad()
        val rootAvailable = KeytaoAndroidPaths.userRootOrNull(this) != null
        val config = KeytaoAndroidImeConfig.load(this)
        clipboardConfigLoaded = rootAvailable
        return config
    }

    private fun resolvePresentationTheme(): KeytaoImeTheme {
        markPresentationLoad()
        return KeytaoThemeResolver.resolve(this)
    }

    private fun applyKeyboardPresentation(config: KeytaoAndroidImeConfig) {
        val isLandscape = resources.configuration.orientation == Configuration.ORIENTATION_LANDSCAPE
        baseKeyboardConfig = config
        suppressSensitiveClipboardCapture()
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
        val theme = resolvePresentationTheme()
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
        if (clipboardConfigLoaded) {
            val previousLimit = clipboardHistory.limit
            clipboardHistory.updateLimit(config.clipboardHistoryLimit)
            if (previousLimit != clipboardHistory.limit) persistClipboardHistory()
        }
        if (!clipboardRecordingAllowed) cancelClipboardCaptures()
        if (!clipboardHistoryVisible || !config.clipboardSuggestionEnabled) {
            keyboardView?.clearRecentClipboardSuggestion()
        }
        if (inputViewShown && clipboardHistoryVisible) registerClipboardListener()
        else unregisterClipboardListener()
        keyboardView?.refreshClipboardItems(clipboardEntries())
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
        val config = baseKeyboardConfig ?: loadPresentationConfig()
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
        val config = baseKeyboardConfig ?: loadPresentationConfig()
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
        val contentHeight = if (emojiSearchQuery != null) config.emojiSearchHeightDp() else config.effectiveKeyboardHeightDp
        return contentHeight + config.candidateBarHeightDp + androidBottomInsetDp(config)
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
        cancelVoiceInput()
        onEmojiSearchRequested(false)
        val started = System.nanoTime()
        discardPendingHostBackspace(resetOutstanding = true)
        editorUpdateGeneration++
        shiftPressedWithoutKey = false
        pendingShiftKeyCode = 0
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
        onUserInteraction()
        discardPendingHostBackspace()
        inputCounts.record("key_down")
        if (keyCode == KeyEvent.KEYCODE_BACK) {
            if (panelBackDownConsumed || keyboardView?.closeOverlayPanel() == true) {
                panelBackDownConsumed = true
                return true
            }
            return super.onKeyDown(keyCode, event)
        }
        if (emojiSearchQuery != null) {
            if (isShiftKey(keyCode)) {
                shiftPressedWithoutKey = AndroidKeyMapper.permitsSoloShift(event)
                pendingShiftKeyCode = if (shiftPressedWithoutKey) keyCode else 0
            } else {
                shiftPressedWithoutKey = false
                pendingShiftKeyCode = 0
                when (keyCode) {
                    KeyEvent.KEYCODE_ESCAPE -> onEmojiSearchRequested(false)
                    KeyEvent.KEYCODE_DEL -> handleBackspace()
                    KeyEvent.KEYCODE_SPACE -> handleSpace()
                    KeyEvent.KEYCODE_ENTER -> handleEnter()
                    else -> if (!event.isCtrlPressed && !event.isMetaPressed && !event.isAltPressed) {
                        val key = AndroidKeyMapper.fromAndroidKeyEvent(event)
                        if (key != null && !bypassesComposition() && !engine.shouldBypassKey(key.keyCode, key.modifiers)) {
                            val result = engine.processKey(key.keyCode, key.modifiers)
                            applyState(result)
                            if (!result.accepted && !result.hasComposition && event.unicodeChar > 0 &&
                                Character.isValidCodePoint(event.unicodeChar)) {
                                commitDirect(String(Character.toChars(event.unicodeChar)))
                            }
                        } else if (event.unicodeChar > 0 && Character.isValidCodePoint(event.unicodeChar)) {
                            commitDirect(String(Character.toChars(event.unicodeChar)))
                        }
                    }
                }
            }
            return true
        }
        if (isShiftKey(keyCode)) {
            shiftPressedWithoutKey = AndroidKeyMapper.permitsSoloShift(event)
            pendingShiftKeyCode = if (shiftPressedWithoutKey) keyCode else 0
            return super.onKeyDown(keyCode, event)
        }
        if (shiftPressedWithoutKey) {
            shiftPressedWithoutKey = false
            pendingShiftKeyCode = 0
        }
        if (!inputAvailable || event.isMetaPressed) {
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
        pendingKeystrokeStartNs = System.nanoTime()
        try {
            val result = if (key.keyCode == AndroidKeyMapper.XK_RETURN &&
                key.modifiers and AndroidKeyMapper.RIME_MOD_LOCK.inv() == 0
            ) {
                engine.processEnter()
            } else {
                engine.processKey(key.keyCode, key.modifiers)
            }
            if (!result.accepted) {
                KeytaoRimeInput.applyRejectedResult(rimeKeySink, result)
                pendingKeystrokeStartNs = 0L
                return super.onKeyDown(keyCode, event)
            }
            applyState(result)
            return true
        } finally {
            pendingKeystrokeStartNs = 0L
        }
    }

    override fun onKeyUp(keyCode: Int, event: KeyEvent): Boolean {
        inputCounts.record("key_up")
        if (keyCode == KeyEvent.KEYCODE_BACK && panelBackDownConsumed) {
            panelBackDownConsumed = false
            return true
        }
        if (emojiSearchQuery != null && !isShiftKey(keyCode)) return true
        if (!isShiftKey(keyCode)) return super.onKeyUp(keyCode, event)
        val shouldToggle = shiftPressedWithoutKey && pendingShiftKeyCode == keyCode &&
            AndroidKeyMapper.permitsSoloShift(event)
        shiftPressedWithoutKey = false
        pendingShiftKeyCode = 0
        if (!shouldToggle) return super.onKeyUp(keyCode, event)
        if (bypassesComposition()) return super.onKeyUp(keyCode, event)

        val shiftKeysym = if (keyCode == KeyEvent.KEYCODE_SHIFT_RIGHT) {
            AndroidKeyMapper.XK_SHIFT_R
        } else {
            AndroidKeyMapper.XK_SHIFT_L
        }
        val beforeAsciiMode = currentState.asciiMode
        val result = KeytaoRimeInput.processSoloShift(shiftKeysym, engine::processKey, ::applyState)
        if (result.accepted || result.hasComposition || result.asciiMode != beforeAsciiMode) {
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
        onUserInteraction()
        if (command.type != KeyCommandTypes.BACKSPACE) discardPendingHostBackspace()
        inputCounts.recordCommand(command.type)
        if (!inputAvailable && command.requiresInstalledSchema()) {
            showUnavailableMessage()
            return
        }
        pendingKeystrokeStartNs = System.nanoTime()
        try {
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
        } finally {
            pendingKeystrokeStartNs = 0L
        }
    }

    override fun onCandidate(index: Int, global: Boolean) {
        onUserInteraction()
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

    override fun onBeforeDirectInsertPage() {
        commitCompositionBeforeDirectInput()
    }

    override fun onEmojiSearchRequested(active: Boolean) {
        if (active == (emojiSearchQuery != null)) return
        if (active) {
            commitCompositionBeforeDirectInput()
            cancelVoiceInput()
            selectionModeActive = false
            doubleSpacePeriodTracker.reset()
            emojiSearchQuery = ""
        } else {
            emojiSearchQuery = null
            currentState = if (!editorUpdatePending && ::engine.isInitialized && engine.nativeReady) {
                engine.clearComposition().withoutTransientCommit()
            } else KeytaoImeState.empty(asciiMode = currentState.asciiMode)
            composing = false
        }
        keyboardView?.updateEmojiSearch(emojiSearchQuery)
        keyboardView?.updateState(currentState)
        baseKeyboardConfig?.let { applyKeyboardPresentation(it, keyboardLayoutState) }
    }

    override fun onEmojiSearchResult(emoji: String) {
        if (emojiSearchQuery == null || emoji.isBlank()) return
        val connection = currentInputConnection ?: return
        if (connection.commitText(emoji, 1)) {
            rememberCommittedText(emoji)
            lastCommittedText = emoji.takeIf { privacyMode.allowsTextRecall }
        }
    }

    override fun onCandidateIsUserPhrase(index: Int): Boolean {
        return inputAvailable && engine.candidateIsUserPhrase(index)
    }

    override fun onDeleteCandidate(index: Int): Boolean {
        onUserInteraction()
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
        onUserInteraction()
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
        if (!clipboardHistoryVisible) {
            callback(emptyList())
            return
        }
        rememberCurrentClipboard(suggest = false)
        callback(clipboardEntries())
    }

    override fun onDeleteClipboardEntry(key: String, media: Boolean) {
        if (!clipboardHistoryVisible) return
        if (media) {
            val item = clipboardMedia.firstOrNull { "media:${it.id}" == key } ?: return
            clipboardMedia.remove(item)
            item.file.delete()
            clipboardHistory.remove(key, media = true)
            persistClipboardHistory()
            logClipboardMedia()
            return
        }
        // History is de-duplicated on insert, so text identity is stable while
        // the asynchronously rendered panel index may already be stale.
        clipboardHistory.remove(key)
        currentClipboardSnapshot()?.takeIf { it.text == key }?.let { clipboardSuppression = it }
        clipboardSuppression?.let { clipboardSuppressionFingerprint = clipboardFingerprint(it.text, it.timestamp) }
        persistClipboardHistory()
        keyboardView?.clearRecentClipboardSuggestion()
    }

    override fun onClearClipboardHistory() {
        if (!clipboardHistoryVisible) return
        val clip = runCatching { clipboardManager?.primaryClip }.getOrNull()
        cancelClipboardCaptures()
        clipboardHistory.clearUnpinned()
        // Only an explicit clear suppresses re-capturing the current media clip.
        lastSeenClipboardMedia = clip?.takeIf { it.itemCount > 0 }?.let {
            val uri = it.getItemAt(0).uri ?: return@let null
            val timestamp = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) it.description.timestamp else 0L
            ClipboardMediaSnapshot(uri.toString(), timestamp)
        }
        clipboardSuppression = currentClipboardSnapshot()
        clipboardSuppressionFingerprint = clipboardSuppression?.let { clipboardFingerprint(it.text, it.timestamp) }
        lastClipboardMediaFingerprint = lastSeenClipboardMedia?.let { clipboardFingerprint(it.uri, it.timestamp) }
        persistClipboardHistory()
        keyboardView?.clearRecentClipboardSuggestion()
    }

    override fun onIsClipboardEntryPinned(key: String, media: Boolean): Boolean =
        clipboardHistory.isPinned(key, media)

    override fun onToggleClipboardPin(key: String, media: Boolean) {
        if (!clipboardHistoryVisible) return
        if (!clipboardHistory.togglePin(key, media, maxBytes = clipboardHistoryItemsMaxBytes)) {
            Toast.makeText(this, "置顶空间已满，请先取消部分置顶", Toast.LENGTH_SHORT).show()
            return
        }
        persistClipboardHistory()
    }

    override fun onCommitClipboardMedia(key: String) {
        discardPendingHostBackspace()
        if (!clipboardHistoryVisible) return
        val item = clipboardMedia.firstOrNull { "media:${it.id}" == key } ?: return
        val uri = runCatching {
            FileProvider.getUriForFile(this, "$packageName.fileprovider", item.file)
        }.getOrNull() ?: return
        val editor = currentInputEditorInfo
        val connection = currentInputConnection
        if (editor != null && connection != null && Build.VERSION.SDK_INT >= Build.VERSION_CODES.N_MR1 &&
            mimeAccepted(item.mime, EditorInfoCompat.getContentMimeTypes(editor))
        ) {
            commitCompositionBeforeDirectInput()
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

    override fun onToolbarCustomization(order: List<String>, pinnedCount: Int, symbolLayer: Boolean) {
        if (!KeytaoAndroidImeConfig.persistToolbarCustomization(this, order, pinnedCount, symbolLayer)) {
            keyboardView?.showMessage("工具栏顺序保存失败")
        }
    }

    override fun onSettingPreview(key: String, value: String) {
        val next = applySettingToConfig(baseKeyboardConfig ?: loadPresentationConfig(), key, value)
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
            val themeFile = KeytaoAndroidPaths.themeFileOrNull(this) ?: run {
                showUnavailableMessage()
                return
            }
            val patch = defaultPanelSettingsPatch()
            var next = baseKeyboardConfig ?: loadPresentationConfig()
            patch.forEach { (settingKey, settingValue) ->
                next = applySettingToConfig(next, settingKey, settingValue.toString()) ?: next
            }
            baseKeyboardConfig = next
            applyKeyboardPresentation(next, keyboardLayoutState)
            val configWritten = KeytaoAndroidImeConfig.persistSettings(this, patch)
            val themeWritten = KeytaoNativeBridge.writeThemeUi(
                themeFile.absolutePath,
                "auto",
                "",
            )
            KeytaoThemeResolver.invalidate()
            keyboardView?.updateTheme(resolvePresentationTheme())
            if (!configWritten || !themeWritten) keyboardView?.showMessage("键盘内设置恢复失败")
            return
        }
        val patch = settingPatch(key, value) ?: return
        if (!KeytaoAndroidImeConfig.persistSettings(this, mapOf(key to patch))) {
            keyboardView?.showMessage("设置保存失败")
        }
    }

    private fun persistThemeSetting(key: String, value: String) {
        val themeFile = KeytaoAndroidPaths.themeFileOrNull(this) ?: run {
            showUnavailableMessage()
            return
        }
        val currentScheme = keyboardView?.currentThemeColorScheme() ?: "auto"
        val colorScheme = if (key == "colorScheme") value else currentScheme
        val accent = if (key == "accentColor") value else null
        if (!KeytaoNativeBridge.writeThemeUi(
                themeFile.absolutePath,
                colorScheme,
                accent,
            )) {
            keyboardView?.showMessage("主题设置保存失败")
            return
        }
        KeytaoThemeResolver.invalidate()
        keyboardView?.updateTheme(resolvePresentationTheme())
    }

    private fun settingPatch(key: String, value: String): Any? = when (key) {
        "keyboardHeightDp", "candidateBarHeightDp", "haptics.intensity", "keySoundVolume" ->
            value.toFloatOrNull()?.roundToInt()
        "candidateFontScale" -> value.toFloatOrNull()
        "keyHintVisible", "mnemonicHintsEnabled", "numberRowEnabled", "haptics.enabled", "keySoundEnabled", "keyPreviewEnabled" ->
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
            "mnemonicHintsEnabled" -> current.copy(mnemonicHintsEnabled = value.toBooleanStrictOrNull() ?: return null)
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
        "mnemonicHintsEnabled" to false,
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

        override fun canProcessText(text: String): Boolean = AndroidKeyMapper.fromText(text) != null

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

    private fun resolveBackspaceDecision(
        hasComposition: Boolean = composing || currentState.hasComposition,
        hasSelection: Boolean = false,
    ): BackspaceDecision = KeytaoEditorPolicy.resolveBackspaceDecision(
        hasComposition = hasComposition,
        inputType = currentInputEditorInfo?.inputType ?: InputType.TYPE_NULL,
        hasKnownCursor = hasKnownCursor,
        hasSelection = hasSelection,
    )

    private fun sendBackspaceKeyEvents(count: Int): Boolean {
        backspaceRestoreStack.clear()
        backspaceGestureRestoreStart = 0
        restoreAllOnNextDirectionalRestore = false
        backspaceSelectionSession = null
        recentCommittedUnits.clear()
        selectionModeActive = false
        keyboardView?.showBackspaceDeletionPreview("")
        if (currentInputConnection == null) return false
        repeat(count.coerceIn(0, maxBackspaceGestureBatchCount)) {
            sendKeyStroke(KeyEvent.KEYCODE_DEL)
        }
        return true
    }

    private fun discardPendingHostBackspace(resetOutstanding: Boolean = false) {
        val hadOutstanding = outstandingHostBackspaces > 0
        if (resetOutstanding) {
            outstandingHostBackspaces = 0
            hostBackspaceGeneration++
        }
        if (pendingHostBackspace == null && !hadOutstanding) return
        pendingHostBackspace = null
        backspaceRestoreStack.clear()
        backspaceGestureRestoreStart = 0
        restoreAllOnNextDirectionalRestore = false
        recentCommittedUnits.clear()
        keyboardView?.showBackspaceDeletionPreview("")
    }

    private fun sendTrackedHostBackspace() {
        if (currentInputConnection == null) return
        outstandingHostBackspaces++
        val generation = ++hostBackspaceGeneration
        mainHandler.postDelayed({
            if (generation == hostBackspaceGeneration) {
                discardPendingHostBackspace(resetOutstanding = true)
            }
        }, hostBackspaceTimeoutMs)
        sendKeyStroke(KeyEvent.KEYCODE_DEL)
    }

    private fun reconcilePendingHostBackspace(
        oldSelStart: Int,
        oldSelEnd: Int,
        newSelStart: Int,
        newSelEnd: Int,
        hasComposition: Boolean,
    ) {
        // Overlapping or otherwise unprovable deletes remain outstanding until
        // timeout. Selection callbacks can be coalesced and cannot safely drain
        // a count one by one or identify which injected key they acknowledge.
        if (outstandingHostBackspaces > 1) {
            discardPendingHostBackspace()
            return
        }
        val snapshot = pendingHostBackspace ?: return
        if (hasComposition || newSelStart != newSelEnd ||
            oldSelStart != snapshot.selectionStart || oldSelEnd != snapshot.selectionStart
        ) {
            discardPendingHostBackspace()
            return
        }
        // An unchanged selection is not an acknowledgement of the delete key.
        if (newSelStart == snapshot.selectionStart) return
        if (!privacyMode.allowsTextRecall) {
            discardPendingHostBackspace()
            return
        }
        val connection = currentInputConnection
        val deleted = KeytaoEditorPolicy.reconcileHostBackspace(
            snapshot.beforeCursor, snapshot.selectionStart, newSelStart,
            connection?.getTextBeforeCursor(hostBackspaceContextLimit, 0)?.toString(),
            connection?.getTextAfterCursor(hostBackspaceContextLimit, 0)?.toString(),
            outstandingHostBackspaces,
        )
        if (deleted == null) {
            discardPendingHostBackspace()
            return
        }
        pendingHostBackspace = null
        outstandingHostBackspaces = 0
        hostBackspaceGeneration++
        backspaceRestoreStack.add(deleted)
        discardRecentCommittedSuffix(textUnits(deleted))
    }

    private fun deleteTrailingBracketTokenWithHost(): Boolean {
        val connection = currentInputConnection ?: return false
        if (selectionModeActive || !connection.getSelectedText(0).isNullOrEmpty()) return false
        val beforeCursor = connection.getTextBeforeCursor(hostBackspaceContextLimit, 0)
            ?.toString() ?: return false
        if (KeytaoEditorPolicy.trailingBracketTokenRange(beforeCursor) == null) return false
        // Read text and caret together: a previous repeat tick may have changed
        // the host between getTextBeforeCursor and this request.
        val extracted = connection.getExtractedText(ExtractedTextRequest().apply {
            hintMaxChars = hostBackspaceContextLimit
            hintMaxLines = 1
        }, 0)
        if (extracted != null && extracted.selectionStart != extracted.selectionEnd) return false
        val snapshotText = extracted?.text?.takeIf {
            extracted.partialStartOffset < 0 && extracted.startOffset >= 0 &&
                extracted.selectionStart in 0..it.length
        }?.subSequence(0, extracted.selectionStart)?.takeLast(hostBackspaceContextLimit)?.toString()
        if (snapshotText != null && KeytaoEditorPolicy.trailingBracketTokenRange(snapshotText) == null) {
            return false
        }
        restoreAllOnNextDirectionalRestore = false
        if (privacyMode.allowsTextRecall && snapshotText != null) {
            pendingHostBackspace = HostBackspaceSnapshot(
                extracted.startOffset + extracted.selectionStart, snapshotText,
            )
        } else {
            backspaceRestoreStack.clear()
            backspaceGestureRestoreStart = 0
            recentCommittedUnits.clear()
        }
        sendTrackedHostBackspace()
        return true
    }

    private fun handleBackspace() {
        emojiSearchQuery?.let { query ->
            if (currentState.hasComposition && !bypassesComposition()) {
                applyState(engine.processKey(AndroidKeyMapper.XK_BACK_SPACE, 0))
            } else {
                val lastUnit = textUnits(query).lastOrNull().orEmpty()
                emojiSearchQuery = query.dropLast(lastUnit.length)
                keyboardView?.updateEmojiSearch(emojiSearchQuery)
            }
            return
        }
        if (outstandingHostBackspaces > 0) {
            discardPendingHostBackspace()
            sendTrackedHostBackspace()
            return
        }
        if (resolveBackspaceDecision() == BackspaceDecision.DELETE_BEFORE_CURSOR &&
            deleteTrailingBracketTokenWithHost()
        ) return
        if (resolveBackspaceDecision() != BackspaceDecision.ENGINE) {
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
        if (emojiSearchQuery != null) {
            if (action in setOf("delete", "deleteSegment", "deleteAll", "commitSelection")) handleBackspace()
            return
        }
        val count = countValue
            ?.toIntOrNull()
            ?.coerceIn(1, maxBackspaceGestureBatchCount)
            ?: 1
        if (resolveBackspaceDecision(hasComposition = false) == BackspaceDecision.SEND_DEL_KEY) {
            // There is no buffer to preview or restore. Selection gestures carry
            // their final count on release; delete-all is only one gesture step.
            sendBackspaceKeyEvents(0)
            if (action in setOf("delete", "deleteSegment", "deleteAll", "commitSelection")) {
                repeat(count) { handleBackspace() }
            }
            return
        }
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
        if (resolveBackspaceDecision() == BackspaceDecision.SEND_DEL_KEY) {
            sendBackspaceKeyEvents(1)
            return
        }
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
        if (resolveBackspaceDecision(hasComposition = false, hasSelection = true) !=
            BackspaceDecision.DELETE_SELECTION
        ) return false
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
        // This helper is also the fallback after Rime declines Backspace. The
        // engine result has already been flushed, so only the host policy applies.
        if (resolveBackspaceDecision(hasComposition = false) == BackspaceDecision.SEND_DEL_KEY) {
            return sendBackspaceKeyEvents(count)
        }
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
        if (resolveBackspaceDecision(hasComposition = false) == BackspaceDecision.SEND_DEL_KEY) {
            sendBackspaceKeyEvents(1)
            return
        }
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
        if (emojiSearchQuery != null) {
            if (currentState.hasComposition && !bypassesComposition()) applyState(engine.processKey(AndroidKeyMapper.XK_SPACE, 0))
            else commitDirect(" ")
            return
        }
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
        if (emojiSearchQuery != null) {
            if (currentState.hasComposition && !bypassesComposition()) applyState(engine.processEnter())
            return
        }
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
        if (emojiSearchQuery != null) {
            if (action == "paste") currentClipboardText()?.let { commitDirect(it) }
            return
        }
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
            "cancelSelection" -> exitSelectionMode()
            "selectWord" -> selectAroundCursor(line = false)
            "selectLine" -> selectAroundCursor(line = true)
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
        if (selectionModeActive) exitSelectionMode()
        else {
            selectionModeLocked = true
            selectionModeActive = true
        }
    }

    private fun exitSelectionMode() {
        if (!selectionModeActive) return
        selectionModeActive = false
        val connection = currentInputConnection ?: return
        val extracted = connection.getExtractedText(ExtractedTextRequest(), 0)
        val cursor = extracted?.takeIf { it.startOffset >= 0 && it.selectionEnd >= 0 }
            ?.let { it.startOffset + it.selectionEnd } ?: knownSelectionEnd
        if (cursor >= 0) connection.setSelection(cursor, cursor)
    }

    private fun selectAroundCursor(line: Boolean) {
        if (!privacyMode.allowsTextRecall) return
        clearCompositionBeforeEdit()
        val connection = currentInputConnection ?: return
        val extracted = connection.getExtractedText(ExtractedTextRequest(), 0)
        val cursor = extracted?.takeIf { it.startOffset >= 0 && it.selectionEnd >= 0 }
            ?.let { it.startOffset + it.selectionEnd } ?: knownSelectionEnd
        if (cursor < 0 || !connection.setSelection(cursor, cursor)) return
        val limit = selectionContextLimit
        val before = connection.getTextBeforeCursor(limit, 0)?.toString() ?: return
        val after = connection.getTextAfterCursor(limit, 0)?.toString() ?: return
        val range = selectionRangeAroundCursor(before, after, line) ?: return
        if (cursor + range.first < 0 || (cursor > before.length && range.first == -before.length) ||
            (after.length == limit && range.second == limit)) {
            keyboardView?.showMessage("文本范围过长，无法完整选择")
            return
        }
        if (connection.setSelection(cursor + range.first, cursor + range.second)) {
            knownSelectionEnd = cursor + range.second
            selectionModeLocked = true
            selectionModeActive = true
        }
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
        moveCursor(if (start) KeyEvent.KEYCODE_MOVE_HOME else KeyEvent.KEYCODE_MOVE_END)
    }

    private fun moveCursor(keyCode: Int) {
        clearCompositionBeforeEdit()
        sendKeyStroke(
            keyCode,
            if (selectionModeActive) KeyEvent.META_SHIFT_ON or KeyEvent.META_SHIFT_LEFT_ON else 0,
        )
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
        if (emojiSearchQuery != null) return
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
        discardPendingHostBackspace()
        if (!currentState.hasComposition && !composing) return
        currentInputConnection?.finishComposingText()
        composing = false
        currentState = engine.reset().withoutTransientCommit()
        keyboardView?.updateState(currentState)
    }

    /** Clipboard access follows the user's sensitive-field preference while the keyboard is visible. */
    private fun registerClipboardListener() {
        if (clipboardListenerRegistered || !clipboardHistoryVisible) return
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
        if (!clipboardHistoryVisible || !clipboardCaptureAllowed) return null
        val clip = runCatching { clipboardManager?.primaryClip }.getOrNull() ?: return null
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
        // Reject oversized text and URI coercion before allocating or retaining a history copy.
        val value = clip.getItemAt(0).text ?: return null
        if (value.isEmpty() || value.length > clipboardTextMaxChars) return null
        val text = value.toString()
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
        if (baseKeyboardConfig?.clipboardHideSensitive == false || description == null) return false
        return description.extras?.getBoolean("android.content.extra.IS_SENSITIVE") == true
    }

    private fun rememberCurrentClipboard(suggest: Boolean) {
        if (!clipboardHistoryLoaded || !clipboardHistoryVisible || !clipboardCaptureAllowed || (!clipboardRecordingAllowed && !suggest)) return
        val clip = runCatching { clipboardManager?.primaryClip }.getOrNull() ?: return
        if (clipboardRecordingAllowed && captureClipboardMedia(clip)) return
        val snapshot = currentUnsuppressedClipboardSnapshot() ?: return
        val timestamp = snapshot.timestamp.takeIf { it > 0L }
            ?: if (suggest) System.currentTimeMillis() else 0L
        rememberClipboardText(snapshot.text, suggest, timestamp)
    }

    private fun clipboardDirectory(): File = File(filesDir, "clipboard-history")

    private fun clipboardEntries(): List<ClipboardEntry> {
        if (!clipboardHistoryVisible) return emptyList()
        return clipboardHistory.items.mapNotNull { stored ->
            if (baseKeyboardConfig?.clipboardHistoryEnabled == false && !stored.pinned) return@mapNotNull null
            stored.text?.let { return@mapNotNull ClipboardEntry(key = stored.key, text = it) }
            val media = clipboardMedia.firstOrNull { "media:${it.id}" == stored.key } ?: return@mapNotNull null
            ClipboardEntry(stored.key, mime = media.mime, name = media.name,
                size = media.size, width = media.width, height = media.height, thumb = media.thumb)
        }
    }

    /** Open the provider stream during the focused IME's clipboard callback grant window. */
    private fun captureClipboardMedia(clip: ClipData): Boolean {
        if (!clipboardRecordingAllowed || isSensitiveClip(clip.description)) return true
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
        val fingerprint = clipboardFingerprint(snapshot.uri, snapshot.timestamp)
        if (snapshot == lastSeenClipboardMedia || fingerprint == lastClipboardMediaFingerprint) return true
        val reservedBytes = (clipboardMediaStreams.size + 1L) * clipboardMediaMaxBytes
        val pinnedBytes = clipboardMedia.filter { clipboardHistory.isPinned("media:${it.id}", media = true) }.sumOf { it.size }
        if (clipboardMediaStreams.size >= clipboardMediaLimit || pinnedBytes + reservedBytes > clipboardMediaTotalBytes) return true
        val name = runCatching {
            contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
                if (cursor.moveToFirst()) cursor.getString(0) else null
            }
        }.getOrNull()?.take(160)?.takeIf { it.isNotBlank() } ?: "剪贴板文件"
        val stream = runCatching { contentResolver.openInputStream(uri) }.getOrNull() ?: return true
        // Reserve a full item budget for every opened stream, including jobs awaiting main completion.
        // This bounds backing files during capture as well as after insertion.
        while (clipboardMedia.sumOf { it.size } + reservedBytes > clipboardMediaTotalBytes) {
            val oldest = clipboardMedia.lastOrNull { !clipboardHistory.isPinned("media:${it.id}", media = true) }
                ?: break
            clipboardMedia.remove(oldest)
            clipboardHistory.remove("media:${oldest.id}", media = true)
            oldest.file.delete()
        }
        persistClipboardHistory()
        keyboardView?.refreshClipboardItems(clipboardEntries())
        val id = UUID.randomUUID().toString()
        val generation = clipboardMediaGeneration
        val capturedAt = timestamp.takeIf { it > 0 } ?: System.currentTimeMillis()
        val extension = MimeTypeMap.getSingleton().getExtensionFromMimeType(mime)
            ?.takeIf { it.matches(Regex("[a-zA-Z0-9]{1,10}")) } ?: "bin"
        clipboardMediaStreams[id] = stream
        try {
            clipboardExecutor.execute {
                val media = snapshotClipboardStream(stream, id, extension, mime, name, capturedAt, generation)
                mainHandler.post {
                    clipboardMediaStreams.remove(id)
                    if (generation != clipboardMediaGeneration || !clipboardRecordingAllowed) {
                        media?.file?.delete()
                        return@post
                    }
                    if (media != null) {
                        clipboardMedia.add(0, media)
                        clipboardMedia.sortByDescending { it.ts }
                        clipboardHistory.remember(StoredClipboardItem(
                            key = "media:${media.id}", timestamp = media.ts, fileName = media.file.name,
                            mime = media.mime, name = media.name, size = media.size, width = media.width, height = media.height,
                        ))
                        lastClipboardMediaFingerprint = fingerprint
                        persistClipboardHistory()
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
        val directory = clipboardDirectory()
        val file = File(directory, "$id.$extension")
        var retained = false
        try {
            stream.use { input ->
                // Generation changes cancel captures without deleting persisted history.
                val plain = ClipboardEncryption.readBounded(input, clipboardMediaMaxBytes.toInt())
                val bytes = plain.size.toLong()
                if (bytes == 0L || generation != clipboardMediaGeneration) return null
                val encrypted = ClipboardEncryption.encrypt(plain)
                val output = synchronized(clipboardMediaFileLock) {
                    if (generation != clipboardMediaGeneration) return null
                    file.parentFile?.mkdirs()
                    file.outputStream()
                }
                output.use { it.write(encrypted) }
                var width = 0
                var height = 0
                var thumb: Bitmap? = null
                if (mime.startsWith("image/")) {
                    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
                    BitmapFactory.decodeByteArray(plain, 0, plain.size, bounds)
                    width = bounds.outWidth.coerceAtLeast(0)
                    height = bounds.outHeight.coerceAtLeast(0)
                    if (width > 0 && height > 0 && width.toLong() * height <= clipboardMaxPixels) {
                        var sample = 1
                        while ((maxOf(width, height) + sample - 1) / sample > clipboardThumbPx) sample *= 2
                        val options = BitmapFactory.Options().apply {
                            inSampleSize = sample
                            inPreferredConfig = Bitmap.Config.RGB_565
                        }
                        thumb = BitmapFactory.decodeByteArray(plain, 0, plain.size, options)?.let { decoded ->
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

    private fun cancelClipboardCaptures() {
        synchronized(clipboardMediaFileLock) {
            clipboardMediaGeneration++
        }
        clipboardMediaStreams.values.forEach { runCatching { it.close() } }
        clipboardMediaStreams.clear()
        lastSeenClipboardMedia = null
    }

    private fun restoreClipboardHistory() {
        clipboardExecutor.execute {
            // Keep up to the supported maximum until the current config is available on the main thread.
            val restored = ClipboardHistoryState(200)
            val media = mutableListOf<ClipboardMedia>()
            val payload = try {
                val prefsFile = File(applicationInfo.dataDir, "shared_prefs/keytao_ime_clipboard.xml")
                if (listOf(prefsFile, File("${prefsFile.path}.bak")).any { it.length() > clipboardPrefsMaxBytes }) {
                    require(deleteSharedPreferences("keytao_ime_clipboard"))
                }
                val encoded = clipboardPreferences.getString("encryptedHistory", null)
                if (encoded == null) null else {
                    require(encoded.length <= clipboardPrefsMaxBytes)
                    val plain = ClipboardEncryption.decrypt(Base64.decode(encoded, Base64.NO_WRAP))
                    require(plain.size <= clipboardHistoryMaxBytes)
                    JSONObject(String(plain, Charsets.UTF_8)).also {
                        restored.restore(it.getJSONArray("history").toString())
                    }
                }
            } catch (_: Exception) {
                null
            }
            if (payload == null) restored.restore("[]")
            restored.items.forEach { stored ->
                if (!stored.isMedia) {
                    if ((stored.text?.length ?: 0) > clipboardTextMaxChars || stored.key.length > clipboardTextMaxChars) {
                        restored.remove(stored.key)
                    }
                    return@forEach
                }
                val file = File(clipboardDirectory(), stored.fileName ?: return@forEach)
                val valid = runCatching {
                    require(stored.size in 1..clipboardMediaMaxBytes)
                    require(file.isFile && file.length() == stored.size + ClipboardEncryption.overheadBytes)
                    require(media.sumOf { it.size } + stored.size <= clipboardMediaTotalBytes)
                    require(stored.mime != null)
                    val plain = file.inputStream().use {
                        ClipboardEncryption.decrypt(ClipboardEncryption.readBounded(
                            it, ClipboardEncryption.mediaMaxBytes + ClipboardEncryption.overheadBytes,
                        ))
                    }
                    require(plain.size.toLong() == stored.size)
                }.isSuccess
                if (!valid) restored.remove(stored.key, media = true)
                else media.add(ClipboardMedia(
                    stored.key.removePrefix("media:"), file, stored.mime ?: return@forEach,
                    stored.name ?: "剪贴板文件", stored.size, stored.width, stored.height, stored.timestamp, null,
                ))
            }
            val retained = media.map { it.file.name }.toSet()
            runCatching {
                clipboardDirectory().listFiles()?.filter { it.isFile && it.name !in retained }?.forEach { it.delete() }
            }
            // Legacy plaintext history is discarded rather than reintroducing unencrypted records.
            if (payload == null) runCatching { deleteSharedPreferences("keytao_ime_clipboard") }
            mainHandler.post {
                if (serviceDestroyed) return@post
                if (clipboardConfigLoaded) baseKeyboardConfig?.let { restored.updateLimit(it.clipboardHistoryLimit) }
                clipboardHistory = restored
                clipboardMedia.addAll(media.sortedByDescending { it.ts })
                clipboardCaptureSuppressed = clipboardCaptureSuppressed || (payload?.optBoolean("captureSuppressed", false) ?: false)
                clipboardSuppressionFingerprint = payload?.opt("suppressedFingerprint") as? String
                lastClipboardMediaFingerprint = payload?.opt("lastMediaFingerprint") as? String
                clipboardHistoryLoaded = true
                persistClipboardHistory()
                if (inputViewShown) {
                    rememberCurrentClipboard(suggest = false)
                    offerCurrentClipboardSuggestionOnShow()
                }
                keyboardView?.refreshClipboardItems(clipboardEntries())
            }
        }
    }

    private fun persistClipboardHistory() {
        if (!clipboardHistoryLoaded || serviceDestroyed) return
        val retained = clipboardHistory.items.filter { it.isMedia }.map { it.key }.toSet()
        val removed = clipboardMedia.filter { "media:${it.id}" !in retained }
        clipboardMedia.removeAll(removed.toSet())
        if (removed.isNotEmpty()) clipboardExecutor.execute { removed.forEach { it.file.delete() } }
        synchronized(clipboardPersistenceLock) {
            pendingClipboardPersistence = ClipboardPersistence(
                clipboardHistory.items, clipboardHistory.limit,
                clipboardCaptureSuppressed,
                clipboardSuppressionFingerprint, lastClipboardMediaFingerprint,
            )
            if (clipboardPersistenceRunning) return
            clipboardPersistenceRunning = true
        }
        clipboardExecutor.execute {
            while (true) {
                val snapshot = synchronized(clipboardPersistenceLock) {
                    val next = pendingClipboardPersistence
                    pendingClipboardPersistence = null
                    if (next == null) clipboardPersistenceRunning = false
                    next
                } ?: break
                try {
                    val history = ClipboardHistoryState(snapshot.historyLimit)
                    snapshot.items.asReversed().forEach { history.remember(it) }
                    val payload = JSONObject().apply {
                        put("history", JSONArray(history.encode(clipboardHistoryItemsMaxBytes)))
                        put("captureSuppressed", snapshot.captureSuppressed)
                        put("suppressedFingerprint", snapshot.suppressedFingerprint)
                        put("lastMediaFingerprint", snapshot.lastMediaFingerprint)
                    }.toString().toByteArray(Charsets.UTF_8)
                    require(payload.size <= clipboardHistoryMaxBytes)
                    val encrypted = Base64.encodeToString(ClipboardEncryption.encrypt(payload), Base64.NO_WRAP)
                    clipboardPreferences.edit().clear().putString("encryptedHistory", encrypted).commit()
                } catch (_: Exception) {
                    runCatching { clipboardPreferences.edit().clear().commit() }
                }
            }
        }
    }

    private fun clipboardFingerprint(value: String, timestamp: Long): String =
        MessageDigest.getInstance("SHA-256").digest("$timestamp\u0000$value".toByteArray(Charsets.UTF_8))
            .joinToString("") { "%02x".format(it.toInt() and 0xff) }

    private fun logClipboardMedia() {
        val count = clipboardMedia.size
        val bytes = clipboardMedia.sumOf { it.size }
        KeytaoRuntimeLog.event("ui", "clipboard_media") {
            put("count", count)
            put("bytes", bytes)
        }
    }

    private fun suppressSensitiveClipboardCapture() {
        if (privacyMode.allowsClipboard || baseKeyboardConfig?.clipboardHideSensitive == false || clipboardCaptureSuppressed) return
        clipboardCaptureSuppressed = true
        persistClipboardHistory()
    }

    private fun offerCurrentClipboardSuggestionOnShow() {
        if (!clipboardHistoryLoaded || !clipboardHistoryVisible) return
        val snapshot = currentUnsuppressedClipboardSnapshot() ?: return
        offerClipboardSuggestion(snapshot)
    }

    private fun offerClipboardSuggestion(snapshot: ClipboardSnapshot) {
        if (!clipboardHistoryVisible || !clipboardCaptureAllowed || baseKeyboardConfig?.clipboardSuggestionEnabled == false) return
        val now = System.currentTimeMillis()
        val timeoutMs = (baseKeyboardConfig?.clipboardSuggestionTimeoutSec ?: 30).coerceIn(5, 120) * 1_000L
        if (!shouldOfferClipboardSuggestion(
                text = snapshot.text,
                timestamp = snapshot.timestamp,
                now = now,
                lastOffered = lastOfferedClip,
                windowMs = timeoutMs,
            )
        ) {
            return
        }
        lastOfferedClip = ClipboardSuggestionOffer(snapshot.text, snapshot.timestamp)
        val remainingMs = if (snapshot.timestamp > 0L) timeoutMs - (now - snapshot.timestamp) else timeoutMs
        keyboardView?.showRecentClipboardSuggestion(snapshot.text, remainingMs)
    }

    private fun currentUnsuppressedClipboardSnapshot(): ClipboardSnapshot? {
        val snapshot = currentClipboardSnapshot()
        if (snapshot != null && clipboardFingerprint(snapshot.text, snapshot.timestamp) == clipboardSuppressionFingerprint) return null
        clipboardSuppression?.let { suppressed ->
            val sameClipboardWrite = snapshot?.text == suppressed.text &&
                (suppressed.timestamp == 0L || snapshot.timestamp == suppressed.timestamp)
            if (sameClipboardWrite) return null
            clipboardSuppression = null
        }
        return snapshot
    }

    private fun rememberClipboardText(text: String, suggest: Boolean, timestamp: Long = 0L) {
        // Reject copies over 20K UTF-16 units instead of keeping or persisting a truncated secret.
        if (!clipboardHistoryLoaded || text.length > clipboardTextMaxChars || text.isBlank() || !clipboardHistoryVisible || !clipboardCaptureAllowed) return
        if (clipboardRecordingAllowed) {
            val previous = clipboardHistory.items.firstOrNull { !it.isMedia && it.key == text }
            clipboardHistory.remember(StoredClipboardItem(
                key = text, text = text,
                timestamp = timestamp.takeIf { it > 0 } ?: previous?.timestamp ?: System.currentTimeMillis(),
            ))
            persistClipboardHistory()
        }
        if (suggest) offerClipboardSuggestion(ClipboardSnapshot(text, timestamp))
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

    private fun scheduleAvailabilityRefresh(pollReloadStamp: Boolean = false) {
        if (serviceDestroyed || !::engine.isInitialized || availabilityRefreshPending) return
        val retryableReadiness = when (lastReadiness) {
            Readiness.STORAGE_NOT_READY, Readiness.NOT_DEPLOYED, Readiness.DEPLOYING, Readiness.NATIVE_UNAVAILABLE -> true
            else -> false
        }
        if (pollReloadStamp && (!inputViewShown || !retryableReadiness)) return
        availabilityRefreshPending = true
        val reloadPresentation = presentationLoadedWithoutRoot
        val retryReadiness = retryableReadiness &&
            (lastReadiness == Readiness.DEPLOYING || readinessRetryCount < 30)
        if (pollReloadStamp && retryReadiness) readinessRetryCount++
        engine.runInBackground {
            val signature = runCatching { engine.reloadStampSignature() }.getOrNull()
            val stampChanged = signature != lastReloadStampSignature
            lastReloadStampSignature = signature
            val readiness = if (!pollReloadStamp || retryReadiness || stampChanged) {
                runCatching { engine.refreshReadiness() }.getOrDefault(Readiness.NATIVE_UNAVAILABLE)
            } else null
            val rootAvailable = readiness != null && readiness != Readiness.STORAGE_NOT_READY &&
                readiness != Readiness.PERMISSION_MISSING
            val recovered = reloadPresentation && rootAvailable
            val config = if (recovered && KeytaoAndroidPaths.isUserRootResolved()) {
                runCatching { KeytaoAndroidImeConfig.load(this) }.getOrNull()
            } else null
            val theme = if (recovered) runCatching { KeytaoThemeResolver.resolve(this) }.getOrNull() else null
            mainHandler.post {
                availabilityRefreshPending = false
                if (serviceDestroyed) return@post
                if (stampChanged) {
                    readinessRetryCount = 0
                    readinessRetryDelayMs = 500L
                }
                config?.let {
                    clipboardConfigLoaded = true
                    applyKeyboardPresentation(it)
                    if (inputViewShown) {
                        rememberCurrentClipboard(suggest = false)
                        offerCurrentClipboardSuggestionOnShow()
                    }
                }
                theme?.let { keyboardView?.updateTheme(it) }
                if (config != null && theme != null) presentationLoadedWithoutRoot = false
                if (readiness != null) applyReadiness(readiness) else scheduleReadinessRetry()
                // The first view may have loaded defaults after this refresh was scheduled.
                if (!reloadPresentation && presentationLoadedWithoutRoot && rootAvailable) {
                    scheduleAvailabilityRefresh()
                }
            }
        }
    }

    private fun applyReadiness(snapshot: Readiness) {
        val readiness = if (snapshot == Readiness.READY && !engine.nativeReady) {
            Readiness.NATIVE_UNAVAILABLE
        } else snapshot
        lastReadiness = readiness
        val message = when (readiness) {
            Readiness.STORAGE_NOT_READY -> preparingMessage
            Readiness.PERMISSION_MISSING -> "请在 KeyTao App 中开启文件访问权限"
            Readiness.UNWRITABLE -> "无法写入 KeyTao 数据目录，请重新安装 KeyTao"
            Readiness.NOT_INSTALLED -> defaultUnavailableMessage
            Readiness.NOT_DEPLOYED -> "请先在 KeyTao App 部署方案"
            Readiness.DEPLOYING -> "正在部署方案…"
            Readiness.NATIVE_UNAVAILABLE -> "RIME 运行库未就绪，请重新安装 KeyTao"
            Readiness.READY -> ""
        }
        inputAvailable = message.isEmpty() && !editorUpdatePending && engine.nativeReady
        unavailableMessage = message.ifEmpty { preparingMessage }
        keyboardView?.updateAvailability(inputAvailable, unavailableMessage)
        if (inputAvailable) refreshRimeOptions(inBackground = true)
        scheduleReadinessRetry()
    }

    private fun scheduleReadinessRetry() {
        mainHandler.removeCallbacks(readinessRetry)
        val retryableReadiness = when (lastReadiness) {
            Readiness.STORAGE_NOT_READY, Readiness.NOT_DEPLOYED, Readiness.DEPLOYING, Readiness.NATIVE_UNAVAILABLE -> true
            else -> false
        }
        if (inputViewShown && !serviceDestroyed && retryableReadiness) {
            // Keep polling the stamp after the full readiness probe budget is exhausted.
            mainHandler.postDelayed(readinessRetry, readinessRetryDelayMs)
            readinessRetryDelayMs = (readinessRetryDelayMs * 2).coerceAtMost(5_000L)
        }
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

    override fun onUserInteraction() {
        if (voiceInput.state != KeytaoVoiceInput.State.IDLE) cancelVoiceInput()
    }

    private fun refreshVoiceInput() {
        keyboardView?.updateVoiceInput(
            available = voiceRecognitionAvailable && inputViewShown && !serviceDestroyed,
            privacyAllowed = !directInputEditor && privacyMode.allowsComposing && privacyMode.allowsLearning,
            permanentlyDenied = KeytaoVoicePermissionActivity.isPermanentlyDenied(this),
            voiceState = voiceInput.state,
        )
    }

    private fun cancelVoiceInput() {
        voiceInput.cancel()
        val recognizer = speechRecognizer
        speechRecognizer = null
        runCatching { recognizer?.cancel() }
        runCatching { recognizer?.destroy() }
        refreshVoiceInput()
    }

    override fun onVoiceInputRequested(): Boolean {
        if (voiceInput.state != KeytaoVoiceInput.State.IDLE || !inputViewShown || serviceDestroyed || currentInputConnection == null) return false
        if (!KeytaoVoiceInput.isOffered(
                recognitionAvailable = voiceRecognitionAvailable,
                privacyAllowed = !directInputEditor && privacyMode.allowsComposing && privacyMode.allowsLearning,
                hasComposition = composing || currentState.hasComposition,
                hasConfiguredAction = false,
                permanentlyDenied = KeytaoVoicePermissionActivity.isPermanentlyDenied(this),
            )
        ) {
            refreshVoiceInput()
            return false
        }
        if (!KeytaoVoicePermissionActivity.hasPermission(this)) {
            runCatching {
                startActivity(Intent(this, KeytaoVoicePermissionActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            }
            return true
        }
        val token = voiceInput.start()
        refreshVoiceInput()
        try {
            val recognizer = SpeechRecognizer.createSpeechRecognizer(this)
            speechRecognizer = recognizer
            recognizer.setRecognitionListener(object : RecognitionListener {
                override fun onReadyForSpeech(params: Bundle?) = voiceInput.runIfCurrent(token) { }
                override fun onBeginningOfSpeech() = voiceInput.runIfCurrent(token) { }
                override fun onRmsChanged(rmsdB: Float) = voiceInput.runIfCurrent(token) {
                    keyboardView?.updateVoiceLevel(rmsdB)
                }
                override fun onBufferReceived(buffer: ByteArray?) = voiceInput.runIfCurrent(token) { }
                override fun onEndOfSpeech() = voiceInput.runIfCurrent(token) {
                    voiceInput.transcribe()
                    refreshVoiceInput()
                }
                override fun onPartialResults(partialResults: Bundle?) = voiceInput.runIfCurrent(token) { }
                override fun onEvent(eventType: Int, params: Bundle?) = voiceInput.runIfCurrent(token) { }

                override fun onError(error: Int) = voiceInput.runIfCurrent(token) {
                    cancelVoiceInput()
                    if (error == SpeechRecognizer.ERROR_INSUFFICIENT_PERMISSIONS) {
                        KeytaoVoicePermissionActivity.hasPermission(this@KeytaoInputMethodService)
                        refreshVoiceInput()
                    }
                    if (error != SpeechRecognizer.ERROR_NO_MATCH && error != SpeechRecognizer.ERROR_SPEECH_TIMEOUT) {
                        Toast.makeText(this@KeytaoInputMethodService, "语音输入失败", Toast.LENGTH_SHORT).show()
                    }
                }

                override fun onResults(results: Bundle?) = voiceInput.runIfCurrent(token) {
                    val text = results?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)?.firstOrNull()
                    cancelVoiceInput()
                    if (!text.isNullOrEmpty()) commitDirect(text)
                }
            })
            recognizer.startListening(Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH).apply {
                putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
                putExtra(RecognizerIntent.EXTRA_LANGUAGE, if (currentState.asciiMode) "en-US" else "zh-CN")
                putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, false)
            })
        } catch (_: RuntimeException) {
            voiceInput.runIfCurrent(token) {
                cancelVoiceInput()
                Toast.makeText(this, "语音输入失败", Toast.LENGTH_SHORT).show()
            }
        }
        return true
    }

    private fun commitCompositionBeforeDirectInput() {
        if (!composing && !currentState.hasComposition) return
        if (currentInputConnection == null) return
        var fallbackState = currentState
        if (!editorUpdatePending && engine.nativeReady) {
            val selected = if (currentState.candidates.isNotEmpty()) {
                engine.selectCandidate(0)
            } else {
                engine.processEnter()
            }
            if (selected.committed.isNotEmpty() || selected.hasComposition) applyState(selected)
            if (selected.committed.isNotEmpty() && !selected.hasComposition) return
            if (selected.hasComposition) fallbackState = selected
            val finished = engine.commitComposition()
            if (finished.committed.isNotEmpty()) {
                applyState(finished)
                if (!finished.hasComposition) return
                fallbackState = finished
            }
        }
        val text = fallbackState.candidates.firstOrNull()?.text ?: fallbackState.preedit
        val cleared = if (!editorUpdatePending && engine.nativeReady) {
            engine.clearComposition().withoutTransientCommit()
        } else {
            KeytaoImeState.empty(asciiMode = currentState.asciiMode)
        }
        applyState(cleared.copy(committed = text))
    }

    private fun commitDirect(text: String) {
        discardPendingHostBackspace()
        inputCounts.record("commit_direct")
        if (emojiSearchQuery != null) {
            commitCompositionBeforeDirectInput()
            emojiSearchQuery = emojiSearchQuery.orEmpty() + text
            keyboardView?.updateEmojiSearch(emojiSearchQuery)
            return
        }
        val connection = currentInputConnection ?: return
        commitCompositionBeforeDirectInput()
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
        } else {
            engine.state().withoutTransientCommit()
        }
        keyboardView?.updateState(currentState)
        if (text.isNotEmpty()) keyboardView?.onDirectTextCommitted()
    }

    private fun applyState(state: KeytaoImeState) {
        if (state.hasComposition || state.committed.isNotEmpty()) discardPendingHostBackspace()
        val started = applyStateDurations.start()
        try {
            if (emojiSearchQuery != null) {
                emojiSearchQuery = emojiSearchQuery.orEmpty() + state.committed
                currentState = state.withoutTransientCommit()
                keyboardView?.updateState(currentState)
                keyboardView?.updateEmojiSearch(emojiSearchQuery)
                return
            }
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
            val keyStarted = pendingKeystrokeStartNs
            pendingKeystrokeStartNs = 0L
            if (keyStarted != 0L) {
                runCatching {
                    KeytaoNativeBridge.nativeLogKeystroke(KeytaoRuntimeLog.elapsedMs(keyStarted))
                }
            }
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
        private const val clipboardTextMaxChars = 20_000
        private const val clipboardHistoryMaxBytes = 256 * 1024
        private const val clipboardHistoryItemsMaxBytes = clipboardHistoryMaxBytes - 1024
        private const val clipboardPrefsMaxBytes = 512 * 1024L
        private const val clipboardMediaLimit = 50
        private const val clipboardMediaMaxBytes = 8L * 1024 * 1024
        private const val clipboardMediaTotalBytes = 48L * 1024 * 1024
        private const val clipboardThumbPx = 176
        private const val clipboardMaxPixels = 20_000_000L
        private const val expandedCandidateLimit = 96

        /** `EditorInfo.MEMORY_EFFICIENT_TEXT_LENGTH`: the budget AOSP recommends
         *  for surrounding-text requests, which cross a binder on every call. */
        private const val backspaceContextLimit = 2048
        // shortcut: selection reads 64K UTF-16 units per side; page context for larger words or lines.
        private const val selectionContextLimit = 65_536
        private const val hostBackspaceContextLimit = 64
        private const val hostBackspaceTimeoutMs = 300L
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
