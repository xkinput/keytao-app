import Foundation
import UIKit
import CKeytaoCore

public enum KeyTaoThemeFontWeight: String, Codable {
    case ultraLight = "ultralight"
    case thin
    case light
    case regular
    case medium
    case semiBold = "semibold"
    case bold
    case heavy
    case black

    var uiWeight: UIFont.Weight {
        switch self {
        case .ultraLight:
            return .ultraLight
        case .thin:
            return .thin
        case .light:
            return .light
        case .regular:
            return .regular
        case .medium:
            return .medium
        case .semiBold:
            return .semibold
        case .bold:
            return .bold
        case .heavy:
            return .heavy
        case .black:
            return .black
        }
    }
}

public struct KeyTaoThemeColor: Codable, Equatable {
    public var red: Int
    public var green: Int
    public var blue: Int
    public var alpha: Int

    public var uiColor: UIColor {
        UIColor(
            red: CGFloat(red.clampedColor) / 255,
            green: CGFloat(green.clampedColor) / 255,
            blue: CGFloat(blue.clampedColor) / 255,
            alpha: CGFloat(alpha.clampedColor) / 255
        )
    }

    var hex: String {
        String(format: "#%02X%02X%02X", red.clampedColor, green.clampedColor, blue.clampedColor)
    }

    static func from(hex value: String) -> KeyTaoThemeColor? {
        let hex = value.trimmingCharacters(in: .whitespacesAndNewlines).replacingOccurrences(of: "#", with: "")
        guard hex.count == 6, let number = Int(hex, radix: 16) else { return nil }
        return KeyTaoThemeColor(
            red: (number >> 16) & 0xFF,
            green: (number >> 8) & 0xFF,
            blue: number & 0xFF,
            alpha: 0xFF
        )
    }
}

public struct KeyTaoImeTheme: Codable {
    public struct Ui: Codable {
        public var colorScheme: String
        public var effectiveColorScheme: String
        public var accentColor: KeyTaoThemeColor?
    }

    public struct Font: Codable {
        public var family: String?
        public var effectiveFamily: String?
        public var size: CGFloat
        public var labelSize: CGFloat
        public var commentSize: CGFloat
        public var preeditSize: CGFloat
        public var weight: KeyTaoThemeFontWeight
    }

    public struct Panel: Codable {
        public var orientation: String
        public var background: KeyTaoThemeColor
        public var borderColor: KeyTaoThemeColor
        public var borderWidth: CGFloat
        public var cornerRadius: CGFloat
        public var paddingX: CGFloat
        public var paddingY: CGFloat
        public var gap: CGFloat
        public var minWidth: CGFloat
        public var maxWidth: CGFloat
        public var maxHeight: CGFloat
        public var screenMargin: CGFloat
        public var shadow: Bool
    }

    public struct Candidate: Codable {
        public var background: KeyTaoThemeColor
        public var hoverBackground: KeyTaoThemeColor
        public var pressedBackground: KeyTaoThemeColor
        public var pressedForeground: KeyTaoThemeColor
        public var selectedBackground: KeyTaoThemeColor
        public var foreground: KeyTaoThemeColor
        public var selectedForeground: KeyTaoThemeColor
        public var labelColor: KeyTaoThemeColor
        public var selectedLabelColor: KeyTaoThemeColor
        public var commentColor: KeyTaoThemeColor
        public var selectedCommentColor: KeyTaoThemeColor
        public var borderColor: KeyTaoThemeColor
        public var selectedBorderColor: KeyTaoThemeColor
        public var borderWidth: CGFloat
        public var cornerRadius: CGFloat
        public var paddingX: CGFloat
        public var paddingY: CGFloat
        public var inlineGap: CGFloat
        public var minHeight: CGFloat
        public var maxWidth: CGFloat
        public var separatorVisible: Bool
        public var separatorColor: KeyTaoThemeColor
        public var labelSuffix: String
    }

    public struct Navigation: Codable {
        public var foreground: KeyTaoThemeColor
        public var disabledForeground: KeyTaoThemeColor
        public var hoverBackground: KeyTaoThemeColor
        public var buttonSize: CGFloat
        public var cornerRadius: CGFloat
    }

