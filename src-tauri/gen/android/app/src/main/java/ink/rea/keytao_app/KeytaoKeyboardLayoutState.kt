package ink.rea.keytao_app

import android.content.Context

enum class KeyboardLayoutMode(val preferenceValue: String) {
    FULL("full"),
    ONE_HANDED("one_handed"),
    FLOATING("floating");

    companion object {
        fun fromPreference(value: String?): KeyboardLayoutMode? {
            return entries.firstOrNull { it.preferenceValue == value }
        }
    }
}

enum class KeyboardSide(val preferenceValue: String) {
    LEFT("left"),
    RIGHT("right");

    val opposite: KeyboardSide
        get() = if (this == LEFT) RIGHT else LEFT

    companion object {
        fun fromPreference(value: String?): KeyboardSide? {
            return entries.firstOrNull { it.preferenceValue == value }
        }
    }
}

data class KeyboardLayoutState(
    val mode: KeyboardLayoutMode,
    val floatingScale: Float,
    val floatingHorizontalPosition: Float = 0.5f,
    val floatingVerticalPosition: Float = 1f,
    val oneHandedScale: Float = DEFAULT_ONE_HANDED_SCALE,
    val oneHandedSide: KeyboardSide = KeyboardSide.RIGHT,
) {
    fun normalized(
        allowOneHanded: Boolean = true,
        isLandscape: Boolean = false,
    ): KeyboardLayoutState {
        return copy(
            mode = if (!allowOneHanded && mode == KeyboardLayoutMode.ONE_HANDED) {
                KeyboardLayoutMode.FULL
            } else {
                mode
            },
            floatingScale = floatingScale.coerceIn(minimumFloatingScale(isLandscape), MAX_SCALE),
            floatingHorizontalPosition = floatingHorizontalPosition.coerceIn(0f, 1f),
            floatingVerticalPosition = floatingVerticalPosition.coerceIn(0f, 1f),
            oneHandedScale = oneHandedScale.coerceIn(MIN_ONE_HANDED_SCALE, MAX_ONE_HANDED_SCALE),
        )
    }

    val activeScale: Float
        get() = when (mode) {
            KeyboardLayoutMode.FULL -> 1f
            KeyboardLayoutMode.ONE_HANDED -> oneHandedScale
            KeyboardLayoutMode.FLOATING -> floatingScale
        }

    fun floatingHeightScale(isLandscape: Boolean): Float {
        return heightScaleForFloatingWidth(floatingScale, isLandscape)
    }

    fun resolveHostHeight(
        availableHeight: Int,
        normalHeight: Int,
        childHeight: Int,
        margin: Int,
    ): Int {
        val viewportHeight = availableHeight.takeIf { it > 0 } ?: normalHeight
        return when (mode) {
            KeyboardLayoutMode.FLOATING -> viewportHeight
            KeyboardLayoutMode.ONE_HANDED -> (childHeight + margin * 2).coerceAtMost(viewportHeight)
            KeyboardLayoutMode.FULL -> childHeight.coerceAtMost(viewportHeight)
        }.coerceAtLeast(1)
    }

    companion object {
        const val MIN_SCALE = 0.45f
        const val MIN_PORTRAIT_FLOATING_SCALE = 0.70f
        const val MIN_LANDSCAPE_FLOATING_SCALE = MIN_SCALE
        const val MAX_SCALE = 1f
        const val LANDSCAPE_HEIGHT_BOOST_END_SCALE = 0.70f
        const val MIN_LANDSCAPE_HEIGHT_SCALE = 0.60f
        const val MIN_ONE_HANDED_SCALE = 0.78f
        const val MAX_ONE_HANDED_SCALE = 0.92f
        const val DEFAULT_ONE_HANDED_SCALE = 0.86f

        fun minimumFloatingScale(isLandscape: Boolean): Float {
            return if (isLandscape) MIN_LANDSCAPE_FLOATING_SCALE else MIN_PORTRAIT_FLOATING_SCALE
        }

        fun heightScaleForFloatingWidth(widthScale: Float, isLandscape: Boolean): Float {
            val normalizedWidth = widthScale.coerceIn(minimumFloatingScale(isLandscape), MAX_SCALE)
            if (!isLandscape || normalizedWidth >= LANDSCAPE_HEIGHT_BOOST_END_SCALE) {
                return normalizedWidth
            }
            val progress = (normalizedWidth - MIN_LANDSCAPE_FLOATING_SCALE) /
                (LANDSCAPE_HEIGHT_BOOST_END_SCALE - MIN_LANDSCAPE_FLOATING_SCALE)
            return MIN_LANDSCAPE_HEIGHT_SCALE + progress *
                (LANDSCAPE_HEIGHT_BOOST_END_SCALE - MIN_LANDSCAPE_HEIGHT_SCALE)
        }

        fun widthScaleForFloatingHeight(heightScale: Float, isLandscape: Boolean): Float {
            if (!isLandscape) {
                return heightScale.coerceIn(MIN_PORTRAIT_FLOATING_SCALE, MAX_SCALE)
            }
            val normalizedHeight = heightScale.coerceIn(MIN_LANDSCAPE_HEIGHT_SCALE, MAX_SCALE)
            if (normalizedHeight >= LANDSCAPE_HEIGHT_BOOST_END_SCALE) {
                return normalizedHeight
            }
            val progress = (normalizedHeight - MIN_LANDSCAPE_HEIGHT_SCALE) /
                (LANDSCAPE_HEIGHT_BOOST_END_SCALE - MIN_LANDSCAPE_HEIGHT_SCALE)
            return MIN_LANDSCAPE_FLOATING_SCALE + progress *
                (LANDSCAPE_HEIGHT_BOOST_END_SCALE - MIN_LANDSCAPE_FLOATING_SCALE)
        }
    }
}

