package ink.rea.keytao_app

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.security.MessageDigest
import kotlin.math.roundToInt

object KeyCommandTypes {
    const val INPUT = "input"
    const val DIRECT_INPUT = "directInput"
    const val RIME_INPUT = "rimeInput"
    const val BACKSPACE = "backspace"
    const val BACKSPACE_GESTURE = "backspaceGesture"
    const val ENTER = "enter"
    const val SPACE = "space"
    const val SHIFT = "shift"
    const val MODE = "mode"
    const val OPEN_PAGE = "openPage"
    const val KEYBOARD_PICKER = "keyboardPicker"
    const val NEXT_INPUT_METHOD = "nextInputMethod"
    const val KEYBOARD_MODE = "keyboardMode"
    const val NEXT_PAGE = "nextCandidatePage"
    const val PREVIOUS_PAGE = "previousCandidatePage"
    const val RESET = "reset"
    const val RIME_MENU = "rimeMenu"
    const val RIME_SCHEMA = "rimeSchema"
    const val RIME_OPTION = "rimeOption"
    const val PANEL = "panel"
    const val EDIT = "edit"
    const val ONE_HANDED = "oneHanded"
    const val FLOATING = "floating"
}

object EnterKeyBehaviors {
    const val SYSTEM = "system"
    const val NEWLINE = "newline"
}

data class KeyCommand(
    val type: String,
    val value: String? = null,
    val fallbackValue: String? = null,
) {
    companion object {
        fun input(value: String, fallbackValue: String? = null) = KeyCommand(KeyCommandTypes.INPUT, value, fallbackValue)
        fun directInput(value: String) = KeyCommand(KeyCommandTypes.DIRECT_INPUT, value)
        fun rimeInput(value: String, fallbackValue: String? = null) =
            KeyCommand(KeyCommandTypes.RIME_INPUT, value, fallbackValue)
        fun panel(value: String) = KeyCommand(KeyCommandTypes.PANEL, value)
        fun edit(value: String, fallbackValue: String? = null) =
            KeyCommand(KeyCommandTypes.EDIT, value, fallbackValue)
    }
}

data class KeyStackItem(
    val label: String,
    val value: String? = null,
    val asciiLabel: String? = null,
    val asciiValue: String? = null,
    val rimeValue: String? = null,
    val action: KeyCommand? = null,
    val asciiAction: KeyCommand? = null,
)

data class KeyAlternate(
    val label: String,
    val value: String? = null,
    val rimeValue: String? = null,
    val action: KeyCommand = KeyCommand.input(value ?: label),
)

data class KeySpec(
    val label: String,
    val value: String,
    val asciiLabel: String? = null,
    val asciiValue: String? = null,
    val rimeValue: String? = null,
    val weight: Float = 1f,
    val style: String? = null,
    val hint: String? = null,
    val action: KeyCommand = KeyCommand.input(value),
    val asciiAction: KeyCommand? = null,
    val swipeUp: KeyCommand? = null,
    val swipeDown: KeyCommand? = null,
    val longPress: KeyCommand? = null,
    val asciiLongPress: KeyCommand? = null,
    val alternates: List<KeyAlternate> = emptyList(),
    val asciiAlternates: List<KeyAlternate> = emptyList(),
    val rowSpan: Int = 1,
    val stack: List<KeyStackItem> = emptyList(),
)

data class FloatingKeyboardProfile(
    val enabled: Boolean,
    val scale: Float,
)

data class FloatingKeyboardConfig(
    val marginDp: Float,
    val portrait: FloatingKeyboardProfile,
    val landscape: FloatingKeyboardProfile,
) {
    fun profile(isLandscape: Boolean): FloatingKeyboardProfile {
        return if (isLandscape) landscape else portrait
    }
}

