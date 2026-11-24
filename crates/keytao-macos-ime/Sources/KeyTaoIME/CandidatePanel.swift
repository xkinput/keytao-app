import Cocoa
import CKeytaoCore

/// A lightweight floating panel that displays Rime candidates.
/// Positioned just below the cursor of the active text field.
class CandidatePanel: NSPanel {

    var onSelect: ((Int) -> Void)?
    var onPageChange: ((Bool) -> Void)?

    private var chromeInset: CGFloat = 0
    private var chromeConstraints: [NSLayoutConstraint] = []

    private let rootView = NSView()
    private let containerView = NSView()
    private let stackView = NSStackView()
    private let candidateStack = NSStackView()
    private lazy var candidateWidthConstraint = candidateStack.widthAnchor.constraint(lessThanOrEqualToConstant: 1)
    private var stackLeadingConstraint: NSLayoutConstraint?
    private var stackTrailingConstraint: NSLayoutConstraint?
    private var stackTopConstraint: NSLayoutConstraint?
    private var stackBottomConstraint: NSLayoutConstraint?
    private var orientation: CandidatePanelOrientation = .vertical

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
        hasShadow = false
        isMovable = false
        hidesOnDeactivate = false

        rootView.wantsLayer = true
        rootView.layer?.backgroundColor = NSColor.clear.cgColor

        containerView.translatesAutoresizingMaskIntoConstraints = false
        containerView.wantsLayer = true
        containerView.layer?.masksToBounds = true

        stackView.translatesAutoresizingMaskIntoConstraints = false
        stackView.setContentHuggingPriority(.required, for: .horizontal)
        stackView.setContentHuggingPriority(.required, for: .vertical)
        candidateStack.translatesAutoresizingMaskIntoConstraints = false

        rootView.addSubview(containerView)
        containerView.addSubview(stackView)

        let leading = stackView.leadingAnchor.constraint(equalTo: containerView.leadingAnchor)
        let trailing = stackView.trailingAnchor.constraint(equalTo: containerView.trailingAnchor)
        let top = stackView.topAnchor.constraint(equalTo: containerView.topAnchor)
        let bottom = stackView.bottomAnchor.constraint(equalTo: containerView.bottomAnchor)
        stackLeadingConstraint = leading
        stackTrailingConstraint = trailing
        stackTopConstraint = top
        stackBottomConstraint = bottom

        chromeConstraints = [
            containerView.leadingAnchor.constraint(equalTo: rootView.leadingAnchor),
            containerView.trailingAnchor.constraint(equalTo: rootView.trailingAnchor),
            containerView.topAnchor.constraint(equalTo: rootView.topAnchor),
            containerView.bottomAnchor.constraint(equalTo: rootView.bottomAnchor),
        ]
        NSLayoutConstraint.activate(chromeConstraints + [
            leading,
            trailing,
            top,
            bottom,
        ])

