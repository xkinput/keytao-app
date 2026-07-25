package ink.rea.keytao_app

import org.junit.Assert.assertEquals
import org.junit.Test

class KeytaoKeyboardLayoutStateTest {
    @Test
    fun `layout state clamps independent floating and one handed geometry`() {
        val state = KeyboardLayoutState(
            mode = KeyboardLayoutMode.FLOATING,
            floatingScale = 0.4f,
            floatingHorizontalPosition = -2f,
            floatingVerticalPosition = 3f,
            oneHandedScale = 0.99f,
            oneHandedSide = KeyboardSide.LEFT,
        ).normalized()

        assertEquals(KeyboardLayoutMode.FLOATING, state.mode)
        assertEquals(KeyboardLayoutState.MIN_SCALE, state.floatingScale)
        assertEquals(0f, state.floatingHorizontalPosition)
        assertEquals(1f, state.floatingVerticalPosition)
        assertEquals(KeyboardLayoutState.MAX_ONE_HANDED_SCALE, state.oneHandedScale)
        assertEquals(KeyboardSide.LEFT, state.oneHandedSide)
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
}
