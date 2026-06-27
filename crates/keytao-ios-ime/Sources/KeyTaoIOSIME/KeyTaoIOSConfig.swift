import Foundation
import UIKit
import CKeytaoCore

public enum KeyTaoCommandType {
    public static let input = "input"
    public static let directInput = "directInput"
    public static let rimeInput = "rimeInput"
    public static let backspace = "backspace"
    public static let backspaceGesture = "backspaceGesture"
    public static let enter = "enter"
    public static let space = "space"
    public static let shift = "shift"
    public static let mode = "mode"
    public static let openPage = "openPage"
    public static let keyboardPicker = "keyboardPicker"
    public static let keyboardMode = "keyboardMode"
    public static let nextCandidatePage = "nextCandidatePage"
    public static let previousCandidatePage = "previousCandidatePage"
    public static let reset = "reset"
    public static let rimeMenu = "rimeMenu"
    public static let edit = "edit"
    public static let panel = "panel"
}

public struct KeyTaoKeyCommand: Codable, Equatable {
    public var type: String
    public var value: String?
    public var fallbackValue: String?

    public static func input(_ value: String) -> KeyTaoKeyCommand {
        KeyTaoKeyCommand(type: KeyTaoCommandType.input, value: value, fallbackValue: nil)
    }

    public static func directInput(_ value: String) -> KeyTaoKeyCommand {
        KeyTaoKeyCommand(type: KeyTaoCommandType.directInput, value: value, fallbackValue: nil)
    }

    public static func edit(_ value: String, fallbackValue: String? = nil) -> KeyTaoKeyCommand {
        KeyTaoKeyCommand(type: KeyTaoCommandType.edit, value: value, fallbackValue: fallbackValue)
    }

    public static func panel(_ value: String) -> KeyTaoKeyCommand {
        KeyTaoKeyCommand(type: KeyTaoCommandType.panel, value: value, fallbackValue: nil)
    }
}

public struct KeyTaoKeyStackItem: Codable, Equatable {
    public var label: String
    public var value: String?
    public var asciiLabel: String?
    public var asciiValue: String?
    public var rimeValue: String?
    public var action: KeyTaoKeyCommand?
    public var asciiAction: KeyTaoKeyCommand?
}

public struct KeyTaoKeySpec: Codable, Equatable {
    public var label: String
    public var value: String?
    public var rimeValue: String?
    public var hint: String?
    public var weight: CGFloat?
    public var style: String?
    public var action: KeyTaoKeyCommand?
    public var swipeUp: KeyTaoKeyCommand?
    public var swipeDown: KeyTaoKeyCommand?
    public var longPress: KeyTaoKeyCommand?
    public var asciiLongPress: KeyTaoKeyCommand?
    public var asciiLabel: String?
    public var asciiValue: String?
    public var asciiAction: KeyTaoKeyCommand?
    public var rowSpan: CGFloat?
    public var stack: [KeyTaoKeyStackItem]?

    public init(
        label: String,
        value: String? = nil,
        rimeValue: String? = nil,
        hint: String? = nil,
        weight: CGFloat? = nil,
        style: String? = nil,
        action: KeyTaoKeyCommand? = nil,
        swipeUp: KeyTaoKeyCommand? = nil,
        swipeDown: KeyTaoKeyCommand? = nil,
        longPress: KeyTaoKeyCommand? = nil,
        asciiLongPress: KeyTaoKeyCommand? = nil,
        asciiLabel: String? = nil,
        asciiValue: String? = nil,
        asciiAction: KeyTaoKeyCommand? = nil,
        rowSpan: CGFloat? = nil,
        stack: [KeyTaoKeyStackItem]? = nil
    ) {
        self.label = label
        self.value = value
        self.rimeValue = rimeValue
        self.hint = hint
        self.weight = weight
        self.style = style
        self.action = action
        self.swipeUp = swipeUp
        self.swipeDown = swipeDown
        self.longPress = longPress
        self.asciiLongPress = asciiLongPress
        self.asciiLabel = asciiLabel
        self.asciiValue = asciiValue
        self.asciiAction = asciiAction
        self.rowSpan = rowSpan
        self.stack = stack
    }

