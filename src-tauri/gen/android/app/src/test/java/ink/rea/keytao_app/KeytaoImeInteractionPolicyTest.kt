package ink.rea.keytao_app

import org.junit.Assert.assertEquals
import org.junit.Test

class KeytaoImeInteractionPolicyTest {
    @Test
    fun `standard backspace cadence matches the mobile UX contract`() {
        val profile = KeytaoImeInteractionTuning.backspaceProfile(DeleteSpeed.STANDARD)
        val policy = BackspaceRepeatPolicy(profile)

        assertEquals(400L, profile.initialDelayMs)
        assertEquals(50L, profile.intervalMs)
        assertEquals(1_500L, profile.segmentThresholdMs)
        assertEquals(0, policy.repeatCountAt(399L))
        assertEquals(13, policy.repeatCountAt(1_000L))
        assertEquals(BackspaceDeletionGranularity.CHARACTER, policy.granularityAt(1_499L))
        assertEquals(BackspaceDeletionGranularity.SEGMENT, policy.granularityAt(1_500L))
    }

    @Test
    fun `segment deletion stops at punctuation and Chinese Latin boundaries`() {
        assertEquals(2, trailingDeletionSegmentLength("前文，测试"))
        assertEquals(5, trailingDeletionSegmentLength("测试hello"))
        assertEquals(2, trailingDeletionSegmentLength("hello中文"))
        assertEquals(1, trailingDeletionSegmentLength("hello！"))
        assertEquals(5, trailingDeletionSegmentsLength("中文 hello", 1))
        assertEquals(6, trailingDeletionSegmentsLength("中文 hello", 2))
        assertEquals(8, trailingDeletionSegmentsLength("中文 hello", 3))
    }

    @Test
    fun `backspace gesture modes share one transition policy`() {
        assertEquals(BackspaceGestureMode.IMMEDIATE, BackspaceGestureMode.fromSetting(null))
        assertEquals(BackspaceGestureMode.SELECT_THEN_DELETE, BackspaceGestureMode.fromSetting("selectThenDelete"))
        assertEquals(
            BackspaceGestureCommand("delete", 2),
            BackspaceGesturePolicy.dragCommand(BackspaceGestureMode.IMMEDIATE, 1, 3, 96),
        )
        assertEquals(
            BackspaceGestureCommand("restore", 2),
            BackspaceGesturePolicy.dragCommand(BackspaceGestureMode.IMMEDIATE, 3, 1, 96),
        )
        assertEquals(
            BackspaceGestureCommand("select", 3),
            BackspaceGesturePolicy.dragCommand(BackspaceGestureMode.SELECT_THEN_DELETE, 1, 3, 96),
        )
        assertEquals(
            BackspaceGestureCommand("cancelSelection", 0),
            BackspaceGesturePolicy.dragCommand(BackspaceGestureMode.SELECT_THEN_DELETE, 3, -2, 96),
        )
        assertEquals(
            BackspaceGestureCommand("commitSelection", 3),
            BackspaceGesturePolicy.releaseCommand(BackspaceGestureMode.SELECT_THEN_DELETE, 3),
        )
    }

    @Test
    fun `English completion uses the active ASCII word prefix`() {
        assertEquals("he", englishCompletionPrefix("say he"))
        assertEquals(null, englishCompletionPrefix("say h"))
        assertEquals(null, englishCompletionPrefix("say he!"))
        assertEquals(
            listOf("hello", "help", "hero"),
            completeEnglishPrefix("he", listOf("hello", "help", "he", "world", "hero")),
        )
    }

    @Test
    fun `space cursor gesture activates after touch noise and emits fixed steps`() {
        val tracker = CursorGestureTracker(startX = 100f)

        assertEquals(CursorGestureUpdate(active = false, stepDelta = 0), tracker.update(112.5f))
        assertEquals(CursorGestureUpdate(active = true, stepDelta = 1), tracker.update(112.6f))
        assertEquals(CursorGestureUpdate(active = true, stepDelta = 1), tracker.update(120f))
        assertEquals(CursorGestureUpdate(active = true, stepDelta = -3), tracker.update(90f))
    }

