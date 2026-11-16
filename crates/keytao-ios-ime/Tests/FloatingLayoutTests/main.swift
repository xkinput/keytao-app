import Foundation

// Decode tests for the floating / one-handed keyboard scale contract.
//
// `keytao-theme::mobile_layout` is the single source of truth for the mobile
// layout: it defaults landscape to 0.62 and sanitises portrait to 0.70..1.0 and
// landscape to 0.45..1.0. iOS decodes the very JSON that layer emits through
// `keytao_resolve_keyboard_json`, so the two clamps have to agree or the same
// keyboard.yaml renders differently on iOS than on Android.
//
// Run with `crates/keytao-ios-ime/test-floating-layout.sh`; the file under test
// is UIKit-free on purpose so this builds and runs on the host toolchain.

var failures: [String] = []

func expect(_ condition: Bool, _ message: String, line: Int = #line) {
    if !condition {
        failures.append("line \(line): \(message)")
    }
}

func expectScale(_ actual: CGFloat, _ expected: CGFloat, _ message: String, line: Int = #line) {
    if abs(actual - expected) > 0.0001 {
        failures.append("line \(line): \(message) (expected \(expected), got \(actual))")
    }
}

func decodeFloating(_ json: String, line: Int = #line) -> KeyTaoFloatingKeyboardConfig? {
    guard let data = json.data(using: .utf8) else {
        failures.append("line \(line): payload is not utf8")
        return nil
    }
    do {
        return try JSONDecoder().decode(KeyTaoFloatingKeyboardConfig.self, from: data)
    } catch {
        failures.append("line \(line): decode failed: \(error)")
        return nil
    }
}

// The `floating` object exactly as `MobileFloatingLayout::default()` serialises
// it (camelCase, keytao-theme/src/mobile_layout.rs).
let sharedDefaultJson = """
{"margin":8.0,"portrait":{"enabled":false,"scale":0.88},"landscape":{"enabled":true,"scale":0.62}}
"""

if let config = decodeFloating(sharedDefaultJson) {
    expectScale(config.marginDp, 8, "shared default margin must survive decoding")
    expect(!config.portrait.enabled, "shared default portrait floating is off")
    expectScale(config.portrait.scale, 0.88, "shared default portrait scale must survive decoding")
    expect(config.landscape.enabled, "shared default landscape floating is on")
    expectScale(
        config.landscape.scale,
        0.62,
        "shared default landscape scale must not be raised to the portrait floor"
    )
    expectScale(config.profile(isLandscape: true).scale, 0.62, "profile(isLandscape: true) picks landscape")
    expectScale(config.profile(isLandscape: false).scale, 0.88, "profile(isLandscape: false) picks portrait")
}

// Same fixture as keytao-theme's own sanitize test: both orientations asking
// for 0.42 must land on their own floor, not on a shared one.
if let config = decodeFloating(
    """
    {"margin":40,"portrait":{"enabled":true,"scale":0.42},"landscape":{"enabled":false,"scale":0.42}}
    """
) {
    expectScale(config.marginDp, 24, "margin is clamped to 24 like the shared layer")
    expectScale(config.portrait.scale, 0.70, "portrait floor is 0.70")
    expectScale(config.landscape.scale, 0.45, "landscape floor is 0.45")
}

if let config = decodeFloating(
    """
    {"portrait":{"scale":1.4},"landscape":{"scale":0.5}}
    """
) {
    expectScale(config.portrait.scale, 1.0, "scales are capped at 1.0")
    expectScale(config.landscape.scale, 0.5, "a landscape scale between the two floors is kept")
}

// keyboard.yaml is allowed to spell the scale as a percentage.
if let config = decodeFloating(
    """
    {"portrait":{"scale":88},"landscape":{"scale":62}}
    """
) {
    expectScale(config.portrait.scale, 0.88, "percentage portrait scale is normalised")
    expectScale(config.landscape.scale, 0.62, "percentage landscape scale is normalised")
}

// Missing keys fall back to the shared-layer defaults, not to iOS-only ones.
if let config = decodeFloating("{}") {
    expectScale(config.portrait.scale, KeyTaoFloatingKeyboardConfig.defaultPortraitScale, "portrait default")
    expectScale(config.landscape.scale, KeyTaoFloatingKeyboardConfig.defaultLandscapeScale, "landscape default")
    expectScale(config.landscape.scale, 0.62, "landscape default matches MobileFloatingLayout::default()")
}

// The persisted one-handed state runs through the same per-orientation floor,
// otherwise the store would silently widen a decoded landscape profile again.
let landscapeState = KeyTaoIOSKeyboardLayoutState(
    enabled: true,
    scale: 0.62,
    heightScale: 0.74,
    side: .right,
    orientation: .landscape
).normalized()
expectScale(landscapeState.scale, 0.62, "landscape layout state keeps 0.62")
expectScale(landscapeState.heightScale, 0.74, "landscape layout state keeps an independent height")

