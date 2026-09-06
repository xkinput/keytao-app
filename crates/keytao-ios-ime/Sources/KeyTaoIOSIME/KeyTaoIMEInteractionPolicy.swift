import Foundation

public enum KeyTaoDeleteSpeed: String, Codable {
    case slow
    case standard
    case fast

    public init(setting: String?) {
        self = KeyTaoDeleteSpeed(rawValue: setting?.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() ?? "") ?? .standard
    }
}

struct KeyTaoBackspaceRepeatProfile: Equatable {
    let initialDelayMs: Int
    let intervalMs: Int
    let segmentThresholdMs: Int
}

enum KeyTaoBackspaceDeletionGranularity: Equatable {
    case character
    case segment
}

enum KeyTaoBackspaceGestureMode: String {
    case immediate
    case selectThenDelete

    init(setting: String?) {
        self = KeyTaoBackspaceGestureMode(rawValue: setting ?? "") ?? .immediate
    }
}

struct KeyTaoBackspaceGestureCommand: Equatable {
    let action: String
    let count: Int
}

enum KeyTaoBackspaceGesturePolicy {
    static func dragCommand(
        mode: KeyTaoBackspaceGestureMode,
        currentUnits: Int,
        requestedUnits: Int,
        maximumUnits: Int
    ) -> KeyTaoBackspaceGestureCommand? {
        switch mode {
        case .immediate:
            let target = max(-maximumUnits, min(maximumUnits, requestedUnits))
            let delta = target - currentUnits
            guard delta != 0 else { return nil }
            return KeyTaoBackspaceGestureCommand(
                action: delta > 0 ? "delete" : "restore",
                count: abs(delta)
            )
        case .selectThenDelete:
            let target = max(0, min(maximumUnits, requestedUnits))
            guard target != currentUnits else { return nil }
            return KeyTaoBackspaceGestureCommand(
                action: target == 0 ? "cancelSelection" : "select",
                count: target
            )
        }
    }

    static func releaseCommand(
        mode: KeyTaoBackspaceGestureMode,
        selectedUnits: Int
    ) -> KeyTaoBackspaceGestureCommand? {
        guard mode == .selectThenDelete else { return nil }
        return KeyTaoBackspaceGestureCommand(
            action: selectedUnits > 0 ? "commitSelection" : "cancelSelection",
            count: max(0, selectedUnits)
        )
    }
}

public enum KeyTaoIMEInteractionTuning {
    public static let longPressDelayMinMs = 100
    public static let longPressDelayDefaultMs = 300
    public static let longPressDelayMaxMs = 700
    public static let slideRetargetHysteresis: CGFloat = 8
    public static let bounceIntervalMs = 40
    public static let bounceDistance: CGFloat = 12.6
    public static let backspaceHoldTolerance: CGFloat = 8
    public static let cursorGestureActivation: CGFloat = 12.6
    public static let cursorGestureStep: CGFloat = 10
    public static let candidateDragSlop: CGFloat = 8
    public static let doubleSpacePeriodTimeoutMs = 1_100
    static let repeatableEditIntervalMs = 72
    static let repeatTimerToleranceFraction = 0.2
    static let repeatTimerMaximumToleranceSeconds = 0.01
    static let accentBorderWidth: CGFloat = 1
    static let softAccentKeyFillAmount: CGFloat = 0.22
    static let softAccentKeyBorderAlpha: CGFloat = 0.60
    static let accentToolbarBorderAlpha: CGFloat = 0.60
    static let candidateBorderAlpha: CGFloat = 0.35
    static let darkAccentBorderAlphaMultiplier: CGFloat = 0.70
    static let backspacePreviewMinimumHorizontalInset: CGFloat = 4
    static let backspacePreviewVerticalInset: CGFloat = 6
    static let backspacePreviewTextHorizontalPadding: CGFloat = 8
    static let backspacePreviewMaxTailGraphemes = 18
    public static let colorPreviewThrottleMs: TimeInterval = 0.05

    private static let slowBackspace = KeyTaoBackspaceRepeatProfile(
        initialDelayMs: 500,
        intervalMs: 70,
        segmentThresholdMs: 1_800
    )
    private static let standardBackspace = KeyTaoBackspaceRepeatProfile(
        initialDelayMs: 400,
        intervalMs: 50,
        segmentThresholdMs: 1_500
    )
    private static let fastBackspace = KeyTaoBackspaceRepeatProfile(
        initialDelayMs: 300,
        intervalMs: 35,
        segmentThresholdMs: 1_200
    )

    static func backspaceProfile(for speed: KeyTaoDeleteSpeed) -> KeyTaoBackspaceRepeatProfile {
        switch speed {
        case .slow:
            return slowBackspace
        case .standard:
            return standardBackspace
        case .fast:
            return fastBackspace
        }
    }

