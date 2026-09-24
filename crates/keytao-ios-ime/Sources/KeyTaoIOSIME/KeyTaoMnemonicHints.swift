enum KeyTaoMnemonicHints {
    enum Tone {
        case outer
        case special
        case brand
    }

    struct Hint {
        var initial: String? = nil
        var finals: [String] = []
        var roots: [String] = []
        var rootKey: String? = nil
        var tone: Tone = .outer

        var topText: String? {
            switch tone {
            case .outer: return initial ?? rootKey
            case .special: return rootKey
            case .brand: return nil
            }
        }

        var bottomLines: [String] {
            let items = tone == .special ? roots : finals
            return stride(from: 0, to: items.count, by: 2).map { start in
                items[start..<min(start + 2, items.count)].joined(separator: " ")
            }
        }
    }

    static let byKey: [String: Hint] = [
        "Q": Hint(initial: "zh", finals: ["iu", "ua"]),
        "W": Hint(initial: "ch", finals: ["ei", "un"]),
        "E": Hint(initial: "sh", finals: ["e"]),
        "R": Hint(finals: ["eng"]),
        "T": Hint(finals: ["uan"]),
        "Y": Hint(finals: ["iong", "ong"]),
        "U": Hint(roots: ["月", "十o"], rootKey: "丿", tone: .special),
        "I": Hint(roots: ["人", "手u", "草i", "金o"], rootKey: "丨", tone: .special),
        "O": Hint(roots: ["口", "日i"], rootKey: "丶", tone: .special),
        "P": Hint(finals: ["ang"]),
        "A": Hint(roots: ["水", "贝o"], rootKey: "㇕", tone: .special),
        "S": Hint(finals: ["a", "ia"]),
        "D": Hint(finals: ["ie", "ou"]),
        "F": Hint(initial: "zh", finals: ["an"]),
        "G": Hint(finals: ["ing", "uai"]),
        "H": Hint(finals: ["ai", "üe"]),
        "J": Hint(initial: "ch", finals: ["er", "u"]),
        "K": Hint(finals: ["i"]),
        "L": Hint(finals: ["o", "uo", "ü"]),
        ";": Hint(),
        "Z": Hint(finals: ["ao"]),
        "X": Hint(finals: ["iang", "uang"], rootKey: "~"),
        "C": Hint(finals: ["iao"]),
        "V": Hint(roots: ["木", "土o"], rootKey: "一", tone: .special),
        "B": Hint(finals: ["in", "ui"]),
        "N": Hint(finals: ["en"]),
        "M": Hint(finals: ["ian", "uang"]),
        ",": Hint(),
        ".": Hint(),
        "/": Hint(finals: ["键道6"], tone: .brand),
    ]
}
