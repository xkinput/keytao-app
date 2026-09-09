package ink.rea.keytao_app

/** Entries are ordered most recent first; return removed entries for backing-file cleanup. */
internal fun <T> evictClipboardMedia(
    items: MutableList<T>,
    countLimit: Int,
    byteLimit: Long,
    size: (T) -> Long,
): List<T> {
    val removed = mutableListOf<T>()
    var bytes = items.sumOf(size)
    while (items.isNotEmpty() && (items.size > countLimit || bytes > byteLimit)) {
        val item = items.removeAt(items.lastIndex)
        bytes -= size(item)
        removed += item
    }
    return removed
}

/** Mirrors ClipDescription.compareMimeTypes without Android runtime calls in JVM tests. */
internal fun mimeAccepted(itemMime: String, editorMimes: Array<String>): Boolean = editorMimes.any { accepted ->
    accepted == "*/*" || accepted == itemMime ||
        (accepted.endsWith("/*") && itemMime.startsWith(accepted.dropLast(1)))
}
