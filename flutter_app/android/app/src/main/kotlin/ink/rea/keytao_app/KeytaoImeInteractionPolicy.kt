package ink.rea.keytao_app

import org.json.JSONArray
import org.json.JSONObject
import java.text.BreakIterator
import java.util.Locale

internal enum class DeleteSpeed {
    SLOW,
    STANDARD,
    FAST;

    companion object {
        fun fromSetting(value: String?): DeleteSpeed = when (value?.trim()?.lowercase()) {
            "slow" -> SLOW
            "fast" -> FAST
            else -> STANDARD
        }
    }
}

internal data class BackspaceRepeatProfile(
    val initialDelayMs: Long,
    val intervalMs: Long,
    val segmentThresholdMs: Long,
)

internal enum class BackspaceDeletionGranularity { CHARACTER, SEGMENT }

internal enum class BackspaceGestureMode {
    IMMEDIATE,
    SELECT_THEN_DELETE;

    companion object {
        fun fromSetting(value: String?): BackspaceGestureMode = when (value?.trim()?.lowercase()) {
            "selectthendelete" -> SELECT_THEN_DELETE
            else -> IMMEDIATE
        }
    }
}

internal data class BackspaceGestureCommand(
    val action: String,
    val count: Int,
)

internal object BackspaceGesturePolicy {
    fun dragCommand(
        mode: BackspaceGestureMode,
        currentUnits: Int,
        requestedUnits: Int,
        maximumUnits: Int,
    ): BackspaceGestureCommand? {
        return when (mode) {
            BackspaceGestureMode.IMMEDIATE -> {
                val target = requestedUnits.coerceIn(-maximumUnits, maximumUnits)
                val delta = target - currentUnits
                if (delta == 0) null else BackspaceGestureCommand(
                    action = if (delta > 0) "delete" else "restore",
                    count = kotlin.math.abs(delta),
                )
            }
            BackspaceGestureMode.SELECT_THEN_DELETE -> {
                val target = requestedUnits.coerceIn(0, maximumUnits)
                if (target == currentUnits) null else BackspaceGestureCommand(
                    action = if (target == 0) "cancelSelection" else "select",
                    count = target,
                )
            }
        }
    }

    fun releaseCommand(mode: BackspaceGestureMode, selectedUnits: Int): BackspaceGestureCommand? {
        if (mode != BackspaceGestureMode.SELECT_THEN_DELETE) return null
        return BackspaceGestureCommand(
            action = if (selectedUnits > 0) "commitSelection" else "cancelSelection",
            count = selectedUnits.coerceAtLeast(0),
        )
    }
}

internal object KeytaoImeInteractionTuning {
    const val LONG_PRESS_DELAY_MIN_MS = 100L
    const val LONG_PRESS_DELAY_DEFAULT_MS = 300L
    const val LONG_PRESS_DELAY_MAX_MS = 700L
    const val SLIDE_RETARGET_HYSTERESIS_DP = 8f
    const val BOUNCE_INTERVAL_MS = 40L
    const val BOUNCE_DISTANCE_DP = 12.6f
    const val KEYBOARD_DISMISS_VELOCITY_DP_PER_SECOND = 600f
    const val BACKSPACE_HOLD_TOLERANCE_DP = 8f
    const val CURSOR_GESTURE_ACTIVATION_DP = 12.6f
    const val CURSOR_GESTURE_STEP_DP = 10f
    const val DOUBLE_SPACE_PERIOD_TIMEOUT_MS = 1_100L
    const val REPEATABLE_EDIT_INTERVAL_MS = 72L
    const val ACCENT_BORDER_WIDTH_DP = 1f
    const val SOFT_ACCENT_KEY_FILL_AMOUNT = 0.22f
    const val SOFT_ACCENT_KEY_BORDER_ALPHA = 0.60f
    const val ACCENT_TOOLBAR_BORDER_ALPHA = 0.60f
    const val CANDIDATE_BORDER_ALPHA = 0.35f
    const val DARK_ACCENT_BORDER_ALPHA_MULTIPLIER = 0.70f
    const val BACKSPACE_PREVIEW_MINIMUM_HORIZONTAL_INSET_DP = 4f
    const val BACKSPACE_PREVIEW_VERTICAL_INSET_DP = 6f
    const val BACKSPACE_PREVIEW_TEXT_HORIZONTAL_PADDING_DP = 8f
    const val BACKSPACE_PREVIEW_MAX_TAIL_GRAPHEMES = 18
    const val CLIPBOARD_SUGGESTION_WINDOW_MS = 5 * 60 * 1_000L
    const val COLOR_PREVIEW_THROTTLE_MS = 50L

