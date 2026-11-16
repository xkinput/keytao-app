import Foundation

var failures: [String] = []
var checks = 0

func expect<T: Equatable>(_ actual: T, _ expected: T, _ message: String, line: Int = #line) {
    checks += 1
    if actual != expected {
        failures.append("line \(line): \(message) (expected \(expected), got \(actual))")
    }
}

let mathSymbols = KeyTaoKeyboardLayer("symbols_math")
let animalEmoji = KeyTaoKeyboardLayer("symbols_emoji_animal")
let editorPage = KeyTaoKeyboardLayer("editor")
var navigation = KeyTaoKeyboardNavigation()
expect(navigation.resolve(nil), .letters, "an unspecified field layer resolves to letters")
expect(navigation.resolve("symbols:last"), .symbols, "symbols initially open the default category")
expect(navigation.resolve("emoji:last"), KeyTaoKeyboardLayer("symbols_emoji_face"), "emoji initially open the face category")
navigation.navigate(to: .layer(.numbers))
expect(navigation.returnLayer, .letters, "numbers remember letters as their origin")
navigation.navigate(to: .layer(mathSymbols))
expect(navigation.returnLayer, .letters, "numbers and symbols share the original return target")
navigation.navigate(to: .layer(animalEmoji))
expect(navigation.returnLayer, .letters, "switching symbols to emoji preserves the original return target")
expect(navigation.resolve("symbols:last"), mathSymbols, "symbol category memory is independent of emoji")
expect(navigation.resolve("emoji:last"), animalEmoji, "emoji category memory follows the current category")
navigation.navigate(to: .back)
expect(navigation.layer, .letters, "back from the numbers-symbols-emoji group returns to letters")
navigation.navigate(to: .layer(navigation.resolve("symbols:last")))
expect(navigation.layer, mathSymbols, "re-entering symbols restores the previous category")
navigation.navigate(to: .layer(editorPage))
expect(navigation.returnLayer, mathSymbols, "editor remembers the page from which it was opened")
navigation.navigate(to: .back)
expect(navigation.layer, mathSymbols, "editor back returns to its symbol origin")
expect(navigation.returnLayer, .letters, "return memory has one level rather than a navigation stack")
navigation.navigate(to: .back)
expect(navigation.layer, .letters, "a second back does not restore a consumed editor origin")
navigation.navigate(to: .layer(editorPage))
navigation.navigate(to: .layer(.numbers))
expect(navigation.resolve("back"), .letters, "numbers back always resolves to letters even when opened from editor")
navigation.navigate(to: .back)
expect(navigation.layer, .letters, "numbers back returns to letters")

for source in [KeyTaoKeyboardLayer.numbers, mathSymbols, animalEmoji, editorPage] {
    for panel in [KeyTaoNavigationPanel.candidates, .function(.clipboard), .function(.rime), .function(.settings), .function(.phrases)] {
        var coveredPage = KeyTaoKeyboardNavigation()
        coveredPage.navigate(to: .layer(source))
        let origin = coveredPage.returnLayer
        coveredPage.navigate(to: .panel(panel))
        expect(coveredPage.layer, source, "opening \(panel) preserves \(source.id)")
        expect(coveredPage.panel, panel, "the requested overlay is active")
        coveredPage.navigate(to: .back)
        expect(coveredPage.panel, nil, "back closes the overlay before leaving its page")
        expect(coveredPage.layer, source, "back from \(panel) stays on \(source.id)")
        expect(coveredPage.returnLayer, origin, "closing an overlay preserves its page origin")
        coveredPage.navigate(to: .panel(panel))
        coveredPage.navigate(to: .panel(nil))
        expect(coveredPage.layer, source, "explicit overlay close stays on \(source.id)")
        expect(coveredPage.panel, nil, "explicit overlay close clears the panel")
    }
}

navigation.navigate(to: .layer(mathSymbols))
navigation.navigate(to: .layer(animalEmoji))
navigation.shift = .locked
navigation.navigate(to: .layer(.numbers))
expect(navigation.shift == .locked, true, "page switches retain caps lock")
navigation.navigate(to: .back)
expect(navigation.shift == .locked, true, "back retains caps lock")
navigation.shift = .once
navigation.navigate(to: .layer(mathSymbols))
expect(navigation.shift == .off, true, "page switches clear one-shot shift")
navigation.shift = .once
navigation.navigate(to: .panel(.function(.clipboard)))
expect(navigation.shift == .once, true, "opening an overlay is not a page switch")
navigation.navigate(to: .back)
expect(navigation.shift == .once, true, "closing an overlay preserves one-shot shift on its page")

for forcedLayer: KeyTaoKeyboardLayer? in [nil, .numbers] {
    navigation.shift = .locked
    navigation.navigate(to: .panel(.function(.phrases)))
    navigation.navigate(to: .reset(forcedLayer ?? navigation.resolve(nil)))
    expect(navigation.layer, forcedLayer ?? .letters, "new fields choose their forced layer or letters")
    expect(navigation.returnLayer, .letters, "new fields discard the previous origin")
    expect(navigation.panel, nil, "new fields close overlays")
    expect(navigation.shift == .off, true, "new fields clear caps lock")
    expect(navigation.resolve("symbols:last"), mathSymbols, "new fields retain the last symbol category")
    expect(navigation.resolve("emoji:last"), animalEmoji, "new fields retain the last emoji category")
}
navigation.shift = .once
navigation.navigate(to: .reset(.letters))
expect(navigation.shift == .off, true, "new fields also clear one-shot shift")

