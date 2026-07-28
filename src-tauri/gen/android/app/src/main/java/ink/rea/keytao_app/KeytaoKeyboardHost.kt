package ink.rea.keytao_app

import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Path
import android.graphics.Rect
import android.graphics.Region
import android.graphics.RectF
import android.os.Build
import android.view.MotionEvent
import android.view.View
import android.view.ViewConfiguration
import android.view.WindowInsets
import android.widget.FrameLayout
import kotlin.math.abs
import kotlin.math.max
import kotlin.math.roundToInt

class KeytaoKeyboardHost(context: Context) : FrameLayout(context) {
    interface Listener {
        fun onLayoutStateChanged(state: KeyboardLayoutState, finished: Boolean)

        /** Real bottom avoidance for this window, in pixels. */
        fun onSystemBottomInsetChanged(insetPx: Int)
    }

    private enum class DragMode {
        NONE,
        MOVE,
        RESIZE_LEFT,
        RESIZE_TOP,
        RESIZE_RIGHT,
        RESIZE_BOTTOM,
        RESIZE_TOP_LEFT,
        RESIZE_TOP_RIGHT,
        RESIZE_BOTTOM_LEFT,
        RESIZE_BOTTOM_RIGHT,
    }

    var listener: Listener? = null

    private var layoutState = KeyboardLayoutState(
        mode = KeyboardLayoutMode.FULL,
        floatingScale = 1f,
    )
    private var marginPx = 0
    private var normalHeightPx = 0
    private var safeBottomInsetPx = 0
    private var reportedBottomInsetPx = -1
    private var isLandscape = false
    private var dragMode = DragMode.NONE
    private var dragStartX = 0f
    private var dragStartY = 0f
    private var dragStartScale = 1f
    private var dragStartRect = RectF()
    private var dragHasMoved = false
    private val edgeTouchSizePx = 18f * resources.displayMetrics.density
    private val touchSlopPx = ViewConfiguration.get(context).scaledTouchSlop.toFloat()
    private val sideSwitchView = KeyboardSideSwitchView(context).apply {
        visibility = View.GONE
        setOnClickListener {
            if (layoutState.mode != KeyboardLayoutMode.ONE_HANDED) return@setOnClickListener
            layoutState = layoutState.copy(oneHandedSide = layoutState.oneHandedSide.opposite)
            listener?.onLayoutStateChanged(layoutState, true)
            requestLayout()
        }
    }
    private val floatingControlsView = FloatingControlCapsuleView(context).apply {
        visibility = View.GONE
    }
    private val hideFloatingControls = Runnable {
        floatingControlsView.visibility = View.GONE
        requestLayout()
    }

    init {
        floatingControlsView.setOnClickListener {
            if (layoutState.mode != KeyboardLayoutMode.FLOATING) return@setOnClickListener
            removeCallbacks(hideFloatingControls)
            floatingControlsView.visibility = View.GONE
            layoutState = layoutState.copy(mode = KeyboardLayoutMode.FULL)
            listener?.onLayoutStateChanged(layoutState, true)
            requestLayout()
        }
        setBackgroundColor(Color.TRANSPARENT)
        clipChildren = false
        clipToPadding = false
        addView(
            sideSwitchView,
            LayoutParams(
                (48f * resources.displayMetrics.density).roundToInt(),
                (48f * resources.displayMetrics.density).roundToInt(),
            ),
        )
        addView(
            floatingControlsView,
            LayoutParams(
                (52f * resources.displayMetrics.density).roundToInt(),
                (36f * resources.displayMetrics.density).roundToInt(),
            ),
        )
    }