    public func displayLabel(asciiMode: Bool, shiftState: KeyTaoShiftState) -> String {
        let base = asciiMode ? (asciiLabel ?? label) : label
        guard shiftState != .off, base.count == 1, base.range(of: "[a-z]", options: .regularExpression) != nil else {
            return base
        }
        return base.uppercased()
    }

    public func primaryCommand(asciiMode: Bool, shiftState: KeyTaoShiftState) -> KeyTaoKeyCommand {
        if asciiMode, let asciiAction {
            return asciiAction
        }
        if let action {
            return action
        }

        let baseValue = asciiMode ? (asciiValue ?? value ?? label) : (rimeValue ?? value ?? label)
        let value = shiftedValue(baseValue, shiftState: shiftState)
        if !asciiMode, let rimeValue {
            return KeyTaoKeyCommand(type: KeyTaoCommandType.rimeInput, value: rimeValue, fallbackValue: self.value ?? label)
        }
        return KeyTaoKeyCommand.input(value)
    }

    public func longPressCommand(asciiMode: Bool) -> KeyTaoKeyCommand? {
        if asciiMode, let asciiLongPress {
            return asciiLongPress
        }
        if let longPress {
            return longPress
        }
        if let hint, hint.count == 1 {
            return .input(hint)
        }
        return nil
    }

    public func swipeUpCommand(asciiMode: Bool) -> KeyTaoKeyCommand? {
        swipeUp ?? longPressCommand(asciiMode: asciiMode)
    }

    public func swipeDownCommand() -> KeyTaoKeyCommand? {
        swipeDown
    }

    private func shiftedValue(_ value: String, shiftState: KeyTaoShiftState) -> String {
        guard shiftState != .off, value.count == 1, value.range(of: "[a-z]", options: .regularExpression) != nil else {
            return value
        }
        return value.uppercased()
    }
}

public enum KeyTaoShiftState {
    case off
    case once
    case locked
}

public struct KeyTaoKeyboardLayer: Equatable {
    public var id: String

    public init(_ id: String) {
        self.id = id
    }

    public static let letters = KeyTaoKeyboardLayer("letters")
    public static let numbers = KeyTaoKeyboardLayer("numbers")
    public static let symbols = KeyTaoKeyboardLayer("symbols")
}

public struct KeyTaoIOSImeConfig: Codable, Equatable {
    public var keyboardHeightDp: CGFloat
    public var candidateBarHeightDp: CGFloat
    public var keyboardBottomInsetDp: CGFloat
    public var horizontalGapDp: CGFloat
    public var verticalGapDp: CGFloat
    public var outerInsetDp: CGFloat
    public var maxKeyHeightDp: CGFloat
    public var hapticsEnabled: Bool
    public var hapticIntensity: Int
    public var swipeThresholdDp: CGFloat
    public var rows: [[KeyTaoKeySpec]]
    public var numberRows: [[KeyTaoKeySpec]]
    public var symbolRows: [[KeyTaoKeySpec]]
    public var customRows: [String: [[KeyTaoKeySpec]]]

    private enum CodingKeys: String, CodingKey {
        case keyboardHeightDp
        case candidateBarHeightDp
        case keyboardBottomInsetDp
        case horizontalGapDp
        case verticalGapDp
        case outerInsetDp
        case maxKeyHeightDp
        case haptics
        case hapticsEnabled
        case hapticIntensity
        case swipeThresholdDp
        case rows
        case numberRows
        case symbolRows
        case layers
        case pages
        case keyboards
    }

    private enum HapticsCodingKeys: String, CodingKey {
        case enabled
        case intensity
    }