    public struct ModeHint: Codable {
        public var background: KeyTaoThemeColor
        public var foreground: KeyTaoThemeColor
        public var fontSize: CGFloat
        public var width: CGFloat
        public var height: CGFloat
        public var cornerRadius: CGFloat
        public var duration: TimeInterval
        public var shadow: Bool
        public var chineseText: String
        public var englishText: String
    }

    public struct KeyStyle: Codable {
        public var background: KeyTaoThemeColor
        public var foreground: KeyTaoThemeColor
        public var hintForeground: KeyTaoThemeColor
        public var pressedBackground: KeyTaoThemeColor
        public var pressedForeground: KeyTaoThemeColor
        public var borderColor: KeyTaoThemeColor
        public var borderWidth: CGFloat
        public var cornerRadius: CGFloat
        public var shadowColor: KeyTaoThemeColor
        public var shadowOffsetY: CGFloat
        public var fontScale: CGFloat
        public var hintFontScale: CGFloat
        public var fontWeight: KeyTaoThemeFontWeight
        public var swipeHintForeground: KeyTaoThemeColor
        public var swipeHintVisible: Bool

        func applying(_ overlay: KeyStyleOverride?) -> KeyStyle {
            guard let overlay else { return self }
            var style = self
            style.background = overlay.background ?? background
            style.foreground = overlay.foreground ?? foreground
            style.hintForeground = overlay.hintForeground ?? hintForeground
            style.pressedBackground = overlay.pressedBackground ?? pressedBackground
            style.pressedForeground = overlay.pressedForeground ?? pressedForeground
            style.borderColor = overlay.borderColor ?? borderColor
            style.borderWidth = overlay.borderWidth ?? borderWidth
            style.cornerRadius = overlay.cornerRadius ?? cornerRadius
            style.shadowColor = overlay.shadowColor ?? shadowColor
            style.shadowOffsetY = overlay.shadowOffsetY ?? shadowOffsetY
            style.fontScale = overlay.fontScale ?? fontScale
            style.hintFontScale = overlay.hintFontScale ?? hintFontScale
            style.fontWeight = overlay.fontWeight ?? fontWeight
            return style
        }
    }

    public struct KeyStyleOverride: Codable {
        public var background: KeyTaoThemeColor?
        public var foreground: KeyTaoThemeColor?
        public var hintForeground: KeyTaoThemeColor?
        public var pressedBackground: KeyTaoThemeColor?
        public var pressedForeground: KeyTaoThemeColor?
        public var borderColor: KeyTaoThemeColor?
        public var borderWidth: CGFloat?
        public var cornerRadius: CGFloat?
        public var shadowColor: KeyTaoThemeColor?
        public var shadowOffsetY: CGFloat?
        public var fontScale: CGFloat?
        public var hintFontScale: CGFloat?
        public var fontWeight: KeyTaoThemeFontWeight?
        public var softBackground: KeyTaoThemeColor?
    }

    public struct Mobile: Codable {
        public struct Keyboard: Codable {
            public struct Toolbar: Codable {
                public var background: KeyTaoThemeColor
                public var foreground: KeyTaoThemeColor
                public var selectedForeground: KeyTaoThemeColor
                public var buttonBackground: KeyTaoThemeColor
                public var buttonBorderColor: KeyTaoThemeColor
                public var activeBackground: KeyTaoThemeColor
                public var selectedBackground: KeyTaoThemeColor
                public var pressedBackground: KeyTaoThemeColor
                public var activeBorderColor: KeyTaoThemeColor
                public var buttonBorderWidth: CGFloat
                public var buttonCornerRadius: CGFloat
            }

            public struct Floating: Codable {
                public var borderColor: KeyTaoThemeColor
                public var shadowColor: KeyTaoThemeColor
            }

            public var background: KeyTaoThemeColor
            public var pageBackground: KeyTaoThemeColor
            public var key: KeyStyle
            public var keyTypes: [String: KeyStyleOverride]
            public var keyClasses: [String: KeyStyleOverride]
            public var keyStates: [String: KeyStyleOverride]
            public var toolbar: Toolbar
            public var floating: Floating
        }