let landscapeTooSmall = KeyTaoIOSKeyboardLayoutState(
    enabled: true,
    scale: 0.20,
    heightScale: 0.20,
    side: .right,
    orientation: .landscape
).normalized()
expectScale(landscapeTooSmall.scale, 0.45, "landscape layout state floor is 0.45")
expectScale(
    landscapeTooSmall.heightScale,
    KeyTaoIOSKeyboardLayoutState.minimumHeightScale,
    "height uses its own floor"
)

let portraitState = KeyTaoIOSKeyboardLayoutState(
    enabled: true,
    scale: 0.62,
    heightScale: 1.25,
    side: .right,
    orientation: .portrait
).normalized()
expectScale(portraitState.scale, 0.70, "portrait layout state floor is 0.70")
expectScale(
    portraitState.heightScale,
    KeyTaoIOSKeyboardLayoutState.maximumHeightScale,
    "height uses its own ceiling"
)

let widestState = KeyTaoIOSKeyboardLayoutState(
    enabled: true,
    scale: 1.0,
    side: .right,
    orientation: .landscape
).normalized()
expectScale(
    widestState.scale,
    KeyTaoIOSKeyboardLayoutState.maximumScale,
    "the interactive resize stays below the screen edge"
)

let landscapeHeights = KeyTaoIOSKeyboardSizing.compactLandscapeHeights(
    screenHeight: 375, keyboardHeight: 266, candidateBarHeight: 52, rowCount: 4, verticalGap: 5
)
expectScale(landscapeHeights.keyboard + landscapeHeights.candidateBar, 206.25, "SE3 full keyboard uses 55% of the landscape screen")
let landscapeRowHeight = (landscapeHeights.keyboard - landscapeHeights.verticalGap * 5) / 4
expect(landscapeRowHeight >= 32, "SE3 letter rows remain tappable")
expect(landscapeHeights.candidateBar >= 32, "landscape toolbar remains tappable")

let numberRowHeights = KeyTaoIOSKeyboardSizing.compactLandscapeHeights(
    screenHeight: 375, keyboardHeight: 332.5, candidateBarHeight: 52, rowCount: 5, verticalGap: 5
)
expectScale((numberRowHeights.keyboard - numberRowHeights.verticalGap * 6) / 5, 32, "number row keeps the 32pt minimum")
expectScale(numberRowHeights.keyboard + numberRowHeights.candidateBar, 206.25, "SE3 number row respects the screen cap")

let alreadyShortHeights = KeyTaoIOSKeyboardSizing.compactLandscapeHeights(
    screenHeight: 375, keyboardHeight: 150, candidateBarHeight: 36, rowCount: 4, verticalGap: 4
)
expectScale(alreadyShortHeights.keyboard, 150, "already short keyboards keep their height")
expectScale(alreadyShortHeights.candidateBar, 36, "already short toolbars keep their height")

let shortNumberRowHeights = KeyTaoIOSKeyboardSizing.compactLandscapeHeights(
    screenHeight: 375, keyboardHeight: 170, candidateBarHeight: 36, rowCount: 5, verticalGap: 5
)
expectScale((shortNumberRowHeights.keyboard - shortNumberRowHeights.verticalGap * 6) / 5, 32, "small height settings cannot squeeze number keys below 32pt")

let largeGapHeights = KeyTaoIOSKeyboardSizing.compactLandscapeHeights(
    screenHeight: 375, keyboardHeight: 160, candidateBarHeight: 36, rowCount: 6, verticalGap: 24
)
expectScale(largeGapHeights.keyboard + largeGapHeights.candidateBar, 228, "large gaps cannot inflate the keyboard beyond the tappable row minimum")
expectScale(largeGapHeights.verticalGap, 0, "large gaps yield to tappable keys")