    static func isBounceDown(sinceLastUpMs: Double, distanceFromLastUp: CGFloat) -> Bool {
        sinceLastUpMs >= 0
            && sinceLastUpMs < Double(bounceIntervalMs)
            && distanceFromLastUp < bounceDistance
    }
}

/// Atomically replaces the geometry consumed by touch hit-testing. Layout and
/// state changes publish a complete snapshot before the next touch is handled.
final class KeyTaoImmediateHitLayout<Element> {
    private(set) var items: [Element] = []

    func rebuild(_ next: [Element]) {
        items = next
    }

    func first(where predicate: (Element) -> Bool) -> Element? {
        items.first(where: predicate)
    }

    func firstIndex(where predicate: (Element) -> Bool) -> Int? {
        items.firstIndex(where: predicate)
    }
}

final class KeyTaoPerPointerBounceTracker<PointerID: Hashable> {
    private struct PreviousUp {
        let eventTimeMs: Double
        let x: CGFloat
        let y: CGFloat
    }

    private var previousUps: [PointerID: PreviousUp] = [:]
    private var bouncedPointers: Set<PointerID> = []

    func isBounceDown(
        pointerID: PointerID,
        eventTimeMs: Double,
        x: CGFloat,
        y: CGFloat
    ) -> Bool {
        let isBounce = previousUps[pointerID].map { previousUp in
            KeyTaoIMEInteractionTuning.isBounceDown(
                sinceLastUpMs: eventTimeMs - previousUp.eventTimeMs,
                distanceFromLastUp: hypot(x - previousUp.x, y - previousUp.y)
            )
        } ?? false
        if isBounce {
            bouncedPointers.insert(pointerID)
        } else {
            bouncedPointers.remove(pointerID)
        }
        return isBounce
    }

    @discardableResult
    func recordUp(
        pointerID: PointerID,
        eventTimeMs: Double,
        x: CGFloat,
        y: CGFloat
    ) -> Bool {
        previousUps[pointerID] = PreviousUp(eventTimeMs: eventTimeMs, x: x, y: y)
        return bouncedPointers.remove(pointerID) != nil
    }

    func cancel(pointerID: PointerID) {
        bouncedPointers.remove(pointerID)
    }

    func reset() {
        previousUps.removeAll()
        bouncedPointers.removeAll()
    }
}

final class KeyTaoDoubleSpacePeriodTracker {
    private let timeoutMs: Int
    private var lastEligibleSpaceTimeMs: Int?

    init(timeoutMs: Int = KeyTaoIMEInteractionTuning.doubleSpacePeriodTimeoutMs) {
        self.timeoutMs = timeoutMs
    }

    func shouldReplaceSpace(
        nowMs: Int,
        contextBefore: String,
        enabled: Bool,
        hasComposition: Bool
    ) -> Bool {
        guard enabled, !hasComposition else {
            reset()
            return false
        }
        let canReplace = lastEligibleSpaceTimeMs.map { nowMs - $0 >= 0 && nowMs - $0 <= timeoutMs } == true
            && contextBefore.hasSuffix(" ")
            && keyTaoHasDoubleSpaceEligibleSuffix(String(contextBefore.dropLast()))
        if canReplace {
            reset()
            return true
        }
        lastEligibleSpaceTimeMs = keyTaoHasDoubleSpaceEligibleSuffix(contextBefore) ? nowMs : nil
        return false
    }

    func reset() {
        lastEligibleSpaceTimeMs = nil
    }
}

private func keyTaoHasDoubleSpaceEligibleSuffix(_ text: String) -> Bool {
    guard let last = text.last else {
        return false
    }
    switch keyTaoDeletionSegmentClass(last) {
    case .whitespace, .punctuation:
        return false
    default:
        return true
    }
}

struct KeyTaoCursorGestureUpdate: Equatable {
    let active: Bool
    let stepDelta: Int
}

struct KeyTaoAlternateSelectionTracker {
    private let startX: CGFloat
    private let movementThreshold: CGFloat
    private var hasMoved = false

    init(startX: CGFloat, movementThreshold: CGFloat) {
        self.startX = startX
        self.movementThreshold = movementThreshold
    }

    mutating func selectedIndex(
        x: CGFloat,
        insideSelection: Bool,
        panelLeft: CGFloat,
        itemWidth: CGFloat,
        itemCount: Int
    ) -> Int? {
        guard insideSelection, itemWidth > 0, itemCount > 0 else {
            return nil
        }
        if !hasMoved, abs(x - startX) <= movementThreshold {
            return 0
        }
        hasMoved = true
        return max(0, min(itemCount - 1, Int((x - panelLeft) / itemWidth)))
    }
}

