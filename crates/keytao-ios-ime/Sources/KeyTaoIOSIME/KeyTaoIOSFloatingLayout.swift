import Foundation

/// Which orientation a floating / one-handed keyboard profile belongs to.
///
/// `keytao-theme::mobile_layout` sanitises the shared layout with a different
/// floor per orientation (portrait `0.70`, landscape `0.45`), so iOS has to
/// accept the same ranges. Clamping both orientations to `0.70` would silently
/// widen a landscape keyboard that Android renders exactly as `keyboard.yaml`
/// asked for.
///
/// Everything in this file stays free of UIKit so that the decode and clamp
/// rules can be exercised by `Tests/FloatingLayoutTests` on the host toolchain.
public enum KeyTaoFloatingOrientation: String, Codable, Equatable {
    case portrait
    case landscape

    public init(isLandscape: Bool) {
        self = isLandscape ? .landscape : .portrait
    }

    public var minimumScale: CGFloat {
        switch self {
        case .portrait:
            return 0.70
        case .landscape:
            return 0.45
        }
    }

    public var maximumScale: CGFloat {
        1
    }
}

/// Deliberately `Encodable` only: the clamp depends on the orientation, which a
/// bare `Decoder` cannot supply. `KeyTaoFloatingKeyboardConfig` decodes the raw
/// numbers and runs them through this initialiser with the right orientation.
public struct KeyTaoFloatingKeyboardProfile: Encodable, Equatable {
    public var enabled: Bool
    public var scale: CGFloat

    public init(enabled: Bool, scale: CGFloat, orientation: KeyTaoFloatingOrientation) {
        self.enabled = enabled
        // keyboard.yaml may spell the scale as a percentage.
        let ratio = scale > 1.5 ? scale / 100 : scale
        self.scale = Swift.min(Swift.max(ratio, orientation.minimumScale), orientation.maximumScale)
    }
}

public struct KeyTaoFloatingKeyboardConfig: Codable, Equatable {
    /// Shared-layer defaults, mirroring `MobileFloatingLayout::default()`.
    public static let defaultMargin: CGFloat = 8
    public static let defaultPortraitScale: CGFloat = 0.88
    public static let defaultLandscapeScale: CGFloat = 0.62
    public static let defaultHeightScale: CGFloat = 1.0

    public var marginDp: CGFloat
    public var portrait: KeyTaoFloatingKeyboardProfile
    public var landscape: KeyTaoFloatingKeyboardProfile

    private enum CodingKeys: String, CodingKey {
        case margin
        case marginDp
        case portrait
        case landscape
    }

    public init(
        marginDp: CGFloat = KeyTaoFloatingKeyboardConfig.defaultMargin,
        portrait: KeyTaoFloatingKeyboardProfile = .init(
            enabled: false,
            scale: KeyTaoFloatingKeyboardConfig.defaultPortraitScale,
            orientation: .portrait
        ),
        landscape: KeyTaoFloatingKeyboardProfile = .init(
            enabled: true,
            scale: KeyTaoFloatingKeyboardConfig.defaultLandscapeScale,
            orientation: .landscape
        )
    ) {
        self.marginDp = Swift.min(Swift.max(marginDp, 0), 24)
        self.portrait = portrait
        self.landscape = landscape
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let margin = (try? container.decode(CGFloat.self, forKey: .margin))
            ?? (try? container.decode(CGFloat.self, forKey: .marginDp))
            ?? Self.defaultMargin
        let portrait = (try? container.decode(KeyTaoPartialFloatingProfile.self, forKey: .portrait))
        let landscape = (try? container.decode(KeyTaoPartialFloatingProfile.self, forKey: .landscape))
        self.init(
            marginDp: margin,
            portrait: KeyTaoFloatingKeyboardProfile(
                enabled: portrait?.enabled ?? false,
                scale: portrait?.scale ?? Self.defaultPortraitScale,
                orientation: .portrait
            ),
            landscape: KeyTaoFloatingKeyboardProfile(
                enabled: landscape?.enabled ?? true,
                scale: landscape?.scale ?? Self.defaultLandscapeScale,
                orientation: .landscape
            )
        )
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(marginDp, forKey: .margin)
        try container.encode(portrait, forKey: .portrait)
        try container.encode(landscape, forKey: .landscape)
    }

    public func profile(isLandscape: Bool) -> KeyTaoFloatingKeyboardProfile {
        isLandscape ? landscape : portrait
    }
}

struct KeyTaoPartialFloatingProfile: Decodable {
    var enabled: Bool?
    var scale: CGFloat?

    func applying(
        to profile: KeyTaoFloatingKeyboardProfile,
        orientation: KeyTaoFloatingOrientation
    ) -> KeyTaoFloatingKeyboardProfile {
        KeyTaoFloatingKeyboardProfile(
            enabled: enabled ?? profile.enabled,
            scale: scale ?? profile.scale,
            orientation: orientation
        )
    }
}

struct KeyTaoPartialFloatingConfig: Decodable {
    var margin: CGFloat?
    var marginDp: CGFloat?
    var portrait: KeyTaoPartialFloatingProfile?
    var landscape: KeyTaoPartialFloatingProfile?
}

enum KeyTaoIOSKeyboardSide: String, Equatable {
    case left
    case right

    var opposite: KeyTaoIOSKeyboardSide {
        self == .left ? .right : .left
    }
}

struct KeyTaoIOSKeyboardLayoutState: Equatable {
    /// Upper bound of the *interactive* resize only: past this the drag edge
    /// would sit under the screen edge and the keyboard could never be shrunk
    /// again. The lower bound follows the shared layout contract instead, which
    /// is orientation dependent.
    static let maximumScale: CGFloat = 0.94
    static let minimumHeightScale: CGFloat = 0.55
    static let maximumHeightScale: CGFloat = 1.15

    var enabled: Bool
    var scale: CGFloat
    var heightScale: CGFloat = KeyTaoFloatingKeyboardConfig.defaultHeightScale
    var side: KeyTaoIOSKeyboardSide = .right
    var orientation: KeyTaoFloatingOrientation = .portrait

    func normalized() -> KeyTaoIOSKeyboardLayoutState {
        KeyTaoIOSKeyboardLayoutState(
            enabled: enabled,
            scale: min(max(scale, orientation.minimumScale), Self.maximumScale),
            heightScale: min(max(heightScale, Self.minimumHeightScale), Self.maximumHeightScale),
            side: side,
            orientation: orientation
        )
    }
}
