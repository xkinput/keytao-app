package ink.rea.keytao_app

import android.content.Context
import android.content.res.Configuration
import android.graphics.Color
import android.graphics.Typeface
import org.json.JSONObject

data class KeytaoColor(val red: Int, val green: Int, val blue: Int, val alpha: Int = 255) {
    fun toArgb(): Int = Color.argb(alpha, red, green, blue)
    fun toHex(): String = "#%02X%02X%02X".format(red, green, blue)

    companion object {
        fun fromHex(value: String): KeytaoColor? {
            val hex = value.trim().removePrefix("#")
            if (hex.length != 6 || hex.any { !it.isDigit() && it.lowercaseChar() !in 'a'..'f' }) return null
            return runCatching {
                KeytaoColor(hex.substring(0, 2).toInt(16), hex.substring(2, 4).toInt(16), hex.substring(4, 6).toInt(16))
            }.getOrNull()
        }
    }
}

internal fun JSONObject.themeColor(field: String): KeytaoColor = getJSONObject(field).let {
    KeytaoColor(it.getInt("red"), it.getInt("green"), it.getInt("blue"), it.getInt("alpha"))
}

private fun themeTypeface(weight: String): Typeface {
    val fontWeight = when (weight) {
        "ultralight" -> 100
        "thin" -> 200
        "light" -> 300
        "medium" -> 500
        "semibold" -> 600
        "bold" -> 700
        "heavy" -> 800
        "black" -> 900
        else -> 400
    }
    return if (android.os.Build.VERSION.SDK_INT >= 28) {
        Typeface.create(Typeface.DEFAULT, fontWeight, false)
    } else {
        Typeface.create(Typeface.DEFAULT, if (fontWeight >= 700) Typeface.BOLD else Typeface.NORMAL)
    }
}

class KeytaoKeyStyle(json: JSONObject) {
    val background = json.themeColor("background")
    val foreground = json.themeColor("foreground")
    val hintForeground = json.themeColor("hintForeground")
    val pressedBackground = json.themeColor("pressedBackground")
    val pressedForeground = json.themeColor("pressedForeground")
    val borderColor = json.themeColor("borderColor")
    val borderWidth = json.getDouble("borderWidth").toFloat()
    val cornerRadius = json.getDouble("cornerRadius").toFloat()
    val shadowColor = json.themeColor("shadowColor")
    val shadowOffsetY = json.getDouble("shadowOffsetY").toFloat()
    val fontScale = json.getDouble("fontScale").toFloat()
    val hintFontScale = json.getDouble("hintFontScale").toFloat()
    val swipeHintForeground = json.themeColor("swipeHintForeground")
    val swipeHintVisible = json.getBoolean("swipeHintVisible")
    val typeface = themeTypeface(json.getString("fontWeight"))
}

class KeytaoImeTheme private constructor(root: JSONObject) {
    private val ui = root.getJSONObject("ui")
    private val font = root.getJSONObject("font")
    private val modeHint = root.getJSONObject("modeHint")
    private val mobile = root.getJSONObject("mobile")
    val keyboard = mobile.getJSONObject("keyboard")
    val candidateBar = mobile.getJSONObject("candidateBar")
    val toolbar = keyboard.getJSONObject("toolbar")
    val floating = keyboard.getJSONObject("floating")
    val key = KeytaoKeyStyle(keyboard.getJSONObject("key"))
    val isDark = mobile.getBoolean("isDark")
    val uiColorScheme = ui.getString("colorScheme")
    val effectiveColorScheme = ui.getString("effectiveColorScheme")
    val accentColor = ui.optJSONObject("accentColor")?.let {
        KeytaoColor(it.getInt("red"), it.getInt("green"), it.getInt("blue"), it.getInt("alpha"))
    } ?: candidateBar.themeColor("selectedLabelColor")
    val panelBackground = keyboard.themeColor("background")
    val panelBorder = key.borderColor
    val keyBackground = key.background
    val keyPressedBackground = key.pressedBackground
    val keyPressedForeground = key.pressedForeground
    val keySelectedBackground = keyboard.getJSONObject("keyStates").getJSONObject("selected").themeColor("background")
    val keyForeground = key.foreground
    val keySelectedForeground = keyboard.getJSONObject("keyStates").getJSONObject("selected").themeColor("foreground")
    val labelColor = candidateBar.themeColor("labelColor")
    val selectedLabelColor = candidateBar.themeColor("selectedLabelColor")
    val commentColor = candidateBar.themeColor("commentColor")
    val selectedCommentColor = candidateBar.themeColor("selectedCommentColor")
    val candidateBackground = candidateBar.themeColor("itemBackground")
    val candidateBorderColor = candidateBar.themeColor("borderColor")
    val candidateSelectedBorderColor = candidateBar.themeColor("selectedBorderColor")
    val candidateBorderWidthDp = candidateBar.getDouble("borderWidth").toFloat()
    val candidateSelectedBorderWidthDp = candidateBar.getDouble("selectedBorderWidth").toFloat()
    val candidateCornerRadiusDp = candidateBar.getDouble("cornerRadius").toFloat()
    val candidatePaddingXDp = candidateBar.getDouble("paddingX").toFloat()
    val candidateInlineGapDp = candidateBar.getDouble("inlineGap").toFloat()
    val candidateSelectedBackground = candidateBar.themeColor("selectedBackground")
    val candidateSelectedForeground = candidateBar.themeColor("selectedForeground")
    val candidateForeground = candidateBar.themeColor("foreground")
    val typeface = themeTypeface(font.getString("weight"))
    val panelCornerRadiusDp = key.cornerRadius
    val keyCornerRadiusDp = key.cornerRadius
    val modeHintChineseText = modeHint.getString("chineseText")
    val modeHintEnglishText = modeHint.getString("englishText")
    val candidateFontScale = candidateBar.getDouble("fontScale").toFloat()
    val showComment = candidateBar.getBoolean("showComment")