final class KeyTaoCursorGestureTracker {
    private let startX: CGFloat
    private let activationDistance: CGFloat
    private let stepDistance: CGFloat
    private(set) var active = false
    private var dispatchedSteps = 0

    init(
        startX: CGFloat,
        activationDistance: CGFloat = KeyTaoIMEInteractionTuning.cursorGestureActivation,
        stepDistance: CGFloat = KeyTaoIMEInteractionTuning.cursorGestureStep
    ) {
        self.startX = startX
        self.activationDistance = activationDistance
        self.stepDistance = stepDistance
    }

    func update(x: CGFloat) -> KeyTaoCursorGestureUpdate {
        let displacement = x - startX
        if !active, abs(displacement) + Self.floatingPointComparisonEpsilon < activationDistance {
            return KeyTaoCursorGestureUpdate(active: false, stepDelta: 0)
        }
        active = true
        let targetSteps = Int(displacement / stepDistance)
        let delta = targetSteps - dispatchedSteps
        dispatchedSteps = targetSteps
        return KeyTaoCursorGestureUpdate(active: true, stepDelta: delta)
    }


    private static let floatingPointComparisonEpsilon: CGFloat = 0.0001
}

struct KeyTaoBackspaceRepeatPolicy {
    let profile: KeyTaoBackspaceRepeatProfile

    func repeatCount(at holdDurationMs: Int) -> Int {
        guard holdDurationMs >= profile.initialDelayMs else {
            return 0
        }
        return 1 + (holdDurationMs - profile.initialDelayMs) / profile.intervalMs
    }

    func granularity(at holdDurationMs: Int) -> KeyTaoBackspaceDeletionGranularity {
        holdDurationMs >= profile.segmentThresholdMs ? .segment : .character
    }
}

private enum KeyTaoDeletionSegmentClass {
    case whitespace
    case cjk
    case latin
    case punctuation
    case other
}

func keyTaoTrailingDeletionSegmentLength(_ text: String) -> Int {
    max(1, keyTaoTrailingDeletionSegmentsLength(text, count: 1))
}

func keyTaoTrailingDeletionSegmentsLength(_ text: String, count: Int) -> Int {
    let units = Array(text)
    guard !units.isEmpty else { return 0 }
    let limit = max(1, count)
    var selectedUnits = 0
    var selectedSegments = 0
    var previousClass: KeyTaoDeletionSegmentClass?
    for unit in units.reversed() {
        let currentClass = keyTaoDeletionSegmentClass(unit)
        if currentClass != previousClass {
            guard selectedSegments < limit else { break }
            selectedSegments += 1
            previousClass = currentClass
        }
        selectedUnits += 1
    }
    return selectedUnits
}

struct KeyTaoLanguageModeDecision: Equatable {
    let usesEnglishSchema: Bool
    let targetEnglish: Bool
}

func keyTaoEnglishSchemaID(schemas: [(id: String, name: String)]) -> String? {
    schemas.first(where: { $0.id == "easy_en" })?.id
        ?? schemas.first(where: { $0.id == "english" })?.id
        ?? schemas.first(where: {
            $0.name.caseInsensitiveCompare("Easy English") == .orderedSame
                || $0.name.caseInsensitiveCompare("English") == .orderedSame
        })?.id
}

func keyTaoLanguageModeDecision(
    englishMode: String,
    englishSchemaID: String?,
    value: String?,
    currentSchemaID: String?,
    asciiMode: Bool
) -> KeyTaoLanguageModeDecision {
    let usesEnglishSchema = englishMode.trimmingCharacters(in: .whitespacesAndNewlines)
        .caseInsensitiveCompare("schema") == .orderedSame && englishSchemaID != nil
    let targetEnglish: Bool
    switch value?.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() {
    case "ascii", "english", "en":
        targetEnglish = true
    case "chinese", "zh", "cn":
        targetEnglish = false
    default:
        targetEnglish = usesEnglishSchema ? currentSchemaID != englishSchemaID : !asciiMode
    }
    return KeyTaoLanguageModeDecision(
        usesEnglishSchema: usesEnglishSchema,
        targetEnglish: targetEnglish
    )
}

func keyTaoChineseSwitchSnapshot(
    names: [String],
    optionValue: (String) -> Bool
) -> [String: Bool] {
    var snapshot: [String: Bool] = [:]
    for name in names where name != "ascii_mode" && snapshot[name] == nil {
        snapshot[name] = optionValue(name)
    }
    return snapshot
}

