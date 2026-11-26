import Cocoa
import CKeytaoCore

/// A lightweight floating panel that displays Rime candidates.
/// Positioned just below the cursor of the active text field.
class CandidatePanel: NSPanel {

    var onSelect: ((Int) -> Void)?
    var onPageChange: ((Bool) -> Void)?

    private let rootView = NSView()
    private let containerView = NSView()
    private let preeditField = NSTextField(labelWithString: "")
    private let candidateContent = NSView()
    private var options: [CandidateOptionView] = []
    private var separators: [NSView] = []
    private let previousButton = CandidateNavigationButton(symbol: "‹")
    private let nextButton = CandidateNavigationButton(symbol: "›")
    private var fonts: CandidatePanelFonts?
    private var chromeInset: CGFloat = 0
    private var shadowCornerRadius: CGFloat?

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
        containerView.wantsLayer = true
        containerView.layer?.masksToBounds = true
        preeditField.lineBreakMode = .byTruncatingTail

        rootView.addSubview(containerView)
        containerView.addSubview(preeditField)
        containerView.addSubview(candidateContent)
        previousButton.target = self
        previousButton.action = #selector(prevPage(_:))
        nextButton.target = self
        nextButton.action = #selector(nextPage(_:))
        candidateContent.addSubview(previousButton)
        candidateContent.addSubview(nextButton)
        contentView = rootView
    }

    // MARK: - Update

    /// Renders the panel model keytao-theme already built: labels, highlight and
    /// page availability are decided there, so every frontend shows the same
    /// candidate list.
    func update(model: KeyTaoPanelModel, theme: ImeTheme, windowLevel: NSWindow.Level, near cursorRect: NSRect) {
        if level != windowLevel { level = windowLevel }
        apply(theme)
        let fonts: CandidatePanelFonts
        if let cached = self.fonts, cached.matches(theme.font) {
            fonts = cached
        } else {
            fonts = CandidatePanelFonts(theme.font)
            self.fonts = fonts
        }
        let screen = NSScreen.screen(containing: cursorRect) ?? NSScreen.main
        let visibleFrame = screen?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
        let maxWidth = max(1, visibleFrame.width - theme.panel.screenMargin * 2 - chromeInset * 2)
        let maxHeight = min(theme.panel.maxHeight, max(60, visibleFrame.height - theme.panel.screenMargin * 2 - chromeInset * 2))
        let horizontal = model.orientation == .horizontal
        let gap = theme.panel.gap
        let horizontalPadding = theme.panel.paddingLeft + theme.panel.paddingRight
        let innerWidth = max(1, maxWidth - horizontalPadding)

        var preeditSize = NSSize.zero
        preeditField.isHidden = model.preedit?.isEmpty ?? true
        if let preedit = model.preedit, !preedit.isEmpty {
            if preeditField.font != fonts.preedit { preeditField.font = fonts.preedit }
            preeditField.stringValue = preedit
            preeditField.textColor = theme.candidate.foreground.nsColor
            if let caretRange = model.preeditCaretRange {
                let text = NSMutableAttributedString(attributedString: preeditField.attributedStringValue)
                text.addAttribute(.foregroundColor, value: theme.candidate.preeditCaretColor.nsColor, range: caretRange)
                preeditField.attributedStringValue = text
            }
            preeditSize = preeditField.intrinsicContentSize
            preeditSize.width = min(preeditSize.width, innerWidth)
        }

        previousButton.isHidden = !model.navigation.canGoPrevious
        nextButton.isHidden = !model.navigation.canGoNext
        let navigation = [previousButton, nextButton].filter { !$0.isHidden }
        for button in navigation {
            button.update(theme.navigation, font: fonts.navigation)
        }
        let navigationSize = max(theme.navigation.buttonSize, fonts.navigationHeight)
        let navigationWidth = CGFloat(navigation.count) * navigationSize
        let navigationGaps = CGFloat(max(0, navigation.count - 1)) * gap

        while options.count < model.candidates.count {
            let option = CandidateOptionView(style: theme.candidate)
            option.target = self
            option.action = #selector(candidateClicked(_:))
            candidateContent.addSubview(option, positioned: .below, relativeTo: previousButton)
            options.append(option)
        }

        let alignsFirstRow = horizontal && theme.candidate.commentPosition == .below
            && model.candidates.contains { !($0.comment ?? "").isEmpty }
        let active = options.prefix(model.candidates.count)
        for (option, candidate) in zip(active, model.candidates) {
            option.update(candidate, style: theme.candidate, fonts: fonts)
        }
        for option in options.dropFirst(model.candidates.count) {
            option.isHidden = true
        }

        var widths = active.map(\.naturalWidth)
        let separatorCount = theme.candidate.separatorVisible ? max(0, active.count - 1) : 0
        if horizontal && !active.isEmpty {
            let gaps = max(0, active.count + separatorCount + navigation.count - 1)
            let available = maxWidth - horizontalPadding - navigationWidth - CGFloat(separatorCount) - CGFloat(gaps) * gap
            if widths.reduce(0, +) > available {
                let selected = model.candidates.firstIndex(where: \.selected) ?? 0
                let natural = widths.map { Float($0) }
                let minimum = active.enumerated().map { position, option in
                    Float(option.minimumWidth(nearSelection: abs(position - selected) <= 1, font: fonts.text, ellipsisWidth: fonts.ellipsisWidth))
                }
                var fitted = Array(repeating: Float(0), count: active.count)
                if keytao_fit_candidate_widths(natural, minimum, UInt(active.count), UInt(selected), Float(available), &fitted) {
                    widths = fitted.map { CGFloat($0) }
                    for (position, option) in active.enumerated() where fitted[position] > 0 {
                        if abs(position - selected) > 1 && fitted[position] <= minimum[position] && fitted[position] < natural[position] {
                            option.collapseToEllipsis()
                        }
                    }
                }
            }
        } else {
            widths = widths.map { min($0, innerWidth) }
        }

        let visibleOptions = zip(active, widths).filter { option, width in
            option.isHidden = width <= 0
            return !option.isHidden
        }
        let visibleSeparators = theme.candidate.separatorVisible ? max(0, visibleOptions.count - 1) : 0
        while separators.count < visibleSeparators {
            let separator = NSView()
            separator.wantsLayer = true
            candidateContent.addSubview(separator)
            separators.append(separator)
        }
        for (index, separator) in separators.enumerated() {
            separator.isHidden = index >= visibleSeparators
            if !separator.isHidden {
                separator.layer?.backgroundColor = theme.candidate.separatorColor.cgColor
            }
        }

        let optionHeights = visibleOptions.map { $0.0.naturalHeight(alignsFirstRow: alignsFirstRow) }
        let rowHeight = optionHeights.max() ?? 0
        let separatorHeight = max(1, theme.candidate.minHeight - theme.candidate.paddingY * 2)
        let navHeight = navigation.isEmpty ? 0 : navigationSize
        let itemCount = visibleOptions.count + visibleSeparators + (horizontal ? navigation.count : min(1, navigation.count))
        let distance = CGFloat(max(0, itemCount - 1)) * gap + CGFloat(visibleSeparators)
        let optionWidths = visibleOptions.map { $0.1 }
        let candidateSize = horizontal
            ? NSSize(width: min(innerWidth, (optionWidths.reduce(navigationWidth, +) + distance).rounded(.up)),
                     height: max(rowHeight, navHeight, visibleSeparators > 0 ? separatorHeight : 0))
            : NSSize(width: min(innerWidth, max(optionWidths.max() ?? 0, navigationWidth + navigationGaps)),
                     height: optionHeights.reduce(navHeight, +) + distance)
        let hasCandidates = !model.candidates.isEmpty || !navigation.isEmpty
        let preeditHeight = preeditSize.height + (!preeditField.isHidden && hasCandidates ? theme.panel.preeditGap : 0)
        let naturalHeight = preeditHeight + candidateSize.height + theme.panel.paddingTop + theme.panel.paddingBottom
        let panelSize = NSSize(
            width: min(max(max(preeditSize.width, candidateSize.width) + horizontalPadding, theme.panel.minWidth), maxWidth),
            height: min(naturalHeight, maxHeight)
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
        let windowFrame = NSRect(origin: origin, size: windowSize)
        if frame != windowFrame {
            setFrame(windowFrame, display: false, animate: false)
        }

        let chromeFrame = NSRect(x: chromeInset, y: chromeInset, width: panelSize.width, height: panelSize.height)
        if containerView.frame != chromeFrame || shadowCornerRadius != theme.panel.cornerRadius {
            containerView.frame = chromeFrame
            shadowCornerRadius = theme.panel.cornerRadius
            rootView.layer?.shadowPath = CGPath(roundedRect: chromeFrame, cornerWidth: theme.panel.cornerRadius,
                                                cornerHeight: theme.panel.cornerRadius, transform: nil)
        }
        let top = panelSize.height - theme.panel.paddingTop
        preeditField.frame = preeditField.frame(forAlignmentRect: NSRect(
            x: theme.panel.paddingLeft, y: top - preeditSize.height, width: preeditSize.width, height: preeditSize.height
        ))
        candidateContent.frame = NSRect(
            x: theme.panel.paddingLeft, y: top - preeditHeight - candidateSize.height,
            width: candidateSize.width, height: candidateSize.height
        )

        var offset: CGFloat = 0
        for (position, item) in visibleOptions.enumerated() {
            let (option, width) = item
            if position > 0 && visibleSeparators > 0 {
                let separator = separators[position - 1]
                separator.frame = horizontal
                    ? NSRect(x: offset.rounded(), y: ((candidateSize.height - separatorHeight) / 2).rounded(), width: 1, height: separatorHeight)
                    : NSRect(x: 0, y: candidateSize.height - offset - 1, width: candidateSize.width, height: 1)
                offset += 1 + gap
            }
            let height = alignsFirstRow ? rowHeight : optionHeights[position]
            let x = offset.rounded(.down)
            option.frame = horizontal
                ? NSRect(x: x, y: ((candidateSize.height - height) / 2).rounded(),
                         width: (offset + width).rounded(.up) - x, height: height)
                : NSRect(x: 0, y: candidateSize.height - offset - height, width: width, height: height)
            option.placeFields(alignsFirstRow: alignsFirstRow)
            offset += (horizontal ? width : height) + gap
        }

        var navX = horizontal ? offset : 0
        let navY = horizontal ? ((candidateSize.height - navHeight) / 2).rounded() : 0
        for button in navigation {
            let x = navX.rounded()
            button.frame = NSRect(x: x, y: navY,
                                  width: max(0, min(candidateSize.width, (navX + navigationSize).rounded()) - x),
                                  height: navigationSize)
            button.placeLabel()
            navX += navigationSize + gap
        }

        if !isVisible { orderFront(nil) }
    }

    private func apply(_ theme: ImeTheme) {
        let shadow = theme.panel.shadowStyle
        chromeInset = theme.panel.shadow ? (shadow?.margin ?? 0) : 0
        rootView.layer?.shadowOpacity = theme.panel.shadow ? 1 : 0
        rootView.layer?.shadowColor = shadow?.color.cgColor
        rootView.layer?.shadowOffset = CGSize(width: 0, height: -(shadow?.offsetY ?? 0))
        rootView.layer?.shadowRadius = (shadow?.blurRadius ?? 0) / 2
        containerView.layer?.backgroundColor = theme.panel.background.cgColor
        containerView.layer?.cornerRadius = theme.panel.cornerRadius
        containerView.layer?.borderColor = theme.panel.borderColor.cgColor
        containerView.layer?.borderWidth = theme.panel.borderWidth
    }

    // MARK: - Actions

    @objc private func candidateClicked(_ sender: NSControl) {
        guard isVisible, !sender.isHidden, sender.window === self, sender.isDescendant(of: candidateContent) else { return }
        onSelect?(sender.tag)
    }

    @objc private func prevPage(_ sender: NSControl) {
        guard isVisible, !sender.isHidden, sender.window === self, sender.isDescendant(of: candidateContent) else { return }
        onPageChange?(true)
    }
    @objc private func nextPage(_ sender: NSControl) {
        guard isVisible, !sender.isHidden, sender.window === self, sender.isDescendant(of: candidateContent) else { return }
        onPageChange?(false)
    }
}