navigation.navigate(to: .layer(mathSymbols))
expect(navigation.shouldReturnAfterDirectCommit(symbolAutoReturn: false), false, "symbol commits stay on the page by default")
expect(navigation.layer, mathSymbols, "checking the default symbol commit policy leaves the category intact")
expect(navigation.shouldReturnAfterDirectCommit(symbolAutoReturn: true), true, "enabled symbol auto-return requests letters after a commit")
navigation.navigate(to: .panel(.function(.phrases)))
expect(navigation.shouldReturnAfterDirectCommit(symbolAutoReturn: true), false, "overlay commits do not trigger symbol auto-return")
navigation.navigate(to: .panel(nil))
navigation.navigate(to: .layer(animalEmoji))
expect(navigation.shouldReturnAfterDirectCommit(symbolAutoReturn: true), false, "emoji commits stay on emoji even with symbol auto-return enabled")
navigation.navigate(to: .layer(.numbers))
expect(navigation.shouldReturnAfterDirectCommit(symbolAutoReturn: true), false, "number commits do not trigger symbol auto-return")

for target in [KeyTaoKeyboardLayer.numbers, mathSymbols, animalEmoji, editorPage] {
    expect(KeyTaoNavigationAction.layer(target).requiresCompositionCommit, true, "entering \(target.id) first commits pending composition")
}
expect(KeyTaoNavigationAction.panel(.function(.clipboard)).requiresCompositionCommit, true, "clipboard entry first commits pending composition")
expect(KeyTaoNavigationAction.panel(.function(.phrases)).requiresCompositionCommit, true, "phrase entry first commits pending composition")
for action in [KeyTaoNavigationAction.layer(.letters), .back, .panel(nil), .panel(.candidates), .panel(.function(.rime)), .panel(.function(.settings)), .reset(.numbers)] {
    expect(action.requiresCompositionCommit, false, "non-insertion navigation leaves composition handling to its existing path")
}

let clipboardAccessCases: [(enabled: Bool, hideSensitive: Bool, sensitive: Bool, record: Bool, show: Bool)] = [
    (false, false, false, false, true),
    (false, false, true, false, true),
    (false, true, false, false, true),
    (false, true, true, false, false),
    (true, false, false, true, true),
    (true, false, true, true, true),
    (true, true, false, true, true),
    (true, true, true, false, false),
]
for item in clipboardAccessCases {
    let access = KeyTaoClipboardAccessPolicy(
        historyEnabled: item.enabled, hideSensitive: item.hideSensitive, isSensitive: item.sensitive
    )
    let settings = "enabled=\(item.enabled), hideSensitive=\(item.hideSensitive), sensitive=\(item.sensitive)"
    expect(access.canRecord, item.record, "clipboard recording decision for \(settings)")
    expect(access.canShowHistory, item.show, "clipboard visibility decision for \(settings)")
    for suggestionsEnabled in [false, true] {
        expect(
            access.canSuggest(enabled: suggestionsEnabled), suggestionsEnabled && item.record,
            "clipboard suggestion decision for \(settings), suggestionsEnabled=\(suggestionsEnabled)"
        )
    }
}

var clipboard = KeyTaoClipboardHistory()
clipboard.upsert(KeyTaoClipboardRecord(key: "pinned", text: "Keep me", timestamp: 0))
clipboard.togglePin("pinned")
for index in 1...60 {
    clipboard.upsert(KeyTaoClipboardRecord(key: "text-\(index)", text: "Text \(index)", timestamp: Double(index)))
}
expect(clipboard.records.count, 51, "fifty unpinned entries do not count the pinned entry")
expect(clipboard.records.filter { !$0.pinned }.count, 50, "clipboard unpinned capacity is fifty")
expect(clipboard.records.first?.key, "pinned", "an old pin stays above new unpinned entries")
expect(clipboard.records.dropFirst().first?.key, "text-60", "unpinned clipboard entries use descending recency")
expect(clipboard.records.last?.key, "text-11", "oldest unpinned clipboard entries are evicted")
clipboard.upsert(KeyTaoClipboardRecord(key: "pinned", text: "Updated pin", timestamp: 100))
expect(clipboard.records.count, 51, "recapturing a clipboard entry deduplicates its key")
expect(clipboard.records.first?.pinned, true, "recapturing a pinned entry preserves its pin")
expect(clipboard.records.first?.text, "Updated pin", "recapturing a pinned entry updates its payload")
clipboard.upsert(KeyTaoClipboardRecord(key: "text-11", text: "Recaptured", timestamp: 101))
expect(clipboard.records.count, 51, "recapturing an unpinned entry does not consume capacity")
expect(clipboard.records.dropFirst().first?.key, "text-11", "recapturing an unpinned entry updates its recency")
clipboard.togglePin("text-11")
expect(clipboard.records.first?.key, "text-11", "pins are also sorted by recency")
clipboard.togglePin("missing")
clipboard.remove("text-60")
expect(clipboard.records.contains { $0.key == "text-60" }, false, "individual unpinned entries can be deleted")
clipboard.clearUnpinned()
expect(clipboard.records.map(\.key), ["text-11", "pinned"], "clear-all preserves every pin")
expect(clipboard.records.allSatisfy(\.pinned), true, "clear-all removes all unpinned entries")
clipboard.remove("pinned")
expect(clipboard.records.map(\.key), ["text-11"], "individual deletion may remove a pinned entry")

var unpinOverflow = KeyTaoClipboardHistory(records: [
    KeyTaoClipboardRecord(key: "old-pin", timestamp: 0, pinned: true),
] + (1...50).map { KeyTaoClipboardRecord(key: "new-\($0)", timestamp: Double($0)) })
unpinOverflow.togglePin("old-pin")
expect(unpinOverflow.records.count, 50, "unpinning re-applies the unpinned capacity")
expect(unpinOverflow.records.contains { $0.key == "old-pin" }, false, "an old unpinned item is evicted before newer entries")
let duplicateHistory = KeyTaoClipboardHistory(records: [
    KeyTaoClipboardRecord(key: "same", text: "Old", timestamp: 1),
    KeyTaoClipboardRecord(key: "same", text: "New", timestamp: 2),
])
expect(duplicateHistory.records.count, 1, "initial clipboard records are deduplicated")
expect(duplicateHistory.records.first?.text, "New", "initial clipboard deduplication keeps the most recent value")

