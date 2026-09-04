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

internal enum class BackspaceGestureMode {
    IMMEDIATE,
    SELECT_THEN_DELETE;

    companion object {
        fun fromSetting(value: String?): BackspaceGestureMode = when (value?.trim()?.lowercase()) {
            "selectthendelete" -> SELECT_THEN_DELETE
            else -> IMMEDIATE
        }
    }
}

internal data class BackspaceGestureCommand(
    val action: String,
    val count: Int,
)

internal object BackspaceGesturePolicy {
    fun dragCommand(
        mode: BackspaceGestureMode,
        currentUnits: Int,
        requestedUnits: Int,
        maximumUnits: Int,
    ): BackspaceGestureCommand? {
        return when (mode) {
            BackspaceGestureMode.IMMEDIATE -> {
                val target = requestedUnits.coerceIn(-maximumUnits, maximumUnits)
                val delta = target - currentUnits
                if (delta == 0) null else BackspaceGestureCommand(
                    action = if (delta > 0) "delete" else "restore",
                    count = kotlin.math.abs(delta),
                )
            }
            BackspaceGestureMode.SELECT_THEN_DELETE -> {
                val target = requestedUnits.coerceIn(0, maximumUnits)
                if (target == currentUnits) null else BackspaceGestureCommand(
                    action = if (target == 0) "cancelSelection" else "select",
                    count = target,
                )
            }
        }
    }

    fun releaseCommand(mode: BackspaceGestureMode, selectedUnits: Int): BackspaceGestureCommand? {
        if (mode != BackspaceGestureMode.SELECT_THEN_DELETE) return null
        return BackspaceGestureCommand(
            action = if (selectedUnits > 0) "commitSelection" else "cancelSelection",
            count = selectedUnits.coerceAtLeast(0),
        )
    }
}

internal object KeytaoImeInteractionTuning {
    const val LONG_PRESS_DELAY_MIN_MS = 100L
    const val LONG_PRESS_DELAY_DEFAULT_MS = 300L
    const val LONG_PRESS_DELAY_MAX_MS = 700L
    const val SLIDE_RETARGET_HYSTERESIS_DP = 8f
    const val BOUNCE_INTERVAL_MS = 40L
    const val BOUNCE_DISTANCE_DP = 12.6f
    const val KEYBOARD_DISMISS_VELOCITY_DP_PER_SECOND = 600f
    const val BACKSPACE_HOLD_TOLERANCE_DP = 8f
    const val CURSOR_GESTURE_ACTIVATION_DP = 12.6f
    const val CURSOR_GESTURE_STEP_DP = 10f
    const val CANDIDATE_DRAG_SLOP_DP = 8f
    const val DOUBLE_SPACE_PERIOD_TIMEOUT_MS = 1_100L
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

    fun isBounceDown(sinceLastUpMs: Long, distanceFromLastUpDp: Float): Boolean {
        return sinceLastUpMs >= 0L &&
            sinceLastUpMs < BOUNCE_INTERVAL_MS &&
            distanceFromLastUpDp < BOUNCE_DISTANCE_DP
    }
}

/**
 * Atomically replaces the geometry consumed by touch hit-testing. Views rebuild
 * this store during layout/state changes, so a touch never observes a partially
 * rebuilt or previous-frame list.
 */
internal class ImmediateHitLayout<Element> {
    private var snapshot: List<Element> = emptyList()

    val items: List<Element>
        get() = snapshot

    fun rebuild(next: List<Element>) {
        snapshot = next
    }

    fun firstOrNull(predicate: (Element) -> Boolean): Element? = snapshot.firstOrNull(predicate)

    fun firstIndexOrNull(predicate: (Element) -> Boolean): Int? {
        return snapshot.indexOfFirst(predicate).takeIf { it >= 0 }
    }
}

internal class PerPointerBounceTracker<PointerId> {
    private data class PreviousUp(
        val eventTimeMs: Long,
        val xDp: Float,
        val yDp: Float,
    )

    private val previousUps = mutableMapOf<PointerId, PreviousUp>()
    private val bouncedPointers = mutableSetOf<PointerId>()