    private val slowBackspace = BackspaceRepeatProfile(
        initialDelayMs = 500L,
        intervalMs = 70L,
        segmentThresholdMs = 1_800L,
    )
    private val standardBackspace = BackspaceRepeatProfile(
        initialDelayMs = 400L,
        intervalMs = 50L,
        segmentThresholdMs = 1_500L,
    )
    private val fastBackspace = BackspaceRepeatProfile(
        initialDelayMs = 300L,
        intervalMs = 35L,
        segmentThresholdMs = 1_200L,
    )

    fun backspaceProfile(speed: DeleteSpeed): BackspaceRepeatProfile = when (speed) {
        DeleteSpeed.SLOW -> slowBackspace
        DeleteSpeed.STANDARD -> standardBackspace
        DeleteSpeed.FAST -> fastBackspace
    }

    fun isBounceDown(sinceLastUpMs: Long, distanceFromLastUpDp: Float): Boolean {
        return sinceLastUpMs >= 0L &&
            sinceLastUpMs < BOUNCE_INTERVAL_MS &&
            distanceFromLastUpDp < BOUNCE_DISTANCE_DP
    }
}

internal data class ClipboardSuggestionOffer(
    val text: String,
    val timestamp: Long,
)

internal fun shouldOfferClipboardSuggestion(
    text: String,
    timestamp: Long,
    now: Long,
    lastOffered: ClipboardSuggestionOffer?,
    windowMs: Long,
): Boolean {
    if (text.isBlank()) return false
    if (lastOffered?.text == text && (timestamp <= 0L || lastOffered.timestamp == timestamp)) {
        return false
    }
    if (timestamp <= 0L) return lastOffered?.text != text
    return now - timestamp in 0L..windowMs
}

/**
 * Atomically replaces the geometry consumed by touch hit-testing. Views rebuild
 * this store during layout/state changes, so a touch never observes a partially
 * rebuilt or previous-frame list.
 */
internal class ImmediateHitLayout<Element> {
    private var snapshot: List<Element> = emptyList()

    val items: List<Element>
        get() = snapshot

    fun rebuild(next: List<Element>) {
        snapshot = next
    }

    fun firstOrNull(predicate: (Element) -> Boolean): Element? = snapshot.firstOrNull(predicate)

    fun firstIndexOrNull(predicate: (Element) -> Boolean): Int? {
        return snapshot.indexOfFirst(predicate).takeIf { it >= 0 }
    }
}

internal class PerPointerBounceTracker<PointerId> {
    private data class PreviousUp(
        val eventTimeMs: Long,
        val xDp: Float,
        val yDp: Float,
    )

    private val previousUps = mutableMapOf<PointerId, PreviousUp>()
    private val bouncedPointers = mutableSetOf<PointerId>()

    fun isBounceDown(pointerId: PointerId, eventTimeMs: Long, xDp: Float, yDp: Float): Boolean {
        val previousUp = previousUps[pointerId]
        val isBounce = previousUp != null && KeytaoImeInteractionTuning.isBounceDown(
            sinceLastUpMs = eventTimeMs - previousUp.eventTimeMs,
            distanceFromLastUpDp = kotlin.math.hypot(xDp - previousUp.xDp, yDp - previousUp.yDp),
        )
        if (isBounce) {
            bouncedPointers.add(pointerId)
        } else {
            bouncedPointers.remove(pointerId)
        }
        return isBounce
    }

    fun recordUp(pointerId: PointerId, eventTimeMs: Long, xDp: Float, yDp: Float): Boolean {
        previousUps[pointerId] = PreviousUp(eventTimeMs, xDp, yDp)
        return bouncedPointers.remove(pointerId)
    }

    fun cancel(pointerId: PointerId) {
        bouncedPointers.remove(pointerId)
    }

    fun reset() {
        previousUps.clear()
        bouncedPointers.clear()
    }
}