private struct CandidatePanelFonts {
    private let style: ImeTheme.Font
    let preedit: NSFont
    let label: NSFont
    let text: NSFont
    let comment: NSFont
    let navigation: NSFont
    let navigationHeight: CGFloat
    let ellipsisWidth: CGFloat

    init(_ style: ImeTheme.Font) {
        self.style = style
        preedit = .keytaoThemeFont(family: style.effectiveFamily, size: style.preeditSize, weight: style.weight)
        label = .keytaoThemeFont(family: style.effectiveFamily, size: style.labelSize, weight: style.weight)
        text = .keytaoThemeFont(family: style.effectiveFamily, size: style.size, weight: style.weight)
        comment = .keytaoThemeFont(family: style.effectiveFamily, size: style.commentSize, weight: style.weight)
        navigation = .keytaoThemeFont(family: style.effectiveFamily, size: style.size * 1.3)
        let navigationLabel = NSTextField(labelWithString: "‹")
        navigationLabel.font = navigation
        navigationHeight = navigationLabel.intrinsicContentSize.height
        ellipsisWidth = ("…" as NSString).size(withAttributes: [.font: text]).width
    }

    func matches(_ other: ImeTheme.Font) -> Bool {
        style.effectiveFamily == other.effectiveFamily && style.weight == other.weight
            && style.size == other.size && style.labelSize == other.labelSize
            && style.commentSize == other.commentSize && style.preeditSize == other.preeditSize
    }
}