let clipboardLimitRecords = [KeyTaoClipboardRecord(key: "limit-pin", timestamp: 0, pinned: true)]
    + (1...210).map { KeyTaoClipboardRecord(key: "limit-\($0)", timestamp: Double($0)) }
do {
    let saved = KeyTaoClipboardHistory(records: clipboardLimitRecords, limit: 200)
    var loading = try KeyTaoClipboardHistory(data: saved.encoded(), limit: 200)
    for _ in 0..<2 {
        expect(loading.setLimit(nil), false, "unknown configuration does not trim history below two hundred entries")
        expect(loading.limit, 200, "history loading and presentation use the maximum cap before config loads")
        expect(loading.records, saved.records, "unknown configuration preserves all two hundred entries and pins")
    }
    let historyFile = FileManager.default.temporaryDirectory.appendingPathComponent("keytao-clipboard-loading-\(UUID().uuidString).json")
    defer { try? FileManager.default.removeItem(at: historyFile) }
    var editing = loading
    editing.togglePin("limit-210")
    try editing.encoded().write(to: historyFile, options: .atomic)
    expect(try KeyTaoClipboardHistory(contentsOf: historyFile, limit: 200).records, editing.records, "pinning persists while configuration is unknown")
    editing.togglePin("limit-210")
    try editing.encoded().write(to: historyFile, options: .atomic)
    expect(try KeyTaoClipboardHistory(contentsOf: historyFile, limit: 200).records, saved.records, "unpinning persists without trimming below two hundred entries while configuration is unknown")
    editing.remove("limit-209")
    try editing.encoded().write(to: historyFile, options: .atomic)
    expect(try KeyTaoClipboardHistory(contentsOf: historyFile, limit: 200).records, editing.records, "deletion persists while configuration is unknown")
    editing.clearUnpinned()
    try editing.encoded().write(to: historyFile, options: .atomic)
    expect(try KeyTaoClipboardHistory(contentsOf: historyFile, limit: 200).records, saved.records.filter(\.pinned), "clearing persists and preserves pins while configuration is unknown")
    expect(loading.setLimit(200), false, "loading the user's two hundred entry limit preserves history")
    expect(loading.records, saved.records, "the user's larger history survives config loading")
    expect(loading.setLimit(50), true, "a known smaller user limit requests persistence of its trim")
    expect(loading.records.filter { !$0.pinned }.count, 50, "a known smaller user limit trims unpinned history")
    expect(loading.records.first?.key, "limit-pin", "applying a known limit preserves pins")
} catch {
    failures.append("clipboard history before config load failed: \(error)")
}
for (requested, expected) in [(10, 10), (200, 200), (-1, 10), (201, 200)] {
    let limited = KeyTaoClipboardHistory(records: clipboardLimitRecords, limit: requested)
    expect(limited.records.count, expected + 1, "requested limit \(requested) caps only unpinned records at \(expected)")
    expect(limited.records.first?.key, "limit-pin", "requested limit \(requested) preserves an old pin")
    expect(limited.records.dropFirst().first?.key, "limit-210", "requested limit \(requested) keeps the newest unpinned item")
    expect(limited.records.last?.key, "limit-\(211 - expected)", "requested limit \(requested) evicts the oldest unpinned items")
}
var customClipboard = KeyTaoClipboardHistory(records: Array(clipboardLimitRecords.prefix(11)), limit: 10)
customClipboard.upsert(KeyTaoClipboardRecord(key: "limit-11", timestamp: 11))
expect(customClipboard.records.count, 11, "insertion applies the configured unpinned limit")
expect(customClipboard.records.contains { $0.key == "limit-1" }, false, "insertion evicts the oldest unpinned item at the configured limit")
customClipboard.togglePin("limit-2")
customClipboard.upsert(KeyTaoClipboardRecord(key: "limit-12", timestamp: 12))
expect(customClipboard.records.count, 12, "pinning frees an unpinned slot without evicting a record")
expect(customClipboard.records.filter(\.pinned).map(\.key), ["limit-2", "limit-pin"], "configured limits preserve every pin")
customClipboard.togglePin("limit-pin")
expect(customClipboard.records.count, 11, "unpinning re-applies the configured limit")
expect(customClipboard.records.contains { $0.key == "limit-pin" }, false, "unpinning an old item evicts it before recent entries")

var resizedClipboard = KeyTaoClipboardHistory(records: clipboardLimitRecords, limit: 200)
resizedClipboard.setLimit(10)
expect(resizedClipboard.records.count, 11, "lowering the limit trims existing history immediately")
expect(resizedClipboard.records.last?.key, "limit-201", "lowering the limit keeps the ten newest unpinned items")
expect(resizedClipboard.records.first?.key, "limit-pin", "lowering the limit never evicts a pin")
resizedClipboard.setLimit(-1)
expect(resizedClipboard.records.count, 11, "changing the limit clamps below ten")
resizedClipboard.setLimit(500)
for index in 211...410 {
    resizedClipboard.upsert(KeyTaoClipboardRecord(key: "limit-\(index)", timestamp: Double(index)))
}
expect(resizedClipboard.records.count, 201, "raising the limit clamps at two hundred and permits new entries")
expect(resizedClipboard.records.last?.key, "limit-211", "raised limits retain the newest two hundred unpinned items")
do {
    let restoredLimited = try KeyTaoClipboardHistory(data: resizedClipboard.encoded(), limit: 10)
    expect(restoredLimited.records.count, 11, "loading persisted history applies the current limit")
    expect(restoredLimited.records.first?.key, "limit-pin", "loading with a smaller limit preserves pins")
    expect(restoredLimited.records.last?.key, "limit-401", "loading with a smaller limit evicts oldest unpinned entries")
    var restoredInsertion = restoredLimited
    restoredInsertion.upsert(KeyTaoClipboardRecord(key: "limit-411", timestamp: 411))
    expect(restoredInsertion.records.count, 11, "restored stores keep using their configured limit on insertion")
    expect(restoredInsertion.records.last?.key, "limit-402", "restored store insertion evicts its oldest unpinned item")
} catch {
    failures.append("configured clipboard limit roundtrip failed: \(error)")
}

