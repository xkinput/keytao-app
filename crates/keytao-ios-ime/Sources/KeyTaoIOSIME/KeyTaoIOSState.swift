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
    public var cursor: Int
    public var candidates: [KeyTaoCandidate]
    public var allCandidates: [KeyTaoCandidate]
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
        candidates: [],
        allCandidates: [],
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
}

extension KeyTaoImeState {
    static func decode(json: String?) -> KeyTaoImeState? {
        guard let json, let data = json.data(using: .utf8) else {
            return nil
        }
        return try? JSONDecoder().decode(KeyTaoImeState.self, from: data)
    }
}