    @Test
    fun `same-key fast double tap commits both clean 25ms taps`() {
        val tracker = PerPointerBounceTracker<Int>()
        var commitCount = 0

        if (!tracker.isBounceDown(0, eventTimeMs = 0, xDp = 10f, yDp = 10f)) commitCount++
        tracker.recordUp(0, eventTimeMs = 25, xDp = 10f, yDp = 10f)
        if (!tracker.isBounceDown(0, eventTimeMs = 85, xDp = 10f, yDp = 10f)) commitCount++
        tracker.recordUp(0, eventTimeMs = 110, xDp = 10f, yDp = 10f)

        assertEquals(2, commitCount)
    }

    @Test
    fun `different-key fast typing commits both clean 25ms taps`() {
        val tracker = PerPointerBounceTracker<Int>()
        var commitCount = 0

        if (!tracker.isBounceDown(0, eventTimeMs = 0, xDp = 10f, yDp = 10f)) commitCount++
        tracker.recordUp(0, eventTimeMs = 25, xDp = 10f, yDp = 10f)
        if (!tracker.isBounceDown(0, eventTimeMs = 55, xDp = 30f, yDp = 10f)) commitCount++
        tracker.recordUp(0, eventTimeMs = 80, xDp = 30f, yDp = 10f)

        assertEquals(2, commitCount)
    }

    @Test
    fun `same-position down 20ms after up is rejected as bounce`() {
        val tracker = PerPointerBounceTracker<Int>()

        assertEquals(false, tracker.isBounceDown(0, eventTimeMs = 0, xDp = 10f, yDp = 10f))
        tracker.recordUp(0, eventTimeMs = 25, xDp = 10f, yDp = 10f)
        assertEquals(true, tracker.isBounceDown(0, eventTimeMs = 45, xDp = 10f, yDp = 10f))
        assertEquals(false, tracker.isBounceDown(1, eventTimeMs = 45, xDp = 10f, yDp = 10f))
    }

    @Test
    fun `bounce down requires both dimensions to remain below the boundary`() {
        assertEquals(true, KeytaoImeInteractionTuning.isBounceDown(39, 12.59f))
        assertEquals(false, KeytaoImeInteractionTuning.isBounceDown(40, 12.59f))
        assertEquals(false, KeytaoImeInteractionTuning.isBounceDown(39, 12.6f))
        assertEquals(false, KeytaoImeInteractionTuning.isBounceDown(-1, 1f))
    }

    @Test
    fun `alternate selection keeps the first item until the finger moves`() {
        val tracker = AlternateSelectionTracker(startX = 100f, movementThreshold = 8f)

        assertEquals(0, tracker.selectedIndex(100f, true, 20f, 40f, 4))
        assertEquals(0, tracker.selectedIndex(106f, true, 20f, 40f, 4))
        assertEquals(1, tracker.selectedIndex(61f, true, 20f, 40f, 4))
        assertEquals(null, tracker.selectedIndex(61f, false, 20f, 40f, 4))
    }

    @Test
    fun `double space period requires an eligible character and the exact window`() {
        val tracker = DoubleSpacePeriodTracker()

        assertEquals(false, tracker.shouldReplaceSpace(1_000, "字", enabled = true, hasComposition = false))
        assertEquals(true, tracker.shouldReplaceSpace(2_100, "字 ", enabled = true, hasComposition = false))
        assertEquals(false, tracker.shouldReplaceSpace(3_000, "字。", enabled = true, hasComposition = false))
        assertEquals(false, tracker.shouldReplaceSpace(3_100, "字。 ", enabled = true, hasComposition = false))
        assertEquals(false, tracker.shouldReplaceSpace(5_000, "word", enabled = true, hasComposition = false))
        assertEquals(false, tracker.shouldReplaceSpace(6_101, "word ", enabled = true, hasComposition = false))
        assertEquals(false, tracker.shouldReplaceSpace(7_000, "word", enabled = false, hasComposition = false))
        assertEquals(false, tracker.shouldReplaceSpace(7_100, "word ", enabled = true, hasComposition = true))
    }
}