    /**
     * An IME window is not padded for the navigation bar or the gesture handle,
     * so the keyboard has to read the real bottom inset itself instead of
     * assuming the three-button 48dp.
     */
    override fun onApplyWindowInsets(insets: WindowInsets): WindowInsets {
        val bottom = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            insets.getInsets(
                WindowInsets.Type.navigationBars() or WindowInsets.Type.systemGestures()
            ).bottom
        } else {
            @Suppress("DEPRECATION")
            insets.systemWindowInsetBottom
        }
        if (bottom != reportedBottomInsetPx) {
            reportedBottomInsetPx = bottom
            listener?.onSystemBottomInsetChanged(bottom)
        }
        return super.onApplyWindowInsets(insets)
    }

    fun updatePresentation(
        nextState: KeyboardLayoutState,
        marginDp: Float,
        normalHeightDp: Float,
        safeBottomInsetDp: Float,
        isLandscape: Boolean,
        theme: KeytaoImeTheme,
    ) {
        this.isLandscape = isLandscape
        layoutState = nextState.normalized(
            allowOneHanded = !isLandscape,
            isLandscape = isLandscape,
        )
        marginPx = (marginDp.coerceIn(0f, 24f) * resources.displayMetrics.density).roundToInt()
        normalHeightPx = (normalHeightDp.coerceAtLeast(1f) * resources.displayMetrics.density).roundToInt()
        safeBottomInsetPx = (safeBottomInsetDp.coerceIn(0f, 80f) * resources.displayMetrics.density).roundToInt()
        sideSwitchView.updateTheme(theme)
        floatingControlsView.updateTheme(theme)
        sideSwitchView.destination = layoutState.oneHandedSide.opposite
        if (layoutState.mode != KeyboardLayoutMode.FLOATING) {
            removeCallbacks(hideFloatingControls)
            floatingControlsView.visibility = View.GONE
        }
        requestLayout()
    }

    fun populateFloatingTouchableRegion(outRegion: Region): Boolean {
        if (layoutState.mode != KeyboardLayoutMode.FLOATING || width <= 0 || height <= 0) {
            return false
        }
        val child = getChildAt(0) ?: return false
        val left = (child.left - edgeTouchSizePx).roundToInt().coerceIn(0, width)
        val top = (child.top - edgeTouchSizePx).roundToInt().coerceIn(0, height)
        val right = (child.right + edgeTouchSizePx).roundToInt().coerceIn(left, width)
        val bottom = (child.bottom + edgeTouchSizePx).roundToInt().coerceIn(top, height)
        outRegion.set(left, top, right, bottom)
        if (floatingControlsView.visibility == View.VISIBLE) {
            outRegion.union(Rect(
                floatingControlsView.left,
                floatingControlsView.top,
                floatingControlsView.right,
                floatingControlsView.bottom,
            ))
        }
        return true
    }

    override fun onMeasure(widthMeasureSpec: Int, heightMeasureSpec: Int) {
        val child = getChildAt(0)
        if (child == null) {
            setMeasuredDimension(
                resolveSize(suggestedMinimumWidth, widthMeasureSpec),
                resolveSize(suggestedMinimumHeight, heightMeasureSpec),
            )
            return
        }

        val width = MeasureSpec.getSize(widthMeasureSpec)
        val compact = layoutState.mode != KeyboardLayoutMode.FULL
        val horizontalInsets = paddingLeft + paddingRight + if (compact) marginPx * 2 else 0
        val availableWidth = (width - horizontalInsets).coerceAtLeast(1)
        val targetWidth = if (compact) {
            (width * layoutState.activeScale).roundToInt().coerceAtMost(availableWidth)
        } else {
            availableWidth
        }
        val availableHeight = MeasureSpec.getSize(heightMeasureSpec).takeIf { it > 0 } ?: normalHeightPx
        child.measure(
            MeasureSpec.makeMeasureSpec(targetWidth, MeasureSpec.EXACTLY),
            MeasureSpec.makeMeasureSpec(availableHeight.coerceAtLeast(1), MeasureSpec.AT_MOST),
        )

        val desiredHeight = layoutState.resolveHostHeight(
            availableHeight = availableHeight,
            normalHeight = normalHeightPx,
            childHeight = child.measuredHeight + if (layoutState.mode == KeyboardLayoutMode.FULL) {
                paddingTop + paddingBottom
            } else {
                0
            },
            margin = marginPx,
        )
        sideSwitchView.measure(
            MeasureSpec.makeMeasureSpec(sideSwitchView.layoutParams.width, MeasureSpec.EXACTLY),
            MeasureSpec.makeMeasureSpec(sideSwitchView.layoutParams.height, MeasureSpec.EXACTLY),
        )
        floatingControlsView.measure(
            MeasureSpec.makeMeasureSpec(floatingControlsView.layoutParams.width, MeasureSpec.EXACTLY),
            MeasureSpec.makeMeasureSpec(floatingControlsView.layoutParams.height, MeasureSpec.EXACTLY),
        )
        setMeasuredDimension(
            resolveSize(width, widthMeasureSpec),
            resolveSize(desiredHeight, heightMeasureSpec),
        )
    }

    override fun onLayout(changed: Boolean, left: Int, top: Int, right: Int, bottom: Int) {
        val child = getChildAt(0) ?: return
        when (layoutState.mode) {
            KeyboardLayoutMode.FULL -> {
                sideSwitchView.visibility = View.GONE
                floatingControlsView.visibility = View.GONE
                child.layout(
                    paddingLeft,
                    paddingTop,
                    paddingLeft + child.measuredWidth,
                    paddingTop + child.measuredHeight,
                )
            }
            KeyboardLayoutMode.ONE_HANDED -> {
                floatingControlsView.visibility = View.GONE
                val bounds = availableBounds()
                val childLeft = if (layoutState.oneHandedSide == KeyboardSide.LEFT) {
                    bounds.left
                } else {
                    bounds.right - child.measuredWidth
                }
                val childTop = bounds.bottom - child.measuredHeight
                child.layout(
                    childLeft.roundToInt(),
                    childTop.roundToInt(),
                    (childLeft + child.measuredWidth).roundToInt(),
                    (childTop + child.measuredHeight).roundToInt(),
                )
                layoutSideSwitch(childRect())
            }
            KeyboardLayoutMode.FLOATING -> {
                sideSwitchView.visibility = View.GONE
                val bounds = availableBounds()
                val horizontalSpace = (bounds.width() - child.measuredWidth).coerceAtLeast(0f)
                val verticalSpace = (bounds.height() - child.measuredHeight).coerceAtLeast(0f)
                val childLeft = bounds.left + horizontalSpace * layoutState.floatingHorizontalPosition
                val childTop = bounds.top + verticalSpace * layoutState.floatingVerticalPosition
                child.layout(
                    childLeft.roundToInt(),
                    childTop.roundToInt(),
                    (childLeft + child.measuredWidth).roundToInt(),
                    (childTop + child.measuredHeight).roundToInt(),
                )
                layoutFloatingControls(childRect())
            }
        }
    }

    override fun onInterceptTouchEvent(event: MotionEvent): Boolean {
        if (layoutState.mode != KeyboardLayoutMode.FLOATING) return false
        return when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                if (isInsideFloatingControls(event.x, event.y)) {
                    return false
                }
                dragMode = dragModeAt(event.x, event.y)
                if (dragMode != DragMode.NONE) {
                    beginDrag(event)
                    true
                } else {
                    false
                }
            }
            else -> dragMode != DragMode.NONE
        }
    }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        if (layoutState.mode != KeyboardLayoutMode.FLOATING || dragMode == DragMode.NONE) return false
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> beginDrag(event)
            MotionEvent.ACTION_MOVE -> updateDrag(event, finished = false)
            MotionEvent.ACTION_UP -> {
                updateDrag(event, finished = true)
                dragMode = DragMode.NONE
            }
            MotionEvent.ACTION_CANCEL -> {
                listener?.onLayoutStateChanged(layoutState, true)
                dragMode = DragMode.NONE
                dragHasMoved = false
            }
        }
        return true
    }

    private fun beginDrag(event: MotionEvent) {
        dragStartX = event.x
        dragStartY = event.y
        dragStartScale = layoutState.floatingScale
        dragStartRect = childRect()
        dragHasMoved = false
    }

    private fun updateDrag(event: MotionEvent, finished: Boolean) {
        val deltaX = event.x - dragStartX
        val deltaY = event.y - dragStartY
        if (dragMode == DragMode.MOVE && !dragHasMoved) {
            if (FloatingHandleInteraction.isTap(deltaX, deltaY, touchSlopPx)) {
                if (finished) toggleFloatingControls()
                return
            }
            dragHasMoved = true
            dismissFloatingControls()
        }
        val next = if (dragMode == DragMode.MOVE) {
            movedGeometry(deltaX, deltaY)
        } else {
            resizedGeometry(deltaX, deltaY)
        }
        layoutState = next.normalized(
            allowOneHanded = !isLandscape,
            isLandscape = isLandscape,
        )
        requestLayout()
        listener?.onLayoutStateChanged(layoutState, finished)
    }

    private fun toggleFloatingControls() {
        removeCallbacks(hideFloatingControls)
        floatingControlsView.visibility = if (floatingControlsView.visibility == View.VISIBLE) {
            View.GONE
        } else {
            View.VISIBLE
        }
        if (floatingControlsView.visibility == View.VISIBLE) {
            postDelayed(hideFloatingControls, FLOATING_CONTROLS_TIMEOUT_MS)
        }
        requestLayout()
    }

    private fun dismissFloatingControls() {
        removeCallbacks(hideFloatingControls)
        if (floatingControlsView.visibility != View.GONE) {
            floatingControlsView.visibility = View.GONE
            requestLayout()
        }
    }

    private fun movedGeometry(deltaX: Float, deltaY: Float): KeyboardLayoutState {
        val bounds = availableBounds()
        val width = dragStartRect.width()
        val height = dragStartRect.height()
        val left = (dragStartRect.left + deltaX).coerceIn(bounds.left, max(bounds.left, bounds.right - width))
        val top = (dragStartRect.top + deltaY).coerceIn(bounds.top, max(bounds.top, bounds.bottom - height))
        return geometryForRect(left, top, width, height, dragStartScale)
    }

    private fun resizedGeometry(deltaX: Float, deltaY: Float): KeyboardLayoutState {
        val baseWidth = dragStartRect.width() / dragStartScale.coerceAtLeast(0.01f)
        val dragStartHeightScale = KeyboardLayoutState.heightScaleForFloatingWidth(
            dragStartScale,
            isLandscape,
        )
        val baseHeight = dragStartRect.height() / dragStartHeightScale.coerceAtLeast(0.01f)
        val scaleCandidates = mutableListOf<Float>()
        if (dragMode.hasLeftEdge()) scaleCandidates += dragStartScale - deltaX / baseWidth
        if (dragMode.hasRightEdge()) scaleCandidates += dragStartScale + deltaX / baseWidth
        if (dragMode.hasTopEdge()) {
            scaleCandidates += KeyboardLayoutState.widthScaleForFloatingHeight(
                dragStartHeightScale - deltaY / baseHeight,
                isLandscape,
            )
        }
        if (dragMode.hasBottomEdge()) {
            scaleCandidates += KeyboardLayoutState.widthScaleForFloatingHeight(
                dragStartHeightScale + deltaY / baseHeight,
                isLandscape,
            )
        }
        val nextScale = scaleCandidates
            .maxByOrNull { abs(it - dragStartScale) }
            ?.coerceIn(
                KeyboardLayoutState.minimumFloatingScale(isLandscape),
                KeyboardLayoutState.MAX_SCALE,
            )
            ?: dragStartScale
        val width = baseWidth * nextScale
        val height = baseHeight * KeyboardLayoutState.heightScaleForFloatingWidth(nextScale, isLandscape)
        val left = when {
            dragMode.hasLeftEdge() -> dragStartRect.right - width
            dragMode.hasRightEdge() -> dragStartRect.left
            else -> dragStartRect.centerX() - width / 2f
        }
        val top = when {
            dragMode.hasTopEdge() -> dragStartRect.bottom - height
            dragMode.hasBottomEdge() -> dragStartRect.top
            else -> dragStartRect.centerY() - height / 2f
        }
        val bounds = availableBounds()
        val clampedLeft = left.coerceIn(bounds.left, max(bounds.left, bounds.right - width))
        val clampedTop = top.coerceIn(bounds.top, max(bounds.top, bounds.bottom - height))
        return geometryForRect(clampedLeft, clampedTop, width, height, nextScale)
    }

    private fun geometryForRect(
        left: Float,
        top: Float,
        childWidth: Float,
        childHeight: Float,
        scale: Float,
    ): KeyboardLayoutState {
        val bounds = availableBounds()
        val horizontalSpace = (bounds.width() - childWidth).coerceAtLeast(0f)
        val verticalSpace = (bounds.height() - childHeight).coerceAtLeast(0f)
        return layoutState.copy(
            mode = KeyboardLayoutMode.FLOATING,
            floatingScale = scale,
            floatingHorizontalPosition = if (horizontalSpace > 0f) (left - bounds.left) / horizontalSpace else 0.5f,
            floatingVerticalPosition = if (verticalSpace > 0f) (top - bounds.top) / verticalSpace else 0.5f,
        )
    }

    private fun dragModeAt(x: Float, y: Float): DragMode {
        val rect = childRect()
        if (!RectF(
                rect.left - edgeTouchSizePx,
                rect.top - edgeTouchSizePx,
                rect.right + edgeTouchSizePx,
                rect.bottom + edgeTouchSizePx,
            ).contains(x, y)
        ) {
            return DragMode.NONE
        }
        val nearLeft = abs(x - rect.left) <= edgeTouchSizePx
        val nearTop = abs(y - rect.top) <= edgeTouchSizePx
        val nearRight = abs(x - rect.right) <= edgeTouchSizePx
        val nearBottom = abs(y - rect.bottom) <= edgeTouchSizePx
        val horizontalRatio = if (rect.width() > 0f) (x - rect.left) / rect.width() else 0f
        if (nearTop && horizontalRatio in 0.32f..0.68f) return DragMode.MOVE
        return when {
            nearTop && nearLeft -> DragMode.RESIZE_TOP_LEFT
            nearTop && nearRight -> DragMode.RESIZE_TOP_RIGHT
            nearBottom && nearLeft -> DragMode.RESIZE_BOTTOM_LEFT
            nearBottom && nearRight -> DragMode.RESIZE_BOTTOM_RIGHT
            nearLeft -> DragMode.RESIZE_LEFT
            nearTop -> DragMode.RESIZE_TOP
            nearRight -> DragMode.RESIZE_RIGHT
            nearBottom -> DragMode.RESIZE_BOTTOM
            else -> DragMode.NONE
        }
    }

    private fun childRect(): RectF {
        val child = getChildAt(0) ?: return RectF()
        return RectF(child.left.toFloat(), child.top.toFloat(), child.right.toFloat(), child.bottom.toFloat())
    }

    private fun layoutSideSwitch(keyboardRect: RectF) {
        val bounds = availableBounds()
        val gap = 4f * resources.displayMetrics.density
        val availableLeft = if (layoutState.oneHandedSide == KeyboardSide.LEFT) {
            keyboardRect.right + gap
        } else {
            bounds.left
        }
        val availableRight = if (layoutState.oneHandedSide == KeyboardSide.LEFT) {
            bounds.right
        } else {
            keyboardRect.left - gap
        }
        val availableWidth = availableRight - availableLeft
        val minimumWidth = 26f * resources.displayMetrics.density
        if (availableWidth < minimumWidth + gap) {
            sideSwitchView.visibility = View.GONE
            return
        }

        val buttonSize = minOf(
            sideSwitchView.measuredWidth.toFloat(),
            availableWidth - gap,
            keyboardRect.height() - gap * 2f,
        ).coerceAtLeast(minimumWidth)
        val buttonLeft = (availableLeft + availableRight - buttonSize) / 2f
        val buttonTop = keyboardRect.centerY() - buttonSize / 2f
        sideSwitchView.destination = layoutState.oneHandedSide.opposite
        sideSwitchView.visibility = View.VISIBLE
        sideSwitchView.layout(
            buttonLeft.roundToInt(),
            buttonTop.roundToInt(),
            (buttonLeft + buttonSize).roundToInt(),
            (buttonTop + buttonSize).roundToInt(),
        )
    }

    private fun layoutFloatingControls(keyboardRect: RectF) {
        if (floatingControlsView.visibility != View.VISIBLE) return
        val bounds = availableBounds()
        val gap = 6f * resources.displayMetrics.density
        val controlWidth = floatingControlsView.measuredWidth.toFloat()
        val controlHeight = floatingControlsView.measuredHeight.toFloat()
        val controlLeft = (keyboardRect.centerX() - controlWidth / 2f).coerceIn(
            bounds.left,
            max(bounds.left, bounds.right - controlWidth),
        )
        val preferredTop = keyboardRect.top - controlHeight - gap
        val controlTop = if (preferredTop >= bounds.top) {
            preferredTop
        } else {
            (keyboardRect.top + gap).coerceAtMost(max(bounds.top, bounds.bottom - controlHeight))
        }
        floatingControlsView.layout(
            controlLeft.roundToInt(),
            controlTop.roundToInt(),
            (controlLeft + controlWidth).roundToInt(),
            (controlTop + controlHeight).roundToInt(),
        )
        floatingControlsView.bringToFront()
    }

    private fun isInsideFloatingControls(x: Float, y: Float): Boolean {
        return floatingControlsView.visibility == View.VISIBLE &&
            x >= floatingControlsView.left && x <= floatingControlsView.right &&
            y >= floatingControlsView.top && y <= floatingControlsView.bottom
    }

    private fun availableBounds(): RectF {
        val inset = if (layoutState.mode == KeyboardLayoutMode.FULL) 0f else marginPx.toFloat()
        val safeBottom = if (layoutState.mode == KeyboardLayoutMode.FLOATING) safeBottomInsetPx.toFloat() else 0f
        val boundsTop = paddingTop + inset
        return RectF(
            paddingLeft + inset,
            boundsTop,
            width - paddingRight - inset,
            (height - paddingBottom - inset - safeBottom).coerceAtLeast(boundsTop + 1f),
        )
    }

    private fun DragMode.hasLeftEdge(): Boolean {
        return this == DragMode.RESIZE_LEFT || this == DragMode.RESIZE_TOP_LEFT || this == DragMode.RESIZE_BOTTOM_LEFT
    }

    private fun DragMode.hasTopEdge(): Boolean {
        return this == DragMode.RESIZE_TOP || this == DragMode.RESIZE_TOP_LEFT || this == DragMode.RESIZE_TOP_RIGHT
    }

    private fun DragMode.hasRightEdge(): Boolean {
        return this == DragMode.RESIZE_RIGHT || this == DragMode.RESIZE_TOP_RIGHT || this == DragMode.RESIZE_BOTTOM_RIGHT
    }

    private fun DragMode.hasBottomEdge(): Boolean {
        return this == DragMode.RESIZE_BOTTOM || this == DragMode.RESIZE_BOTTOM_LEFT || this == DragMode.RESIZE_BOTTOM_RIGHT
    }

    companion object {
        private const val FLOATING_CONTROLS_TIMEOUT_MS = 3_500L
    }
}

