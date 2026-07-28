import Foundation
import CKeytaoCore

/// One candidate as keytao-theme already laid it out: label, text and comment
/// are decided by the shared UI model, never by the AppKit renderer.
struct KeyTaoPanelCandidate: Decodable {
    var index: Int
    var label: String
    var text: String
    var comment: String?
    var selected: Bool
}

struct KeyTaoPanelNavigation: Decodable {
    var canGoPrevious: Bool
    var canGoNext: Bool

    static let empty = KeyTaoPanelNavigation(canGoPrevious: false, canGoNext: false)
}

struct KeyTaoPanelModel: Decodable {
    var preedit: String?
    var orientation: CandidatePanelOrientation
    var candidates: [KeyTaoPanelCandidate]
    var navigation: KeyTaoPanelNavigation

    static let empty = KeyTaoPanelModel(
        preedit: nil,
        orientation: .vertical,
        candidates: [],
        navigation: .empty
    )
}

struct KeyTaoModeHintModel: Decodable {
    var asciiMode: Bool
    var text: String

    static let empty = KeyTaoModeHintModel(asciiMode: false, text: "")
}

/// The shared IME state as keytao-core-ffi serializes it.
///
/// macOS reads the JSON flavour rather than the flat C struct so that the
/// candidate panel and the mode hint come from keytao-theme's models instead of
/// being recomputed here.
struct KeyTaoImeState: Decodable {
    var preedit: String
    /// Caret inside `preedit`, counted in Unicode scalars. Map it with
    /// `keytao_utf16_offset_from_chars` before handing it to IMKit.
    var cursor: Int
    /// Converted/selected span inside `preedit`, also in Unicode scalars.
    var selStart: Int
    var selEnd: Int
    var committed: String
    var asciiMode: Bool
    var accepted: Bool
    var candidatePanel: KeyTaoPanelModel
    var modeHint: KeyTaoModeHintModel

    var hasComposition: Bool {
        !preedit.isEmpty || !candidatePanel.candidates.isEmpty
    }

    static let empty = KeyTaoImeState(
        preedit: "",
        cursor: 0,
        selStart: 0,
        selEnd: 0,
        committed: "",
        asciiMode: false,
        accepted: false,
        candidatePanel: .empty,
        modeHint: .empty
    )

    enum CodingKeys: String, CodingKey {
        case preedit
        case cursor
        case selStart
        case selEnd
        case committed
        case asciiMode
        case accepted
        case candidatePanel
        case modeHint
    }

    // Decoded field by field so that a runtime which adds or drops a key
    // degrades that one field instead of failing the whole decode, which would
    // leave the controller without any state at all.
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let empty = Self.empty
        preedit = Self.decode(container, .preedit, empty.preedit)
        cursor = Self.decode(container, .cursor, empty.cursor)
        selStart = Self.decode(container, .selStart, empty.selStart)
        selEnd = Self.decode(container, .selEnd, empty.selEnd)
        committed = Self.decode(container, .committed, empty.committed)
        asciiMode = Self.decode(container, .asciiMode, empty.asciiMode)
        accepted = Self.decode(container, .accepted, empty.accepted)
        candidatePanel = Self.decode(container, .candidatePanel, empty.candidatePanel)
        modeHint = Self.decode(container, .modeHint, empty.modeHint)
    }

    init(
        preedit: String,
        cursor: Int,
        selStart: Int,
        selEnd: Int,
        committed: String,
        asciiMode: Bool,
        accepted: Bool,
        candidatePanel: KeyTaoPanelModel,
        modeHint: KeyTaoModeHintModel
    ) {
        self.preedit = preedit
        self.cursor = cursor
        self.selStart = selStart
        self.selEnd = selEnd
        self.committed = committed
        self.asciiMode = asciiMode
        self.accepted = accepted
        self.candidatePanel = candidatePanel
        self.modeHint = modeHint
    }

    private static func decode<Value: Decodable>(
        _ container: KeyedDecodingContainer<CodingKeys>,
        _ key: CodingKeys,
        _ fallback: Value
    ) -> Value {
        ((try? container.decodeIfPresent(Value.self, forKey: key)) ?? nil) ?? fallback
    }

    /// Decodes a string keytao-core-ffi handed over and frees it.
    static func consuming(_ json: UnsafeMutablePointer<CChar>?) -> KeyTaoImeState? {
        guard let json else { return nil }
        defer { keytao_free_string(json) }
        guard let data = String(cString: json).data(using: .utf8) else { return nil }
        do {
            return try JSONDecoder().decode(KeyTaoImeState.self, from: data)
        } catch {
            NSLog("KeyTao: failed to decode IME state JSON: %@", "\(error)")
            return nil
        }
    }
}

extension String {
    /// Converts a Unicode scalar offset from `ImeState` into the UTF-16 offset
    /// IMKit counts `NSRange` in. The conversion lives in keytao-core so that
    /// every frontend agrees on the unit.
    func keytaoUtf16Offset(fromCharacterOffset offset: Int) -> Int {
        guard offset > 0, !isEmpty else { return 0 }
        let converted = withCString { keytao_utf16_offset_from_chars($0, UInt32(clamping: offset)) }
        return Swift.min(Int(converted), utf16.count)
    }
}
