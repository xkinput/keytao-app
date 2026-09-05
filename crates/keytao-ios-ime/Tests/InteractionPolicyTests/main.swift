import Foundation

var failures: [String] = []

func expect<T: Equatable>(_ actual: T, _ expected: T, _ message: String, line: Int = #line) {
    if actual != expected {
        failures.append("line \(line): \(message) (expected \(expected), got \(actual))")
    }
}

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

if failures.isEmpty {
    print("interaction policy tests passed")
} else {
    for failure in failures {
        FileHandle.standardError.write(Data("FAIL \(failure)\n".utf8))
    }
    exit(1)
}