private func keyTaoDeletionSegmentClass(_ character: Character) -> KeyTaoDeletionSegmentClass {
    if character.isWhitespace {
        return .whitespace
    }
    guard let scalar = character.unicodeScalars.first else {
        return .other
    }
    if keyTaoIsCJK(scalar.value) {
        return .cjk
    }
    if character.isNumber || keyTaoIsLatin(scalar.value) {
        return .latin
    }
    switch scalar.properties.generalCategory {
    case .connectorPunctuation,
         .dashPunctuation,
         .openPunctuation,
         .closePunctuation,
         .initialPunctuation,
         .finalPunctuation,
         .otherPunctuation:
        return .punctuation
    default:
        return .other
    }
}

private func keyTaoIsCJK(_ value: UInt32) -> Bool {
    (0x3400...0x4DBF).contains(value) ||
        (0x4E00...0x9FFF).contains(value) ||
        (0xF900...0xFAFF).contains(value) ||
        (0x20000...0x3134F).contains(value)
}

private func keyTaoIsLatin(_ value: UInt32) -> Bool {
    (0x0041...0x005A).contains(value) ||
        (0x0061...0x007A).contains(value) ||
        (0x00C0...0x024F).contains(value)
}

public struct KeyTaoHSV: Equatable {
    public var hue: CGFloat
    public var saturation: CGFloat
    public var value: CGFloat

    public init(hue: CGFloat, saturation: CGFloat, value: CGFloat) {
        self.hue = hue
        self.saturation = saturation
        self.value = value
    }
}

/// The only colour maths the in-keyboard 主题色 picker uses: HSV ⇄ sRGB plus `#RRGGBB` text.
public enum KeyTaoColorMath {
    public static func normalizeHue(_ hue: CGFloat) -> CGFloat {
        guard hue.isFinite else { return 0 }
        let wrapped = hue.truncatingRemainder(dividingBy: 360)
        return wrapped < 0 ? wrapped + 360 : wrapped
    }

    public static func rgb(hue: CGFloat, saturation: CGFloat, value: CGFloat) -> (red: Int, green: Int, blue: Int) {
        let h = normalizeHue(hue)
        let s = min(max(saturation, 0), 1)
        let v = min(max(value, 0), 1)
        let chroma = v * s
        let sector = h / 60
        let x = chroma * (1 - abs(sector.truncatingRemainder(dividingBy: 2) - 1))
        let base: (CGFloat, CGFloat, CGFloat)
        switch Int(sector) {
        case 0: base = (chroma, x, 0)
        case 1: base = (x, chroma, 0)
        case 2: base = (0, chroma, x)
        case 3: base = (0, x, chroma)
        case 4: base = (x, 0, chroma)
        default: base = (chroma, 0, x)
        }
        let m = v - chroma
        return (channel(base.0 + m), channel(base.1 + m), channel(base.2 + m))
    }

    public static func hsv(red: Int, green: Int, blue: Int) -> KeyTaoHSV {
        let r = CGFloat(min(max(red, 0), 255)) / 255
        let g = CGFloat(min(max(green, 0), 255)) / 255
        let b = CGFloat(min(max(blue, 0), 255)) / 255
        let maxChannel = max(r, max(g, b))
        let minChannel = min(r, min(g, b))
        let delta = maxChannel - minChannel
        let hue: CGFloat
        if delta == 0 {
            hue = 0
        } else if maxChannel == r {
            hue = 60 * (((g - b) / delta).truncatingRemainder(dividingBy: 6))
        } else if maxChannel == g {
            hue = 60 * ((b - r) / delta + 2)
        } else {
            hue = 60 * ((r - g) / delta + 4)
        }
        return KeyTaoHSV(
            hue: normalizeHue(hue),
            saturation: maxChannel == 0 ? 0 : delta / maxChannel,
            value: maxChannel
        )
    }

    public static func hex(hue: CGFloat, saturation: CGFloat, value: CGFloat) -> String {
        let color = rgb(hue: hue, saturation: saturation, value: value)
        return String(format: "#%02X%02X%02X", color.red, color.green, color.blue)
    }

    public static func hsv(fromHex value: String) -> KeyTaoHSV? {
        var hex = value.trimmingCharacters(in: .whitespacesAndNewlines)
        if hex.hasPrefix("#") { hex.removeFirst() }
        guard hex.count == 6, hex.allSatisfy(\.isHexDigit), let packed = UInt32(hex, radix: 16) else { return nil }
        return hsv(
            red: Int((packed >> 16) & 0xFF),
            green: Int((packed >> 8) & 0xFF),
            blue: Int(packed & 0xFF)
        )
    }

    private static func channel(_ value: CGFloat) -> Int {
        min(max(Int((value * 255).rounded()), 0), 255)
    }
}
