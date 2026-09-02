package ink.rea.keytao_app

import java.text.BreakIterator
import java.util.Locale

internal enum class DeleteSpeed {
    SLOW,
    STANDARD,
    FAST;

    companion object {
        fun fromSetting(value: String?): DeleteSpeed = when (value?.trim()?.lowercase()) {
            "slow" -> SLOW
            "fast" -> FAST
            else -> STANDARD
        }
    }
}

internal data class BackspaceRepeatProfile(
    val initialDelayMs: Long,
    val intervalMs: Long,
    val segmentThresholdMs: Long,
)

internal enum class BackspaceDeletionGranularity { CHARACTER, SEGMENT }

internal object KeytaoImeInteractionTuning {
    const val LONG_PRESS_DELAY_MIN_MS = 100L
    const val LONG_PRESS_DELAY_DEFAULT_MS = 300L
    const val LONG_PRESS_DELAY_MAX_MS = 700L
    const val SLIDE_RETARGET_HYSTERESIS_DP = 8f
    const val BACKSPACE_HOLD_TOLERANCE_DP = 8f
    const val CURSOR_GESTURE_ACTIVATION_DP = 12.6f
    const val CURSOR_GESTURE_STEP_DP = 10f
    const val CANDIDATE_DRAG_SLOP_DP = 8f
    const val CANDIDATE_TOOLBAR_TOGGLE_WIDTH_DP = 28f
    const val REPEATABLE_EDIT_INTERVAL_MS = 72L

    private val slowBackspace = BackspaceRepeatProfile(
        initialDelayMs = 500L,
        intervalMs = 70L,
        segmentThresholdMs = 1_800L,
    )
    private val standardBackspace = BackspaceRepeatProfile(
        initialDelayMs = 400L,
        intervalMs = 50L,
        segmentThresholdMs = 1_500L,
    )
    private val fastBackspace = BackspaceRepeatProfile(
        initialDelayMs = 300L,
        intervalMs = 35L,
        segmentThresholdMs = 1_200L,
    )

    fun backspaceProfile(speed: DeleteSpeed): BackspaceRepeatProfile = when (speed) {
        DeleteSpeed.SLOW -> slowBackspace
        DeleteSpeed.STANDARD -> standardBackspace
        DeleteSpeed.FAST -> fastBackspace
    }
}

internal data class CursorGestureUpdate(
    val active: Boolean,
    val stepDelta: Int,
)

internal class AlternateSelectionTracker(
    private val startX: Float,
    private val movementThreshold: Float,
) {
    private var hasMoved = false

    fun selectedIndex(
        x: Float,
        insideSelection: Boolean,
        panelLeft: Float,
        itemWidth: Float,
        itemCount: Int,
    ): Int? {
        if (!insideSelection || itemWidth <= 0f || itemCount <= 0) return null
        if (!hasMoved && kotlin.math.abs(x - startX) <= movementThreshold) return 0
        hasMoved = true
        return ((x - panelLeft) / itemWidth).toInt().coerceIn(0, itemCount - 1)
    }
}

internal class CursorGestureTracker(
    private val startX: Float,
    private val activationDistance: Float = KeytaoImeInteractionTuning.CURSOR_GESTURE_ACTIVATION_DP,
    private val stepDistance: Float = KeytaoImeInteractionTuning.CURSOR_GESTURE_STEP_DP,
) {
    var active: Boolean = false
        private set

    private var dispatchedSteps = 0

    fun update(x: Float): CursorGestureUpdate {
        val displacement = x - startX
        if (!active && kotlin.math.abs(displacement) + FLOAT_COMPARISON_EPSILON < activationDistance) {
            return CursorGestureUpdate(active = false, stepDelta = 0)
        }
        active = true
        val targetSteps = (displacement / stepDistance).toInt()
        val delta = targetSteps - dispatchedSteps
        dispatchedSteps = targetSteps
        return CursorGestureUpdate(active = true, stepDelta = delta)
    }

    private companion object {
        const val FLOAT_COMPARISON_EPSILON = 0.0001f
    }
}

internal class BackspaceRepeatPolicy(
    private val profile: BackspaceRepeatProfile,
) {
    fun repeatCountAt(holdDurationMs: Long): Int {
        if (holdDurationMs < profile.initialDelayMs) return 0
        return 1 + ((holdDurationMs - profile.initialDelayMs) / profile.intervalMs).toInt()
    }

    fun granularityAt(holdDurationMs: Long): BackspaceDeletionGranularity {
        return if (holdDurationMs >= profile.segmentThresholdMs) {
            BackspaceDeletionGranularity.SEGMENT
        } else {
            BackspaceDeletionGranularity.CHARACTER
        }
    }
}

private enum class DeletionSegmentClass { WHITESPACE, CJK, LATIN, PUNCTUATION, OTHER }

internal fun trailingDeletionSegmentLength(text: String): Int {
    val units = graphemeUnits(text)
    val trailingClass = units.lastOrNull()?.let(::deletionSegmentClass) ?: return 1
    return units.asReversed().takeWhile { deletionSegmentClass(it) == trailingClass }.size.coerceAtLeast(1)
}

private fun graphemeUnits(text: String): List<String> {
    if (text.isEmpty()) return emptyList()
    val iterator = BreakIterator.getCharacterInstance(Locale.ROOT)
    iterator.setText(text)
    return buildList {
        var start = iterator.first()
        var end = iterator.next()
        while (end != BreakIterator.DONE) {
            add(text.substring(start, end))
            start = end
            end = iterator.next()
        }
    }
}

private fun deletionSegmentClass(unit: String): DeletionSegmentClass {
    if (unit.all(Char::isWhitespace)) return DeletionSegmentClass.WHITESPACE
    val codePoint = unit.codePointAt(0)
    return when {
        Character.UnicodeScript.of(codePoint) == Character.UnicodeScript.HAN -> DeletionSegmentClass.CJK
        Character.UnicodeScript.of(codePoint) == Character.UnicodeScript.LATIN || Character.isDigit(codePoint) -> {
            DeletionSegmentClass.LATIN
        }
        Character.getType(codePoint) in punctuationTypes -> DeletionSegmentClass.PUNCTUATION
        else -> DeletionSegmentClass.OTHER
    }
}

private val punctuationTypes = setOf(
    Character.CONNECTOR_PUNCTUATION.toInt(),
    Character.DASH_PUNCTUATION.toInt(),
    Character.START_PUNCTUATION.toInt(),
    Character.END_PUNCTUATION.toInt(),
    Character.INITIAL_QUOTE_PUNCTUATION.toInt(),
    Character.FINAL_QUOTE_PUNCTUATION.toInt(),
    Character.OTHER_PUNCTUATION.toInt(),
)