    public init(
        keyboardHeightDp: CGFloat,
        candidateBarHeightDp: CGFloat,
        keyboardBottomInsetDp: CGFloat,
        horizontalGapDp: CGFloat,
        verticalGapDp: CGFloat,
        outerInsetDp: CGFloat,
        maxKeyHeightDp: CGFloat,
        hapticsEnabled: Bool,
        hapticIntensity: Int,
        swipeThresholdDp: CGFloat,
        rows: [[KeyTaoKeySpec]],
        numberRows: [[KeyTaoKeySpec]],
        symbolRows: [[KeyTaoKeySpec]],
        customRows: [String: [[KeyTaoKeySpec]]] = [:]
    ) {
        self.keyboardHeightDp = keyboardHeightDp
        self.candidateBarHeightDp = candidateBarHeightDp
        self.keyboardBottomInsetDp = keyboardBottomInsetDp
        self.horizontalGapDp = Self.clamp(horizontalGapDp, min: 0, max: 24)
        self.verticalGapDp = Self.clamp(verticalGapDp, min: 0, max: 24)
        self.outerInsetDp = Self.clamp(outerInsetDp, min: 0, max: 32)
        self.maxKeyHeightDp = Self.clamp(maxKeyHeightDp, min: 36, max: 84)
        self.hapticsEnabled = hapticsEnabled
        self.hapticIntensity = hapticIntensity
        self.swipeThresholdDp = swipeThresholdDp
        self.rows = Self.normalizeRows(rows)
        self.numberRows = Self.normalizeNumberRows(numberRows)
        self.symbolRows = Self.normalizeRows(symbolRows)
        self.customRows = Self.normalizeCustomRows(customRows)
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let haptics = try? container.nestedContainer(keyedBy: HapticsCodingKeys.self, forKey: .haptics)
        self.keyboardHeightDp = Self.clamp(
            (try? container.decode(CGFloat.self, forKey: .keyboardHeightDp)) ?? Self.fallback.keyboardHeightDp,
            min: 160,
            max: 420
        )
        self.candidateBarHeightDp = Self.clamp(
            (try? container.decode(CGFloat.self, forKey: .candidateBarHeightDp)) ?? Self.fallback.candidateBarHeightDp,
            min: 36,
            max: 96
        )
        self.keyboardBottomInsetDp = Self.clamp(
            (try? container.decode(CGFloat.self, forKey: .keyboardBottomInsetDp)) ?? Self.fallback.keyboardBottomInsetDp,
            min: 0,
            max: 80
        )
        self.horizontalGapDp = Self.clamp(
            (try? container.decode(CGFloat.self, forKey: .horizontalGapDp)) ?? Self.fallback.horizontalGapDp,
            min: 0,
            max: 24
        )
        self.verticalGapDp = Self.clamp(
            (try? container.decode(CGFloat.self, forKey: .verticalGapDp)) ?? Self.fallback.verticalGapDp,
            min: 0,
            max: 24
        )
        self.outerInsetDp = Self.clamp(
            (try? container.decode(CGFloat.self, forKey: .outerInsetDp)) ?? Self.fallback.outerInsetDp,
            min: 0,
            max: 32
        )
        self.maxKeyHeightDp = Self.clamp(
            (try? container.decode(CGFloat.self, forKey: .maxKeyHeightDp)) ?? Self.fallback.maxKeyHeightDp,
            min: 36,
            max: 84
        )
        self.hapticsEnabled = (try? haptics?.decode(Bool.self, forKey: .enabled))
            ?? (try? container.decode(Bool.self, forKey: .hapticsEnabled))
            ?? Self.fallback.hapticsEnabled
        self.hapticIntensity = Self.clampInt(
            (try? haptics?.decode(Int.self, forKey: .intensity))
                ?? (try? container.decode(Int.self, forKey: .hapticIntensity))
                ?? Self.fallback.hapticIntensity,
            min: 1,
            max: 100
        )
        self.swipeThresholdDp = Self.clamp(
            (try? container.decode(CGFloat.self, forKey: .swipeThresholdDp)) ?? Self.fallback.swipeThresholdDp,
            min: 12,
            max: 96
        )
        self.rows = Self.normalizeRows(
            (try? container.decode([[KeyTaoKeySpec]].self, forKey: .rows)) ?? Self.fallback.rows
        )
        self.numberRows = Self.normalizeNumberRows(
            (try? container.decode([[KeyTaoKeySpec]].self, forKey: .numberRows)) ?? Self.fallback.numberRows
        )
        self.symbolRows = Self.normalizeRows(
            (try? container.decode([[KeyTaoKeySpec]].self, forKey: .symbolRows)) ?? Self.fallback.symbolRows
        )
        self.customRows = Self.normalizeCustomRows(
            Self.decodeLayerRows(from: container, forKey: .layers)
                ?? Self.decodeLayerRows(from: container, forKey: .pages)
                ?? Self.decodeLayerRows(from: container, forKey: .keyboards)
                ?? Self.fallback.customRows
        )
    }