    fun isBounceDown(pointerId: PointerId, eventTimeMs: Long, xDp: Float, yDp: Float): Boolean {
        val previousUp = previousUps[pointerId]
        val isBounce = previousUp != null && KeytaoImeInteractionTuning.isBounceDown(
            sinceLastUpMs = eventTimeMs - previousUp.eventTimeMs,
            distanceFromLastUpDp = kotlin.math.hypot(xDp - previousUp.xDp, yDp - previousUp.yDp),
        )
        if (isBounce) {
            bouncedPointers.add(pointerId)
        } else {
            bouncedPointers.remove(pointerId)
        }
        return isBounce
    }

    fun recordUp(pointerId: PointerId, eventTimeMs: Long, xDp: Float, yDp: Float): Boolean {
        previousUps[pointerId] = PreviousUp(eventTimeMs, xDp, yDp)
        return bouncedPointers.remove(pointerId)
    }

    fun cancel(pointerId: PointerId) {
        bouncedPointers.remove(pointerId)
    }

    fun reset() {
        previousUps.clear()
        bouncedPointers.clear()
    }
}

internal class DoubleSpacePeriodTracker(
    private val timeoutMs: Long = KeytaoImeInteractionTuning.DOUBLE_SPACE_PERIOD_TIMEOUT_MS,
) {
    private var lastEligibleSpaceTimeMs: Long? = null

    fun shouldReplaceSpace(
        nowMs: Long,
        contextBefore: String,
        enabled: Boolean,
        hasComposition: Boolean,
    ): Boolean {
        if (!enabled || hasComposition) {
            reset()
            return false
        }
        val previousTime = lastEligibleSpaceTimeMs
        val canReplace = previousTime != null &&
            nowMs - previousTime in 0..timeoutMs &&
            contextBefore.endsWith(" ") &&
            hasDoubleSpaceEligibleSuffix(contextBefore.dropLast(1))
        if (canReplace) {
            reset()
            return true
        }
        lastEligibleSpaceTimeMs = nowMs.takeIf { hasDoubleSpaceEligibleSuffix(contextBefore) }
        return false
    }

    fun reset() {
        lastEligibleSpaceTimeMs = null
    }
}

private fun hasDoubleSpaceEligibleSuffix(text: String): Boolean {
    if (text.isEmpty()) return false
    val codePoint = text.codePointBefore(text.length)
    if (Character.isWhitespace(codePoint) || Character.isSpaceChar(codePoint)) return false
    return Character.getType(codePoint) !in punctuationTypes
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
    return trailingDeletionSegmentsLength(text, 1).coerceAtLeast(1)
}

internal fun trailingDeletionSegmentsLength(text: String, segmentCount: Int): Int {
    val units = graphemeUnits(text)
    if (units.isEmpty()) return 0
    val limit = segmentCount.coerceAtLeast(1)
    var selectedUnits = 0
    var selectedSegments = 0
    var previousClass: DeletionSegmentClass? = null
    for (unit in units.asReversed()) {
        val currentClass = deletionSegmentClass(unit)
        if (currentClass != previousClass) {
            if (selectedSegments == limit) break
            selectedSegments += 1
            previousClass = currentClass
        }
        selectedUnits += 1
    }
    return selectedUnits
}

internal fun englishCompletionPrefix(text: String): String? {
    val prefix = text.takeLastWhile { it in 'a'..'z' || it in 'A'..'Z' }
    return prefix.takeIf { it.length >= 2 }
}

internal fun completeEnglishPrefix(
    prefix: String,
    lexicon: List<String>,
    limit: Int = 5,
): List<String> {
    if (prefix.length < 2 || limit <= 0) return emptyList()
    val normalized = prefix.lowercase(Locale.ROOT)
    return lexicon.asSequence()
        .map(String::trim)
        .filter { it.length > prefix.length && it.lowercase(Locale.ROOT).startsWith(normalized) }
        .distinctBy { it.lowercase(Locale.ROOT) }
        .take(limit)
        .toList()
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