let mediaRecord = KeyTaoClipboardRecord(
    key: "media:image", mime: "image/png", name: "photo.png", size: 123,
    width: 30, height: 40, timestamp: 102, pinned: true
)
clipboard.upsert(mediaRecord)
let clipboardTextByteLimit = 64 * 1024
let clipboardHistoryByteLimit = 1024 * 1024
let boundaryText = String(repeating: "a", count: clipboardTextByteLimit)
var boundedClipboard = KeyTaoClipboardHistory()
boundedClipboard.upsert(KeyTaoClipboardRecord(key: boundaryText, text: boundaryText, timestamp: 1))
expect(boundedClipboard.records.count, 1, "text at the UTF-8 byte limit is retained")
boundedClipboard.upsert(KeyTaoClipboardRecord(key: "oversized", text: boundaryText + "a", timestamp: 2))
boundedClipboard.upsert(KeyTaoClipboardRecord(key: "unicode", text: String(repeating: "😀", count: clipboardTextByteLimit / 4 + 1), timestamp: 3))
expect(boundedClipboard.records.map(\.key) == [boundaryText], true, "oversized ASCII and multibyte text are rejected")
let oversizedLegacyClipboard = KeyTaoClipboardHistory(records: [
    KeyTaoClipboardRecord(key: "legacy", text: boundaryText + "a", timestamp: 1, pinned: true),
    mediaRecord,
])
expect(oversizedLegacyClipboard.records == [mediaRecord], true, "legacy oversized text is discarded even when pinned")
do {
    // Control characters exercise JSON escaping, and real text keys duplicate the payload.
    var budgetClipboard = KeyTaoClipboardHistory()
    for index in 1...60 {
        let text = "\(index)" + String(repeating: "\u{0001}", count: 16 * 1024)
        budgetClipboard.upsert(KeyTaoClipboardRecord(key: text, text: text, timestamp: Double(index)))
    }
    expect(try budgetClipboard.encoded().count <= clipboardHistoryByteLimit, true, "serialized history stays within its total byte budget")
    expect(budgetClipboard.records.count < 50, true, "the byte budget evicts old text before the count limit")
    expect(budgetClipboard.records.first?.timestamp, 60, "byte eviction retains the newest unpinned entry")
    for record in budgetClipboard.records { budgetClipboard.togglePin(record.key) }
    for index in 61...120 {
        let text = "\(index)" + String(repeating: "\u{0001}", count: 16 * 1024)
        budgetClipboard.upsert(KeyTaoClipboardRecord(key: text, text: text, timestamp: Double(index)))
        budgetClipboard.togglePin(text)
    }
    expect(try budgetClipboard.encoded().count <= clipboardHistoryByteLimit, true, "pins cannot bypass the serialized byte budget")
    expect(budgetClipboard.records.first?.pinned, true, "byte eviction retains pinned-entry priority")
} catch {
    failures.append("clipboard byte budget failed: \(error)")
}
do {
    let restoredClipboard = try KeyTaoClipboardHistory(data: clipboard.encoded())
    expect(restoredClipboard.records, clipboard.records, "clipboard text, media metadata and pins persist through JSON")
    var restoredCleared = restoredClipboard
    restoredCleared.clearUnpinned()
    expect(restoredCleared.records, clipboard.records, "restored pins survive clear-all")
    let emptyClipboard = try KeyTaoClipboardHistory(data: KeyTaoClipboardHistory().encoded())
    expect(emptyClipboard.records.isEmpty, true, "empty clipboard history persists")
    let historyFile = FileManager.default.temporaryDirectory.appendingPathComponent("keytao-clipboard-\(UUID().uuidString).json")
    defer { try? FileManager.default.removeItem(at: historyFile) }
    try clipboard.encoded().write(to: historyFile, options: .atomic)
    var diskClipboard = try KeyTaoClipboardHistory(contentsOf: historyFile)
    expect(diskClipboard.records, clipboard.records, "clipboard history survives an atomic file write and a fresh load")
    for index in 200...260 {
        diskClipboard.upsert(KeyTaoClipboardRecord(key: "after-restart-\(index)", timestamp: Double(index)))
    }
    expect(diskClipboard.records.filter(\.pinned), clipboard.records, "restored pins survive later eviction")
    diskClipboard.clearUnpinned()
    expect(diskClipboard.records, clipboard.records, "restored pins survive clear-all after later captures")
    let oversizedData = Data(repeating: 0, count: clipboardHistoryByteLimit + 1)
    expect(try KeyTaoClipboardHistory(data: oversizedData).records.isEmpty, true, "oversized JSON is skipped before decoding")
    let handle = try FileHandle(forWritingTo: historyFile)
    try handle.truncate(atOffset: 20 * 1024 * 1024)
    try handle.close()
    expect(try KeyTaoClipboardHistory(contentsOf: historyFile).records.isEmpty, true, "a bloated legacy file is skipped before reading its contents")
    try clipboard.encoded().write(to: historyFile, options: .atomic)
    expect(try KeyTaoClipboardHistory(contentsOf: historyFile).records, clipboard.records, "valid history can be saved and loaded after skipping a bloated file")
} catch {
    failures.append("clipboard roundtrip failed: \(error)")
}
for invalidData in ["not json", "{}", "[{}]", "[{\"key\":\"missing-timestamp\"}]"] {
    do {
        _ = try KeyTaoClipboardHistory(data: Data(invalidData.utf8))
        failures.append("malformed clipboard data was accepted: \(invalidData)")
    } catch {
        checks += 1
    }
}