    public static func load(
        resolvedKeyboardJson: String? = nil,
        userConfigURL: URL?,
        resolvedThemeJson: String? = nil
    ) -> KeyTaoIOSImeConfig {
        if let keyboardConfig = decodeKeyboard(json: resolvedKeyboardJson) {
            return keyboardConfig
        }
        if let userConfigURL, let config = decode(url: userConfigURL) {
            return config
        }
        if let themeConfig = decodeThemeKeyboard(json: resolvedThemeJson) {
            return themeConfig
        }
        for url in [bundledConfigURL()].compactMap({ $0 }) {
            if let config = decode(url: url) {
                return config
            }
        }
        return fallback
    }

    public func rows(for layer: KeyTaoKeyboardLayer) -> [[KeyTaoKeySpec]] {
        if layer == .letters {
            return rows
        }
        if layer == .numbers {
            return numberRows
        }
        if layer == .symbols {
            return symbolRows
        }
        return customRows[layer.id] ?? rows
    }

    public func hasLayer(_ layer: KeyTaoKeyboardLayer) -> Bool {
        layer == .letters || layer == .numbers || layer == .symbols || customRows[layer.id] != nil
    }

    public func normalizedLayer(_ value: String?) -> KeyTaoKeyboardLayer {
        let layer = KeyTaoKeyboardLayer(value?.isEmpty == false ? value! : KeyTaoKeyboardLayer.letters.id)
        return hasLayer(layer) ? layer : .letters
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(keyboardHeightDp, forKey: .keyboardHeightDp)
        try container.encode(candidateBarHeightDp, forKey: .candidateBarHeightDp)
        try container.encode(keyboardBottomInsetDp, forKey: .keyboardBottomInsetDp)
        try container.encode(horizontalGapDp, forKey: .horizontalGapDp)
        try container.encode(verticalGapDp, forKey: .verticalGapDp)
        try container.encode(outerInsetDp, forKey: .outerInsetDp)
        try container.encode(maxKeyHeightDp, forKey: .maxKeyHeightDp)
        var haptics = container.nestedContainer(keyedBy: HapticsCodingKeys.self, forKey: .haptics)
        try haptics.encode(hapticsEnabled, forKey: .enabled)
        try haptics.encode(hapticIntensity, forKey: .intensity)
        try container.encode(swipeThresholdDp, forKey: .swipeThresholdDp)
        try container.encode(rows, forKey: .rows)
        try container.encode(numberRows, forKey: .numberRows)
        try container.encode(symbolRows, forKey: .symbolRows)
        if !customRows.isEmpty {
            try container.encode(customRows, forKey: .layers)
        }
    }

    private static func decode(url: URL) -> KeyTaoIOSImeConfig? {
        guard let data = try? Data(contentsOf: url) else {
            return nil
        }
        return try? JSONDecoder().decode(KeyTaoIOSImeConfig.self, from: data)
    }

    private static func decodeThemeKeyboard(json: String?) -> KeyTaoIOSImeConfig? {
        guard let json,
              let data = json.data(using: .utf8),
              let payload = try? JSONDecoder().decode(KeyTaoThemeKeyboardRoot.self, from: data),
              let keyboard = payload.keyboard else {
            return nil
        }
        return KeyTaoIOSImeConfig(
            keyboardHeightDp: keyboard.height ?? Self.fallback.keyboardHeightDp,
            candidateBarHeightDp: keyboard.candidateBarHeight ?? Self.fallback.candidateBarHeightDp,
            keyboardBottomInsetDp: keyboard.bottomInset ?? Self.fallback.keyboardBottomInsetDp,
            horizontalGapDp: keyboard.horizontalGap ?? Self.fallback.horizontalGapDp,
            verticalGapDp: keyboard.verticalGap ?? Self.fallback.verticalGapDp,
            outerInsetDp: keyboard.outerInset ?? Self.fallback.outerInsetDp,
            maxKeyHeightDp: keyboard.maxKeyHeight ?? Self.fallback.maxKeyHeightDp,
            hapticsEnabled: Self.fallback.hapticsEnabled,
            hapticIntensity: Self.fallback.hapticIntensity,
            swipeThresholdDp: Self.fallback.swipeThresholdDp,
            rows: keyboard.rows ?? Self.fallback.rows,
            numberRows: keyboard.numberRows ?? Self.fallback.numberRows,
            symbolRows: keyboard.symbolRows ?? Self.fallback.symbolRows,
            customRows: keyboard.layerRows ?? Self.fallback.customRows
        )
    }

