import Foundation

public struct KeyTaoCandidate: Codable, Equatable {
    public var text: String
    public var comment: String?
    public var index: Int?

    public init(text: String, comment: String?, index: Int? = nil) {
        self.text = text
        self.comment = comment
        self.index = index
    }
}

public struct KeyTaoPanelCandidate: Codable, Equatable {
    public var index: Int
    public var label: String
    public var text: String
    public var comment: String?
    public var selected: Bool
}

public struct KeyTaoPanelNavigation: Codable, Equatable {
    public var canGoPrevious: Bool
    public var canGoNext: Bool
}

public struct KeyTaoPanelModel: Codable, Equatable {
    public var preedit: String?
    public var candidates: [KeyTaoPanelCandidate]
    public var navigation: KeyTaoPanelNavigation

    public static let empty = KeyTaoPanelModel(
        preedit: nil,
        candidates: [],
        navigation: KeyTaoPanelNavigation(canGoPrevious: false, canGoNext: false)
    )
}

public struct KeyTaoModeHintModel: Codable, Equatable {
    public var asciiMode: Bool
    public var text: String

    public static let empty = KeyTaoModeHintModel(asciiMode: false, text: "")
}

public struct KeyTaoImeState: Codable, Equatable {
    public var preedit: String
    /// Caret inside `preedit`, in Unicode scalars. Convert with
    /// `keytao_utf16_offset_from_chars` before handing it to UIKit.
    public var cursor: Int
    /// Converted/selected span inside `preedit`, also in Unicode scalars.
    public var selStart: Int
    public var selEnd: Int
    public var candidates: [KeyTaoCandidate]
    public var highlightedCandidateIndex: Int
    public var pageSize: Int
    public var page: Int
    public var isLastPage: Bool
    public var committed: String
    public var selectKeys: String
    public var asciiMode: Bool
    public var schemaName: String
    public var accepted: Bool
    public var candidatePanel: KeyTaoPanelModel
    public var modeHint: KeyTaoModeHintModel

    public var hasComposition: Bool {
        !preedit.isEmpty || !candidates.isEmpty || !candidatePanel.candidates.isEmpty
    }

    public func withoutTransientCommit() -> KeyTaoImeState {
        var next = self
        next.committed = ""
        next.accepted = false
        return next
    }

    public static let empty = KeyTaoImeState(
        preedit: "",
        cursor: 0,
        selStart: 0,
        selEnd: 0,
        candidates: [],
        highlightedCandidateIndex: 0,
        pageSize: 0,
        page: 0,
        isLastPage: true,
        committed: "",
        selectKeys: "",
        asciiMode: false,
        schemaName: "",
        accepted: false,
        candidatePanel: .empty,
        modeHint: .empty
    )

    public enum CodingKeys: String, CodingKey {
        case preedit
        case cursor
        case selStart
        case selEnd
        case candidates
        case highlightedCandidateIndex
        case pageSize
        case page
        case isLastPage
        case committed
        case selectKeys
        case asciiMode
        case schemaName
        case accepted
        case candidatePanel
        case modeHint
    }

    // Decoded field by field so that a runtime that adds or drops a key in the
    // shared state JSON degrades that one field instead of failing the whole
    // decode, which would leave the keyboard without any state at all.
    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let empty = Self.empty
        preedit = Self.field(container, .preedit, empty.preedit)
        cursor = Self.field(container, .cursor, empty.cursor)
        selStart = Self.field(container, .selStart, empty.selStart)
        selEnd = Self.field(container, .selEnd, empty.selEnd)
        candidates = Self.field(container, .candidates, empty.candidates)
        highlightedCandidateIndex = Self.field(container, .highlightedCandidateIndex, empty.highlightedCandidateIndex)
        pageSize = Self.field(container, .pageSize, empty.pageSize)
        page = Self.field(container, .page, empty.page)
        isLastPage = Self.field(container, .isLastPage, empty.isLastPage)
        committed = Self.field(container, .committed, empty.committed)
        selectKeys = Self.field(container, .selectKeys, empty.selectKeys)
        asciiMode = Self.field(container, .asciiMode, empty.asciiMode)
        schemaName = Self.field(container, .schemaName, empty.schemaName)
        accepted = Self.field(container, .accepted, empty.accepted)
        candidatePanel = Self.field(container, .candidatePanel, empty.candidatePanel)
        modeHint = Self.field(container, .modeHint, empty.modeHint)
    }

    private static func field<Value: Decodable>(
        _ container: KeyedDecodingContainer<CodingKeys>,
        _ key: CodingKeys,
        _ fallback: Value
    ) -> Value {
        ((try? container.decodeIfPresent(Value.self, forKey: key)) ?? nil) ?? fallback
    }

    public init(
        preedit: String,
        cursor: Int,
        selStart: Int,
        selEnd: Int,
        candidates: [KeyTaoCandidate],
        highlightedCandidateIndex: Int,
        pageSize: Int,
        page: Int,
        isLastPage: Bool,
        committed: String,
        selectKeys: String,
        asciiMode: Bool,
        schemaName: String,
        accepted: Bool,
        candidatePanel: KeyTaoPanelModel,
        modeHint: KeyTaoModeHintModel
    ) {
        self.preedit = preedit
        self.cursor = cursor
        self.selStart = selStart
        self.selEnd = selEnd
        self.candidates = candidates
        self.highlightedCandidateIndex = highlightedCandidateIndex
        self.pageSize = pageSize
        self.page = page
        self.isLastPage = isLastPage
        self.committed = committed
        self.selectKeys = selectKeys
        self.asciiMode = asciiMode
        self.schemaName = schemaName
        self.accepted = accepted
        self.candidatePanel = candidatePanel
        self.modeHint = modeHint
    }
}

extension KeyTaoImeState {
    static func decode(json: String?) -> KeyTaoImeState? {
        guard let json, let data = json.data(using: .utf8) else {
            return nil
        }
        return try? JSONDecoder().decode(KeyTaoImeState.self, from: data)
    }
}