internal enum class FloatingDragMode {
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

internal object FloatingHandleInteraction {
    fun dragModeAt(
        x: Float,
        y: Float,
        left: Float,
        top: Float,
        right: Float,
        bottom: Float,
        edgeTouchSize: Float,
    ): FloatingDragMode {
        val edgeSize = edgeTouchSize.coerceAtLeast(0f)
        if (x < left - edgeSize || x > right + edgeSize ||
            y < top - edgeSize || y > bottom + edgeSize
        ) {
            return FloatingDragMode.NONE
        }
        val nearLeft = kotlin.math.abs(x - left) <= edgeSize
        val nearTop = kotlin.math.abs(y - top) <= edgeSize
        val nearRight = kotlin.math.abs(x - right) <= edgeSize
        val nearBottom = kotlin.math.abs(y - bottom) <= edgeSize
        val width = right - left
        val horizontalRatio = if (width > 0f) (x - left) / width else 0f
        if (nearBottom && horizontalRatio in 0.32f..0.68f) return FloatingDragMode.MOVE
        return when {
            nearTop && nearLeft -> FloatingDragMode.RESIZE_TOP_LEFT
            nearTop && nearRight -> FloatingDragMode.RESIZE_TOP_RIGHT
            nearBottom && nearLeft -> FloatingDragMode.RESIZE_BOTTOM_LEFT
            nearBottom && nearRight -> FloatingDragMode.RESIZE_BOTTOM_RIGHT
            nearLeft -> FloatingDragMode.RESIZE_LEFT
            nearTop -> FloatingDragMode.RESIZE_TOP
            nearRight -> FloatingDragMode.RESIZE_RIGHT
            nearBottom -> FloatingDragMode.RESIZE_BOTTOM
            else -> FloatingDragMode.NONE
        }
    }

    fun shouldDockOnRelease(
        dragMode: FloatingDragMode,
        dragHasMoved: Boolean,
        releaseY: Float,
        bottomEdge: Float,
        threshold: Float,
    ): Boolean {
        return dragMode == FloatingDragMode.MOVE &&
            dragHasMoved &&
            releaseY >= bottomEdge - threshold.coerceAtLeast(0f)
    }