        contentView = rootView
    }

    // MARK: - Update

    /// Renders the panel model keytao-theme already built: labels, highlight and
    /// page availability are decided there, so every frontend shows the same
    /// candidate list.
    func update(model: KeyTaoPanelModel, theme: ImeTheme, windowLevel: NSWindow.Level, near cursorRect: NSRect) {
        level = windowLevel
        apply(theme, orientation: model.orientation)
        let screen = NSScreen.screen(containing: cursorRect) ?? NSScreen.main
        let visibleFrame = screen?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
        let maxWidth = max(1, visibleFrame.width - theme.panel.screenMargin * 2 - chromeInset * 2)
        let maxHeight = min(theme.panel.maxHeight, max(60, visibleFrame.height - theme.panel.screenMargin * 2 - chromeInset * 2))

        stackView.arrangedSubviews.forEach { $0.removeFromSuperview() }
        candidateStack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        if let preedit = model.preedit, !preedit.isEmpty {
            let label = NSTextField(labelWithString: preedit)
            label.font = NSFont.keytaoThemeFont(family: theme.font.effectiveFamily, size: theme.font.preeditSize, weight: theme.font.weight)
            let foreground = theme.candidate.foreground.nsColor
            label.textColor = foreground
            if let caretRange = model.preeditCaretRange {
                let text = NSMutableAttributedString(attributedString: label.attributedStringValue)
                text.addAttribute(
                    .foregroundColor,
                    value: theme.candidate.preeditCaretColor.nsColor,
                    range: caretRange
                )
                label.attributedStringValue = text
            }
            label.lineBreakMode = .byTruncatingTail
            label.translatesAutoresizingMaskIntoConstraints = false
            label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
            label.widthAnchor.constraint(lessThanOrEqualToConstant: max(1, maxWidth - theme.panel.paddingLeft - theme.panel.paddingRight)).isActive = true
            stackView.addArrangedSubview(label)
            stackView.setCustomSpacing(theme.panel.preeditGap, after: label)
        }
        if !model.candidates.isEmpty || model.navigation.canGoPrevious || model.navigation.canGoNext {
            stackView.addArrangedSubview(candidateStack)
            candidateWidthConstraint.constant = max(1, maxWidth - theme.panel.paddingLeft - theme.panel.paddingRight)
            candidateWidthConstraint.isActive = true
        }

        let alignsFirstRow = orientation == .horizontal && theme.candidate.commentPosition == .below
            && model.candidates.contains { !($0.comment ?? "").isEmpty }
        let options = model.candidates.map { candidate in
            let option = CandidateOptionView(
                label: candidate.label,
                text: candidate.text,
                comment: candidate.comment ?? "",
                highlighted: candidate.selected,
                theme: theme,
                alignsFirstRow: alignsFirstRow
            )
            option.target = self
            option.tag = candidate.index
            option.action = #selector(candidateClicked(_:))
            return option
        }
        let navigation = navigationButtons(model.navigation, theme: theme)
        let separatorCount = theme.candidate.separatorVisible ? max(0, options.count - 1) : 0
        let selected = model.candidates.firstIndex(where: \.selected) ?? 0
        var rowWidths: (fitted: [Float], minimum: [Float])?
        if orientation == .horizontal && !options.isEmpty {
            let navigationWidth = navigation.reduce(CGFloat(0)) { $0 + $1.fittingSize.width }
            let gaps = max(0, options.count + separatorCount + navigation.count - 1)
            let available = maxWidth - theme.panel.paddingLeft - theme.panel.paddingRight
                - navigationWidth - CGFloat(separatorCount) - CGFloat(gaps) * theme.panel.gap
            let natural = options.map { Float($0.intrinsicContentSize.width) }
            let minimum = options.enumerated().map { position, option in
                Float(option.minimumWidth(nearSelection: abs(position - selected) <= 1))
            }
            var fitted = Array(repeating: Float(0), count: options.count)
            if keytao_fit_candidate_widths(natural, minimum, UInt(options.count), UInt(selected), Float(available), &fitted) {
                rowWidths = (fitted, minimum)
            }
        }

        var firstOption: CandidateOptionView?
        for (position, option) in options.enumerated() {
            if let rowWidths {
                let width = rowWidths.fitted[position]
                guard width > 0 else { continue }
                option.fitWidth(
                    CGFloat(width), minimum: CGFloat(rowWidths.minimum[position]),
                    nearSelection: abs(position - selected) <= 1
                )
            }
            if theme.candidate.separatorVisible && firstOption != nil {
                let separator = makeSeparator(theme: theme)
                candidateStack.addArrangedSubview(separator)
                if orientation == .vertical {
                    separator.widthAnchor.constraint(equalTo: candidateStack.widthAnchor).isActive = true
                }
            }
            candidateStack.addArrangedSubview(option)
            option.widthAnchor.constraint(lessThanOrEqualTo: candidateStack.widthAnchor).isActive = true
            if let firstOption, alignsFirstRow {
                option.heightAnchor.constraint(equalTo: firstOption.heightAnchor).isActive = true
            }
            firstOption = firstOption ?? option
        }

        if !navigation.isEmpty {
            addNavigation(navigation, theme: theme)
        }

        contentView?.layoutSubtreeIfNeeded()
        let contentSize = stackView.fittingSize
        let panelSize = NSSize(
            width: min(max(contentSize.width + theme.panel.paddingLeft + theme.panel.paddingRight, theme.panel.minWidth), maxWidth),
            height: min(contentSize.height + theme.panel.paddingTop + theme.panel.paddingBottom, maxHeight)
        )
        let windowSize = NSSize(
            width: panelSize.width + chromeInset * 2,
            height: panelSize.height + chromeInset * 2
        )
        let anchor = cursorRect.isUsableTextInputRect
            ? cursorRect
            : NSRect(origin: NSEvent.mouseLocation, size: .zero)

        let margin = theme.panel.screenMargin
        var origin = NSPoint(x: anchor.minX - chromeInset, y: anchor.minY - panelSize.height - margin / 2 - chromeInset)
        if origin.y < visibleFrame.minY + margin {
            origin.y = anchor.maxY + margin / 2 - chromeInset
        }
        origin.x = min(max(origin.x, visibleFrame.minX + margin), visibleFrame.maxX - windowSize.width - margin)
        origin.y = min(max(origin.y, visibleFrame.minY + margin), visibleFrame.maxY - windowSize.height - margin)
        setFrame(NSRect(origin: origin, size: windowSize), display: true, animate: false)
        rootView.layoutSubtreeIfNeeded()
        rootView.layer?.shadowPath = CGPath(roundedRect: containerView.frame, cornerWidth: theme.panel.cornerRadius, cornerHeight: theme.panel.cornerRadius, transform: nil)

        orderFront(nil)
    }

    private func apply(_ theme: ImeTheme, orientation: CandidatePanelOrientation) {
        self.orientation = orientation
        hasShadow = false
        let shadow = theme.panel.shadowStyle
        chromeInset = theme.panel.shadow ? (shadow?.margin ?? 0) : 0
        for (constraint, sign) in zip(chromeConstraints, [CGFloat(1), -1, 1, -1]) {
            constraint.constant = chromeInset * sign
        }
        rootView.layer?.shadowOpacity = theme.panel.shadow ? 1 : 0
        rootView.layer?.shadowColor = shadow?.color.cgColor
        rootView.layer?.shadowOffset = CGSize(width: 0, height: -(shadow?.offsetY ?? 0))
        rootView.layer?.shadowRadius = (shadow?.blurRadius ?? 0) / 2
        containerView.layer?.backgroundColor = theme.panel.background.cgColor
        containerView.layer?.cornerRadius = theme.panel.cornerRadius
        containerView.layer?.borderColor = theme.panel.borderColor.cgColor
        containerView.layer?.borderWidth = theme.panel.borderWidth

        stackView.orientation = .vertical
        stackView.alignment = .leading
        stackView.spacing = 0
        candidateStack.orientation = orientation == .vertical ? .vertical : .horizontal
        candidateStack.alignment = orientation == .vertical ? .leading : .centerY
        candidateStack.spacing = theme.panel.gap
        stackLeadingConstraint?.constant = theme.panel.paddingLeft
        stackTrailingConstraint?.constant = -theme.panel.paddingRight
        stackTopConstraint?.constant = theme.panel.paddingTop
        stackBottomConstraint?.constant = -theme.panel.paddingBottom
    }

    // MARK: - Actions

    @objc private func candidateClicked(_ sender: NSControl) {
        guard isVisible, sender.window === self, sender.isDescendant(of: candidateStack) else { return }
        onSelect?(sender.tag)
    }

    @objc private func prevPage(_ sender: NSControl) {
        guard isVisible, sender.window === self, sender.isDescendant(of: candidateStack) else { return }
        onPageChange?(true)
    }
    @objc private func nextPage(_ sender: NSControl) {
        guard isVisible, sender.window === self, sender.isDescendant(of: candidateStack) else { return }
        onPageChange?(false)
    }

    // MARK: - View factories

    private func navigationButtons(_ navigation: KeyTaoPanelNavigation, theme: ImeTheme) -> [CandidateNavigationButton] {
        var buttons: [CandidateNavigationButton] = []
        if navigation.canGoPrevious {
            buttons.append(makeNavButton(symbol: "‹", action: #selector(prevPage(_:)), theme: theme))
        }
        if navigation.canGoNext {
            buttons.append(makeNavButton(symbol: "›", action: #selector(nextPage(_:)), theme: theme))
        }
        return buttons
    }

    private func addNavigation(_ buttons: [CandidateNavigationButton], theme: ImeTheme) {
        if orientation == .vertical {
            let row = NSStackView(views: buttons)
            row.orientation = .horizontal
            row.alignment = .centerY
            row.spacing = theme.panel.gap
            candidateStack.addArrangedSubview(row)
        } else {
            buttons.forEach(candidateStack.addArrangedSubview)
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
        if orientation == .vertical {
            separator.heightAnchor.constraint(equalToConstant: 1).isActive = true
        } else {
            separator.widthAnchor.constraint(equalToConstant: 1).isActive = true
            separator.heightAnchor.constraint(equalToConstant: max(1, theme.candidate.minHeight - theme.candidate.paddingY * 2)).isActive = true
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
    private var stacksComment: Bool {
        theme.candidate.commentPosition == .below && !commentField.isHidden
    }
    private var hovered = false {
        didSet { applyState() }
    }
    private var pressed = false {
        didSet { applyState() }
    }

    init(label: String, text: String, comment: String, highlighted: Bool, theme: ImeTheme, alignsFirstRow: Bool) {
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
        // A long comment (e.g. reverse-lookup codes) must truncate before the candidate text.
        labelField.setContentCompressionResistancePriority(.required, for: .horizontal)
        textField.setContentCompressionResistancePriority(.defaultHigh, for: .horizontal)
        commentField.setContentCompressionResistancePriority(.init(1), for: .horizontal)

        contentStack.orientation = .horizontal
        contentStack.alignment = stacksComment || alignsFirstRow ? .top : .firstBaseline
        contentStack.spacing = theme.candidate.inlineGap
        contentStack.translatesAutoresizingMaskIntoConstraints = false
        contentStack.addArrangedSubview(labelField)
        if stacksComment {
            let wordStack = NSStackView(views: [textField, commentField])
            wordStack.orientation = .vertical
            wordStack.alignment = .leading
            wordStack.spacing = theme.candidate.inlineGap * 0.5
            contentStack.addArrangedSubview(wordStack)
        } else {
            contentStack.addArrangedSubview(textField)
            contentStack.addArrangedSubview(commentField)
        }
        addSubview(contentStack)

        let bottomConstraint = alignsFirstRow
            ? contentStack.bottomAnchor.constraint(lessThanOrEqualTo: bottomAnchor, constant: -theme.candidate.paddingY)
            : contentStack.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -theme.candidate.paddingY)
        NSLayoutConstraint.activate([
            contentStack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: theme.candidate.paddingX),
            contentStack.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -theme.candidate.paddingX),
            contentStack.topAnchor.constraint(equalTo: topAnchor, constant: theme.candidate.paddingY),
            bottomConstraint,
            heightAnchor.constraint(greaterThanOrEqualToConstant: theme.candidate.minHeight),
        ])

        applyState()
    }

    required init?(coder: NSCoder) {
        return nil
    }

    override var intrinsicContentSize: NSSize {
        let contentSize = contentStack.fittingSize
        let gap = theme.candidate.inlineGap
        let contentWidth: CGFloat
        // Fitting size compresses the comment even when the screen has room.
        if stacksComment {
            contentWidth = labelField.intrinsicContentSize.width + gap
                + max(textField.intrinsicContentSize.width, commentField.intrinsicContentSize.width)
        } else {
            let fields = [labelField, textField, commentField].filter { !$0.isHidden }
            contentWidth = fields.reduce(0) { $0 + $1.intrinsicContentSize.width }
                + CGFloat(fields.count - 1) * gap
        }
        return NSSize(
            width: contentWidth + theme.candidate.paddingX * 2,
            height: max(
                contentSize.height + theme.candidate.paddingY * 2,
                theme.candidate.minHeight
            )
        )
    }

    func minimumWidth(nearSelection: Bool) -> CGFloat {
        let prefix = nearSelection ? String(textField.stringValue.prefix(1)) : ""
        let font = textField.font ?? .systemFont(ofSize: theme.font.size)
        let textWidth = ((prefix + "…") as NSString).size(withAttributes: [.font: font]).width
        return ceil(labelField.intrinsicContentSize.width + theme.candidate.inlineGap
            + textWidth + theme.candidate.paddingX * 2)
    }

    func fitWidth(_ width: CGFloat, minimum: CGFloat, nearSelection: Bool) {
        if !nearSelection && width <= minimum && width < intrinsicContentSize.width {
            textField.stringValue = "…"
            commentField.isHidden = true
            invalidateIntrinsicContentSize()
        }
        if width < minimum {
            labelField.setContentCompressionResistancePriority(.defaultHigh, for: .horizontal)
        }
        widthAnchor.constraint(equalToConstant: width).isActive = true
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
        pressed = true
        defer { pressed = false }
        while let next = window?.nextEvent(matching: [.leftMouseDragged, .leftMouseUp]) {
            pressed = bounds.contains(convert(next.locationInWindow, from: nil))
            if next.type == .leftMouseUp {
                if pressed { sendAction(action, to: target) }
                break
            }
        }
    }

    private func applyState() {
        let option = theme.candidate
        let background = pressed ? option.pressedBackground
            : isSelectedOption ? option.selectedBackground
            : hovered ? option.hoverBackground : option.background
        layer?.backgroundColor = background.cgColor
        layer?.cornerRadius = option.cornerRadius
        layer?.borderWidth = option.borderWidth
        layer?.borderColor = (isSelectedOption ? option.selectedBorderColor : option.borderColor).cgColor

        labelField.font = NSFont.keytaoThemeFont(family: theme.font.effectiveFamily, size: theme.font.labelSize, weight: theme.font.weight)
        textField.font = NSFont.keytaoThemeFont(family: theme.font.effectiveFamily, size: theme.font.size, weight: theme.font.weight)
        commentField.font = NSFont.keytaoThemeFont(family: theme.font.effectiveFamily, size: theme.font.commentSize, weight: theme.font.weight)

        labelField.textColor = (isSelectedOption ? option.selectedLabelColor : option.labelColor).nsColor
        textField.textColor = (pressed ? option.pressedForeground : isSelectedOption ? option.selectedForeground : option.foreground).nsColor
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
        label.font = .keytaoThemeFont(family: theme.font.effectiveFamily, size: theme.font.size * 1.3)
        label.translatesAutoresizingMaskIntoConstraints = false
        addSubview(label)

        let buttonSize = max(theme.navigation.buttonSize, label.intrinsicContentSize.height)
        NSLayoutConstraint.activate([
            widthAnchor.constraint(equalToConstant: buttonSize),
            heightAnchor.constraint(equalToConstant: buttonSize),
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
