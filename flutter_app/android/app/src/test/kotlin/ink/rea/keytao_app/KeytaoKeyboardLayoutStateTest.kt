package ink.rea.keytao_app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class KeytaoKeyboardLayoutStateTest {
    @Test
    fun `portrait layout clamps independent floating and one handed geometry`() {
        val state = KeyboardLayoutState(
            mode = KeyboardLayoutMode.FLOATING,
            floatingScale = 0.4f,
            floatingHorizontalPosition = -2f,
            floatingVerticalPosition = 3f,
            oneHandedScale = 0.99f,
            oneHandedSide = KeyboardSide.LEFT,
        ).normalized()

        assertEquals(KeyboardLayoutMode.FLOATING, state.mode)
        assertEquals(KeyboardLayoutState.MIN_PORTRAIT_FLOATING_SCALE, state.floatingScale)
        assertEquals(0f, state.floatingHorizontalPosition)
        assertEquals(1f, state.floatingVerticalPosition)
        assertEquals(KeyboardLayoutState.MAX_ONE_HANDED_SCALE, state.oneHandedScale)
        assertEquals(KeyboardSide.LEFT, state.oneHandedSide)
    }

    @Test
    fun `landscape floating layout supports forty five percent screen width`() {
        val state = KeyboardLayoutState(
            mode = KeyboardLayoutMode.FLOATING,
            floatingScale = 0.4f,
            floatingHorizontalPosition = 0.25f,
            floatingVerticalPosition = 0.75f,
        ).normalized(isLandscape = true)

        assertEquals(KeyboardLayoutState.MIN_LANDSCAPE_FLOATING_SCALE, state.floatingScale)
        assertEquals(KeyboardLayoutState.MIN_LANDSCAPE_HEIGHT_SCALE, state.floatingHeightScale(true))
        assertEquals(0.25f, state.floatingHorizontalPosition)
        assertEquals(0.75f, state.floatingVerticalPosition)
    }

    @Test
    fun `landscape height boost remains reversible across resize edges`() {
        val widthScales = listOf(0.45f, 0.50f, 0.56f, 0.64f, 0.70f, 0.85f)

        assertEquals(
            0.62f,
            KeyboardLayoutState.heightScaleForFloatingWidth(0.50f, isLandscape = true),
            0.0001f,
        )
        for (widthScale in widthScales) {
            val heightScale = KeyboardLayoutState.heightScaleForFloatingWidth(widthScale, isLandscape = true)
            val restoredWidth = KeyboardLayoutState.widthScaleForFloatingHeight(heightScale, isLandscape = true)
            assertEquals(widthScale, restoredWidth, 0.0001f)
        }
    }

    @Test
    fun `landscape rejects one handed mode without losing its side`() {
        val state = KeyboardLayoutState(
            mode = KeyboardLayoutMode.ONE_HANDED,
            floatingScale = 0.82f,
            oneHandedSide = KeyboardSide.LEFT,
        ).normalized(allowOneHanded = false)

        assertEquals(KeyboardLayoutMode.FULL, state.mode)
        assertEquals(KeyboardSide.LEFT, state.oneHandedSide)
    }

    @Test
    fun `active scale follows the selected layout mode`() {
        val base = KeyboardLayoutState(
            mode = KeyboardLayoutMode.FULL,
            floatingScale = 0.74f,
            oneHandedScale = 0.88f,
        )

        assertEquals(1f, base.activeScale)
        assertEquals(0.88f, base.copy(mode = KeyboardLayoutMode.ONE_HANDED).activeScale)
        assertEquals(0.74f, base.copy(mode = KeyboardLayoutMode.FLOATING).activeScale)
    }

    @Test
    fun `floating layout occupies the complete ime viewport`() {
        val floating = KeyboardLayoutState(
            mode = KeyboardLayoutMode.FLOATING,
            floatingScale = 0.74f,
        )
        val oneHanded = floating.copy(mode = KeyboardLayoutMode.ONE_HANDED)
        val full = floating.copy(mode = KeyboardLayoutMode.FULL)

        assertEquals(
            1600,
            floating.resolveHostHeight(
                availableHeight = 1600,
                normalHeight = 620,
                childHeight = 480,
                margin = 12,
            ),
        )
        assertEquals(
            504,
            oneHanded.resolveHostHeight(
                availableHeight = 1600,
                normalHeight = 620,
                childHeight = 480,
                margin = 12,
            ),
        )
        assertEquals(
            480,
            full.resolveHostHeight(
                availableHeight = 1600,
                normalHeight = 620,
                childHeight = 480,
                margin = 12,
            ),
        )
    }

    @Test
    fun `floating drag handle distinguishes taps from drags`() {
        assertTrue(
            FloatingHandleInteraction.isTap(
                deltaX = 3f,
                deltaY = -4f,
                touchSlop = 8f,
            ),
        )
        assertFalse(
            FloatingHandleInteraction.isTap(
                deltaX = 9f,
                deltaY = 1f,
                touchSlop = 8f,
            ),
        )
        assertFalse(
            FloatingHandleInteraction.isTap(
                deltaX = 2f,
                deltaY = 9f,
                touchSlop = 8f,
            ),
        )
    }

    @Test
    fun `floating move handle is bottom center while top center resizes`() {
        assertEquals(
            FloatingDragMode.MOVE,
            FloatingHandleInteraction.dragModeAt(
                x = 300f,
                y = 500f,
                left = 100f,
                top = 200f,
                right = 500f,
                bottom = 500f,
                edgeTouchSize = 18f,
            ),
        )
        assertEquals(
            FloatingDragMode.RESIZE_TOP,
            FloatingHandleInteraction.dragModeAt(
                x = 300f,
                y = 200f,
                left = 100f,
                top = 200f,
                right = 500f,
                bottom = 500f,
                edgeTouchSize = 18f,
            ),
        )
        assertEquals(
            FloatingDragMode.RESIZE_BOTTOM_LEFT,
            FloatingHandleInteraction.dragModeAt(
                x = 100f,
                y = 500f,
                left = 100f,
                top = 200f,
                right = 500f,
                bottom = 500f,
                edgeTouchSize = 18f,
            ),
        )
    }

    @Test
    fun `floating move release docks only after dragging near the bottom edge`() {
        assertTrue(
            FloatingHandleInteraction.shouldDockOnRelease(
                dragMode = FloatingDragMode.MOVE,
                dragHasMoved = true,
                releaseY = 976f,
                bottomEdge = 1_000f,
                threshold = 24f,
            ),
        )
        assertFalse(
            FloatingHandleInteraction.shouldDockOnRelease(
                dragMode = FloatingDragMode.MOVE,
                dragHasMoved = true,
                releaseY = 975f,
                bottomEdge = 1_000f,
                threshold = 24f,
            ),
        )
        assertFalse(
            FloatingHandleInteraction.shouldDockOnRelease(
                dragMode = FloatingDragMode.RESIZE_BOTTOM,
                dragHasMoved = true,
                releaseY = 1_000f,
                bottomEdge = 1_000f,
                threshold = 24f,
            ),
        )
        assertFalse(
            FloatingHandleInteraction.shouldDockOnRelease(
                dragMode = FloatingDragMode.MOVE,
                dragHasMoved = false,
                releaseY = 1_000f,
                bottomEdge = 1_000f,
                threshold = 24f,
            ),
        )
    }
}