private class KeyboardSideSwitchView(context: Context) : View(context) {
    var destination = KeyboardSide.LEFT
        set(value) {
            field = value
            contentDescription = if (value == KeyboardSide.LEFT) {
                "切换到左侧单手键盘"
            } else {
                "切换到右侧单手键盘"
            }
            invalidate()
        }

    private var theme = KeytaoImeTheme.fallback()
    private val paint = Paint(Paint.ANTI_ALIAS_FLAG)

    init {
        isClickable = true
        isFocusable = true
        destination = KeyboardSide.LEFT
        elevation = dp(2f)
    }

    fun updateTheme(next: KeytaoImeTheme) {
        theme = next
        invalidate()
    }

    override fun onDraw(canvas: Canvas) {
        super.onDraw(canvas)
        val rect = RectF(0f, 0f, width.toFloat(), height.toFloat())
        val radius = dp(theme.keyCornerRadiusDp)

        paint.style = Paint.Style.FILL
        paint.color = if (isPressed) {
            theme.keySelectedBackground.toArgb()
        } else {
            theme.panelBackground.toArgb()
        }
        canvas.drawRoundRect(rect, radius, radius, paint)

        paint.style = Paint.Style.STROKE
        paint.strokeWidth = dp(theme.candidateBorderWidthDp.coerceAtLeast(1f))
        paint.color = theme.panelBorder.toArgb()
        canvas.drawRoundRect(rect, radius, radius, paint)

        paint.color = if (isPressed) {
            theme.keySelectedForeground.toArgb()
        } else {
            theme.keyForeground.toArgb()
        }
        paint.strokeWidth = dp(2.2f)
        paint.strokeCap = Paint.Cap.ROUND
        paint.strokeJoin = Paint.Join.ROUND
        val direction = if (destination == KeyboardSide.LEFT) -1f else 1f
        val centerX = width / 2f
        val centerY = height / 2f
        val size = minOf(width, height) * 0.22f
        val arrow = Path().apply {
            moveTo(centerX - direction * size * 0.55f, centerY - size)
            lineTo(centerX + direction * size * 0.55f, centerY)
            lineTo(centerX - direction * size * 0.55f, centerY + size)
        }
        canvas.drawPath(arrow, paint)
    }

