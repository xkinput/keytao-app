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

let cursorTracker = KeyTaoCursorGestureTracker(startX: 100)
expect(cursorTracker.update(x: 112.5), KeyTaoCursorGestureUpdate(active: false, stepDelta: 0), "cursor gesture ignores touch noise")
expect(cursorTracker.update(x: 112.6), KeyTaoCursorGestureUpdate(active: true, stepDelta: 1), "cursor gesture activates at the threshold")
expect(cursorTracker.update(x: 120), KeyTaoCursorGestureUpdate(active: true, stepDelta: 1), "cursor gesture emits fixed forward steps")
expect(cursorTracker.update(x: 90), KeyTaoCursorGestureUpdate(active: true, stepDelta: -3), "cursor gesture emits reverse steps")

var alternateTracker = KeyTaoAlternateSelectionTracker(startX: 100, movementThreshold: 8)
expect(alternateTracker.selectedIndex(x: 100, insideSelection: true, panelLeft: 20, itemWidth: 40, itemCount: 4), 0, "alternate defaults to first item")
expect(alternateTracker.selectedIndex(x: 106, insideSelection: true, panelLeft: 20, itemWidth: 40, itemCount: 4), 0, "touch slop preserves default alternate")
expect(alternateTracker.selectedIndex(x: 61, insideSelection: true, panelLeft: 20, itemWidth: 40, itemCount: 4), 1, "drag updates alternate selection")
expect(alternateTracker.selectedIndex(x: 61, insideSelection: false, panelLeft: 20, itemWidth: 40, itemCount: 4), nil, "release outside cancels alternate selection")

if failures.isEmpty {
    print("interaction policy tests passed")
} else {
    for failure in failures {
        FileHandle.standardError.write(Data("FAIL \(failure)\n".utf8))
    }
    exit(1)
}