#if canImport(UIKit)
// Include KeyTaoIOSConfig.swift and its dependencies when running on iOS.
do {
    let decoder = JSONDecoder()
    let legacy = try decoder.decode(KeyTaoIOSImeConfig.self, from: Data("{}".utf8))
    expect(!legacy.mnemonicHintsEnabled, "legacy config keeps mnemonic hints off")
    expect(!KeyTaoIOSImeConfig.fallback.mnemonicHintsEnabled, "mnemonic hints default to off")
    expect(!legacy.symbolAutoReturn, "symbol auto return defaults to off")
    expect(legacy.phrases == ["谢谢", "好的", "收到", "稍等", "我在路上"], "phrases have shared defaults")
    let navigation = try decoder.decode(KeyTaoIOSImeConfig.self, from: Data("""
    {"rows":[[
      {"label":"返回","action":{"type":"keyboardMode","value":"back"}},
      {"label":"符号","action":{"type":"keyboardMode","value":"symbols:last"}},
      {"label":"Emoji","action":{"type":"keyboardMode","value":"emoji:last"}},
      {"label":"短语","action":{"type":"panel","value":"phrases"}},
      {"label":"剪贴板","action":{"type":"panel","value":"clipboard"}}
    ]],"layers":{"symbols_emoji_face":[[{"label":"😀","value":"😀"}]]}}
    """.utf8))
    expect(navigation.rows[0].compactMap { $0.action?.value } == ["back", "symbols:last", "emoji:last", "phrases", "clipboard"], "shared command markers survive decoding")
    expect(navigation.normalizedLayer("symbols:last").id == "symbols", "last symbols has a compatible initial target")
    expect(navigation.normalizedLayer("emoji:last").id == "symbols_emoji_face", "last emoji has a compatible initial target")
    expect(navigation.normalizedLayer("back").id == "letters", "back preserves the old return target until navigation owns the origin")
    expect(navigation.normalizedLayer("unknown:last").id == "letters", "unknown targets safely fall back")
    let malformed = try decoder.decode(KeyTaoIOSImeConfig.self, from: Data("""
    {"symbolAutoReturn":{},"phrases":[42]}
    """.utf8))
    expect(!malformed.symbolAutoReturn && malformed.phrases == legacy.phrases, "malformed keyboard settings retain defaults")
    let configURL = FileManager.default.temporaryDirectory.appendingPathComponent("mnemonic-\(UUID().uuidString).json")
    defer { try? FileManager.default.removeItem(at: configURL) }
    for enabled in [true, false] {
        var config = legacy
        config.mnemonicHintsEnabled = enabled
        config.symbolAutoReturn = enabled
        config.phrases = enabled ? ["自定义", "收到"] : []
        let data = try JSONEncoder().encode(config)
        let decoded = try decoder.decode(KeyTaoIOSImeConfig.self, from: data)
        expect(decoded.mnemonicHintsEnabled == enabled, "mnemonic hints survive a config round trip")
        expect(decoded.symbolAutoReturn == enabled, "symbol auto return survives a config round trip")
        expect(decoded.phrases == config.phrases, "phrases preserve order and intentional empty lists")
        try data.write(to: configURL, options: .atomic)
        let loaded = KeyTaoIOSImeConfig.load(resolvedKeyboardJson: "{}", userConfigURL: configURL)
        expect(loaded.mnemonicHintsEnabled == enabled, "runtime settings override the resolved layout default")
        expect(loaded.symbolAutoReturn == enabled && loaded.phrases == config.phrases, "shared runtime settings override layout defaults")
    }
    let keyboard = KeyTaoIOSImeConfig.load(resolvedKeyboardJson: """
    {"symbolAutoReturn":true,"phrases":["自定义"]}
    """, userConfigURL: nil)
    expect(keyboard.symbolAutoReturn && keyboard.phrases == ["自定义"], "resolved keyboard settings survive the shared parser")
    let malformedLayout = KeyTaoIOSImeConfig.load(resolvedKeyboardJson: """
    {"height":310,"rows":[[{"label":"custom","value":"c"}]],"symbolAutoReturn":{},"phrases":[42]}
    """, userConfigURL: nil)
    expect(malformedLayout.keyboardHeightDp == 310 && malformedLayout.rows[0][0].label == "custom", "malformed new preferences preserve valid resolved layout fields")
    expect(!malformedLayout.symbolAutoReturn && malformedLayout.phrases == legacy.phrases, "malformed resolved preferences individually fall back")
    try Data("""
    {"hapticsEnabled":false,"symbolAutoReturn":{},"phrases":[42]}
    """.utf8).write(to: configURL, options: .atomic)
    let malformedRuntime = KeyTaoIOSImeConfig.load(resolvedKeyboardJson: """
    {"height":310,"rows":[[{"label":"custom","value":"c"}]],"symbolAutoReturn":true,"phrases":["保留"]}
    """, userConfigURL: configURL)
    expect(!malformedRuntime.hapticsEnabled, "malformed new preferences preserve valid existing runtime settings")
    expect(malformedRuntime.symbolAutoReturn && malformedRuntime.phrases == ["保留"], "malformed runtime preferences retain resolved values")
    expect(malformedRuntime.keyboardHeightDp == 310 && malformedRuntime.rows[0][0].label == "custom", "malformed runtime preferences preserve layout rows")
    try Data("""
    {"symbolAutoReturn":true,"phrases":[42]}
    """.utf8).write(to: configURL, options: .atomic)
    let partialRuntime = KeyTaoIOSImeConfig.load(resolvedKeyboardJson: "{}", userConfigURL: configURL)
    expect(partialRuntime.symbolAutoReturn && partialRuntime.phrases == legacy.phrases, "one malformed preference does not discard another valid preference")
} catch {
    failures.append("keyboard config round trip failed: \(error)")
}
#else
print("keyboard command/settings round trips require UIKit; skipped on the host")
#endif

if failures.isEmpty {
    print("floating layout decode tests passed")
} else {
    for failure in failures {
        FileHandle.standardError.write(Data("FAIL \(failure)\n".utf8))
    }
    exit(1)
}
