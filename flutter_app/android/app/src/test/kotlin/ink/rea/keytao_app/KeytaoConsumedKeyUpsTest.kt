package ink.rea.keytao_app

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class KeytaoConsumedKeyUpsTest {
    @Test
    fun `only the matching device and key are consumed once even after repeat downs`() {
        val keys = KeytaoConsumedKeyUps()
        keys.recordDown(1, 66)
        keys.recordDown(1, 66)
        keys.recordDown(2, 66)

        assertFalse(keys.removeDown(3, 66))
        assertFalse(keys.removeDown(1, 67))
        assertTrue(keys.removeDown(1, 66))
        assertFalse(keys.removeDown(1, 66))
        assertTrue(keys.removeDown(2, 66))
    }

    @Test
    fun `clearing the tracker forgets pending key ups`() {
        val keys = KeytaoConsumedKeyUps()
        keys.recordDown(1, 66)
        keys.clear()

        assertFalse(keys.removeDown(1, 66))
        keys.recordDown(1, 66)
        assertTrue(keys.removeDown(1, 66))
    }
}
