import Foundation

enum KeyTaoMarkedTextAction: Equatable {
    case none
    case clear
    case placeholder(replacementRange: NSRange)
    case inline(text: String, hiddenStyle: Bool)
}

enum KeyTaoMarkedTextPolicy {
    static let placeholderText = "\u{200B}"

    static func decide(
        preedit: String,
        commitPreview: String,
        hasComposition: Bool,
        isPackageSwitcher: Bool,
        embeddedComposition: Bool,
        hideCandidatePanel: Bool,
        hadMarkedText: Bool,
        selectedRange: NSRange = NSRange(location: NSNotFound, length: 0)
    ) -> KeyTaoMarkedTextAction {
        guard hasComposition else {
            return hadMarkedText ? .clear : .none
        }
        guard embeddedComposition else {
            return .placeholder(
                replacementRange: placeholderReplacementRange(
                    hadMarkedText: hadMarkedText,
                    candidateOnly: preedit.isEmpty,
                    selectedRange: selectedRange
                )
            )
        }
        let hidden = hideCandidatePanel && !isPackageSwitcher
        let inlineText = hidden && !commitPreview.isEmpty ? commitPreview : preedit
        guard !inlineText.isEmpty else {
            // Candidate-only compositions (such as the F4 package/schema menu)
            // still need a non-empty marked range so AppKit, Chromium, Electron
            // and WebKit clients treat navigation and Return as IME-owned.
            return .placeholder(
                replacementRange: placeholderReplacementRange(
                    hadMarkedText: hadMarkedText,
                    candidateOnly: true,
                    selectedRange: selectedRange
                )
            )
        }
        return .inline(text: inlineText, hiddenStyle: hidden)
    }

    static func placeholderReplacementRange(
        hadMarkedText: Bool,
        candidateOnly: Bool,
        selectedRange: NSRange
    ) -> NSRange {
        if !hadMarkedText,
           candidateOnly,
           selectedRange.location != NSNotFound,
           selectedRange.length > 0 {
            let collapsed = selectedRange.location.addingReportingOverflow(selectedRange.length)
            if !collapsed.overflow {
                return NSRange(location: collapsed.partialValue, length: 0)
            }
        }
        return NSRange(location: NSNotFound, length: 0)
    }
}