let emojiIndex = KeyTaoEmojiSearchIndex(keywords: [
    "😀": ["笑", "笑脸", "GRINNING FACE"],
    "😁": ["笑颜"],
    "😂": ["笑哭", "喜极而泣", "lol"],
    "😄": ["哈哈大笑", "开心"],
    "😺": ["笑脸猫"],
    "🧑🏽‍💻": ["程序员", "Coder"],
])
expect(emojiIndex.search(query: "笑", recents: []), ["😀", "😁", "😂", "😺", "😄"], "emoji search ranks exact before prefix before contains")
expect(emojiIndex.search(query: "笑", recents: ["😄", "😂", "😁"]), ["😀", "😂", "😁", "😺", "😄"], "recents boost results within their match rank")
expect(emojiIndex.search(query: "笑", recents: ["😂", "😁", "😂"]), ["😀", "😂", "😁", "😺", "😄"], "duplicate recents preserve their first ranking")
expect(emojiIndex.search(query: "  grinning FACE\n", recents: []), ["😀"], "emoji search normalizes query and tts name case")
expect(emojiIndex.search(query: "喜极而泣", recents: []), ["😂"], "emoji search matches Chinese tts names")
expect(emojiIndex.search(query: "程序", recents: []), ["🧑🏽‍💻"], "emoji search preserves joined skin-tone Unicode results")
expect(emojiIndex.search(query: "🧑🏽‍💻", recents: []), ["🧑🏽‍💻"], "emoji search may match the complete emoji itself")
expect(emojiIndex.search(query: "missing", recents: ["😀"]), [], "recents do not inject unrelated results")
expect(emojiIndex.search(query: " \n", recents: ["😄", "😀", "😄"]), ["😄", "😀"], "empty queries return unique recents in recency order")
expect(emojiIndex.search(query: "笑", recents: [], limit: 2), ["😀", "😁"], "emoji search caps ranked results")
expect(emojiIndex.search(query: "", recents: ["😄", "😀"], limit: 1), ["😄"], "empty-query recents respect the result cap")
expect(emojiIndex.search(query: "笑", recents: [], limit: 0), [], "zero result cap is a no-op")
expect(emojiIndex.search(query: "", recents: ["😀"], limit: -1), [], "negative result caps are a no-op")
let reversedEmojiIndex = KeyTaoEmojiSearchIndex(keywords: ["😺": ["笑脸猫"], "😂": ["笑哭"], "😁": ["笑颜"]])
expect(reversedEmojiIndex.search(query: "笑", recents: []), ["😁", "😂", "😺"], "equal-rank emoji ordering is deterministic")

let profile = KeyTaoIMEInteractionTuning.backspaceProfile(for: .standard)
let policy = KeyTaoBackspaceRepeatPolicy(profile: profile)

expect(profile.initialDelayMs, 400, "standard initial delay")
expect(profile.intervalMs, 50, "standard repeat interval")
expect(profile.segmentThresholdMs, 1_500, "standard segment threshold")
expect(policy.repeatCount(at: 399), 0, "no repeat before initial delay")
expect(policy.repeatCount(at: 1_000), 13, "one second produces thirteen repeats after the initial deletion")
expect(policy.granularity(at: 1_499), .character, "character deletion before escalation")
expect(policy.granularity(at: 1_500), .segment, "segment deletion at escalation threshold")

expect(keyTaoTrailingDeletionSegmentLength("前文，测试"), 2, "Chinese segment")
expect(keyTaoTrailingDeletionSegmentLength("测试hello"), 5, "Latin word after Chinese")
expect(keyTaoTrailingDeletionSegmentLength("hello中文"), 2, "Chinese run after Latin")
expect(keyTaoTrailingDeletionSegmentLength("hello！"), 1, "punctuation is its own segment")
expect(keyTaoTrailingDeletionSegmentsLength("中文 hello", count: 1), 5, "one selection step covers one trailing segment")
expect(keyTaoTrailingDeletionSegmentsLength("中文 hello", count: 2), 6, "two selection steps include the preceding segment")
expect(keyTaoTrailingDeletionSegmentsLength("中文 hello", count: 3), 8, "selection steps stop at segment boundaries")

let longDeletionContext = String(repeating: "中😀e\u{301}", count: 2_000)
let boundedDeletion = keyTaoBackwardDeletionBatch(longDeletionContext, remainingLimit: 4_096)
expect(boundedDeletion.count, 4_096, "delete-all stops at the character budget")
expect(
    String(longDeletionContext.dropLast(boundedDeletion.count)) + boundedDeletion.reversed().joined(),
    longDeletionContext,
    "undo restores only the deleted suffix without duplicating the untouched prefix"
)
expect(
    keyTaoBackwardDeletionBatch("a中😀e\u{301}", remainingLimit: 2),
    ["e\u{301}", "😀"],
    "deletion batches preserve supplementary and combining characters"
)
expect(keyTaoBackwardDeletionBatch("中", remainingLimit: 0), [], "an exhausted deletion budget is a no-op")
expect(keyTaoBackwardDeletionBatch("中", remainingLimit: -1), [], "a negative deletion budget is a no-op")