internal class DoubleSpacePeriodTracker(
    private val timeoutMs: Long = KeytaoImeInteractionTuning.DOUBLE_SPACE_PERIOD_TIMEOUT_MS,
) {
    private var lastEligibleSpaceTimeMs: Long? = null

    fun shouldReplaceSpace(
        nowMs: Long,
        contextBefore: String,
        enabled: Boolean,
        hasComposition: Boolean,
    ): Boolean {
        if (!enabled || hasComposition) {
            reset()
            return false
        }
        val previousTime = lastEligibleSpaceTimeMs
        val canReplace = previousTime != null &&
            nowMs - previousTime in 0..timeoutMs &&
            contextBefore.endsWith(" ") &&
            hasDoubleSpaceEligibleSuffix(contextBefore.dropLast(1))
        if (canReplace) {
            reset()
            return true
        }
        lastEligibleSpaceTimeMs = nowMs.takeIf { hasDoubleSpaceEligibleSuffix(contextBefore) }
        return false
    }

    fun reset() {
        lastEligibleSpaceTimeMs = null
    }
}

private fun hasDoubleSpaceEligibleSuffix(text: String): Boolean {
    if (text.isEmpty()) return false
    val codePoint = text.codePointBefore(text.length)
    if (Character.isWhitespace(codePoint) || Character.isSpaceChar(codePoint)) return false
    return Character.getType(codePoint) !in punctuationTypes
}

internal data class CursorGestureUpdate(
    val active: Boolean,
    val stepDelta: Int,
)

internal class AlternateSelectionTracker(
    private val startX: Float,
    private val movementThreshold: Float,
) {
    private var hasMoved = false

    fun selectedIndex(
        x: Float,
        insideSelection: Boolean,
        panelLeft: Float,
        itemWidth: Float,
        itemCount: Int,
    ): Int? {
        if (!insideSelection || itemWidth <= 0f || itemCount <= 0) return null
        if (!hasMoved && kotlin.math.abs(x - startX) <= movementThreshold) return 0
        hasMoved = true
        return ((x - panelLeft) / itemWidth).toInt().coerceIn(0, itemCount - 1)
    }
}

internal class CursorGestureTracker(
    private val startX: Float,
    private val activationDistance: Float = KeytaoImeInteractionTuning.CURSOR_GESTURE_ACTIVATION_DP,
    private val stepDistance: Float = KeytaoImeInteractionTuning.CURSOR_GESTURE_STEP_DP,
) {
    var active: Boolean = false
        private set

    private var dispatchedSteps = 0

    fun update(x: Float): CursorGestureUpdate {
        val displacement = x - startX
        if (!active && kotlin.math.abs(displacement) + FLOAT_COMPARISON_EPSILON < activationDistance) {
            return CursorGestureUpdate(active = false, stepDelta = 0)
        }
        active = true
        val targetSteps = (displacement / stepDistance).toInt()
        val delta = targetSteps - dispatchedSteps
        dispatchedSteps = targetSteps
        return CursorGestureUpdate(active = true, stepDelta = delta)
    }

    private companion object {
        const val FLOAT_COMPARISON_EPSILON = 0.0001f
    }
}

internal class BackspaceRepeatPolicy(
    private val profile: BackspaceRepeatProfile,
) {
    fun repeatCountAt(holdDurationMs: Long): Int {
        if (holdDurationMs < profile.initialDelayMs) return 0
        return 1 + ((holdDurationMs - profile.initialDelayMs) / profile.intervalMs).toInt()
    }

    fun granularityAt(holdDurationMs: Long): BackspaceDeletionGranularity {
        return if (holdDurationMs >= profile.segmentThresholdMs) {
            BackspaceDeletionGranularity.SEGMENT
        } else {
            BackspaceDeletionGranularity.CHARACTER
        }
    }
}

private enum class DeletionSegmentClass { WHITESPACE, CJK, LATIN, PUNCTUATION, OTHER }

internal fun trailingDeletionSegmentLength(text: String): Int {
    return trailingDeletionSegmentsLength(text, 1).coerceAtLeast(1)
}

