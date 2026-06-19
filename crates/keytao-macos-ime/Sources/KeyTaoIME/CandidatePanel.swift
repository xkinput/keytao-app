import Cocoa

/// A lightweight floating panel that displays Rime candidates.
/// Positioned just below the cursor of the active text field.
class CandidatePanel: NSPanel {

    var onSelect: ((Int) -> Void)?
    var onPageChange: ((Bool) -> Void)?

    private let containerView = NSView()
    private let stackView = NSStackView()

    // MARK: - Init

    override init(contentRect: NSRect, styleMask style: NSWindow.StyleMask,
                  backing backingStoreType: NSWindow.BackingStoreType, defer flag: Bool) {
        super.init(contentRect: NSRect(x: 0, y: 0, width: 400, height: 36),
                   styleMask: [.nonactivatingPanel, .borderless],
                   backing: .buffered, defer: false)
        configure()
    }

    convenience init() {
        self.init(contentRect: .zero, styleMask: [], backing: .buffered, defer: false)
    }

    private func configure() {
        isFloatingPanel = true
        level = .popUpMenu
        isOpaque = false
        backgroundColor = .clear
        hasShadow = true
        isMovable = false
        hidesOnDeactivate = false

        containerView.wantsLayer = true
        containerView.layer?.masksToBounds = true

        stackView.translatesAutoresizingMaskIntoConstraints = false
        stackView.setContentHuggingPriority(.required, for: .horizontal)
        stackView.setContentHuggingPriority(.required, for: .vertical)

        containerView.addSubview(stackView)
        NSLayoutConstraint.activate([
            stackView.leadingAnchor.constraint(equalTo: containerView.leadingAnchor),
            stackView.trailingAnchor.constraint(equalTo: containerView.trailingAnchor),
            stackView.topAnchor.constraint(equalTo: containerView.topAnchor),
            stackView.bottomAnchor.constraint(equalTo: containerView.bottomAnchor),
        ])

        contentView = containerView
    }

    // MARK: - Update

    func update(texts: [String], comments: [String],
                highlightedIndex: Int,
                page: Int, isLastPage: Bool, selectKeys: String,
                near cursorRect: NSRect) {
        let theme = ImeThemeManager.shared.theme()
        apply(theme)

        stackView.arrangedSubviews.forEach { $0.removeFromSuperview() }

        let keys = Array(selectKeys.isEmpty ? "1234567890" : selectKeys)
        let selectedIndex = highlightedIndex.clamped(to: 0...Swift.max(texts.count - 1, 0))

        for (i, text) in texts.enumerated() {
            let option = CandidateOptionView(
                label: "\(keys[safe: i] ?? Character("?"))\(theme.candidate.labelSuffix)",
                text: text,
                comment: comments[safe: i] ?? "",
                highlighted: i == selectedIndex,
                theme: theme
            )
            let idx = i
            option.target = self
            option.tag = idx
            option.action = #selector(candidateClicked(_:))
            stackView.addArrangedSubview(option)

            if theme.candidate.separatorVisible && i < texts.count - 1 {
                stackView.addArrangedSubview(makeSeparator(theme: theme))
            }
        }

        if page > 0 || !isLastPage {
            addNavigation(page: page, isLastPage: isLastPage, theme: theme)
        }

        contentView?.layoutSubtreeIfNeeded()
        let fittingSize = stackView.fittingSize
        let screen = NSScreen.screen(containing: cursorRect) ?? NSScreen.main
        let visibleFrame = screen?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
        let maxWidth = min(theme.panel.maxWidth, max(80, visibleFrame.width - theme.panel.screenMargin * 2))
        let maxHeight = min(theme.panel.maxHeight, max(60, visibleFrame.height - theme.panel.screenMargin * 2))
        let finalSize = NSSize(
            width: min(max(fittingSize.width, theme.panel.minWidth), maxWidth),
            height: min(fittingSize.height, maxHeight)
        )
        let anchor = cursorRect.isUsableTextInputRect
            ? cursorRect
            : NSRect(origin: NSEvent.mouseLocation, size: .zero)

        let margin = theme.panel.screenMargin
        var origin = NSPoint(x: anchor.minX, y: anchor.minY - finalSize.height - margin / 2)
        if origin.y < visibleFrame.minY + margin {
            origin.y = anchor.maxY + margin / 2
        }
        origin.x = min(max(origin.x, visibleFrame.minX + margin), visibleFrame.maxX - finalSize.width - margin)
        origin.y = min(max(origin.y, visibleFrame.minY + margin), visibleFrame.maxY - finalSize.height - margin)
        setFrame(NSRect(origin: origin, size: finalSize), display: true, animate: false)

        orderFront(nil)
    }