    private val keyStyleCache = mutableMapOf<String, KeytaoKeyStyle>()

    fun keyStyle(type: String, accent: Boolean, punctuation: Boolean, selected: Boolean, shiftOn: Boolean): KeytaoKeyStyle {
        val cacheKey = "$type:$accent:$punctuation:$selected:$shiftOn"
        return keyStyleCache.getOrPut(cacheKey) {
            val style = JSONObject(keyboard.getJSONObject("key").toString())
            fun overlay(source: JSONObject, colorsOnly: Boolean = false) {
                source.keys().forEach { field ->
                    if (!colorsOnly || source.opt(field) is JSONObject) style.put(field, source.get(field))
                }
            }
            val function = type in setOf("mode", "keyboardMode", "space", "enter", "backspace")
            val types = keyboard.getJSONObject("keyTypes")
            val classes = keyboard.getJSONObject("keyClasses")
            val states = keyboard.getJSONObject("keyStates")
            overlay(types.getJSONObject(if (function) "function" else "character"))
            if (type in setOf("space", "enter", "backspace", "shift")) overlay(types.getJSONObject(type))
            if (accent) overlay(classes.getJSONObject("accent"))
            if (punctuation) overlay(classes.getJSONObject("accentPunctuation"))
            if (selected) {
                val state = states.getJSONObject("selected")
                overlay(state, colorsOnly = true)
                if (function || accent || punctuation) style.put("background", state.getJSONObject("softBackground"))
            }
            if (shiftOn && type == "shift") overlay(states.getJSONObject("shiftOn"), colorsOnly = true)
            KeytaoKeyStyle(style)
        }
    }

    companion object {
        private var bundledDefault: String? = null
        private var defaultTheme: KeytaoImeTheme? = null

        fun loadDefault(context: Context) {
            if (bundledDefault == null) bundledDefault = context.assets.open("keytao-default-theme.json").bufferedReader().use { it.readText() }
        }

        fun fallback(context: Context? = null): KeytaoImeTheme {
            defaultTheme?.let { return it }
            context?.let(::loadDefault)
            val resolved = KeytaoNativeBridge.resolveThemeJson(null, null, null)
                ?.let { runCatching { KeytaoImeTheme(JSONObject(it)) }.getOrNull() }
            return (resolved ?: KeytaoImeTheme(JSONObject(bundledDefault
                ?: error("The bundled theme has not been loaded")))).also { defaultTheme = it }
        }

        fun fromJson(json: String?): KeytaoImeTheme = runCatching {
            KeytaoImeTheme(JSONObject(json ?: error("Theme resolution failed")))
        }.getOrElse {
            KeytaoRuntimeLog.event("error", "theme_resolve_failed")
            fallback()
        }
    }
}

object KeytaoThemeResolver {
    private var cachedSignature: String? = null
    private var cachedTheme: KeytaoImeTheme? = null

    @Synchronized
    fun resolve(context: Context): KeytaoImeTheme {
        KeytaoImeTheme.loadDefault(context)
        val userTheme = KeytaoAndroidPaths.themeFileOrNull(context)
        val scheme = systemColorScheme(context)
        val signature = "${fileSignature(userTheme)}|$scheme"
        cachedTheme?.let { if (signature == cachedSignature) return it }
        val theme = KeytaoImeTheme.fromJson(KeytaoNativeBridge.resolveThemeJson(null, userTheme?.takeIf { it.isFile }?.absolutePath, scheme))
        cachedSignature = signature
        cachedTheme = theme
        return theme
    }

    @Synchronized
    fun invalidate() {
        cachedSignature = null
        cachedTheme = null
    }

    private fun systemColorScheme(context: Context): String {
        val nightMode = context.resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK
        return if (nightMode == Configuration.UI_MODE_NIGHT_YES) "dark" else "light"
    }
}

internal fun fileSignature(file: java.io.File?): String {
    if (file == null) return "-"
    val metadata = if (file.isFile) "${file.length()}:${file.lastModified()}" else "-"
    return "${file.absolutePath}:$metadata"
}