private final class CandidateOptionView: NSControl {
    private let labelField = NSTextField(labelWithString: "")
    private let textField = NSTextField(labelWithString: "")
    private let commentField = NSTextField(labelWithString: "")
    private var style: ImeTheme.CandidateOption
    private var isSelectedOption = false
    private var revision: UInt = 0
    private var labelSize = NSSize.zero
    private var textSize = NSSize.zero
    private var commentSize = NSSize.zero
    private var stacksComment = false
    private var hovered = false {
        didSet { applyState() }
    }
    private var pressed = false {
        didSet { applyState() }
    }

    init(style: ImeTheme.CandidateOption) {
        self.style = style
        super.init(frame: .zero)
        wantsLayer = true
        for field in [labelField, textField, commentField] {
            field.lineBreakMode = .byTruncatingTail
            addSubview(field)
        }
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.activeAlways, .mouseEnteredAndExited, .inVisibleRect],
                                       owner: self, userInfo: nil))
    }

    required init?(coder: NSCoder) {
        return nil
    }

    func update(_ candidate: KeyTaoPanelCandidate, style: ImeTheme.CandidateOption, fonts: CandidatePanelFonts) {
        revision &+= 1
        tag = candidate.index
        self.style = style
        isSelectedOption = candidate.selected
        labelSize = setText(candidate.label, font: fonts.label, field: labelField, previous: labelSize)
        textSize = setText(candidate.text, font: fonts.text, field: textField, previous: textSize)
        let comment = candidate.comment ?? ""
        commentField.isHidden = comment.isEmpty
        if !comment.isEmpty {
            commentSize = setText(comment, font: fonts.comment, field: commentField, previous: commentSize)
        }
        stacksComment = style.commentPosition == .below && !comment.isEmpty
        applyState()
    }

    private func setText(_ text: String, font: NSFont, field: NSTextField, previous: NSSize) -> NSSize {
        let fontChanged = field.font != font
        guard fontChanged || field.stringValue != text else { return previous }
        if fontChanged { field.font = font }
        field.stringValue = text
        return field.intrinsicContentSize
    }

    var naturalWidth: CGFloat {
        let gap = style.inlineGap
        let commentWidth = commentField.isHidden ? 0 : commentSize.width + gap
        let wordWidth = stacksComment ? max(textSize.width, commentSize.width) : textSize.width + commentWidth
        return labelSize.width + gap + wordWidth + style.paddingX * 2
    }

    private var baseline: CGFloat {
        max(labelField.firstBaselineOffsetFromTop, textField.firstBaselineOffsetFromTop,
            commentField.isHidden ? 0 : commentField.firstBaselineOffsetFromTop)
    }

    func naturalHeight(alignsFirstRow: Bool) -> CGFloat {
        let height: CGFloat
        if stacksComment {
            height = max(labelSize.height, textSize.height + (commentField.isHidden ? 0 : style.inlineGap * 0.5 + commentSize.height))
        } else if alignsFirstRow {
            height = max(labelSize.height, textSize.height, commentField.isHidden ? 0 : commentSize.height)
        } else {
            let descent = max(labelSize.height - labelField.firstBaselineOffsetFromTop,
                              textSize.height - textField.firstBaselineOffsetFromTop,
                              commentField.isHidden ? 0 : commentSize.height - commentField.firstBaselineOffsetFromTop)
            height = baseline + descent
        }
        return max(style.minHeight, height + style.paddingY * 2)
    }

    func minimumWidth(nearSelection: Bool, font: NSFont, ellipsisWidth: CGFloat) -> CGFloat {
        let width: CGFloat
        if nearSelection {
            let prefix = String(textField.stringValue.prefix(1))
            width = ((prefix + "…") as NSString).size(withAttributes: [.font: font]).width
        } else {
            width = ellipsisWidth
        }
        return ceil(labelSize.width + style.inlineGap + width + style.paddingX * 2)
    }

    func collapseToEllipsis() {
        textField.stringValue = "…"
        textSize = textField.intrinsicContentSize
        commentField.isHidden = true
    }

    func placeFields(alignsFirstRow: Bool) {
        let padding = style.paddingX
        let gap = style.inlineGap
        let contentWidth = max(0, bounds.width - padding * 2)
        let labelWidth = min(labelSize.width, max(0, contentWidth - gap))
        let wordX = padding + labelWidth + gap
        let wordWidth = max(0, contentWidth - labelWidth - gap)
        let textWidth: CGFloat
        let commentWidth: CGFloat
        if stacksComment {
            textWidth = min(textSize.width, wordWidth)
            commentWidth = min(commentSize.width, wordWidth)
        } else if commentField.isHidden {
            textWidth = wordWidth
            commentWidth = 0
        } else {
            textWidth = min(textSize.width, max(0, wordWidth - gap))
            commentWidth = max(0, wordWidth - textWidth - gap)
        }

        let top = bounds.height - style.paddingY
        if stacksComment || alignsFirstRow {
            place(labelField, x: padding, top: top, width: labelWidth, height: labelSize.height)
            place(textField, x: wordX, top: top, width: textWidth, height: textSize.height)
            if !commentField.isHidden {
                let commentX = stacksComment ? wordX : wordX + textWidth + gap
                let commentTop = stacksComment ? top - textSize.height - gap * 0.5 : top
                place(commentField, x: commentX, top: commentTop, width: commentWidth, height: commentSize.height)
            }
        } else {
            let baselineY = top - baseline
            place(labelField, x: padding, top: baselineY + labelField.firstBaselineOffsetFromTop,
                  width: labelWidth, height: labelSize.height)
            place(textField, x: wordX, top: baselineY + textField.firstBaselineOffsetFromTop,
                  width: textWidth, height: textSize.height)
            if !commentField.isHidden {
                place(commentField, x: wordX + textWidth + gap, top: baselineY + commentField.firstBaselineOffsetFromTop,
                      width: commentWidth, height: commentSize.height)
            }
        }
    }

    private func place(_ field: NSTextField, x: CGFloat, top: CGFloat, width: CGFloat, height: CGFloat) {
        field.frame = field.frame(forAlignmentRect: NSRect(x: x, y: top - height, width: width, height: height))
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        guard isSelectedOption, let color = style.selectedIndicatorColor, color.alpha > 0 else { return }
        let width = min(3, style.paddingX / 3, bounds.width / 3, bounds.height / 3)
        guard width > 0 else { return }
        let height = min(20, bounds.height * 0.55)
        let rect = NSRect(x: bounds.minX + width, y: bounds.midY - height / 2, width: width, height: height)
        color.nsColor.setFill()
        NSBezierPath(roundedRect: rect, xRadius: width / 2, yRadius: width / 2).fill()
    }

    override func mouseEntered(with event: NSEvent) {
        hovered = true
    }

    override func mouseExited(with event: NSEvent) {
        hovered = false
    }

    override func mouseDown(with event: NSEvent) {
        let pressedRevision = revision
        pressed = true
        defer { pressed = false }
        while let next = window?.nextEvent(matching: [.leftMouseDragged, .leftMouseUp]) {
            pressed = bounds.contains(convert(next.locationInWindow, from: nil))
            if next.type == .leftMouseUp {
                if pressed && revision == pressedRevision { sendAction(action, to: target) }
                break
            }
        }
    }

    private func applyState() {
        let background = pressed ? style.pressedBackground
            : isSelectedOption ? style.selectedBackground
            : hovered ? style.hoverBackground : style.background
        layer?.backgroundColor = background.cgColor
        layer?.cornerRadius = style.cornerRadius
        layer?.borderWidth = style.borderWidth
        layer?.borderColor = (isSelectedOption ? style.selectedBorderColor : style.borderColor).cgColor
        labelField.textColor = (isSelectedOption ? style.selectedLabelColor : style.labelColor).nsColor
        textField.textColor = (pressed ? style.pressedForeground : isSelectedOption ? style.selectedForeground : style.foreground).nsColor
        commentField.textColor = (isSelectedOption ? style.selectedCommentColor : style.commentColor).nsColor
        needsDisplay = true
    }
}

private final class CandidateNavigationButton: NSControl {
    private let label = NSTextField(labelWithString: "")
    private var style: ImeTheme.Navigation?
    private var hovered = false {
        didSet { applyState() }
    }

    init(symbol: String) {
        super.init(frame: .zero)
        wantsLayer = true
        label.stringValue = symbol
        label.alignment = .center
        addSubview(label)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.activeAlways, .mouseEnteredAndExited, .inVisibleRect],
                                       owner: self, userInfo: nil))
    }

    required init?(coder: NSCoder) {
        return nil
    }

    func update(_ style: ImeTheme.Navigation, font: NSFont) {
        self.style = style
        if label.font != font { label.font = font }
        applyState()
    }

    func placeLabel() {
        label.frame = label.frame(forAlignmentRect: bounds)
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
        guard let style else { return }
        layer?.backgroundColor = hovered ? style.hoverBackground.cgColor : NSColor.clear.cgColor
        layer?.cornerRadius = style.cornerRadius
        label.textColor = style.foreground.nsColor
    }
}