internal fun trailingDeletionSegmentsLength(text: String, segmentCount: Int): Int {
    val units = graphemeUnits(text)
    if (units.isEmpty()) return 0
    val limit = segmentCount.coerceAtLeast(1)
    var selectedUnits = 0
    var selectedSegments = 0
    var previousClass: DeletionSegmentClass? = null
    for (unit in units.asReversed()) {
        val currentClass = deletionSegmentClass(unit)
        if (currentClass != previousClass) {
            if (selectedSegments == limit) break
            selectedSegments += 1
            previousClass = currentClass
        }
        selectedUnits += 1
    }
    return selectedUnits
}

internal data class LanguageModeDecision(
    val usesEnglishSchema: Boolean,
    val targetEnglish: Boolean,
)

internal fun resolveEnglishSchemaId(schemas: List<Pair<String, String>>): String? {
    return schemas.firstOrNull { it.first == "easy_en" }?.first
        ?: schemas.firstOrNull { it.first == "english" }?.first
        ?: schemas.firstOrNull {
            it.second.equals("Easy English", ignoreCase = true) ||
                it.second.equals("English", ignoreCase = true)
        }?.first
}

internal fun decideLanguageMode(
    englishMode: String,
    englishSchemaId: String?,
    value: String?,
    currentSchemaId: String?,
    asciiMode: Boolean,
): LanguageModeDecision {
    val usesEnglishSchema = englishMode.trim().equals("schema", ignoreCase = true) && englishSchemaId != null
    val targetEnglish = when (value?.trim()?.lowercase(Locale.ROOT)) {
        "ascii", "english", "en" -> true
        "chinese", "zh", "cn" -> false
        else -> if (usesEnglishSchema) currentSchemaId != englishSchemaId else !asciiMode
    }
    return LanguageModeDecision(usesEnglishSchema, targetEnglish)
}

internal fun snapshotChineseSwitchOptions(
    switchNames: List<String>,
    optionValue: (String) -> Boolean,
): Map<String, Boolean> = switchNames
    .asSequence()
    .filter { it != "ascii_mode" }
    .distinct()
    .associateWith(optionValue)

private fun graphemeUnits(text: String): List<String> {
    if (text.isEmpty()) return emptyList()
    val iterator = BreakIterator.getCharacterInstance(Locale.ROOT)
    iterator.setText(text)
    return buildList {
        var start = iterator.first()
        var end = iterator.next()
        while (end != BreakIterator.DONE) {
            add(text.substring(start, end))
            start = end
            end = iterator.next()
        }
    }
}

private fun deletionSegmentClass(unit: String): DeletionSegmentClass {
    if (unit.all(Char::isWhitespace)) return DeletionSegmentClass.WHITESPACE
    val codePoint = unit.codePointAt(0)
    return when {
        Character.UnicodeScript.of(codePoint) == Character.UnicodeScript.HAN -> DeletionSegmentClass.CJK
        Character.UnicodeScript.of(codePoint) == Character.UnicodeScript.LATIN || Character.isDigit(codePoint) -> {
            DeletionSegmentClass.LATIN
        }
        Character.getType(codePoint) in punctuationTypes -> DeletionSegmentClass.PUNCTUATION
        else -> DeletionSegmentClass.OTHER
    }
}

private val punctuationTypes = setOf(
    Character.CONNECTOR_PUNCTUATION.toInt(),
    Character.DASH_PUNCTUATION.toInt(),
    Character.START_PUNCTUATION.toInt(),
    Character.END_PUNCTUATION.toInt(),
    Character.INITIAL_QUOTE_PUNCTUATION.toInt(),
    Character.FINAL_QUOTE_PUNCTUATION.toInt(),
    Character.OTHER_PUNCTUATION.toInt(),
)

internal data class KeytaoHsv(val hue: Float, val saturation: Float, val value: Float)

/** The only colour maths the in-keyboard 主题色 picker uses: HSV ⇄ sRGB plus `#RRGGBB` text. */
internal object KeytaoColorMath {
    fun normalizeHue(hue: Float): Float {
        if (!hue.isFinite()) return 0f
        val wrapped = hue % 360f
        return if (wrapped < 0f) wrapped + 360f else wrapped
    }