    private static func decodeKeyboard(json: String?) -> KeyTaoIOSImeConfig? {
        guard let json,
              let data = json.data(using: .utf8),
              let keyboard = try? JSONDecoder().decode(KeyTaoThemeKeyboard.self, from: data) else {
            return nil
        }
        return KeyTaoIOSImeConfig(
            keyboardHeightDp: keyboard.height ?? Self.fallback.keyboardHeightDp,
            candidateBarHeightDp: keyboard.candidateBarHeight ?? Self.fallback.candidateBarHeightDp,
            keyboardBottomInsetDp: keyboard.bottomInset ?? Self.fallback.keyboardBottomInsetDp,
            horizontalGapDp: keyboard.horizontalGap ?? Self.fallback.horizontalGapDp,
            verticalGapDp: keyboard.verticalGap ?? Self.fallback.verticalGapDp,
            outerInsetDp: keyboard.outerInset ?? Self.fallback.outerInsetDp,
            maxKeyHeightDp: keyboard.maxKeyHeight ?? Self.fallback.maxKeyHeightDp,
            hapticsEnabled: Self.fallback.hapticsEnabled,
            hapticIntensity: Self.fallback.hapticIntensity,
            swipeThresholdDp: Self.fallback.swipeThresholdDp,
            rows: keyboard.rows ?? Self.fallback.rows,
            numberRows: keyboard.numberRows ?? Self.fallback.numberRows,
            symbolRows: keyboard.symbolRows ?? Self.fallback.symbolRows,
            customRows: keyboard.layerRows ?? Self.fallback.customRows
        )
    }

    private static func bundledConfigURL() -> URL? {
        KeyTaoIOSBundle.url(forResource: "keytao_ios_ime", withExtension: "json")
    }

    private static func normalizeNumberRows(_ rows: [[KeyTaoKeySpec]]) -> [[KeyTaoKeySpec]] {
        normalizeRows(rows).map { row in
            row.map { key in
                guard key.label == "#+=",
                      key.action?.type == KeyTaoCommandType.input || key.action == nil else {
                    return key
                }
                var next = key
                next.value = nil
                next.action = KeyTaoKeyCommand(type: KeyTaoCommandType.keyboardMode, value: "symbols", fallbackValue: nil)
                return next
            }
        }
    }

    private static func normalizeRows(_ rows: [[KeyTaoKeySpec]]) -> [[KeyTaoKeySpec]] {
        rows.map { row in
            row.map { key in
                switch key.label {
                case "，":
                    return key.withAsciiVariant(label: ",", value: ",")
                case "。":
                    return key.withAsciiVariant(label: ".", value: ".")
                default:
                    return key
                }
            }
        }
    }

    private static func normalizeCustomRows(_ rows: [String: [[KeyTaoKeySpec]]]) -> [String: [[KeyTaoKeySpec]]] {
        rows.reduce(into: [:]) { result, entry in
            let name = entry.key.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !name.isEmpty,
                  name != KeyTaoKeyboardLayer.letters.id,
                  name != KeyTaoKeyboardLayer.numbers.id,
                  name != KeyTaoKeyboardLayer.symbols.id else {
                return
            }
            let normalized = normalizeRows(entry.value)
            if !normalized.isEmpty {
                result[name] = normalized
            }
        }
    }

    private static func decodeLayerRows(
        from container: KeyedDecodingContainer<CodingKeys>,
        forKey key: CodingKeys
    ) -> [String: [[KeyTaoKeySpec]]]? {
        guard let decoded = try? container.decode([String: KeyTaoLayerRows].self, forKey: key) else {
            return nil
        }
        return decoded.mapValues(\.rows)
    }

    private static func clamp(_ value: CGFloat, min minimum: CGFloat, max maximum: CGFloat) -> CGFloat {
        Swift.min(Swift.max(value, minimum), maximum)
    }

