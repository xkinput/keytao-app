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
    struct ShadowStyle: Codable {
        var color: ThemeColor
        var offsetY: CGFloat
        var blurRadius: CGFloat
        var margin: CGFloat
    }

    struct Ui: Codable {
        var colorScheme: ThemeColorScheme
        var effectiveColorScheme: EffectiveThemeColorScheme
        var accentColor: ThemeColor?
        var embeddedComposition: Bool
        var hideCandidatePanel: Bool

        var effectiveEmbeddedComposition: Bool {
            embeddedComposition || hideCandidatePanel
        }
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
        var preeditGap: CGFloat
        var minWidth: CGFloat
        var maxWidth: CGFloat
        var maxHeight: CGFloat
        var screenMargin: CGFloat
        var shadow: Bool
        var shadowStyle: ShadowStyle?
    }

    struct CandidateOption: Codable {
        var background: ThemeColor
        var hoverBackground: ThemeColor
        var pressedBackground: ThemeColor
        var pressedForeground: ThemeColor
        var preeditCaretColor: ThemeColor
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
        var shadowStyle: ShadowStyle?
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

    static let `default`: ImeTheme = {
        let json: String? = "light".withCString { schemePtr in
            guard let ptr = keytao_resolve_renderer_theme_json_with_system_scheme(nil, nil, schemePtr) else {
                return nil
            }
            defer { keytao_free_string(ptr) }
            return String(cString: ptr)
        }
        if let json, let theme = try? JSONDecoder().decode(ImeTheme.self, from: Data(json.utf8)) {
            return theme
        }
        return try! JSONDecoder().decode(ImeTheme.self, from: Data(fallbackJSON.utf8))
    }()

    private static let fallbackJSON = #"{"candidate":{"background":{"alpha":0,"blue":0,"green":0,"red":0},"borderColor":{"alpha":0,"blue":0,"green":0,"red":0},"borderWidth":1.0,"commentColor":{"alpha":255,"blue":94,"green":78,"red":63},"cornerRadius":11.0,"foreground":{"alpha":255,"blue":66,"green":52,"red":38},"hoverBackground":{"alpha":255,"blue":255,"green":246,"red":241},"inlineGap":4.0,"labelColor":{"alpha":255,"blue":156,"green":141,"red":127},"labelSuffix":".","maxWidth":190.0,"minHeight":34.0,"paddingX":10.0,"paddingY":5.0,"preeditCaretColor":{"alpha":89,"blue":66,"green":52,"red":38},"pressedBackground":{"alpha":255,"blue":255,"green":231,"red":212},"pressedForeground":{"alpha":255,"blue":59,"green":35,"red":20},"selectedBackground":{"alpha":255,"blue":255,"green":240,"red":230},"selectedBorderColor":{"alpha":255,"blue":215,"green":167,"red":93},"selectedCommentColor":{"alpha":255,"blue":80,"green":62,"red":46},"selectedForeground":{"alpha":255,"blue":65,"green":50,"red":36},"selectedLabelColor":{"alpha":255,"blue":246,"green":141,"red":74},"separatorColor":{"alpha":255,"blue":247,"green":231,"red":220},"separatorVisible":false},"font":{"commentSize":16.0,"family":"","labelSize":15.0,"preeditSize":15.0,"size":20.0,"weight":"semibold"},"mobile":{"candidateBar":{"background":{"alpha":0,"blue":0,"green":0,"red":0},"borderColor":{"alpha":89,"blue":246,"green":141,"red":74},"borderWidth":1.0,"commentColor":{"alpha":255,"blue":94,"green":78,"red":63},"cornerRadius":8.0,"expandedBackground":{"alpha":242,"blue":254,"green":242,"red":236},"fontScale":1.0,"foreground":{"alpha":255,"blue":66,"green":52,"red":38},"inlineGap":4.0,"itemBackground":{"alpha":0,"blue":0,"green":0,"red":0},"labelColor":{"alpha":255,"blue":156,"green":141,"red":127},"paddingX":9.0,"preeditBackground":{"alpha":0,"blue":0,"green":0,"red":0},"preeditForeground":{"alpha":255,"blue":66,"green":52,"red":38},"selectedBackground":{"alpha":255,"blue":255,"green":240,"red":230},"selectedBorderColor":{"alpha":255,"blue":215,"green":167,"red":93},"selectedBorderWidth":1.0,"selectedCommentColor":{"alpha":255,"blue":80,"green":62,"red":46},"selectedForeground":{"alpha":255,"blue":65,"green":50,"red":36},"selectedLabelColor":{"alpha":255,"blue":246,"green":141,"red":74},"showComment":true},"isDark":false,"keyboard":{"background":{"alpha":242,"blue":254,"green":242,"red":236},"floating":{"borderColor":{"alpha":28,"blue":44,"green":34,"red":26},"shadowColor":{"alpha":0,"blue":0,"green":0,"red":0}},"key":{"background":{"alpha":210,"blue":255,"green":255,"red":255},"borderColor":{"alpha":28,"blue":44,"green":34,"red":26},"borderWidth":1.0,"cornerRadius":10.0,"fontScale":1.0,"fontWeight":"regular","foreground":{"alpha":255,"blue":66,"green":52,"red":38},"hintFontScale":1.0,"hintForeground":{"alpha":255,"blue":94,"green":78,"red":63},"pressedBackground":{"alpha":255,"blue":255,"green":231,"red":212},"pressedForeground":{"alpha":255,"blue":59,"green":35,"red":20},"shadowColor":{"alpha":28,"blue":44,"green":34,"red":26},"shadowOffsetY":1.6,"swipeHintForeground":{"alpha":255,"blue":94,"green":78,"red":63},"swipeHintVisible":true},"keyClasses":{"accent":{"background":{"alpha":242,"blue":252,"green":220,"red":200},"borderColor":{"alpha":153,"blue":246,"green":141,"red":74},"borderWidth":1.0,"foreground":{"alpha":255,"blue":65,"green":50,"red":36},"pressedBackground":{"alpha":242,"blue":254,"green":228,"red":209}},"accentPunctuation":{"background":{"alpha":242,"blue":252,"green":220,"red":200},"borderColor":{"alpha":153,"blue":246,"green":141,"red":74},"borderWidth":1.0,"pressedBackground":{"alpha":242,"blue":254,"green":228,"red":209}}},"keyStates":{"selected":{"background":{"alpha":255,"blue":255,"green":240,"red":230},"foreground":{"alpha":255,"blue":65,"green":50,"red":36},"pressedBackground":{"alpha":255,"blue":255,"green":235,"red":221},"softBackground":{"alpha":255,"blue":255,"green":240,"red":230}},"shiftOn":{"background":{"alpha":210,"blue":255,"green":255,"red":255},"borderColor":{"alpha":255,"blue":215,"green":167,"red":93},"foreground":{"alpha":255,"blue":65,"green":50,"red":36},"pressedBackground":{"alpha":242,"blue":254,"green":228,"red":209}}},"keyTypes":{"backspace":{},"character":{},"enter":{},"function":{"background":{"alpha":242,"blue":252,"green":220,"red":200},"borderColor":{"alpha":153,"blue":246,"green":141,"red":74},"borderWidth":1.0,"pressedBackground":{"alpha":242,"blue":254,"green":228,"red":209}},"shift":{},"space":{}},"pageBackground":{"alpha":242,"blue":254,"green":242,"red":236},"toolbar":{"activeBackground":{"alpha":242,"blue":253,"green":229,"red":215},"activeBorderColor":{"alpha":153,"blue":246,"green":141,"red":74},"background":{"alpha":0,"blue":0,"green":0,"red":0},"buttonBackground":{"alpha":210,"blue":255,"green":255,"red":255},"buttonBorderColor":{"alpha":28,"blue":44,"green":34,"red":26},"buttonBorderWidth":1.0,"buttonCornerRadius":10.0,"foreground":{"alpha":255,"blue":66,"green":52,"red":38},"pressedBackground":{"alpha":242,"blue":252,"green":218,"red":197},"selectedBackground":{"alpha":242,"blue":253,"green":224,"red":207},"selectedForeground":{"alpha":255,"blue":65,"green":50,"red":36}}}},"modeHint":{"background":{"alpha":242,"blue":255,"green":250,"red":248},"borderColor":{"alpha":255,"blue":215,"green":167,"red":93},"borderWidth":1.0,"chineseText":"中","cornerRadius":14.0,"duration":0.75,"englishText":"英","fontSize":24.0,"foreground":{"alpha":255,"blue":66,"green":52,"red":38},"height":44.0,"shadow":true,"shadowStyle":{"blurRadius":16.0,"color":{"alpha":40,"blue":0,"green":0,"red":0},"margin":24.0,"offsetY":8.0},"width":72.0},"navigation":{"buttonSize":28.0,"cornerRadius":10.0,"disabledForeground":{"alpha":255,"blue":184,"green":176,"red":165},"foreground":{"alpha":255,"blue":132,"green":118,"red":104},"hoverBackground":{"alpha":255,"blue":255,"green":246,"red":241}},"panel":{"background":{"alpha":242,"blue":255,"green":250,"red":248},"borderColor":{"alpha":255,"blue":208,"green":195,"red":184},"borderWidth":1.0,"cornerRadius":16.0,"gap":4.0,"maxHeight":460.0,"maxWidth":320.0,"minWidth":128.0,"orientation":"horizontal","paddingX":10.0,"paddingY":10.0,"preeditGap":10.0,"screenMargin":8.0,"shadow":true,"shadowStyle":{"blurRadius":16.0,"color":{"alpha":40,"blue":0,"green":0,"red":0},"margin":24.0,"offsetY":8.0}},"ui":{"accentColor":null,"backgroundColor":null,"colorScheme":"auto","cornerRadius":null,"effectiveColorScheme":"light","effectiveEmbeddedComposition":true,"embeddedComposition":true,"hideCandidatePanel":false,"showTrayModeIndicator":true},"version":2}"#
}