    fun hsvToRgb(hue: Float, saturation: Float, value: Float): Triple<Int, Int, Int> {
        val h = normalizeHue(hue)
        val s = saturation.coerceIn(0f, 1f)
        val v = value.coerceIn(0f, 1f)
        val chroma = v * s
        val sector = h / 60f
        val x = chroma * (1f - kotlin.math.abs(sector % 2f - 1f))
        val (r, g, b) = when (sector.toInt()) {
            0 -> Triple(chroma, x, 0f)
            1 -> Triple(x, chroma, 0f)
            2 -> Triple(0f, chroma, x)
            3 -> Triple(0f, x, chroma)
            4 -> Triple(x, 0f, chroma)
            else -> Triple(chroma, 0f, x)
        }
        val m = v - chroma
        return Triple(channel(r + m), channel(g + m), channel(b + m))
    }

    fun rgbToHsv(red: Int, green: Int, blue: Int): KeytaoHsv {
        val r = red.coerceIn(0, 255) / 255f
        val g = green.coerceIn(0, 255) / 255f
        val b = blue.coerceIn(0, 255) / 255f
        val maxChannel = maxOf(r, g, b)
        val minChannel = minOf(r, g, b)
        val delta = maxChannel - minChannel
        val hue = when {
            delta == 0f -> 0f
            maxChannel == r -> 60f * (((g - b) / delta) % 6f)
            maxChannel == g -> 60f * ((b - r) / delta + 2f)
            else -> 60f * ((r - g) / delta + 4f)
        }
        val saturation = if (maxChannel == 0f) 0f else delta / maxChannel
        return KeytaoHsv(normalizeHue(hue), saturation, maxChannel)
    }

    fun hsvToHex(hue: Float, saturation: Float, value: Float): String {
        val (r, g, b) = hsvToRgb(hue, saturation, value)
        return "#%02X%02X%02X".format(Locale.ROOT, r, g, b)
    }

    fun hexToHsv(value: String): KeytaoHsv? {
        val hex = value.trim().removePrefix("#")
        if (hex.length != 6 || hex.any { it.digitToIntOrNull(16) == null }) return null
        return rgbToHsv(
            hex.substring(0, 2).toInt(16),
            hex.substring(2, 4).toInt(16),
            hex.substring(4, 6).toInt(16),
        )
    }

    private fun channel(value: Float): Int = kotlin.math.round(value * 255f).toInt().coerceIn(0, 255)
}

internal enum class KeyboardShiftState { OFF, ONCE, LOCKED }

internal enum class KeyboardPanel { RIME, CLIPBOARD, SETTINGS, PHRASES, EXPANDED_CANDIDATES }

internal class KeyboardNavigationState {
    var layer: String = "letters"
        private set
    var panel: KeyboardPanel? = null
        private set
    var returnLayer: String = "letters"
        private set
    var lastSymbolCategory: String = "symbols"
        private set
    var lastEmojiCategory: String = "symbols_emoji_face"
        private set
    var shift: KeyboardShiftState = KeyboardShiftState.OFF

    fun resolveLayer(value: String?): String = when (value) {
        "back" -> returnLayer
        "symbols:last" -> lastSymbolCategory
        "emoji:last" -> lastEmojiCategory
        else -> value?.takeIf(String::isNotBlank) ?: "letters"
    }

    fun shouldCommitBeforeNavigation(layer: String, panel: KeyboardPanel?, reset: Boolean = false): Boolean {
        return !reset && (
            (layer != this.layer && layer.isDirectInputPage()) ||
                (panel != this.panel && panel in setOf(KeyboardPanel.CLIPBOARD, KeyboardPanel.PHRASES))
            )
    }

    fun navigate(layer: String = this.layer, panel: KeyboardPanel? = null, reset: Boolean = false) {
        if (reset) {
            this.layer = layer
            this.panel = null
            returnLayer = "letters"
            shift = KeyboardShiftState.OFF
            return
        }
        if (layer != this.layer) {
            returnLayer = when {
                layer == "letters" || layer == returnLayer -> "letters"
                this.layer.isDirectInputPage() && layer.isDirectInputPage() -> returnLayer
                else -> this.layer
            }
            if (shift == KeyboardShiftState.ONCE) shift = KeyboardShiftState.OFF
            this.layer = layer
        }
        this.panel = panel
        when {
            layer.startsWith("symbols_emoji_") -> lastEmojiCategory = layer
            layer == "symbols" || layer.startsWith("symbols_") -> lastSymbolCategory = layer
        }
    }