let firstMarkedDocument = UUID()
let nextMarkedDocument = UUID()
var markedOwnership = KeyTaoMarkedTextOwnership()
expect(markedOwnership.belongs(to: nil), false, "unknown documents do not share marked-text ownership")
expect(markedOwnership.release(for: nil), false, "unknown documents cannot clear unowned marked text")
markedOwnership.claim(firstMarkedDocument)
expect(markedOwnership.belongs(to: nil), false, "unknown identity cannot reuse an existing document's mark")
expect(markedOwnership.release(for: nil), false, "unknown identity discards ownership without clearing host text")
expect(markedOwnership.belongs(to: firstMarkedDocument), false, "ownership stays discarded if the old document returns")
markedOwnership.claim(firstMarkedDocument)
markedOwnership.claim(nil)
expect(markedOwnership.belongs(to: nil), false, "claiming unknown identity never creates reusable ownership")
expect(markedOwnership.belongs(to: firstMarkedDocument), false, "claiming unknown identity forgets the old mark")
expect(markedOwnership.release(for: nil), false, "two unknown identities never authorize a marked-text clear")
markedOwnership.claim(firstMarkedDocument)
expect(markedOwnership.belongs(to: firstMarkedDocument), true, "mode changes see the current document's marked text")
expect(markedOwnership.belongs(to: nextMarkedDocument), false, "mode changes do not own another document's mark")
expect(markedOwnership.release(for: firstMarkedDocument), true, "the owning document may clear its marked text")
expect(markedOwnership.belongs(to: firstMarkedDocument), false, "released marked text is absent from mode-change checks")
expect(markedOwnership.release(for: firstMarkedDocument), false, "a reentrant callback cannot clear an already released mark")
markedOwnership.claim(firstMarkedDocument)
expect(markedOwnership.release(for: nextMarkedDocument), false, "a switched document must not receive the old mark's clear")
expect(markedOwnership.release(for: firstMarkedDocument), false, "discarded ownership cannot revive on a late callback")
markedOwnership.claim(nextMarkedDocument)
expect(markedOwnership.release(for: nextMarkedDocument), true, "the next document can own a fresh mark")

var replayGeneration = 0
var replayedCommands: [String] = []
keyTaoReplayPendingCommands(
    ["first", "old second"],
    isCurrent: { replayGeneration == 0 },
    perform: {
        replayedCommands.append($0)
        replayGeneration += 1
    }
)
expect(replayedCommands, ["first"], "a synchronous context reset stops an already copied startup queue")
keyTaoReplayPendingCommands(
    ["already stale"],
    isCurrent: { false },
    perform: { replayedCommands.append($0) }
)
expect(replayedCommands, ["first"], "a stale queue never invokes its first command")
keyTaoReplayPendingCommands(
    ["new first", "new second"],
    isCurrent: { true },
    perform: { replayedCommands.append($0) }
)
expect(replayedCommands, ["first", "new first", "new second"], "a stable context preserves replay order")

expect(KeyTaoBackspaceGestureMode(setting: nil), .immediate, "backspace gesture defaults to immediate deletion")
expect(KeyTaoBackspaceGestureMode(setting: "selectThenDelete"), .selectThenDelete, "selection mode parses from runtime settings")
expect(
    KeyTaoBackspaceGesturePolicy.dragCommand(mode: .immediate, currentUnits: 1, requestedUnits: 3, maximumUnits: 96),
    KeyTaoBackspaceGestureCommand(action: "delete", count: 2),
    "immediate mode deletes the forward delta"
)
expect(
    KeyTaoBackspaceGesturePolicy.dragCommand(mode: .immediate, currentUnits: 3, requestedUnits: 1, maximumUnits: 96),
    KeyTaoBackspaceGestureCommand(action: "restore", count: 2),
    "immediate mode restores the reverse delta"
)
expect(
    KeyTaoBackspaceGesturePolicy.dragCommand(mode: .selectThenDelete, currentUnits: 1, requestedUnits: 3, maximumUnits: 96),
    KeyTaoBackspaceGestureCommand(action: "select", count: 3),
    "selection mode updates the pending selection without deleting"
)
expect(
    KeyTaoBackspaceGesturePolicy.dragCommand(mode: .selectThenDelete, currentUnits: 3, requestedUnits: -2, maximumUnits: 96),
    KeyTaoBackspaceGestureCommand(action: "cancelSelection", count: 0),
    "selection mode cancels after dragging back to zero"
)
expect(
    KeyTaoBackspaceGesturePolicy.releaseCommand(mode: .selectThenDelete, selectedUnits: 3),
    KeyTaoBackspaceGestureCommand(action: "commitSelection", count: 3),
    "selection mode deletes only on release"
)

expect(
    keyTaoEnglishSchemaID(schemas: [("english", "English"), ("easy_en", "Easy English")]),
    "easy_en",
    "English schema resolution prefers easy_en"
)
expect(keyTaoEnglishSchemaID(schemas: [("english", "English")]), "english", "English schema ID fallback")
expect(keyTaoEnglishSchemaID(schemas: [("custom_easy", "Easy English")]), "custom_easy", "Easy English name fallback")
expect(keyTaoEnglishSchemaID(schemas: [("custom_english", "eNgLiSh")]), "custom_english", "English name fallback")
expect(keyTaoEnglishSchemaID(schemas: [("keytao", "键道")]), nil, "missing English schema")

let legacyAscii = keyTaoLanguageModeDecision(
    englishMode: "schema",
    englishSchemaID: nil,
    value: "ascii",
    currentSchemaID: "keytao",
    asciiMode: false
)
let legacyChinese = keyTaoLanguageModeDecision(
    englishMode: "schema",
    englishSchemaID: nil,
    value: "chinese",
    currentSchemaID: "keytao",
    asciiMode: true
)
let legacyToggle = keyTaoLanguageModeDecision(
    englishMode: "schema",
    englishSchemaID: nil,
    value: nil,
    currentSchemaID: "keytao",
    asciiMode: false
)
let configuredAscii = keyTaoLanguageModeDecision(
    englishMode: "ascii",
    englishSchemaID: "easy_en",
    value: nil,
    currentSchemaID: "keytao",
    asciiMode: false
)
let schemaEnglish = keyTaoLanguageModeDecision(
    englishMode: "schema",
    englishSchemaID: "easy_en",
    value: "ascii",
    currentSchemaID: "keytao",
    asciiMode: false
)
let schemaChinese = keyTaoLanguageModeDecision(
    englishMode: "schema",
    englishSchemaID: "easy_en",
    value: "chinese",
    currentSchemaID: "easy_en",
    asciiMode: true
)
let schemaToggleToEnglish = keyTaoLanguageModeDecision(
    englishMode: "schema",
    englishSchemaID: "easy_en",
    value: nil,
    currentSchemaID: "keytao",
    asciiMode: true
)
let schemaToggleToChinese = keyTaoLanguageModeDecision(
    englishMode: "schema",
    englishSchemaID: "easy_en",
    value: nil,
    currentSchemaID: "easy_en",
    asciiMode: false
)

