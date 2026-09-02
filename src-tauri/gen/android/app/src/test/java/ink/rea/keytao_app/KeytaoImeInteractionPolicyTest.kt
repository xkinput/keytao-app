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
    fun `alternate selection keeps the first item until the finger moves`() {
        val tracker = AlternateSelectionTracker(startX = 100f, movementThreshold = 8f)

        assertEquals(0, tracker.selectedIndex(100f, true, 20f, 40f, 4))
        assertEquals(0, tracker.selectedIndex(106f, true, 20f, 40f, 4))
        assertEquals(1, tracker.selectedIndex(61f, true, 20f, 40f, 4))
        assertEquals(null, tracker.selectedIndex(61f, false, 20f, 40f, 4))
    }
}