data class KeytaoAndroidImeConfig(
    val keyboardHeightDp: Int,
    val candidateBarHeightDp: Int,
    val clipboardRowHeightDp: Float,
    val clipboardDeleteHitWidthDp: Float,
    val keyboardBottomInsetDp: Int,
    val horizontalGapDp: Float,
    val verticalGapDp: Float,
    val outerInsetDp: Float,
    val maxKeyHeightDp: Float,
    val floating: FloatingKeyboardConfig,
    val hapticsEnabled: Boolean,
    val hapticIntensity: Int,
    val enterKeyBehavior: String,
    val keyPreviewEnabled: Boolean,
    val longPressDelayMs: Long,
    val keyboardHeightScale: Int,
    val deleteSpeed: String,
    val keySoundEnabled: Boolean,
    val keySoundVolume: Int,
    val keyHintVisible: Boolean,
    val flickKeysEnabled: Boolean,
    val numberRowEnabled: Boolean,
    val candidateFontScale: Float,
    val doubleSpacePeriodEnabled: Boolean,
    val swipeThresholdDp: Float,
    val rows: List<List<KeySpec>>,
    val numberRows: List<List<KeySpec>>,
    val symbolRows: List<List<KeySpec>>,
    val customRows: Map<String, List<List<KeySpec>>> = emptyMap(),
) {
    val effectiveKeyboardHeightDp: Float
        get() {
            val baseHeight = if (!numberRowEnabled || rows.isEmpty()) {
                keyboardHeightDp.toFloat()
            } else {
                keyboardHeightDp * (rows.size + 1f) / rows.size
            }
            return baseHeight * keyboardHeightScaleFactor
        }

    val keyboardHeightScaleFactor: Float
        get() = keyboardHeightScale / 100f

    fun rowsForLayer(layer: String): List<List<KeySpec>> {
        return when (layer) {
            "numbers" -> numberRows
            "symbols" -> symbolRows
            "letters" -> rows
            else -> customRows[layer] ?: rows
        }
    }

    fun hasLayer(layer: String): Boolean {
        return layer == "letters" || layer == "numbers" || layer == "symbols" || customRows.containsKey(layer)
    }

    fun normalizedLayer(layer: String?): String {
        val value = layer?.takeIf { it.isNotBlank() } ?: "letters"
        return if (hasLayer(value)) value else "letters"
    }

    fun scaledForFloating(
        profile: FloatingKeyboardProfile,
        isLandscape: Boolean = false,
    ): KeytaoAndroidImeConfig {
        val widthScale = profile.scale.coerceIn(
            KeyboardLayoutState.minimumFloatingScale(isLandscape),
            1f,
        )
        if (!profile.enabled || widthScale >= 0.999f) return this
        return scaledForCompact(
            horizontalScale = widthScale,
            verticalScale = KeyboardLayoutState.heightScaleForFloatingWidth(widthScale, isLandscape),
            clearBottomInset = true,
        )
    }

    fun scaledForOneHanded(requestedScale: Float): KeytaoAndroidImeConfig {
        val scale = requestedScale.coerceIn(minOneHandedScale, 1f)
        if (scale >= 0.999f) return this
        return scaledForCompact(
            horizontalScale = scale,
            verticalScale = scale,
            clearBottomInset = false,
        )
    }

    private fun scaledForCompact(
        horizontalScale: Float,
        verticalScale: Float,
        clearBottomInset: Boolean,
    ): KeytaoAndroidImeConfig {
        return copy(
            keyboardHeightDp = (keyboardHeightDp * verticalScale).roundToInt().coerceAtLeast(120),
            candidateBarHeightDp = (candidateBarHeightDp * verticalScale).roundToInt().coerceAtLeast(32),
            keyboardBottomInsetDp = if (clearBottomInset) 0 else keyboardBottomInsetDp,
            horizontalGapDp = horizontalGapDp * horizontalScale,
            verticalGapDp = verticalGapDp * verticalScale,
            outerInsetDp = outerInsetDp * horizontalScale,
            maxKeyHeightDp = (maxKeyHeightDp * verticalScale).coerceAtLeast(30f),
            swipeThresholdDp = (swipeThresholdDp * minOf(horizontalScale, verticalScale)).coerceAtLeast(12f),
        )
    }

    companion object {
        private const val minOneHandedScale = 0.78f
        private const val keyboardHeightScaleMin = 85
        private const val keyboardHeightScaleMax = 130
        private const val keyboardHeightScaleStep = 5
        private const val keyboardHeightScaleDefault = 100
        private const val keyboardSeedFileName = ".keyboard.seed"
        private val legacyDefaultKeyboardHashes = setOf(
            "e4d7aa7445ac138286941d095017ee7d9e397ecc5501cfb744482835538e5329",
            "3ebe95295376bfeffb79c0106f86bc7e3d8631311dae0c595d330ed4c1b2805c",
            "25ae1b176617c64fec16a55b73c9faae8abd17c3eba12d1fc67ec9f66364b854",
            "34475509153894fcd8e53f5d21b1f6d0852b800731d670e9cfe7694cdf64df2c",
        )

        private var cachedSignature: String? = null
        private var cachedConfig: KeytaoAndroidImeConfig? = null

        /**
         * Write the bundled keyboard.yaml when the user has none, or reseed it
         * when the sidecar hash proves it is still untouched. Blocking and
         * writing, so it belongs on a background thread — [load] never does it.
         */
        fun ensureDefaults(context: Context) {
            ensureDefaultKeyboardConfig(context)
        }

        /**
         * Parsing means reading keyboard.yaml plus android_ime.json and crossing
         * JNI twice, so the result is cached until one of those files changes.
         * Only the cache miss touches the disk; the steady state costs two stats.
         */
        @Synchronized
        fun load(context: Context): KeytaoAndroidImeConfig {
            val userConfig = KeytaoAndroidPaths.imeConfigFile(context)
            val userKeyboardFile = KeytaoAndroidPaths.keyboardFile(context)
            val signature = "${fileSignature(userConfig)}|${fileSignature(userKeyboardFile)}"
            cachedConfig?.let { config ->
                if (signature == cachedSignature) return config
            }
            val defaultJson = context.resources
                .openRawResource(R.raw.keytao_android_ime)
                .bufferedReader()
                .use { it.readText() }
            val userJson = userConfig.takeIf { it.isFile }?.readText()
            val userKeyboard = resolvedUserKeyboard(context)
            val defaultRoot = JSONObject(defaultJson)
            val parsed = runCatching {
                val root = userKeyboard ?: userJson?.let { JSONObject(it) } ?: defaultRoot
                val fallbackRoot = when {
                    userKeyboard != null && userJson != null -> JSONObject(userJson)
                    userKeyboard != null -> defaultRoot
                    userJson != null -> defaultRoot
                    else -> null
                }
                parseRoot(root, fallbackRoot)
            }.getOrElse { parseRoot(defaultRoot, null) }
            val config = applyRuntimeSettings(
                parsed,
                userJson?.let { runCatching { JSONObject(it) }.getOrNull() },
            )
            cachedSignature = signature
            cachedConfig = config
            return config
        }

        fun parse(json: String): KeytaoAndroidImeConfig {
            val root = JSONObject(json)
            return parseRoot(root, null)
        }

        private fun parse(json: String, defaultJson: String): KeytaoAndroidImeConfig {
            return parseRoot(JSONObject(json), JSONObject(defaultJson))
        }

        private fun parseRoot(root: JSONObject, fallbackRoot: JSONObject?): KeytaoAndroidImeConfig {
            val rows = rowArray(root, fallbackRoot, "rows")
                ?.let { normalizeRows(parseRows(it)) }
                .orEmpty()
            val numberRows = rowArray(root, fallbackRoot, "numberRows")
                ?.let { normalizeNumberRows(parseRows(it)) }
                .orEmpty()
            val symbolRows = rowArray(root, fallbackRoot, "symbolRows")
                ?.let { normalizeRows(parseRows(it)) }
                .orEmpty()
            val customRows = layerRows(root, fallbackRoot)
            val haptics = root.optJSONObject("haptics")
            val fallbackHaptics = fallbackRoot?.optJSONObject("haptics")
            val floating = parseFloatingConfig(root, fallbackRoot)
            return KeytaoAndroidImeConfig(
                keyboardHeightDp = mergedInt(root, fallbackRoot, listOf("height", "heightDp", "keyboardHeightDp"), 246)
                    .coerceIn(160, 420),
                candidateBarHeightDp = mergedInt(
                    root,
                    fallbackRoot,
                    listOf("candidateBarHeight", "candidateBarHeightDp"),
                    52,
                ).coerceIn(36, 96),
                clipboardRowHeightDp = mergedDouble(
                    root,
                    fallbackRoot,
                    listOf("clipboardRowHeight", "clipboardRowHeightDp"),
                    44.0,
                ).toFloat().coerceIn(44f, 44f),
                clipboardDeleteHitWidthDp = mergedDouble(
                    root,
                    fallbackRoot,
                    listOf("clipboardDeleteHitWidth", "clipboardDeleteHitWidthDp"),
                    44.0,
                ).toFloat().coerceIn(44f, 44f),
                keyboardBottomInsetDp = mergedInt(root, fallbackRoot, listOf("bottomInset", "bottomInsetDp", "keyboardBottomInsetDp"), 48)
                    .coerceIn(0, 80),
                horizontalGapDp = mergedDouble(root, fallbackRoot, listOf("horizontalGap", "horizontalGapDp"), 4.0)
                    .toFloat()
                    .coerceIn(0f, 24f),
                verticalGapDp = mergedDouble(root, fallbackRoot, listOf("verticalGap", "verticalGapDp"), 5.0)
                    .toFloat()
                    .coerceIn(0f, 24f),
                outerInsetDp = mergedDouble(root, fallbackRoot, listOf("outerInset", "outerInsetDp"), 5.0)
                    .toFloat()
                    .coerceIn(0f, 32f),
                maxKeyHeightDp = mergedDouble(root, fallbackRoot, listOf("maxKeyHeight", "maxKeyHeightDp"), 54.0)
                    .toFloat()
                    .coerceIn(36f, 84f),
                floating = floating,
                hapticsEnabled = mergedBoolean(root, fallbackRoot, haptics, fallbackHaptics, "enabled", "hapticsEnabled", true),
                hapticIntensity = mergedInt(root, fallbackRoot, haptics, fallbackHaptics, "intensity", "hapticIntensity", 42)
                    .coerceIn(1, 100),
                enterKeyBehavior = normalizeEnterKeyBehavior(
                    mergedString(root, fallbackRoot, "enterKeyBehavior", EnterKeyBehaviors.SYSTEM),
                ),
                keyPreviewEnabled = mergedBoolean(root, fallbackRoot, "keyPreviewEnabled", true),
                longPressDelayMs = mergedInt(
                    root,
                    fallbackRoot,
                    "longPressDelayMs",
                    KeytaoImeInteractionTuning.LONG_PRESS_DELAY_DEFAULT_MS.toInt(),
                ).toLong().coerceIn(
                    KeytaoImeInteractionTuning.LONG_PRESS_DELAY_MIN_MS,
                    KeytaoImeInteractionTuning.LONG_PRESS_DELAY_MAX_MS,
                ),
                keyboardHeightScale = mergedKeyboardHeightScale(root, fallbackRoot),
                deleteSpeed = normalizeDeleteSpeed(mergedString(root, fallbackRoot, "deleteSpeed", "standard")),
                keySoundEnabled = mergedBoolean(root, fallbackRoot, "keySoundEnabled", true),
                keySoundVolume = mergedInt(root, fallbackRoot, "keySoundVolume", 100).coerceIn(0, 100),
                keyHintVisible = mergedBoolean(root, fallbackRoot, "keyHintVisible", true),
                flickKeysEnabled = mergedBoolean(root, fallbackRoot, "flickKeysEnabled", true),
                numberRowEnabled = mergedBoolean(root, fallbackRoot, "numberRowEnabled", false),
                candidateFontScale = mergedDouble(root, fallbackRoot, "candidateFontScale", 1.0)
                    .toFloat()
                    .coerceIn(0.8f, 1.4f),
                doubleSpacePeriodEnabled = mergedBoolean(root, fallbackRoot, "doubleSpacePeriodEnabled", true),
                swipeThresholdDp = mergedDouble(root, fallbackRoot, "swipeThresholdDp", 34.0).toFloat().coerceIn(12f, 96f),
                rows = rows.ifEmpty { defaultRows() },
                numberRows = numberRows.ifEmpty { defaultNumberRows() },
                symbolRows = symbolRows.ifEmpty { defaultSymbolRows() },
                customRows = customRows,
            )
        }

        private fun parseFloatingConfig(root: JSONObject, fallbackRoot: JSONObject?): FloatingKeyboardConfig {
            val floating = root.optJSONObject("floating")
            val fallbackFloating = fallbackRoot?.optJSONObject("floating")
            return FloatingKeyboardConfig(
                marginDp = mergedFloatingDouble(floating, fallbackFloating, "margin", 8.0)
                    .toFloat()
                    .coerceIn(0f, 24f),
                portrait = parseFloatingProfile(
                    floating?.optJSONObject("portrait"),
                    fallbackFloating?.optJSONObject("portrait"),
                    defaultEnabled = false,
                    defaultScale = 0.88f,
                    minimumScale = KeyboardLayoutState.MIN_PORTRAIT_FLOATING_SCALE,
                ),
                landscape = parseFloatingProfile(
                    floating?.optJSONObject("landscape"),
                    fallbackFloating?.optJSONObject("landscape"),
                    defaultEnabled = true,
                    defaultScale = 0.62f,
                    minimumScale = KeyboardLayoutState.MIN_LANDSCAPE_FLOATING_SCALE,
                ),
            )
        }

        private fun parseFloatingProfile(
            root: JSONObject?,
            fallbackRoot: JSONObject?,
            defaultEnabled: Boolean,
            defaultScale: Float,
            minimumScale: Float,
        ): FloatingKeyboardProfile {
            val enabled = when {
                root?.has("enabled") == true -> root.optBoolean("enabled", defaultEnabled)
                fallbackRoot?.has("enabled") == true -> fallbackRoot.optBoolean("enabled", defaultEnabled)
                else -> defaultEnabled
            }
            val scale = mergedFloatingDouble(root, fallbackRoot, "scale", defaultScale.toDouble())
            return FloatingKeyboardProfile(enabled, normalizeFloatingScale(scale, minimumScale))
        }

        private fun mergedFloatingDouble(
            root: JSONObject?,
            fallbackRoot: JSONObject?,
            name: String,
            defaultValue: Double,
        ): Double {
            return when {
                root?.has(name) == true -> root.optDouble(name, defaultValue)
                fallbackRoot?.has(name) == true -> fallbackRoot.optDouble(name, defaultValue)
                else -> defaultValue
            }
        }

        private fun normalizeFloatingScale(value: Double, minimumScale: Float): Float {
            val ratio = if (value > 1.5) value / 100.0 else value
            return ratio.toFloat().coerceIn(minimumScale, 1f)
        }

        private fun mergedKeyboardHeightScale(root: JSONObject, fallbackRoot: JSONObject?): Int {
            val value = when {
                root.has("keyboardHeightScale") -> root.opt("keyboardHeightScale")
                fallbackRoot?.has("keyboardHeightScale") == true -> fallbackRoot.opt("keyboardHeightScale")
                else -> null
            }
            return normalizeKeyboardHeightScale(value)
        }

        private fun normalizeKeyboardHeightScale(value: Any?): Int {
            val numericValue = (value as? Number)?.toDouble() ?: return keyboardHeightScaleDefault
            return if (
                numericValue.isFinite() &&
                numericValue >= keyboardHeightScaleMin &&
                numericValue <= keyboardHeightScaleMax &&
                numericValue % keyboardHeightScaleStep == 0.0
            ) {
                numericValue.toInt()
            } else {
                keyboardHeightScaleDefault
            }
        }

        private fun applyRuntimeSettings(
            config: KeytaoAndroidImeConfig,
            runtimeRoot: JSONObject?,
        ): KeytaoAndroidImeConfig {
            if (runtimeRoot == null) return config
            val haptics = runtimeRoot.optJSONObject("haptics")
            val floating = runtimeRoot.optJSONObject("floating")
            return config.copy(
                hapticsEnabled = when {
                    haptics?.has("enabled") == true -> haptics.optBoolean("enabled", config.hapticsEnabled)
                    runtimeRoot.has("hapticsEnabled") -> runtimeRoot.optBoolean("hapticsEnabled", config.hapticsEnabled)
                    else -> config.hapticsEnabled
                },
                hapticIntensity = when {
                    haptics?.has("intensity") == true -> haptics.optInt("intensity", config.hapticIntensity)
                    runtimeRoot.has("hapticIntensity") -> runtimeRoot.optInt("hapticIntensity", config.hapticIntensity)
                    else -> config.hapticIntensity
                }.coerceIn(1, 100),
                enterKeyBehavior = if (runtimeRoot.has("enterKeyBehavior")) {
                    normalizeEnterKeyBehavior(runtimeRoot.optString("enterKeyBehavior"))
                } else {
                    config.enterKeyBehavior
                },
                keyPreviewEnabled = if (runtimeRoot.has("keyPreviewEnabled")) {
                    runtimeRoot.optBoolean("keyPreviewEnabled", config.keyPreviewEnabled)
                } else {
                    config.keyPreviewEnabled
                },
                longPressDelayMs = if (runtimeRoot.has("longPressDelayMs")) {
                    runtimeRoot.optLong("longPressDelayMs", config.longPressDelayMs).coerceIn(
                        KeytaoImeInteractionTuning.LONG_PRESS_DELAY_MIN_MS,
                        KeytaoImeInteractionTuning.LONG_PRESS_DELAY_MAX_MS,
                    )
                } else {
                    config.longPressDelayMs
                },
                keyboardHeightScale = if (runtimeRoot.has("keyboardHeightScale")) {
                    normalizeKeyboardHeightScale(runtimeRoot.opt("keyboardHeightScale"))
                } else {
                    config.keyboardHeightScale
                },
                deleteSpeed = if (runtimeRoot.has("deleteSpeed")) {
                    normalizeDeleteSpeed(runtimeRoot.optString("deleteSpeed"))
                } else {
                    config.deleteSpeed
                },
                keyboardHeightDp = if (runtimeRoot.has("keyboardHeightDp")) {
                    runtimeRoot.optInt("keyboardHeightDp", config.keyboardHeightDp).coerceIn(160, 420)
                } else {
                    config.keyboardHeightDp
                },
                candidateBarHeightDp = if (runtimeRoot.has("candidateBarHeightDp")) {
                    runtimeRoot.optInt("candidateBarHeightDp", config.candidateBarHeightDp).coerceIn(36, 96)
                } else {
                    config.candidateBarHeightDp
                },
                swipeThresholdDp = if (runtimeRoot.has("swipeThresholdDp")) {
                    runtimeRoot.optDouble("swipeThresholdDp", config.swipeThresholdDp.toDouble())
                        .toFloat()
                        .coerceIn(12f, 96f)
                } else {
                    config.swipeThresholdDp
                },
                keySoundEnabled = if (runtimeRoot.has("keySoundEnabled")) {
                    runtimeRoot.optBoolean("keySoundEnabled", config.keySoundEnabled)
                } else {
                    config.keySoundEnabled
                },
                keySoundVolume = if (runtimeRoot.has("keySoundVolume")) {
                    runtimeRoot.optInt("keySoundVolume", config.keySoundVolume).coerceIn(0, 100)
                } else {
                    config.keySoundVolume
                },
                keyHintVisible = if (runtimeRoot.has("keyHintVisible")) {
                    runtimeRoot.optBoolean("keyHintVisible", config.keyHintVisible)
                } else {
                    config.keyHintVisible
                },
                flickKeysEnabled = if (runtimeRoot.has("flickKeysEnabled")) {
                    runtimeRoot.optBoolean("flickKeysEnabled", config.flickKeysEnabled)
                } else {
                    config.flickKeysEnabled
                },
                numberRowEnabled = if (runtimeRoot.has("numberRowEnabled")) {
                    runtimeRoot.optBoolean("numberRowEnabled", config.numberRowEnabled)
                } else {
                    config.numberRowEnabled
                },
                candidateFontScale = if (runtimeRoot.has("candidateFontScale")) {
                    runtimeRoot.optDouble("candidateFontScale", config.candidateFontScale.toDouble())
                        .toFloat()
                        .coerceIn(0.8f, 1.4f)
                } else {
                    config.candidateFontScale
                },
                doubleSpacePeriodEnabled = if (runtimeRoot.has("doubleSpacePeriodEnabled")) {
                    runtimeRoot.optBoolean("doubleSpacePeriodEnabled", config.doubleSpacePeriodEnabled)
                } else {
                    config.doubleSpacePeriodEnabled
                },
                floating = config.floating.copy(
                    marginDp = floating?.optDouble("margin", config.floating.marginDp.toDouble())
                        ?.toFloat()
                        ?.coerceIn(0f, 24f)
                        ?: config.floating.marginDp,
                    portrait = applyRuntimeFloatingProfile(
                        config.floating.portrait,
                        floating?.optJSONObject("portrait"),
                        KeyboardLayoutState.MIN_PORTRAIT_FLOATING_SCALE,
                    ),
                    landscape = applyRuntimeFloatingProfile(
                        config.floating.landscape,
                        floating?.optJSONObject("landscape"),
                        KeyboardLayoutState.MIN_LANDSCAPE_FLOATING_SCALE,
                    ),
                ),
            )
        }

        private fun applyRuntimeFloatingProfile(
            profile: FloatingKeyboardProfile,
            runtime: JSONObject?,
            minimumScale: Float,
        ): FloatingKeyboardProfile {
            if (runtime == null) return profile
            return profile.copy(
                enabled = if (runtime.has("enabled")) {
                    runtime.optBoolean("enabled", profile.enabled)
                } else {
                    profile.enabled
                },
                scale = if (runtime.has("scale")) {
                    normalizeFloatingScale(runtime.optDouble("scale", profile.scale.toDouble()), minimumScale)
                } else {
                    profile.scale
                },
            )
        }

        private fun rowArray(root: JSONObject, fallbackRoot: JSONObject?, name: String): JSONArray? {
            return root.optJSONArray(name) ?: fallbackRoot?.optJSONArray(name)
        }

        private fun layerRows(root: JSONObject, fallbackRoot: JSONObject?): Map<String, List<List<KeySpec>>> {
            return parseLayerRows(fallbackRoot).toMutableMap().apply {
                putAll(parseLayerRows(root))
            }.filterKeys { it.isNotBlank() && it !in builtInLayers }
        }

        private fun parseLayerRows(root: JSONObject?): Map<String, List<List<KeySpec>>> {
            if (root == null) return emptyMap()
            val layers = root.optJSONObject("layers")
                ?: root.optJSONObject("pages")
                ?: root.optJSONObject("keyboards")
                ?: return emptyMap()
            return buildMap {
                val names = layers.keys()
                while (names.hasNext()) {
                    val name = names.next()
                    val rows = when (val value = layers.opt(name)) {
                        is JSONArray -> value
                        is JSONObject -> value.optJSONArray("rows")
                        else -> null
                    } ?: continue
                    val parsed = normalizeRows(parseRows(rows))
                    if (parsed.isNotEmpty()) put(name, parsed)
                }
            }
        }

        private fun ensureDefaultKeyboardConfig(context: Context) {
            val file = KeytaoAndroidPaths.keyboardFile(context)
            val seedFile = File(KeytaoAndroidPaths.userRoot(context), keyboardSeedFileName)
            val yaml = KeytaoNativeBridge.defaultKeyboardYaml() ?: return
            val bundledHash = sha256(yaml)
            if (file.isFile) {
                val existing = runCatching { file.readText() }.getOrNull()
                    ?: return
                val seededHash = runCatching { seedFile.readText().trim() }.getOrNull()
                if (!shouldRefreshDefaultKeyboard(existing, bundledHash, seededHash)) {
                    if (sha256(existing) == bundledHash && seededHash != bundledHash) {
                        runCatching { seedFile.writeText(bundledHash) }
                    }
                    return
                }
            }
            runCatching {
                file.parentFile?.mkdirs()
                file.writeText(yaml)
                seedFile.writeText(bundledHash)
            }
        }

        private fun shouldRefreshDefaultKeyboard(
            existing: String,
            bundledHash: String,
            seededHash: String?,
        ): Boolean {
            if (existing.isBlank()) return true
            val existingHash = sha256(existing)
            if (existingHash == bundledHash) return false
            return if (seededHash.isNullOrBlank()) {
                existingHash in legacyDefaultKeyboardHashes
            } else {
                existingHash == seededHash
            }
        }

        private fun sha256(value: String): String {
            return MessageDigest.getInstance("SHA-256")
                .digest(value.toByteArray(Charsets.UTF_8))
                .joinToString("") { byte -> "%02x".format(byte.toInt() and 0xff) }
        }

        /**
         * The mobile layout lives in keyboard.yaml only; keytao-theme dropped the
         * `keyboard:` section from the shared theme model, so theme.yaml is no
         * longer consulted for key rows.
         */
        private fun resolvedUserKeyboard(context: Context): JSONObject? {
            return runCatching {
                val userKeyboard = KeytaoAndroidPaths.keyboardFile(context)
                if (!userKeyboard.isFile) return@runCatching null
                val json = KeytaoNativeBridge.resolveKeyboardJson(null, userKeyboard.absolutePath)
                    ?: return@runCatching null
                JSONObject(json)
            }.getOrNull()
        }

        private fun mergedInt(root: JSONObject, fallbackRoot: JSONObject?, names: List<String>, defaultValue: Int): Int {
            for (name in names) {
                if (root.has(name)) return root.optInt(name, defaultValue)
            }
            if (fallbackRoot != null) {
                for (name in names) {
                    if (fallbackRoot.has(name)) return fallbackRoot.optInt(name, defaultValue)
                }
            }
            return defaultValue
        }

        private fun mergedDouble(root: JSONObject, fallbackRoot: JSONObject?, names: List<String>, defaultValue: Double): Double {
            for (name in names) {
                if (root.has(name)) return root.optDouble(name, defaultValue)
            }
            if (fallbackRoot != null) {
                for (name in names) {
                    if (fallbackRoot.has(name)) return fallbackRoot.optDouble(name, defaultValue)
                }
            }
            return defaultValue
        }

        private fun mergedInt(root: JSONObject, fallbackRoot: JSONObject?, name: String, defaultValue: Int): Int {
            return when {
                root.has(name) -> root.optInt(name, defaultValue)
                fallbackRoot?.has(name) == true -> fallbackRoot.optInt(name, defaultValue)
                else -> defaultValue
            }
        }

        private fun mergedDouble(root: JSONObject, fallbackRoot: JSONObject?, name: String, defaultValue: Double): Double {
            return when {
                root.has(name) -> root.optDouble(name, defaultValue)
                fallbackRoot?.has(name) == true -> fallbackRoot.optDouble(name, defaultValue)
                else -> defaultValue
            }
        }

        private fun mergedString(root: JSONObject, fallbackRoot: JSONObject?, name: String, defaultValue: String): String {
            return when {
                root.has(name) -> root.optString(name, defaultValue)
                fallbackRoot?.has(name) == true -> fallbackRoot.optString(name, defaultValue)
                else -> defaultValue
            }
        }

        private fun normalizeEnterKeyBehavior(value: String): String {
            return when (value.trim().lowercase()) {
                "newline", "linebreak", "line_break" -> EnterKeyBehaviors.NEWLINE
                else -> EnterKeyBehaviors.SYSTEM
            }
        }

        private fun normalizeDeleteSpeed(value: String): String {
            return when (DeleteSpeed.fromSetting(value)) {
                DeleteSpeed.SLOW -> "slow"
                DeleteSpeed.STANDARD -> "standard"
                DeleteSpeed.FAST -> "fast"
            }
        }

        private fun mergedBoolean(
            root: JSONObject,
            fallbackRoot: JSONObject?,
            name: String,
            defaultValue: Boolean,
        ): Boolean {
            return when {
                root.has(name) -> root.optBoolean(name, defaultValue)
                fallbackRoot?.has(name) == true -> fallbackRoot.optBoolean(name, defaultValue)
                else -> defaultValue
            }
        }

        private fun mergedInt(
            root: JSONObject,
            fallbackRoot: JSONObject?,
            nested: JSONObject?,
            fallbackNested: JSONObject?,
            nestedName: String,
            flatName: String,
            defaultValue: Int,
        ): Int {
            return when {
                nested?.has(nestedName) == true -> nested.optInt(nestedName, defaultValue)
                root.has(flatName) -> root.optInt(flatName, defaultValue)
                fallbackNested?.has(nestedName) == true -> fallbackNested.optInt(nestedName, defaultValue)
                fallbackRoot?.has(flatName) == true -> fallbackRoot.optInt(flatName, defaultValue)
                else -> defaultValue
            }
        }

        private fun mergedBoolean(
            root: JSONObject,
            fallbackRoot: JSONObject?,
            nested: JSONObject?,
            fallbackNested: JSONObject?,
            nestedName: String,
            flatName: String,
            defaultValue: Boolean,
        ): Boolean {
            return when {
                nested?.has(nestedName) == true -> nested.optBoolean(nestedName, defaultValue)
                root.has(flatName) -> root.optBoolean(flatName, defaultValue)
                fallbackNested?.has(nestedName) == true -> fallbackNested.optBoolean(nestedName, defaultValue)
                fallbackRoot?.has(flatName) == true -> fallbackRoot.optBoolean(flatName, defaultValue)
                else -> defaultValue
            }
        }

        private fun normalizeNumberRows(rows: List<List<KeySpec>>): List<List<KeySpec>> {
            return normalizeRows(rows).map { row ->
                row.map { key ->
                    if (key.label == "#+=" && key.action.type == KeyCommandTypes.INPUT) {
                        key.copy(
                            value = "",
                            action = KeyCommand(KeyCommandTypes.KEYBOARD_MODE, "symbols"),
                        )
                    } else {
                        key
                    }
                }
            }
        }

        private fun normalizeRows(rows: List<List<KeySpec>>): List<List<KeySpec>> {
            return rows.map { row ->
                row.map { key ->
                    when (key.label) {
                        "，" -> key.withAsciiVariant(",", ",")
                        "。" -> key.withAsciiVariant(".", ".")
                        else -> key
                    }
                }
            }
        }

        private fun KeySpec.withAsciiVariant(label: String, value: String): KeySpec {
            if (asciiLabel != null || asciiValue != null || asciiAction != null) return this
            return copy(asciiLabel = label, asciiValue = value)
        }

        private fun parseRows(rows: JSONArray): List<List<KeySpec>> {
            return buildList {
                for (rowIndex in 0 until rows.length()) {
                    val row = rows.optJSONArray(rowIndex) ?: continue
                    val keys = buildList {
                        for (keyIndex in 0 until row.length()) {
                            val key = row.optJSONObject(keyIndex) ?: continue
                            add(parseKey(key))
                        }
                    }
                    if (keys.isNotEmpty()) add(keys)
                }
            }
        }

        private fun parseKey(json: JSONObject): KeySpec {
            val label = json.optString("label", "")
            val value = json.optString("value", label)
            return KeySpec(
                label = label,
                value = value,
                asciiLabel = json.optString("asciiLabel").takeIf { it.isNotBlank() },
                asciiValue = json.optString("asciiValue").takeIf { it.isNotBlank() },
                rimeValue = json.optString("rimeValue").takeIf { it.isNotBlank() },
                weight = json.optDouble("weight", 1.0).toFloat().coerceIn(0.25f, 8f),
                style = json.optString("style").takeIf { it.isNotBlank() },
                hint = json.optString("hint").takeIf { it.isNotBlank() },
                action = parseCommand(json.opt("action"), value),
                asciiAction = parseOptionalCommand(json.opt("asciiAction")),
                swipeUp = parseOptionalCommand(json.opt("swipeUp")),
                swipeDown = parseOptionalCommand(json.opt("swipeDown")),
                longPress = parseOptionalCommand(json.opt("longPress")),
                asciiLongPress = parseOptionalCommand(json.opt("asciiLongPress")),
                alternates = parseAlternates(json.optJSONArray("alternates")),
                asciiAlternates = parseAlternates(json.optJSONArray("asciiAlternates")),
                rowSpan = json.optInt("rowSpan", 1).coerceIn(1, 8),
                stack = parseKeyStack(json.optJSONArray("stack")),
            )
        }

        private fun parseAlternates(alternates: JSONArray?): List<KeyAlternate> {
            if (alternates == null) return emptyList()
            return buildList {
                for (index in 0 until alternates.length()) {
                    val alternate = alternates.optJSONObject(index) ?: continue
                    val label = alternate.optString("label").takeIf { it.isNotBlank() } ?: continue
                    val value = alternate.optString("value").takeIf { it.isNotBlank() }
                    val rimeValue = alternate.optString("rimeValue").takeIf { it.isNotBlank() }
                    val action = parseOptionalCommand(alternate.opt("action"))
                        ?: rimeValue?.let { KeyCommand.rimeInput(it, value ?: label) }
                        ?: KeyCommand.input(value ?: label)
                    add(KeyAlternate(label = label, value = value, rimeValue = rimeValue, action = action))
                }
            }
        }

        private fun parseKeyStack(stack: JSONArray?): List<KeyStackItem> {
            if (stack == null) return emptyList()
            return buildList {
                for (index in 0 until stack.length()) {
                    val item = stack.optJSONObject(index) ?: continue
                    val label = item.optString("label", "")
                    val value = item.optString("value").takeIf { it.isNotBlank() }
                    add(
                        KeyStackItem(
                            label = label,
                            value = value,
                            asciiLabel = item.optString("asciiLabel").takeIf { it.isNotBlank() },
                            asciiValue = item.optString("asciiValue").takeIf { it.isNotBlank() },
                            rimeValue = item.optString("rimeValue").takeIf { it.isNotBlank() },
                            action = parseOptionalCommand(item.opt("action")),
                            asciiAction = parseOptionalCommand(item.opt("asciiAction")),
                        ),
                    )
                }
            }
        }

        private fun parseOptionalCommand(value: Any?): KeyCommand? {
            if (value == null || value == JSONObject.NULL) return null
            return parseCommand(value, "")
        }

        private fun parseCommand(value: Any?, fallbackValue: String): KeyCommand {
            return when (value) {
                is JSONObject -> {
                    val type = value.optString("type", KeyCommandTypes.INPUT)
                    val commandValue = value.optString("value").takeIf { it.isNotBlank() }
                    val commandFallbackValue = value.optString("fallbackValue").takeIf { it.isNotBlank() }
                    KeyCommand(type, commandValue, commandFallbackValue)
                }
                is String -> KeyCommand.input(value)
                else -> KeyCommand.input(fallbackValue)
            }
        }

        private fun defaultRows(): List<List<KeySpec>> = listOf(
            listOf(
                letterKey("q", "1"),
                letterKey("w", "2"),
                letterKey("e", "3"),
                letterKey("r", "4"),
                letterKey("t", "5"),
                letterKey("y", "6"),
                letterKey("u", "7"),
                letterKey("i", "8"),
                letterKey("o", "9"),
                letterKey("p", "0"),
            ),
            listOf(
                letterKey("a", "@"),
                letterKey("s", "#"),
                letterKey("d", "$"),
                letterKey("f", "%"),
                letterKey("g", "&"),
                letterKey("h", "*"),
                letterKey("j", "("),
                letterKey("k", ")"),
                letterKey("l", "_"),
            ),
            listOf(
                letterKey("z", "-"),
                letterKey("x", "/"),
                letterKey("c", ":"),
                letterKey("v", ";"),
                letterKey("b", "\""),
                letterKey("n", "'"),
                letterKey("m", "="),
            ),
        )

        private fun defaultNumberRows(): List<List<KeySpec>> = listOf(
            listOf(
                KeySpec(
                    label = "+",
                    value = "+",
                    rowSpan = 3,
                    stack = listOf("+", "*", "-", "/").map { KeyStackItem(label = it, value = it) },
                ),
                KeySpec(label = "1", value = "1"),
                KeySpec(label = "2", value = "2"),
                KeySpec(label = "3", value = "3"),
                KeySpec(label = "⌫", value = "", action = KeyCommand(KeyCommandTypes.BACKSPACE)),
            ),
            listOf(
                KeySpec(label = "4", value = "4"),
                KeySpec(label = "5", value = "5"),
                KeySpec(label = "6", value = "6"),
                KeySpec(label = "·", value = "."),
            ),
            listOf(
                KeySpec(label = "7", value = "7"),
                KeySpec(label = "8", value = "8"),
                KeySpec(label = "9", value = "9"),
                KeySpec(label = "=", value = "="),
            ),
            listOf(
                KeySpec(
                    label = "返回",
                    value = "",
                    action = KeyCommand(KeyCommandTypes.KEYBOARD_MODE, "letters"),
                ),
                KeySpec(
                    label = "#+=",
                    value = "",
                    action = KeyCommand(KeyCommandTypes.KEYBOARD_MODE, "symbols"),
                ),
                KeySpec(label = "0", value = "0"),
                KeySpec(
                    label = "␣",
                    value = " ",
                    action = KeyCommand(KeyCommandTypes.SPACE),
                ),
                KeySpec(
                    label = "发送",
                    value = "\n",
                    action = KeyCommand(KeyCommandTypes.ENTER),
                ),
            ),
        )

        private fun defaultSymbolRows(): List<List<KeySpec>> = listOf(
            listOf(
                symbolKey("【", "【", "[", "["),
                symbolKey("】", "】", "]", "]"),
                symbolKey("《", "《", "<", "<"),
                symbolKey("》", "》", ">", ">"),
                symbolKey("「", "「", "{", "{"),
                symbolKey("」", "」", "}", "}"),
                symbolKey("、", "、", "\\", "\\"),
                symbolKey("：", "：", ":", ":"),
                symbolKey("；", "；", ";", ";"),
                symbolKey("？", "？", "?", "?"),
            ),
            listOf(
                symbolKey("！", "！", "!", "!"),
                symbolKey("（", "（", "(", "("),
                symbolKey("）", "）", ")", ")"),
                symbolKey("￥", "￥", "$", "$"),
                symbolKey("……", "……", "^", "^"),
                symbolKey("—", "—", "_", "_"),
                symbolKey("·", "·", "`", "`"),
                symbolKey("～", "～", "~", "~"),
                symbolKey("“", "“", "\"", "\""),
                symbolKey("”", "”", "'", "'"),
            ),
            listOf(
                KeySpec(
                    label = "123",
                    value = "",
                    weight = 1.35f,
                    action = KeyCommand(KeyCommandTypes.KEYBOARD_MODE, "numbers"),
                ),
                symbolKey("，", "，", ",", ","),
                symbolKey("。", "。", ".", "."),
                symbolKey("、", "、", "/", "/"),
                symbolKey("…", "…", "...", "..."),
                KeySpec(
                    label = "⌫",
                    value = "",
                    weight = 1.35f,
                    action = KeyCommand(KeyCommandTypes.BACKSPACE),
                ),
            ),
            listOf(
                KeySpec(
                    label = "ABC",
                    value = "",
                    weight = 1.35f,
                    action = KeyCommand(KeyCommandTypes.KEYBOARD_MODE, "letters"),
                ),
                KeySpec(
                    label = "空格",
                    value = " ",
                    weight = 4.4f,
                    action = KeyCommand(KeyCommandTypes.SPACE),
                ),
                KeySpec(
                    label = "↵",
                    value = "\n",
                    weight = 1.35f,
                    action = KeyCommand(KeyCommandTypes.ENTER),
                ),
            ),
        )

        private fun symbolKey(
            label: String,
            value: String,
            asciiLabel: String,
            asciiValue: String,
            weight: Float = 1f,
        ) = KeySpec(
            label = label,
            value = value,
            asciiLabel = asciiLabel,
            asciiValue = asciiValue,
            weight = weight,
        )

        private fun letterKey(
            label: String,
            hint: String,
            longPress: KeyCommand = KeyCommand.input(hint),
        ) = KeySpec(
            label = label,
            value = label,
            hint = hint,
            longPress = longPress,
        )

        private val builtInLayers = setOf("letters", "numbers", "symbols")
    }
}