    override fun drawableStateChanged() {
        super.drawableStateChanged()
        invalidate()
    }

    private fun dp(value: Float): Float = value * resources.displayMetrics.density
}

private class FloatingControlCapsuleView(context: Context) : View(context) {
    private var theme = KeytaoImeTheme.fallback()
    private val paint = Paint(Paint.ANTI_ALIAS_FLAG)

    init {
        isClickable = true
        isFocusable = true
        contentDescription = "退出悬浮键盘"
        elevation = dp(8f)
    }

    fun updateTheme(next: KeytaoImeTheme) {
        theme = next
        invalidate()
    }

    override fun onDraw(canvas: Canvas) {
        super.onDraw(canvas)
        val rect = RectF(0f, 0f, width.toFloat(), height.toFloat())
        val radius = height / 2f
        paint.style = Paint.Style.FILL
        paint.color = if (isPressed) {
            theme.keySelectedBackground.toArgb()
        } else {
            theme.panelBackground.toArgb()
        }
        canvas.drawRoundRect(rect, radius, radius, paint)

        paint.style = Paint.Style.STROKE
        paint.strokeWidth = dp(theme.candidateBorderWidthDp.coerceAtLeast(1f))
        paint.color = theme.panelBorder.toArgb()
        canvas.drawRoundRect(rect, radius, radius, paint)

        paint.color = if (isPressed) {
            theme.keySelectedForeground.toArgb()
        } else {
            theme.keyForeground.toArgb()
        }
        paint.strokeWidth = dp(2.2f)
        paint.strokeCap = Paint.Cap.ROUND
        val iconRadius = minOf(width, height) * 0.18f
        canvas.drawLine(
            width / 2f - iconRadius,
            height / 2f - iconRadius,
            width / 2f + iconRadius,
            height / 2f + iconRadius,
            paint,
        )
        canvas.drawLine(
            width / 2f + iconRadius,
            height / 2f - iconRadius,
            width / 2f - iconRadius,
            height / 2f + iconRadius,
            paint,
        )
    }

    private fun dp(value: Float): Float = value * resources.displayMetrics.density
}