    fun layerAfterDirectCommit(symbolAutoReturn: Boolean): String? {
        return "letters".takeIf {
            symbolAutoReturn && panel == null &&
                (layer == "symbols" || layer.startsWith("symbols_")) &&
                !layer.startsWith("symbols_emoji_")
        }
    }

    fun restoreFrom(previous: KeyboardNavigationState) {
        layer = previous.layer
        panel = previous.panel
        returnLayer = previous.returnLayer
        lastSymbolCategory = previous.lastSymbolCategory
        lastEmojiCategory = previous.lastEmojiCategory
        shift = previous.shift
    }

    private fun String.isDirectInputPage(): Boolean {
        return this == "numbers" || this == "symbols" || startsWith("symbols_")
    }
}

internal data class StoredClipboardItem(
    val key: String,
    val timestamp: Long,
    val pinned: Boolean = false,
    val text: String? = null,
    val fileName: String? = null,
    val mime: String? = null,
    val name: String? = null,
    val size: Long = 0,
    val width: Int = 0,
    val height: Int = 0,
) {
    val isMedia: Boolean get() = fileName != null

    fun isValid(): Boolean {
        if (key.isBlank()) return false
        return if (text != null) {
            text.isNotBlank() && fileName == null
        } else {
            fileName != null && fileName.isNotBlank() && fileName !in setOf(".", "..") &&
                '/' !in fileName && '\\' !in fileName && '\u0000' !in fileName
        }
    }
}

internal class ClipboardHistoryState {
    var items: List<StoredClipboardItem> = emptyList()
        private set

    fun remember(item: StoredClipboardItem) {
        if (!item.isValid()) return
        val previous = items.firstOrNull { it.key == item.key && it.isMedia == item.isMedia }
        replace(listOf(item.copy(pinned = previous?.pinned ?: item.pinned)) + items.filterNot {
            it.key == item.key && it.isMedia == item.isMedia
        })
    }

    fun isPinned(key: String, media: Boolean = false): Boolean {
        return items.any { it.key == key && it.isMedia == media && it.pinned }
    }

    fun togglePin(key: String, media: Boolean = false, maxBytes: Int = Int.MAX_VALUE): Boolean {
        val item = items.firstOrNull { it.key == key && it.isMedia == media } ?: return true
        if (!item.pinned) {
            val pinned = items.filter { it.pinned } + item.copy(pinned = true)
            val bytes = 2L + pinned.sumOf { encodeItem(it).toByteArray(Charsets.UTF_8).size.toLong() } + (pinned.size - 1)
            if (bytes > maxBytes) return false
        }
        replace(items.map { if (it.key == key && it.isMedia == media) it.copy(pinned = !it.pinned) else it })
        return true
    }

    fun remove(key: String, media: Boolean = false) {
        items = items.filterNot { it.key == key && it.isMedia == media }
    }

    fun clearUnpinned() {
        items = items.filter { it.pinned }
    }

    fun encode(maxBytes: Int = Int.MAX_VALUE): String {
        require(maxBytes >= 2)
        val encoded = StringBuilder("[")
        var bytes = 2
        for (item in items) {
            val value = encodeItem(item)
            val extra = value.toByteArray(Charsets.UTF_8).size + if (encoded.length > 1) 1 else 0
            if (extra > maxBytes - bytes) continue
            if (encoded.length > 1) encoded.append(',')
            encoded.append(value)
            bytes += extra
        }
        return encoded.append(']').toString()
    }

    private fun encodeItem(item: StoredClipboardItem): String = JSONObject().apply {
        put("key", item.key)
        put("timestamp", item.timestamp)
        put("pinned", item.pinned)
        if (item.text != item.key) put("text", item.text)
        put("fileName", item.fileName)
        put("mime", item.mime)
        put("name", item.name)
        put("size", item.size)
        put("width", item.width)
        put("height", item.height)
    }.toString()

