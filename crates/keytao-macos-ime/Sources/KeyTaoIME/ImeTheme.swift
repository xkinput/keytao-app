import Cocoa
import CKeytaoCore

enum CandidatePanelOrientation: String, Codable {
    case horizontal
    case vertical
}

enum ThemeFontWeight: String, Codable {
    case ultraLight = "ultralight"
    case thin
    case light
    case regular
    case medium
    case semiBold = "semibold"
    case bold
    case heavy
    case black

    var nsWeight: NSFont.Weight {
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

enum ThemeColorScheme: String, Codable {
    case auto
    case light
    case dark
}

enum EffectiveThemeColorScheme: String, Codable {
    case light
    case dark
}

struct ThemeColor: Codable {
    var red: Int
    var green: Int
    var blue: Int
    var alpha: Int

    var nsColor: NSColor {
        NSColor(
            calibratedRed: CGFloat(red.clampedColor) / 255,
            green: CGFloat(green.clampedColor) / 255,
            blue: CGFloat(blue.clampedColor) / 255,
            alpha: CGFloat(alpha.clampedColor) / 255
        )
    }

    var cgColor: CGColor {
        (nsColor.usingColorSpace(.deviceRGB) ?? nsColor).cgColor
    }
}

struct ImeTheme: Codable {
    struct Ui: Codable {
        var colorScheme: ThemeColorScheme
        var effectiveColorScheme: EffectiveThemeColorScheme
        var accentColor: ThemeColor?
    }

    struct Font: Codable {
        var family: String?
        var size: CGFloat
        var labelSize: CGFloat
        var commentSize: CGFloat
        var preeditSize: CGFloat
        var weight: ThemeFontWeight
    }

    struct Panel: Codable {
        var orientation: CandidatePanelOrientation
        var background: ThemeColor
        var borderColor: ThemeColor
        var borderWidth: CGFloat
        var cornerRadius: CGFloat
        var paddingX: CGFloat
        var paddingY: CGFloat
        var gap: CGFloat
        var minWidth: CGFloat
        var maxWidth: CGFloat
        var maxHeight: CGFloat
        var screenMargin: CGFloat
        var shadow: Bool
    }

    struct CandidateOption: Codable {
        var background: ThemeColor
        var hoverBackground: ThemeColor
        var selectedBackground: ThemeColor
        var foreground: ThemeColor
        var selectedForeground: ThemeColor
        var labelColor: ThemeColor
        var selectedLabelColor: ThemeColor
        var commentColor: ThemeColor
        var selectedCommentColor: ThemeColor
        var borderColor: ThemeColor
        var selectedBorderColor: ThemeColor
        var borderWidth: CGFloat
        var cornerRadius: CGFloat
        var paddingX: CGFloat
        var paddingY: CGFloat
        var inlineGap: CGFloat
        var minHeight: CGFloat
        var maxWidth: CGFloat
        var separatorVisible: Bool
        var separatorColor: ThemeColor
        var labelSuffix: String
    }

    struct Navigation: Codable {
        var foreground: ThemeColor
        var disabledForeground: ThemeColor
        var hoverBackground: ThemeColor
        var buttonSize: CGFloat
        var cornerRadius: CGFloat
    }

    struct ModeHint: Codable {
        var background: ThemeColor
        var foreground: ThemeColor
        var borderColor: ThemeColor
        var borderWidth: CGFloat
        var fontSize: CGFloat
        var width: CGFloat
        var height: CGFloat
        var cornerRadius: CGFloat
        var duration: TimeInterval
        var shadow: Bool
        var chineseText: String
        var englishText: String
    }

    var version: Int
    var ui: Ui
    var font: Font
    var panel: Panel
    var candidate: CandidateOption
    var navigation: Navigation
    var modeHint: ModeHint

    static var `default`: ImeTheme {
        ImeTheme(
            version: 2,
            ui: Ui(
                colorScheme: .auto,
                effectiveColorScheme: .light,
                accentColor: nil
            ),
            font: Font(
                family: nil,
                size: 18,
                labelSize: 14,
                commentSize: 13,
                preeditSize: 15,
                weight: .medium
            ),
            panel: Panel(
                orientation: .horizontal,
                background: ThemeColor(red: 0xF8, green: 0xFA, blue: 0xFF, alpha: 0xF2),
                borderColor: ThemeColor(red: 0xD8, green: 0xE2, blue: 0xF1, alpha: 0xFF),
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
            candidate: CandidateOption(
                background: ThemeColor(red: 0, green: 0, blue: 0, alpha: 0),
                hoverBackground: ThemeColor(red: 0xF1, green: 0xF6, blue: 0xFF, alpha: 0xFF),
                selectedBackground: ThemeColor(red: 0xE6, green: 0xF0, blue: 0xFF, alpha: 0xFF),
                foreground: ThemeColor(red: 0x1F, green: 0x29, blue: 0x33, alpha: 0xFF),
                selectedForeground: ThemeColor(red: 0x14, green: 0x23, blue: 0x3B, alpha: 0xFF),
                labelColor: ThemeColor(red: 0x6B, green: 0x77, blue: 0x85, alpha: 0xFF),
                selectedLabelColor: ThemeColor(red: 0x3B, green: 0x73, blue: 0xD9, alpha: 0xFF),
                commentColor: ThemeColor(red: 0x7A, green: 0x87, blue: 0x90, alpha: 0xFF),
                selectedCommentColor: ThemeColor(red: 0x52, green: 0x6A, blue: 0x91, alpha: 0xFF),
                borderColor: ThemeColor(red: 0, green: 0, blue: 0, alpha: 0),
                selectedBorderColor: ThemeColor(red: 0xA8, green: 0xC7, blue: 0xFA, alpha: 0xFF),
                borderWidth: 0,
                cornerRadius: 9,
                paddingX: 11,
                paddingY: 6,
                inlineGap: 5,
                minHeight: 32,
                maxWidth: 210,
                separatorVisible: false,
                separatorColor: ThemeColor(red: 0xDC, green: 0xE7, blue: 0xF7, alpha: 0xFF),
                labelSuffix: "."
            ),
            navigation: Navigation(
                foreground: ThemeColor(red: 0x4A, green: 0x59, blue: 0x66, alpha: 0xFF),
                disabledForeground: ThemeColor(red: 0xA5, green: 0xB0, blue: 0xB8, alpha: 0xFF),
                hoverBackground: ThemeColor(red: 0xF1, green: 0xF6, blue: 0xFF, alpha: 0xFF),
                buttonSize: 28,
                cornerRadius: 8
            ),
            modeHint: ModeHint(
                background: ThemeColor(red: 0x2D, green: 0x4B, blue: 0x63, alpha: 0xFF),
                foreground: ThemeColor(red: 0xFF, green: 0xFF, blue: 0xFF, alpha: 0xFF),
                borderColor: ThemeColor(red: 0x5D, green: 0xA7, blue: 0xD7, alpha: 0xFF),
                borderWidth: 1,
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
    }
}

final class ImeThemeManager {
    static let shared = ImeThemeManager()

    private let lock = NSLock()
    private var cachedTheme = ImeTheme.default
    private var cachedSignature: String?

    func theme() -> ImeTheme {
        lock.lock()
        defer { lock.unlock() }

        let defaultURL = defaultThemeURL()
        let userURL = userThemeURL()
        let signatureParts = [defaultURL, userURL]
            .compactMap { $0 }
            .map { "\($0.path):\(fileSignature(for: $0))" }
        let signature = (signatureParts + ["system:\(systemAppearanceSignature())"])
            .joined(separator: "|")

        if signature == cachedSignature {
            return cachedTheme
        }

        let theme = loadTheme(defaultPath: defaultURL?.path, userPath: userURL?.path) ?? .default
        cachedSignature = signature
        cachedTheme = theme
        return theme
    }

    private func loadTheme(defaultPath: String?, userPath: String?) -> ImeTheme? {
        let json: String?
        if let defaultPath {
            json = defaultPath.withCString { defaultPtr in
                resolveThemeJSON(defaultThemePath: defaultPtr, userThemePath: userPath)
            }
        } else {
            json = resolveThemeJSON(defaultThemePath: nil, userThemePath: userPath)
        }

        guard let data = json?.data(using: .utf8) else {
            return nil
        }
        do {
            return try JSONDecoder().decode(ImeTheme.self, from: data)
        } catch {
            NSLog("KeyTao: failed to decode resolved theme JSON: %@", "\(error)")
            return nil
        }
    }

    private func resolveThemeJSON(defaultThemePath: UnsafePointer<CChar>?, userThemePath: String?) -> String? {
        let ptr: UnsafeMutablePointer<CChar>?
        if let userThemePath {
            ptr = userThemePath.withCString { userPtr in
                keytao_resolve_theme_json(defaultThemePath, userPtr)
            }
        } else {
            ptr = keytao_resolve_theme_json(defaultThemePath, nil)
        }

        guard let ptr else {
            return nil
        }
        defer { keytao_free_string(ptr) }
        return String(cString: ptr)
    }

    private func defaultThemeURL() -> URL? {
        if let url = Bundle.main.url(forResource: "default-theme", withExtension: "yaml") {
            return url
        }
        return Bundle.main.resourceURL?.appendingPathComponent("default-theme.yaml")
    }

    private func userThemeURL() -> URL? {
        let environment = ProcessInfo.processInfo.environment
        if let override = environment["KEYTAO_IME_THEME_PATH"], !override.isEmpty {
            return URL(fileURLWithPath: (override as NSString).expandingTildeInPath)
        }

        let home = FileManager.default.homeDirectoryForCurrentUser.path
        return URL(fileURLWithPath: (home as NSString).appendingPathComponent("Library/keytao/theme.yaml"))
    }

    private func fileSignature(for url: URL) -> String {
        guard let attrs = try? FileManager.default.attributesOfItem(atPath: url.path) else {
            return "missing"
        }
        let modified = (attrs[.modificationDate] as? Date)?.timeIntervalSince1970 ?? 0
        let size = (attrs[.size] as? NSNumber)?.int64Value ?? 0
        return "\(modified):\(size)"
    }

    private func systemAppearanceSignature() -> String {
        let environment = ProcessInfo.processInfo.environment
        if let override = environment["KEYTAO_IME_SYSTEM_COLOR_SCHEME"], !override.isEmpty {
            return override.lowercased()
        }
        return UserDefaults.standard.string(forKey: "AppleInterfaceStyle")?.lowercased() ?? "light"
    }
}

extension NSFont {
    static func keytaoThemeFont(family: String?, size: CGFloat, weight: ThemeFontWeight) -> NSFont {
        if let font = keytaoThemeFontFromFamily(family, size: size) {
            return font
        }
        return .systemFont(ofSize: size, weight: weight.nsWeight)
    }

    static func keytaoThemeFont(family: String?, size: CGFloat) -> NSFont {
        keytaoThemeFontFromFamily(family, size: size) ?? .systemFont(ofSize: size)
    }

    private static func keytaoThemeFontFromFamily(_ family: String?, size: CGFloat) -> NSFont? {
        guard let family else {
            return nil
        }
        for candidate in family.split(separator: ",").map({ $0.trimmingCharacters(in: .whitespaces) }) {
            if let font = NSFont(name: candidate, size: size) {
                return font
            }
        }
        return nil
    }
}

private extension Int {
    var clampedColor: Int {
        Swift.min(Swift.max(self, 0), 255)
    }
}
