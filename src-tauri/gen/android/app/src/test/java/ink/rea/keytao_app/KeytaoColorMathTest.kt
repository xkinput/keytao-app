package ink.rea.keytao_app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class KeytaoColorMathTest {
    @Test
    fun `preset accents survive a hex to hsv round trip`() {
        for (hex in listOf("#3B73D9", "#0F9F8F", "#D87A32", "#8B5CF6", "#FFFFFF", "#000000")) {
            val hsv = requireNotNull(KeytaoColorMath.hexToHsv(hex)) { hex }
            assertEquals(hex, KeytaoColorMath.hsvToHex(hsv.hue, hsv.saturation, hsv.value))
        }
    }

    @Test
    fun `lowercase hex round trips to the canonical uppercase form`() {
        val hsv = requireNotNull(KeytaoColorMath.hexToHsv("#3b73d9"))
        assertEquals("#3B73D9", KeytaoColorMath.hsvToHex(hsv.hue, hsv.saturation, hsv.value))
    }

    @Test
    fun `hue wraps into 0 until 360`() {
        assertEquals(0f, KeytaoColorMath.normalizeHue(360f), 0.0001f)
        assertEquals(350f, KeytaoColorMath.normalizeHue(-10f), 0.0001f)
        assertEquals(10f, KeytaoColorMath.normalizeHue(730f), 0.0001f)
        assertEquals(
            KeytaoColorMath.hsvToHex(0f, 1f, 1f),
            KeytaoColorMath.hsvToHex(360f, 1f, 1f),
        )
    }

    @Test
    fun `zero saturation is grey and zero value is black at any hue`() {
        assertEquals("#808080", KeytaoColorMath.hsvToHex(210f, 0f, 0.5019608f))
        assertEquals("#000000", KeytaoColorMath.hsvToHex(210f, 1f, 0f))
        val grey = KeytaoColorMath.rgbToHsv(128, 128, 128)
        assertEquals(0f, grey.hue, 0.0001f)
        assertEquals(0f, grey.saturation, 0.0001f)
        val black = KeytaoColorMath.rgbToHsv(0, 0, 0)
        assertEquals(0f, black.saturation, 0.0001f)
        assertEquals(0f, black.value, 0.0001f)
    }

    @Test
    fun `hue positions match the primaries`() {
        assertEquals("#FF0000", KeytaoColorMath.hsvToHex(0f, 1f, 1f))
        assertEquals("#00FF00", KeytaoColorMath.hsvToHex(120f, 1f, 1f))
        assertEquals("#0000FF", KeytaoColorMath.hsvToHex(240f, 1f, 1f))
        assertEquals(240f, KeytaoColorMath.rgbToHsv(0, 0, 255).hue, 0.0001f)
    }

    @Test
    fun `malformed hex is rejected`() {
        assertNull(KeytaoColorMath.hexToHsv("#12345"))
        assertNull(KeytaoColorMath.hexToHsv("#GGGGGG"))
        assertNull(KeytaoColorMath.hexToHsv("custom"))
        assertNull(KeytaoColorMath.hexToHsv("+12345"))
    }
}