    private static func clampInt(_ value: Int, min minimum: Int, max maximum: Int) -> Int {
        Swift.min(Swift.max(value, minimum), maximum)
    }

    public static let fallback = KeyTaoIOSImeConfig(
        keyboardHeightDp: 266,
        candidateBarHeightDp: 52,
        keyboardBottomInsetDp: 0,
        horizontalGapDp: 4,
        verticalGapDp: 5,
        outerInsetDp: 5,
        maxKeyHeightDp: 54,
        hapticsEnabled: true,
        hapticIntensity: 42,
        swipeThresholdDp: 34,
        rows: [
            "qwertyuiop".map { KeyTaoKeySpec(label: String($0), value: String($0), rimeValue: nil, hint: nil, weight: nil, style: nil, action: nil, swipeUp: nil, swipeDown: nil, longPress: nil, asciiLongPress: nil, asciiLabel: nil, asciiValue: nil, asciiAction: nil) },
            "asdfghjkl".map { KeyTaoKeySpec(label: String($0), value: String($0), rimeValue: nil, hint: nil, weight: nil, style: nil, action: nil, swipeUp: nil, swipeDown: nil, longPress: nil, asciiLongPress: nil, asciiLabel: nil, asciiValue: nil, asciiAction: nil) },
            [
                KeyTaoKeySpec(label: "⇧", value: nil, rimeValue: nil, hint: nil, weight: 1.3, style: nil, action: KeyTaoKeyCommand(type: KeyTaoCommandType.shift, value: nil, fallbackValue: nil), swipeUp: nil, swipeDown: nil, longPress: nil, asciiLongPress: nil, asciiLabel: nil, asciiValue: nil, asciiAction: nil),
            ] + "zxcvbnm".map { KeyTaoKeySpec(label: String($0), value: String($0), rimeValue: nil, hint: nil, weight: nil, style: nil, action: nil, swipeUp: nil, swipeDown: nil, longPress: nil, asciiLongPress: nil, asciiLabel: nil, asciiValue: nil, asciiAction: nil) } + [
                KeyTaoKeySpec(label: "⌫", value: nil, rimeValue: nil, hint: nil, weight: 1.3, style: nil, action: KeyTaoKeyCommand(type: KeyTaoCommandType.backspace, value: nil, fallbackValue: nil), swipeUp: nil, swipeDown: nil, longPress: nil, asciiLongPress: nil, asciiLabel: nil, asciiValue: nil, asciiAction: nil),
            ],
            [
                KeyTaoKeySpec(label: "🌐", value: nil, rimeValue: nil, hint: nil, weight: 1.05, style: nil, action: KeyTaoKeyCommand(type: KeyTaoCommandType.keyboardPicker, value: nil, fallbackValue: nil), swipeUp: nil, swipeDown: nil, longPress: nil, asciiLongPress: nil, asciiLabel: nil, asciiValue: nil, asciiAction: nil),
                KeyTaoKeySpec(label: "123", value: nil, rimeValue: nil, hint: nil, weight: 1.15, style: nil, action: KeyTaoKeyCommand(type: KeyTaoCommandType.keyboardMode, value: "numbers", fallbackValue: nil), swipeUp: nil, swipeDown: nil, longPress: nil, asciiLongPress: nil, asciiLabel: nil, asciiValue: nil, asciiAction: nil),
                KeyTaoKeySpec(label: "空格", value: nil, rimeValue: nil, hint: nil, weight: 4.6, style: nil, action: KeyTaoKeyCommand(type: KeyTaoCommandType.space, value: nil, fallbackValue: nil), swipeUp: nil, swipeDown: nil, longPress: nil, asciiLongPress: nil, asciiLabel: nil, asciiValue: nil, asciiAction: nil),
                KeyTaoKeySpec(label: "↵", value: nil, rimeValue: nil, hint: nil, weight: 1.45, style: nil, action: KeyTaoKeyCommand(type: KeyTaoCommandType.enter, value: nil, fallbackValue: nil), swipeUp: nil, swipeDown: nil, longPress: nil, asciiLongPress: nil, asciiLabel: nil, asciiValue: nil, asciiAction: nil),
            ],
        ],
        numberRows: [
            [
                KeyTaoKeySpec(
                    label: "+",
                    value: "+",
                    rowSpan: 3,
                    stack: ["+", "*", "-", "/"].map { KeyTaoKeyStackItem(label: $0, value: $0) }
                ),
                KeyTaoKeySpec(label: "1", value: "1"),
                KeyTaoKeySpec(label: "2", value: "2"),
                KeyTaoKeySpec(label: "3", value: "3"),
                KeyTaoKeySpec(label: "⌫", action: KeyTaoKeyCommand(type: KeyTaoCommandType.backspace, value: nil, fallbackValue: nil)),
            ],
            [
                KeyTaoKeySpec(label: "4", value: "4"),
                KeyTaoKeySpec(label: "5", value: "5"),
                KeyTaoKeySpec(label: "6", value: "6"),
                KeyTaoKeySpec(label: "·", value: "."),
            ],
            [
                KeyTaoKeySpec(label: "7", value: "7"),
                KeyTaoKeySpec(label: "8", value: "8"),
                KeyTaoKeySpec(label: "9", value: "9"),
                KeyTaoKeySpec(label: "=", value: "="),
            ],
            [
                KeyTaoKeySpec(label: "返回", action: KeyTaoKeyCommand(type: KeyTaoCommandType.keyboardMode, value: "letters", fallbackValue: nil)),
                KeyTaoKeySpec(label: "#+=", action: KeyTaoKeyCommand(type: KeyTaoCommandType.keyboardMode, value: "symbols", fallbackValue: nil)),
                KeyTaoKeySpec(label: "0", value: "0"),
                KeyTaoKeySpec(label: "␣", action: KeyTaoKeyCommand(type: KeyTaoCommandType.space, value: nil, fallbackValue: nil)),
                KeyTaoKeySpec(label: "发送", action: KeyTaoKeyCommand(type: KeyTaoCommandType.enter, value: nil, fallbackValue: nil)),
            ],
        ],
        symbolRows: [],
        customRows: [:]
    )
}

