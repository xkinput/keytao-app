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

    public var version: Int
    public var ui: Ui
    public var font: Font
    public var panel: Panel
    public var candidate: Candidate
    public var navigation: Navigation
    public var modeHint: ModeHint

    public static let fallback = KeyTaoImeTheme(
        version: 2,
        ui: Ui(colorScheme: "auto", effectiveColorScheme: "light", accentColor: nil),
        font: Font(family: nil, size: 18, labelSize: 14, commentSize: 13, preeditSize: 15, weight: .medium),
        panel: Panel(
            orientation: "horizontal",
            background: KeyTaoThemeColor(red: 0xF8, green: 0xFA, blue: 0xFF, alpha: 0xF2),
            borderColor: KeyTaoThemeColor(red: 0xD8, green: 0xE2, blue: 0xF1, alpha: 0xFF),
            borderWidth: 1,
            cornerRadius: 14,
            paddingX: 8,
            paddingY: 7,
            gap: 6,
            minWidth: 96,
            maxWidth: 820,
            maxHeight: 460,
            screenMargin: 8,
            shadow: true
        ),
        candidate: Candidate(
            background: KeyTaoThemeColor(red: 0, green: 0, blue: 0, alpha: 0),
            hoverBackground: KeyTaoThemeColor(red: 0xF1, green: 0xF6, blue: 0xFF, alpha: 0xFF),
            pressedBackground: KeyTaoThemeColor(red: 0xD4, green: 0xE7, blue: 0xFF, alpha: 0xFF),
            pressedForeground: KeyTaoThemeColor(red: 0x14, green: 0x23, blue: 0x3B, alpha: 0xFF),
            selectedBackground: KeyTaoThemeColor(red: 0xE6, green: 0xF0, blue: 0xFF, alpha: 0xFF),
            foreground: KeyTaoThemeColor(red: 0x1F, green: 0x29, blue: 0x33, alpha: 0xFF),
            selectedForeground: KeyTaoThemeColor(red: 0x14, green: 0x23, blue: 0x3B, alpha: 0xFF),
            labelColor: KeyTaoThemeColor(red: 0x6B, green: 0x77, blue: 0x85, alpha: 0xFF),
            selectedLabelColor: KeyTaoThemeColor(red: 0x3B, green: 0x73, blue: 0xD9, alpha: 0xFF),
            commentColor: KeyTaoThemeColor(red: 0x7A, green: 0x87, blue: 0x90, alpha: 0xFF),
            selectedCommentColor: KeyTaoThemeColor(red: 0x52, green: 0x6A, blue: 0x91, alpha: 0xFF),
            borderColor: KeyTaoThemeColor(red: 0, green: 0, blue: 0, alpha: 0),
            selectedBorderColor: KeyTaoThemeColor(red: 0xA8, green: 0xC7, blue: 0xFA, alpha: 0xFF),
            borderWidth: 0,
            cornerRadius: 9,
            paddingX: 11,
            paddingY: 6,
            inlineGap: 5,
            minHeight: 32,
            maxWidth: 210,
            separatorVisible: false,
            separatorColor: KeyTaoThemeColor(red: 0xDC, green: 0xE7, blue: 0xF7, alpha: 0xFF),
            labelSuffix: "."
        ),
        navigation: Navigation(
            foreground: KeyTaoThemeColor(red: 0x4A, green: 0x59, blue: 0x66, alpha: 0xFF),
            disabledForeground: KeyTaoThemeColor(red: 0xA5, green: 0xB0, blue: 0xB8, alpha: 0xFF),
            hoverBackground: KeyTaoThemeColor(red: 0xF1, green: 0xF6, blue: 0xFF, alpha: 0xFF),
            buttonSize: 28,
            cornerRadius: 8
        ),
        modeHint: ModeHint(
            background: KeyTaoThemeColor(red: 0xE6, green: 0xF0, blue: 0xFF, alpha: 0xF2),
            foreground: KeyTaoThemeColor(red: 0x2F, green: 0x5F, blue: 0xB8, alpha: 0xFF),
            fontSize: 24,
            width: 72,
            height: 48,
            cornerRadius: 14,
            duration: 0.75,
            shadow: true,
            chineseText: "中",
            englishText: "英"
        )
    )

    func withAccent(hex: String) -> KeyTaoImeTheme {
        guard let accent = KeyTaoThemeColor.from(hex: hex) else { return self }
        var next = self
        let dark = ui.effectiveColorScheme == "dark"
        next.ui.accentColor = accent
        next.candidate.selectedLabelColor = accent
        next.candidate.selectedBorderColor = accent
        next.candidate.selectedBackground = Self.mix(
            base: panel.background,
            accent: accent,
            weight: dark ? 0.42 : 0.18
        )
        next.candidate.pressedBackground = Self.mix(
            base: panel.background,
            accent: accent,
            weight: dark ? 0.54 : 0.28
        )
        next.candidate.hoverBackground = Self.mix(
            base: panel.background,
            accent: accent,
            weight: dark ? 0.22 : 0.09
        )
        return next
    }

    private static func mix(base: KeyTaoThemeColor, accent: KeyTaoThemeColor, weight: CGFloat) -> KeyTaoThemeColor {
        let ratio = max(0, min(weight, 1))
        func channel(_ baseValue: Int, _ accentValue: Int) -> Int {
            Int((CGFloat(baseValue) * (1 - ratio) + CGFloat(accentValue) * ratio).rounded())
        }
        return KeyTaoThemeColor(
            red: channel(base.red, accent.red),
            green: channel(base.green, accent.green),
            blue: channel(base.blue, accent.blue),
            alpha: 0xFF
        )
    }
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
        if let family {
            for name in family.split(separator: ",").map({ $0.trimmingCharacters(in: .whitespaces) }) {
                if let font = UIFont(name: name, size: size) {
                    return font
                }
            }
        }
        return .systemFont(ofSize: size, weight: weight.uiWeight)
    }
}

private extension Int {
    var clampedColor: Int {
        Swift.min(Swift.max(self, 0), 255)
    }
}