expect(legacyAscii.usesEnglishSchema, false, "legacy ASCII request keeps the ascii_mode path")
expect(legacyAscii.targetEnglish, true, "legacy ASCII request enables ascii_mode")
expect(legacyChinese.targetEnglish, false, "legacy Chinese request disables ascii_mode")
expect(legacyToggle.targetEnglish, true, "legacy toggle flips ascii_mode")
expect(configuredAscii.usesEnglishSchema, false, "ASCII setting ignores an installed English schema")
expect(configuredAscii.targetEnglish, true, "ASCII setting toggles ascii_mode")
expect(schemaEnglish.usesEnglishSchema, true, "English schema selects the schema path")
expect(schemaEnglish.targetEnglish, true, "ASCII request selects the English schema")
expect(schemaChinese.targetEnglish, false, "Chinese request selects the Chinese schema")
expect(schemaToggleToEnglish.targetEnglish, true, "Chinese schema toggles to English")
expect(schemaToggleToChinese.targetEnglish, false, "English schema toggles to Chinese")

let switchValues = [
    "ascii_mode": true,
    "simplification": false,
    "emoji_cn": true,
    "danzi_mode": false,
    "ascii_punct": true,
]
let switchSnapshot = keyTaoChineseSwitchSnapshot(names: Array(switchValues.keys)) {
    switchValues[$0] ?? false
}
expect(
    switchSnapshot,
    ["simplification": false, "emoji_cn": true, "danzi_mode": false, "ascii_punct": true],
    "Chinese switch snapshot and restore exclude ascii_mode"
)
expect(switchSnapshot["ascii_mode"], nil, "ascii_mode is never restored across schema changes")

let cursorTracker = KeyTaoCursorGestureTracker(startX: 100)
expect(cursorTracker.update(x: 112.5), KeyTaoCursorGestureUpdate(active: false, stepDelta: 0), "cursor gesture ignores touch noise")
expect(cursorTracker.update(x: 112.6), KeyTaoCursorGestureUpdate(active: true, stepDelta: 1), "cursor gesture activates at the threshold")
expect(cursorTracker.update(x: 120), KeyTaoCursorGestureUpdate(active: true, stepDelta: 1), "cursor gesture emits fixed forward steps")
expect(cursorTracker.update(x: 90), KeyTaoCursorGestureUpdate(active: true, stepDelta: -3), "cursor gesture emits reverse steps")

let sameKeyTracker = KeyTaoPerPointerBounceTracker<Int>()
var sameKeyCommitCount = 0
if !sameKeyTracker.isBounceDown(pointerID: 0, eventTimeMs: 0, x: 10, y: 10) { sameKeyCommitCount += 1 }
sameKeyTracker.recordUp(pointerID: 0, eventTimeMs: 25, x: 10, y: 10)
if !sameKeyTracker.isBounceDown(pointerID: 0, eventTimeMs: 85, x: 10, y: 10) { sameKeyCommitCount += 1 }
sameKeyTracker.recordUp(pointerID: 0, eventTimeMs: 110, x: 10, y: 10)
expect(sameKeyCommitCount, 2, "same-key 60ms-gap double tap commits both clean 25ms taps")

let differentKeyTracker = KeyTaoPerPointerBounceTracker<Int>()
var differentKeyCommitCount = 0
if !differentKeyTracker.isBounceDown(pointerID: 0, eventTimeMs: 0, x: 10, y: 10) { differentKeyCommitCount += 1 }
differentKeyTracker.recordUp(pointerID: 0, eventTimeMs: 25, x: 10, y: 10)
if !differentKeyTracker.isBounceDown(pointerID: 0, eventTimeMs: 55, x: 30, y: 10) { differentKeyCommitCount += 1 }
differentKeyTracker.recordUp(pointerID: 0, eventTimeMs: 80, x: 30, y: 10)
expect(differentKeyCommitCount, 2, "different-key 30ms-gap typing commits both clean 25ms taps")

let bounceTracker = KeyTaoPerPointerBounceTracker<Int>()
expect(bounceTracker.isBounceDown(pointerID: 0, eventTimeMs: 0, x: 10, y: 10), false, "first down is accepted")
bounceTracker.recordUp(pointerID: 0, eventTimeMs: 25, x: 10, y: 10)
expect(bounceTracker.isBounceDown(pointerID: 0, eventTimeMs: 45, x: 10, y: 10), true, "same-position down 20ms after up is rejected")
expect(bounceTracker.isBounceDown(pointerID: 1, eventTimeMs: 45, x: 10, y: 10), false, "a separate pointer remains independent")

expect(KeyTaoIMEInteractionTuning.isBounceDown(sinceLastUpMs: 39, distanceFromLastUp: 12.59), true, "bounce stays below both boundaries")
expect(KeyTaoIMEInteractionTuning.isBounceDown(sinceLastUpMs: 40, distanceFromLastUp: 12.59), false, "40ms down is accepted")
expect(KeyTaoIMEInteractionTuning.isBounceDown(sinceLastUpMs: 39, distanceFromLastUp: 12.6), false, "12.6pt movement is accepted")
expect(KeyTaoIMEInteractionTuning.isBounceDown(sinceLastUpMs: -1, distanceFromLastUp: 1), false, "out-of-order down is accepted")