enum KeyTaoIOSKeyboardConfigResolver {
    static func defaultKeyboardYaml() -> String? {
        guard let ptr = keytao_default_keyboard_yaml() else {
            return nil
        }
        defer { keytao_free_string(ptr) }
        let yaml = String(cString: ptr)
        return yaml.isEmpty ? nil : yaml
    }

    static func resolveJson(userKeyboardPath: String?) -> String? {
        let ptr: UnsafeMutablePointer<CChar>? = withOptionalCString(userKeyboardPath) { userPtr in
            keytao_resolve_keyboard_json(nil, userPtr)
        }
        guard let ptr else {
            return nil
        }
        defer { keytao_free_string(ptr) }
        let json = String(cString: ptr)
        return json.isEmpty ? nil : json
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

private struct KeyTaoThemeKeyboardRoot: Decodable {
    var keyboard: KeyTaoThemeKeyboard?
}

private struct KeyTaoThemeKeyboard: Decodable {
    var height: CGFloat?
    var candidateBarHeight: CGFloat?
    var bottomInset: CGFloat?
    var horizontalGap: CGFloat?
    var verticalGap: CGFloat?
    var outerInset: CGFloat?
    var maxKeyHeight: CGFloat?
    var rows: [[KeyTaoKeySpec]]?
    var numberRows: [[KeyTaoKeySpec]]?
    var symbolRows: [[KeyTaoKeySpec]]?
    var layers: [String: KeyTaoLayerRows]?

    var layerRows: [String: [[KeyTaoKeySpec]]]? {
        layers?.mapValues(\.rows)
    }
}

private struct KeyTaoLayerRows: Decodable {
    var rows: [[KeyTaoKeySpec]]

    private enum CodingKeys: String, CodingKey {
        case rows
    }

    init(from decoder: Decoder) throws {
        if let rows = try? [[KeyTaoKeySpec]](from: decoder) {
            self.rows = rows
            return
        }
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.rows = try container.decode([[KeyTaoKeySpec]].self, forKey: .rows)
    }
}

private extension KeyTaoKeySpec {
    func withAsciiVariant(label: String, value: String) -> KeyTaoKeySpec {
        if asciiLabel != nil || asciiValue != nil || asciiAction != nil {
            return self
        }
        var next = self
        next.asciiLabel = label
        next.asciiValue = value
        return next
    }
}