    fun restore(encoded: String) {
        val restored = runCatching {
            val array = JSONArray(encoded)
            (0 until array.length()).mapNotNull { index ->
                val item = array.optJSONObject(index) ?: return@mapNotNull null
                val key = item.opt("key") as? String ?: return@mapNotNull null
                val fileName = item.opt("fileName") as? String
                StoredClipboardItem(
                    key = key,
                    timestamp = item.optLong("timestamp", 0L).coerceAtLeast(0L),
                    pinned = item.optBoolean("pinned", false),
                    text = item.opt("text") as? String ?: key.takeIf { fileName == null },
                    fileName = fileName,
                    mime = item.opt("mime") as? String,
                    name = item.opt("name") as? String,
                    size = item.optLong("size", 0L).coerceAtLeast(0L),
                    width = item.optInt("width", 0).coerceAtLeast(0),
                    height = item.optInt("height", 0).coerceAtLeast(0),
                ).takeIf { it.isValid() }
            }
        }.getOrDefault(emptyList())
        replace(restored)
    }

    private fun replace(next: List<StoredClipboardItem>) {
        val sorted = next.distinctBy { it.isMedia to it.key }.sortedByDescending { it.timestamp }
        items = sorted.filter { it.pinned } + sorted.filterNot { it.pinned }.take(50)
    }
}

internal fun rankEmojiSearch(
    index: Map<String, List<String>>,
    query: String,
    recents: List<String>,
    limit: Int = 24,
): List<String> {
    val count = limit.coerceAtLeast(0)
    val needle = query.trim().lowercase(Locale.ROOT)
    if (needle.isEmpty()) return recents.distinct().take(count)
    val recentOrder = recents.distinct().withIndex().associate { it.value to it.index }
    return index.mapNotNull { (emoji, keywords) ->
        val tier = keywords.minOfOrNull { keyword ->
            val normalized = keyword.lowercase(Locale.ROOT)
            when {
                normalized == needle -> 0
                normalized.startsWith(needle) -> 1
                needle in normalized -> 2
                else -> 3
            }
        } ?: 3
        if (tier < 3) emoji to tier else null
    }.sortedWith(compareBy<Pair<String, Int>> { it.second }.thenBy { recentOrder[it.first] ?: Int.MAX_VALUE })
        .take(count).map { it.first }
}

internal fun selectionRangeAroundCursor(
    before: String,
    after: String,
    line: Boolean = false,
): Pair<Int, Int>? {
    if (line) {
        val left = before.length - before.lastIndexOf('\n') - 1
        val right = after.indexOf('\n').takeIf { it >= 0 } ?: after.length
        return (-left to right).takeIf { left + right > 0 }
    }
    val rightClass = after.takeIf { it.isNotEmpty() }?.codePointAt(0)?.let(::selectionWordClass) ?: 0
    val leftClass = before.takeIf { it.isNotEmpty() }?.codePointBefore(before.length)?.let(::selectionWordClass) ?: 0
    val wordClass = if (rightClass != 0) rightClass else leftClass
    if (wordClass == 0) return null
    val cap = if (wordClass == 1) 32 else Int.MAX_VALUE
    var leftUnits = 0
    var leftCount = 0
    while (leftUnits < before.length && leftCount < cap) {
        val codePoint = before.codePointBefore(before.length - leftUnits)
        if (selectionWordClass(codePoint) != wordClass) break
        leftUnits += Character.charCount(codePoint)
        leftCount++
    }
    var rightUnits = 0
    var rightCount = 0
    while (rightUnits < after.length && rightCount < cap) {
        val codePoint = after.codePointAt(rightUnits)
        if (selectionWordClass(codePoint) != wordClass) break
        rightUnits += Character.charCount(codePoint)
        rightCount++
    }
    if (wordClass == 1 && leftCount + rightCount > cap) {
        rightCount = minOf(rightCount, maxOf(cap / 2, cap - leftCount))
        leftCount = minOf(leftCount, cap - rightCount)
        leftUnits = before.length - before.offsetByCodePoints(before.length, -leftCount)
        rightUnits = after.offsetByCodePoints(0, rightCount)
    }
    return -leftUnits to rightUnits
}

private fun selectionWordClass(codePoint: Int): Int = when (Character.UnicodeScript.of(codePoint)) {
    Character.UnicodeScript.HAN,
    Character.UnicodeScript.HIRAGANA,
    Character.UnicodeScript.KATAKANA,
    Character.UnicodeScript.HANGUL,
    Character.UnicodeScript.BOPOMOFO -> 1
    Character.UnicodeScript.LATIN -> if (Character.isLetter(codePoint)) 2 else 0
    else -> if (Character.isDigit(codePoint) || codePoint == '_'.code) 2 else 0
}