    fun isTap(deltaX: Float, deltaY: Float, touchSlop: Float): Boolean {
        val threshold = touchSlop.coerceAtLeast(0f)
        return deltaX * deltaX + deltaY * deltaY <= threshold * threshold
    }
}

class KeytaoKeyboardLayoutStateStore(context: Context) {
    private val preferences = context.getSharedPreferences(PREFERENCES_NAME, Context.MODE_PRIVATE)

    fun load(isLandscape: Boolean, fallback: FloatingKeyboardProfile): KeyboardLayoutState {
        val prefix = orientationPrefix(isLandscape)
        val fallbackMode = if (fallback.enabled) KeyboardLayoutMode.FLOATING else KeyboardLayoutMode.FULL
        val storedMode = KeyboardLayoutMode.fromPreference(preferences.getString("${prefix}_mode", null))
        val migratedMode = if (preferences.contains("${prefix}_enabled")) {
            if (preferences.getBoolean("${prefix}_enabled", fallback.enabled)) {
                KeyboardLayoutMode.FLOATING
            } else {
                KeyboardLayoutMode.FULL
            }
        } else {
            fallbackMode
        }
        return KeyboardLayoutState(
            mode = storedMode ?: migratedMode,
            floatingScale = when {
                preferences.contains("${prefix}_floating_scale") -> {
                    preferences.getFloat("${prefix}_floating_scale", fallback.scale)
                }
                preferences.contains("${prefix}_scale") -> {
                    preferences.getFloat("${prefix}_scale", fallback.scale)
                }
                else -> fallback.scale
            },
            floatingHorizontalPosition = preferences.getFloat(
                "${prefix}_floating_horizontal_position",
                preferences.getFloat("${prefix}_horizontal_position", 0.5f),
            ),
            floatingVerticalPosition = preferences.getFloat(
                "${prefix}_floating_vertical_position",
                preferences.getFloat("${prefix}_vertical_position", 1f),
            ),
            oneHandedScale = preferences.getFloat(
                "${prefix}_one_handed_scale",
                KeyboardLayoutState.DEFAULT_ONE_HANDED_SCALE,
            ),
            oneHandedSide = KeyboardSide.fromPreference(
                preferences.getString("${prefix}_one_handed_side", null),
            ) ?: KeyboardSide.RIGHT,
        ).normalized(allowOneHanded = !isLandscape, isLandscape = isLandscape)
    }

    fun save(isLandscape: Boolean, state: KeyboardLayoutState) {
        val prefix = orientationPrefix(isLandscape)
        val normalized = state.normalized(allowOneHanded = !isLandscape, isLandscape = isLandscape)
        preferences.edit()
            .putString("${prefix}_mode", normalized.mode.preferenceValue)
            .putFloat("${prefix}_floating_scale", normalized.floatingScale)
            .putFloat("${prefix}_floating_horizontal_position", normalized.floatingHorizontalPosition)
            .putFloat("${prefix}_floating_vertical_position", normalized.floatingVerticalPosition)
            .putFloat("${prefix}_one_handed_scale", normalized.oneHandedScale)
            .putString("${prefix}_one_handed_side", normalized.oneHandedSide.preferenceValue)
            .putBoolean("${prefix}_enabled", normalized.mode == KeyboardLayoutMode.FLOATING)
            .putFloat("${prefix}_scale", normalized.floatingScale)
            .putFloat("${prefix}_horizontal_position", normalized.floatingHorizontalPosition)
            .putFloat("${prefix}_vertical_position", normalized.floatingVerticalPosition)
            .apply()
    }

    private fun orientationPrefix(isLandscape: Boolean): String {
        return if (isLandscape) "landscape" else "portrait"
    }

    companion object {
        private const val PREFERENCES_NAME = "keytao_floating_keyboard"
    }
}