        public struct CandidateBar: Codable {
            public var background: KeyTaoThemeColor
            public var itemBackground: KeyTaoThemeColor
            public var foreground: KeyTaoThemeColor
            public var selectedForeground: KeyTaoThemeColor
            public var selectedBackground: KeyTaoThemeColor
            public var commentColor: KeyTaoThemeColor
            public var selectedCommentColor: KeyTaoThemeColor
            public var labelColor: KeyTaoThemeColor
            public var selectedLabelColor: KeyTaoThemeColor
            public var borderColor: KeyTaoThemeColor
            public var selectedBorderColor: KeyTaoThemeColor
            public var borderWidth: CGFloat
            public var selectedBorderWidth: CGFloat
            public var cornerRadius: CGFloat
            public var paddingX: CGFloat
            public var inlineGap: CGFloat
            public var fontScale: CGFloat
            public var commentFontSize: CGFloat?
            public var showComment: Bool
            public var commentPosition: String?
            public var preeditForeground: KeyTaoThemeColor
            public var preeditBackground: KeyTaoThemeColor
            public var expandedBackground: KeyTaoThemeColor
        }

        public var isDark: Bool
        public var keyboard: Keyboard
        public var candidateBar: CandidateBar
    }

    public var version: Int
    public var ui: Ui
    public var font: Font
    public var panel: Panel
    public var candidate: Candidate
    public var navigation: Navigation
    public var modeHint: ModeHint
    public var mobile: Mobile

    public static let fallback: KeyTaoImeTheme = {
        if let theme = KeyTaoIOSThemeResolver.decode(
            json: KeyTaoIOSThemeResolver.resolveJson(userThemePath: nil, systemColorScheme: .light)
        ) {
            return theme
        }
        let url = KeyTaoIOSBundle.url(forResource: "default-theme-light", withExtension: "json")!
        return try! JSONDecoder().decode(KeyTaoImeTheme.self, from: Data(contentsOf: url))
    }()
}

public enum KeyTaoEffectiveColorScheme: String {
    case light
    case dark
}

enum KeyTaoIOSThemeResolver {
    static func resolveJson(
        userThemePath: String?,
        systemColorScheme: KeyTaoEffectiveColorScheme?
    ) -> String? {
        let ptr: UnsafeMutablePointer<CChar>? = withOptionalCString(userThemePath) { userPtr in
            withOptionalCString(systemColorScheme?.rawValue) { schemePtr in
                keytao_resolve_theme_json_with_system_scheme(nil, userPtr, schemePtr)
            }
        }
        guard let ptr else {
            return nil
        }
        defer { keytao_free_string(ptr) }
        return String(cString: ptr)
    }

    static func decode(json: String?) -> KeyTaoImeTheme? {
        guard let json, let data = json.data(using: .utf8) else {
            return nil
        }
        return try? JSONDecoder().decode(KeyTaoImeTheme.self, from: data)
    }

    static func resolve(
        userThemePath: String?,
        systemColorScheme: KeyTaoEffectiveColorScheme?
    ) -> KeyTaoImeTheme {
        decode(json: resolveJson(userThemePath: userThemePath, systemColorScheme: systemColorScheme)) ?? .fallback
    }

    private static func withOptionalCString<Result>(
        _ value: String?,
        _ body: (UnsafePointer<CChar>?) -> Result
    ) -> Result {
        if let value {
            return value.withCString { body($0) }
        }
        return body(nil)
    }
}

extension UIFont {
    static func keytaoThemeFont(family: String?, size: CGFloat, weight: KeyTaoThemeFontWeight) -> UIFont {
        if let family, !family.isEmpty {
            let descriptor = UIFontDescriptor(fontAttributes: [
                .family: family,
                .traits: [UIFontDescriptor.TraitKey.weight: weight.uiWeight.rawValue],
            ])
            return UIFont(descriptor: descriptor, size: size)
        }
        return .systemFont(ofSize: size, weight: weight.uiWeight)
    }
}

private extension Int {
    var clampedColor: Int {
        Swift.min(Swift.max(self, 0), 255)
    }
}
