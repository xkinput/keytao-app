package ink.rea.keytao_app

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject

object KeyCommandTypes {
    const val INPUT = "input"
    const val DIRECT_INPUT = "directInput"
    const val RIME_INPUT = "rimeInput"
    const val BACKSPACE = "backspace"
    const val ENTER = "enter"
    const val SPACE = "space"
    const val SHIFT = "shift"
    const val MODE = "mode"
    const val OPEN_PAGE = "openPage"
    const val KEYBOARD_PICKER = "keyboardPicker"
    const val KEYBOARD_MODE = "keyboardMode"
    const val NEXT_PAGE = "nextCandidatePage"
    const val PREVIOUS_PAGE = "previousCandidatePage"
    const val RESET = "reset"
    const val RIME_MENU = "rimeMenu"
    const val PANEL = "panel"
    const val EDIT = "edit"
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
)

data class KeytaoAndroidImeConfig(
    val keyboardHeightDp: Int,
    val candidateBarHeightDp: Int,
    val keyboardBottomInsetDp: Int,
    val hapticsEnabled: Boolean,
    val hapticIntensity: Int,
    val swipeThresholdDp: Float,
    val rows: List<List<KeySpec>>,
    val numberRows: List<List<KeySpec>>,
    val symbolRows: List<List<KeySpec>>,
) {
    companion object {
        fun load(context: Context): KeytaoAndroidImeConfig {
            val userConfig = KeytaoAndroidPaths.imeConfigFile()
            val defaultJson = context.resources
                .openRawResource(R.raw.keytao_android_ime)
                .bufferedReader()
                .use { it.readText() }
            val userJson = userConfig.takeIf { it.isFile }?.readText()
            return runCatching {
                if (userJson == null) parse(defaultJson) else parse(userJson, defaultJson)
            }.getOrElse { parse(defaultJson) }
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
            val haptics = root.optJSONObject("haptics")
            val fallbackHaptics = fallbackRoot?.optJSONObject("haptics")
            return KeytaoAndroidImeConfig(
                keyboardHeightDp = mergedInt(root, fallbackRoot, "keyboardHeightDp", 246).coerceIn(160, 420),
                candidateBarHeightDp = mergedInt(root, fallbackRoot, "candidateBarHeightDp", 52).coerceIn(36, 96),
                keyboardBottomInsetDp = mergedInt(root, fallbackRoot, "keyboardBottomInsetDp", 48).coerceIn(0, 80),
                hapticsEnabled = mergedBoolean(root, fallbackRoot, haptics, fallbackHaptics, "enabled", "hapticsEnabled", true),
                hapticIntensity = mergedInt(root, fallbackRoot, haptics, fallbackHaptics, "intensity", "hapticIntensity", 42)
                    .coerceIn(1, 100),
                swipeThresholdDp = mergedDouble(root, fallbackRoot, "swipeThresholdDp", 34.0).toFloat().coerceIn(12f, 96f),
                rows = rows.ifEmpty { defaultRows() },
                numberRows = numberRows.ifEmpty { defaultNumberRows() },
                symbolRows = symbolRows.ifEmpty { defaultSymbolRows() },
            )
        }

        private fun rowArray(root: JSONObject, fallbackRoot: JSONObject?, name: String): JSONArray? {
            return root.optJSONArray(name) ?: fallbackRoot?.optJSONArray(name)
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
            )
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
            "1234567890".map { KeySpec(label = it.toString(), value = it.toString()) },
            listOf("-", "/", ":", ";", "(", ")", "\$", "&", "@", "\"").map { KeySpec(label = it, value = it) },
            listOf("#+=", ".", ",", "?", "!", "'", "⌫").map { label ->
                if (label == "⌫") {
                    KeySpec(label = label, value = "", action = KeyCommand(KeyCommandTypes.BACKSPACE))
                } else if (label == "#+=") {
                    KeySpec(
                        label = label,
                        value = "",
                        action = KeyCommand(KeyCommandTypes.KEYBOARD_MODE, "symbols"),
                    )
                } else {
                    KeySpec(label = label, value = label)
                }
            },
            listOf(
                KeySpec(
                    label = "ABC",
                    value = "",
                    weight = 1.4f,
                    action = KeyCommand(KeyCommandTypes.KEYBOARD_MODE, "letters"),
                ),
                KeySpec(
                    label = "空格",
                    value = " ",
                    weight = 4.2f,
                    action = KeyCommand(KeyCommandTypes.SPACE),
                ),
                KeySpec(
                    label = "回车",
                    value = "\n",
                    weight = 1.4f,
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
    }
}
