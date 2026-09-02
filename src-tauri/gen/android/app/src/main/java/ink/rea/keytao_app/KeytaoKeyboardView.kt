package ink.rea.keytao_app

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Outline
import android.graphics.Paint
import android.graphics.Path
import android.graphics.Rect
import android.graphics.RectF
import android.media.AudioManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.provider.Settings
import android.text.TextUtils
import android.text.TextPaint
import android.util.AttributeSet
import android.os.VibrationAttributes
import android.os.VibrationEffect
import android.os.Vibrator
import android.view.HapticFeedbackConstants
import android.view.MotionEvent
import android.view.View
import android.view.ViewConfiguration
import android.view.ViewOutlineProvider
import android.view.accessibility.AccessibilityManager
import android.widget.Button
import androidx.core.view.ViewCompat
import androidx.core.view.accessibility.AccessibilityNodeInfoCompat
import androidx.customview.widget.ExploreByTouchHelper
import kotlin.math.abs
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

class KeytaoKeyboardView @JvmOverloads constructor(
    context: Context,
    attrs: AttributeSet? = null,
) : View(context, attrs) {
    interface Listener {
        fun onKeyCommand(command: KeyCommand)
        fun onCandidate(index: Int, global: Boolean)
        fun onRequestExpandCandidates(callback: (List<KeytaoCandidate>) -> Unit)
        fun onRequestClipboardHistory(callback: (List<String>) -> Unit)
        fun onDeleteClipboardEntry(text: String)
        fun onClearClipboardHistory()
    }

    private data class KeyRect(val spec: KeySpec, val rect: RectF, val sticky: Boolean = false)
    private data class ActiveRowSpan(val weight: Float, var remainingRows: Int)
    private data class KeyTouch(
        var key: KeyRect,
        var keyIndex: Int,
        val originKey: KeyRect,
        val originKeyIndex: Int,
        val downX: Float,
        val downY: Float,
        val downTimeMs: Long = SystemClock.uptimeMillis(),
        var currentX: Float = downX,
        var currentY: Float = downY,
        var longPressConsumed: Boolean = false,
        var backspaceGestureUnits: Int = 0,
        var backspaceGestureConsumed: Boolean = false,
        var alternatePanel: AlternatePanel? = null,
        var cursorGesture: CursorGestureTracker? = null,
    )
    private data class AlternateOption(val label: String, val command: KeyCommand)
    private data class AlternatePanel(
        val options: List<AlternateOption>,
        val rect: RectF,
        val selectionRect: RectF,
        val selectionTracker: AlternateSelectionTracker,
        var selectedIndex: Int?,
    )
    private data class CandidateRect(
        val index: Int,
        val rect: RectF,
        val global: Boolean = false,
        val command: KeyCommand? = null,
        val label: String = "",
    )
    private data class ClipboardDeleteRect(val text: String, val rect: RectF)
    private data class CandidateDrawItem(
        val index: Int,
        val label: String,
        val text: String,
        val comment: String? = null,
        val selected: Boolean = false,
        val global: Boolean = false,
        val command: KeyCommand? = null,
        val clipboardText: String? = null,
        val style: PanelItemStyle = PanelItemStyle.DEFAULT,
    )
    private data class ToolbarAction(
        val label: String,
        val command: KeyCommand,
        val selected: Boolean = false,
        val secondaryLabel: String? = null,
        val icon: ToolbarIcon? = null,
        val longPressCommand: KeyCommand? = null,
    )
    private data class ToolbarRect(
        val label: String,
        val command: KeyCommand,
        val rect: RectF,
        val selected: Boolean = false,
        val secondaryLabel: String? = null,
        val icon: ToolbarIcon? = null,
        val longPressCommand: KeyCommand? = null,
    )
    private data class PanelItem(val label: String, val text: String, val command: KeyCommand, val comment: String? = null)
    private data class RimeOptionSpec(
        val name: String,
        val label: String,
        val onLabel: String,
        val offLabel: String,
    )
    private data class KeyboardLayoutCache(val signature: String, val keys: List<KeyRect>)
    private enum class ToolbarIcon { FUNCTION, SELECTION, CLIPBOARD, EMOJI, GLOBE, ONE_HANDED, FLOATING, BACK, SETTINGS }
    private enum class PanelItemStyle { DEFAULT, SECTION, SCHEMA, OPTION }
    private enum class ShiftState { OFF, ONCE, LOCKED }
    private enum class FunctionPanelMode { RIME, CLIPBOARD }

    var listener: Listener? = null

    private var config: KeytaoAndroidImeConfig = KeytaoAndroidImeConfig.load(context)
    private var theme: KeytaoImeTheme = KeytaoThemeResolver.resolve(context)
    private var keyboardLayoutMode = KeyboardLayoutMode.FULL
    private var oneHandedSide = KeyboardSide.RIGHT
    private var oneHandedAvailable = true
    private var state: KeytaoImeState = KeytaoImeState.empty()
    private var shiftState = ShiftState.OFF
    private var keyboardLayer = "letters"
    private var schemaReady = true
    private var statusMessage: String? = null
    private var systemBottomInsetDp = -1
    private var enterLabelOverride: String? = null
    private var editorRequestedLayer: String? = null
    private var inputMethodSwitchingAvailable = false
    private var keyRects: List<KeyRect> = emptyList()
    private var candidateRects: List<CandidateRect> = emptyList()
    private var expandedCandidateRects: List<CandidateRect> = emptyList()
    private var clipboardDeleteRects: List<ClipboardDeleteRect> = emptyList()
    private var expandedCandidates: List<KeytaoCandidate> = emptyList()
    private var visibleCandidateGlobalIndexes: Set<Int> = emptySet()
    private var toolbarRects: List<ToolbarRect> = emptyList()
    private var candidateExpandRect: RectF? = null
    private var candidateScrollX = 0f
    private var candidateContentWidth = 0f
    private var candidateViewportWidth = 0f
    private var candidateTouchActive = false
    private var candidateDragging = false
    private var candidatePagingConsumed = false
    private var candidatePanelExpanded = false
    private var toolbarOverlayActive = false
    private var functionPanelActive = false
    private var functionPanelMode = FunctionPanelMode.RIME
    private var rimeOptionsState = KeytaoRimeOptionsState.EMPTY
    private var rimeOptionsLoading = false
    private var candidateExpandPressed = false
    private var expandedTouchActive = false
    private var expandedDragging = false
    private var expandedCandidatesLoading = false
    private var clipboardItemsLoading = false
    private var clipboardItems: List<String> = emptyList()
    private var clipboardClearConfirmationPending = false
    private var recentClipboardSuggestion: String? = null
    private var expandedCandidateScrollY = 0f
    private var expandedCandidateContentHeight = 0f
    private var keyboardScrollY = 0f
    private var keyboardDownY = 0f
    private var keyboardDownScrollY = 0f
    private var keyboardDragging = false
    private var keyboardScrollTouchActive = false
    private var keyboardScrollContentHeight = 0f
    private var keyboardScrollViewportHeight = 0f
    private var keyboardScrollViewportTop = 0f
    private var keyboardScrollViewportBottom = 0f
    private var pendingExpandedCandidateLoad: Runnable? = null
    private val candidateWidthCache = mutableMapOf<String, Float>()
    private var expandedCandidateItemsCacheSignature = ""
    private var expandedCandidateItemsCache: List<CandidateDrawItem> = emptyList()
    private var keyboardLayoutCache = KeyboardLayoutCache("", emptyList())
    private var candidateDownX = 0f
    private var candidateDownY = 0f
    private var candidateDownScrollX = 0f
    private var expandedDownY = 0f
    private var expandedDownScrollY = 0f
    private var candidateSignature = ""
    private var contentTransitionStartMs = 0L
    private var expandRequestToken = 0
    private val vibrator: Vibrator? = runCatching {
        @Suppress("DEPRECATION")
        context.getSystemService(Context.VIBRATOR_SERVICE) as? Vibrator
    }.getOrNull()
    private val audioManager: AudioManager? = runCatching {
        context.getSystemService(Context.AUDIO_SERVICE) as? AudioManager
    }.getOrNull()
    private val longPressHandler = Handler(Looper.getMainLooper())
    private val activeKeyTouches = PerPointerLongPressDispatcher<KeyTouch>(
        postDelayed = { runnable, delayMs -> longPressHandler.postDelayed(runnable, delayMs) },
        removeCallbacks = { runnable -> longPressHandler.removeCallbacks(runnable) },
    )
    private var panelGesturePointerId: Int? = null
    private val repeatRunnablesByPointerId = mutableMapOf<Int, Runnable>()
    private var pressedExpandedCandidate: CandidateRect? = null
    private var pressedClipboardDelete: ClipboardDeleteRect? = null
    private var pressedToolbar: ToolbarRect? = null
    private var toolbarTouchActive = false
    private var toolbarLongPressConsumed = false
    private var downX = 0f
    private var downY = 0f
    private var lastShiftTapTimeMs = 0L
    private val toolbarLongPressRunnable = Runnable {
        val toolbar = pressedToolbar ?: return@Runnable
        val command = toolbar.longPressCommand ?: return@Runnable
        toolbarLongPressConsumed = true
        performConfiguredHaptic(strong = true, playSound = false)
        listener?.onKeyCommand(command)
        invalidate()
    }
    private val touchSlop = ViewConfiguration.get(context).scaledTouchSlop
    private val shiftDoubleTapTimeoutMs = ViewConfiguration.getDoubleTapTimeout().toLong()
    private val logoBitmap: Bitmap? = runCatching {
        BitmapFactory.decodeResource(resources, R.mipmap.ic_launcher_foreground)
    }.getOrNull()

    private val paint = Paint(Paint.ANTI_ALIAS_FLAG)
    private val textPaint = TextPaint(Paint.ANTI_ALIAS_FLAG).apply {
        textAlign = Paint.Align.CENTER
    }

    fun updateConfig(next: KeytaoAndroidImeConfig) {
        config = next
        invalidateKeyboardLayoutCache()
        invalidateExpandedCandidateItemsCache()
        resetKeyboardScroll()
        resetCandidateTouch()
        resetCandidateScroll()
        requestLayout()
        invalidate()
    }

    fun currentConfig(): KeytaoAndroidImeConfig = config

    fun updateSystemBottomInsetDp(value: Int) {
        if (value == systemBottomInsetDp) return
        systemBottomInsetDp = value
        invalidateKeyboardLayoutCache()
        requestLayout()
        invalidate()
    }

    fun updateInputMethodSwitching(available: Boolean) {
        if (available == inputMethodSwitchingAvailable) return
        inputMethodSwitchingAvailable = available
        invalidateExpandedCandidateItemsCache()
        invalidate()
    }

    fun updateRimeOptions(next: KeytaoRimeOptionsState) {
        rimeOptionsState = next
        rimeOptionsLoading = false
        invalidateExpandedCandidateItemsCache()
        resetExpandedCandidateScroll()
        invalidate()
    }

    /**
     * What the current editor asked for: the Enter key caption it declared through
     * `imeOptions`/`actionLabel`, and the layer its `inputType` implies.
     */
    fun updateEditorPresentation(enterLabel: String?, requestedLayer: String?) {
        val labelChanged = enterLabel != enterLabelOverride
        val previousRequest = editorRequestedLayer
        enterLabelOverride = enterLabel
        editorRequestedLayer = requestedLayer
        val targetLayer = when {
            requestedLayer != null && config.hasLayer(requestedLayer) -> requestedLayer
            // Leaving a numeric editor has to undo the layer it forced on us;
            // an editor with no request never overrides the user's own choice.
            previousRequest != null -> "letters"
            else -> null
        }
        if (targetLayer != null && targetLayer != keyboardLayer) {
            setKeyboardLayer(targetLayer)
            return
        }
        if (labelChanged) {
            invalidateKeyboardLayoutCache()
            invalidate()
        }
    }

    fun updateTheme(next: KeytaoImeTheme) {
        theme = next
        candidateWidthCache.clear()
        invalidateKeyboardLayoutCache()
        invalidateOutline()
        invalidate()
    }

    fun updateLayoutPresentation(
        mode: KeyboardLayoutMode,
        oneHandedSide: KeyboardSide,
        oneHandedAvailable: Boolean,
    ) {
        if (
            keyboardLayoutMode == mode &&
            this.oneHandedSide == oneHandedSide &&
            this.oneHandedAvailable == oneHandedAvailable
        ) {
            return
        }
        keyboardLayoutMode = mode
        this.oneHandedSide = oneHandedSide
        this.oneHandedAvailable = oneHandedAvailable
        val compact = mode != KeyboardLayoutMode.FULL
        clipToOutline = compact
        elevation = when (mode) {
            KeyboardLayoutMode.FLOATING -> dp(8f)
            KeyboardLayoutMode.ONE_HANDED -> dp(4f)
            KeyboardLayoutMode.FULL -> 0f
        }
        outlineProvider = if (compact) floatingOutlineProvider else ViewOutlineProvider.BACKGROUND
        invalidateOutline()
        invalidate()
    }

    fun updateState(next: KeytaoImeState) {
        val nextSignature = candidateSignature(next)
        if (nextSignature != candidateSignature) {
            candidateSignature = nextSignature
            cancelExpandedCandidateRequest()
            expandedCandidates = emptyList()
            invalidateExpandedCandidateItemsCache()
            resetCandidateScroll()
            resetExpandedCandidateScroll()
        }
        val wasExpanded = candidatePanelExpanded
        if (next.candidatePanel.candidates.isEmpty() && !functionPanelActive) {
            candidatePanelExpanded = false
            expandedCandidates = emptyList()
            expandedCandidatesLoading = false
            invalidateExpandedCandidateItemsCache()
            resetExpandedCandidateScroll()
        }
        if (next.candidatePanel.candidates.isEmpty() && next.candidatePanel.preedit.isNullOrEmpty() && next.preedit.isEmpty()) {
            toolbarOverlayActive = false
        }
        state = next
        if (schemaReady) statusMessage = null
        if (wasExpanded != candidatePanelExpanded) {
            startContentTransition()
        }
        if (next.hasComposition || next.candidatePanel.candidates.isNotEmpty()) {
            recentClipboardSuggestion = null
        }
        invalidate()
    }

    fun updateAvailability(ready: Boolean, message: String) {
        schemaReady = ready
        statusMessage = if (ready) null else message
        invalidate()
    }

    fun showMessage(message: String) {
        statusMessage = message
        invalidate()
    }

    fun showRecentClipboardSuggestion(text: String) {
        val normalized = text
            .replace(whitespaceRegex, " ")
            .trim()
            .takeIf { it.isNotEmpty() }
            ?: return
        recentClipboardSuggestion = normalized
        if (functionPanelActive || candidatePanelExpanded) {
            closeCandidatePanel()
        }
        invalidate()
    }

    fun clearRecentClipboardSuggestion() {
        if (recentClipboardSuggestion == null) return
        recentClipboardSuggestion = null
        invalidate()
    }

    fun resetClipboardClearConfirmation() {
        if (!clipboardClearConfirmationPending) return
        clipboardClearConfirmationPending = false
        invalidate()
    }

    fun setKeyboardLayer(value: String?) {
        val nextLayer = config.normalizedLayer(value)
        val changed = nextLayer != keyboardLayer || candidatePanelExpanded
        keyboardLayer = nextLayer
        candidatePanelExpanded = false
        toolbarOverlayActive = false
        functionPanelActive = false
        functionPanelMode = FunctionPanelMode.RIME
        rimeOptionsState = KeytaoRimeOptionsState.EMPTY
        rimeOptionsLoading = false
        clipboardClearConfirmationPending = false
        expandedCandidates = emptyList()
        cancelExpandedCandidateRequest()
        clipboardItemsLoading = false
        clearActiveKeyTouches()
        pressedToolbar = null
        toolbarTouchActive = false
        resetExpandedCandidateScroll()
        resetKeyboardScroll()
        if (changed) startContentTransition()
        invalidate()
    }

    fun toggleShift() {
        val now = System.currentTimeMillis()
        shiftState = when (shiftState) {
            ShiftState.OFF -> {
                lastShiftTapTimeMs = now
                ShiftState.ONCE
            }
            ShiftState.ONCE -> {
                val doubleTap = now - lastShiftTapTimeMs <= shiftDoubleTapTimeoutMs
                lastShiftTapTimeMs = 0L
                if (doubleTap) ShiftState.LOCKED else ShiftState.OFF
            }
            ShiftState.LOCKED -> {
                lastShiftTapTimeMs = 0L
                ShiftState.OFF
            }
        }
        invalidate()
    }

    override fun onMeasure(widthMeasureSpec: Int, heightMeasureSpec: Int) {
        val width = MeasureSpec.getSize(widthMeasureSpec)
        val desiredHeight = dp(config.keyboardHeightDp + config.candidateBarHeightDp + effectiveKeyboardBottomInsetDp()).toInt()
        val resolvedHeight = resolveSize(desiredHeight, heightMeasureSpec)
        setMeasuredDimension(width, resolvedHeight)
    }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        invalidateKeyboardLayoutCache()
        coerceCandidateScroll()
        coerceExpandedCandidateScroll()
    }

    override fun onDraw(canvas: Canvas) {
        super.onDraw(canvas)
        keyRects = emptyList()
        candidateRects = emptyList()
        expandedCandidateRects = emptyList()
        clipboardDeleteRects = emptyList()
        toolbarRects = emptyList()
        candidateExpandRect = null
        drawBackground(canvas)
        drawCandidateBar(canvas)
        if (candidatePanelExpanded) {
            drawExpandedCandidatePanel(canvas)
        } else {
            drawKeyboard(canvas)
            drawKeyFeedbackOverlays(canvas)
        }
        drawFloatingInteractionHints(canvas)
        refreshAccessibilityNodes()
    }

    // The keyboard is one self-drawn View, so a screen reader can only reach the
    // keys through virtual nodes; they are backed by the very rectangles
    // onTouchEvent hit-tests, and activating one runs the same command path.

    private data class AccessibilityTarget(
        val id: Int,
        val label: String,
        val rect: RectF,
        val activate: () -> Unit,
    )

    private val accessibilityHelper = KeyboardAccessibilityHelper()
    private val accessibilityManager: AccessibilityManager? = runCatching {
        context.getSystemService(Context.ACCESSIBILITY_SERVICE) as? AccessibilityManager
    }.getOrNull()
    private var accessibilityNodeSignature: String? = null

    init {
        ViewCompat.setAccessibilityDelegate(this, accessibilityHelper)
        contentDescription = context.getString(R.string.keytao_keyboard_description)
    }

    override fun dispatchHoverEvent(event: MotionEvent): Boolean {
        return accessibilityHelper.dispatchHoverEvent(event) || super.dispatchHoverEvent(event)
    }

    private fun refreshAccessibilityNodes() {
        if (accessibilityManager?.isEnabled != true) return
        val signature = buildString {
            append(keyboardLayer).append('|')
            append(shiftState).append('|')
            append(keyRects.size).append('|')
            append(candidateRects.size).append('|')
            append(expandedCandidateRects.size).append('|')
            append(clipboardDeleteRects.size).append('|')
            append(toolbarRects.size).append('|')
            append(clipboardClearConfirmationPending).append('|')
            append(candidateExpandRect != null).append('|')
            append(candidateSignature)
        }
        if (signature == accessibilityNodeSignature) return
        accessibilityNodeSignature = signature
        accessibilityHelper.invalidateRoot()
    }

    private fun accessibilityTargets(): List<AccessibilityTarget> {
        val targets = mutableListOf<AccessibilityTarget>()
        candidateExpandRect?.let { rect ->
            targets.add(
                AccessibilityTarget(accessibilityExpandNodeId, "展开候选", rect) { toggleCandidatePanel() }
            )
        }
        toolbarRects.forEachIndexed { index, toolbar ->
            targets.add(
                AccessibilityTarget(
                    accessibilityToolbarNodeBase + index,
                    toolbar.label.ifBlank { toolbar.secondaryLabel.orEmpty() }.ifBlank { "工具栏按钮" },
                    toolbar.rect,
                ) { handleToolbarCommand(toolbar.command) }
            )
        }
        candidateRects.forEachIndexed { index, candidate ->
            targets.add(
                AccessibilityTarget(
                    accessibilityCandidateNodeBase + index,
                    candidate.label.ifBlank { "候选 ${candidate.index + 1}" },
                    candidate.rect,
                ) { listener?.onCandidate(candidate.index, candidate.global) }
            )
        }
        clipboardDeleteRects.forEachIndexed { index, delete ->
            targets.add(
                AccessibilityTarget(
                    accessibilityClipboardDeleteNodeBase + index,
                    "删除剪贴板历史：${delete.text}",
                    delete.rect,
                ) { deleteClipboardEntry(delete.text) }
            )
        }
        expandedCandidateRects.forEachIndexed { index, candidate ->
            targets.add(
                AccessibilityTarget(
                    accessibilityExpandedNodeBase + index,
                    candidate.label.ifBlank { "候选 ${candidate.index + 1}" },
                    candidate.rect,
                ) {
                    val command = candidate.command
                    if (command != null) {
                        handlePanelCommand(command)
                    } else {
                        closeCandidatePanel()
                        listener?.onCandidate(candidate.index, candidate.global)
                    }
                }
            )
        }
        keyRects.forEachIndexed { index, key ->
            targets.add(
                AccessibilityTarget(
                    accessibilityKeyNodeBase + index,
                    displayLabel(key.spec).ifBlank { key.spec.label }.ifBlank { "按键" },
                    key.rect,
                ) { activateKey(key) }
            )
        }
        return targets
    }

    private inner class KeyboardAccessibilityHelper : ExploreByTouchHelper(this@KeytaoKeyboardView) {
        override fun getVirtualViewAt(x: Float, y: Float): Int {
            return accessibilityTargets().firstOrNull { it.rect.contains(x, y) }?.id ?: HOST_ID
        }

        override fun getVisibleVirtualViews(virtualViewIds: MutableList<Int>) {
            accessibilityTargets().forEach { virtualViewIds.add(it.id) }
        }

        @Suppress("DEPRECATION")
        override fun onPopulateNodeForVirtualView(
            virtualViewId: Int,
            node: AccessibilityNodeInfoCompat,
        ) {
            val target = accessibilityTargets().firstOrNull { it.id == virtualViewId }
            if (target == null) {
                node.contentDescription = ""
                node.setBoundsInParent(Rect(0, 0, 1, 1))
                return
            }
            node.className = Button::class.java.name
            node.contentDescription = target.label
            node.isEnabled = true
            node.isClickable = true
            node.addAction(AccessibilityNodeInfoCompat.ACTION_CLICK)
            node.setBoundsInParent(
                Rect(
                    target.rect.left.roundToInt(),
                    target.rect.top.roundToInt(),
                    target.rect.right.roundToInt(),
                    target.rect.bottom.roundToInt(),
                )
            )
        }

        override fun onPerformActionForVirtualView(
            virtualViewId: Int,
            action: Int,
            arguments: Bundle?,
        ): Boolean {
            if (action != AccessibilityNodeInfoCompat.ACTION_CLICK) return false
            val target = accessibilityTargets().firstOrNull { it.id == virtualViewId } ?: return false
            target.activate()
            return true
        }
    }

    override fun onDetachedFromWindow() {
        clearActiveKeyTouches()
        super.onDetachedFromWindow()
    }

    override fun onWindowVisibilityChanged(visibility: Int) {
        if (visibility != VISIBLE) {
            clearActiveKeyTouches()
        }
        super.onWindowVisibilityChanged(visibility)
    }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                clearActiveKeyTouches()
                panelGesturePointerId = null
                downX = event.x
                downY = event.y
                candidateDownX = event.x
                candidateDownY = event.y
                candidateDownScrollX = candidateScrollX
                candidatePagingConsumed = false
                expandedDownY = event.y
                expandedDownScrollY = expandedCandidateScrollY
                val hasCandidates = state.candidatePanel.candidates.isNotEmpty()
                candidateExpandPressed = !functionPanelActive && hasCandidates && isInCandidateBar(event.y) &&
                    candidateExpandRect?.contains(event.x, event.y) == true
                val toolbar = if (isInCandidateBar(event.y)) {
                    findToolbar(event.x, event.y)
                } else {
                    null
                }
                pressedToolbar = toolbar
                toolbarTouchActive = toolbar != null
                candidateTouchActive = !functionPanelActive && !toolbarOverlayActive && !candidateExpandPressed &&
                    !toolbarTouchActive && isInCandidateBar(event.y) && hasCandidates
                expandedTouchActive = !candidateTouchActive && !candidateExpandPressed && isInExpandedCandidatePanel(event.y)
                candidateDragging = false
                expandedDragging = false
                keyboardDownY = event.y
                keyboardDownScrollY = keyboardScrollY
                keyboardDragging = false
                keyboardScrollTouchActive = !candidateTouchActive &&
                    !toolbarTouchActive &&
                    !candidateExpandPressed &&
                    !expandedTouchActive &&
                    usesCategorizedSymbolKeyboard() &&
                    maxKeyboardScroll() > 0f &&
                    event.y >= keyboardScrollViewportTop &&
                    event.y < keyboardScrollViewportBottom
                stopLongPressAndRepeat()
                toolbarLongPressConsumed = false
                if (toolbar?.longPressCommand != null) {
                    longPressHandler.postDelayed(toolbarLongPressRunnable, config.longPressDelayMs)
                }
                pressedClipboardDelete = if (expandedTouchActive) findClipboardDelete(event.x, event.y) else null
                pressedExpandedCandidate = if (expandedTouchActive && pressedClipboardDelete == null) {
                    findExpandedCandidate(event.x, event.y)
                } else {
                    null
                }
                if (!candidateTouchActive && !toolbarTouchActive && !candidateExpandPressed && !expandedTouchActive) {
                    findKey(event.x, event.y)?.let { key ->
                        beginKeyTouch(
                            event.getPointerId(event.actionIndex),
                            key,
                            event.x,
                            event.y,
                        )
                    }
                }
                if (candidateTouchActive || toolbarTouchActive || candidateExpandPressed || expandedTouchActive || keyboardScrollTouchActive) {
                    val pointerId = event.getPointerId(event.actionIndex)
                    panelGesturePointerId = pointerId
                }
                invalidate()
                return true
            }
            MotionEvent.ACTION_POINTER_DOWN -> {
                val pointerIndex = event.actionIndex
                val pointerId = event.getPointerId(pointerIndex)
                val x = event.getX(pointerIndex)
                val y = event.getY(pointerIndex)
                if (
                    keyboardScrollTouchActive &&
                    keyboardDragging &&
                    y >= keyboardScrollViewportTop &&
                    y < keyboardScrollViewportBottom
                ) {
                    return true
                }
                if (isInCandidateBar(y) || isInExpandedCandidatePanel(y)) {
                    return true
                }
                findKey(x, y)?.let { key ->
                    beginKeyTouch(
                        pointerId,
                        key,
                        x,
                        y,
                    )
                    invalidate()
                }
                return true
            }
            MotionEvent.ACTION_MOVE -> {
                updatePanelGestureMove(event)
                if (activeKeyTouches.isNotEmpty()) {
                    updateKeyTouchMove(event)
                }
                return true
            }
            MotionEvent.ACTION_POINTER_UP -> {
                val pointerIndex = event.actionIndex
                val pointerId = event.getPointerId(pointerIndex)
                val x = event.getX(pointerIndex)
                val y = event.getY(pointerIndex)
                val handled = if (pointerId == panelGesturePointerId) {
                    panelGesturePointerId = null
                    finishPanelGesture(x, y) || finishKeyTouch(pointerId, x, y)
                } else {
                    finishKeyTouch(pointerId, x, y)
                }
                if (handled) {
                    invalidate()
                }
                return true
            }
            MotionEvent.ACTION_UP -> {
                val pointerId = event.getPointerId(event.actionIndex)
                if (pointerId == panelGesturePointerId && finishPanelGesture(event.x, event.y)) {
                    panelGesturePointerId = null
                    invalidate()
                    return true
                }
                if (finishKeyTouch(pointerId, event.x, event.y)) {
                    panelGesturePointerId = null
                    invalidate()
                    return true
                }
                stopLongPressAndRepeat()
                panelGesturePointerId = null
                invalidate()
                return true
            }
            MotionEvent.ACTION_CANCEL -> {
                stopLongPressAndRepeat()
                clearActiveKeyTouches()
                resetCandidateTouch()
                resetExpandedCandidateTouch()
                keyboardScrollTouchActive = false
                keyboardDragging = false
                panelGesturePointerId = null
                pressedToolbar = null
                toolbarTouchActive = false
                toolbarLongPressConsumed = false
                candidateExpandPressed = false
                invalidate()
                return true
            }
        }
        return true
    }

    private fun drawBackground(canvas: Canvas) {
        if (keyboardLayoutMode != KeyboardLayoutMode.FULL) {
            val strokeWidth = max(1f, dp(1f))
            val halfStroke = strokeWidth / 2f
            val panelRect = RectF(
                halfStroke,
                halfStroke,
                width.toFloat() - halfStroke,
                height.toFloat() - halfStroke,
            )
            val radius = dp(theme.panelCornerRadiusDp)
            paint.style = Paint.Style.FILL
            paint.color = panelBackgroundColor()
            canvas.drawRoundRect(panelRect, radius, radius, paint)
            paint.style = Paint.Style.STROKE
            paint.strokeWidth = strokeWidth
            paint.color = theme.panelBorder.toArgb()
            canvas.drawRoundRect(panelRect, radius, radius, paint)
            return
        }
        paint.style = Paint.Style.FILL
        paint.color = panelBackgroundColor()
        canvas.drawRect(0f, 0f, width.toFloat(), height.toFloat(), paint)
        paint.style = Paint.Style.STROKE
        paint.strokeWidth = max(1f, dp(1f))
        paint.color = theme.panelBorder.toArgb()
        canvas.drawLine(0f, 0f, width.toFloat(), 0f, paint)
        val bottomInset = bottomReservedInset()
        if (bottomInset > 0f) {
            val bottomTop = height.toFloat() - bottomInset
            paint.color = Color.argb(38, theme.panelBorder.red, theme.panelBorder.green, theme.panelBorder.blue)
            canvas.drawLine(0f, bottomTop, width.toFloat(), bottomTop, paint)
        }
    }

    private fun drawCandidateBar(canvas: Canvas) {
        val barHeight = dp(config.candidateBarHeightDp)
        val gap = dp(theme.panelGapDp)
        val leftPadding = gap * 1.5f
        val centerY = barHeight / 2f
        val panelModel = state.candidatePanel
        val preedit = panelModel.preedit ?: state.preedit
        val hasInlineInput = panelModel.candidates.isNotEmpty() || preedit.isNotEmpty()
        val message = statusMessage?.takeIf { it.isNotBlank() }
        visibleCandidateGlobalIndexes = emptySet()

        if (!schemaReady || (message != null && panelModel.candidates.isEmpty() && panelModel.preedit.isNullOrEmpty())) {
            resetCandidateScroll()
            toolbarRects = emptyList()
            candidateRects = emptyList()
            candidateExpandRect = null
            textPaint.textSize = sp(theme.preeditSizeSp)
            textPaint.color = statusMessageColor()
            textPaint.textAlign = Paint.Align.LEFT
            canvas.drawText(
                message ?: "请先在 KeyTao App 安装键道方案",
                leftPadding,
                centerY + textBaselineOffset(textPaint),
                textPaint,
            )
            return
        }

        if (functionPanelActive) {
            resetCandidateScroll()
            candidateRects = emptyList()
            candidateExpandRect = null
            drawFunctionPanelBar(canvas, barHeight, leftPadding)
            return
        }

        if (usesFullHeightSymbolKeyboard()) {
            resetCandidateScroll()
            toolbarRects = emptyList()
            candidateRects = emptyList()
            candidateExpandRect = null
            return
        }

        if (toolbarOverlayActive && hasInlineInput) {
            resetCandidateScroll()
            candidateRects = emptyList()
            candidateExpandRect = null
            drawToolbar(
                canvas,
                barHeight,
                leftPadding,
                actions = listOf(toolbarToggleAction(selected = true)) + toolbarActions(),
                showLogo = false,
            )
            return
        }

        if (panelModel.candidates.isEmpty()) {
            resetCandidateScroll()
            candidateRects = emptyList()
            candidateExpandRect = null
            if (preedit.isNotEmpty()) {
                val toggle = drawToolbarToggleButton(canvas, barHeight, leftPadding)
                toolbarRects = listOf(toggle)
                textPaint.color = theme.labelColor.toArgb()
                textPaint.textSize = sp(theme.preeditSizeSp)
                textPaint.textAlign = Paint.Align.LEFT
                canvas.drawText(preedit, toggle.rect.right + gap, centerY + textBaselineOffset(textPaint), textPaint)
                drawKeytaoLogo(canvas, barHeight, leftPadding)
                return
            }
            if (recentClipboardSuggestion != null) {
                drawClipboardSuggestionBar(canvas, barHeight, leftPadding)
                return
            }
            drawToolbar(canvas, barHeight, leftPadding)
            return
        }

        val toggle = drawToolbarToggleButton(canvas, barHeight, leftPadding)
        toolbarRects = listOf(toggle)
        val expandRect = drawCandidateExpandButton(canvas, barHeight, leftPadding)
        val viewportLeft = toggle.rect.right
        val maxRight = expandRect.left - gap
        val viewportWidth = (maxRight - viewportLeft).coerceAtLeast(0f)
        candidateViewportWidth = viewportWidth
        val items = panelModel.candidates.map { candidate ->
            CandidateDrawItem(
                index = candidate.index,
                label = candidate.label,
                text = candidate.text,
                comment = candidate.comment,
                selected = candidate.selected,
            )
        }
        val itemWidths = items.map(::candidateWidth)
        candidateContentWidth = itemWidths.sum() + gap * (itemWidths.size - 1).coerceAtLeast(0)
        candidateScrollX = candidateScrollX.coerceIn(0f, max(0f, candidateContentWidth - viewportWidth))
        val nextCandidateRects = mutableListOf<CandidateRect>()
        val nextVisibleGlobalIndexes = mutableSetOf<Int>()
        canvas.save()
        canvas.clipRect(viewportLeft, 0f, maxRight, barHeight)

        val candidateHeight = minOf(dp(38f), barHeight - gap * 1.8f)
        val candidateTop = (barHeight - candidateHeight) / 2f
        var contentX = viewportLeft - candidateScrollX
        for ((item, requestedWidth) in items.zip(itemWidths)) {
            val globalIndex = panelCandidateGlobalIndex(item.index)
            val rect = RectF(contentX, candidateTop, contentX + requestedWidth, candidateTop + candidateHeight)
            drawCandidateOption(canvas, item, rect)
            val hitRect = RectF(
                max(rect.left, viewportLeft),
                rect.top,
                min(rect.right, maxRight),
                rect.bottom,
            )
            if (hitRect.right > hitRect.left) {
                nextCandidateRects.add(CandidateRect(globalIndex, hitRect, global = true, label = item.text))
                nextVisibleGlobalIndexes.add(globalIndex)
            }
            contentX = rect.right + gap
        }
        canvas.restore()
        candidateRects = nextCandidateRects
        visibleCandidateGlobalIndexes = nextVisibleGlobalIndexes
    }

    private fun toolbarToggleAction(selected: Boolean = false): ToolbarAction {
        return ToolbarAction(
            label = if (state.asciiMode) "EN" else "中",
            command = KeyCommand.panel("toggleToolbar"),
            selected = selected,
        )
    }

    private fun drawToolbarToggleButton(canvas: Canvas, barHeight: Float, leftPadding: Float): ToolbarRect {
        val height = minOf(dp(34f), barHeight - dp(12f))
        val top = (barHeight - height) / 2f
        val action = toolbarToggleAction()
        val item = ToolbarRect(
            label = action.label,
            command = action.command,
            rect = RectF(
                leftPadding,
                top,
                leftPadding + dp(KeytaoImeInteractionTuning.CANDIDATE_TOOLBAR_TOGGLE_WIDTH_DP),
                top + height,
            ),
            selected = action.selected,
        )
        drawToolbarChip(canvas, item)
        return item
    }

    private fun drawCandidateExpandButton(canvas: Canvas, barHeight: Float, leftPadding: Float): RectF {
        val size = minOf(dp(38f), barHeight - dp(10f))
        val left = width - leftPadding - size
        val top = (barHeight - size) / 2f
        val rect = RectF(left, top, left + size, top + size)
        candidateExpandRect = rect

        drawSurfaceShadow(canvas, rect, candidateExpandPressed)
        paint.style = Paint.Style.FILL
        paint.color = if (candidateExpandPressed) {
            theme.keySelectedBackground.toArgb()
        } else {
            keyBackgroundColor()
        }
        canvas.drawRoundRect(rect, dp(keyCornerRadiusDp()), dp(keyCornerRadiusDp()), paint)

        textPaint.textAlign = Paint.Align.CENTER
        textPaint.textSize = sp(theme.fontSizeSp)
        textPaint.color = if (candidateExpandPressed) {
            theme.keySelectedForeground.toArgb()
        } else {
            theme.keyForeground.toArgb()
        }
        canvas.drawText(
            if (candidatePanelExpanded) "⌃" else "⌄",
            rect.centerX(),
            rect.centerY() + textBaselineOffset(textPaint),
            textPaint,
        )
        return rect
    }

    private fun drawExpandedCandidatePanel(canvas: Canvas) {
        val panelHeight = expandedCandidatePanelHeight()
        if (panelHeight <= 0f) return

        val top = dp(config.candidateBarHeightDp)
        val bottom = keyboardBottom()
        val gap = dp(7f)
        val left = gap * 1.5f
        val right = width - left
        val columns = panelColumns(if (functionPanelActive) functionPanelMode else FunctionPanelMode.RIME)
        val defaultRowHeight = dp(
            when (columns) {
                4 -> 52f
                1 -> config.clipboardRowHeightDp
                else -> 36f
            }
        )
        val cellWidth = columns?.let { columnCount ->
            (right - left - gap * (columnCount - 1)) / columnCount
        }
        val visibleRect = RectF(0f, top, width.toFloat(), bottom)
        val items = expandedCandidateItems()
        val structuredRime = functionPanelActive && functionPanelMode == FunctionPanelMode.RIME
        val nextRects = mutableListOf<CandidateRect>()
        val nextClipboardDeleteRects = mutableListOf<ClipboardDeleteRect>()

        drawContentLayer(canvas, top) {
            paint.style = Paint.Style.FILL
            paint.color = panelBackgroundColor()
            canvas.drawRect(visibleRect, paint)
            paint.style = Paint.Style.STROKE
            paint.strokeWidth = max(1f, dp(1f))
            paint.color = theme.panelBorder.toArgb()
            canvas.drawLine(0f, top, width.toFloat(), top, paint)

            var x = left
            var y = top + gap - expandedCandidateScrollY
            var contentBottom = top + gap
            canvas.save()
            canvas.clipRect(visibleRect)
            if (items.isEmpty()) {
                textPaint.textAlign = Paint.Align.CENTER
                textPaint.textSize = sp(theme.labelSizeSp)
                textPaint.color = theme.commentColor.toArgb()
                val message = when {
                    clipboardItemsLoading -> "正在读取剪贴板"
                    rimeOptionsLoading && functionPanelMode == FunctionPanelMode.RIME -> "正在加载 Rime 选项"
                    expandedCandidatesLoading && functionPanelActive -> "正在加载功能"
                    expandedCandidatesLoading -> "正在加载候选"
                    functionPanelActive && functionPanelMode == FunctionPanelMode.CLIPBOARD -> "剪贴板为空"
                    functionPanelActive -> "暂无功能项"
                    else -> "没有更多候选"
                }
                canvas.drawText(message, width / 2f, top + panelHeight / 2f + textBaselineOffset(textPaint), textPaint)
            }
            for ((index, item) in items.withIndex()) {
                val chipWidth: Float
                val itemRowHeight: Float
                if (structuredRime) {
                    when (item.style) {
                        PanelItemStyle.SECTION -> {
                            if (x > left) {
                                x = left
                                y += dp(44f) + gap
                            }
                            chipWidth = right - left
                            itemRowHeight = dp(28f)
                        }
                        PanelItemStyle.SCHEMA -> {
                            if (x > left) {
                                x = left
                                y += dp(44f) + gap
                            }
                            chipWidth = right - left
                            itemRowHeight = dp(44f)
                        }
                        PanelItemStyle.OPTION -> {
                            chipWidth = (right - left - gap) / 2f
                            itemRowHeight = dp(44f)
                        }
                        PanelItemStyle.DEFAULT -> {
                            chipWidth = right - left
                            itemRowHeight = defaultRowHeight
                        }
                    }
                } else if (columns != null && cellWidth != null) {
                    val column = index % columns
                    val row = index / columns
                    x = left + column * (cellWidth + gap)
                    y = top + gap + row * (defaultRowHeight + gap) - expandedCandidateScrollY
                    chipWidth = cellWidth
                    itemRowHeight = defaultRowHeight
                } else {
                    chipWidth = candidateWidth(item)
                        .coerceAtLeast(dp(56f))
                        .coerceAtMost(right - left)
                    if (x + chipWidth > right && x > left) {
                        x = left
                        y += defaultRowHeight + gap
                    }
                    itemRowHeight = defaultRowHeight
                }
                val rect = RectF(x, y, x + chipWidth, y + itemRowHeight)
                if (rect.bottom >= top && rect.top <= bottom) {
                    drawCandidateOption(canvas, item, rect)
                    val hitRect = if (item.clipboardText != null) {
                        RectF(rect.left, rect.top, rect.right - dp(config.clipboardDeleteHitWidthDp), rect.bottom)
                    } else {
                        rect
                    }
                    if (item.style != PanelItemStyle.SECTION) {
                        nextRects.add(
                            CandidateRect(
                                item.index,
                                hitRect,
                                item.global,
                                item.command,
                                listOf(item.label, item.text).filter(String::isNotBlank).joinToString(" "),
                            )
                        )
                    }
                    item.clipboardText?.let { clipboardText ->
                        nextClipboardDeleteRects.add(
                            ClipboardDeleteRect(
                                clipboardText,
                                RectF(hitRect.right, rect.top, rect.right, rect.bottom),
                            )
                        )
                    }
                }
                contentBottom = max(contentBottom, rect.bottom + expandedCandidateScrollY)
                if (structuredRime) {
                    when (item.style) {
                        PanelItemStyle.SECTION, PanelItemStyle.SCHEMA, PanelItemStyle.DEFAULT -> {
                            x = left
                            y = rect.bottom + gap
                        }
                        PanelItemStyle.OPTION -> {
                            if (x > left) {
                                x = left
                                y = rect.bottom + gap
                            } else {
                                x = rect.right + gap
                            }
                        }
                    }
                } else if (columns == null) {
                    x = rect.right + gap
                }
            }
            canvas.restore()

            expandedCandidateContentHeight = (contentBottom - top + gap).coerceAtLeast(panelHeight)
        }

        expandedCandidateRects = nextRects
        clipboardDeleteRects = nextClipboardDeleteRects
        coerceExpandedCandidateScroll()
    }

    private fun panelColumns(mode: FunctionPanelMode): Int? {
        return when (mode) {
            FunctionPanelMode.CLIPBOARD -> 1
            FunctionPanelMode.RIME -> null
        }
    }

    private fun expandedCandidateItems(): List<CandidateDrawItem> {
        val signature = expandedCandidateItemsSignature()
        if (signature == expandedCandidateItemsCacheSignature) {
            return expandedCandidateItemsCache
        }
        val items = if (functionPanelActive) {
            when (functionPanelMode) {
                FunctionPanelMode.CLIPBOARD -> clipboardPanelItems()
                FunctionPanelMode.RIME -> rimePanelItems()
            }
        } else {
            rimePanelItems()
        }
        expandedCandidateItemsCacheSignature = signature
        expandedCandidateItemsCache = items
        return items
    }

    private fun rimePanelItems(): List<CandidateDrawItem> {
        if (functionPanelActive && functionPanelMode == FunctionPanelMode.RIME) {
            if (rimeOptionsLoading && rimeOptionsState == KeytaoRimeOptionsState.EMPTY) {
                return emptyList()
            }
            val current = rimeOptionsState.currentSchema
            val schemas = buildList<KeytaoRimeSchema> {
                addAll(rimeOptionsState.schemas)
                if (current != null && none { it.id == current.id }) add(current)
            }
            val items = mutableListOf(
                CandidateDrawItem(
                    index = -2000,
                    label = "输入方案",
                    text = "",
                    style = PanelItemStyle.SECTION,
                )
            )
            schemas.forEachIndexed { index, schema ->
                items.add(
                    CandidateDrawItem(
                        index = -2100 - index,
                        label = schema.name,
                        text = schema.id,
                        selected = schema.id == current?.id,
                        command = KeyCommand(KeyCommandTypes.RIME_SCHEMA, schema.id),
                        style = PanelItemStyle.SCHEMA,
                    )
                )
            }
            items.add(
                CandidateDrawItem(
                    index = -3000,
                    label = "选项",
                    text = "",
                    style = PanelItemStyle.SECTION,
                )
            )
            rimeOptionSpecs.forEachIndexed { index, spec ->
                val enabled = rimeOptionsState.options[spec.name] == true
                items.add(
                    CandidateDrawItem(
                        index = -3100 - index,
                        label = spec.label,
                        text = if (enabled) spec.onLabel else spec.offLabel,
                        selected = enabled,
                        command = KeyCommand(
                            KeyCommandTypes.RIME_OPTION,
                            spec.name,
                            (!enabled).toString(),
                        ),
                        style = PanelItemStyle.OPTION,
                    )
                )
            }
            return items
        }
        val all = expandedCandidates
            .takeIf { it.isNotEmpty() }
            ?: state.candidates.map { candidate ->
                candidate.copy(index = panelCandidateGlobalIndex(candidate.index))
            }
        val selectedGlobalIndex = selectedGlobalCandidateIndex()
        return all.map { candidate ->
            CandidateDrawItem(
                index = candidate.index,
                label = "${candidate.index + 1}.",
                text = candidate.text,
                comment = candidate.comment,
                selected = candidate.index == selectedGlobalIndex,
                global = true,
            )
        }.filterNot { item -> !functionPanelActive && item.index in visibleCandidateGlobalIndexes }
    }

    private fun expandedCandidateItemsSignature(): String {
        return buildString {
            append(functionPanelActive)
            append('|')
            append(functionPanelMode)
            append('|')
            append(panelColumns(if (functionPanelActive) functionPanelMode else FunctionPanelMode.RIME) ?: "flow")
            append('|')
            append(candidateSignature)
            append('|')
            append(selectedGlobalCandidateIndex())
            append('|')
            if (!functionPanelActive) {
                visibleCandidateGlobalIndexes.sorted().forEach { index ->
                    append(index)
                    append(',')
                }
            }
            append('|')
            val source = expandedCandidates
                .takeIf { it.isNotEmpty() }
                ?: state.candidates
            appendCandidateListSignature(source)
            if (functionPanelActive && functionPanelMode == FunctionPanelMode.RIME) {
                append('|')
                append(rimeOptionsLoading)
                append('|')
                append(rimeOptionsState.currentSchema?.id.orEmpty())
                rimeOptionsState.schemas.forEach { schema ->
                    append('|')
                    append(schema.id)
                    append(':')
                    append(schema.name)
                }
                rimeOptionsState.options.toSortedMap().forEach { (name, enabled) ->
                    append('|')
                    append(name)
                    append(':')
                    append(enabled)
                }
            }
            if (functionPanelActive && functionPanelMode == FunctionPanelMode.CLIPBOARD) {
                append('|')
                clipboardItems.forEach { item ->
                    append(item.length)
                    append(':')
                    append(item)
                    append('\u0001')
                }
            }
        }
    }

    private fun StringBuilder.appendCandidateListSignature(candidates: List<KeytaoCandidate>) {
        candidates.forEach { candidate ->
            append(candidate.index)
            append(':')
            append(candidate.text)
            append(':')
            append(candidate.comment.orEmpty())
            append('\u0001')
        }
    }

    private fun invalidateExpandedCandidateItemsCache() {
        expandedCandidateItemsCacheSignature = ""
        expandedCandidateItemsCache = emptyList()
    }

    private fun clipboardPanelItems(): List<CandidateDrawItem> {
        return clipboardItems.mapIndexed { index, text ->
            val previewEnd = text.offsetByCodePoints(0, minOf(120, text.codePointCount(0, text.length)))
            CandidateDrawItem(
                index = -1000 - index,
                label = "剪贴 ${index + 1}",
                text = text.substring(0, previewEnd),
                command = KeyCommand.directInput(text),
                clipboardText = text,
            )
        }
    }

    private fun panelItems(vararg items: PanelItem): List<CandidateDrawItem> {
        return items.mapIndexed { index, item ->
            CandidateDrawItem(
                index = -1000 - index,
                label = item.label,
                text = item.text,
                comment = item.comment,
                command = item.command,
            )
        }
    }

    private fun candidateWidth(item: CandidateDrawItem): Float {
        val cacheKey = candidateWidthCacheKey(item)
        candidateWidthCache[cacheKey]?.let { return it }
        textPaint.textSize = sp(candidateLabelSizeSp())
        val labelWidth = item.label.takeIf { it.isNotBlank() }?.let { textPaint.measureText(it) } ?: 0f
        textPaint.textSize = sp(candidateTextSizeSp())
        val textWidth = textPaint.measureText(item.text)
        textPaint.textSize = sp(candidateCommentSizeSp())
        val commentWidth = item.comment?.takeIf { it.isNotBlank() }?.let { textPaint.measureText(it) } ?: 0f
        val inlineGap = dp(candidateInlineGapDp())
        var segmentCount = 0
        if (labelWidth > 0f) segmentCount++
        if (textWidth > 0f) segmentCount++
        if (commentWidth > 0f) segmentCount++
        val textGaps = segmentCount.minus(1).coerceAtLeast(0).toFloat() * inlineGap
        val width = labelWidth + textWidth + commentWidth + textGaps + dp(candidatePaddingXDp() * 2)
        candidateWidthCache[cacheKey] = width
        return width
    }

    private fun candidateWidthCacheKey(item: CandidateDrawItem): String {
        return buildString {
            append(item.label)
            append('\u0000')
            append(item.text)
            append('\u0000')
            append(item.comment.orEmpty())
            append('\u0000')
            append(item.selected)
        }
    }

    private fun candidateTextSizeSp(): Float = min(theme.fontSizeSp - 2f, 16f).coerceAtLeast(13f)

    private fun candidateLabelSizeSp(): Float = min(theme.labelSizeSp - 1f, 13f).coerceAtLeast(10f)

    private fun candidateCommentSizeSp(): Float = min(theme.commentSizeSp - 1f, 12f).coerceAtLeast(10f)

    private fun keyLabelSizeSp(label: String): Float {
        if (keyboardLayer.isSymbolLayer() &&
            !containsCjk(label) &&
            label.codePointCount(0, label.length) <= 2
        ) {
            return max(theme.fontSizeSp, 22f)
        }
        if (label.length > 2 || containsCjk(label)) {
            return min(min(theme.labelSizeSp, theme.fontSizeSp - 4f), 16f).coerceAtLeast(12f)
        }
        return theme.fontSizeSp
    }

    private fun keyHintSizeSp(): Float {
        return min(min(theme.commentSizeSp - 2f, keyLabelSizeSp("中") - 2f), 12f).coerceAtLeast(9f)
    }

    private fun containsCjk(text: String): Boolean {
        return text.any { char ->
            val block = Character.UnicodeBlock.of(char)
            block == Character.UnicodeBlock.CJK_UNIFIED_IDEOGRAPHS ||
                block == Character.UnicodeBlock.CJK_UNIFIED_IDEOGRAPHS_EXTENSION_A ||
                block == Character.UnicodeBlock.CJK_UNIFIED_IDEOGRAPHS_EXTENSION_B ||
                block == Character.UnicodeBlock.CJK_COMPATIBILITY_IDEOGRAPHS
        }
    }

    private fun candidatePaddingXDp(): Float = min(theme.candidatePaddingXDp, 9f).coerceAtLeast(7f)

    private fun candidateInlineGapDp(): Float = min(theme.candidateInlineGapDp, 4f).coerceAtLeast(2f)

    private fun candidateCornerRadiusDp(): Float = min(theme.keyCornerRadiusDp, 8f).coerceAtLeast(6f)

    private fun keyCornerRadiusDp(): Float = min(theme.keyCornerRadiusDp + 1f, 10f).coerceAtLeast(7f)

    private fun drawCandidateOption(canvas: Canvas, item: CandidateDrawItem, rect: RectF) {
        if (item.style == PanelItemStyle.SECTION) {
            drawRimeSectionHeader(canvas, item, rect)
            return
        }
        val radius = dp(candidateCornerRadiusDp())
        if (item.command != null || item.selected) {
            drawSurfaceShadow(canvas, rect, pressed = false)
        }
        paint.style = Paint.Style.FILL
        paint.color = if (item.selected) {
            theme.candidateSelectedBackground.toArgb()
        } else {
            theme.keyBackground.toArgb()
        }
        canvas.drawRoundRect(rect, radius, radius, paint)

        val borderWidth = if (item.selected) {
            dp(theme.candidateBorderWidthDp.coerceAtLeast(1f))
        } else {
            dp(theme.candidateBorderWidthDp)
        }
        if (borderWidth > 0f) {
            paint.style = Paint.Style.STROKE
            paint.strokeWidth = borderWidth
            paint.color = if (item.selected) {
                theme.candidateSelectedBorderColor.toArgb()
            } else {
                theme.candidateBorderColor.toArgb()
            }
            canvas.drawRoundRect(rect, radius, radius, paint)
        }

        when (item.style) {
            PanelItemStyle.SCHEMA -> drawRimeSchemaRow(canvas, item, rect)
            PanelItemStyle.OPTION -> drawRimeOptionPill(canvas, item, rect)
            else -> when (panelColumns(if (functionPanelActive) functionPanelMode else FunctionPanelMode.RIME)) {
                4 -> drawCandidateGridCell(canvas, item, rect)
                1 -> drawClipboardCandidateRow(canvas, item, rect)
                else -> drawInlineCandidateOption(canvas, item, rect)
            }
        }
    }

    private fun drawRimeSectionHeader(canvas: Canvas, item: CandidateDrawItem, rect: RectF) {
        val labelX = rect.left + dp(4f)
        textPaint.textAlign = Paint.Align.LEFT
        textPaint.textSize = sp(candidateLabelSizeSp())
        textPaint.color = theme.commentColor.toArgb()
        canvas.drawText(item.label, labelX, rect.centerY() + textBaselineOffset(textPaint), textPaint)
        val lineLeft = labelX + textPaint.measureText(item.label) + dp(10f)
        if (lineLeft < rect.right) {
            paint.style = Paint.Style.STROKE
            paint.strokeWidth = max(1f, dp(theme.candidateBorderWidthDp))
            paint.color = theme.panelBorder.toArgb()
            canvas.drawLine(lineLeft, rect.centerY(), rect.right, rect.centerY(), paint)
        }
    }

    private fun drawRimeSchemaRow(canvas: Canvas, item: CandidateDrawItem, rect: RectF) {
        val radioCenterX = rect.left + dp(18f)
        paint.style = Paint.Style.STROKE
        paint.strokeWidth = dp(if (item.selected) 2f else 1.4f)
        paint.color = if (item.selected) {
            theme.candidateSelectedForeground.toArgb()
        } else {
            theme.commentColor.toArgb()
        }
        canvas.drawCircle(radioCenterX, rect.centerY(), dp(7f), paint)
        if (item.selected) {
            paint.style = Paint.Style.FILL
            canvas.drawCircle(radioCenterX, rect.centerY(), dp(3.5f), paint)
        }

        val textLeft = rect.left + dp(36f)
        val maxWidth = (rect.right - dp(10f) - textLeft).coerceAtLeast(0f)
        textPaint.textAlign = Paint.Align.LEFT
        textPaint.textSize = sp(candidateTextSizeSp())
        textPaint.color = if (item.selected) theme.candidateSelectedForeground.toArgb() else theme.keyForeground.toArgb()
        val name = TextUtils.ellipsize(item.label, textPaint, maxWidth, TextUtils.TruncateAt.END).toString()
        canvas.drawText(name, textLeft, rect.centerY() - dp(7f) + textBaselineOffset(textPaint), textPaint)
        textPaint.textSize = sp(candidateCommentSizeSp())
        textPaint.color = if (item.selected) theme.selectedCommentColor.toArgb() else theme.commentColor.toArgb()
        val id = TextUtils.ellipsize(item.text, textPaint, maxWidth, TextUtils.TruncateAt.END).toString()
        canvas.drawText(id, textLeft, rect.centerY() + dp(9f) + textBaselineOffset(textPaint), textPaint)
    }

    private fun drawRimeOptionPill(canvas: Canvas, item: CandidateDrawItem, rect: RectF) {
        val statusWidth = dp(42f)
        val statusHeight = dp(22f)
        val statusRect = RectF(
            rect.right - dp(8f) - statusWidth,
            rect.centerY() - statusHeight / 2f,
            rect.right - dp(8f),
            rect.centerY() + statusHeight / 2f,
        )
        val textLeft = rect.left + dp(10f)
        val maxWidth = (statusRect.left - dp(8f) - textLeft).coerceAtLeast(0f)
        textPaint.textAlign = Paint.Align.LEFT
        textPaint.textSize = sp(candidateLabelSizeSp())
        textPaint.color = if (item.selected) theme.candidateSelectedForeground.toArgb() else theme.keyForeground.toArgb()
        val label = TextUtils.ellipsize(item.label, textPaint, maxWidth, TextUtils.TruncateAt.END).toString()
        canvas.drawText(label, textLeft, rect.centerY() - dp(7f) + textBaselineOffset(textPaint), textPaint)
        textPaint.textSize = sp(candidateCommentSizeSp())
        textPaint.color = if (item.selected) theme.selectedCommentColor.toArgb() else theme.commentColor.toArgb()
        val stateLabel = TextUtils.ellipsize(item.text, textPaint, maxWidth, TextUtils.TruncateAt.END).toString()
        canvas.drawText(stateLabel, textLeft, rect.centerY() + dp(9f) + textBaselineOffset(textPaint), textPaint)

        paint.style = Paint.Style.FILL
        paint.color = if (item.selected) theme.candidateSelectedForeground.toArgb() else theme.candidateBorderColor.toArgb()
        canvas.drawRoundRect(statusRect, statusHeight / 2f, statusHeight / 2f, paint)
        textPaint.textAlign = Paint.Align.CENTER
        textPaint.textSize = sp(10f)
        textPaint.color = if (item.selected) theme.candidateSelectedBackground.toArgb() else theme.commentColor.toArgb()
        canvas.drawText(if (item.selected) "ON" else "OFF", statusRect.centerX(), statusRect.centerY() + textBaselineOffset(textPaint), textPaint)
    }

    private fun drawCandidateGridCell(canvas: Canvas, item: CandidateDrawItem, rect: RectF) {
        val maxWidth = (rect.width() - dp(12f)).coerceAtLeast(0f)
        val labelY = rect.centerY() - dp(10f)
        val captionY = rect.centerY() + dp(10f)
        textPaint.textAlign = Paint.Align.CENTER
        textPaint.textSize = sp(candidateTextSizeSp())
        textPaint.color = if (item.selected) theme.candidateSelectedForeground.toArgb() else theme.keyForeground.toArgb()
        val label = TextUtils.ellipsize(item.label, textPaint, maxWidth, TextUtils.TruncateAt.END).toString()
        canvas.drawText(label, rect.centerX(), labelY + textBaselineOffset(textPaint), textPaint)

        val caption = listOfNotNull(item.text.takeIf { it.isNotBlank() }, item.comment?.takeIf { it.isNotBlank() })
            .joinToString(" ")
        textPaint.textSize = sp(candidateCommentSizeSp())
        textPaint.color = if (item.selected) theme.selectedCommentColor.toArgb() else theme.commentColor.toArgb()
        val ellipsizedCaption = TextUtils.ellipsize(caption, textPaint, maxWidth, TextUtils.TruncateAt.END).toString()
        canvas.drawText(ellipsizedCaption, rect.centerX(), captionY + textBaselineOffset(textPaint), textPaint)
    }

    private fun drawClipboardCandidateRow(canvas: Canvas, item: CandidateDrawItem, rect: RectF) {
        val padding = dp(candidatePaddingXDp())
        val inlineGap = dp(candidateInlineGapDp())
        val deleteWidth = dp(config.clipboardDeleteHitWidthDp)
        val deleteLeft = rect.right - deleteWidth
        val centerY = rect.centerY()
        var textX = rect.left + padding
        textPaint.textAlign = Paint.Align.LEFT
        if (item.label.isNotBlank()) {
            textPaint.textSize = sp(candidateLabelSizeSp())
            textPaint.color = if (item.selected) theme.selectedLabelColor.toArgb() else theme.labelColor.toArgb()
            canvas.drawText(item.label, textX, centerY + textBaselineOffset(textPaint), textPaint)
            textX += textPaint.measureText(item.label) + inlineGap
        }
        textPaint.textSize = sp(candidateTextSizeSp())
        textPaint.color = if (item.selected) theme.candidateSelectedForeground.toArgb() else theme.keyForeground.toArgb()
        val maxWidth = (deleteLeft - padding - textX).coerceAtLeast(0f)
        val preview = TextUtils.ellipsize(item.text, textPaint, maxWidth, TextUtils.TruncateAt.END).toString()
        canvas.drawText(preview, textX, centerY + textBaselineOffset(textPaint), textPaint)

        paint.style = Paint.Style.STROKE
        paint.strokeWidth = max(1f, dp(0.7f))
        paint.color = theme.candidateBorderColor.toArgb()
        canvas.drawLine(deleteLeft, rect.top + dp(7f), deleteLeft, rect.bottom - dp(7f), paint)
        textPaint.textAlign = Paint.Align.CENTER
        textPaint.textSize = sp(18f)
        textPaint.color = theme.commentColor.toArgb()
        canvas.drawText("✕", deleteLeft + deleteWidth / 2f, centerY + textBaselineOffset(textPaint), textPaint)
    }

    private fun drawInlineCandidateOption(canvas: Canvas, item: CandidateDrawItem, rect: RectF) {
        textPaint.textAlign = Paint.Align.LEFT
        var textX = rect.left + dp(candidatePaddingXDp())
        val inlineGap = dp(candidateInlineGapDp())
        canvas.save()
        canvas.clipRect(rect.left + dp(4f), rect.top, rect.right - dp(4f), rect.bottom)
        if (item.label.isNotBlank()) {
            textPaint.textSize = sp(candidateLabelSizeSp())
            textPaint.color = if (item.selected) theme.selectedLabelColor.toArgb() else theme.labelColor.toArgb()
            canvas.drawText(item.label, textX, rect.centerY() + textBaselineOffset(textPaint), textPaint)
            textX += textPaint.measureText(item.label) + inlineGap
        }
        textPaint.textSize = sp(candidateTextSizeSp())
        textPaint.color = if (item.selected) theme.candidateSelectedForeground.toArgb() else theme.keyForeground.toArgb()
        canvas.drawText(item.text, textX, rect.centerY() + textBaselineOffset(textPaint), textPaint)
        textX += textPaint.measureText(item.text) + inlineGap
        item.comment?.takeIf { it.isNotBlank() }?.let { comment ->
            textPaint.textSize = sp(candidateCommentSizeSp())
            textPaint.color = if (item.selected) theme.selectedCommentColor.toArgb() else theme.commentColor.toArgb()
            canvas.drawText(comment, textX, rect.centerY() + textBaselineOffset(textPaint), textPaint)
        }
        canvas.restore()
    }

    private fun drawToolbar(
        canvas: Canvas,
        barHeight: Float,
        leftPadding: Float,
        actions: List<ToolbarAction> = toolbarActions(),
        showLogo: Boolean = true,
    ) {
        val preferredWidths = actions.map(::toolbarChipWidth)
        val minimumWidths = actions.map(::minimumToolbarChipWidth)
        val availableWidth = (width - leftPadding * 2f).coerceAtLeast(0f)
        val preferredTotal = preferredWidths.sum() +
            dp(6f) * (actions.size - 1).coerceAtLeast(0) +
            if (showLogo) dp(8f) + dp(30f) else 0f
        val compression = if (preferredTotal > 0f) {
            (availableWidth / preferredTotal).coerceIn(0.6f, 1f)
        } else {
            1f
        }
        val logoSize = if (showLogo) max(dp(18f), dp(30f) * compression) else 0f
        val logoGap = if (showLogo) max(dp(2f), dp(8f) * compression) else 0f
        val gap = max(dp(2f), dp(6f) * compression)
        val logoLeft = width - leftPadding - logoSize
        val maxRight = if (showLogo) logoLeft - logoGap else width - leftPadding
        val rects = mutableListOf<ToolbarRect>()
        val chipHeight = minOf(dp(34f), barHeight - dp(12f))
        val widthBudget = (maxRight - leftPadding - gap * (actions.size - 1).coerceAtLeast(0))
            .coerceAtLeast(0f)
        val preferredWidthTotal = preferredWidths.sum()
        val minimumWidthTotal = minimumWidths.sum()
        val chipWidths = when {
            preferredWidthTotal <= widthBudget -> preferredWidths
            minimumWidthTotal >= widthBudget && minimumWidthTotal > 0f -> {
                val scale = widthBudget / minimumWidthTotal
                minimumWidths.map { it * scale }
            }
            else -> {
                val flexibleWidth = preferredWidthTotal - minimumWidthTotal
                val progress = if (flexibleWidth > 0f) {
                    ((widthBudget - minimumWidthTotal) / flexibleWidth).coerceIn(0f, 1f)
                } else {
                    0f
                }
                preferredWidths.mapIndexed { index, preferred ->
                    minimumWidths[index] + (preferred - minimumWidths[index]) * progress
                }
            }
        }
        var x = leftPadding
        val top = (barHeight - chipHeight) / 2f

        for ((index, action) in actions.withIndex()) {
            val chipWidth = chipWidths[index]
            val rect = RectF(x, top, x + chipWidth, top + chipHeight)
            val toolbarRect = ToolbarRect(
                action.label,
                action.command,
                rect,
                action.selected,
                action.secondaryLabel,
                action.icon,
                action.longPressCommand,
            )
            drawToolbarChip(canvas, toolbarRect)
            rects.add(toolbarRect)
            x = rect.right + gap
        }

        toolbarRects = rects
        if (showLogo) {
            drawKeytaoLogo(canvas, barHeight, leftPadding, logoSize)
        }
    }

    private fun drawClipboardSuggestionBar(canvas: Canvas, barHeight: Float, leftPadding: Float) {
        val text = recentClipboardSuggestion ?: return
        val chipHeight = minOf(dp(36f), barHeight - dp(10f))
        val top = (barHeight - chipHeight) / 2f
        val gap = dp(6f)
        val backWidth = dp(72f)
        val back = ToolbarRect(
            "返回",
            KeyCommand.panel("dismissClipboard"),
            RectF(leftPadding, top, leftPadding + backWidth, top + chipHeight),
        )
        val paste = ToolbarRect(
            "粘贴",
            KeyCommand.edit("pasteText", text),
            RectF(back.rect.right + gap, top, width - leftPadding, top + chipHeight),
            secondaryLabel = text,
        )
        toolbarRects = listOf(back, paste)
        drawToolbarChip(canvas, back, forceAccent = true)
        drawClipboardPasteChip(canvas, paste, text)
    }

    private fun drawClipboardPasteChip(canvas: Canvas, item: ToolbarRect, preview: String) {
        val pressed = isToolbarPressed(item)
        drawSurfaceShadow(canvas, item.rect, pressed)
        paint.style = Paint.Style.FILL
        paint.color = toolbarBackgroundColor(item, pressed, forceAccent = true)
        canvas.drawRoundRect(item.rect, dp(keyCornerRadiusDp()), dp(keyCornerRadiusDp()), paint)

        val padding = dp(13f)
        val inlineGap = dp(8f)
        textPaint.textAlign = Paint.Align.LEFT
        textPaint.textSize = sp(theme.labelSizeSp)
        val labelWidth = textPaint.measureText(item.label)
        val labelX = item.rect.left + padding
        val textY = item.rect.centerY() + textBaselineOffset(textPaint)

        canvas.save()
        canvas.clipRect(item.rect.left + padding, item.rect.top, item.rect.right - padding, item.rect.bottom)
        textPaint.color = if (pressed) theme.keySelectedForeground.toArgb() else theme.selectedLabelColor.toArgb()
        canvas.drawText(item.label, labelX, textY, textPaint)

        textPaint.textSize = sp(theme.commentSizeSp)
        textPaint.color = if (pressed) theme.keySelectedForeground.toArgb() else theme.commentColor.toArgb()
        canvas.drawText(
            preview,
            labelX + labelWidth + inlineGap,
            item.rect.centerY() + textBaselineOffset(textPaint),
            textPaint,
        )
        canvas.restore()
    }

    private fun toolbarChipWidth(action: ToolbarAction): Float {
        if (action.icon != null && action.secondaryLabel.isNullOrBlank()) {
            return dp(46f)
        }
        textPaint.textSize = sp(theme.labelSizeSp)
        val labelWidth = textPaint.measureText(action.label)
        val secondaryWidth = action.secondaryLabel
            ?.takeIf { it.isNotBlank() }
            ?.let {
                textPaint.textSize = sp(theme.commentSizeSp)
                textPaint.measureText(it)
            }
            ?: 0f
        val inlineGap = if (secondaryWidth > 0f) dp(5f) else 0f
        return (labelWidth + inlineGap + secondaryWidth + dp(22f)).coerceAtLeast(
            if (secondaryWidth > 0f) dp(58f) else dp(48f)
        )
    }

    private fun minimumToolbarChipWidth(action: ToolbarAction): Float {
        return when {
            action.icon != null && action.secondaryLabel.isNullOrBlank() -> dp(28f)
            !action.secondaryLabel.isNullOrBlank() -> dp(46f)
            else -> dp(38f)
        }
    }

    private fun drawFunctionPanelBar(canvas: Canvas, barHeight: Float, leftPadding: Float) {
        val chipHeight = minOf(dp(34f), barHeight - dp(12f))
        val top = (barHeight - chipHeight) / 2f
        val backAction = ToolbarAction("返回", KeyCommand.panel("close"), icon = ToolbarIcon.BACK)
        val pasteAction = ToolbarAction("粘贴", KeyCommand.edit("paste"))
        val clearAction = ToolbarAction(
            if (clipboardClearConfirmationPending) "确认清空" else "清空",
            KeyCommand.panel("clearClipboardHistory"),
        )
        val settingsAction = ToolbarAction("设置", KeyCommand(KeyCommandTypes.OPEN_PAGE, "settings"), icon = ToolbarIcon.SETTINGS)
        val backWidth = toolbarChipWidth(backAction)
        val pasteWidth = toolbarChipWidth(pasteAction)
        val clearWidth = toolbarChipWidth(clearAction)
        val settingsWidth = toolbarChipWidth(settingsAction)
        val back = ToolbarRect(
            backAction.label,
            backAction.command,
            RectF(leftPadding, top, leftPadding + backWidth, top + chipHeight),
            icon = backAction.icon,
        )
        val settings = ToolbarRect(
            settingsAction.label,
            settingsAction.command,
            RectF(width - leftPadding - settingsWidth, top, width - leftPadding, top + chipHeight),
            icon = settingsAction.icon,
        )
        val paste = ToolbarRect(
            pasteAction.label,
            pasteAction.command,
            RectF(back.rect.right + dp(6f), top, back.rect.right + dp(6f) + pasteWidth, top + chipHeight),
        )
        val clear = ToolbarRect(
            clearAction.label,
            clearAction.command,
            RectF(paste.rect.right + dp(6f), top, paste.rect.right + dp(6f) + clearWidth, top + chipHeight),
        )
        val showsClear = functionPanelMode == FunctionPanelMode.CLIPBOARD && clipboardItems.isNotEmpty()
        toolbarRects = when {
            showsClear -> listOf(back, paste, clear, settings)
            functionPanelMode == FunctionPanelMode.CLIPBOARD -> listOf(back, paste, settings)
            else -> listOf(back, settings)
        }
        drawToolbarChip(canvas, back)
        if (functionPanelMode == FunctionPanelMode.CLIPBOARD) {
            drawToolbarChip(canvas, paste)
            if (showsClear) {
                drawToolbarChip(canvas, clear)
            }
        }
        drawToolbarChip(canvas, settings)

        if (!showsClear) {
            textPaint.textAlign = Paint.Align.CENTER
            textPaint.textSize = sp(theme.labelSizeSp)
            textPaint.color = theme.commentColor.toArgb()
            canvas.drawText(functionPanelTitle(), width / 2f, barHeight / 2f + textBaselineOffset(textPaint), textPaint)
        }

        if (expandedCandidatesLoading || clipboardItemsLoading || rimeOptionsLoading) {
            paint.style = Paint.Style.FILL
            paint.color = theme.selectedLabelColor.toArgb()
            val indicatorWidth = dp(44f)
            val indicatorLeft = (width - indicatorWidth) / 2f
            canvas.drawRoundRect(
                RectF(indicatorLeft, barHeight - dp(3f), indicatorLeft + indicatorWidth, barHeight - dp(1f)),
                dp(1f),
                dp(1f),
                paint,
            )
        }
    }

    private fun drawToolbarChip(canvas: Canvas, item: ToolbarRect, forceAccent: Boolean = false) {
        val pressed = isToolbarPressed(item)
        drawSurfaceShadow(canvas, item.rect, pressed)
        paint.style = Paint.Style.FILL
        paint.color = toolbarBackgroundColor(item, pressed, forceAccent)
        canvas.drawRoundRect(item.rect, dp(keyCornerRadiusDp()), dp(keyCornerRadiusDp()), paint)

        if (item.selected) {
            paint.style = Paint.Style.STROKE
            paint.strokeWidth = dp(theme.candidateBorderWidthDp.coerceAtLeast(1f))
            paint.color = theme.candidateSelectedBorderColor.toArgb()
            canvas.drawRoundRect(item.rect, dp(keyCornerRadiusDp()), dp(keyCornerRadiusDp()), paint)
        }

        textPaint.textAlign = Paint.Align.CENTER
        val secondary = item.secondaryLabel?.takeIf { it.isNotBlank() }
        if (secondary == null) {
            val color = when {
                pressed -> theme.keySelectedForeground.toArgb()
                item.selected -> theme.candidateSelectedForeground.toArgb()
                else -> theme.keyForeground.toArgb()
            }
            if (item.icon != null) {
                drawToolbarIcon(canvas, item.icon, item.rect, color)
            } else {
                textPaint.textSize = sp(theme.labelSizeSp)
                textPaint.color = color
                canvas.drawText(item.label, item.rect.centerX(), item.rect.centerY() + textBaselineOffset(textPaint), textPaint)
            }
        } else {
            textPaint.textSize = sp(theme.labelSizeSp)
            val primaryWidth = textPaint.measureText(item.label)
            textPaint.textSize = sp(theme.commentSizeSp)
            val secondaryWidth = textPaint.measureText(secondary)
            val groupWidth = primaryWidth + dp(5f) + secondaryWidth
            val primaryX = item.rect.centerX() - groupWidth / 2f + primaryWidth / 2f
            val secondaryX = primaryX + primaryWidth / 2f + dp(5f) + secondaryWidth / 2f

            textPaint.textSize = sp(theme.labelSizeSp)
            textPaint.color = if (pressed) theme.keySelectedForeground.toArgb() else theme.keyForeground.toArgb()
            canvas.drawText(item.label, primaryX, item.rect.centerY() + textBaselineOffset(textPaint), textPaint)

            textPaint.textSize = sp(theme.commentSizeSp)
            textPaint.color = if (pressed) theme.keySelectedForeground.toArgb() else theme.commentColor.toArgb()
            canvas.drawText(secondary, secondaryX, item.rect.centerY() + textBaselineOffset(textPaint), textPaint)
        }
    }

    private fun drawToolbarIcon(canvas: Canvas, icon: ToolbarIcon, rect: RectF, color: Int) {
        val size = minOf(dp(21f), rect.width() - dp(16f), rect.height() - dp(11f)).coerceAtLeast(dp(14f))
        val iconRect = RectF(
            rect.centerX() - size / 2f,
            rect.centerY() - size / 2f,
            rect.centerX() + size / 2f,
            rect.centerY() + size / 2f,
        )
        val oldStyle = paint.style
        val oldColor = paint.color
        val oldStrokeWidth = paint.strokeWidth
        val oldStrokeCap = paint.strokeCap
        val oldStrokeJoin = paint.strokeJoin

        paint.color = color
        paint.strokeWidth = max(dp(1.7f), size * 0.095f)
        paint.strokeCap = Paint.Cap.ROUND
        paint.strokeJoin = Paint.Join.ROUND

        when (icon) {
            ToolbarIcon.FUNCTION -> drawGridToolbarIcon(canvas, iconRect)
            ToolbarIcon.SELECTION -> drawSelectionToolbarIcon(canvas, iconRect)
            ToolbarIcon.CLIPBOARD -> drawClipboardToolbarIcon(canvas, iconRect)
            ToolbarIcon.EMOJI -> drawEmojiToolbarIcon(canvas, iconRect)
            ToolbarIcon.GLOBE -> drawGlobeToolbarIcon(canvas, iconRect)
            ToolbarIcon.ONE_HANDED -> drawOneHandedToolbarIcon(canvas, iconRect)
            ToolbarIcon.FLOATING -> drawFloatingToolbarIcon(canvas, iconRect)
            ToolbarIcon.BACK -> drawBackToolbarIcon(canvas, iconRect)
            ToolbarIcon.SETTINGS -> drawSettingsToolbarIcon(canvas, iconRect)
        }

        paint.style = oldStyle
        paint.color = oldColor
        paint.strokeWidth = oldStrokeWidth
        paint.strokeCap = oldStrokeCap
        paint.strokeJoin = oldStrokeJoin
    }

    private fun drawGridToolbarIcon(canvas: Canvas, rect: RectF) {
        paint.style = Paint.Style.STROKE
        val cell = rect.width() * 0.34f
        val gap = rect.width() - cell * 2f
        for (row in 0 until 2) {
            for (column in 0 until 2) {
                val left = rect.left + column * (cell + gap)
                val top = rect.top + row * (cell + gap)
                canvas.drawRoundRect(RectF(left, top, left + cell, top + cell), cell * 0.22f, cell * 0.22f, paint)
            }
        }
    }

    private fun drawGlobeToolbarIcon(canvas: Canvas, rect: RectF) {
        paint.style = Paint.Style.STROKE
        canvas.drawOval(rect, paint)
        canvas.drawOval(
            RectF(
                rect.left + rect.width() * 0.28f,
                rect.top,
                rect.right - rect.width() * 0.28f,
                rect.bottom,
            ),
            paint,
        )
        canvas.drawLine(
            rect.left + rect.width() * 0.08f,
            rect.centerY(),
            rect.right - rect.width() * 0.08f,
            rect.centerY(),
            paint,
        )
    }

    private fun drawFloatingToolbarIcon(canvas: Canvas, rect: RectF) {
        paint.style = Paint.Style.STROKE
        val window = RectF(
            rect.left + rect.width() * 0.10f,
            rect.top + rect.height() * 0.16f,
            rect.right - rect.width() * 0.10f,
            rect.bottom - rect.height() * 0.16f,
        )
        canvas.drawRoundRect(window, rect.width() * 0.10f, rect.width() * 0.10f, paint)
        val arrow = Path().apply {
            moveTo(rect.left + rect.width() * 0.30f, rect.top + rect.height() * 0.32f)
            lineTo(rect.left + rect.width() * 0.18f, rect.top + rect.height() * 0.32f)
            lineTo(rect.left + rect.width() * 0.18f, rect.top + rect.height() * 0.47f)
            moveTo(rect.left + rect.width() * 0.18f, rect.top + rect.height() * 0.32f)
            lineTo(rect.left + rect.width() * 0.38f, rect.top + rect.height() * 0.52f)
            moveTo(rect.right - rect.width() * 0.30f, rect.bottom - rect.height() * 0.32f)
            lineTo(rect.right - rect.width() * 0.18f, rect.bottom - rect.height() * 0.32f)
            lineTo(rect.right - rect.width() * 0.18f, rect.bottom - rect.height() * 0.47f)
            moveTo(rect.right - rect.width() * 0.18f, rect.bottom - rect.height() * 0.32f)
            lineTo(rect.right - rect.width() * 0.38f, rect.bottom - rect.height() * 0.52f)
        }
        canvas.drawPath(arrow, paint)
    }

    private fun drawOneHandedToolbarIcon(canvas: Canvas, rect: RectF) {
        paint.style = Paint.Style.STROKE
        val window = RectF(
            rect.left + rect.width() * 0.08f,
            rect.top + rect.height() * 0.08f,
            rect.right - rect.width() * 0.08f,
            rect.bottom - rect.height() * 0.08f,
        )
        canvas.drawRoundRect(window, rect.width() * 0.10f, rect.width() * 0.10f, paint)
        val keyboardWidth = window.width() * 0.60f
        val keyboardRect = if (oneHandedSide == KeyboardSide.LEFT) {
            RectF(window.left, window.top, window.left + keyboardWidth, window.bottom)
        } else {
            RectF(window.right - keyboardWidth, window.top, window.right, window.bottom)
        }
        canvas.drawRoundRect(keyboardRect, rect.width() * 0.08f, rect.width() * 0.08f, paint)
        val separatorX = if (oneHandedSide == KeyboardSide.LEFT) keyboardRect.right else keyboardRect.left
        canvas.drawLine(separatorX, keyboardRect.top, separatorX, keyboardRect.bottom, paint)
    }

    private fun drawSelectionToolbarIcon(canvas: Canvas, rect: RectF) {
        paint.style = Paint.Style.STROKE
        val path = Path()
        path.reset()
        path.moveTo(rect.left + rect.width() * 0.24f, rect.top + rect.height() * 0.12f)
        path.lineTo(rect.left + rect.width() * 0.24f, rect.bottom - rect.height() * 0.14f)
        path.lineTo(rect.left + rect.width() * 0.42f, rect.top + rect.height() * 0.66f)
        path.lineTo(rect.left + rect.width() * 0.54f, rect.bottom - rect.height() * 0.10f)
        path.lineTo(rect.left + rect.width() * 0.68f, rect.bottom - rect.height() * 0.18f)
        path.lineTo(rect.left + rect.width() * 0.56f, rect.top + rect.height() * 0.58f)
        path.lineTo(rect.right - rect.width() * 0.20f, rect.top + rect.height() * 0.58f)
        path.close()
        canvas.drawPath(path, paint)
    }

    private fun drawClipboardToolbarIcon(canvas: Canvas, rect: RectF) {
        paint.style = Paint.Style.STROKE
        val body = RectF(rect.left + rect.width() * 0.2f, rect.top + rect.height() * 0.16f, rect.right - rect.width() * 0.2f, rect.bottom - rect.height() * 0.12f)
        canvas.drawRoundRect(body, rect.width() * 0.1f, rect.width() * 0.1f, paint)
        val clip = RectF(rect.left + rect.width() * 0.36f, rect.top + rect.height() * 0.08f, rect.right - rect.width() * 0.36f, rect.top + rect.height() * 0.26f)
        canvas.drawRoundRect(clip, rect.width() * 0.06f, rect.width() * 0.06f, paint)
        canvas.drawLine(body.left + body.width() * 0.22f, body.centerY(), body.right - body.width() * 0.22f, body.centerY(), paint)
    }

    private fun drawEmojiToolbarIcon(canvas: Canvas, rect: RectF) {
        paint.style = Paint.Style.STROKE
        canvas.drawOval(RectF(rect.left + rect.width() * 0.08f, rect.top + rect.height() * 0.08f, rect.right - rect.width() * 0.08f, rect.bottom - rect.height() * 0.08f), paint)
        paint.style = Paint.Style.FILL
        val eye = rect.width() * 0.07f
        canvas.drawOval(RectF(rect.left + rect.width() * 0.32f, rect.top + rect.height() * 0.36f, rect.left + rect.width() * 0.32f + eye, rect.top + rect.height() * 0.36f + eye), paint)
        canvas.drawOval(RectF(rect.right - rect.width() * 0.39f, rect.top + rect.height() * 0.36f, rect.right - rect.width() * 0.39f + eye, rect.top + rect.height() * 0.36f + eye), paint)
        paint.style = Paint.Style.STROKE
        val smile = Path()
        smile.moveTo(rect.left + rect.width() * 0.32f, rect.top + rect.height() * 0.62f)
        smile.quadTo(
            rect.centerX(),
            rect.bottom - rect.height() * 0.18f,
            rect.right - rect.width() * 0.32f,
            rect.top + rect.height() * 0.62f,
        )
        canvas.drawPath(smile, paint)
    }

    private fun drawBackToolbarIcon(canvas: Canvas, rect: RectF) {
        paint.style = Paint.Style.STROKE
        canvas.drawLine(rect.right - rect.width() * 0.15f, rect.centerY(), rect.left + rect.width() * 0.18f, rect.centerY(), paint)
        canvas.drawLine(rect.left + rect.width() * 0.18f, rect.centerY(), rect.left + rect.width() * 0.42f, rect.top + rect.height() * 0.26f, paint)
        canvas.drawLine(rect.left + rect.width() * 0.18f, rect.centerY(), rect.left + rect.width() * 0.42f, rect.bottom - rect.height() * 0.26f, paint)
    }

    private fun drawSettingsToolbarIcon(canvas: Canvas, rect: RectF) {
        paint.style = Paint.Style.STROKE
        val rows = listOf(0.28f to 0.65f, 0.5f to 0.34f, 0.72f to 0.58f)
        for ((yRatio, knobRatio) in rows) {
            val y = rect.top + rect.height() * yRatio
            canvas.drawLine(rect.left + rect.width() * 0.14f, y, rect.right - rect.width() * 0.14f, y, paint)
            paint.style = Paint.Style.FILL
            canvas.drawCircle(rect.left + rect.width() * knobRatio, y, rect.width() * 0.085f, paint)
            paint.style = Paint.Style.STROKE
        }
    }

    private fun drawKeytaoLogo(canvas: Canvas, barHeight: Float, leftPadding: Float, size: Float = dp(30f)) {
        val left = width - leftPadding - size
        val top = (barHeight - size) / 2f
        val rect = RectF(left, top, left + size, top + size)
        val bitmap = logoBitmap
        if (bitmap != null) {
            paint.alpha = 215
            canvas.drawBitmap(bitmap, null, rect, paint)
            paint.alpha = 255
        } else {
            paint.style = Paint.Style.FILL
            paint.color = theme.selectedLabelColor.toArgb()
            canvas.drawOval(rect, paint)
            textPaint.textAlign = Paint.Align.CENTER
            textPaint.textSize = sp(theme.commentSizeSp)
            textPaint.color = theme.candidateSelectedForeground.toArgb()
            canvas.drawText("K", rect.centerX(), rect.centerY() + textBaselineOffset(textPaint), textPaint)
        }
    }

    private fun drawKeyboard(canvas: Canvas) {
        val layout = keyboardLayout()
        val top = keyboardTop()
        drawContentLayer(canvas, top) {
            if (usesCategorizedSymbolKeyboard(activeRows())) {
                canvas.save()
                canvas.clipRect(0f, keyboardScrollViewportTop, width.toFloat(), keyboardScrollViewportBottom)
                for ((index, keyRect) in layout.withIndex()) {
                    if (keyRect.sticky) continue
                    val pressed = activeKeyTouches.values.any { it.keyIndex == index }
                    drawKey(canvas, keyRect.spec, keyRect.rect, pressed, pressedStackIndexFor(index, keyRect))
                }
                canvas.restore()
                for ((index, keyRect) in layout.withIndex()) {
                    if (!keyRect.sticky) continue
                    val pressed = activeKeyTouches.values.any { it.keyIndex == index }
                    drawKey(canvas, keyRect.spec, keyRect.rect, pressed, pressedStackIndexFor(index, keyRect))
                }
            } else {
                for ((index, keyRect) in layout.withIndex()) {
                    val pressed = activeKeyTouches.values.any { it.keyIndex == index }
                    drawKey(canvas, keyRect.spec, keyRect.rect, pressed, pressedStackIndexFor(index, keyRect))
                }
            }
        }

        keyRects = layout
    }

    private fun drawKey(canvas: Canvas, key: KeySpec, rect: RectF, pressed: Boolean, pressedStackIndex: Int? = null) {
        if (key.stack.isNotEmpty()) {
            drawStackKey(canvas, key, rect, pressedStackIndex)
            return
        }

        val keyRect = RectF(rect)
        if (pressed) {
            keyRect.offset(0f, dp(1f))
        }
        val selected = isActiveKey(key)
        drawKeyShadow(canvas, keyRect, pressed)

        paint.style = Paint.Style.FILL
        paint.color = keySurfaceColor(key, pressed)
        canvas.drawRoundRect(keyRect, dp(keyCornerRadiusDp()), dp(keyCornerRadiusDp()), paint)
        drawKeyOutline(canvas, key, keyRect, pressed)
        drawShiftStateDecoration(canvas, key, keyRect)

        val label = displayLabel(key)
        textPaint.textAlign = Paint.Align.CENTER
        var labelSize = sp(keyLabelSizeSp(label))
        textPaint.textSize = labelSize
        val maxLabelWidth = keyRect.width() - dp(10f)
        while (labelSize > sp(12f) && textPaint.measureText(label) > maxLabelWidth) {
            labelSize -= dp(1f)
            textPaint.textSize = labelSize
        }
        textPaint.color = keyForegroundColor(key, selected || pressed)
        canvas.drawText(label, keyRect.centerX(), keyRect.centerY() + textBaselineOffset(textPaint), textPaint)

        key.hint?.let { hint ->
            textPaint.textAlign = Paint.Align.RIGHT
            textPaint.textSize = sp(keyHintSizeSp())
            textPaint.color = theme.commentColor.toArgb()
            canvas.drawText(hint, keyRect.right - dp(7f), keyRect.top + dp(13f), textPaint)
        }
    }

    private fun drawStackKey(canvas: Canvas, key: KeySpec, rect: RectF, pressedStackIndex: Int?) {
        val stackRects = stackItemRects(rect, key.stack.size)
        for ((index, item) in key.stack.withIndex()) {
            val pressed = pressedStackIndex == index
            val keyRect = RectF(stackRects[index])
            if (pressed) {
                keyRect.offset(0f, dp(1f))
            }
            val selected = isActiveKey(key)
            drawKeyShadow(canvas, keyRect, pressed)

            paint.style = Paint.Style.FILL
            paint.color = keySurfaceColor(key, pressed)
            canvas.drawRoundRect(keyRect, dp(keyCornerRadiusDp()), dp(keyCornerRadiusDp()), paint)
            drawKeyOutline(canvas, key, keyRect, pressed)

            val label = stackLabelForMode(item)
            val maxLabelWidth = keyRect.width() - dp(10f)
            textPaint.textAlign = Paint.Align.CENTER
            textPaint.color = keyForegroundColor(key, selected || pressed)
            var labelSize = sp(keyLabelSizeSp(label))
            textPaint.textSize = labelSize
            while (labelSize > sp(12f) && textPaint.measureText(label) > maxLabelWidth) {
                labelSize -= dp(1f)
                textPaint.textSize = labelSize
            }
            canvas.drawText(label, keyRect.centerX(), keyRect.centerY() + textBaselineOffset(textPaint), textPaint)
        }
    }

    private fun keySurfaceColor(key: KeySpec, pressed: Boolean): Int {
        val active = isActiveKey(key)
        return when {
            pressed && active -> blendColor(
                theme.keyPressedBackground.toArgb(),
                theme.keySelectedBackground.toArgb(),
                0.52f,
            )
            pressed && isSoftAccentKey(key) -> blendColor(
                theme.keyPressedBackground.toArgb(),
                softenedAccentSurfaceColor(0.16f),
                0.72f,
            )
            pressed -> theme.keyPressedBackground.toArgb()
            active && shiftState == ShiftState.LOCKED -> theme.keySelectedBackground.toArgb()
            else -> keyBackgroundColor(key)
        }
    }

    private fun drawShiftStateDecoration(canvas: Canvas, key: KeySpec, rect: RectF) {
        if (key.action.type != KeyCommandTypes.SHIFT || shiftState == ShiftState.OFF) return
        paint.color = theme.candidateSelectedBorderColor.toArgb()
        if (shiftState == ShiftState.ONCE) {
            paint.style = Paint.Style.STROKE
            paint.strokeWidth = dp(2f)
            val inset = dp(1.5f)
            canvas.drawRoundRect(
                RectF(rect.left + inset, rect.top + inset, rect.right - inset, rect.bottom - inset),
                dp(keyCornerRadiusDp()),
                dp(keyCornerRadiusDp()),
                paint,
            )
        } else {
            paint.style = Paint.Style.FILL
            val barWidth = rect.width() * 0.36f
            val barHeight = dp(2.5f)
            canvas.drawRoundRect(
                RectF(
                    rect.centerX() - barWidth / 2f,
                    rect.bottom - dp(7f),
                    rect.centerX() + barWidth / 2f,
                    rect.bottom - dp(7f) + barHeight,
                ),
                barHeight / 2f,
                barHeight / 2f,
                paint,
            )
        }
    }

    private fun drawKeyFeedbackOverlays(canvas: Canvas) {
        for (touch in activeKeyTouches.values) {
            touch.alternatePanel?.let { drawAlternatePanel(canvas, it) }
                ?: previewText(touch)?.let { drawKeyPreview(canvas, touch.key, it) }
        }
    }

    private fun previewText(touch: KeyTouch): String? {
        if (!config.keyPreviewEnabled || touch.longPressConsumed || keyboardDragging || functionPanelActive) return null
        val deltaY = touch.currentY - touch.downY
        val key = if (abs(deltaY) >= dp(config.swipeThresholdDp)) touch.originKey else touch.key
        if (!isCharacterKey(key.spec)) return null
        if (abs(deltaY) < dp(config.swipeThresholdDp)) {
            val retained = RectF(key.rect).apply {
                val hysteresis = dp(KeytaoImeInteractionTuning.SLIDE_RETARGET_HYSTERESIS_DP)
                inset(-hysteresis, -hysteresis)
            }
            if (!retained.contains(touch.currentX, touch.currentY)) return null
        }
        val command = resolveCommand(key.spec, deltaY, key.rect, touch.currentY)
        if (command.type !in setOf(KeyCommandTypes.INPUT, KeyCommandTypes.DIRECT_INPUT, KeyCommandTypes.RIME_INPUT)) {
            return null
        }
        return command.value?.takeIf { it.isNotEmpty() } ?: displayLabel(key.spec)
    }

    private fun drawKeyPreview(canvas: Canvas, key: KeyRect, text: String) {
        val margin = dp(keyPreviewMarginDp)
        val bubbleWidth = max(dp(keyPreviewMinimumWidthDp), key.rect.width() * 1.08f)
            .coerceAtMost((width.toFloat() - margin * 2f).coerceAtLeast(1f))
        val bubbleHeight = (key.rect.height() * 1.12f)
            .coerceIn(dp(keyPreviewMinimumHeightDp), dp(keyPreviewMaximumHeightDp))
        val left = (key.rect.centerX() - bubbleWidth / 2f)
            .coerceIn(margin, width.toFloat() - margin - bubbleWidth)
        val top = (key.rect.top - bubbleHeight + dp(keyPreviewKeyOverlapDp)).coerceAtLeast(margin)
        val bubble = RectF(left, top, left + bubbleWidth, top + bubbleHeight)
        drawSurfaceShadow(canvas, bubble, pressed = false)
        paint.style = Paint.Style.FILL
        paint.color = theme.keyPressedBackground.toArgb()
        canvas.drawRoundRect(bubble, dp(keyCornerRadiusDp() + 3f), dp(keyCornerRadiusDp() + 3f), paint)
        paint.style = Paint.Style.STROKE
        paint.strokeWidth = max(1f, dp(1f))
        paint.color = theme.candidateSelectedBorderColor.toArgb()
        canvas.drawRoundRect(bubble, dp(keyCornerRadiusDp() + 3f), dp(keyCornerRadiusDp() + 3f), paint)
        textPaint.textAlign = Paint.Align.CENTER
        textPaint.textSize = sp(keyPreviewTextSizeSp)
        textPaint.color = theme.keySelectedForeground.toArgb()
        canvas.drawText(text, bubble.centerX(), bubble.centerY() + textBaselineOffset(textPaint), textPaint)
    }

    private fun drawAlternatePanel(canvas: Canvas, panel: AlternatePanel) {
        drawSurfaceShadow(canvas, panel.rect, pressed = false)
        paint.style = Paint.Style.FILL
        paint.color = panelBackgroundColor()
        canvas.drawRoundRect(panel.rect, dp(keyCornerRadiusDp() + 3f), dp(keyCornerRadiusDp() + 3f), paint)
        val itemWidth = panel.rect.width() / panel.options.size
        for ((index, option) in panel.options.withIndex()) {
            val item = RectF(
                panel.rect.left + itemWidth * index,
                panel.rect.top,
                panel.rect.left + itemWidth * (index + 1),
                panel.rect.bottom,
            )
            if (panel.selectedIndex == index) {
                paint.style = Paint.Style.FILL
                paint.color = theme.keyPressedBackground.toArgb()
                canvas.drawRoundRect(item, dp(keyCornerRadiusDp()), dp(keyCornerRadiusDp()), paint)
            }
            textPaint.textAlign = Paint.Align.CENTER
            textPaint.textSize = sp(alternatePanelTextSizeSp)
            textPaint.color = if (panel.selectedIndex == index) {
                theme.keySelectedForeground.toArgb()
            } else {
                theme.keyForeground.toArgb()
            }
            canvas.drawText(option.label, item.centerX(), item.centerY() + textBaselineOffset(textPaint), textPaint)
        }
        paint.style = Paint.Style.STROKE
        paint.strokeWidth = max(1f, dp(1f))
        paint.color = theme.candidateSelectedBorderColor.toArgb()
        canvas.drawRoundRect(panel.rect, dp(keyCornerRadiusDp() + 3f), dp(keyCornerRadiusDp() + 3f), paint)
    }

    private fun resolveCommand(key: KeySpec, deltaY: Float, rect: RectF? = null, releaseY: Float? = null): KeyCommand {
        val threshold = dp(config.swipeThresholdDp)
        val command = when {
            deltaY < -threshold -> resolveSwipeUpCommand(key)
            deltaY > threshold -> key.swipeDown ?: key.action
            else -> stackCommandForPoint(key, rect, releaseY) ?: actionForMode(key)
        }
        return applyShift(command)
    }

    private fun resolveSwipeUpCommand(key: KeySpec): KeyCommand {
        key.swipeUp?.let { return it }
        if (state.asciiMode) {
            key.asciiLongPress?.let { return it }
        }
        key.longPress?.let { return it }
        key.hint?.takeIf { it.length == 1 }?.let { return KeyCommand.input(it) }
        return key.action
    }

    private fun resolveLongPressCommand(key: KeySpec): KeyCommand {
        val command = if (state.asciiMode) {
            key.asciiLongPress ?: key.longPress
        } else {
            key.longPress
        }
            ?: key.hint?.takeIf { it.length == 1 }?.let { KeyCommand.input(it) }
            ?: key.action
        return applyShift(command)
    }

    private fun explicitAlternates(key: KeySpec): List<AlternateOption> {
        val source = if (state.asciiMode && key.asciiAlternates.isNotEmpty()) {
            key.asciiAlternates
        } else {
            key.alternates
        }
        if (source.isEmpty()) {
            val hasLegacyLongPress = key.longPress != null ||
                (state.asciiMode && key.asciiLongPress != null) ||
                !key.hint.isNullOrBlank()
            if (!hasLegacyLongPress) return emptyList()
            val command = resolveLongPressCommand(key)
            return listOf(
                AlternateOption(
                    label = command.value?.takeIf { it.isNotEmpty() } ?: key.hint ?: displayLabel(key),
                    command = command,
                )
            )
        }
        return source.map { alternate ->
            val command = if (keyboardLayer.isSymbolLayer() && alternate.action.type == KeyCommandTypes.INPUT) {
                KeyCommand.directInput(alternate.value ?: alternate.rimeValue ?: alternate.label)
            } else {
                alternate.action
            }
            AlternateOption(alternate.label, applyShift(command))
        }
    }

    private fun createAlternatePanel(
        key: KeyRect,
        options: List<AlternateOption>,
        currentX: Float,
    ): AlternatePanel {
        val margin = dp(alternatePanelMarginDp)
        val availableWidth = (width.toFloat() - margin * 2f).coerceAtLeast(1f)
        val desiredItemWidth = max(dp(alternatePanelMinimumItemWidthDp), key.rect.width())
        val panelWidth = min(availableWidth, desiredItemWidth * options.size)
        val itemWidth = panelWidth / options.size
        val left = (key.rect.centerX() - itemWidth / 2f).coerceIn(margin, width.toFloat() - margin - panelWidth)
        val panelHeight = key.rect.height()
            .coerceIn(dp(alternatePanelMinimumHeightDp), dp(alternatePanelMaximumHeightDp))
        val top = (key.rect.top - panelHeight - dp(alternatePanelGapDp)).coerceAtLeast(margin)
        val rect = RectF(left, top, left + panelWidth, top + panelHeight)
        val selectionRect = RectF(rect.left, rect.top, rect.right, max(rect.bottom, key.rect.bottom))
        return AlternatePanel(
            options = options,
            rect = rect,
            selectionRect = selectionRect,
            selectionTracker = AlternateSelectionTracker(currentX, touchSlop.toFloat()),
            selectedIndex = 0,
        )
    }

    private fun updateAlternateSelection(touch: KeyTouch, x: Float, y: Float) {
        val panel = touch.alternatePanel ?: return
        val itemWidth = panel.rect.width() / panel.options.size
        panel.selectedIndex = panel.selectionTracker.selectedIndex(
            x = x,
            insideSelection = panel.selectionRect.contains(x, y),
            panelLeft = panel.rect.left,
            itemWidth = itemWidth,
            itemCount = panel.options.size,
        )
    }

    private fun applyShift(command: KeyCommand): KeyCommand {
        val value = command.value
        if (isShiftActive() && command.type == KeyCommandTypes.INPUT && value != null && value.length == 1 && value[0].isLetter()) {
            return command.copy(value = value.uppercase())
        }
        return command
    }

    private fun displayLabel(key: KeySpec): String {
        if (key.action.type == KeyCommandTypes.SHIFT) {
            return if (shiftState == ShiftState.LOCKED) "⇪" else key.label
        }
        if (key.action.type == KeyCommandTypes.ENTER) {
            enterLabelOverride?.let { return it }
        }
        if (key.action.type == KeyCommandTypes.SPACE) {
            return state.schemaName.ifBlank { key.label }
        }
        if (key.action.type == KeyCommandTypes.MODE) {
            return if (state.asciiMode) theme.modeHintEnglishText else theme.modeHintChineseText
        }
        val label = labelForMode(key)
        val value = valueForMode(key)
        return if (isShiftActive() && value.length == 1 && value[0].isLetter()) {
            label.uppercase()
        } else {
            label
        }
    }

    private fun isShiftActive(): Boolean {
        return shiftState != ShiftState.OFF
    }

    private fun isActiveKey(key: KeySpec): Boolean {
        return key.action.type == KeyCommandTypes.SHIFT && isShiftActive()
    }

    private fun clearOneShotShiftAfter(command: KeyCommand) {
        if (shiftState != ShiftState.ONCE) return
        val value = command.value ?: return
        val consumesShift = command.type == KeyCommandTypes.INPUT && value.length == 1 && value[0].isLetter()
        if (!consumesShift) return
        shiftState = ShiftState.OFF
        lastShiftTapTimeMs = 0L
    }

    private fun activeRows(): List<List<KeySpec>> {
        val rows = config.rowsForLayer(keyboardLayer)
        if (keyboardLayer != "letters" || !shouldUseInlineNumberRow()) {
            return rows
        }
        return rows.mapIndexed { index, row ->
            if (index == 0) inlineNumberRow(row) else row
        }
    }

    private fun shouldUseInlineNumberRow(): Boolean {
        return !state.asciiMode && state.hasComposition && state.preedit.contains("=")
    }

    private fun inlineNumberRow(sourceRow: List<KeySpec>): List<KeySpec> {
        val digits = "1234567890"
        return sourceRow.mapIndexed { index, source ->
            val digit = digits.getOrNull(index)?.toString() ?: source.label
            source.copy(
                label = digit,
                value = digit,
                asciiLabel = digit,
                asciiValue = digit,
                rimeValue = null,
                hint = null,
                action = KeyCommand.input(digit),
                asciiAction = KeyCommand.input(digit),
                swipeUp = null,
                swipeDown = null,
                longPress = null,
                asciiLongPress = null,
            )
        }
    }

    private fun keyboardLayout(): List<KeyRect> {
        val signature = keyboardLayoutSignature()
        if (signature == keyboardLayoutCache.signature) {
            return keyboardLayoutCache.keys
        }

        val top = keyboardTop()
        val bottom = keyboardBottom()
        val horizontalGap = keyboardHorizontalGap()
        val verticalGapFloor = keyboardVerticalGap()
        val rows = activeRows()
        val rowCount = rows.size.coerceAtLeast(1)
        val availableHeight = (bottom - top).coerceAtLeast(0f)
        val nextRects = mutableListOf<KeyRect>()
        val maximumRowWidth = (width - keyboardOuterInset() * 2f).coerceAtLeast(1f)
        val referenceUnitWidth = keyboardReferenceUnitWidth(rows, horizontalGap)

        fun appendRows(
            layoutRows: List<List<KeySpec>>,
            rowIndexOffset: Int,
            startY: Float,
            rowHeight: Float,
            verticalGap: Float,
            sticky: Boolean,
        ): Float {
            var y = startY
            var activeLeadingSpans = mutableListOf<ActiveRowSpan>()
            for ((localRowIndex, row) in layoutRows.withIndex()) {
                val rowIndex = rowIndexOffset + localRowIndex
                if (row.isEmpty()) {
                    activeLeadingSpans = advanceRowSpans(activeLeadingSpans)
                    y += rowHeight + verticalGap
                    continue
                }
                val leadingWeight = activeLeadingSpans.sumOf { it.weight.toDouble() }.toFloat()
                val totalWeight = (leadingWeight + rowWeight(row)).coerceAtLeast(1f)
                val effectiveKeyCount = activeLeadingSpans.size + row.size
                val gapWidth = horizontalGap * (effectiveKeyCount - 1).coerceAtLeast(0)
                val rowWidth = keyboardRowWidth(
                    row = row,
                    rowIndex = rowIndex,
                    rows = rows,
                    referenceUnitWidth = referenceUnitWidth,
                    horizontalGap = horizontalGap,
                    maximumRowWidth = maximumRowWidth,
                    effectiveKeyCount = effectiveKeyCount,
                    effectiveWeight = totalWeight,
                )
                val unitWidth = ((rowWidth - gapWidth) / totalWeight).coerceAtLeast(1f)
                var x = (width - rowWidth) / 2f
                for (span in activeLeadingSpans) {
                    x += unitWidth * span.weight + horizontalGap
                }
                val nextLeadingSpans = mutableListOf<ActiveRowSpan>()
                var acceptingLeadingSpan = true
                for (key in row) {
                    val keyWidth = unitWidth * key.weight
                    val spanRows = keyRowSpan(key)
                    val keyHeight = rowHeight * spanRows + verticalGap * (spanRows - 1)
                    val rect = RectF(x, y, x + keyWidth, y + keyHeight)
                    nextRects.add(KeyRect(key, rect, sticky = sticky))
                    if (acceptingLeadingSpan && spanRows > 1) {
                        nextLeadingSpans.add(ActiveRowSpan(key.weight, spanRows - 1))
                    } else {
                        acceptingLeadingSpan = false
                    }
                    x = rect.right + horizontalGap
                }
                activeLeadingSpans = advanceRowSpans(activeLeadingSpans)
                activeLeadingSpans.addAll(nextLeadingSpans)
                y += rowHeight + verticalGap
            }
            return y
        }

        if (usesCategorizedSymbolKeyboard(rows)) {
            val targetVisibleRows = min(5, rowCount)
            val rowHeight = min(
                ((availableHeight - verticalGapFloor * (targetVisibleRows + 1)) / targetVisibleRows)
                    .coerceAtLeast(dp(40f)),
                keyboardMaxKeyHeight(),
            )
            val verticalGap = verticalGapFloor
            val headerRow = rows.take(1)
            val bodyRows = rows.drop(1).dropLast(1)
            val footerRow = rows.takeLast(1)
            val headerTop = top + verticalGap
            val footerTop = bottom - verticalGap - rowHeight
            keyboardScrollViewportTop = headerTop + rowHeight + verticalGap
            keyboardScrollViewportBottom = (footerTop - verticalGap).coerceAtLeast(keyboardScrollViewportTop)
            keyboardScrollViewportHeight = (keyboardScrollViewportBottom - keyboardScrollViewportTop).coerceAtLeast(0f)
            keyboardScrollContentHeight = (bodyRows.size * rowHeight + (bodyRows.size - 1).coerceAtLeast(0) * verticalGap)
                .coerceAtLeast(0f)
            keyboardScrollY = keyboardScrollY.coerceIn(0f, maxKeyboardScroll())
            appendRows(headerRow, 0, headerTop, rowHeight, verticalGap, sticky = true)
            appendRows(bodyRows, 1, keyboardScrollViewportTop - keyboardScrollY, rowHeight, verticalGap, sticky = false)
            appendRows(footerRow, rows.lastIndex, footerTop, rowHeight, verticalGap, sticky = true)
        } else {
            val naturalRowHeight = ((availableHeight - verticalGapFloor * (rowCount + 1)) / rowCount)
                .coerceAtLeast(dp(36f))
            val rowHeight = min(naturalRowHeight, keyboardMaxKeyHeight())
            val verticalGap = ((availableHeight - rowHeight * rowCount) / (rowCount + 1))
                .coerceAtLeast(verticalGapFloor)
            keyboardScrollY = 0f
            keyboardScrollContentHeight = availableHeight
            keyboardScrollViewportHeight = availableHeight
            keyboardScrollViewportTop = top
            keyboardScrollViewportBottom = bottom
            appendRows(rows, 0, top + verticalGap, rowHeight, verticalGap, sticky = false)
        }

        keyboardLayoutCache = KeyboardLayoutCache(signature, nextRects)
        return nextRects
    }

    private fun keyboardLayoutSignature(): String {
        return buildString {
            append(width)
            append('x')
            append(height)
            append('|')
            append(keyboardLayer)
            append('|')
            append(config.keyboardHeightDp)
            append(':')
            append(config.candidateBarHeightDp)
            append(':')
            append(config.keyboardBottomInsetDp)
            append(':')
            append(effectiveKeyboardBottomInsetDp())
            append(':')
            append(config.horizontalGapDp)
            append(':')
            append(config.verticalGapDp)
            append(':')
            append(config.outerInsetDp)
            append(':')
            append(config.maxKeyHeightDp)
            append(':')
            append(config.swipeThresholdDp)
            append('|')
            append(theme.panelGapDp)
            append(':')
            append(theme.fontSizeSp)
            append(':')
            append(theme.labelSizeSp)
            append(':')
            append(theme.commentSizeSp)
            append('|')
            append(activeRows().hashCode())
            append('|')
            append(keyboardScrollY.roundToInt())
        }
    }

    private fun invalidateKeyboardLayoutCache() {
        keyboardLayoutCache = KeyboardLayoutCache("", emptyList())
    }

    private fun actionForMode(key: KeySpec): KeyCommand {
        if (keyboardLayer.isSymbolLayer() && key.action.isTextInputCommand()) {
            return KeyCommand.directInput(valueForMode(key))
        }
        if (state.asciiMode) {
            key.asciiAction?.let { return it }
            key.asciiValue?.let { return KeyCommand.input(it) }
        } else {
            key.rimeValue?.let { return KeyCommand.rimeInput(it, key.value) }
            key.asciiValue?.takeIf { it != key.value }?.let { return KeyCommand.rimeInput(it, key.value) }
        }
        return key.action
    }

    private fun stackCommandForPoint(key: KeySpec, rect: RectF?, releaseY: Float?): KeyCommand? {
        val stack = key.stack.takeIf { it.isNotEmpty() } ?: return null
        val item = if (rect == null || releaseY == null || rect.height() <= 0f) {
            stack.first()
        } else {
            stack[stackIndexAt(key, rect, releaseY)]
        }
        return actionForMode(item)
    }

    private fun pressedStackIndexFor(keyIndex: Int, keyRect: KeyRect): Int? {
        val stack = keyRect.spec.stack
        if (stack.isEmpty()) return null
        val touch = activeKeyTouches.values.lastOrNull { it.keyIndex == keyIndex } ?: return null
        if (!keyRect.rect.contains(touch.currentX, touch.currentY)) return null
        return stackIndexAt(keyRect.spec, keyRect.rect, touch.currentY)
    }

    private fun stackIndexAt(key: KeySpec, rect: RectF, y: Float): Int {
        val count = key.stack.size
        if (count <= 1 || rect.height() <= 0f) return 0
        val itemRects = stackItemRects(rect, count)
        for ((index, itemRect) in itemRects.withIndex()) {
            if (y >= itemRect.top && y <= itemRect.bottom) return index
        }
        val ratio = ((y - rect.top) / rect.height()).coerceIn(0f, 0.999f)
        return (ratio * count).toInt().coerceIn(0, count - 1)
    }

    private fun stackItemRects(rect: RectF, count: Int): List<RectF> {
        if (count <= 1) return listOf(RectF(rect))
        val gap = min(keyboardVerticalGap(), dp(6f)).coerceAtLeast(0f)
        val itemHeight = ((rect.height() - gap * (count - 1)) / count).coerceAtLeast(1f)
        return List(count) { index ->
            val top = rect.top + (itemHeight + gap) * index
            RectF(rect.left, top, rect.right, top + itemHeight)
        }
    }

    private fun actionForMode(item: KeyStackItem): KeyCommand {
        if (keyboardLayer.isSymbolLayer() && item.isTextInputItem()) {
            return KeyCommand.directInput(valueForMode(item))
        }
        if (state.asciiMode) {
            item.asciiAction?.let { return it }
            item.asciiValue?.let { return KeyCommand.input(it) }
        } else {
            item.rimeValue?.let { return KeyCommand.rimeInput(it, item.value ?: item.label) }
            item.asciiValue?.takeIf { it != (item.value ?: item.label) }?.let {
                return KeyCommand.rimeInput(it, item.value ?: item.label)
            }
        }
        item.action?.let { return it }
        return KeyCommand.input(item.value ?: item.label)
    }

    private fun labelForMode(key: KeySpec): String {
        return if (state.asciiMode) {
            key.asciiLabel ?: key.asciiValue ?: key.label
        } else {
            key.label
        }
    }

    private fun stackLabelForMode(item: KeyStackItem): String {
        return if (state.asciiMode) {
            item.asciiLabel ?: item.asciiValue ?: item.label
        } else {
            item.label
        }
    }

    private fun valueForMode(key: KeySpec): String {
        return if (state.asciiMode) {
            key.asciiValue ?: key.value
        } else {
            key.value
        }
    }

    private fun valueForMode(item: KeyStackItem): String {
        val value = item.value ?: item.label
        return if (state.asciiMode) {
            item.asciiValue ?: value
        } else {
            value
        }
    }

    private fun KeyCommand.isTextInputCommand(): Boolean {
        return type == KeyCommandTypes.INPUT || type == KeyCommandTypes.RIME_INPUT || type == KeyCommandTypes.DIRECT_INPUT
    }

    private fun KeyStackItem.isTextInputItem(): Boolean {
        val actionType = action?.type
        return actionType == null ||
            actionType == KeyCommandTypes.INPUT ||
            actionType == KeyCommandTypes.RIME_INPUT ||
            actionType == KeyCommandTypes.DIRECT_INPUT
    }

    private fun toolbarActions(): List<ToolbarAction> {
        val function = ToolbarAction("Rime", KeyCommand.panel("rime"), icon = ToolbarIcon.FUNCTION)
        val languageToggle = languageToggleAction()
        val oneHanded = ToolbarAction(
            if (keyboardLayoutMode == KeyboardLayoutMode.ONE_HANDED) "退出单手" else "单手",
            KeyCommand(KeyCommandTypes.ONE_HANDED),
            selected = keyboardLayoutMode == KeyboardLayoutMode.ONE_HANDED,
            icon = ToolbarIcon.ONE_HANDED,
        )
        val floating = ToolbarAction(
            if (keyboardLayoutMode == KeyboardLayoutMode.FLOATING) "退出悬浮" else "悬浮",
            KeyCommand(KeyCommandTypes.FLOATING),
            selected = keyboardLayoutMode == KeyboardLayoutMode.FLOATING,
            icon = ToolbarIcon.FLOATING,
        )
        val layoutActions = buildList {
            if (oneHandedAvailable) add(oneHanded)
            add(floating)
        }
        return if (keyboardLayer == "symbols") {
            buildList {
                addAll(listOf(
                function,
                ToolbarAction("中", KeyCommand(KeyCommandTypes.MODE, "chinese"), selected = !state.asciiMode),
                ToolbarAction("En", KeyCommand(KeyCommandTypes.MODE, "ascii"), selected = state.asciiMode),
                ToolbarAction("123", KeyCommand(KeyCommandTypes.KEYBOARD_MODE, "numbers")),
                ToolbarAction("ABC", KeyCommand(KeyCommandTypes.KEYBOARD_MODE, "letters")),
                ))
                addAll(layoutActions)
            }
        } else {
            buildList {
                addAll(listOf(
                function,
                languageToggle,
                ToolbarAction("选择", KeyCommand(KeyCommandTypes.KEYBOARD_MODE, "editor"), icon = ToolbarIcon.SELECTION),
                ToolbarAction("剪贴板", KeyCommand.panel("clipboard"), icon = ToolbarIcon.CLIPBOARD),
                ToolbarAction("Emoji", KeyCommand(KeyCommandTypes.KEYBOARD_MODE, "symbols_emoji_face"), icon = ToolbarIcon.EMOJI),
                ))
                addAll(layoutActions)
            }
        }
    }

    private fun drawFloatingInteractionHints(canvas: Canvas) {
        if (keyboardLayoutMode != KeyboardLayoutMode.FLOATING) return
        val color = Color.argb(
            150,
            theme.commentColor.red,
            theme.commentColor.green,
            theme.commentColor.blue,
        )
        paint.style = Paint.Style.FILL
        paint.color = color
        val handleWidth = min(dp(30f), width * 0.12f)
        val handleHeight = max(dp(2f), dp(theme.candidateBorderWidthDp))
        val handleBottom = height - dp(2f)
        canvas.drawRoundRect(
            RectF(
                width / 2f - handleWidth / 2f,
                handleBottom - handleHeight,
                width / 2f + handleWidth / 2f,
                handleBottom,
            ),
            handleHeight / 2f,
            handleHeight / 2f,
            paint,
        )

        paint.style = Paint.Style.STROKE
        paint.strokeWidth = max(dp(1.4f), dp(theme.candidateBorderWidthDp))
        paint.strokeCap = Paint.Cap.ROUND
        paint.strokeJoin = Paint.Join.ROUND
        paint.color = Color.argb(
            72,
            theme.commentColor.red,
            theme.commentColor.green,
            theme.commentColor.blue,
        )
        val frameInset = paint.strokeWidth / 2f
        val frameRadius = dp(theme.keyCornerRadiusDp).coerceAtMost(min(width, height) / 2f)
        canvas.drawRoundRect(
            RectF(frameInset, frameInset, width - frameInset, height - frameInset),
            frameRadius,
            frameRadius,
            paint,
        )

        paint.color = color
        val inset = dp(5f)
        val size = dp(9f)
        val corners = Path().apply {
            moveTo(inset + size, inset)
            lineTo(inset, inset)
            lineTo(inset, inset + size)

            moveTo(width - inset - size, inset)
            lineTo(width - inset, inset)
            lineTo(width - inset, inset + size)

            moveTo(inset, height - inset - size)
            lineTo(inset, height - inset)
            lineTo(inset + size, height - inset)

            moveTo(width - inset - size, height - inset)
            lineTo(width - inset, height - inset)
            lineTo(width - inset, height - inset - size)
        }
        canvas.drawPath(corners, paint)
    }

    private fun languageToggleAction(): ToolbarAction {
        return if (state.asciiMode) {
            ToolbarAction(
                "En",
                KeyCommand(KeyCommandTypes.MODE),
                secondaryLabel = "中",
            )
        } else {
            ToolbarAction(
                "中",
                KeyCommand(KeyCommandTypes.MODE),
                secondaryLabel = "En",
            )
        }
    }

    private fun functionPanelTitle(): String {
        return when (functionPanelMode) {
            FunctionPanelMode.RIME -> "Rime 选项"
            FunctionPanelMode.CLIPBOARD -> "剪贴板"
        }
    }

    private fun isInCandidateBar(y: Float): Boolean {
        return y >= 0f && y < dp(config.candidateBarHeightDp)
    }

    private fun isInExpandedCandidatePanel(y: Float): Boolean {
        val top = dp(config.candidateBarHeightDp)
        return candidatePanelExpanded && y >= top && y < keyboardBottom()
    }

    private fun usesFullHeightSymbolKeyboard(): Boolean {
        return keyboardLayer.isSymbolLayer() && !candidatePanelExpanded && !functionPanelActive
    }

    private fun String.isSymbolLayer(): Boolean {
        return this == "symbols" || startsWith("symbols_")
    }

    private fun usesCategorizedSymbolKeyboard(rows: List<List<KeySpec>> = activeRows()): Boolean {
        return usesFullHeightSymbolKeyboard() && rows.size >= 3
    }

    private fun usesScrollableSymbolKeyboard(rows: List<List<KeySpec>> = activeRows()): Boolean {
        return usesCategorizedSymbolKeyboard(rows) && rows.size > 5
    }

    private fun expandedCandidatePanelHeight(): Float {
        return if (candidatePanelExpanded && (functionPanelActive || state.candidatePanel.candidates.isNotEmpty() || expandedCandidatesLoading)) {
            (keyboardBottom() - dp(config.candidateBarHeightDp)).coerceAtLeast(0f)
        } else {
            0f
        }
    }

    private fun keyboardTop(): Float {
        if (usesFullHeightSymbolKeyboard()) return 0f
        return dp(config.candidateBarHeightDp)
    }

    private fun keyboardBottom(): Float {
        return height.toFloat() - bottomReservedInset()
    }

    private fun bottomReservedInset(): Float {
        val requested = min(dp(effectiveKeyboardBottomInsetDp()), dp(64f))
        val minKeyboardContentHeight = dp(180f)
        val available = (height.toFloat() - keyboardTop() - minKeyboardContentHeight).coerceAtLeast(0f)
        return min(requested, available)
    }

    /** Same rule as the service: the system inset is the floor, config is extra. */
    private fun effectiveKeyboardBottomInsetDp(): Int {
        if (keyboardLayoutMode == KeyboardLayoutMode.FLOATING) return 0
        val system = if (systemBottomInsetDp >= 0) systemBottomInsetDp else androidSystemBottomInsetDp
        return max(system, config.keyboardBottomInsetDp)
    }

    private fun toggleCandidatePanel() {
        if (candidatePanelExpanded) {
            closeCandidatePanel()
        } else {
            openCandidatePanel()
        }
    }

    private fun openCandidatePanel() {
        if (state.candidatePanel.candidates.isEmpty()) return
        toolbarOverlayActive = false
        functionPanelActive = false
        clipboardClearConfirmationPending = false
        candidatePanelExpanded = true
        expandedCandidates = emptyList()
        pressedToolbar = null
        toolbarTouchActive = false
        resetExpandedCandidateScroll()
        requestExpandedCandidatesAsync()
        startContentTransition()
    }

    private fun closeCandidatePanel() {
        if (!candidatePanelExpanded && expandedCandidates.isEmpty() && !functionPanelActive) return
        candidatePanelExpanded = false
        toolbarOverlayActive = false
        functionPanelActive = false
        functionPanelMode = FunctionPanelMode.RIME
        keyboardLayer = "letters"
        rimeOptionsState = KeytaoRimeOptionsState.EMPTY
        rimeOptionsLoading = false
        clipboardClearConfirmationPending = false
        recentClipboardSuggestion = null
        expandedCandidates = emptyList()
        cancelExpandedCandidateRequest()
        clipboardItemsLoading = false
        resetExpandedCandidateTouch()
        resetExpandedCandidateScroll()
        resetKeyboardScroll()
        invalidateKeyboardLayoutCache()
        invalidateExpandedCandidateItemsCache()
        startContentTransition()
    }

    private fun openFunctionPanel(mode: FunctionPanelMode) {
        if (mode != FunctionPanelMode.CLIPBOARD || functionPanelMode != mode) {
            clipboardClearConfirmationPending = false
        }
        toolbarOverlayActive = false
        functionPanelActive = true
        candidatePanelExpanded = true
        functionPanelMode = mode
        expandedCandidates = emptyList()
        cancelExpandedCandidateRequest()
        clipboardItemsLoading = mode == FunctionPanelMode.CLIPBOARD
        rimeOptionsLoading = mode == FunctionPanelMode.RIME
        if (rimeOptionsLoading) {
            rimeOptionsState = KeytaoRimeOptionsState.EMPTY
        }
        pressedToolbar = null
        toolbarTouchActive = false
        resetExpandedCandidateScroll()
        if (mode == FunctionPanelMode.CLIPBOARD) {
            requestClipboardItemsAsync()
        }
        startContentTransition()
    }

    private fun handleToolbarCommand(command: KeyCommand) {
        if (command.type == KeyCommandTypes.PANEL && command.value == "toggleToolbar") {
            toolbarOverlayActive = !toolbarOverlayActive
            resetCandidateTouch()
            performConfiguredSelectionFeedback()
            invalidate()
            return
        }
        toolbarOverlayActive = false
        if (handlePanelCommand(command)) {
            return
        }
        if (command.type == KeyCommandTypes.EDIT && command.value == "pasteText") {
            clearRecentClipboardSuggestion()
        }
        performConfiguredSelectionFeedback()
        listener?.onKeyCommand(command)
    }

    private fun handlePanelCommand(command: KeyCommand): Boolean {
        if (command.type == KeyCommandTypes.PANEL) {
            when (command.value) {
                "close" -> closeCandidatePanel()
                "dismissClipboard" -> clearRecentClipboardSuggestion()
                "rime" -> {
                    openFunctionPanel(FunctionPanelMode.RIME)
                    listener?.onKeyCommand(KeyCommand(KeyCommandTypes.RIME_MENU))
                }
                "clipboard" -> openFunctionPanel(FunctionPanelMode.CLIPBOARD)
                "clearClipboardHistory" -> handleClearClipboardHistory()
                else -> setKeyboardLayer("letters")
            }
            performConfiguredSelectionFeedback()
            invalidate()
            return true
        }
        performConfiguredSelectionFeedback()
        listener?.onKeyCommand(command)
        return true
    }

    private fun requestExpandedCandidatesAsync() {
        pendingExpandedCandidateLoad?.let(longPressHandler::removeCallbacks)
        pendingExpandedCandidateLoad = null

        if (!canRequestExpandedCandidates()) {
            expandedCandidatesLoading = false
            return
        }

        val callback = listener ?: run {
            expandedCandidatesLoading = false
            return
        }
        val token = ++expandRequestToken
        expandedCandidatesLoading = true
        val request = Runnable {
            pendingExpandedCandidateLoad = null
            if (token != expandRequestToken || !canRequestExpandedCandidates()) {
                expandedCandidatesLoading = false
                invalidate()
                return@Runnable
            }
            callback.onRequestExpandCandidates { candidates ->
                if (token != expandRequestToken || !canRequestExpandedCandidates()) return@onRequestExpandCandidates
                expandedCandidates = candidates
                expandedCandidatesLoading = false
                coerceExpandedCandidateScroll()
                invalidate()
            }
        }
        pendingExpandedCandidateLoad = request
        longPressHandler.postDelayed(request, expandedCandidateLoadDelayMs)
        invalidate()
    }

    private fun canRequestExpandedCandidates(): Boolean {
        if (!candidatePanelExpanded || state.candidatePanel.candidates.isEmpty()) return false
        return !functionPanelActive
    }

    private fun cancelExpandedCandidateRequest() {
        pendingExpandedCandidateLoad?.let(longPressHandler::removeCallbacks)
        pendingExpandedCandidateLoad = null
        expandRequestToken++
        expandedCandidatesLoading = false
    }

    private fun requestClipboardItemsAsync() {
        val callback = listener ?: run {
            clipboardItemsLoading = false
            return
        }
        val token = ++expandRequestToken
        clipboardItemsLoading = true
        callback.onRequestClipboardHistory { items ->
            if (token != expandRequestToken || !candidatePanelExpanded || functionPanelMode != FunctionPanelMode.CLIPBOARD) {
                return@onRequestClipboardHistory
            }
            clipboardItems = items
            if (items.isEmpty()) {
                clipboardClearConfirmationPending = false
            }
            clipboardItemsLoading = false
            coerceExpandedCandidateScroll()
            invalidate()
        }
    }

    private fun panelCandidateGlobalIndex(localIndex: Int): Int {
        val pageSize = state.pageSize.takeIf { it > 0 }
            ?: state.candidatePanel.candidates.size.coerceAtLeast(1)
        return state.page * pageSize + localIndex
    }

    private fun selectedGlobalCandidateIndex(): Int {
        return panelCandidateGlobalIndex(state.highlightedCandidateIndex)
    }

    private fun resetCandidateTouch() {
        candidateTouchActive = false
        candidateDragging = false
        candidatePagingConsumed = false
    }

    private fun resetExpandedCandidateTouch() {
        expandedTouchActive = false
        expandedDragging = false
        pressedExpandedCandidate = null
        pressedClipboardDelete = null
    }

    private fun deleteClipboardEntry(text: String) {
        if (!functionPanelActive || functionPanelMode != FunctionPanelMode.CLIPBOARD) return
        clipboardClearConfirmationPending = false
        performConfiguredSelectionFeedback()
        listener?.onDeleteClipboardEntry(text)
        requestClipboardItemsAsync()
        invalidate()
    }

    private fun handleClearClipboardHistory() {
        if (!functionPanelActive || functionPanelMode != FunctionPanelMode.CLIPBOARD || clipboardItems.isEmpty()) {
            clipboardClearConfirmationPending = false
            return
        }
        if (!clipboardClearConfirmationPending) {
            clipboardClearConfirmationPending = true
            return
        }
        clipboardClearConfirmationPending = false
        listener?.onClearClipboardHistory()
        requestClipboardItemsAsync()
    }

    private fun resetCandidateScroll() {
        candidateScrollX = 0f
        candidateContentWidth = width.toFloat()
        candidateViewportWidth = width.toFloat()
    }

    private fun resetExpandedCandidateScroll() {
        expandedCandidateScrollY = 0f
        expandedCandidateContentHeight = expandedCandidatePanelHeight()
    }

    private fun resetKeyboardScroll() {
        keyboardScrollY = 0f
        keyboardDownY = 0f
        keyboardDownScrollY = 0f
        keyboardDragging = false
        keyboardScrollTouchActive = false
        keyboardScrollContentHeight = 0f
        keyboardScrollViewportHeight = 0f
        keyboardScrollViewportTop = keyboardTop()
        keyboardScrollViewportBottom = keyboardBottom()
        invalidateKeyboardLayoutCache()
    }

    private fun maxCandidateScroll(): Float {
        return max(0f, candidateContentWidth - candidateViewportWidth)
    }

    private fun maxKeyboardScroll(): Float {
        return max(0f, keyboardScrollContentHeight - keyboardScrollViewportHeight)
    }

    private fun coerceCandidateScroll() {
        candidateScrollX = candidateScrollX.coerceIn(0f, maxCandidateScroll())
    }

    private fun maxExpandedCandidateScroll(): Float {
        return max(0f, expandedCandidateContentHeight - expandedCandidatePanelHeight())
    }

    private fun coerceExpandedCandidateScroll() {
        expandedCandidateScrollY = expandedCandidateScrollY.coerceIn(0f, maxExpandedCandidateScroll())
    }

    private inline fun drawContentLayer(canvas: Canvas, top: Float, draw: () -> Unit) {
        val progress = contentTransitionProgress()
        if (progress >= 0.999f) {
            draw()
            return
        }
        val alpha = (255f * progress).toInt().coerceIn(0, 255)
        val offsetY = dp(10f) * (1f - progress)
        val checkpoint = canvas.saveLayerAlpha(0f, top, width.toFloat(), height.toFloat(), alpha)
        canvas.translate(0f, offsetY)
        draw()
        canvas.restoreToCount(checkpoint)
    }

    private fun startContentTransition() {
        contentTransitionStartMs = System.currentTimeMillis()
        postInvalidateOnAnimation()
    }

    private fun contentTransitionProgress(): Float {
        if (contentTransitionStartMs == 0L) return 1f
        val elapsed = System.currentTimeMillis() - contentTransitionStartMs
        if (elapsed >= contentTransitionDurationMs) return 1f
        postInvalidateOnAnimation()
        val t = (elapsed.toFloat() / contentTransitionDurationMs).coerceIn(0f, 1f)
        return 1f - (1f - t) * (1f - t)
    }

    private fun candidateSignature(next: KeytaoImeState): String {
        val panel = next.candidatePanel
        return buildString {
            append(panel.preedit.orEmpty())
            append('|')
            append(panel.navigation.canGoPrevious)
            append(':')
            append(panel.navigation.canGoNext)
            append('|')
            append(next.schemaName)
            append('|')
            append(next.pageSize)
            append('|')
            append(next.page)
            panel.candidates.forEach { candidate ->
                append('|')
                append(candidate.index)
                append(':')
                append(candidate.label)
                append(':')
                append(candidate.text)
                append(':')
                append(candidate.comment.orEmpty())
                append(':')
                append(candidate.selected)
            }
        }
    }

    private fun updatePanelGestureMove(event: MotionEvent) {
        val pointerId = panelGesturePointerId ?: return
        val pointerIndex = event.findPointerIndex(pointerId)
        if (pointerIndex < 0) return
        val x = event.getX(pointerIndex)
        val y = event.getY(pointerIndex)
        when {
            toolbarTouchActive -> {
                val toolbar = pressedToolbar
                if (toolbar != null && !toolbar.rect.contains(x, y)) {
                    longPressHandler.removeCallbacks(toolbarLongPressRunnable)
                    pressedToolbar = null
                    invalidate()
                }
            }
            candidateExpandPressed -> invalidate()
            expandedTouchActive -> {
                val deltaY = y - expandedDownY
                if (!expandedDragging && abs(deltaY) > touchSlop) {
                    expandedDragging = true
                    pressedExpandedCandidate = null
                    pressedClipboardDelete = null
                }
                if (expandedDragging) {
                    expandedCandidateScrollY = (expandedDownScrollY - deltaY).coerceIn(0f, maxExpandedCandidateScroll())
                    invalidate()
                }
            }
            candidateTouchActive -> {
                val deltaX = x - candidateDownX
                val deltaY = y - candidateDownY
                val dragSlop = touchSlop.toFloat()
                if (!candidateDragging && (abs(deltaX) > dragSlop || abs(deltaY) > dragSlop)) {
                    candidateDragging = true
                }
                if (candidateDragging) {
                    val maxScroll = maxCandidateScroll()
                    val requestedScroll = candidateDownScrollX - deltaX
                    candidateScrollX = requestedScroll.coerceIn(0f, maxScroll)
                    if (!candidatePagingConsumed) {
                        val navigation = state.candidatePanel.navigation
                        val pageCommand = when {
                            requestedScroll > maxScroll + dragSlop && navigation.canGoNext -> {
                                KeyCommand(KeyCommandTypes.NEXT_PAGE)
                            }
                            requestedScroll < -dragSlop && navigation.canGoPrevious -> {
                                KeyCommand(KeyCommandTypes.PREVIOUS_PAGE)
                            }
                            else -> null
                        }
                        if (pageCommand != null) {
                            candidatePagingConsumed = true
                            candidateScrollX = 0f
                            candidateDownScrollX = 0f
                            candidateDownX = x
                            performConfiguredSelectionFeedback(playSound = false)
                            listener?.onKeyCommand(pageCommand)
                        }
                    }
                    invalidate()
                }
            }
            keyboardScrollTouchActive -> {
                val deltaY = y - keyboardDownY
                if (!keyboardDragging && abs(deltaY) > touchSlop) {
                    keyboardDragging = true
                    cancelKeyTouch(panelGesturePointerId)
                }
                if (keyboardDragging) {
                    keyboardScrollY = (keyboardDownScrollY - deltaY).coerceIn(0f, maxKeyboardScroll())
                    invalidateKeyboardLayoutCache()
                    invalidate()
                }
            }
        }
    }

    private fun finishPanelGesture(x: Float, y: Float): Boolean {
        if (toolbarTouchActive) {
            longPressHandler.removeCallbacks(toolbarLongPressRunnable)
            val toolbar = pressedToolbar
            val longPressConsumed = toolbarLongPressConsumed
            pressedToolbar = null
            toolbarTouchActive = false
            toolbarLongPressConsumed = false
            if (!longPressConsumed && toolbar != null && toolbar.rect.contains(x, y)) {
                handleToolbarCommand(toolbar.command)
            }
            return true
        }
        if (candidateExpandPressed) {
            candidateExpandPressed = false
            if (candidateExpandRect?.contains(x, y) == true) {
                toggleCandidatePanel()
                performConfiguredSelectionFeedback()
            }
            return true
        }
        if (expandedTouchActive) {
            val candidate = pressedExpandedCandidate
            val clipboardDelete = pressedClipboardDelete
            expandedTouchActive = false
            pressedExpandedCandidate = null
            pressedClipboardDelete = null
            if (!expandedDragging && clipboardDelete != null && clipboardDelete.rect.contains(x, y)) {
                deleteClipboardEntry(clipboardDelete.text)
            } else if (!expandedDragging && candidate != null && candidate.rect.contains(x, y)) {
                val command = candidate.command
                if (command != null) {
                    handlePanelCommand(command)
                } else {
                    closeCandidatePanel()
                    performConfiguredSelectionFeedback()
                    listener?.onCandidate(candidate.index, candidate.global)
                }
            }
            expandedDragging = false
            return true
        }
        if (candidateTouchActive) {
            val wasDragging = candidateDragging
            resetCandidateTouch()
            val dragSlop = touchSlop.toFloat()
            if (!wasDragging && abs(x - candidateDownX) <= dragSlop && abs(y - candidateDownY) <= dragSlop) {
                findCandidate(x, y)?.let {
                    performConfiguredSelectionFeedback()
                    listener?.onCandidate(it.index, it.global)
                }
            }
            return true
        }
        if (keyboardScrollTouchActive) {
            val wasDragging = keyboardDragging
            keyboardScrollTouchActive = false
            keyboardDragging = false
            if (wasDragging) {
                cancelKeyTouch(panelGesturePointerId)
                return true
            }
        }
        return false
    }

    private fun beginKeyTouch(pointerId: Int, key: KeyRect, x: Float, y: Float): Boolean {
        val keyIndex = keyRects.indexOfFirst { it === key }
        if (keyIndex < 0) return false
        val touch = KeyTouch(
            key = key,
            keyIndex = keyIndex,
            originKey = key,
            originKeyIndex = keyIndex,
            downX = x,
            downY = y,
            cursorGesture = if (isSpaceKey(key.spec) && !hasActiveComposition()) {
                CursorGestureTracker(
                    startX = x,
                    activationDistance = dp(KeytaoImeInteractionTuning.CURSOR_GESTURE_ACTIVATION_DP),
                    stepDistance = dp(KeytaoImeInteractionTuning.CURSOR_GESTURE_STEP_DP),
                )
            } else {
                null
            },
        )
        performConfiguredKeyPress(actionForMode(key.spec))
        if (isBackspaceKey(key.spec)) {
            touch.longPressConsumed = true
            touch.backspaceGestureUnits = 1
            val profile = backspaceRepeatProfile()
            activeKeyTouches.begin(
                pointerId = pointerId,
                state = touch,
                delayMs = profile.initialDelayMs,
                onLongPress = { activeTouch -> startRepeatingKey(pointerId, activeTouch.key) },
            )
            dispatchKeyCommand(actionForMode(key.spec))
        } else {
            registerLongPress(pointerId, touch)
        }
        return true
    }

    private fun registerLongPress(pointerId: Int, touch: KeyTouch) {
        val spec = touch.key.spec
        val hasLongPressAction = spec.longPress != null ||
            spec.asciiLongPress != null ||
            !spec.hint.isNullOrBlank() ||
            explicitAlternates(spec).isNotEmpty() ||
            isRepeatableKey(spec)
        activeKeyTouches.begin(
            pointerId = pointerId,
            state = touch,
            delayMs = config.longPressDelayMs.takeIf { hasLongPressAction },
            onLongPress = if (hasLongPressAction) {
                { activeTouch ->
                    activeTouch.longPressConsumed = true
                    val options = explicitAlternates(activeTouch.key.spec)
                    if (options.isNotEmpty()) {
                        activeTouch.alternatePanel = createAlternatePanel(activeTouch.key, options, activeTouch.currentX)
                    } else if (isRepeatableKey(activeTouch.key.spec)) {
                        startRepeatingKey(pointerId, activeTouch.key)
                    } else {
                        val command = resolveLongPressCommand(activeTouch.key.spec)
                        clearRecentClipboardSuggestionForCommand(command)
                        listener?.onKeyCommand(command)
                        clearOneShotShiftAfter(command)
                    }
                    performConfiguredHaptic(strong = true, playSound = false)
                    invalidate()
                }
            } else {
                null
            },
        )
    }

    private fun updateKeyTouchMove(event: MotionEvent) {
        for (pointerIndex in 0 until event.pointerCount) {
            val pointerId = event.getPointerId(pointerIndex)
            val touch = activeKeyTouches[pointerId] ?: continue
            val x = event.getX(pointerIndex)
            val y = event.getY(pointerIndex)
            touch.currentX = x
            touch.currentY = y
            if (touch.alternatePanel != null) {
                updateAlternateSelection(touch, x, y)
                continue
            }
            if (handleBackspaceDrag(touch, pointerId, x, y)) {
                continue
            }
            if (handleSpaceCursorDrag(touch, pointerId, x)) {
                continue
            }
            val deltaY = y - touch.downY
            if (abs(deltaY) >= dp(config.swipeThresholdDp)) {
                if (touch.keyIndex != touch.originKeyIndex) {
                    touch.key = touch.originKey
                    touch.keyIndex = touch.originKeyIndex
                }
                stopLongPressAndRepeat(pointerId)
                continue
            }
            if (retargetKeyIfNeeded(pointerId, touch, x, y)) {
                continue
            }
            val holdRect = RectF(touch.key.rect)
            if (isBackspaceKey(touch.key.spec)) {
                val tolerance = dp(KeytaoImeInteractionTuning.BACKSPACE_HOLD_TOLERANCE_DP)
                holdRect.inset(-tolerance, -tolerance)
            }
            if (!holdRect.contains(x, y)) {
                stopLongPressAndRepeat(pointerId)
            }
        }
        invalidate()
    }

    private fun finishKeyTouch(pointerId: Int, x: Float, y: Float): Boolean {
        stopLongPressAndRepeat(pointerId)
        val touch = activeKeyTouches.finish(pointerId) ?: return false
        touch.alternatePanel?.let { panel ->
            updateAlternateSelection(touch, x, y)
            panel.selectedIndex
                ?.takeIf { panel.selectionRect.contains(x, y) }
                ?.let(panel.options::get)
                ?.let { option ->
                    performConfiguredSelectionFeedback(playSound = false)
                    dispatchKeyCommand(option.command)
                }
            return true
        }
        if (touch.backspaceGestureConsumed) {
            handleBackspaceDrag(touch, pointerId, x, y, final = true)
            return true
        }
        if (handleBackspaceRelease(touch, x, y)) {
            return true
        }
        if (shouldAcceptKeyRelease(touch, x, y) && !touch.longPressConsumed) {
            val composingSpace = isSpaceKey(touch.key.spec) && hasActiveComposition()
            activateKey(
                touch.key,
                deltaY = if (composingSpace) 0f else y - touch.downY,
                releaseY = if (composingSpace) touch.key.rect.centerY() else y,
                feedbackAlreadyDelivered = true,
            )
        }
        return true
    }

    private fun handleSpaceCursorDrag(touch: KeyTouch, pointerId: Int, x: Float): Boolean {
        val tracker = touch.cursorGesture ?: return false
        if (hasActiveComposition()) {
            touch.cursorGesture = null
            return false
        }
        val update = tracker.update(x)
        if (!update.active) return false
        touch.longPressConsumed = true
        stopLongPressAndRepeat(pointerId)
        val command = KeyCommand.edit(if (update.stepDelta < 0) "cursorLeft" else "cursorRight")
        repeat(abs(update.stepDelta)) {
            listener?.onKeyCommand(command)
        }
        return true
    }

    private fun retargetKeyIfNeeded(pointerId: Int, touch: KeyTouch, x: Float, y: Float): Boolean {
        if (!isCharacterKey(touch.originKey.spec) || !isCharacterKey(touch.key.spec)) return false
        val hysteresis = dp(KeytaoImeInteractionTuning.SLIDE_RETARGET_HYSTERESIS_DP)
        val retainedRect = RectF(touch.key.rect).apply { inset(-hysteresis, -hysteresis) }
        if (retainedRect.contains(x, y)) return false
        val targetIndex = findKeyIndex(x, y) ?: return false
        if (targetIndex == touch.keyIndex) return false
        val target = keyRects[targetIndex]
        if (!isCharacterKey(target.spec)) return false
        touch.key = target
        touch.keyIndex = targetIndex
        touch.alternatePanel = null
        registerLongPress(pointerId, touch)
        return true
    }

    private fun activateKey(
        key: KeyRect,
        deltaY: Float = 0f,
        releaseY: Float? = null,
        feedbackAlreadyDelivered: Boolean = false,
    ) {
        val command = resolveCommand(key.spec, deltaY, key.rect, releaseY)
        if (!feedbackAlreadyDelivered) {
            performConfiguredHaptic(soundEffect = keySoundEffect(command))
        } else if (isConfirmationCommand(command)) {
            performConfiguredSelectionFeedback(playSound = false)
        }
        dispatchKeyCommand(command)
    }

    private fun dispatchKeyCommand(command: KeyCommand) {
        clearRecentClipboardSuggestionForCommand(command)
        listener?.onKeyCommand(command)
        clearOneShotShiftAfter(command)
    }

    private fun clearActiveKeyTouches() {
        stopLongPressAndRepeat()
        activeKeyTouches.clear()
    }

    private fun cancelKeyTouch(pointerId: Int?) {
        val resolvedPointerId = pointerId ?: return
        stopLongPressAndRepeat(resolvedPointerId)
        activeKeyTouches.finish(resolvedPointerId)
    }

    private fun stopLongPressAndRepeat(pointerId: Int? = null) {
        if (pointerId == null) {
            longPressHandler.removeCallbacks(toolbarLongPressRunnable)
            activeKeyTouches.cancelAllLongPress()
        } else {
            activeKeyTouches.cancelLongPress(pointerId)
        }
        if (pointerId == null) {
            repeatRunnablesByPointerId.values.toList().forEach(longPressHandler::removeCallbacks)
            repeatRunnablesByPointerId.clear()
        } else {
            repeatRunnablesByPointerId.remove(pointerId)?.let(longPressHandler::removeCallbacks)
        }
    }

    private fun isRepeatableKey(key: KeySpec): Boolean {
        val command = actionForMode(key)
        return command.type == KeyCommandTypes.BACKSPACE ||
            (command.type == KeyCommandTypes.EDIT && command.value in repeatableEditVerbs)
    }

    private fun isCharacterKey(key: KeySpec): Boolean {
        if (key.stack.isNotEmpty()) return false
        return actionForMode(key).type in setOf(
            KeyCommandTypes.INPUT,
            KeyCommandTypes.DIRECT_INPUT,
            KeyCommandTypes.RIME_INPUT,
        )
    }

    private fun handleBackspaceDrag(
        touch: KeyTouch,
        pointerId: Int,
        x: Float,
        y: Float,
        final: Boolean = false,
    ): Boolean {
        if (!isBackspaceKey(touch.key.spec)) return false
        val deltaX = x - touch.downX
        val deltaY = y - touch.downY
        val threshold = max(dp(8f), dp(config.swipeThresholdDp) * 0.65f)
        if (abs(deltaX) <= threshold || abs(deltaX) <= abs(deltaY) * 0.75f) {
            return false
        }

        stopLongPressAndRepeat(pointerId)
        touch.longPressConsumed = true
        touch.backspaceGestureConsumed = true

        val stepWidth = max(dp(8f), touch.key.rect.width() * 0.22f)
        val moved = max(0f, abs(deltaX) - threshold)
        val stepCount = max(1, (moved / stepWidth).toInt() + 1)
        val targetUnits = (if (deltaX < 0f) stepCount else -stepCount)
            .coerceIn(-maxBackspaceGestureUnitsPerGesture, maxBackspaceGestureUnitsPerGesture)
        val deltaUnits = targetUnits - touch.backspaceGestureUnits
        if (deltaUnits == 0) return true

        val action = if (deltaUnits > 0) "delete" else "restore"
        listener?.onKeyCommand(backspaceGestureCommand(action, abs(deltaUnits)))
        touch.backspaceGestureUnits = targetUnits
        if (!final) performConfiguredHaptic(soundEffect = AudioManager.FX_KEYPRESS_DELETE)
        return true
    }

    private fun handleBackspaceRelease(touch: KeyTouch, x: Float, y: Float): Boolean {
        if (!isBackspaceKey(touch.key.spec) || touch.backspaceGestureConsumed) return false
        val deltaX = x - touch.downX
        val deltaY = y - touch.downY
        val threshold = max(dp(12f), dp(config.swipeThresholdDp))
        if (abs(deltaY) <= threshold || abs(deltaY) <= abs(deltaX) * 1.1f) {
            return false
        }

        listener?.onKeyCommand(backspaceGestureCommand(if (deltaY < 0f) "deleteAll" else "restoreAll"))
        performConfiguredHaptic(strong = true, soundEffect = AudioManager.FX_KEYPRESS_DELETE)
        return true
    }

    private fun backspaceGestureCommand(action: String, count: Int = 1): KeyCommand {
        return KeyCommand(KeyCommandTypes.BACKSPACE_GESTURE, action, count.coerceAtLeast(1).toString())
    }

    private fun isBackspaceKey(key: KeySpec): Boolean {
        return actionForMode(key).type == KeyCommandTypes.BACKSPACE
    }

    private fun isSpaceKey(key: KeySpec): Boolean {
        return actionForMode(key).type == KeyCommandTypes.SPACE
    }

    private fun hasActiveComposition(): Boolean {
        return state.hasComposition ||
            !state.candidatePanel.preedit.isNullOrEmpty() ||
            state.candidatePanel.candidates.isNotEmpty()
    }

    private fun startRepeatingKey(pointerId: Int, key: KeyRect) {
        val repeatingKeyIndex = activeKeyTouches[pointerId]?.keyIndex ?: return
        repeatRunnablesByPointerId.remove(pointerId)?.let(longPressHandler::removeCallbacks)
        lateinit var repeatRunnable: Runnable
        repeatRunnable = Runnable {
            val touch = activeKeyTouches[pointerId]
            if (
                repeatRunnablesByPointerId[pointerId] !== repeatRunnable ||
                touch == null ||
                touch.keyIndex != repeatingKeyIndex ||
                touch.backspaceGestureConsumed
            ) {
                stopLongPressAndRepeat(pointerId)
                return@Runnable
            }
            dispatchRepeatedKey(touch, key)
            val intervalMs = if (isBackspaceKey(touch.key.spec)) {
                backspaceRepeatProfile().intervalMs
            } else {
                KeytaoImeInteractionTuning.REPEATABLE_EDIT_INTERVAL_MS
            }
            longPressHandler.postDelayed(repeatRunnable, intervalMs)
        }
        repeatRunnablesByPointerId[pointerId] = repeatRunnable
        repeatRunnable.run()
    }

    private fun dispatchRepeatedKey(touch: KeyTouch, key: KeyRect) {
        val holdDurationMs = (SystemClock.uptimeMillis() - touch.downTimeMs).coerceAtLeast(0L)
        val command = if (
            isBackspaceKey(key.spec) &&
            BackspaceRepeatPolicy(backspaceRepeatProfile()).granularityAt(holdDurationMs) == BackspaceDeletionGranularity.SEGMENT
        ) {
            backspaceGestureCommand("deleteSegment")
        } else {
            resolveCommand(key.spec, 0f, key.rect, key.rect.centerY())
        }
        performConfiguredHaptic(soundEffect = keySoundEffect(command))
        clearRecentClipboardSuggestionForCommand(command)
        listener?.onKeyCommand(command)
    }

    private fun backspaceRepeatProfile(): BackspaceRepeatProfile {
        return KeytaoImeInteractionTuning.backspaceProfile(DeleteSpeed.fromSetting(config.deleteSpeed))
    }

    private fun findKey(x: Float, y: Float): KeyRect? {
        return keyRects.firstOrNull { key ->
            val insideVisibleScrollArea = key.sticky ||
                !usesCategorizedSymbolKeyboard() ||
                (y >= keyboardScrollViewportTop && y < keyboardScrollViewportBottom)
            insideVisibleScrollArea && key.rect.contains(x, y)
        }
    }

    private fun findKeyIndex(x: Float, y: Float): Int? {
        val index = keyRects.indexOfFirst { key ->
            val insideVisibleScrollArea = key.sticky ||
                !usesCategorizedSymbolKeyboard() ||
                (y >= keyboardScrollViewportTop && y < keyboardScrollViewportBottom)
            insideVisibleScrollArea && key.rect.contains(x, y)
        }
        return index.takeIf { it >= 0 }
    }

    private fun findCandidate(x: Float, y: Float): CandidateRect? {
        return candidateRects.firstOrNull { it.rect.contains(x, y) }
    }

    private fun findExpandedCandidate(x: Float, y: Float): CandidateRect? {
        return expandedCandidateRects.firstOrNull { it.rect.contains(x, y) }
    }

    private fun findClipboardDelete(x: Float, y: Float): ClipboardDeleteRect? {
        return clipboardDeleteRects.firstOrNull { it.rect.contains(x, y) }
    }

    private fun findToolbar(x: Float, y: Float): ToolbarRect? {
        return toolbarRects.firstOrNull { it.rect.contains(x, y) }
    }

    private fun shouldAcceptKeyRelease(touch: KeyTouch, x: Float, y: Float): Boolean {
        val key = touch.key
        if (isSpaceKey(key.spec) && hasActiveComposition()) return true
        if (key.rect.contains(x, y)) return true
        val deltaY = y - touch.downY
        if (abs(deltaY) < dp(config.swipeThresholdDp)) return false
        val horizontalLimit = max(touchSlop * 2f, key.rect.width() * 0.65f)
        return abs(x - touch.downX) <= horizontalLimit
    }

    private fun drawKeyShadow(canvas: Canvas, rect: RectF, pressed: Boolean) {
        drawSurfaceShadow(canvas, rect, pressed)
    }

    private fun drawSurfaceShadow(canvas: Canvas, rect: RectF, pressed: Boolean) {
        val shadow = RectF(rect)
        shadow.offset(0f, dp(if (pressed) 0.8f else 1.6f))
        paint.style = Paint.Style.FILL
        paint.color = Color.argb(if (pressed) 18 else 28, 26, 34, 44)
        canvas.drawRoundRect(shadow, dp(keyCornerRadiusDp()), dp(keyCornerRadiusDp()), paint)
    }

    private fun drawKeyOutline(canvas: Canvas, key: KeySpec, rect: RectF, pressed: Boolean) {
        if (pressed) return
        val inset = dp(1f)
        val outline = RectF(
            rect.left + inset,
            rect.top + inset,
            rect.right - inset,
            rect.bottom - inset,
        )
        paint.style = Paint.Style.STROKE
        paint.strokeWidth = max(1f, dp(0.7f))
        paint.color = if (isSoftAccentKey(key)) {
            Color.argb(if (isDarkPanel()) 72 else 46, theme.selectedLabelColor.red, theme.selectedLabelColor.green, theme.selectedLabelColor.blue)
        } else if (isDarkPanel()) {
            Color.argb(22, 255, 255, 255)
        } else {
            Color.argb(28, 26, 34, 44)
        }
        val radius = dp(max(0f, keyCornerRadiusDp() - 1f))
        canvas.drawRoundRect(outline, radius, radius, paint)
    }

    private fun keyBackgroundColor(key: KeySpec? = null): Int {
        if (isSoftAccentKey(key)) return softenedAccentSurfaceColor(0.16f)
        if (key?.style == "accent") return theme.candidateSelectedBackground.toArgb()
        if (theme.keyBackground.alpha > 0) return theme.keyBackground.toArgb()
        return if (isDarkPanel()) {
            Color.argb(170, 42, 48, 58)
        } else {
            Color.argb(210, 255, 255, 255)
        }
    }

    private fun keyForegroundColor(key: KeySpec, selected: Boolean): Int {
        return when {
            selected -> theme.keySelectedForeground.toArgb()
            key.style == "accent" -> theme.candidateSelectedForeground.toArgb()
            else -> theme.keyForeground.toArgb()
        }
    }

    private fun isSoftAccentKey(key: KeySpec?): Boolean {
        if (key == null) return false
        val type = actionForMode(key).type
        return key.style == "accent" ||
            isSoftAccentPunctuationKey(key) ||
            type == KeyCommandTypes.MODE ||
            type == KeyCommandTypes.KEYBOARD_MODE ||
            type == KeyCommandTypes.SPACE ||
            type == KeyCommandTypes.ENTER ||
            type == KeyCommandTypes.BACKSPACE
    }

    private fun isSoftAccentPunctuationKey(key: KeySpec): Boolean {
        val punctuation = setOf("，", "。", ",", ".")
        return labelForMode(key) in punctuation || valueForMode(key) in punctuation
    }

    private fun toolbarBackgroundColor(item: ToolbarRect, pressed: Boolean, forceAccent: Boolean = false): Int {
        val useAccent = forceAccent || item.selected || isSoftAccentToolbar(item)
        return when {
            pressed && useAccent -> softenedAccentSurfaceColor(0.24f)
            pressed -> theme.keySelectedBackground.toArgb()
            useAccent -> softenedAccentSurfaceColor(if (item.selected) 0.18f else 0.13f)
            item.selected -> theme.candidateSelectedBackground.toArgb()
            else -> keyBackgroundColor()
        }
    }

    private fun isSoftAccentToolbar(item: ToolbarRect): Boolean {
        if (item.command.type in setOf(
                KeyCommandTypes.MODE,
                KeyCommandTypes.OPEN_PAGE,
                KeyCommandTypes.KEYBOARD_MODE,
                KeyCommandTypes.KEYBOARD_PICKER,
                KeyCommandTypes.NEXT_INPUT_METHOD,
            )
        ) {
            return true
        }
        if (item.command.type == KeyCommandTypes.PANEL && item.command.value in setOf(
                "rime",
                "clipboard",
                "close",
                "dismissClipboard",
            )
        ) {
            return true
        }
        return false
    }

    private fun isToolbarPressed(item: ToolbarRect): Boolean {
        return pressedToolbar?.label == item.label && pressedToolbar?.command == item.command
    }

    private fun clearRecentClipboardSuggestionForCommand(command: KeyCommand) {
        if (command.type == KeyCommandTypes.SHIFT) return
        clearRecentClipboardSuggestion()
    }

    private fun panelBackgroundColor(): Int {
        return blendColor(
            theme.selectedLabelColor.toArgb(),
            theme.panelBackground.toArgb(),
            0.07f,
            theme.panelBackground.alpha,
        )
    }

    private fun statusMessageColor(): Int {
        return if (isDarkPanel()) {
            Color.argb(235, 245, 247, 250)
        } else {
            Color.argb(224, 31, 41, 51)
        }
    }

    private fun softenedAccentSurfaceColor(amount: Float): Int {
        return blendColor(
            theme.selectedLabelColor.toArgb(),
            panelBackgroundColor(),
            amount.coerceIn(0f, 1f),
        )
    }

    private fun blendColor(foreground: Int, background: Int, amount: Float, alpha: Int = Color.alpha(background)): Int {
        val ratio = amount.coerceIn(0f, 1f)
        val inverse = 1f - ratio
        return Color.argb(
            alpha.coerceIn(0, 255),
            (Color.red(foreground) * ratio + Color.red(background) * inverse).roundToInt().coerceIn(0, 255),
            (Color.green(foreground) * ratio + Color.green(background) * inverse).roundToInt().coerceIn(0, 255),
            (Color.blue(foreground) * ratio + Color.blue(background) * inverse).roundToInt().coerceIn(0, 255),
        )
    }

    private fun keyboardHorizontalGap(): Float {
        return dp(config.horizontalGapDp)
    }

    private fun keyboardVerticalGap(): Float {
        return dp(config.verticalGapDp)
    }

    private fun keyboardMaxKeyHeight(): Float {
        return dp(config.maxKeyHeightDp)
    }

    private fun rowWeight(row: List<KeySpec>): Float {
        return row.sumOf { it.weight.toDouble() }.toFloat().coerceAtLeast(1f)
    }

    private fun keyRowSpan(key: KeySpec): Int {
        return key.rowSpan.coerceIn(1, 8)
    }

    private fun advanceRowSpans(spans: List<ActiveRowSpan>): MutableList<ActiveRowSpan> {
        return spans.mapNotNull { span ->
            span.remainingRows -= 1
            if (span.remainingRows > 0) span else null
        }.toMutableList()
    }

    private fun keyboardOuterInset(): Float {
        return dp(config.outerInsetDp)
    }

    private fun keyboardReferenceUnitWidth(rows: List<List<KeySpec>>, horizontalGap: Float): Float {
        var activeLeadingSpans = mutableListOf<ActiveRowSpan>()
        var referenceKeyCount = 0
        var referenceWeight = 1f
        for (row in rows) {
            val effectiveKeyCount = activeLeadingSpans.size + row.size
            val effectiveWeight = (
                activeLeadingSpans.sumOf { it.weight.toDouble() }.toFloat() + rowWeight(row)
            ).coerceAtLeast(1f)
            if (effectiveKeyCount > referenceKeyCount ||
                (effectiveKeyCount == referenceKeyCount && effectiveWeight > referenceWeight)
            ) {
                referenceKeyCount = effectiveKeyCount
                referenceWeight = effectiveWeight
            }
            val nextLeadingSpans = row.takeWhile { keyRowSpan(it) > 1 }
                .map { ActiveRowSpan(it.weight, keyRowSpan(it) - 1) }
            activeLeadingSpans = advanceRowSpans(activeLeadingSpans)
            activeLeadingSpans.addAll(nextLeadingSpans)
        }
        if (referenceKeyCount <= 0) return dp(32f)
        val gapWidth = horizontalGap * (referenceKeyCount - 1).coerceAtLeast(0)
        val availableWidth = (width - keyboardOuterInset() * 2f - gapWidth).coerceAtLeast(1f)
        return (availableWidth / referenceWeight).coerceAtLeast(dp(24f))
    }

    private fun keyboardRowWidth(
        row: List<KeySpec>,
        rowIndex: Int,
        rows: List<List<KeySpec>>,
        referenceUnitWidth: Float,
        horizontalGap: Float,
        maximumRowWidth: Float,
        effectiveKeyCount: Int,
        effectiveWeight: Float,
    ): Float {
        if (keyboardRowShouldFillWidth(row, rowIndex, rows)) {
            return maximumRowWidth
        }
        val gapWidth = horizontalGap * (effectiveKeyCount - 1).coerceAtLeast(0)
        return (referenceUnitWidth * effectiveWeight + gapWidth).coerceAtMost(maximumRowWidth)
    }

    private fun keyboardRowShouldFillWidth(row: List<KeySpec>, rowIndex: Int, rows: List<List<KeySpec>>): Boolean {
        if (keyboardLayer != "letters") return true
        if (rowIndex == 0 || rowIndex == rows.lastIndex) return true
        if (row.size <= 5) return true
        return row.any { key ->
            val type = actionForMode(key).type
            type == KeyCommandTypes.SHIFT || type == KeyCommandTypes.BACKSPACE
        }
    }

    private fun isDarkPanel(): Boolean {
        val luminance = (theme.panelBackground.red * 299 + theme.panelBackground.green * 587 + theme.panelBackground.blue * 114) / 1000
        return luminance < 128
    }

    private fun textBaselineOffset(paint: Paint): Float {
        return -(paint.descent() + paint.ascent()) / 2f
    }

    /**
     * Key feedback has to follow the system switches, not only our own config:
     * "touch vibration" and "keypress sound" in Settings apply to every keyboard,
     * and a raw `Vibrator.vibrate` runs with USAGE_UNKNOWN, which bypasses them.
     */
    private fun performConfiguredHaptic(
        strong: Boolean = false,
        soundEffect: Int = AudioManager.FX_KEYPRESS_STANDARD,
        playSound: Boolean = true,
        hapticConstant: Int = if (strong) HapticFeedbackConstants.LONG_PRESS else HapticFeedbackConstants.KEYBOARD_RELEASE,
    ) {
        performConfiguredVibration(strong, hapticConstant)
        if (playSound) {
            playConfiguredKeySound(soundEffect)
        }
    }

    private fun performConfiguredKeyPress(command: KeyCommand) {
        performConfiguredHaptic(
            soundEffect = keySoundEffect(command),
            hapticConstant = HapticFeedbackConstants.KEYBOARD_PRESS,
        )
    }

    private fun performConfiguredSelectionFeedback(playSound: Boolean = true) {
        performConfiguredHaptic(
            strong = true,
            playSound = playSound,
            hapticConstant = HapticFeedbackConstants.LONG_PRESS,
        )
    }

    private fun performConfiguredVibration(strong: Boolean, hapticConstant: Int) {
        if (!config.hapticsEnabled || !systemSettingEnabled(Settings.System.HAPTIC_FEEDBACK_ENABLED)) {
            return
        }
        val deviceVibrator = vibrator
        if (deviceVibrator == null || !deviceVibrator.hasVibrator()) {
            performHapticFeedback(hapticConstant)
            return
        }
        val scaled = (config.hapticIntensity * if (strong) 3.0f else 2.55f).roundToInt()
        val amplitude = scaled.coerceIn(1, 255)
        val durationMs = if (strong) hapticMediumDurationMs else hapticLightDurationMs
        runCatching {
            val effect = VibrationEffect.createOneShot(durationMs, amplitude)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                deviceVibrator.vibrate(
                    effect,
                    VibrationAttributes.createForUsage(VibrationAttributes.USAGE_TOUCH),
                )
            } else {
                deviceVibrator.vibrate(effect)
            }
        }
    }

    private fun playConfiguredKeySound(soundEffect: Int) {
        if (!systemSettingEnabled(Settings.System.SOUND_EFFECTS_ENABLED)) return
        runCatching { audioManager?.playSoundEffect(soundEffect) }
    }

    private fun systemSettingEnabled(name: String): Boolean {
        return runCatching {
            Settings.System.getInt(context.contentResolver, name, 1) != 0
        }.getOrDefault(true)
    }

    private fun keySoundEffect(command: KeyCommand): Int = when (command.type) {
        KeyCommandTypes.BACKSPACE, KeyCommandTypes.BACKSPACE_GESTURE -> AudioManager.FX_KEYPRESS_DELETE
        KeyCommandTypes.ENTER -> AudioManager.FX_KEYPRESS_RETURN
        KeyCommandTypes.SPACE -> AudioManager.FX_KEYPRESS_SPACEBAR
        else -> AudioManager.FX_KEYPRESS_STANDARD
    }

    private fun isConfirmationCommand(command: KeyCommand): Boolean {
        return command.type in setOf(
            KeyCommandTypes.SHIFT,
            KeyCommandTypes.MODE,
            KeyCommandTypes.OPEN_PAGE,
            KeyCommandTypes.KEYBOARD_PICKER,
            KeyCommandTypes.NEXT_INPUT_METHOD,
            KeyCommandTypes.KEYBOARD_MODE,
            KeyCommandTypes.NEXT_PAGE,
            KeyCommandTypes.PREVIOUS_PAGE,
            KeyCommandTypes.RESET,
            KeyCommandTypes.RIME_MENU,
            KeyCommandTypes.RIME_SCHEMA,
            KeyCommandTypes.RIME_OPTION,
            KeyCommandTypes.PANEL,
            KeyCommandTypes.ONE_HANDED,
            KeyCommandTypes.FLOATING,
        )
    }

    private fun dp(value: Int): Float = dp(value.toFloat())

    private fun dp(value: Float): Float = value * resources.displayMetrics.density

    private fun sp(value: Float): Float = value * resources.displayMetrics.scaledDensity

    private val floatingOutlineProvider = object : ViewOutlineProvider() {
        override fun getOutline(view: View, outline: Outline) {
            outline.setRoundRect(
                0,
                0,
                view.width,
                view.height,
                dp(theme.panelCornerRadiusDp),
            )
        }
    }

    companion object {
        private const val keyPreviewMarginDp = 4f
        private const val keyPreviewMinimumWidthDp = 48f
        private const val keyPreviewMinimumHeightDp = 48f
        private const val keyPreviewMaximumHeightDp = 64f
        private const val keyPreviewKeyOverlapDp = 6f
        private const val keyPreviewTextSizeSp = 28f
        private const val alternatePanelMarginDp = 4f
        private const val alternatePanelGapDp = 6f
        private const val alternatePanelMinimumItemWidthDp = 40f
        private const val alternatePanelMinimumHeightDp = 44f
        private const val alternatePanelMaximumHeightDp = 56f
        private const val alternatePanelTextSizeSp = 20f
        private const val maxBackspaceGestureUnitsPerGesture = 96
        private const val contentTransitionDurationMs = 140L
        private const val expandedCandidateLoadDelayMs = 180L
        private const val hapticLightDurationMs = 8L
        private const val hapticMediumDurationMs = 18L
        private const val androidSystemBottomInsetDp = 48

        private val rimeOptionSpecs = listOf(
            RimeOptionSpec("ascii_mode", "英文模式", "英文", "中文"),
            RimeOptionSpec("ascii_punct", "标点", "英文标点", "中文标点"),
            RimeOptionSpec("full_shape", "全角模式", "全角", "半角"),
            RimeOptionSpec("simplification", "简体输出", "简体", "繁体"),
        )

        /** Virtual accessibility node id ranges, one block per hit-test list. */
        private const val accessibilityExpandNodeId = 1
        private const val accessibilityToolbarNodeBase = 1_000
        private const val accessibilityCandidateNodeBase = 2_000
        private const val accessibilityExpandedNodeBase = 3_000
        private const val accessibilityClipboardDeleteNodeBase = 4_000
        private const val accessibilityKeyNodeBase = 5_000
        private val whitespaceRegex = Regex("\\s+")
        private val repeatableEditVerbs = setOf(
            "cursorLeft",
            "cursorRight",
            "cursorUp",
            "cursorDown",
            "forwardDelete",
        )
    }
}