var alternateTracker = KeyTaoAlternateSelectionTracker(startX: 100, movementThreshold: 8)
expect(alternateTracker.selectedIndex(x: 100, insideSelection: true, panelLeft: 20, itemWidth: 40, itemCount: 4), 0, "alternate defaults to first item")
expect(alternateTracker.selectedIndex(x: 106, insideSelection: true, panelLeft: 20, itemWidth: 40, itemCount: 4), 0, "touch slop preserves default alternate")
expect(alternateTracker.selectedIndex(x: 61, insideSelection: true, panelLeft: 20, itemWidth: 40, itemCount: 4), 1, "drag updates alternate selection")
expect(alternateTracker.selectedIndex(x: 61, insideSelection: false, panelLeft: 20, itemWidth: 40, itemCount: 4), nil, "release outside cancels alternate selection")

let doubleSpaceTracker = KeyTaoDoubleSpacePeriodTracker()
expect(doubleSpaceTracker.shouldReplaceSpace(nowMs: 1_000, contextBefore: "字", enabled: true, hasComposition: false), false, "first eligible space arms replacement")
expect(doubleSpaceTracker.shouldReplaceSpace(nowMs: 2_100, contextBefore: "字 ", enabled: true, hasComposition: false), true, "replacement includes the 1100ms boundary")
expect(doubleSpaceTracker.shouldReplaceSpace(nowMs: 3_000, contextBefore: "字。", enabled: true, hasComposition: false), false, "punctuation does not arm replacement")
expect(doubleSpaceTracker.shouldReplaceSpace(nowMs: 3_100, contextBefore: "字。 ", enabled: true, hasComposition: false), false, "punctuation followed by space does not replace")
expect(doubleSpaceTracker.shouldReplaceSpace(nowMs: 5_000, contextBefore: "word", enabled: true, hasComposition: false), false, "Latin text arms replacement")
expect(doubleSpaceTracker.shouldReplaceSpace(nowMs: 6_101, contextBefore: "word ", enabled: true, hasComposition: false), false, "replacement expires after 1100ms")
expect(doubleSpaceTracker.shouldReplaceSpace(nowMs: 7_000, contextBefore: "word", enabled: false, hasComposition: false), false, "disabled setting stays inactive")
expect(doubleSpaceTracker.shouldReplaceSpace(nowMs: 7_100, contextBefore: "word ", enabled: true, hasComposition: true), false, "composition stays inactive")

for hex in ["#3B73D9", "#0F9F8F", "#D87A32", "#8B5CF6", "#FFFFFF", "#000000"] {
    guard let hsv = KeyTaoColorMath.hsv(fromHex: hex) else {
        failures.append("\(hex) failed to parse")
        continue
    }
    expect(
        KeyTaoColorMath.hex(hue: hsv.hue, saturation: hsv.saturation, value: hsv.value),
        hex,
        "\(hex) survives a hex to HSV round trip"
    )
}

if let lowercase = KeyTaoColorMath.hsv(fromHex: "#3b73d9") {
    expect(
        KeyTaoColorMath.hex(hue: lowercase.hue, saturation: lowercase.saturation, value: lowercase.value),
        "#3B73D9",
        "lowercase hex round trips to the canonical uppercase form"
    )
} else {
    failures.append("#3b73d9 failed to parse")
}

expect(KeyTaoColorMath.normalizeHue(360), 0, "360 wraps to 0")
expect(KeyTaoColorMath.normalizeHue(-10), 350, "negative hue wraps forward")
expect(KeyTaoColorMath.normalizeHue(730), 10, "hue wraps repeatedly")
expect(
    KeyTaoColorMath.hex(hue: 360, saturation: 1, value: 1),
    KeyTaoColorMath.hex(hue: 0, saturation: 1, value: 1),
    "wrapped hue produces the same colour"
)

expect(KeyTaoColorMath.hex(hue: 210, saturation: 0, value: 0.5019608), "#808080", "zero saturation is grey")
expect(KeyTaoColorMath.hex(hue: 210, saturation: 1, value: 0), "#000000", "zero value is black")
expect(KeyTaoColorMath.hsv(red: 128, green: 128, blue: 128).saturation, 0, "grey has no saturation")
expect(KeyTaoColorMath.hsv(red: 0, green: 0, blue: 0).value, 0, "black has no value")

expect(KeyTaoColorMath.hex(hue: 0, saturation: 1, value: 1), "#FF0000", "hue 0 is red")
expect(KeyTaoColorMath.hex(hue: 120, saturation: 1, value: 1), "#00FF00", "hue 120 is green")
expect(KeyTaoColorMath.hex(hue: 240, saturation: 1, value: 1), "#0000FF", "hue 240 is blue")
expect(KeyTaoColorMath.hsv(red: 0, green: 0, blue: 255).hue, 240, "blue reads back as hue 240")

expect(KeyTaoColorMath.hsv(fromHex: "#12345") == nil, true, "short hex is rejected")
expect(KeyTaoColorMath.hsv(fromHex: "#GGGGGG") == nil, true, "non-hex digits are rejected")
expect(KeyTaoColorMath.hsv(fromHex: "custom") == nil, true, "the custom swatch sentinel is not a colour")
expect(KeyTaoColorMath.hsv(fromHex: "+12345") == nil, true, "a signed literal is not a colour")

if failures.isEmpty {
    print("interaction policy tests passed (\(checks) checks)")
} else {
    for failure in failures {
        FileHandle.standardError.write(Data("FAIL \(failure)\n".utf8))
    }
    exit(1)
}