    private func apply(_ theme: ImeTheme) {
        hasShadow = theme.panel.shadow
        containerView.layer?.backgroundColor = theme.panel.background.cgColor
        containerView.layer?.cornerRadius = theme.panel.cornerRadius
        containerView.layer?.borderColor = theme.panel.borderColor.cgColor
        containerView.layer?.borderWidth = theme.panel.borderWidth

        stackView.orientation = theme.panel.orientation == .vertical ? .vertical : .horizontal
        stackView.alignment = theme.panel.orientation == .vertical ? .leading : .centerY
        stackView.spacing = theme.panel.gap
        stackView.edgeInsets = NSEdgeInsets(
            top: theme.panel.paddingY,
            left: theme.panel.paddingX,
            bottom: theme.panel.paddingY,
            right: theme.panel.paddingX
        )
    }

    // MARK: - Actions

    @objc private func candidateClicked(_ sender: NSControl) {
        onSelect?(sender.tag)
    }

    @objc private func prevPage() { onPageChange?(true) }
    @objc private func nextPage() { onPageChange?(false) }

    // MARK: - View factories

    private func addNavigation(page: Int, isLastPage: Bool, theme: ImeTheme) {
        if theme.panel.orientation == .vertical {
            let row = NSStackView()
            row.orientation = .horizontal
            row.alignment = .centerY
            row.spacing = theme.panel.gap
            if page > 0 {
                row.addArrangedSubview(makeNavButton(symbol: "‹", action: #selector(prevPage), theme: theme))
            }
            if !isLastPage {
                row.addArrangedSubview(makeNavButton(symbol: "›", action: #selector(nextPage), theme: theme))
            }
            stackView.addArrangedSubview(row)
            return
        }

        if page > 0 {
            stackView.addArrangedSubview(makeNavButton(symbol: "‹", action: #selector(prevPage), theme: theme))
        }
        if !isLastPage {
            stackView.addArrangedSubview(makeNavButton(symbol: "›", action: #selector(nextPage), theme: theme))
        }
    }

    private func makeNavButton(symbol: String, action: Selector, theme: ImeTheme) -> CandidateNavigationButton {
        let button = CandidateNavigationButton(symbol: symbol, theme: theme)
        button.target = self
        button.action = action
        return button
    }

    private func makeSeparator(theme: ImeTheme) -> NSView {
        let separator = NSView()
        separator.wantsLayer = true
        separator.layer?.backgroundColor = theme.candidate.separatorColor.cgColor
        if theme.panel.orientation == .vertical {
            separator.heightAnchor.constraint(equalToConstant: 1).isActive = true
            separator.widthAnchor.constraint(greaterThanOrEqualToConstant: theme.panel.minWidth - theme.panel.paddingX * 2).isActive = true
        } else {
            separator.widthAnchor.constraint(equalToConstant: 1).isActive = true
            separator.heightAnchor.constraint(equalToConstant: max(1, theme.candidate.minHeight - 8)).isActive = true
        }
        return separator
    }
}

private final class CandidateOptionView: NSControl {
    private let labelField = NSTextField(labelWithString: "")
    private let textField = NSTextField(labelWithString: "")
    private let commentField = NSTextField(labelWithString: "")
    private let contentStack = NSStackView()
    private var trackingArea: NSTrackingArea?
    private let theme: ImeTheme
    private let isSelectedOption: Bool
    private var hovered = false {
        didSet { applyState() }
    }

    init(label: String, text: String, comment: String, highlighted: Bool, theme: ImeTheme) {
        self.theme = theme
        self.isSelectedOption = highlighted
        super.init(frame: .zero)

        wantsLayer = true
        translatesAutoresizingMaskIntoConstraints = false

        labelField.stringValue = label
        textField.stringValue = text
        commentField.stringValue = comment
        commentField.isHidden = comment.isEmpty

        for field in [labelField, textField, commentField] {
            field.lineBreakMode = .byTruncatingTail
            field.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        }
        labelField.setContentHuggingPriority(.required, for: .horizontal)
        commentField.setContentHuggingPriority(.defaultHigh, for: .horizontal)

        contentStack.orientation = .horizontal
        contentStack.alignment = .firstBaseline
        contentStack.spacing = theme.candidate.inlineGap
        contentStack.translatesAutoresizingMaskIntoConstraints = false
        contentStack.addArrangedSubview(labelField)
        contentStack.addArrangedSubview(textField)
        contentStack.addArrangedSubview(commentField)
        addSubview(contentStack)

        NSLayoutConstraint.activate([
            contentStack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: theme.candidate.paddingX),
            contentStack.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -theme.candidate.paddingX),
            contentStack.topAnchor.constraint(equalTo: topAnchor, constant: theme.candidate.paddingY),
            contentStack.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -theme.candidate.paddingY),
            heightAnchor.constraint(greaterThanOrEqualToConstant: theme.candidate.minHeight),
            widthAnchor.constraint(lessThanOrEqualToConstant: theme.candidate.maxWidth),
        ])

        applyState()
    }

    required init?(coder: NSCoder) {
        return nil
    }

    override var intrinsicContentSize: NSSize {
        let contentSize = contentStack.fittingSize
        return NSSize(
            width: min(
                contentSize.width + theme.candidate.paddingX * 2,
                theme.candidate.maxWidth
            ),
            height: max(
                contentSize.height + theme.candidate.paddingY * 2,
                theme.candidate.minHeight
            )
        )
    }

    override func updateTrackingAreas() {
        if let trackingArea {
            removeTrackingArea(trackingArea)
        }
        let nextArea = NSTrackingArea(
            rect: bounds,
            options: [.activeAlways, .mouseEnteredAndExited, .inVisibleRect],
            owner: self,
            userInfo: nil
        )
        addTrackingArea(nextArea)
        trackingArea = nextArea
        super.updateTrackingAreas()
    }

    override func mouseEntered(with event: NSEvent) {
        hovered = true
    }

    override func mouseExited(with event: NSEvent) {
        hovered = false
    }

    override func mouseDown(with event: NSEvent) {
        sendAction(action, to: target)
    }

    private func applyState() {
        let option = theme.candidate
        let background = isSelectedOption
            ? option.selectedBackground
            : hovered ? option.hoverBackground : option.background
        layer?.backgroundColor = background.cgColor
        layer?.cornerRadius = option.cornerRadius
        layer?.borderWidth = isSelectedOption ? Swift.max(option.borderWidth, 1) : option.borderWidth
        layer?.borderColor = (isSelectedOption ? option.selectedBorderColor : option.borderColor).cgColor

        labelField.font = NSFont.keytaoThemeFont(family: theme.font.family, size: theme.font.labelSize, weight: .semiBold)
        textField.font = NSFont.keytaoThemeFont(family: theme.font.family, size: theme.font.size, weight: theme.font.weight)
        commentField.font = NSFont.keytaoThemeFont(family: theme.font.family, size: theme.font.commentSize)

        labelField.textColor = (isSelectedOption ? option.selectedLabelColor : option.labelColor).nsColor
        textField.textColor = (isSelectedOption ? option.selectedForeground : option.foreground).nsColor
        commentField.textColor = (isSelectedOption ? option.selectedCommentColor : option.commentColor).nsColor
    }
}

private final class CandidateNavigationButton: NSControl {
    private let label = NSTextField(labelWithString: "")
    private var trackingArea: NSTrackingArea?
    private let theme: ImeTheme
    private var hovered = false {
        didSet { applyState() }
    }

    init(symbol: String, theme: ImeTheme) {
        self.theme = theme
        super.init(frame: .zero)

        wantsLayer = true
        translatesAutoresizingMaskIntoConstraints = false
        label.stringValue = symbol
        label.alignment = .center
        label.font = .systemFont(ofSize: theme.font.size + 6, weight: .regular)
        label.translatesAutoresizingMaskIntoConstraints = false
        addSubview(label)

        NSLayoutConstraint.activate([
            widthAnchor.constraint(equalToConstant: theme.navigation.buttonSize),
            heightAnchor.constraint(equalToConstant: theme.navigation.buttonSize),
            label.leadingAnchor.constraint(equalTo: leadingAnchor),
            label.trailingAnchor.constraint(equalTo: trailingAnchor),
            label.topAnchor.constraint(equalTo: topAnchor),
            label.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])

        applyState()
    }

    required init?(coder: NSCoder) {
        return nil
    }

    override func updateTrackingAreas() {
        if let trackingArea {
            removeTrackingArea(trackingArea)
        }
        let nextArea = NSTrackingArea(
            rect: bounds,
            options: [.activeAlways, .mouseEnteredAndExited, .inVisibleRect],
            owner: self,
            userInfo: nil
        )
        addTrackingArea(nextArea)
        trackingArea = nextArea
        super.updateTrackingAreas()
    }

    override func mouseEntered(with event: NSEvent) {
        hovered = true
    }

    override func mouseExited(with event: NSEvent) {
        hovered = false
    }

    override func mouseDown(with event: NSEvent) {
        sendAction(action, to: target)
    }

    private func applyState() {
        layer?.backgroundColor = (hovered ? theme.navigation.hoverBackground : ThemeColor(red: 0, green: 0, blue: 0, alpha: 0)).cgColor
        layer?.cornerRadius = theme.navigation.cornerRadius
        label.textColor = theme.navigation.foreground.nsColor
    }
}

// MARK: – Safe array subscript

private extension Array {
    subscript(safe index: Int) -> Element? {
        guard index >= 0 && index < count else { return nil }
        return self[index]
    }
}

private extension Int {
    func clamped(to range: ClosedRange<Int>) -> Int {
        Swift.min(Swift.max(self, range.lowerBound), range.upperBound)
    }
}