extension ImeTheme.Ui {
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        colorScheme = try container.decode(ThemeColorScheme.self, forKey: .colorScheme)
        effectiveColorScheme = try container.decode(EffectiveThemeColorScheme.self, forKey: .effectiveColorScheme)
        accentColor = try container.decodeIfPresent(ThemeColor.self, forKey: .accentColor)
        embeddedComposition = try container.decode(Bool.self, forKey: .embeddedComposition)
        hideCandidatePanel = try container.decodeIfPresent(Bool.self, forKey: .hideCandidatePanel) ?? false
    }
}

final class ImeThemeManager {
    static let shared = ImeThemeManager()

    private let lock = NSLock()
    private var cachedTheme = ImeTheme.default
    private var cachedSignature: String?
    private var pushedColorScheme: EffectiveThemeColorScheme?

    /// Point the shared runtime at the same theme files this renderer reads, so
    /// the candidate panel model it builds for the JSON state uses the user's
    /// theme instead of the built-in defaults.
    func installThemeSources() {
        let defaultPath = defaultThemeURL()?.path
        let userPath = userThemeURL()?.path
        withOptionalCString(defaultPath) { defaultPtr in
            withOptionalCString(userPath) { userPtr in
                keytao_set_theme_paths(defaultPtr, userPtr)
            }
        }
        lock.lock()
        defer { lock.unlock() }
        pushSystemColorSchemeLocked(systemColorScheme())
    }

