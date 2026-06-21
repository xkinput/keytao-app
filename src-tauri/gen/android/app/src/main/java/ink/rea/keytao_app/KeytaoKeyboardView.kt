package ink.rea.keytao_app

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.RectF
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.AttributeSet
import android.os.VibrationEffect
import android.os.Vibrator
import android.view.MotionEvent
import android.view.View
import android.view.ViewConfiguration
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
    }

    private data class KeyRect(val spec: KeySpec, val rect: RectF)
    private data class CandidateRect(
        val index: Int,
        val rect: RectF,
        val global: Boolean = false,
        val command: KeyCommand? = null,
    )
    private data class CandidateDrawItem(
        val index: Int,
        val label: String,
        val text: String,
        val comment: String? = null,
        val selected: Boolean = false,
        val global: Boolean = false,
        val command: KeyCommand? = null,
    )
    private data class ToolbarAction(
        val label: String,
        val command: KeyCommand,
        val selected: Boolean = false,
        val secondaryLabel: String? = null,
    )
    private data class ToolbarRect(
        val label: String,
        val command: KeyCommand,
        val rect: RectF,
        val selected: Boolean = false,
        val secondaryLabel: String? = null,
    )
    private data class PanelItem(val label: String, val text: String, val command: KeyCommand, val comment: String? = null)
    private data class KeyboardLayoutCache(val signature: String, val keys: List<KeyRect>)
    private enum class KeyboardLayer { LETTERS, NUMBERS, SYMBOLS }
    private enum class ShiftState { OFF, ONCE, LOCKED }
    private enum class FunctionPanelMode { HOME, RIME, SELECTION, CLIPBOARD, EMOJI }

    var listener: Listener? = null

    private var config: KeytaoAndroidImeConfig = KeytaoAndroidImeConfig.load(context)
    private var theme: KeytaoImeTheme = KeytaoThemeResolver.resolve()
    private var state: KeytaoImeState = KeytaoImeState.empty()
    private var shiftState = ShiftState.OFF
    private var keyboardLayer = KeyboardLayer.LETTERS
    private var schemaReady = true
    private var statusMessage: String? = null
    private var keyRects: List<KeyRect> = emptyList()
    private var candidateRects: List<CandidateRect> = emptyList()
    private var expandedCandidateRects: List<CandidateRect> = emptyList()
    private var expandedCandidates: List<KeytaoCandidate> = emptyList()
    private var visibleCandidateGlobalIndexes: Set<Int> = emptySet()
    private var toolbarRects: List<ToolbarRect> = emptyList()
    private var candidateExpandRect: RectF? = null
    private var candidateScrollX = 0f
    private var candidateContentWidth = 0f
    private var candidateTouchActive = false
    private var candidateDragging = false
    private var candidatePanelExpanded = false
    private var functionPanelActive = false
    private var functionPanelMode = FunctionPanelMode.HOME
    private var candidateExpandPressed = false
    private var expandedTouchActive = false
    private var expandedDragging = false
    private var expandedCandidatesLoading = false
    private var clipboardItemsLoading = false
    private var clipboardItems: List<String> = emptyList()
    private var recentClipboardSuggestion: String? = null
    private var expandedCandidateScrollY = 0f
    private var expandedCandidateContentHeight = 0f
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
    private var pressedKey: KeyRect? = null
    private var pressedExpandedCandidate: CandidateRect? = null
    private var pressedToolbar: ToolbarRect? = null
    private var toolbarTouchActive = false
    private var downX = 0f
    private var downY = 0f
    private var lastShiftTapTimeMs = 0L
    private var longPressConsumed = false
    private var repeatingKey: KeyRect? = null
    private val longPressHandler = Handler(Looper.getMainLooper())
    private val longPressRunnable = Runnable {
        pressedKey?.let { key ->
            longPressConsumed = true
            performConfiguredHaptic(strong = true)
            if (isRepeatableKey(key.spec)) {
                startRepeatingKey(key)
            } else {
                val command = resolveLongPressCommand(key.spec)
                clearRecentClipboardSuggestionForCommand(command)
                listener?.onKeyCommand(command)
                clearOneShotShiftAfter(command)
            }
            invalidate()
        }
    }
    private val repeatRunnable = object : Runnable {
        override fun run() {
            val key = repeatingKey ?: return
            if (pressedKey?.spec != key.spec) return
            val command = resolveCommand(key.spec, 0f)
            clearRecentClipboardSuggestionForCommand(command)
            listener?.onKeyCommand(command)
            longPressHandler.postDelayed(this, backspaceRepeatIntervalMs)
        }
    }
    private val touchSlop = ViewConfiguration.get(context).scaledTouchSlop
    private val shiftDoubleTapTimeoutMs = ViewConfiguration.getDoubleTapTimeout().toLong()
    private val logoBitmap: Bitmap? = runCatching {
        BitmapFactory.decodeResource(resources, R.mipmap.ic_launcher_foreground)
    }.getOrNull()

    private val paint = Paint(Paint.ANTI_ALIAS_FLAG)
    private val textPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        textAlign = Paint.Align.CENTER
    }

    fun updateConfig(next: KeytaoAndroidImeConfig) {
        config = next
        invalidateKeyboardLayoutCache()
        invalidateExpandedCandidateItemsCache()
        resetCandidateTouch()
        resetCandidateScroll()
        requestLayout()
        invalidate()
    }

    fun updateTheme(next: KeytaoImeTheme) {
        theme = next
        candidateWidthCache.clear()
        invalidateKeyboardLayoutCache()
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
        state = next
        if (
            functionPanelActive &&
            functionPanelMode == FunctionPanelMode.RIME &&
            state.candidatePanel.candidates.isNotEmpty() &&
            expandedCandidates.isEmpty()
        ) {
            requestExpandedCandidatesAsync()
        }
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

    fun setKeyboardLayer(value: String?) {
        val nextLayer = when (value) {
            "numbers" -> KeyboardLayer.NUMBERS
            "symbols" -> KeyboardLayer.SYMBOLS
            else -> KeyboardLayer.LETTERS
        }
        val changed = nextLayer != keyboardLayer || candidatePanelExpanded
        keyboardLayer = nextLayer
        candidatePanelExpanded = false
        functionPanelActive = false
        functionPanelMode = FunctionPanelMode.HOME
        expandedCandidates = emptyList()
        cancelExpandedCandidateRequest()
        clipboardItemsLoading = false
        pressedKey = null
        pressedToolbar = null
        toolbarTouchActive = false
        stopLongPressAndRepeat()
        resetExpandedCandidateScroll()
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
        val desiredHeight = dp(config.keyboardHeightDp + config.candidateBarHeightDp + config.keyboardBottomInsetDp).toInt()
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
        toolbarRects = emptyList()
        candidateExpandRect = null
        drawBackground(canvas)
        drawCandidateBar(canvas)
        if (candidatePanelExpanded) {
            drawExpandedCandidatePanel(canvas)
        } else {
            drawKeyboard(canvas)
        }
    }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                downX = event.x
                downY = event.y
                candidateDownX = event.x
                candidateDownY = event.y
                candidateDownScrollX = candidateScrollX
                expandedDownY = event.y
                expandedDownScrollY = expandedCandidateScrollY
                val hasCandidates = state.candidatePanel.candidates.isNotEmpty()
                candidateExpandPressed = !functionPanelActive && hasCandidates && isInCandidateBar(event.y) &&
                    candidateExpandRect?.contains(event.x, event.y) == true
                val toolbar = if (isInCandidateBar(event.y) && (functionPanelActive || !hasCandidates)) {
                    findToolbar(event.x, event.y)
                } else {
                    null
                }
                pressedToolbar = toolbar
                toolbarTouchActive = toolbar != null
                candidateTouchActive = !functionPanelActive && !candidateExpandPressed && !toolbarTouchActive && isInCandidateBar(event.y) && hasCandidates
                expandedTouchActive = !candidateTouchActive && !candidateExpandPressed && isInExpandedCandidatePanel(event.y)
                candidateDragging = false
                expandedDragging = false
                stopLongPressAndRepeat()
                pressedExpandedCandidate = if (expandedTouchActive) findExpandedCandidate(event.x, event.y) else null
                pressedKey = if (candidateTouchActive || toolbarTouchActive || candidateExpandPressed || expandedTouchActive) {
                    null
                } else {
                    findKey(event.x, event.y)
                }
                longPressConsumed = false
                scheduleLongPress(pressedKey)
                invalidate()
                return true
            }
            MotionEvent.ACTION_MOVE -> {
                if (toolbarTouchActive) {
                    val toolbar = pressedToolbar
                    if (toolbar != null && !toolbar.rect.contains(event.x, event.y)) {
                        pressedToolbar = null
                        invalidate()
                    }
                    return true
                }
                if (candidateExpandPressed) {
                    invalidate()
                    return true
                }
                if (expandedTouchActive) {
                    val deltaY = event.y - expandedDownY
                    if (!expandedDragging && abs(deltaY) > touchSlop) {
                        expandedDragging = true
                        pressedExpandedCandidate = null
                    }
                    if (expandedDragging) {
                        expandedCandidateScrollY = (expandedDownScrollY - deltaY).coerceIn(0f, maxExpandedCandidateScroll())
                        invalidate()
                    }
                    return true
                }
                if (candidateTouchActive) {
                    val deltaX = event.x - candidateDownX
                    val deltaY = event.y - candidateDownY
                    if (!candidateDragging && (abs(deltaX) > touchSlop || abs(deltaY) > touchSlop)) {
                        candidateDragging = true
                    }
                    return true
                }
                val key = pressedKey
                if (key != null && !key.rect.contains(event.x, event.y)) {
                    stopLongPressAndRepeat()
                }
                return true
            }
            MotionEvent.ACTION_UP -> {
                stopLongPressAndRepeat()
                if (toolbarTouchActive) {
                    val toolbar = pressedToolbar
                    pressedToolbar = null
                    toolbarTouchActive = false
                    if (toolbar != null && toolbar.rect.contains(event.x, event.y)) {
                        handleToolbarCommand(toolbar.command)
                    }
                    invalidate()
                    return true
                }
                if (candidateExpandPressed) {
                    candidateExpandPressed = false
                    if (candidateExpandRect?.contains(event.x, event.y) == true) {
                        toggleCandidatePanel()
                        performConfiguredHaptic()
                    }
                    invalidate()
                    return true
                }
                if (expandedTouchActive) {
                    val candidate = pressedExpandedCandidate
                    expandedTouchActive = false
                    pressedExpandedCandidate = null
                    if (!expandedDragging && candidate != null && candidate.rect.contains(event.x, event.y)) {
                        val command = candidate.command
                        if (command != null) {
                            handlePanelCommand(command)
                        } else {
                            closeCandidatePanel()
                            performConfiguredHaptic()
                            listener?.onCandidate(candidate.index, candidate.global)
                        }
                    }
                    expandedDragging = false
                    invalidate()
                    return true
                }
                if (candidateTouchActive) {
                    val wasDragging = candidateDragging
                    resetCandidateTouch()
                    if (!wasDragging && abs(event.x - candidateDownX) <= touchSlop && abs(event.y - candidateDownY) <= touchSlop) {
                        findCandidate(event.x, event.y)?.let {
                            performConfiguredHaptic()
                            listener?.onCandidate(it.index, it.global)
                        }
                    }
                    invalidate()
                    return true
                }
                val key = pressedKey
                pressedKey = null
                if (key != null && shouldAcceptKeyRelease(key, event.x, event.y)) {
                    if (!longPressConsumed) {
                        val command = resolveCommand(key.spec, event.y - downY)
                        performConfiguredHaptic()
                        clearRecentClipboardSuggestionForCommand(command)
                        listener?.onKeyCommand(command)
                        clearOneShotShiftAfter(command)
                    }
                    invalidate()
                    return true
                }
                invalidate()
                return true
            }
            MotionEvent.ACTION_CANCEL -> {
                stopLongPressAndRepeat()
                resetCandidateTouch()
                resetExpandedCandidateTouch()
                pressedToolbar = null
                toolbarTouchActive = false
                candidateExpandPressed = false
                pressedKey = null
                invalidate()
                return true
            }
        }
        return true
    }

    private fun drawBackground(canvas: Canvas) {
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
        var x = leftPadding
        val centerY = barHeight / 2f
        val panelModel = state.candidatePanel
        val message = statusMessage?.takeIf { it.isNotBlank() }
        visibleCandidateGlobalIndexes = emptySet()

        if (!schemaReady || (message != null && panelModel.candidates.isEmpty() && panelModel.preedit.isNullOrEmpty())) {
            resetCandidateScroll()
            textPaint.textSize = sp(theme.preeditSizeSp)
            textPaint.color = theme.labelColor.toArgb()
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
            drawFunctionPanelBar(canvas, barHeight, leftPadding)
            return
        }

        if (panelModel.candidates.isEmpty()) {
            resetCandidateScroll()
            panelModel.preedit?.let { preedit ->
                textPaint.color = theme.labelColor.toArgb()
                textPaint.textSize = sp(theme.preeditSizeSp)
                textPaint.textAlign = Paint.Align.LEFT
                canvas.drawText(preedit, x, centerY + textBaselineOffset(textPaint), textPaint)
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

        resetCandidateScroll()
        val expandRect = drawCandidateExpandButton(canvas, barHeight, leftPadding)
        val maxRight = expandRect.left - gap
        val nextCandidateRects = mutableListOf<CandidateRect>()
        val nextVisibleGlobalIndexes = mutableSetOf<Int>()
        canvas.save()
        canvas.clipRect(0f, 0f, width.toFloat(), barHeight)

        val candidateHeight = minOf(dp(38f), barHeight - gap * 1.8f)
        val candidateTop = (barHeight - candidateHeight) / 2f
        for (candidate in panelModel.candidates) {
            val item = CandidateDrawItem(
                index = candidate.index,
                label = candidate.label,
                text = candidate.text,
                comment = candidate.comment,
                selected = candidate.selected,
            )
            val globalIndex = panelCandidateGlobalIndex(candidate.index)
            val requestedWidth = candidateWidth(item)
            if (x + requestedWidth > maxRight && nextCandidateRects.isNotEmpty()) break
            val rectRight = (x + requestedWidth).coerceAtMost(maxRight)
            if (rectRight <= x + dp(24f)) break
            val rect = RectF(x, candidateTop, rectRight, candidateTop + candidateHeight)
            drawCandidateOption(canvas, item, rect)
            nextCandidateRects.add(CandidateRect(item.index, rect, item.global))
            nextVisibleGlobalIndexes.add(globalIndex)
            x = rect.right + gap
        }
        canvas.restore()
        candidateRects = nextCandidateRects
        visibleCandidateGlobalIndexes = nextVisibleGlobalIndexes
        candidateContentWidth = width.toFloat()

    }

    private fun drawCandidateExpandButton(canvas: Canvas, barHeight: Float, leftPadding: Float): RectF {
        val size = minOf(dp(38f), barHeight - dp(10f))
        val left = width - leftPadding - size
        val top = (barHeight - size) / 2f
        val rect = RectF(left, top, left + size, top + size)
        candidateExpandRect = rect

        paint.style = Paint.Style.FILL
        paint.color = if (candidateExpandPressed) {
            theme.keySelectedBackground.toArgb()
        } else {
            keyBackgroundColor()
        }
        canvas.drawRoundRect(rect, dp(theme.keyCornerRadiusDp), dp(theme.keyCornerRadiusDp), paint)

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
        val rowHeight = dp(36f)
        val visibleRect = RectF(0f, top, width.toFloat(), bottom)
        val items = expandedCandidateItems()
        val nextRects = mutableListOf<CandidateRect>()

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
                    expandedCandidatesLoading && functionPanelMode == FunctionPanelMode.RIME -> "正在加载 Rime 选项"
                    expandedCandidatesLoading && functionPanelActive -> "正在加载功能"
                    expandedCandidatesLoading -> "正在加载候选"
                    functionPanelActive && functionPanelMode == FunctionPanelMode.CLIPBOARD -> "剪贴板为空"
                    functionPanelActive -> "暂无功能项"
                    else -> "没有更多候选"
                }
                canvas.drawText(message, width / 2f, top + panelHeight / 2f + textBaselineOffset(textPaint), textPaint)
            }
            for (item in items) {
                val chipWidth = candidateWidth(item)
                    .coerceAtLeast(dp(56f))
                    .coerceAtMost(right - left)
                if (x + chipWidth > right && x > left) {
                    x = left
                    y += rowHeight + gap
                }
                val rect = RectF(x, y, x + chipWidth, y + rowHeight)
                if (rect.bottom >= top && rect.top <= bottom) {
                    drawCandidateOption(canvas, item, rect)
                    nextRects.add(CandidateRect(item.index, rect, item.global, item.command))
                }
                contentBottom = max(contentBottom, rect.bottom + expandedCandidateScrollY)
                x = rect.right + gap
            }
            canvas.restore()

            expandedCandidateContentHeight = (contentBottom - top + gap).coerceAtLeast(panelHeight)
        }

        expandedCandidateRects = nextRects
        coerceExpandedCandidateScroll()
    }

    private fun expandedCandidateItems(): List<CandidateDrawItem> {
        val signature = expandedCandidateItemsSignature()
        if (signature == expandedCandidateItemsCacheSignature) {
            return expandedCandidateItemsCache
        }
        val items = if (functionPanelActive) {
            when (functionPanelMode) {
                FunctionPanelMode.HOME -> functionHomeItems()
                FunctionPanelMode.SELECTION -> selectionPanelItems()
                FunctionPanelMode.CLIPBOARD -> clipboardPanelItems()
                FunctionPanelMode.EMOJI -> emojiPanelItems()
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
        val all = expandedCandidates
            .takeIf { it.isNotEmpty() }
            ?: state.allCandidates.takeIf { it.isNotEmpty() }
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
                ?: state.allCandidates.takeIf { it.isNotEmpty() }
                ?: state.candidates
            appendCandidateListSignature(source)
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

    private fun functionHomeItems(): List<CandidateDrawItem> = panelItems(
        PanelItem("Rime", "方案/开关", KeyCommand.panel("rime")),
        PanelItem("粘贴", "当前剪贴板", KeyCommand.edit("paste")),
        PanelItem("Tab", "输入制表符", KeyCommand.edit("tab")),
        PanelItem("行首", "移动光标", KeyCommand.edit("lineStart")),
        PanelItem("行尾", "移动光标", KeyCommand.edit("lineEnd")),
    )

    private fun selectionPanelItems(): List<CandidateDrawItem> = panelItems(
        PanelItem("多选", "开始/结束", KeyCommand.edit("toggleSelection")),
        PanelItem("左选", "扩展一字", KeyCommand.edit("selectLeft")),
        PanelItem("右选", "扩展一字", KeyCommand.edit("selectRight")),
        PanelItem("全选", "选择全部", KeyCommand.edit("selectAll")),
        PanelItem("复制", "复制选区", KeyCommand.edit("copy")),
        PanelItem("剪切", "剪切选区", KeyCommand.edit("cut")),
        PanelItem("粘贴", "当前剪贴板", KeyCommand.edit("paste")),
        PanelItem("行首", "移动光标", KeyCommand.edit("lineStart")),
        PanelItem("行尾", "移动光标", KeyCommand.edit("lineEnd")),
        PanelItem("Tab", "输入制表符", KeyCommand.edit("tab")),
    )

    private fun clipboardPanelItems(): List<CandidateDrawItem> {
        val actions = mutableListOf(
            PanelItem("刷新", "读取系统剪贴板", KeyCommand.panel("clipboard")),
            PanelItem("粘贴", "当前剪贴板", KeyCommand.edit("paste")),
        )
        clipboardItems.forEachIndexed { index, text ->
            actions.add(
                PanelItem(
                    "剪贴 ${index + 1}",
                    text.take(32),
                    KeyCommand.directInput(text),
                )
            )
        }
        return panelItems(*actions.toTypedArray())
    }

    private fun emojiPanelItems(): List<CandidateDrawItem> = emojiChoices.mapIndexed { index, emoji ->
        CandidateDrawItem(
            index = -4000 - index,
            label = "",
            text = emoji,
            command = KeyCommand.directInput(emoji),
        )
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
        val selectedAccentSpace = if (item.selected) dp(4f) else 0f
        val width = labelWidth + textWidth + commentWidth + textGaps + selectedAccentSpace + dp(candidatePaddingXDp() * 2)
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

    private fun candidatePaddingXDp(): Float = min(theme.candidatePaddingXDp, 9f).coerceAtLeast(7f)

    private fun candidateInlineGapDp(): Float = min(theme.candidateInlineGapDp, 4f).coerceAtLeast(2f)

    private fun candidateCornerRadiusDp(): Float = min(theme.keyCornerRadiusDp, 8f).coerceAtLeast(6f)

    private fun drawCandidateOption(canvas: Canvas, item: CandidateDrawItem, rect: RectF) {
        val radius = dp(candidateCornerRadiusDp())
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

        if (item.selected) {
            paint.style = Paint.Style.FILL
            paint.color = theme.selectedLabelColor.toArgb()
            val accent = RectF(
                rect.left + dp(5f),
                rect.top + dp(6f),
                rect.left + dp(8f),
                rect.bottom - dp(6f),
            )
            canvas.drawRoundRect(accent, dp(2f), dp(2f), paint)
        }

        textPaint.textAlign = Paint.Align.LEFT
        var textX = rect.left + dp(candidatePaddingXDp()) + if (item.selected) dp(4f) else 0f
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

    private fun drawToolbar(canvas: Canvas, barHeight: Float, leftPadding: Float) {
        val logoSize = dp(30f)
        val logoGap = dp(8f)
        val logoLeft = width - leftPadding - logoSize
        val maxRight = logoLeft - logoGap
        val actions = toolbarActions()
        val rects = mutableListOf<ToolbarRect>()
        val gap = dp(6f)
        val chipHeight = minOf(dp(34f), barHeight - dp(12f))
        var x = leftPadding
        val top = (barHeight - chipHeight) / 2f

        for (action in actions) {
            val chipWidth = toolbarChipWidth(action)
            if (x + chipWidth > maxRight) break
            val rect = RectF(x, top, x + chipWidth, top + chipHeight)
            val toolbarRect = ToolbarRect(
                action.label,
                action.command,
                rect,
                action.selected,
                action.secondaryLabel,
            )
            drawToolbarChip(canvas, toolbarRect)
            rects.add(toolbarRect)
            x = rect.right + gap
        }

        toolbarRects = rects
        drawKeytaoLogo(canvas, barHeight, leftPadding)
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
        paint.style = Paint.Style.FILL
        paint.color = toolbarBackgroundColor(item, pressed, forceAccent = true)
        canvas.drawRoundRect(item.rect, dp(theme.keyCornerRadiusDp), dp(theme.keyCornerRadiusDp), paint)

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

    private fun drawFunctionPanelBar(canvas: Canvas, barHeight: Float, leftPadding: Float) {
        val chipHeight = minOf(dp(34f), barHeight - dp(12f))
        val top = (barHeight - chipHeight) / 2f
        val backWidth = dp(74f)
        val settingsWidth = dp(74f)
        val back = ToolbarRect(
            "返回",
            KeyCommand.panel("close"),
            RectF(leftPadding, top, leftPadding + backWidth, top + chipHeight),
        )
        val settings = ToolbarRect(
            "设置",
            KeyCommand(KeyCommandTypes.OPEN_PAGE, "settings"),
            RectF(width - leftPadding - settingsWidth, top, width - leftPadding, top + chipHeight),
        )
        toolbarRects = listOf(back, settings)
        drawToolbarChip(canvas, back)
        drawToolbarChip(canvas, settings)

        textPaint.textAlign = Paint.Align.CENTER
        textPaint.textSize = sp(theme.labelSizeSp)
        textPaint.color = theme.commentColor.toArgb()
        canvas.drawText(functionPanelTitle(), width / 2f, barHeight / 2f + textBaselineOffset(textPaint), textPaint)

        if (expandedCandidatesLoading || clipboardItemsLoading) {
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
        paint.style = Paint.Style.FILL
        paint.color = toolbarBackgroundColor(item, pressed, forceAccent)
        canvas.drawRoundRect(item.rect, dp(theme.keyCornerRadiusDp), dp(theme.keyCornerRadiusDp), paint)

        if (item.selected) {
            paint.style = Paint.Style.STROKE
            paint.strokeWidth = dp(theme.candidateBorderWidthDp.coerceAtLeast(1f))
            paint.color = theme.candidateSelectedBorderColor.toArgb()
            canvas.drawRoundRect(item.rect, dp(theme.keyCornerRadiusDp), dp(theme.keyCornerRadiusDp), paint)
        }

        textPaint.textAlign = Paint.Align.CENTER
        val secondary = item.secondaryLabel?.takeIf { it.isNotBlank() }
        if (secondary == null) {
            textPaint.textSize = sp(theme.labelSizeSp)
            textPaint.color = when {
                pressed -> theme.keySelectedForeground.toArgb()
                item.selected -> theme.candidateSelectedForeground.toArgb()
                else -> theme.keyForeground.toArgb()
            }
            canvas.drawText(item.label, item.rect.centerX(), item.rect.centerY() + textBaselineOffset(textPaint), textPaint)
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

    private fun drawKeytaoLogo(canvas: Canvas, barHeight: Float, leftPadding: Float) {
        val size = dp(30f)
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
            for (keyRect in layout) {
                drawKey(canvas, keyRect.spec, keyRect.rect, pressedKey?.spec == keyRect.spec)
            }
        }

        keyRects = layout
    }

    private fun drawKey(canvas: Canvas, key: KeySpec, rect: RectF, pressed: Boolean) {
        val keyRect = RectF(rect)
        if (pressed) {
            keyRect.offset(0f, dp(1f))
        }
        val selected = pressed || isActiveKey(key)
        drawKeyShadow(canvas, keyRect, pressed)

        paint.style = Paint.Style.FILL
        paint.color = when {
            selected && isSoftAccentKey(key) -> softenedAccentSurfaceColor(0.24f)
            selected -> theme.keySelectedBackground.toArgb()
            else -> keyBackgroundColor(key)
        }
        canvas.drawRoundRect(keyRect, dp(theme.keyCornerRadiusDp), dp(theme.keyCornerRadiusDp), paint)
        drawKeyOutline(canvas, key, keyRect, pressed)

        val label = displayLabel(key)
        textPaint.textAlign = Paint.Align.CENTER
        var labelSize = sp(if (label.length > 2) theme.labelSizeSp else theme.fontSizeSp)
        textPaint.textSize = labelSize
        val maxLabelWidth = keyRect.width() - dp(10f)
        while (labelSize > sp(12f) && textPaint.measureText(label) > maxLabelWidth) {
            labelSize -= dp(1f)
            textPaint.textSize = labelSize
        }
        textPaint.color = keyForegroundColor(key, selected)
        canvas.drawText(label, keyRect.centerX(), keyRect.centerY() + textBaselineOffset(textPaint), textPaint)

        key.hint?.let { hint ->
            textPaint.textAlign = Paint.Align.RIGHT
            textPaint.textSize = sp(theme.commentSizeSp)
            textPaint.color = theme.commentColor.toArgb()
            canvas.drawText(hint, keyRect.right - dp(7f), keyRect.top + dp(15f), textPaint)
        }
    }

    private fun resolveCommand(key: KeySpec, deltaY: Float): KeyCommand {
        val threshold = dp(config.swipeThresholdDp)
        val command = when {
            deltaY < -threshold -> resolveSwipeUpCommand(key)
            deltaY > threshold -> key.swipeDown ?: key.action
            else -> actionForMode(key)
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
        return when (keyboardLayer) {
            KeyboardLayer.NUMBERS -> config.numberRows
            KeyboardLayer.SYMBOLS -> config.symbolRows
            KeyboardLayer.LETTERS -> config.rows
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
        val naturalRowHeight = ((availableHeight - verticalGapFloor * (rowCount + 1)) / rowCount)
            .coerceAtLeast(dp(36f))
        val rowHeight = min(naturalRowHeight, keyboardMaxKeyHeight())
        val verticalGap = ((availableHeight - rowHeight * rowCount) / (rowCount + 1))
            .coerceAtLeast(verticalGapFloor)
        val nextRects = mutableListOf<KeyRect>()

        var y = top + verticalGap
        for (row in rows) {
            val totalWeight = row.sumOf { it.weight.toDouble() }.toFloat().coerceAtLeast(1f)
            val usableWidth = width - horizontalGap * (row.size + 1)
            var x = horizontalGap
            for (key in row) {
                val keyWidth = usableWidth * key.weight / totalWeight
                val rect = RectF(x, y, x + keyWidth, y + rowHeight)
                nextRects.add(KeyRect(key, rect))
                x = rect.right + horizontalGap
            }
            y += rowHeight + verticalGap
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
        }
    }

    private fun invalidateKeyboardLayoutCache() {
        keyboardLayoutCache = KeyboardLayoutCache("", emptyList())
    }

    private fun actionForMode(key: KeySpec): KeyCommand {
        if (state.asciiMode) {
            key.asciiAction?.let { return it }
            key.asciiValue?.let { return KeyCommand.input(it) }
        } else {
            key.rimeValue?.let { return KeyCommand.rimeInput(it, key.value) }
            key.asciiValue?.takeIf { it != key.value }?.let { return KeyCommand.rimeInput(it, key.value) }
        }
        return key.action
    }

    private fun labelForMode(key: KeySpec): String {
        return if (state.asciiMode) {
            key.asciiLabel ?: key.asciiValue ?: key.label
        } else {
            key.label
        }
    }

    private fun valueForMode(key: KeySpec): String {
        return if (state.asciiMode) {
            key.asciiValue ?: key.value
        } else {
            key.value
        }
    }

    private fun toolbarActions(): List<ToolbarAction> {
        val function = ToolbarAction("功能", KeyCommand.panel("home"))
        return if (keyboardLayer == KeyboardLayer.SYMBOLS) {
            listOf(
                function,
                ToolbarAction("中文", KeyCommand(KeyCommandTypes.MODE, "chinese"), selected = !state.asciiMode),
                ToolbarAction("英文", KeyCommand(KeyCommandTypes.MODE, "ascii"), selected = state.asciiMode),
                ToolbarAction("123", KeyCommand(KeyCommandTypes.KEYBOARD_MODE, "numbers")),
                ToolbarAction("ABC", KeyCommand(KeyCommandTypes.KEYBOARD_MODE, "letters")),
            )
        } else {
            val currentMode = if (state.asciiMode) "英" else "中"
            val nextMode = if (state.asciiMode) "中" else "英"
            listOf(
                function,
                ToolbarAction(
                    currentMode,
                    KeyCommand(KeyCommandTypes.MODE),
                    secondaryLabel = nextMode,
                ),
                ToolbarAction("选择", KeyCommand.panel("selection")),
                ToolbarAction("剪贴板", KeyCommand.panel("clipboard")),
                ToolbarAction("Emoji", KeyCommand.panel("emoji")),
            )
        }
    }

    private fun functionPanelTitle(): String {
        return when (functionPanelMode) {
            FunctionPanelMode.HOME -> "功能"
            FunctionPanelMode.RIME -> "Rime"
            FunctionPanelMode.SELECTION -> "选择"
            FunctionPanelMode.CLIPBOARD -> "剪贴板"
            FunctionPanelMode.EMOJI -> "Emoji"
        }
    }

    private fun isInCandidateBar(y: Float): Boolean {
        return y >= 0f && y < dp(config.candidateBarHeightDp)
    }

    private fun isInExpandedCandidatePanel(y: Float): Boolean {
        val top = dp(config.candidateBarHeightDp)
        return candidatePanelExpanded && y >= top && y < keyboardBottom()
    }

    private fun expandedCandidatePanelHeight(): Float {
        return if (candidatePanelExpanded && (functionPanelActive || state.candidatePanel.candidates.isNotEmpty() || expandedCandidatesLoading)) {
            (keyboardBottom() - dp(config.candidateBarHeightDp)).coerceAtLeast(0f)
        } else {
            0f
        }
    }

    private fun keyboardTop(): Float {
        return dp(config.candidateBarHeightDp)
    }

    private fun keyboardBottom(): Float {
        return height.toFloat() - bottomReservedInset()
    }

    private fun bottomReservedInset(): Float {
        val requested = dp(config.keyboardBottomInsetDp)
        val minKeyboardContentHeight = dp(180f)
        val available = (height.toFloat() - keyboardTop() - minKeyboardContentHeight).coerceAtLeast(0f)
        return min(requested, available)
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
        functionPanelActive = false
        candidatePanelExpanded = true
        expandedCandidates = emptyList()
        pressedKey = null
        pressedToolbar = null
        toolbarTouchActive = false
        resetExpandedCandidateScroll()
        requestExpandedCandidatesAsync()
        startContentTransition()
    }

    private fun closeCandidatePanel() {
        if (!candidatePanelExpanded && expandedCandidates.isEmpty() && !functionPanelActive) return
        candidatePanelExpanded = false
        functionPanelActive = false
        functionPanelMode = FunctionPanelMode.HOME
        recentClipboardSuggestion = null
        expandedCandidates = emptyList()
        cancelExpandedCandidateRequest()
        clipboardItemsLoading = false
        resetExpandedCandidateTouch()
        resetExpandedCandidateScroll()
        startContentTransition()
    }

    private fun openFunctionPanel(mode: FunctionPanelMode = FunctionPanelMode.HOME) {
        functionPanelActive = true
        candidatePanelExpanded = true
        functionPanelMode = mode
        expandedCandidates = emptyList()
        cancelExpandedCandidateRequest()
        clipboardItemsLoading = mode == FunctionPanelMode.CLIPBOARD
        pressedKey = null
        pressedToolbar = null
        toolbarTouchActive = false
        resetExpandedCandidateScroll()
        if (mode == FunctionPanelMode.RIME) {
            requestExpandedCandidatesAsync()
        }
        if (mode == FunctionPanelMode.CLIPBOARD) {
            requestClipboardItemsAsync()
        }
        startContentTransition()
    }

    private fun handleToolbarCommand(command: KeyCommand) {
        if (handlePanelCommand(command)) {
            return
        }
        if (command.type == KeyCommandTypes.EDIT && command.value == "pasteText") {
            clearRecentClipboardSuggestion()
        }
        performConfiguredHaptic()
        listener?.onKeyCommand(command)
    }

    private fun handlePanelCommand(command: KeyCommand): Boolean {
        if (command.type == KeyCommandTypes.PANEL) {
            when (command.value) {
                "close" -> closeCandidatePanel()
                "dismissClipboard" -> clearRecentClipboardSuggestion()
                "home", null -> openFunctionPanel(FunctionPanelMode.HOME)
                "rime" -> {
                    openFunctionPanel(FunctionPanelMode.RIME)
                    listener?.onKeyCommand(KeyCommand(KeyCommandTypes.RIME_MENU))
                }
                "selection" -> openFunctionPanel(FunctionPanelMode.SELECTION)
                "clipboard" -> openFunctionPanel(FunctionPanelMode.CLIPBOARD)
                "emoji" -> openFunctionPanel(FunctionPanelMode.EMOJI)
                else -> openFunctionPanel(FunctionPanelMode.HOME)
            }
            performConfiguredHaptic()
            invalidate()
            return true
        }
        performConfiguredHaptic()
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

        state.allCandidates.takeIf { it.isNotEmpty() }?.let { candidates ->
            expandedCandidates = candidates
            expandedCandidatesLoading = false
            coerceExpandedCandidateScroll()
            invalidate()
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
        return !functionPanelActive || functionPanelMode == FunctionPanelMode.RIME
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
    }

    private fun resetExpandedCandidateTouch() {
        expandedTouchActive = false
        expandedDragging = false
        pressedExpandedCandidate = null
    }

    private fun resetCandidateScroll() {
        candidateScrollX = 0f
        candidateContentWidth = width.toFloat()
    }

    private fun resetExpandedCandidateScroll() {
        expandedCandidateScrollY = 0f
        expandedCandidateContentHeight = expandedCandidatePanelHeight()
    }

    private fun maxCandidateScroll(): Float {
        return max(0f, candidateContentWidth - width.toFloat())
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

    private fun stopLongPressAndRepeat() {
        longPressHandler.removeCallbacks(longPressRunnable)
        longPressHandler.removeCallbacks(repeatRunnable)
        repeatingKey = null
    }

    private fun isRepeatableKey(key: KeySpec): Boolean {
        return actionForMode(key).type == KeyCommandTypes.BACKSPACE
    }

    private fun startRepeatingKey(key: KeyRect) {
        repeatingKey = key
        val command = resolveCommand(key.spec, 0f)
        clearRecentClipboardSuggestionForCommand(command)
        listener?.onKeyCommand(command)
        longPressHandler.removeCallbacks(repeatRunnable)
        longPressHandler.postDelayed(repeatRunnable, backspaceRepeatIntervalMs)
    }

    private fun scheduleLongPress(key: KeyRect?) {
        longPressHandler.removeCallbacks(longPressRunnable)
        val spec = key?.spec ?: return
        val hasLongPressAction = isRepeatableKey(spec) || spec.longPress != null || !spec.hint.isNullOrBlank()
        if (hasLongPressAction) {
            longPressHandler.postDelayed(longPressRunnable, longPressDelayMs)
        }
    }

    private fun findKey(x: Float, y: Float): KeyRect? {
        return keyRects.firstOrNull { it.rect.contains(x, y) }
    }

    private fun findCandidate(x: Float, y: Float): CandidateRect? {
        return candidateRects.firstOrNull { it.rect.contains(x, y) }
    }

    private fun findExpandedCandidate(x: Float, y: Float): CandidateRect? {
        return expandedCandidateRects.firstOrNull { it.rect.contains(x, y) }
    }

    private fun findToolbar(x: Float, y: Float): ToolbarRect? {
        return toolbarRects.firstOrNull { it.rect.contains(x, y) }
    }

    private fun shouldAcceptKeyRelease(key: KeyRect, x: Float, y: Float): Boolean {
        if (key.rect.contains(x, y)) return true
        val deltaY = y - downY
        if (abs(deltaY) < dp(config.swipeThresholdDp)) return false
        val horizontalLimit = max(touchSlop * 2f, key.rect.width() * 0.65f)
        return abs(x - downX) <= horizontalLimit
    }

    private fun drawKeyShadow(canvas: Canvas, rect: RectF, pressed: Boolean) {
        val shadow = RectF(rect)
        shadow.offset(0f, dp(if (pressed) 0.8f else 2.4f))
        paint.style = Paint.Style.FILL
        paint.color = Color.argb(if (pressed) 26 else 44, 26, 34, 44)
        canvas.drawRoundRect(shadow, dp(theme.keyCornerRadiusDp), dp(theme.keyCornerRadiusDp), paint)
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
        val radius = dp(max(0f, theme.keyCornerRadiusDp - 1f))
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
        if (item.command.type == KeyCommandTypes.MODE) return true
        if (item.command.type == KeyCommandTypes.PANEL && item.command.value in setOf("home", "close", "dismissClipboard")) {
            return true
        }
        if (item.command.type == KeyCommandTypes.OPEN_PAGE) return true
        return item.label in setOf("功能", "中", "英", "中文", "英文", "返回", "设置")
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
        return dp((theme.panelGapDp * 0.88f).coerceAtLeast(4f))
    }

    private fun keyboardVerticalGap(): Float {
        return dp((theme.panelGapDp * 1.1f).coerceAtLeast(6f))
    }

    private fun keyboardMaxKeyHeight(): Float {
        return dp(48f)
    }

    private fun isDarkPanel(): Boolean {
        val luminance = (theme.panelBackground.red * 299 + theme.panelBackground.green * 587 + theme.panelBackground.blue * 114) / 1000
        return luminance < 128
    }

    private fun textBaselineOffset(paint: Paint): Float {
        return -(paint.descent() + paint.ascent()) / 2f
    }

    private fun performConfiguredHaptic(strong: Boolean = false) {
        if (!config.hapticsEnabled) return
        val deviceVibrator = vibrator ?: return
        if (!deviceVibrator.hasVibrator()) return
        val scaled = (config.hapticIntensity * if (strong) 3.0f else 2.55f).roundToInt()
        val amplitude = scaled.coerceIn(1, 255)
        val durationMs = if (strong) 18L else 8L
        runCatching {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                deviceVibrator.vibrate(VibrationEffect.createOneShot(durationMs, amplitude))
            } else {
                @Suppress("DEPRECATION")
                deviceVibrator.vibrate(durationMs)
            }
        }
    }

    private fun dp(value: Int): Float = dp(value.toFloat())

    private fun dp(value: Float): Float = value * resources.displayMetrics.density

    private fun sp(value: Float): Float = value * resources.displayMetrics.scaledDensity

    companion object {
        private const val longPressDelayMs = 420L
        private const val backspaceRepeatIntervalMs = 72L
        private const val contentTransitionDurationMs = 140L
        private const val expandedCandidateLoadDelayMs = 180L
        private val whitespaceRegex = Regex("\\s+")
        private val emojiChoices = listOf(
            "😀", "😁", "😂", "🤣", "😊", "😍", "😘", "😎",
            "🥰", "😇", "🙂", "😉", "😋", "🤔", "😭", "😡",
            "👍", "👎", "👌", "🙏", "👏", "💪", "🔥", "✨",
            "🎉", "❤️", "💔", "⭐", "🌟", "✅", "❌", "❓",
            "☕", "🍵", "🍻", "🍚", "🍜", "🌙", "☀️", "🌧️",
        )
    }
}
