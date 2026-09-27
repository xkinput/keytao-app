package ink.rea.keytao_app

import android.os.SystemClock
import org.json.JSONObject
import java.util.ArrayDeque

/** Coarse events only. Startup metadata waits for an existing native init call. */
internal object KeytaoRuntimeLog {
    private val pending = ArrayDeque<() -> Unit>()
    @Volatile private var initialized = false
    @Volatile var collecting = true
        private set

    fun event(cat: String, ev: String, durMs: Double = Double.NaN, build: (JSONObject.() -> Unit)? = null) {
        val wasInitialized = initialized
        if (wasInitialized) {
            refreshEnabled()
            if (!collecting) return
        }
        val uptime = SystemClock.elapsedRealtime()
        val write = {
            KeytaoNativeBridge.rtLog(1, cat, ev, durMs) {
                put("uptime_ms", uptime)
                build?.invoke(this)
            }
        }
        synchronized(pending) {
            if (!initialized) {
                if (pending.size == 32) pending.removeFirst()
                pending.addLast(write)
                return
            }
        }
        if (!wasInitialized) refreshEnabled()
        if (collecting) write()
    }

    fun nativeInitialized() {
        val events = synchronized(pending) {
            initialized = true
            pending.toList().also { pending.clear() }
        }
        refreshEnabled()
        events.forEach { it() }
    }

    /** App lifecycle only, after Tauri has loaded the library; never called by IME startup. */
    fun adoptAppLoggerIfEnabled() {
        if (initialized) {
            refreshEnabled()
            return
        }
        if (!KeytaoNativeBridge.loaded ||
            !runCatching { KeytaoNativeBridge.nativeLogEnabled(1) }.getOrDefault(false)
        ) return
        nativeInitialized()
    }

    fun refreshEnabled() {
        // Recording startup metadata must not load the native library on the UI thread.
        if (!initialized) return
        if (!KeytaoNativeBridge.loaded) {
            collecting = false
            return
        }
        collecting = runCatching { KeytaoNativeBridge.nativeLogEnabled(1) }.getOrDefault(false)
    }

    fun flush(timeoutMs: Int) {
        runCatching {
            if (initialized && KeytaoNativeBridge.loaded) {
                KeytaoNativeBridge.nativeLogFlush(timeoutMs)
            }
        }
    }

    fun elapsedMs(startNs: Long): Double = (System.nanoTime() - startNs) / 1_000_000.0
}

/** Fixed storage: hot paths record locally and cross JNI only when drained. */
internal class KeytaoDurationHistogram {
    private val bounds = doubleArrayOf(0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0)
    private val buckets = LongArray(11)
    private var count = 0L
    private var maximum = 0.0
    private var jank = 0L

    fun start(): Long = if (KeytaoRuntimeLog.collecting) System.nanoTime() else 0L

    fun finish(startNs: Long) {
        if (startNs == 0L) return
        val ms = KeytaoRuntimeLog.elapsedMs(startNs)
        var bucket = 0
        while (bucket < bounds.size && ms > bounds[bucket]) bucket++
        synchronized(this) {
            buckets[bucket]++
            count++
            maximum = maxOf(maximum, ms)
            if (ms > 16.0) jank++
        }
    }

    fun drain(cat: String, ev: String, frames: Boolean = false) {
        val snapshot = synchronized(this) {
            if (count == 0L) return
            Snapshot(count, percentile(0.50), percentile(0.95), maximum, jank).also {
                buckets.fill(0)
                count = 0
                maximum = 0.0
                jank = 0
            }
        }
        // No JSON, JNI, engine monitor or filesystem work under the histogram lock.
        KeytaoRuntimeLog.event(cat, ev) {
            put("n", snapshot.count)
            if (frames) put("frames", snapshot.count)
            put("p50_ms", snapshot.p50)
            put("p95_ms", snapshot.p95)
            put("max_ms", snapshot.maximum)
            if (frames) put("jank_gt_16ms", snapshot.jank)
        }
    }

    private data class Snapshot(val count: Long, val p50: Double, val p95: Double, val maximum: Double, val jank: Long)

    private fun percentile(fraction: Double): Double {
        val target = kotlin.math.ceil(count * fraction).toLong()
        var seen = 0L
        for (index in buckets.indices) {
            seen += buckets[index]
            if (seen >= target) return minOf(bounds.getOrElse(index) { maximum }, maximum)
        }
        return maximum
    }
}

/** Only known command types become field names; arbitrary config strings never do. */
internal class KeytaoInputCounts {
    private val commandCounters = mapOf(
        KeyCommandTypes.INPUT to "input",
        KeyCommandTypes.DIRECT_INPUT to "input",
        KeyCommandTypes.RIME_INPUT to "input",
        KeyCommandTypes.BACKSPACE to "backspace",
        KeyCommandTypes.BACKSPACE_GESTURE to "backspace_gesture",
        KeyCommandTypes.ENTER to "enter",
        KeyCommandTypes.SPACE to "space",
        KeyCommandTypes.SHIFT to "shift",
        KeyCommandTypes.MODE to "mode",
        KeyCommandTypes.OPEN_PAGE to "open_page",
        KeyCommandTypes.KEYBOARD_PICKER to "keyboard_picker",
        KeyCommandTypes.NEXT_INPUT_METHOD to "next_input_method",
        KeyCommandTypes.KEYBOARD_MODE to "keyboard_mode",
        KeyCommandTypes.NEXT_PAGE to "next_candidate_page",
        KeyCommandTypes.PREVIOUS_PAGE to "previous_candidate_page",
        KeyCommandTypes.RESET to "reset",
        KeyCommandTypes.RIME_MENU to "rime_menu",
        KeyCommandTypes.RIME_SCHEMA to "rime_schema",
        KeyCommandTypes.RIME_OPTION to "rime_option",
        KeyCommandTypes.PANEL to "panel",
        KeyCommandTypes.EDIT to "edit",
        KeyCommandTypes.ONE_HANDED to "one_handed",
        KeyCommandTypes.FLOATING to "floating",
        KeyCommandTypes.SETTING to "setting",
    )
    private val types = (listOf("key_down", "key_up", "commit_direct", "clipboard_read", "clipboard_write") +
        commandCounters.values + "other").distinct()
    private val counts = LongArray(types.size)

    fun recordCommand(type: String) {
        record(commandCounters[type] ?: "other")
    }

    fun record(type: String) {
        if (!KeytaoRuntimeLog.collecting) return
        val index = types.indexOf(type).let { if (it < 0) types.lastIndex else it }
        counts[index]++
    }

    fun drain() {
        if (counts.all { it == 0L }) return
        val snapshot = counts.copyOf()
        counts.fill(0)
        KeytaoRuntimeLog.event("input", "input_counts") {
            types.indices.forEach { index ->
                if (snapshot[index] != 0L) put(types[index], snapshot[index])
            }
        }
    }
}
