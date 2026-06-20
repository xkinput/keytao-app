package ink.rea.keytao_app

import android.graphics.Color
import org.json.JSONObject

data class KeytaoColor(
    val red: Int,
    val green: Int,
    val blue: Int,
    val alpha: Int = 255,
) {
    fun toArgb(): Int = Color.argb(alpha, red, green, blue)

    companion object {
        fun rgb(red: Int, green: Int, blue: Int) = KeytaoColor(red, green, blue)

        fun fromJson(json: JSONObject?, fallback: KeytaoColor): KeytaoColor {
            if (json == null) return fallback
            return KeytaoColor(
                red = json.optInt("red", fallback.red).coerceIn(0, 255),
                green = json.optInt("green", fallback.green).coerceIn(0, 255),
                blue = json.optInt("blue", fallback.blue).coerceIn(0, 255),
                alpha = json.optInt("alpha", fallback.alpha).coerceIn(0, 255),
            )
        }
    }
}

data class KeytaoImeTheme(
    val panelBackground: KeytaoColor,
    val panelBorder: KeytaoColor,
    val keyBackground: KeytaoColor,
    val keySelectedBackground: KeytaoColor,
    val keyForeground: KeytaoColor,
    val keySelectedForeground: KeytaoColor,
    val labelColor: KeytaoColor,
    val selectedLabelColor: KeytaoColor,
    val commentColor: KeytaoColor,
    val selectedCommentColor: KeytaoColor,
    val candidateBorderColor: KeytaoColor,
    val candidateSelectedBorderColor: KeytaoColor,
    val candidateBorderWidthDp: Float,
    val candidateSelectedBackground: KeytaoColor,
    val candidateSelectedForeground: KeytaoColor,
    val candidateInlineGapDp: Float,
    val fontSizeSp: Float,
    val labelSizeSp: Float,
    val commentSizeSp: Float,
    val preeditSizeSp: Float,
    val panelCornerRadiusDp: Float,
    val keyCornerRadiusDp: Float,
    val panelGapDp: Float,
    val candidatePaddingXDp: Float,
    val candidatePaddingYDp: Float,
    val modeHintChineseText: String,
    val modeHintEnglishText: String,
) {
    companion object {
        fun fallback(): KeytaoImeTheme = KeytaoImeTheme(
            panelBackground = KeytaoColor(0xF8, 0xFA, 0xFF, 0xF2),
            panelBorder = KeytaoColor.rgb(0xD8, 0xE2, 0xF1),
            keyBackground = KeytaoColor(0, 0, 0, 0),
            keySelectedBackground = KeytaoColor.rgb(0xE6, 0xF0, 0xFF),
            keyForeground = KeytaoColor.rgb(0x1F, 0x29, 0x33),
            keySelectedForeground = KeytaoColor.rgb(0x14, 0x23, 0x3B),
            labelColor = KeytaoColor.rgb(0x6B, 0x77, 0x85),
            selectedLabelColor = KeytaoColor.rgb(0x3B, 0x73, 0xD9),
            commentColor = KeytaoColor.rgb(0x7A, 0x87, 0x90),
            selectedCommentColor = KeytaoColor.rgb(0x52, 0x6A, 0x91),
            candidateBorderColor = KeytaoColor(0, 0, 0, 0),
            candidateSelectedBorderColor = KeytaoColor.rgb(0xA8, 0xC7, 0xFA),
            candidateBorderWidthDp = 0f,
            candidateSelectedBackground = KeytaoColor.rgb(0xE6, 0xF0, 0xFF),
            candidateSelectedForeground = KeytaoColor.rgb(0x14, 0x23, 0x3B),
            candidateInlineGapDp = 5f,
            fontSizeSp = 18f,
            labelSizeSp = 14f,
            commentSizeSp = 13f,
            preeditSizeSp = 15f,
            panelCornerRadiusDp = 14f,
            keyCornerRadiusDp = 9f,
            panelGapDp = 6f,
            candidatePaddingXDp = 11f,
            candidatePaddingYDp = 6f,
            modeHintChineseText = "中",
            modeHintEnglishText = "英",
        )

        fun fromJson(json: String?): KeytaoImeTheme {
            if (json.isNullOrBlank()) return fallback()
            return runCatching {
                val root = JSONObject(json)
                val panel = root.optJSONObject("panel")
                val candidate = root.optJSONObject("candidate")
                val font = root.optJSONObject("font")
                val modeHint = root.optJSONObject("modeHint")
                val fallback = fallback()
                KeytaoImeTheme(
                    panelBackground = KeytaoColor.fromJson(panel?.optJSONObject("background"), fallback.panelBackground),
                    panelBorder = KeytaoColor.fromJson(panel?.optJSONObject("borderColor"), fallback.panelBorder),
                    keyBackground = KeytaoColor.fromJson(candidate?.optJSONObject("background"), fallback.keyBackground),
                    keySelectedBackground = KeytaoColor.fromJson(candidate?.optJSONObject("selectedBackground"), fallback.keySelectedBackground),
                    keyForeground = KeytaoColor.fromJson(candidate?.optJSONObject("foreground"), fallback.keyForeground),
                    keySelectedForeground = KeytaoColor.fromJson(candidate?.optJSONObject("selectedForeground"), fallback.keySelectedForeground),
                    labelColor = KeytaoColor.fromJson(candidate?.optJSONObject("labelColor"), fallback.labelColor),
                    selectedLabelColor = KeytaoColor.fromJson(candidate?.optJSONObject("selectedLabelColor"), fallback.selectedLabelColor),
                    commentColor = KeytaoColor.fromJson(candidate?.optJSONObject("commentColor"), fallback.commentColor),
                    selectedCommentColor = KeytaoColor.fromJson(candidate?.optJSONObject("selectedCommentColor"), fallback.selectedCommentColor),
                    candidateBorderColor = KeytaoColor.fromJson(candidate?.optJSONObject("borderColor"), fallback.candidateBorderColor),
                    candidateSelectedBorderColor = KeytaoColor.fromJson(candidate?.optJSONObject("selectedBorderColor"), fallback.candidateSelectedBorderColor),
                    candidateBorderWidthDp = candidate?.optDouble("borderWidth", fallback.candidateBorderWidthDp.toDouble())?.toFloat()?.coerceIn(0f, 3f)
                        ?: fallback.candidateBorderWidthDp,
                    candidateSelectedBackground = KeytaoColor.fromJson(candidate?.optJSONObject("selectedBackground"), fallback.candidateSelectedBackground),
                    candidateSelectedForeground = KeytaoColor.fromJson(candidate?.optJSONObject("selectedForeground"), fallback.candidateSelectedForeground),
                    candidateInlineGapDp = candidate?.optDouble("inlineGap", fallback.candidateInlineGapDp.toDouble())?.toFloat()?.coerceIn(0f, 18f)
                        ?: fallback.candidateInlineGapDp,
                    fontSizeSp = font?.optDouble("size", fallback.fontSizeSp.toDouble())?.toFloat()?.coerceIn(10f, 36f)
                        ?: fallback.fontSizeSp,
                    labelSizeSp = font?.optDouble("labelSize", fallback.labelSizeSp.toDouble())?.toFloat()?.coerceIn(9f, 28f)
                        ?: fallback.labelSizeSp,
                    commentSizeSp = font?.optDouble("commentSize", fallback.commentSizeSp.toDouble())?.toFloat()?.coerceIn(9f, 28f)
                        ?: fallback.commentSizeSp,
                    preeditSizeSp = font?.optDouble("preeditSize", fallback.preeditSizeSp.toDouble())?.toFloat()?.coerceIn(9f, 28f)
                        ?: fallback.preeditSizeSp,
                    panelCornerRadiusDp = panel?.optDouble("cornerRadius", fallback.panelCornerRadiusDp.toDouble())?.toFloat()?.coerceIn(0f, 28f)
                        ?: fallback.panelCornerRadiusDp,
                    keyCornerRadiusDp = candidate?.optDouble("cornerRadius", fallback.keyCornerRadiusDp.toDouble())?.toFloat()?.coerceIn(0f, 24f)
                        ?: fallback.keyCornerRadiusDp,
                    panelGapDp = panel?.optDouble("gap", fallback.panelGapDp.toDouble())?.toFloat()?.coerceIn(0f, 24f)
                        ?: fallback.panelGapDp,
                    candidatePaddingXDp = candidate?.optDouble("paddingX", fallback.candidatePaddingXDp.toDouble())?.toFloat()?.coerceIn(0f, 28f)
                        ?: fallback.candidatePaddingXDp,
                    candidatePaddingYDp = candidate?.optDouble("paddingY", fallback.candidatePaddingYDp.toDouble())?.toFloat()?.coerceIn(0f, 24f)
                        ?: fallback.candidatePaddingYDp,
                    modeHintChineseText = modeHint?.optString("chineseText", fallback.modeHintChineseText)
                        ?: fallback.modeHintChineseText,
                    modeHintEnglishText = modeHint?.optString("englishText", fallback.modeHintEnglishText)
                        ?: fallback.modeHintEnglishText,
                )
            }.getOrElse { fallback() }
        }
    }
}

object KeytaoThemeResolver {
    fun resolve(): KeytaoImeTheme {
        val userTheme = KeytaoAndroidPaths.themeFile()
        val userThemePath = userTheme.takeIf { it.isFile }?.absolutePath
        return KeytaoImeTheme.fromJson(KeytaoNativeBridge.resolveThemeJson(null, userThemePath))
    }
}
