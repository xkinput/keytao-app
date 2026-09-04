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

if failures.isEmpty {
    print("floating layout decode tests passed")
} else {
    for failure in failures {
        FileHandle.standardError.write(Data("FAIL \(failure)\n".utf8))
    }
    exit(1)
}