    func theme() -> ImeTheme {
        lock.lock()
        defer { lock.unlock() }

        let defaultURL = defaultThemeURL()
        let userURL = userThemeURL()
        let colorScheme = systemColorScheme()
        pushSystemColorSchemeLocked(colorScheme)
        let signatureParts = [defaultURL, userURL]
            .compactMap { $0 }
            .map { "\($0.path):\(fileSignature(for: $0))" }
        let signature = (signatureParts + ["system:\(colorScheme.rawValue)"])
            .joined(separator: "|")

        if signature == cachedSignature {
            return cachedTheme
        }

        let theme = loadTheme(
            defaultPath: defaultURL?.path,
            userPath: userURL?.path,
            systemColorScheme: colorScheme
        ) ?? .default
        cachedSignature = signature
        cachedTheme = theme
        return theme
    }

    /// Hand the appearance to the runtime instead of letting keytao-theme probe
    /// for it: probing means spawning `defaults` from inside the IME process.
    private func pushSystemColorSchemeLocked(_ scheme: EffectiveThemeColorScheme) {
        guard scheme != pushedColorScheme else { return }
        pushedColorScheme = scheme
        scheme.rawValue.withCString { keytao_set_system_color_scheme($0) }
    }

    private func loadTheme(
        defaultPath: String?,
        userPath: String?,
        systemColorScheme: EffectiveThemeColorScheme
    ) -> ImeTheme? {
        let json: String?
        if let defaultPath {
            json = defaultPath.withCString { defaultPtr in
                resolveThemeJSON(
                    defaultThemePath: defaultPtr,
                    userThemePath: userPath,
                    systemColorScheme: systemColorScheme.rawValue
                )
            }
        } else {
            json = resolveThemeJSON(
                defaultThemePath: nil,
                userThemePath: userPath,
                systemColorScheme: systemColorScheme.rawValue
            )
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

    private func resolveThemeJSON(
        defaultThemePath: UnsafePointer<CChar>?,
        userThemePath: String?,
        systemColorScheme: String
    ) -> String? {
        let ptr: UnsafeMutablePointer<CChar>? = withOptionalCString(userThemePath) { userPtr in
            systemColorScheme.withCString { schemePtr in
                keytao_resolve_renderer_theme_json_with_system_scheme(defaultThemePath, userPtr, schemePtr)
            }
        }

        guard let ptr else {
            return nil
        }
        defer { keytao_free_string(ptr) }
        return String(cString: ptr)
    }

    private func withOptionalCString<Result>(
        _ value: String?,
        _ body: (UnsafePointer<CChar>?) -> Result
    ) -> Result {
        if let value {
            return value.withCString { body($0) }
        }
        return body(nil)
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

    private func systemColorScheme() -> EffectiveThemeColorScheme {
        let environment = ProcessInfo.processInfo.environment
        if let override = environment["KEYTAO_IME_SYSTEM_COLOR_SCHEME"]?.lowercased() {
            if override == "dark" || override == "night" {
                return .dark
            }
            if override == "light" || override == "day" {
                return .light
            }
        }
        let appearance = NSApp.effectiveAppearance
        return appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? .dark : .light
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
