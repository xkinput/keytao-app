package ink.rea.keytao_app

import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Path
import android.graphics.Region
import android.graphics.RectF
import android.os.Build
import android.view.HapticFeedbackConstants
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
    private var dragMode = FloatingDragMode.NONE
    private var dragStartX = 0f
    private var dragStartY = 0f
    private var dragStartScale = 1f
    private var dragStartRect = RectF()
    private var dragHasMoved = false
    private val edgeTouchSizePx = 18f * resources.displayMetrics.density
    private val dockThresholdPx = DOCK_THRESHOLD_DP * resources.displayMetrics.density
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
    init {
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
        sideSwitchView.destination = layoutState.oneHandedSide.opposite
        if (layoutState.mode != KeyboardLayoutMode.FLOATING) {
            resetDragState()
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
                child.layout(
                    paddingLeft,
                    paddingTop,
                    paddingLeft + child.measuredWidth,
                    paddingTop + child.measuredHeight,
                )
            }
            KeyboardLayoutMode.ONE_HANDED -> {
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
            }
        }
    }

    override fun onInterceptTouchEvent(event: MotionEvent): Boolean {
        if (layoutState.mode != KeyboardLayoutMode.FLOATING) return false
        return when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                dragMode = dragModeAt(event.x, event.y)
                if (dragMode != FloatingDragMode.NONE) {
                    beginDrag(event)
                    true
                } else {
                    false
                }
            }
            else -> dragMode != FloatingDragMode.NONE
        }
    }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        if (layoutState.mode != KeyboardLayoutMode.FLOATING || dragMode == FloatingDragMode.NONE) return false
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> beginDrag(event)
            MotionEvent.ACTION_MOVE -> updateDrag(event)
            MotionEvent.ACTION_UP -> finishDrag(event)
            MotionEvent.ACTION_CANCEL -> cancelDrag()
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

    private fun updateDrag(event: MotionEvent) {
        val next = geometryForDrag(event, dragMode) ?: return
        layoutState = next.normalized(
            allowOneHanded = !isLandscape,
            isLandscape = isLandscape,
        )
        requestLayout()
        listener?.onLayoutStateChanged(layoutState, false)
    }

    private fun finishDrag(event: MotionEvent) {
        val activeMode = dragMode
        val next = geometryForDrag(event, activeMode)
        val shouldDock = FloatingHandleInteraction.shouldDockOnRelease(
            dragMode = activeMode,
            dragHasMoved = dragHasMoved,
            releaseY = event.y,
            bottomEdge = availableBounds().bottom,
            threshold = dockThresholdPx,
        )
        val finalState = when {
            shouldDock -> (next ?: layoutState).copy(mode = KeyboardLayoutMode.FULL)
            next != null -> next
            else -> null
        }?.normalized(
            allowOneHanded = !isLandscape,
            isLandscape = isLandscape,
        )

        // The listener can synchronously re-apply presentation, so end the gesture
        // before delivering its single finished callback.
        resetDragState()
        if (finalState == null) return

        layoutState = finalState
        requestLayout()
        if (shouldDock) {
            performHapticFeedback(
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                    HapticFeedbackConstants.CONFIRM
                } else {
                    HapticFeedbackConstants.LONG_PRESS
                },
            )
        }
        listener?.onLayoutStateChanged(layoutState, true)
    }

    private fun cancelDrag() {
        val finalState = layoutState
        resetDragState()
        listener?.onLayoutStateChanged(finalState, true)
    }

    private fun resetDragState() {
        dragMode = FloatingDragMode.NONE
        dragStartX = 0f
        dragStartY = 0f
        dragStartScale = 1f
        dragStartRect = RectF()
        dragHasMoved = false
    }

    private fun geometryForDrag(
        event: MotionEvent,
        activeMode: FloatingDragMode,
    ): KeyboardLayoutState? {
        val deltaX = event.x - dragStartX
        val deltaY = event.y - dragStartY
        if (activeMode == FloatingDragMode.MOVE && !dragHasMoved) {
            if (FloatingHandleInteraction.isTap(deltaX, deltaY, touchSlopPx)) {
                return null
            }
            dragHasMoved = true
        }
        return if (activeMode == FloatingDragMode.MOVE) {
            movedGeometry(deltaX, deltaY)
        } else {
            resizedGeometry(deltaX, deltaY, activeMode)
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

    private fun resizedGeometry(
        deltaX: Float,
        deltaY: Float,
        activeMode: FloatingDragMode,
    ): KeyboardLayoutState {
        val baseWidth = dragStartRect.width() / dragStartScale.coerceAtLeast(0.01f)
        val dragStartHeightScale = KeyboardLayoutState.heightScaleForFloatingWidth(
            dragStartScale,
            isLandscape,
        )
        val baseHeight = dragStartRect.height() / dragStartHeightScale.coerceAtLeast(0.01f)
        val scaleCandidates = mutableListOf<Float>()
        if (activeMode.hasLeftEdge()) scaleCandidates += dragStartScale - deltaX / baseWidth
        if (activeMode.hasRightEdge()) scaleCandidates += dragStartScale + deltaX / baseWidth
        if (activeMode.hasTopEdge()) {
            scaleCandidates += KeyboardLayoutState.widthScaleForFloatingHeight(
                dragStartHeightScale - deltaY / baseHeight,
                isLandscape,
            )
        }
        if (activeMode.hasBottomEdge()) {
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
            activeMode.hasLeftEdge() -> dragStartRect.right - width
            activeMode.hasRightEdge() -> dragStartRect.left
            else -> dragStartRect.centerX() - width / 2f
        }
        val top = when {
            activeMode.hasTopEdge() -> dragStartRect.bottom - height
            activeMode.hasBottomEdge() -> dragStartRect.top
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

    private fun dragModeAt(x: Float, y: Float): FloatingDragMode {
        val rect = childRect()
        return FloatingHandleInteraction.dragModeAt(
            x = x,
            y = y,
            left = rect.left,
            top = rect.top,
            right = rect.right,
            bottom = rect.bottom,
            edgeTouchSize = edgeTouchSizePx,
        )
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

    private fun FloatingDragMode.hasLeftEdge(): Boolean {
        return this == FloatingDragMode.RESIZE_LEFT ||
            this == FloatingDragMode.RESIZE_TOP_LEFT ||
            this == FloatingDragMode.RESIZE_BOTTOM_LEFT
    }

    private fun FloatingDragMode.hasTopEdge(): Boolean {
        return this == FloatingDragMode.RESIZE_TOP ||
            this == FloatingDragMode.RESIZE_TOP_LEFT ||
            this == FloatingDragMode.RESIZE_TOP_RIGHT
    }

    private fun FloatingDragMode.hasRightEdge(): Boolean {
        return this == FloatingDragMode.RESIZE_RIGHT ||
            this == FloatingDragMode.RESIZE_TOP_RIGHT ||
            this == FloatingDragMode.RESIZE_BOTTOM_RIGHT
    }

    private fun FloatingDragMode.hasBottomEdge(): Boolean {
        return this == FloatingDragMode.RESIZE_BOTTOM ||
            this == FloatingDragMode.RESIZE_BOTTOM_LEFT ||
            this == FloatingDragMode.RESIZE_BOTTOM_RIGHT
    }

    companion object {
        private const val DOCK_THRESHOLD_DP = 24f
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
