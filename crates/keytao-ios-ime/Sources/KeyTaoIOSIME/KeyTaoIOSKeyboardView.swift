import UIKit

protocol KeyTaoIOSKeyboardViewDelegate: AnyObject {
    func keyboardView(_ view: KeyTaoIOSKeyboardView, didTrigger command: KeyTaoKeyCommand)
    func keyboardView(_ view: KeyTaoIOSKeyboardView, didSelectCandidate index: Int, global: Bool)
    func keyboardView(_ view: KeyTaoIOSKeyboardView, requestExpandedCandidates completion: @escaping ([KeyTaoCandidate]) -> Void)
    func keyboardView(_ view: KeyTaoIOSKeyboardView, requestClipboardHistory completion: @escaping ([String]) -> Void)
}

private enum KeyTaoFunctionPanelMode {
    case home
    case rime
    case selection
    case clipboard
    case emoji
}

private enum KeyTaoToolbarIcon {
    case function
    case selection
    case clipboard
    case emoji
    case back
    case settings
}

final class KeyTaoIOSKeyboardView: UIView {
    weak var delegate: KeyTaoIOSKeyboardViewDelegate?

    private struct KeyRect {
        var spec: KeyTaoKeySpec
        var rect: CGRect
    }

    private struct ActiveRowSpan {
        var weight: CGFloat
        var remainingRows: Int
    }

    private struct CandidateRect {
        var identifierIndex: Int
        var selectIndex: Int
        var rect: CGRect
        var global: Bool
        var command: KeyTaoKeyCommand?
    }

    private struct CandidateDrawItem {
        var identifierIndex: Int
        var selectIndex: Int
        var label: String
        var text: String
        var comment: String?
        var selected: Bool
        var global: Bool
        var command: KeyTaoKeyCommand?
    }

    private struct ToolbarAction {
        var label: String
        var command: KeyTaoKeyCommand
        var selected: Bool = false
        var secondaryLabel: String?
        var icon: KeyTaoToolbarIcon?
    }

    private struct ToolbarRect {
        var action: ToolbarAction
        var rect: CGRect
    }

    private var config: KeyTaoIOSImeConfig
    private var theme: KeyTaoImeTheme
    private var state: KeyTaoImeState
    private var availabilityMessage: String?
    private var layerMode: KeyTaoKeyboardLayer = .letters
    private var shiftState: KeyTaoShiftState = .off
    private var showsInputModeSwitchKey = true
    private var lastShiftTap = Date.distantPast
    private var functionPanelActive = false
    private var functionPanelMode: KeyTaoFunctionPanelMode = .home
    private var expandedCandidates: [KeyTaoCandidate] = []
    private var expandedCandidatesLoading = false
    private var clipboardItemsLoading = false
    private var clipboardItems: [String] = []
    private var expandedCandidateScrollY: CGFloat = 0
    private var expandedCandidateContentHeight: CGFloat = 0
    private var expandRequestToken = 0

    private var keyRects: [KeyRect] = []
    private var inlineCandidateRects: [CandidateRect] = []
    private var expandedCandidateRects: [CandidateRect] = []
    private var candidateRects: [CandidateRect] {
        inlineCandidateRects + expandedCandidateRects
    }
    private var toolbarRects: [ToolbarRect] = []
    private var candidateExpandRect: CGRect?
    private var candidatePanelExpanded = false
    private var pressedKey: KeyRect?
    private var pressedToolbar: ToolbarRect?
    private var pressedCandidate: CandidateRect?
    private var expandedTouchActive = false
    private var expandedDragging = false
    private var candidateExpandPressed = false
    private var touchStart: CGPoint = .zero
    private var currentTouchPoint: CGPoint = .zero
    private var touchStartScrollY: CGFloat = 0
    private var longPressConsumed = false
    private var backspaceGestureUnits = 0
    private var backspaceGestureConsumed = false
    private var pendingExpandedCandidateWorkItem: DispatchWorkItem?
    private var longPressWorkItem: DispatchWorkItem?
    private var repeatTimer: Timer?
    private let hapticGenerator = UIImpactFeedbackGenerator(style: .light)
    private lazy var logoImage = Self.loadLogoImage()

    init(config: KeyTaoIOSImeConfig, theme: KeyTaoImeTheme, state: KeyTaoImeState) {
        self.config = config
        self.theme = theme
        self.state = state
        super.init(frame: .zero)
        setup()
    }

    required init?(coder: NSCoder) {
        self.config = .fallback
        self.theme = .fallback
        self.state = .empty
        super.init(coder: coder)
        setup()
    }

    var preferredHeight: CGFloat {
        config.keyboardHeightDp + config.candidateBarHeightDp
    }

    func update(config: KeyTaoIOSImeConfig) {
        self.config = config
        resetExpandedCandidateScroll()
        invalidateLayoutAndDisplay()
    }

    func update(theme: KeyTaoImeTheme) {
        self.theme = theme
        invalidateLayoutAndDisplay()
    }

    func update(state: KeyTaoImeState) {
        if candidateSignature(state) != candidateSignature(self.state) {
            cancelExpandedCandidateRequest()
            expandedCandidates = []
            resetExpandedCandidateScroll()
        }
        if state.candidatePanel.candidates.isEmpty && !functionPanelActive {
            candidatePanelExpanded = false
            expandedCandidates = []
            expandedCandidatesLoading = false
        }
        self.state = state
        invalidateLayoutAndDisplay()
    }

    func updateAvailability(message: String?) {
        availabilityMessage = message
        invalidateLayoutAndDisplay()
    }

    func updateInputModeSwitchKey(visible: Bool) {
        showsInputModeSwitchKey = visible
        invalidateLayoutAndDisplay()
    }

    func toggleShift() {
        let now = Date()
        switch shiftState {
        case .off:
            lastShiftTap = now
            shiftState = .once
        case .once:
            let doubleTap = now.timeIntervalSince(lastShiftTap) <= 0.35
            lastShiftTap = .distantPast
            shiftState = doubleTap ? .locked : .off
        case .locked:
            lastShiftTap = .distantPast
            shiftState = .off
        }
        invalidateLayoutAndDisplay()
    }

    func clearOneShotShift(after command: KeyTaoKeyCommand) {
        guard shiftState == .once else {
            return
        }
        guard command.type == KeyTaoCommandType.input,
              let value = command.value,
              value.count == 1,
              value.range(of: "[A-Za-z]", options: .regularExpression) != nil else {
            return
        }
        shiftState = .off
        invalidateLayoutAndDisplay()
    }

    func setLayer(_ value: String?) {
        layerMode = config.normalizedLayer(value)
        candidatePanelExpanded = false
        functionPanelActive = false
        functionPanelMode = .home
        expandedCandidates = []
        cancelExpandedCandidateRequest()
        clipboardItemsLoading = false
        resetExpandedCandidateScroll()
        shiftState = .off
        pressedKey = nil
        pressedToolbar = nil
        invalidateLayoutAndDisplay()
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        rebuildInteractiveRects()
        rebuildAccessibilityElements()
    }

    override func draw(_ rect: CGRect) {
        rebuildInteractiveRects()
        drawBackground()
        drawCandidateBar()
        if candidatePanelExpanded {
            drawExpandedCandidatePanel()
        } else {
            drawKeyboard()
        }
    }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        guard let point = touches.first?.location(in: self) else {
            return
        }
        stopLongPressAndRepeat()
        touchStart = point
        currentTouchPoint = point
        touchStartScrollY = expandedCandidateScrollY
        expandedDragging = false
        longPressConsumed = false
        backspaceGestureUnits = 0
        backspaceGestureConsumed = false
        candidateExpandPressed = !functionPanelActive
            && !state.candidatePanel.candidates.isEmpty
            && point.y < config.candidateBarHeightDp
            && candidateExpandRect?.contains(point) == true
        pressedToolbar = point.y < config.candidateBarHeightDp ? toolbarRects.first { $0.rect.contains(point) } : nil
        pressedCandidate = nil
        expandedTouchActive = false
        if pressedToolbar == nil && !candidateExpandPressed && point.y < config.candidateBarHeightDp {
            pressedCandidate = inlineCandidateRects.first { $0.rect.contains(point) }
        } else if pressedToolbar == nil && !candidateExpandPressed && candidatePanelExpanded && point.y >= config.candidateBarHeightDp {
            expandedTouchActive = true
            pressedCandidate = expandedCandidateRects.first { $0.rect.contains(point) }
        }
        if pressedToolbar == nil && pressedCandidate == nil && !candidateExpandPressed && !expandedTouchActive {
            pressedKey = keyRects.first { $0.rect.contains(point) }
            scheduleLongPressIfNeeded()
        }
        setNeedsDisplay()
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) {
        guard let point = touches.first?.location(in: self) else {
            return
        }
        currentTouchPoint = point
        if expandedTouchActive {
            let deltaY = point.y - touchStart.y
            if !expandedDragging && abs(deltaY) > 6 {
                expandedDragging = true
                pressedCandidate = nil
            }
            if expandedDragging {
                expandedCandidateScrollY = max(0, min(maxExpandedCandidateScroll(), touchStartScrollY - deltaY))
                invalidateLayoutAndDisplay()
            }
            return
        }
        if handleBackspaceDrag(at: point) {
            return
        }
        if let key = pressedKey, !key.rect.contains(point) {
            stopLongPressAndRepeat()
        }
    }

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
        stopLongPressAndRepeat()
        guard let point = touches.first?.location(in: self) else {
            clearPressedState()
            return
        }
        currentTouchPoint = point

        if candidateExpandPressed,
           let expand = candidateExpandRect,
           expand.contains(point),
           expand.contains(touchStart) {
            toggleCandidatePanel()
            performConfiguredHaptic()
            clearPressedState()
            invalidateLayoutAndDisplay()
            return
        }

        if let toolbar = pressedToolbar, toolbar.rect.contains(point) {
            clearPressedState()
            handleToolbarCommand(toolbar.action.command)
            return
        }

        if let candidate = pressedCandidate, !expandedDragging, candidate.rect.contains(point) {
            clearPressedState()
            if let command = candidate.command {
                handlePanelCommand(command)
            } else {
                closeCandidatePanelIfNeeded(afterCandidateSelection: candidate.global)
                performConfiguredHaptic()
                delegate?.keyboardView(self, didSelectCandidate: candidate.selectIndex, global: candidate.global)
            }
            return
        }

        if let key = pressedKey, handleBackspaceRelease(for: key, at: point) {
            clearPressedState()
            invalidateLayoutAndDisplay()
            return
        }

        if backspaceGestureConsumed {
            clearPressedState()
            invalidateLayoutAndDisplay()
            return
        }

        if let key = pressedKey, !longPressConsumed {
            let command = resolveCommand(
                key.spec,
                deltaY: point.y - touchStart.y,
                rect: key.rect,
                releaseY: point.y
            )
            clearPressedState()
            performConfiguredHaptic()
            delegate?.keyboardView(self, didTrigger: command)
            clearOneShotShift(after: command)
            return
        }

        clearPressedState()
    }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) {
        stopLongPressAndRepeat()
        clearPressedState()
    }

    private func setup() {
        isOpaque = false
        backgroundColor = .clear
        isAccessibilityElement = false
        isMultipleTouchEnabled = true
        contentMode = .redraw
        hapticGenerator.prepare()
    }

    private func invalidateLayoutAndDisplay() {
        backgroundColor = .clear
        rebuildInteractiveRects()
        rebuildAccessibilityElements()
        setNeedsDisplay()
        invalidateIntrinsicContentSize()
    }

    private func clearPressedState() {
        pressedKey = nil
        pressedToolbar = nil
        pressedCandidate = nil
        candidateExpandPressed = false
        expandedTouchActive = false
        expandedDragging = false
        backspaceGestureUnits = 0
        backspaceGestureConsumed = false
        setNeedsDisplay()
    }

    private func rebuildInteractiveRects() {
        keyRects = keyboardLayout()
        inlineCandidateRects = inlineCandidateLayout()
        expandedCandidateRects = candidatePanelExpanded ? expandedCandidateLayout() : []
        toolbarRects = toolbarLayout()
        candidateExpandRect = expandButtonRect()
    }

    private func rebuildAccessibilityElements() {
        var elements: [UIAccessibilityElement] = []
        for key in keyRects {
            let element = UIAccessibilityElement(accessibilityContainer: self)
            element.accessibilityFrameInContainerSpace = key.rect
            element.accessibilityTraits = .button
            element.accessibilityIdentifier = keyAccessibilityIdentifier(key.spec)
            element.accessibilityLabel = displayLabel(key.spec)
            elements.append(element)
        }
        for candidate in candidateRects {
            let element = UIAccessibilityElement(accessibilityContainer: self)
            element.accessibilityFrameInContainerSpace = candidate.rect
            element.accessibilityTraits = .button
            element.accessibilityIdentifier = "keytao-candidate-\(candidate.identifierIndex)"
            elements.append(element)
        }
        for toolbar in toolbarRects {
            let element = UIAccessibilityElement(accessibilityContainer: self)
            element.accessibilityFrameInContainerSpace = toolbar.rect
            element.accessibilityTraits = .button
            element.accessibilityIdentifier = commandAccessibilityIdentifier(toolbar.action.command, prefix: "keytao-toolbar")
            element.accessibilityLabel = toolbar.action.label
            elements.append(element)
        }
        accessibilityElements = elements
    }

    private func drawBackground() {
        // The root UIInputView(.keyboard) supplies the system keyboard material.
    }

    private func drawCandidateBar() {
        let barHeight = config.candidateBarHeightDp
        let leftPadding = theme.panel.gap * 1.5
        let message = availabilityMessage?.isEmpty == false ? availabilityMessage : nil
        if message != nil && state.candidatePanel.candidates.isEmpty && state.candidatePanel.preedit?.isEmpty != false {
            drawText(
                message ?? "请先在 KeyTao App 安装键道方案",
                in: CGRect(x: leftPadding, y: 0, width: bounds.width - leftPadding * 2, height: barHeight),
                color: statusMessageColor(),
                size: theme.font.preeditSize,
                weight: theme.font.weight,
                alignment: .left
            )
            return
        }

        if functionPanelActive {
            drawFunctionPanelBar()
            return
        }

        if !state.candidatePanel.candidates.isEmpty {
            for candidate in candidateDrawItems(inlineOnly: true) {
                guard let rect = inlineCandidateRects.first(where: { $0.identifierIndex == candidate.identifierIndex })?.rect else {
                    continue
                }
                drawCandidateOption(candidate, rect: rect)
            }
            if let expand = candidateExpandRect {
                drawExpandButton(expand)
            }
            return
        }

        let preedit = state.candidatePanel.preedit ?? state.preedit
        if !preedit.isEmpty {
            drawText(
                preedit,
                in: CGRect(x: leftPadding, y: 0, width: bounds.width - leftPadding * 2 - 36, height: barHeight),
                color: theme.candidate.labelColor.uiColor,
                size: theme.font.preeditSize,
                weight: theme.font.weight,
                alignment: .left
            )
            drawLogo(in: logoRect())
            return
        }

        for toolbar in toolbarRects {
            drawToolbarChip(toolbar)
        }
        drawLogo(in: logoRect())
    }

    private func drawKeyboard() {
        for key in keyRects {
            let pressed = pressedKey?.spec == key.spec
            drawKey(key.spec, rect: key.rect, pressed: pressed, pressedStackIndex: pressedStackIndex(for: key))
        }
    }

    private func drawExpandedCandidatePanel() {
        guard let context = UIGraphicsGetCurrentContext() else {
            return
        }
        let top = config.candidateBarHeightDp
        let panelRect = CGRect(x: 0, y: top, width: bounds.width, height: keyboardBottom() - top)
        context.setStrokeColor(theme.panel.borderColor.uiColor.cgColor)
        context.setLineWidth(max(1, pixel))
        context.move(to: CGPoint(x: 0, y: top))
        context.addLine(to: CGPoint(x: bounds.width, y: top))
        context.strokePath()

        let items = expandedCandidateItems()
        if items.isEmpty {
            drawText(
                expandedPanelEmptyMessage(),
                in: panelRect,
                color: theme.candidate.commentColor.uiColor,
                size: theme.font.labelSize,
                weight: theme.font.weight,
                alignment: .center
            )
            return
        }

        for item in items {
            guard let rect = expandedCandidateRects.first(where: { $0.identifierIndex == item.identifierIndex })?.rect else {
                continue
            }
            drawCandidateOption(item, rect: rect)
        }
    }

    private func drawFunctionPanelBar() {
        for toolbar in toolbarRects {
            drawToolbarChip(toolbar)
        }
        drawText(
            functionPanelTitle(),
            in: CGRect(x: 0, y: 0, width: bounds.width, height: config.candidateBarHeightDp),
            color: theme.candidate.commentColor.uiColor,
            size: theme.font.labelSize,
            weight: theme.font.weight,
            alignment: .center
        )

        if expandedCandidatesLoading || clipboardItemsLoading {
            let width: CGFloat = 44
            let rect = CGRect(
                x: (bounds.width - width) / 2,
                y: config.candidateBarHeightDp - 3,
                width: width,
                height: 2
            )
            theme.candidate.selectedLabelColor.uiColor.setFill()
            UIBezierPath(roundedRect: rect, cornerRadius: 1).fill()
        }
    }

    private func drawExpandButton(_ rect: CGRect) {
        let pressed = candidateExpandPressed
        drawSurfaceShadow(rect, pressed: pressed)
        keyBackgroundColor(nil, selected: pressed).setFill()
        UIBezierPath(roundedRect: rect, cornerRadius: keyCornerRadius(for: rect)).fill()
        drawText(
            candidatePanelExpanded ? "⌃" : "⌄",
            in: rect,
            color: pressed ? theme.candidate.selectedForeground.uiColor : theme.candidate.foreground.uiColor,
            size: theme.font.size,
            weight: theme.font.weight,
            alignment: .center
        )
    }

    private func drawCandidateOption(_ item: CandidateDrawItem, rect: CGRect) {
        let selected = item.selected
        if selected {
            drawSurfaceShadow(rect, pressed: false, cornerRadius: candidateCornerRadius())
        }
        (selected ? theme.candidate.selectedBackground.uiColor : keyBackgroundColor()).setFill()
        UIBezierPath(roundedRect: rect, cornerRadius: candidateCornerRadius()).fill()

        let borderWidth = selected ? max(theme.candidate.borderWidth, 1) : theme.candidate.borderWidth
        if borderWidth > 0 {
            let path = UIBezierPath(roundedRect: rect.insetBy(dx: borderWidth / 2, dy: borderWidth / 2), cornerRadius: candidateCornerRadius())
            path.lineWidth = borderWidth
            (selected ? theme.candidate.selectedBorderColor.uiColor : theme.candidate.borderColor.uiColor).setStroke()
            path.stroke()
        }

        if selected {
            theme.candidate.selectedLabelColor.uiColor.setFill()
            UIBezierPath(
                roundedRect: CGRect(x: rect.minX + 5, y: rect.minY + 6, width: 3, height: rect.height - 12),
                cornerRadius: 2
            ).fill()
        }

        let paddingX = candidatePaddingX()
        let inlineGap = candidateInlineGap()
        var x = rect.minX + paddingX + (selected ? 4 : 0)
        let centerY = rect.midY
        if !item.label.isEmpty {
            x += drawInlineText(
                item.label,
                x: x,
                centerY: centerY,
                color: selected ? theme.candidate.selectedLabelColor.uiColor : theme.candidate.labelColor.uiColor,
                size: candidateLabelSize(),
                weight: theme.font.weight
            ) + inlineGap
        }
        x += drawInlineText(
            item.text,
            x: x,
            centerY: centerY,
            color: selected ? theme.candidate.selectedForeground.uiColor : theme.candidate.foreground.uiColor,
            size: candidateTextSize(),
            weight: theme.font.weight
        ) + inlineGap
        if let comment = item.comment, !comment.isEmpty {
            _ = drawInlineText(
                comment,
                x: x,
                centerY: centerY,
                color: selected ? theme.candidate.selectedCommentColor.uiColor : theme.candidate.commentColor.uiColor,
                size: candidateCommentSize(),
                weight: theme.font.weight
            )
        }
    }

    private func drawToolbarChip(_ item: ToolbarRect) {
        let pressed = pressedToolbar?.action.command == item.action.command && pressedToolbar?.action.label == item.action.label
        drawSurfaceShadow(item.rect, pressed: pressed)
        toolbarBackgroundColor(item.action, pressed: pressed).setFill()
        UIBezierPath(roundedRect: item.rect, cornerRadius: keyCornerRadius(for: item.rect)).fill()

        if item.action.selected {
            let path = UIBezierPath(roundedRect: item.rect.insetBy(dx: 0.5, dy: 0.5), cornerRadius: keyCornerRadius(for: item.rect))
            path.lineWidth = max(theme.candidate.borderWidth, 1)
            theme.candidate.selectedBorderColor.uiColor.setStroke()
            path.stroke()
        }

        if let secondary = item.action.secondaryLabel, !secondary.isEmpty {
            drawToolbarPair(primary: item.action.label, secondary: secondary, rect: item.rect, pressed: pressed)
        } else if let icon = item.action.icon {
            let color = pressed || item.action.selected
                ? theme.candidate.selectedForeground.uiColor
                : theme.candidate.foreground.uiColor
            drawToolbarIcon(icon, in: item.rect, color: color)
        } else {
            drawText(
                item.action.label,
                in: item.rect,
                color: pressed ? theme.candidate.selectedForeground.uiColor : theme.candidate.foreground.uiColor,
                font: fittedFont(for: item.action.label, size: theme.font.labelSize, maxWidth: item.rect.width - 10),
                alignment: .center
            )
        }
    }

    private func drawToolbarPair(primary: String, secondary: String, rect: CGRect, pressed: Bool) {
        var primarySize = theme.font.labelSize
        var secondarySize = theme.font.commentSize
        var primaryFont = themedFont(size: primarySize, weight: theme.font.weight)
        var secondaryFont = themedFont(size: secondarySize, weight: theme.font.weight)
        let primaryWidth = primary.size(withAttributes: [.font: primaryFont]).width
        let secondaryWidth = secondary.size(withAttributes: [.font: secondaryFont]).width
        let gap: CGFloat = 5
        let total = primaryWidth + secondaryWidth + gap
        let maxWidth = max(1, rect.width - 10)
        if total > maxWidth {
            let scale = max(0.78, maxWidth / total)
            primarySize *= scale
            secondarySize *= scale
            primaryFont = themedFont(size: primarySize, weight: theme.font.weight)
            secondaryFont = themedFont(size: secondarySize, weight: theme.font.weight)
        }
        let fittedPrimaryWidth = primary.size(withAttributes: [.font: primaryFont]).width
        let fittedSecondaryWidth = secondary.size(withAttributes: [.font: secondaryFont]).width
        let fittedTotal = fittedPrimaryWidth + fittedSecondaryWidth + gap
        let y1 = rect.midY - primaryFont.lineHeight / 2
        let y2 = rect.midY - secondaryFont.lineHeight / 2
        let x1 = rect.midX - fittedTotal / 2
        let x2 = x1 + fittedPrimaryWidth + gap
        let primaryColor = pressed ? theme.candidate.selectedForeground.uiColor : theme.candidate.foreground.uiColor
        let secondaryColor = pressed ? theme.candidate.selectedForeground.uiColor : theme.candidate.commentColor.uiColor
        primary.draw(at: CGPoint(x: x1, y: y1), withAttributes: [.font: primaryFont, .foregroundColor: primaryColor])
        secondary.draw(at: CGPoint(x: x2, y: y2), withAttributes: [.font: secondaryFont, .foregroundColor: secondaryColor])
    }

    private func drawToolbarIcon(_ icon: KeyTaoToolbarIcon, in rect: CGRect, color: UIColor) {
        guard let context = UIGraphicsGetCurrentContext() else {
            return
        }
        let size = max(14, min(21, rect.width - 16, rect.height - 11))
        let iconRect = CGRect(x: rect.midX - size / 2, y: rect.midY - size / 2, width: size, height: size)
        context.saveGState()
        context.setStrokeColor(color.cgColor)
        context.setFillColor(color.cgColor)
        context.setLineWidth(max(1.7, size * 0.095))
        context.setLineCap(.round)
        context.setLineJoin(.round)

        switch icon {
        case .function:
            drawGridIcon(in: iconRect)
        case .selection:
            drawSelectionIcon(in: iconRect)
        case .clipboard:
            drawClipboardIcon(in: iconRect)
        case .emoji:
            drawEmojiIcon(in: iconRect)
        case .back:
            drawBackIcon(in: iconRect)
        case .settings:
            drawSettingsIcon(in: iconRect)
        }
        context.restoreGState()
    }

    private func drawGridIcon(in rect: CGRect) {
        let cell = rect.width * 0.34
        let gap = rect.width - cell * 2
        for row in 0..<2 {
            for column in 0..<2 {
                let x = rect.minX + CGFloat(column) * (cell + gap)
                let y = rect.minY + CGFloat(row) * (cell + gap)
                UIBezierPath(
                    roundedRect: CGRect(x: x, y: y, width: cell, height: cell),
                    cornerRadius: cell * 0.22
                ).stroke()
            }
        }
    }

    private func drawSelectionIcon(in rect: CGRect) {
        let path = UIBezierPath()
        path.move(to: CGPoint(x: rect.minX + rect.width * 0.24, y: rect.minY + rect.height * 0.12))
        path.addLine(to: CGPoint(x: rect.minX + rect.width * 0.24, y: rect.maxY - rect.height * 0.14))
        path.addLine(to: CGPoint(x: rect.minX + rect.width * 0.42, y: rect.minY + rect.height * 0.66))
        path.addLine(to: CGPoint(x: rect.minX + rect.width * 0.54, y: rect.maxY - rect.height * 0.10))
        path.addLine(to: CGPoint(x: rect.minX + rect.width * 0.68, y: rect.maxY - rect.height * 0.18))
        path.addLine(to: CGPoint(x: rect.minX + rect.width * 0.56, y: rect.minY + rect.height * 0.58))
        path.addLine(to: CGPoint(x: rect.maxX - rect.width * 0.20, y: rect.minY + rect.height * 0.58))
        path.close()
        path.stroke()
    }

    private func drawClipboardIcon(in rect: CGRect) {
        let body = CGRect(x: rect.minX + rect.width * 0.2, y: rect.minY + rect.height * 0.16, width: rect.width * 0.6, height: rect.height * 0.72)
        UIBezierPath(roundedRect: body, cornerRadius: rect.width * 0.1).stroke()
        let clip = CGRect(x: rect.minX + rect.width * 0.36, y: rect.minY + rect.height * 0.08, width: rect.width * 0.28, height: rect.height * 0.18)
        UIBezierPath(roundedRect: clip, cornerRadius: rect.width * 0.06).stroke()
        let line = UIBezierPath()
        line.move(to: CGPoint(x: body.minX + body.width * 0.22, y: body.midY))
        line.addLine(to: CGPoint(x: body.maxX - body.width * 0.22, y: body.midY))
        line.stroke()
    }

    private func drawEmojiIcon(in rect: CGRect) {
        UIBezierPath(ovalIn: rect.insetBy(dx: rect.width * 0.08, dy: rect.height * 0.08)).stroke()
        UIBezierPath(ovalIn: CGRect(x: rect.minX + rect.width * 0.32, y: rect.minY + rect.height * 0.36, width: rect.width * 0.07, height: rect.height * 0.07)).fill()
        UIBezierPath(ovalIn: CGRect(x: rect.maxX - rect.width * 0.39, y: rect.minY + rect.height * 0.36, width: rect.width * 0.07, height: rect.height * 0.07)).fill()
        let smile = UIBezierPath()
        smile.move(to: CGPoint(x: rect.minX + rect.width * 0.32, y: rect.minY + rect.height * 0.62))
        smile.addQuadCurve(
            to: CGPoint(x: rect.maxX - rect.width * 0.32, y: rect.minY + rect.height * 0.62),
            controlPoint: CGPoint(x: rect.midX, y: rect.maxY - rect.height * 0.18)
        )
        smile.stroke()
    }

    private func drawBackIcon(in rect: CGRect) {
        let path = UIBezierPath()
        path.move(to: CGPoint(x: rect.maxX - rect.width * 0.15, y: rect.midY))
        path.addLine(to: CGPoint(x: rect.minX + rect.width * 0.18, y: rect.midY))
        path.move(to: CGPoint(x: rect.minX + rect.width * 0.18, y: rect.midY))
        path.addLine(to: CGPoint(x: rect.minX + rect.width * 0.42, y: rect.minY + rect.height * 0.26))
        path.move(to: CGPoint(x: rect.minX + rect.width * 0.18, y: rect.midY))
        path.addLine(to: CGPoint(x: rect.minX + rect.width * 0.42, y: rect.maxY - rect.height * 0.26))
        path.stroke()
    }

    private func drawSettingsIcon(in rect: CGRect) {
        let rows: [(CGFloat, CGFloat)] = [(0.28, 0.65), (0.5, 0.34), (0.72, 0.58)]
        for (yRatio, knobRatio) in rows {
            let y = rect.minY + rect.height * yRatio
            let path = UIBezierPath()
            path.move(to: CGPoint(x: rect.minX + rect.width * 0.14, y: y))
            path.addLine(to: CGPoint(x: rect.maxX - rect.width * 0.14, y: y))
            path.stroke()
            let knobRadius = rect.width * 0.085
            let knob = CGRect(
                x: rect.minX + rect.width * knobRatio - knobRadius,
                y: y - knobRadius,
                width: knobRadius * 2,
                height: knobRadius * 2
            )
            UIBezierPath(ovalIn: knob).fill()
        }
    }

    private func drawLogo(in rect: CGRect) {
        guard !rect.isEmpty else {
            return
        }
        if let logoImage {
            logoImage.draw(in: rect, blendMode: .normal, alpha: 0.86)
            return
        }
        let color = theme.candidate.selectedLabelColor.uiColor.withAlphaComponent(0.86)
        color.setFill()
        UIBezierPath(ovalIn: rect).fill()
        drawText(
            "K",
            in: rect,
            color: theme.candidate.selectedForeground.uiColor,
            size: theme.font.commentSize,
            weight: theme.font.weight,
            alignment: .center
        )
    }

    private func drawKey(_ key: KeyTaoKeySpec, rect: CGRect, pressed: Bool, pressedStackIndex: Int? = nil) {
        if let stack = key.stack, !stack.isEmpty {
            drawStackKey(stack, key: key, rect: rect, pressedStackIndex: pressedStackIndex)
            return
        }

        var keyRect = rect
        if pressed {
            keyRect.origin.y += 1
        }
        let selected = pressed || isActiveKey(key)
        drawSurfaceShadow(keyRect, pressed: pressed)
        keyBackgroundColor(key, selected: selected).setFill()
        UIBezierPath(roundedRect: keyRect, cornerRadius: keyCornerRadius(for: keyRect)).fill()
        drawKeyOutline(key, rect: keyRect, pressed: pressed)

        let label = displayLabel(key)
        let baseSize = keyLabelSize(for: label)
        let font = fittedFont(for: label, size: baseSize, maxWidth: keyRect.width - 10)
        let color = keyForegroundColor(key, selected: selected)
        drawText(label, in: keyRect, color: color, font: font, alignment: .center)

        if let hint = key.hint, !hint.isEmpty {
            let hintFont = themedFont(size: keyHintSize(), weight: .regular)
            let attributes: [NSAttributedString.Key: Any] = [
                .font: hintFont,
                .foregroundColor: theme.candidate.commentColor.uiColor,
            ]
            let size = hint.size(withAttributes: attributes)
            hint.draw(
                at: CGPoint(x: keyRect.maxX - size.width - 7, y: keyRect.minY + 4),
                withAttributes: attributes
            )
        }
    }

    private func drawStackKey(_ stack: [KeyTaoKeyStackItem], key: KeyTaoKeySpec, rect: CGRect, pressedStackIndex: Int?) {
        let itemRects = stackItemRects(in: rect, count: stack.count)
        for (index, item) in stack.enumerated() {
            let pressed = pressedStackIndex == index
            var itemRect = itemRects[index]
            if pressed {
                itemRect.origin.y += 1
            }
            let selected = pressed || isActiveKey(key)
            drawSurfaceShadow(itemRect, pressed: pressed)
            keyBackgroundColor(key, selected: selected).setFill()
            UIBezierPath(roundedRect: itemRect, cornerRadius: keyCornerRadius(for: itemRect)).fill()
            drawKeyOutline(key, rect: itemRect, pressed: pressed)

            let label = stackLabelForMode(item)
            let baseSize = keyLabelSize(for: label)
            let font = fittedFont(for: label, size: baseSize, maxWidth: itemRect.width - 10)
            let color = keyForegroundColor(key, selected: selected)
            drawText(label, in: itemRect, color: color, font: font, alignment: .center)
        }
    }

    private func drawKeyOutline(_ key: KeyTaoKeySpec, rect: CGRect, pressed: Bool) {
        guard !pressed else {
            return
        }
        let outline = rect.insetBy(dx: 1, dy: 1)
        let path = UIBezierPath(roundedRect: outline, cornerRadius: max(0, keyCornerRadius(for: rect) - 1))
        path.lineWidth = max(1, 0.7)
        if isSoftAccentKey(key) {
            theme.candidate.selectedLabelColor.uiColor.withAlphaComponent(isDarkPanel() ? 0.28 : 0.18).setStroke()
        } else if isDarkPanel() {
            UIColor.white.withAlphaComponent(0.09).setStroke()
        } else {
            UIColor(red: 26 / 255, green: 34 / 255, blue: 44 / 255, alpha: 0.11).setStroke()
        }
        path.stroke()
    }

    private func drawSurfaceShadow(_ rect: CGRect, pressed: Bool, cornerRadius: CGFloat? = nil) {
        var shadow = rect
        shadow.origin.y += pressed ? 0.8 : 2.4
        UIColor(red: 26 / 255, green: 34 / 255, blue: 44 / 255, alpha: pressed ? 0.05 : 0.09).setFill()
        UIBezierPath(roundedRect: shadow, cornerRadius: cornerRadius ?? keyCornerRadius(for: rect)).fill()
    }

    private func drawText(_ text: String, in rect: CGRect, color: UIColor, size: CGFloat, weight: KeyTaoThemeFontWeight, alignment: NSTextAlignment) {
        drawText(text, in: rect, color: color, font: themedFont(size: size, weight: weight), alignment: alignment)
    }

    private func drawText(_ text: String, in rect: CGRect, color: UIColor, font: UIFont, alignment: NSTextAlignment) {
        let paragraph = NSMutableParagraphStyle()
        paragraph.alignment = alignment
        let attributes: [NSAttributedString.Key: Any] = [
            .font: font,
            .foregroundColor: color,
            .paragraphStyle: paragraph,
        ]
        let size = text.size(withAttributes: attributes)
        let x: CGFloat
        switch alignment {
        case .left:
            x = rect.minX
        case .right:
            x = rect.maxX - size.width
        default:
            x = rect.midX - size.width / 2
        }
        text.draw(at: CGPoint(x: x, y: rect.midY - size.height / 2), withAttributes: attributes)
    }

    private func drawInlineText(_ text: String, x: CGFloat, centerY: CGFloat, color: UIColor, size: CGFloat, weight: KeyTaoThemeFontWeight) -> CGFloat {
        let font = themedFont(size: size, weight: weight)
        let attributes: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: color]
        let textSize = text.size(withAttributes: attributes)
        text.draw(at: CGPoint(x: x, y: centerY - textSize.height / 2), withAttributes: attributes)
        return textSize.width
    }

    private func keyboardLayout() -> [KeyRect] {
        let rows = activeRows()
        guard !rows.isEmpty, bounds.width > 0, bounds.height > 0 else {
            return []
        }
        let top = keyboardTop()
        let bottom = keyboardBottom()
        let horizontalGap = keyboardHorizontalGap()
        let verticalGapFloor = keyboardVerticalGap()
        let rowCount = CGFloat(rows.count)
        let availableHeight = max(0, bottom - top)
        let naturalRowHeight = max(36, (availableHeight - verticalGapFloor * (rowCount + 1)) / rowCount)
        let rowHeight = min(naturalRowHeight, keyboardMaxKeyHeight())
        let verticalGap = max(verticalGapFloor, (availableHeight - rowHeight * rowCount) / (rowCount + 1))
        var y = top + verticalGap
        var next: [KeyRect] = []
        let maximumRowWidth = max(1, bounds.width - keyboardOuterInset() * 2)
        let referenceUnitWidth = keyboardReferenceUnitWidth(rows: rows, horizontalGap: horizontalGap)
        var activeLeadingSpans: [ActiveRowSpan] = []
        for (rowIndex, row) in rows.enumerated() {
            guard !row.isEmpty else {
                activeLeadingSpans = advanceRowSpans(activeLeadingSpans)
                y += rowHeight + verticalGap
                continue
            }
            let leadingWeight = activeLeadingSpans.reduce(CGFloat(0)) { $0 + $1.weight }
            let totalWeight = max(1, leadingWeight + rowWeight(row))
            let effectiveKeyCount = activeLeadingSpans.count + row.count
            let gapWidth = horizontalGap * CGFloat(max(0, effectiveKeyCount - 1))
            let rowWidth = keyboardRowWidth(
                row,
                rowIndex: rowIndex,
                rows: rows,
                referenceUnitWidth: referenceUnitWidth,
                horizontalGap: horizontalGap,
                maximumRowWidth: maximumRowWidth,
                effectiveKeyCount: effectiveKeyCount,
                effectiveWeight: totalWeight
            )
            let unitWidth = max(1, (rowWidth - gapWidth) / totalWeight)
            var x = (bounds.width - rowWidth) / 2
            for span in activeLeadingSpans {
                x += unitWidth * span.weight + horizontalGap
            }
            var nextLeadingSpans: [ActiveRowSpan] = []
            var acceptingLeadingSpan = true
            for key in row {
                let width = unitWidth * keyWeight(key)
                let spanRows = keyRowSpan(key)
                let height = rowHeight * CGFloat(spanRows) + verticalGap * CGFloat(spanRows - 1)
                let rect = CGRect(x: x, y: y, width: width, height: height)
                next.append(KeyRect(spec: key, rect: rect))
                if acceptingLeadingSpan && spanRows > 1 {
                    nextLeadingSpans.append(ActiveRowSpan(weight: keyWeight(key), remainingRows: spanRows - 1))
                } else {
                    acceptingLeadingSpan = false
                }
                x = rect.maxX + horizontalGap
            }
            activeLeadingSpans = advanceRowSpans(activeLeadingSpans)
            activeLeadingSpans.append(contentsOf: nextLeadingSpans)
            y += rowHeight + verticalGap
        }
        return next
    }

    private func inlineCandidateLayout() -> [CandidateRect] {
        guard !state.candidatePanel.candidates.isEmpty else {
            return []
        }
        let barHeight = config.candidateBarHeightDp
        let gap = theme.panel.gap
        let leftPadding = gap * 1.5
        let expand = expandButtonRect()
        let maxRight = (expand?.minX ?? bounds.width - leftPadding) - gap
        let candidateHeight = min(38, barHeight - gap * 1.8)
        let top = (barHeight - candidateHeight) / 2
        var x = leftPadding
        var rects: [CandidateRect] = []
        for item in candidateDrawItems(inlineOnly: true) {
            let width = min(candidateWidth(item), maxRight - x)
            if width < 24 {
                break
            }
            let rect = CGRect(x: x, y: top, width: width, height: candidateHeight)
            rects.append(
                CandidateRect(
                    identifierIndex: item.identifierIndex,
                    selectIndex: item.selectIndex,
                    rect: rect,
                    global: item.global,
                    command: item.command
                )
            )
            x = rect.maxX + gap
        }
        return rects
    }

    private func expandedCandidateLayout() -> [CandidateRect] {
        let top = config.candidateBarHeightDp
        let bottom = keyboardBottom()
        let gap: CGFloat = 7
        let left = gap * 1.5
        let right = bounds.width - left
        let rowHeight: CGFloat = 36
        var x = left
        var y = top + gap - expandedCandidateScrollY
        var contentBottom = top + gap
        var rects: [CandidateRect] = []
        for item in expandedCandidateItems() {
            let width = min(max(candidateWidth(item), 56), right - left)
            if x + width > right && x > left {
                x = left
                y += rowHeight + gap
            }
            let rect = CGRect(x: x, y: y, width: width, height: rowHeight)
            if rect.maxY >= top && rect.minY <= bottom {
                rects.append(
                    CandidateRect(
                        identifierIndex: item.identifierIndex,
                        selectIndex: item.selectIndex,
                        rect: rect,
                        global: item.global,
                        command: item.command
                    )
                )
            }
            contentBottom = max(contentBottom, rect.maxY + expandedCandidateScrollY)
            x += width + gap
        }
        expandedCandidateContentHeight = max(contentBottom - top + gap, expandedCandidatePanelHeight())
        coerceExpandedCandidateScroll()
        return rects
    }

    private func toolbarLayout() -> [ToolbarRect] {
        if functionPanelActive {
            let barHeight = config.candidateBarHeightDp
            let leftPadding = theme.panel.gap * 1.5
            let chipHeight = min(34, barHeight - 12)
            let top = (barHeight - chipHeight) / 2
            let backAction = ToolbarAction(
                label: "返回",
                command: .panel("close"),
                icon: .back
            )
            let settingsAction = ToolbarAction(
                label: "设置",
                command: KeyTaoKeyCommand(type: KeyTaoCommandType.openPage, value: "settings", fallbackValue: nil),
                icon: .settings
            )
            let backWidth = toolbarChipWidth(backAction)
            let settingsWidth = toolbarChipWidth(settingsAction)
            return [
                ToolbarRect(
                    action: backAction,
                    rect: CGRect(x: leftPadding, y: top, width: backWidth, height: chipHeight)
                ),
                ToolbarRect(
                    action: settingsAction,
                    rect: CGRect(x: bounds.width - leftPadding - settingsWidth, y: top, width: settingsWidth, height: chipHeight)
                ),
            ]
        }
        guard state.candidatePanel.candidates.isEmpty, (state.candidatePanel.preedit ?? state.preedit).isEmpty else {
            return []
        }
        let barHeight = config.candidateBarHeightDp
        let leftPadding = theme.panel.gap * 1.5
        let logoLeft = logoRect().minX
        let maxRight = logoLeft - 8
        let chipHeight = min(34, barHeight - 12)
        let top = (barHeight - chipHeight) / 2
        let actions = toolbarActions()
        let gap = toolbarGap(for: actions, availableWidth: max(0, maxRight - leftPadding))
        let widths = toolbarChipWidths(for: actions, availableWidth: max(0, maxRight - leftPadding), gap: gap)
        var x = leftPadding
        var rects: [ToolbarRect] = []
        for (action, width) in zip(actions, widths) {
            if x + width > maxRight {
                break
            }
            let rect = CGRect(x: x, y: top, width: width, height: chipHeight)
            rects.append(ToolbarRect(action: action, rect: rect))
            x = rect.maxX + gap
        }
        return rects
    }

    private func expandButtonRect() -> CGRect? {
        guard !state.candidatePanel.candidates.isEmpty else {
            return nil
        }
        let barHeight = config.candidateBarHeightDp
        let leftPadding = theme.panel.gap * 1.5
        let size = min(38, barHeight - 10)
        return CGRect(x: bounds.width - leftPadding - size, y: (barHeight - size) / 2, width: size, height: size)
    }

    private func logoRect() -> CGRect {
        let size: CGFloat = 30
        let leftPadding = theme.panel.gap * 1.5
        let barHeight = config.candidateBarHeightDp
        return CGRect(x: bounds.width - leftPadding - size, y: (barHeight - size) / 2, width: size, height: size)
    }

    private func candidateDrawItems(inlineOnly: Bool) -> [CandidateDrawItem] {
        state.candidatePanel.candidates.map { candidate in
            let global = panelCandidateGlobalIndex(candidate.index)
            return CandidateDrawItem(
                identifierIndex: candidate.index,
                selectIndex: inlineOnly ? global : global,
                label: candidate.label,
                text: candidate.text,
                comment: candidate.comment,
                selected: candidate.selected,
                global: true,
                command: nil
            )
        }
    }

    private func expandedCandidateItems() -> [CandidateDrawItem] {
        if functionPanelActive {
            switch functionPanelMode {
            case .home:
                return functionHomeItems()
            case .rime:
                return rimePanelItems()
            case .selection:
                return selectionPanelItems()
            case .clipboard:
                return clipboardPanelItems()
            case .emoji:
                return emojiPanelItems()
            }
        }
        return rimePanelItems()
    }

    private func rimePanelItems() -> [CandidateDrawItem] {
        let source = !expandedCandidates.isEmpty
            ? expandedCandidates
            : (!state.allCandidates.isEmpty
                ? state.allCandidates
                : (!state.candidates.isEmpty
                    ? state.candidates
                    : state.candidatePanel.candidates.map {
                        KeyTaoCandidate(
                            text: $0.text,
                            comment: $0.comment,
                            index: panelCandidateGlobalIndex($0.index)
                        )
                    }))
        let selected = selectedGlobalCandidateIndex()
        return source.enumerated().map { index, candidate in
            let globalIndex = candidate.index ?? index
            return CandidateDrawItem(
                identifierIndex: globalIndex,
                selectIndex: globalIndex,
                label: "\(globalIndex + 1).",
                text: candidate.text,
                comment: candidate.comment,
                selected: globalIndex == selected,
                global: true,
                command: nil
            )
        }
    }

    private func functionHomeItems() -> [CandidateDrawItem] {
        panelItems(
            PanelItem(label: "Rime", text: "方案/开关", command: .panel("rime")),
            PanelItem(label: "粘贴", text: "当前剪贴板", command: .edit("paste")),
            PanelItem(label: "Tab", text: "输入制表符", command: .edit("tab")),
            PanelItem(label: "行首", text: "移动光标", command: .edit("lineStart")),
            PanelItem(label: "行尾", text: "移动光标", command: .edit("lineEnd"))
        )
    }

    private func selectionPanelItems() -> [CandidateDrawItem] {
        panelItems(
            PanelItem(label: "多选", text: "开始/结束", command: .edit("toggleSelection")),
            PanelItem(label: "左选", text: "扩展一字", command: .edit("selectLeft")),
            PanelItem(label: "右选", text: "扩展一字", command: .edit("selectRight")),
            PanelItem(label: "全选", text: "选择全部", command: .edit("selectAll")),
            PanelItem(label: "复制", text: "复制选区", command: .edit("copy")),
            PanelItem(label: "剪切", text: "剪切选区", command: .edit("cut")),
            PanelItem(label: "粘贴", text: "当前剪贴板", command: .edit("paste")),
            PanelItem(label: "行首", text: "移动光标", command: .edit("lineStart")),
            PanelItem(label: "行尾", text: "移动光标", command: .edit("lineEnd")),
            PanelItem(label: "Tab", text: "输入制表符", command: .edit("tab"))
        )
    }

    private func clipboardPanelItems() -> [CandidateDrawItem] {
        var items = [
            PanelItem(label: "刷新", text: "读取系统剪贴板", command: .panel("clipboard")),
            PanelItem(label: "粘贴", text: "当前剪贴板", command: .edit("paste")),
        ]
        items.append(contentsOf: clipboardItems.enumerated().map { index, text in
            PanelItem(label: "剪贴 \(index + 1)", text: String(text.prefix(32)), command: .directInput(text))
        })
        return panelItems(items)
    }

    private func emojiPanelItems() -> [CandidateDrawItem] {
        Self.emojiChoices.enumerated().map { index, emoji in
            CandidateDrawItem(
                identifierIndex: -4000 - index,
                selectIndex: -4000 - index,
                label: "",
                text: emoji,
                comment: nil,
                selected: false,
                global: false,
                command: .directInput(emoji)
            )
        }
    }

    private struct PanelItem {
        var label: String
        var text: String
        var command: KeyTaoKeyCommand
        var comment: String?
    }

    private func panelItems(_ items: [PanelItem]) -> [CandidateDrawItem] {
        items.enumerated().map { index, item in
            CandidateDrawItem(
                identifierIndex: -1000 - index,
                selectIndex: -1000 - index,
                label: item.label,
                text: item.text,
                comment: item.comment,
                selected: false,
                global: false,
                command: item.command
            )
        }
    }

    private func panelItems(_ items: PanelItem...) -> [CandidateDrawItem] {
        panelItems(items)
    }

    private func candidateWidth(_ item: CandidateDrawItem) -> CGFloat {
        let labelWidth = textWidth(item.label, size: candidateLabelSize())
        let bodyWidth = textWidth(item.text, size: candidateTextSize())
        let commentWidth = item.comment.map { textWidth($0, size: candidateCommentSize()) } ?? 0
        let segmentCount = [labelWidth, bodyWidth, commentWidth].filter { $0 > 0 }.count
        let gaps = CGFloat(max(0, segmentCount - 1)) * candidateInlineGap()
        let accent = item.selected ? 4 : 0
        return labelWidth + bodyWidth + commentWidth + gaps + CGFloat(accent) + candidatePaddingX() * 2
    }

    private func toolbarChipWidth(_ action: ToolbarAction, horizontalPadding: CGFloat = 22, minimumWidth: CGFloat? = nil) -> CGFloat {
        if action.icon != nil && (action.secondaryLabel?.isEmpty ?? true) {
            return max(minimumWidth ?? 46, 46)
        }
        let labelWidth = textWidth(action.label, size: theme.font.labelSize)
        let secondaryWidth = action.secondaryLabel.map { textWidth($0, size: theme.font.commentSize) } ?? 0
        let inlineGap: CGFloat = secondaryWidth > 0 ? 5 : 0
        let fallbackMinimum = secondaryWidth > 0 ? 58 : 48
        return max(labelWidth + secondaryWidth + inlineGap + horizontalPadding, minimumWidth ?? CGFloat(fallbackMinimum))
    }

    private func toolbarGap(for actions: [ToolbarAction], availableWidth: CGFloat) -> CGFloat {
        let naturalGap: CGFloat = 6
        guard actions.count > 1 else {
            return naturalGap
        }
        let naturalTotal = actions.map { toolbarChipWidth($0) }.reduce(0, +) + naturalGap * CGFloat(actions.count - 1)
        return naturalTotal <= availableWidth ? naturalGap : 4
    }

    private func toolbarChipWidths(for actions: [ToolbarAction], availableWidth: CGFloat, gap: CGFloat) -> [CGFloat] {
        guard !actions.isEmpty else {
            return []
        }
        let natural = actions.map { toolbarChipWidth($0) }
        let naturalTotal = natural.reduce(0, +) + gap * CGFloat(max(0, actions.count - 1))
        if naturalTotal <= availableWidth {
            return natural
        }

        let compact = actions.map { toolbarChipWidth($0, horizontalPadding: 16, minimumWidth: 42) }
        let compactTotal = compact.reduce(0, +) + gap * CGFloat(max(0, actions.count - 1))
        if compactTotal <= availableWidth {
            return compact
        }

        let minimums = actions.map { toolbarMinimumChipWidth($0) }
        let minimumTotal = minimums.reduce(0, +) + gap * CGFloat(max(0, actions.count - 1))
        guard minimumTotal < compactTotal, availableWidth > minimumTotal else {
            return compact
        }

        let overflow = compactTotal - availableWidth
        let shrinkable = zip(compact, minimums).map { max(0, $0 - $1) }.reduce(0, +)
        guard shrinkable > 0 else {
            return compact
        }
        return zip(compact, minimums).map { width, minimum in
            let share = max(0, width - minimum) / shrinkable
            return max(minimum, width - overflow * share)
        }
    }

    private func toolbarMinimumChipWidth(_ action: ToolbarAction) -> CGFloat {
        if action.icon != nil && (action.secondaryLabel?.isEmpty ?? true) {
            return 40
        }
        let labelWidth = textWidth(action.label, size: max(12, theme.font.labelSize * 0.82))
        let secondaryWidth = action.secondaryLabel.map { textWidth($0, size: max(11, theme.font.commentSize * 0.82)) } ?? 0
        let inlineGap: CGFloat = secondaryWidth > 0 ? 5 : 0
        return max(labelWidth + secondaryWidth + inlineGap + 10, secondaryWidth > 0 ? 44 : 38)
    }

    private func activeRows() -> [[KeyTaoKeySpec]] {
        let rows = config.rows(for: layerMode)
        guard layerMode == .letters, shouldUseInlineNumberRow() else {
            return rows
        }
        return rows.enumerated().map { index, row in
            index == 0 ? inlineNumberRow(row) : row
        }
    }

    private func shouldUseInlineNumberRow() -> Bool {
        !state.asciiMode && state.hasComposition && state.preedit.contains("=")
    }

    private func inlineNumberRow(_ row: [KeyTaoKeySpec]) -> [KeyTaoKeySpec] {
        let digits = Array("1234567890")
        return row.enumerated().map { index, key in
            guard index < digits.count else {
                return key
            }
            let digit = String(digits[index])
            return KeyTaoKeySpec(
                label: digit,
                value: digit,
                rimeValue: nil,
                hint: nil,
                weight: key.weight,
                style: key.style,
                action: .input(digit),
                swipeUp: nil,
                swipeDown: nil,
                longPress: nil,
                asciiLongPress: nil,
                asciiLabel: digit,
                asciiValue: digit,
                asciiAction: .input(digit)
            )
        }
    }

    private func toolbarActions() -> [ToolbarAction] {
        let function = ToolbarAction(
            label: "功能",
            command: KeyTaoKeyCommand(type: KeyTaoCommandType.panel, value: "home", fallbackValue: nil),
            icon: .function
        )
        let languageToggle = languageToggleAction()
        if layerMode == .symbols {
            return [
                function,
                ToolbarAction(label: "中", command: KeyTaoKeyCommand(type: KeyTaoCommandType.mode, value: "chinese", fallbackValue: nil), selected: !state.asciiMode),
                ToolbarAction(label: "En", command: KeyTaoKeyCommand(type: KeyTaoCommandType.mode, value: "ascii", fallbackValue: nil), selected: state.asciiMode),
                ToolbarAction(label: "123", command: KeyTaoKeyCommand(type: KeyTaoCommandType.keyboardMode, value: "numbers", fallbackValue: nil)),
                ToolbarAction(label: "ABC", command: KeyTaoKeyCommand(type: KeyTaoCommandType.keyboardMode, value: "letters", fallbackValue: nil)),
            ]
        } else {
            return [
                function,
                languageToggle,
                ToolbarAction(label: "选择", command: KeyTaoKeyCommand(type: KeyTaoCommandType.panel, value: "selection", fallbackValue: nil), icon: .selection),
                ToolbarAction(label: "剪贴板", command: KeyTaoKeyCommand(type: KeyTaoCommandType.panel, value: "clipboard", fallbackValue: nil), icon: .clipboard),
                ToolbarAction(label: "Emoji", command: KeyTaoKeyCommand(type: KeyTaoCommandType.panel, value: "emoji", fallbackValue: nil), icon: .emoji),
            ]
        }
    }

    private func languageToggleAction() -> ToolbarAction {
        if state.asciiMode {
            return ToolbarAction(
                label: "En",
                command: KeyTaoKeyCommand(type: KeyTaoCommandType.mode, value: nil, fallbackValue: nil),
                secondaryLabel: "中"
            )
        }
        return ToolbarAction(
            label: "中",
            command: KeyTaoKeyCommand(type: KeyTaoCommandType.mode, value: nil, fallbackValue: nil),
            secondaryLabel: "En"
        )
    }

    private func handleToolbarCommand(_ command: KeyTaoKeyCommand) {
        if handlePanelCommand(command) {
            return
        }
        performConfiguredHaptic()
        delegate?.keyboardView(self, didTrigger: command)
    }

    @discardableResult
    private func handlePanelCommand(_ command: KeyTaoKeyCommand) -> Bool {
        if command.type == KeyTaoCommandType.panel {
            switch command.value {
            case "close":
                closeCandidatePanel()
            case "home", nil:
                openFunctionPanel(.home)
            case "rime":
                openFunctionPanel(.rime)
                delegate?.keyboardView(self, didTrigger: KeyTaoKeyCommand(type: KeyTaoCommandType.rimeMenu, value: nil, fallbackValue: nil))
            case "selection":
                openFunctionPanel(.selection)
            case "clipboard":
                openFunctionPanel(.clipboard)
            case "emoji":
                openFunctionPanel(.emoji)
            default:
                openFunctionPanel(.home)
            }
            performConfiguredHaptic()
            invalidateLayoutAndDisplay()
            return true
        }
        performConfiguredHaptic()
        delegate?.keyboardView(self, didTrigger: command)
        return true
    }

    private func toggleCandidatePanel() {
        if candidatePanelExpanded {
            closeCandidatePanel()
        } else {
            openCandidatePanel()
        }
    }

    private func openCandidatePanel() {
        guard !state.candidatePanel.candidates.isEmpty else {
            return
        }
        functionPanelActive = false
        functionPanelMode = .home
        candidatePanelExpanded = true
        expandedCandidates = []
        resetExpandedCandidateScroll()
        requestExpandedCandidatesAsync()
    }

    private func closeCandidatePanel() {
        guard candidatePanelExpanded || functionPanelActive || !expandedCandidates.isEmpty else {
            return
        }
        candidatePanelExpanded = false
        functionPanelActive = false
        functionPanelMode = .home
        expandedCandidates = []
        expandedCandidatesLoading = false
        clipboardItemsLoading = false
        cancelExpandedCandidateRequest()
        resetExpandedCandidateScroll()
    }

    private func closeCandidatePanelIfNeeded(afterCandidateSelection global: Bool) {
        guard global, candidatePanelExpanded, !functionPanelActive else {
            return
        }
        closeCandidatePanel()
    }

    private func openFunctionPanel(_ mode: KeyTaoFunctionPanelMode) {
        functionPanelActive = true
        candidatePanelExpanded = true
        functionPanelMode = mode
        expandedCandidates = []
        cancelExpandedCandidateRequest()
        clipboardItemsLoading = mode == .clipboard
        resetExpandedCandidateScroll()
        if mode == .rime {
            requestExpandedCandidatesAsync()
        }
        if mode == .clipboard {
            requestClipboardItemsAsync()
        }
    }

    private func requestExpandedCandidatesAsync() {
        cancelPendingExpandedCandidateWorkItem()
        guard canRequestExpandedCandidates() else {
            expandedCandidatesLoading = false
            return
        }
        if !state.allCandidates.isEmpty {
            expandedCandidates = state.allCandidates
            expandedCandidatesLoading = false
            invalidateLayoutAndDisplay()
            return
        }

        let token = nextExpandRequestToken()
        expandedCandidatesLoading = true
        let workItem = DispatchWorkItem { [weak self] in
            guard let self, token == self.expandRequestToken, self.canRequestExpandedCandidates() else {
                return
            }
            self.delegate?.keyboardView(self, requestExpandedCandidates: { [weak self] candidates in
                DispatchQueue.main.async {
                    guard let self, token == self.expandRequestToken, self.canRequestExpandedCandidates() else {
                        return
                    }
                    self.expandedCandidates = candidates
                    self.expandedCandidatesLoading = false
                    self.resetExpandedCandidateScroll()
                    self.invalidateLayoutAndDisplay()
                }
            })
        }
        pendingExpandedCandidateWorkItem = workItem
        DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(Self.expandedCandidateLoadDelayMs), execute: workItem)
        invalidateLayoutAndDisplay()
    }

    private func requestClipboardItemsAsync() {
        let token = nextExpandRequestToken()
        clipboardItemsLoading = true
        delegate?.keyboardView(self, requestClipboardHistory: { [weak self] items in
            DispatchQueue.main.async {
                guard let self,
                      token == self.expandRequestToken,
                      self.candidatePanelExpanded,
                      self.functionPanelMode == .clipboard else {
                    return
                }
                self.clipboardItems = items
                self.clipboardItemsLoading = false
                self.resetExpandedCandidateScroll()
                self.invalidateLayoutAndDisplay()
            }
        })
    }

    private func canRequestExpandedCandidates() -> Bool {
        guard candidatePanelExpanded, !state.candidatePanel.candidates.isEmpty else {
            return false
        }
        return !functionPanelActive || functionPanelMode == .rime
    }

    private func cancelExpandedCandidateRequest() {
        cancelPendingExpandedCandidateWorkItem()
        expandRequestToken += 1
        expandedCandidatesLoading = false
    }

    private func cancelPendingExpandedCandidateWorkItem() {
        pendingExpandedCandidateWorkItem?.cancel()
        pendingExpandedCandidateWorkItem = nil
    }

    private func nextExpandRequestToken() -> Int {
        expandRequestToken += 1
        return expandRequestToken
    }

    private func resetExpandedCandidateScroll() {
        expandedCandidateScrollY = 0
        expandedCandidateContentHeight = expandedCandidatePanelHeight()
    }

    private func maxExpandedCandidateScroll() -> CGFloat {
        max(0, expandedCandidateContentHeight - expandedCandidatePanelHeight())
    }

    private func coerceExpandedCandidateScroll() {
        expandedCandidateScrollY = max(0, min(maxExpandedCandidateScroll(), expandedCandidateScrollY))
    }

    private func expandedCandidatePanelHeight() -> CGFloat {
        guard candidatePanelExpanded else {
            return 0
        }
        return max(0, keyboardBottom() - config.candidateBarHeightDp)
    }

    private func functionPanelTitle() -> String {
        switch functionPanelMode {
        case .home:
            return "功能"
        case .rime:
            return "Rime"
        case .selection:
            return "选择"
        case .clipboard:
            return "剪贴板"
        case .emoji:
            return "Emoji"
        }
    }

    private func expandedPanelEmptyMessage() -> String {
        if clipboardItemsLoading {
            return "正在读取剪贴板"
        }
        if expandedCandidatesLoading && functionPanelMode == .rime {
            return "正在加载 Rime 选项"
        }
        if expandedCandidatesLoading {
            return functionPanelActive ? "正在加载功能" : "正在加载候选"
        }
        if functionPanelActive && functionPanelMode == .clipboard {
            return "剪贴板为空"
        }
        if functionPanelActive {
            return "暂无功能项"
        }
        return "没有更多候选"
    }

    private func handleBackspaceDrag(at point: CGPoint) -> Bool {
        guard let key = pressedKey, isBackspaceKey(key.spec) else {
            return false
        }
        let deltaX = point.x - touchStart.x
        let deltaY = point.y - touchStart.y
        let threshold = max(CGFloat(8), config.swipeThresholdDp * 0.65)
        guard abs(deltaX) > threshold, abs(deltaX) > abs(deltaY) * 0.75 else {
            return false
        }

        stopLongPressAndRepeat()
        longPressConsumed = true
        backspaceGestureConsumed = true

        let stepWidth = max(CGFloat(8), key.rect.width * 0.22)
        let moved = max(CGFloat(0), abs(deltaX) - threshold)
        let stepCount = max(1, Int(floor(moved / stepWidth)) + 1)
        let targetUnits = deltaX < 0 ? stepCount : -stepCount
        let deltaUnits = targetUnits - backspaceGestureUnits
        guard deltaUnits != 0 else {
            return true
        }

        let action = deltaUnits > 0 ? "delete" : "restore"
        for _ in 0..<abs(deltaUnits) {
            delegate?.keyboardView(self, didTrigger: backspaceGestureCommand(action))
        }
        backspaceGestureUnits = targetUnits
        performConfiguredHaptic()
        return true
    }

    private func handleBackspaceRelease(for key: KeyRect, at point: CGPoint) -> Bool {
        guard isBackspaceKey(key.spec), !backspaceGestureConsumed else {
            return false
        }
        let deltaX = point.x - touchStart.x
        let deltaY = point.y - touchStart.y
        let threshold = max(CGFloat(12), config.swipeThresholdDp)
        guard abs(deltaY) > threshold, abs(deltaY) > abs(deltaX) * 1.1 else {
            return false
        }

        delegate?.keyboardView(
            self,
            didTrigger: backspaceGestureCommand(deltaY < 0 ? "deleteAll" : "restoreAll")
        )
        performConfiguredHaptic(strong: true)
        return true
    }

    private func backspaceGestureCommand(_ action: String) -> KeyTaoKeyCommand {
        KeyTaoKeyCommand(type: KeyTaoCommandType.backspaceGesture, value: action, fallbackValue: nil)
    }

    private func isBackspaceKey(_ key: KeyTaoKeySpec) -> Bool {
        actionForMode(key).type == KeyTaoCommandType.backspace
    }

    private func resolveCommand(
        _ key: KeyTaoKeySpec,
        deltaY: CGFloat,
        rect: CGRect? = nil,
        releaseY: CGFloat? = nil
    ) -> KeyTaoKeyCommand {
        let threshold = config.swipeThresholdDp
        let command: KeyTaoKeyCommand
        if deltaY < -threshold {
            command = resolveSwipeUpCommand(key)
        } else if deltaY > threshold {
            command = key.swipeDown ?? actionForMode(key)
        } else {
            command = stackCommandForPoint(key, rect: rect, releaseY: releaseY) ?? actionForMode(key)
        }
        return applyShift(command)
    }

    private func resolveSwipeUpCommand(_ key: KeyTaoKeySpec) -> KeyTaoKeyCommand {
        if let swipeUp = key.swipeUp {
            return swipeUp
        }
        if state.asciiMode, let asciiLongPress = key.asciiLongPress {
            return asciiLongPress
        }
        if let longPress = key.longPress {
            return longPress
        }
        if let hint = key.hint, hint.count == 1 {
            return .input(hint)
        }
        return actionForMode(key)
    }

    private func resolveLongPressCommand(_ key: KeyTaoKeySpec) -> KeyTaoKeyCommand {
        if state.asciiMode, let asciiLongPress = key.asciiLongPress {
            return applyShift(asciiLongPress)
        }
        if let longPress = key.longPress {
            return applyShift(longPress)
        }
        if let hint = key.hint, hint.count == 1 {
            return applyShift(.input(hint))
        }
        return applyShift(actionForMode(key))
    }

    private func actionForMode(_ key: KeyTaoKeySpec) -> KeyTaoKeyCommand {
        if state.asciiMode {
            if let asciiAction = key.asciiAction {
                return asciiAction
            }
            if let asciiValue = key.asciiValue {
                return .input(asciiValue)
            }
        } else {
            if let rimeValue = key.rimeValue {
                return KeyTaoKeyCommand(type: KeyTaoCommandType.rimeInput, value: rimeValue, fallbackValue: key.value)
            }
            if let asciiValue = key.asciiValue, asciiValue != key.value {
                return KeyTaoKeyCommand(type: KeyTaoCommandType.rimeInput, value: asciiValue, fallbackValue: key.value)
            }
        }
        return key.action ?? .input(key.value ?? key.label)
    }

    private func stackCommandForPoint(_ key: KeyTaoKeySpec, rect: CGRect?, releaseY: CGFloat?) -> KeyTaoKeyCommand? {
        guard let stack = key.stack, !stack.isEmpty else {
            return nil
        }
        let item: KeyTaoKeyStackItem
        if let rect, let releaseY, rect.height > 0 {
            let index = stackIndex(in: rect, count: stack.count, y: releaseY)
            item = stack[index]
        } else {
            item = stack[0]
        }
        return actionForMode(item)
    }

    private func pressedStackIndex(for key: KeyRect) -> Int? {
        guard let stack = key.spec.stack, !stack.isEmpty else {
            return nil
        }
        guard pressedKey?.spec == key.spec, key.rect.contains(currentTouchPoint) else {
            return nil
        }
        return stackIndex(in: key.rect, count: stack.count, y: currentTouchPoint.y)
    }

    private func stackIndex(in rect: CGRect, count: Int, y: CGFloat) -> Int {
        guard count > 1, rect.height > 0 else {
            return 0
        }
        let itemRects = stackItemRects(in: rect, count: count)
        if let index = itemRects.firstIndex(where: { y >= $0.minY && y <= $0.maxY }) {
            return index
        }
        let ratio = max(CGFloat(0), min(CGFloat(0.999), (y - rect.minY) / rect.height))
        return max(0, min(count - 1, Int(ratio * CGFloat(count))))
    }

    private func stackItemRects(in rect: CGRect, count: Int) -> [CGRect] {
        guard count > 1 else {
            return [rect]
        }
        let gap = max(CGFloat(0), min(keyboardVerticalGap(), 6))
        let itemHeight = max(CGFloat(1), (rect.height - gap * CGFloat(count - 1)) / CGFloat(count))
        return (0..<count).map { index in
            let y = rect.minY + CGFloat(index) * (itemHeight + gap)
            return CGRect(x: rect.minX, y: y, width: rect.width, height: itemHeight)
        }
    }

    private func actionForMode(_ item: KeyTaoKeyStackItem) -> KeyTaoKeyCommand {
        if state.asciiMode {
            if let asciiAction = item.asciiAction {
                return asciiAction
            }
            if let asciiValue = item.asciiValue {
                return .input(asciiValue)
            }
        } else {
            if let rimeValue = item.rimeValue {
                return KeyTaoKeyCommand(type: KeyTaoCommandType.rimeInput, value: rimeValue, fallbackValue: item.value)
            }
            if let asciiValue = item.asciiValue, asciiValue != item.value {
                return KeyTaoKeyCommand(type: KeyTaoCommandType.rimeInput, value: asciiValue, fallbackValue: item.value)
            }
        }
        return item.action ?? .input(item.value ?? item.label)
    }

    private func applyShift(_ command: KeyTaoKeyCommand) -> KeyTaoKeyCommand {
        guard shiftState != .off,
              command.type == KeyTaoCommandType.input,
              let value = command.value,
              value.count == 1,
              value.range(of: "[A-Za-z]", options: .regularExpression) != nil else {
            return command
        }
        var shifted = command
        shifted.value = value.uppercased()
        return shifted
    }

    private func scheduleLongPressIfNeeded() {
        guard let key = pressedKey, keySupportsLongPress(key.spec) else {
            return
        }
        let workItem = DispatchWorkItem { [weak self] in
            guard let self, self.pressedKey?.spec == key.spec else {
                return
            }
            self.longPressConsumed = true
            self.performConfiguredHaptic(strong: true)
            if self.isRepeatableKey(key.spec) {
                self.startRepeating(key.spec)
            } else {
                let command = self.resolveLongPressCommand(key.spec)
                self.delegate?.keyboardView(self, didTrigger: command)
                self.clearOneShotShift(after: command)
            }
            self.setNeedsDisplay()
        }
        longPressWorkItem = workItem
        DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(Self.longPressDelayMs), execute: workItem)
    }

    private func stopLongPressAndRepeat() {
        longPressWorkItem?.cancel()
        longPressWorkItem = nil
        repeatTimer?.invalidate()
        repeatTimer = nil
    }

    private func keySupportsLongPress(_ key: KeyTaoKeySpec) -> Bool {
        key.longPress != nil || key.asciiLongPress != nil || key.hint?.isEmpty == false
    }

    private func isRepeatableKey(_ key: KeyTaoKeySpec) -> Bool {
        actionForMode(key).type == KeyTaoCommandType.backspace
    }

    private func startRepeating(_ key: KeyTaoKeySpec) {
        let command = resolveCommand(key, deltaY: 0)
        delegate?.keyboardView(self, didTrigger: command)
        repeatTimer?.invalidate()
        repeatTimer = Timer.scheduledTimer(withTimeInterval: TimeInterval(Self.backspaceRepeatIntervalMs) / 1000, repeats: true) { [weak self] _ in
            guard let self, self.pressedKey?.spec == key else {
                self?.repeatTimer?.invalidate()
                self?.repeatTimer = nil
                return
            }
            self.delegate?.keyboardView(self, didTrigger: command)
        }
    }

    private func displayLabel(_ key: KeyTaoKeySpec) -> String {
        if key.action?.type == KeyTaoCommandType.shift {
            return shiftState == .locked ? "⇪" : key.label
        }
        if key.action?.type == KeyTaoCommandType.space {
            return state.schemaName.isEmpty ? key.label : state.schemaName
        }
        if key.action?.type == KeyTaoCommandType.mode {
            return state.asciiMode ? theme.modeHint.englishText : theme.modeHint.chineseText
        }
        let label = state.asciiMode ? (key.asciiLabel ?? key.asciiValue ?? key.label) : key.label
        let value = state.asciiMode ? (key.asciiValue ?? key.value ?? key.label) : (key.value ?? key.label)
        if shiftState != .off, value.count == 1, value.range(of: "[A-Za-z]", options: .regularExpression) != nil {
            return label.uppercased()
        }
        return label
    }

    private func stackLabelForMode(_ item: KeyTaoKeyStackItem) -> String {
        if state.asciiMode {
            return item.asciiLabel ?? item.asciiValue ?? item.label
        }
        return item.label
    }

    private func isActiveKey(_ key: KeyTaoKeySpec) -> Bool {
        key.action?.type == KeyTaoCommandType.shift && shiftState != .off
    }

    private func isSoftAccentKey(_ key: KeyTaoKeySpec?) -> Bool {
        guard let key else {
            return false
        }
        let type = actionForMode(key).type
        return key.style == "accent"
            || isSoftAccentPunctuationKey(key)
            || type == KeyTaoCommandType.mode
            || type == KeyTaoCommandType.keyboardMode
            || type == KeyTaoCommandType.space
            || type == KeyTaoCommandType.enter
            || type == KeyTaoCommandType.backspace
    }

    private func isSoftAccentPunctuationKey(_ key: KeyTaoKeySpec) -> Bool {
        let label = state.asciiMode ? (key.asciiLabel ?? key.asciiValue ?? key.label) : key.label
        let value = state.asciiMode ? (key.asciiValue ?? key.value ?? key.label) : (key.value ?? key.label)
        return ["，", "。", ",", "."].contains(label) || ["，", "。", ",", "."].contains(value)
    }

    private func panelCandidateGlobalIndex(_ localIndex: Int) -> Int {
        let pageSize = state.pageSize > 0 ? state.pageSize : max(state.candidatePanel.candidates.count, 1)
        return state.page * pageSize + localIndex
    }

    private func selectedGlobalCandidateIndex() -> Int {
        panelCandidateGlobalIndex(state.highlightedCandidateIndex)
    }

    private func candidateSignature(_ state: KeyTaoImeState) -> String {
        var parts: [String] = [
            state.candidatePanel.preedit ?? "",
            "\(state.candidatePanel.navigation.canGoPrevious):\(state.candidatePanel.navigation.canGoNext)",
            state.schemaName,
            "\(state.pageSize)",
            "\(state.page)",
        ]
        parts.append(contentsOf: state.candidatePanel.candidates.map { candidate in
            [
                "\(candidate.index)",
                candidate.label,
                candidate.text,
                candidate.comment ?? "",
                "\(candidate.selected)",
            ].joined(separator: ":")
        })
        return parts.joined(separator: "|")
    }

    private func keyboardTop() -> CGFloat {
        config.candidateBarHeightDp
    }

    private func keyboardBottom() -> CGFloat {
        bounds.height
    }

    private func keyboardHorizontalGap() -> CGFloat {
        config.horizontalGapDp
    }

    private func keyboardVerticalGap() -> CGFloat {
        config.verticalGapDp
    }

    private func keyboardMaxKeyHeight() -> CGFloat {
        config.maxKeyHeightDp
    }

    private func candidateTextSize() -> CGFloat {
        max(13, min(theme.font.size - 2, 16))
    }

    private func candidateLabelSize() -> CGFloat {
        max(10, min(theme.font.labelSize - 1, 13))
    }

    private func candidateCommentSize() -> CGFloat {
        max(10, min(theme.font.commentSize - 1, 12))
    }

    private func candidatePaddingX() -> CGFloat {
        max(7, min(theme.candidate.paddingX, 9))
    }

    private func candidateInlineGap() -> CGFloat {
        max(2, min(theme.candidate.inlineGap, 4))
    }

    private func candidateCornerRadius() -> CGFloat {
        max(6, min(theme.candidate.cornerRadius, 8))
    }

    private func keyCornerRadius(for rect: CGRect) -> CGFloat {
        min(max(6, min(theme.candidate.cornerRadius, 9)), rect.width * 0.28, rect.height * 0.28)
    }

    private func keyWeight(_ key: KeyTaoKeySpec) -> CGFloat {
        max(key.weight ?? 1, 0.25)
    }

    private func rowWeight(_ row: [KeyTaoKeySpec]) -> CGFloat {
        max(1, row.reduce(CGFloat(0)) { $0 + keyWeight($1) })
    }

    private func keyRowSpan(_ key: KeyTaoKeySpec) -> Int {
        max(1, min(8, Int(key.rowSpan ?? 1)))
    }

    private func advanceRowSpans(_ spans: [ActiveRowSpan]) -> [ActiveRowSpan] {
        spans.compactMap { span in
            let remainingRows = span.remainingRows - 1
            guard remainingRows > 0 else {
                return nil
            }
            return ActiveRowSpan(weight: span.weight, remainingRows: remainingRows)
        }
    }

    private func keyboardOuterInset() -> CGFloat {
        config.outerInsetDp
    }

    private func keyboardReferenceUnitWidth(rows: [[KeyTaoKeySpec]], horizontalGap: CGFloat) -> CGFloat {
        var activeLeadingSpans: [ActiveRowSpan] = []
        var referenceKeyCount = 0
        var referenceWeight = CGFloat(1)
        for row in rows {
            let effectiveKeyCount = activeLeadingSpans.count + row.count
            let effectiveWeight = max(
                CGFloat(1),
                activeLeadingSpans.reduce(CGFloat(0)) { $0 + $1.weight } + rowWeight(row)
            )
            if effectiveKeyCount > referenceKeyCount ||
                (effectiveKeyCount == referenceKeyCount && effectiveWeight > referenceWeight) {
                referenceKeyCount = effectiveKeyCount
                referenceWeight = effectiveWeight
            }
            let nextLeadingSpans = row.prefix { keyRowSpan($0) > 1 }.map {
                ActiveRowSpan(weight: keyWeight($0), remainingRows: keyRowSpan($0) - 1)
            }
            activeLeadingSpans = advanceRowSpans(activeLeadingSpans)
            activeLeadingSpans.append(contentsOf: nextLeadingSpans)
        }
        guard referenceKeyCount > 0 else {
            return 32
        }
        let gapWidth = horizontalGap * CGFloat(max(0, referenceKeyCount - 1))
        let availableWidth = max(1, bounds.width - keyboardOuterInset() * 2 - gapWidth)
        return max(24, availableWidth / referenceWeight)
    }

    private func keyboardRowWidth(
        _ row: [KeyTaoKeySpec],
        rowIndex: Int,
        rows: [[KeyTaoKeySpec]],
        referenceUnitWidth: CGFloat,
        horizontalGap: CGFloat,
        maximumRowWidth: CGFloat,
        effectiveKeyCount: Int,
        effectiveWeight: CGFloat
    ) -> CGFloat {
        if keyboardRowShouldFillWidth(row, rowIndex: rowIndex, rows: rows) {
            return maximumRowWidth
        }
        let gapWidth = horizontalGap * CGFloat(max(0, effectiveKeyCount - 1))
        return min(maximumRowWidth, referenceUnitWidth * effectiveWeight + gapWidth)
    }

    private func keyboardRowShouldFillWidth(_ row: [KeyTaoKeySpec], rowIndex: Int, rows: [[KeyTaoKeySpec]]) -> Bool {
        if layerMode != .letters {
            return true
        }
        if rowIndex == 0 || rowIndex == rows.count - 1 {
            return true
        }
        if row.count <= 5 {
            return true
        }
        return row.contains { key in
            let type = actionForMode(key).type
            return type == KeyTaoCommandType.shift || type == KeyTaoCommandType.backspace
        }
    }

    private func panelBackgroundColor() -> UIColor {
        blend(
            foreground: theme.candidate.selectedLabelColor.uiColor,
            background: theme.panel.background.uiColor,
            amount: 0.07,
            alpha: CGFloat(theme.panel.background.alpha.clampedColor) / 255
        )
    }

    private func statusMessageColor() -> UIColor {
        isDarkPanel()
            ? UIColor(white: 0.94, alpha: 0.92)
            : UIColor(red: 31 / 255, green: 41 / 255, blue: 51 / 255, alpha: 0.88)
    }

    private func keyBackgroundColor(_ key: KeyTaoKeySpec? = nil, selected: Bool = false) -> UIColor {
        if selected && isSoftAccentKey(key) {
            return softenedAccentSurfaceColor(0.24)
        }
        if selected {
            return theme.candidate.selectedBackground.uiColor.withAlphaComponent(isDarkPanel() ? 0.48 : 0.62)
        }
        if isSoftAccentKey(key) {
            return softenedAccentSurfaceColor(0.16)
        }
        if key?.style == "accent" {
            return theme.candidate.selectedBackground.uiColor.withAlphaComponent(isDarkPanel() ? 0.42 : 0.54)
        }
        if theme.candidate.background.alpha > 0 {
            return theme.candidate.background.uiColor.withAlphaComponent(isDarkPanel() ? 0.34 : 0.52)
        }
        return isDarkPanel()
            ? UIColor(red: 28 / 255, green: 34 / 255, blue: 42 / 255, alpha: 0.38)
            : UIColor.white.withAlphaComponent(0.58)
    }

    private func keyForegroundColor(_ key: KeyTaoKeySpec, selected: Bool) -> UIColor {
        if selected {
            return theme.candidate.selectedForeground.uiColor
        }
        if key.style == "accent" {
            return theme.candidate.selectedForeground.uiColor
        }
        return theme.candidate.foreground.uiColor
    }

    private func toolbarBackgroundColor(_ action: ToolbarAction, pressed: Bool) -> UIColor {
        let accent = action.selected || isSoftAccentToolbar(action)
        if pressed && accent {
            return softenedAccentSurfaceColor(0.24)
        }
        if pressed {
            return theme.candidate.selectedBackground.uiColor
        }
        if accent {
            return softenedAccentSurfaceColor(action.selected ? 0.18 : 0.13)
        }
        return keyBackgroundColor()
    }

    private func isSoftAccentToolbar(_ action: ToolbarAction) -> Bool {
        if action.command.type == KeyTaoCommandType.mode || action.command.type == KeyTaoCommandType.openPage {
            return true
        }
        if action.command.type == KeyTaoCommandType.panel,
           ["home", "selection", "clipboard", "emoji", "close", "dismissClipboard"].contains(action.command.value ?? "home") {
            return true
        }
        return ["功能", "中", "En", "中文", "英文", "选择", "剪贴板", "Emoji", "返回", "设置", "🌐"].contains(action.label)
    }

    private func softenedAccentSurfaceColor(_ amount: CGFloat) -> UIColor {
        let alpha = isDarkPanel()
            ? min(0.82, 0.66 + amount * 0.36)
            : min(0.66, 0.44 + amount * 0.72)
        return blend(
            foreground: theme.candidate.selectedLabelColor.uiColor,
            background: panelBackgroundColor(),
            amount: max(0, min(amount, 1)),
            alpha: alpha
        )
    }

    private func blend(foreground: UIColor, background: UIColor, amount: CGFloat, alpha: CGFloat? = nil) -> UIColor {
        var fr: CGFloat = 0
        var fg: CGFloat = 0
        var fb: CGFloat = 0
        var fa: CGFloat = 0
        var br: CGFloat = 0
        var bg: CGFloat = 0
        var bb: CGFloat = 0
        var ba: CGFloat = 0
        foreground.getRed(&fr, green: &fg, blue: &fb, alpha: &fa)
        background.getRed(&br, green: &bg, blue: &bb, alpha: &ba)
        let ratio = max(0, min(amount, 1))
        let inverse = 1 - ratio
        return UIColor(
            red: fr * ratio + br * inverse,
            green: fg * ratio + bg * inverse,
            blue: fb * ratio + bb * inverse,
            alpha: alpha ?? ba
        )
    }

    private func isDarkPanel() -> Bool {
        let luminance = (theme.panel.background.red * 299 + theme.panel.background.green * 587 + theme.panel.background.blue * 114) / 1000
        return luminance < 128
    }

    private func performConfiguredHaptic(strong: Bool = false) {
        guard config.hapticsEnabled else {
            return
        }
        hapticGenerator.impactOccurred(intensity: min(1, max(0.15, CGFloat(config.hapticIntensity) / (strong ? 60 : 100))))
        hapticGenerator.prepare()
    }

    private func fittedFont(for text: String, size: CGFloat, maxWidth: CGFloat) -> UIFont {
        var nextSize = size
        var font = themedFont(size: nextSize, weight: theme.font.weight)
        while nextSize > 12 && text.size(withAttributes: [.font: font]).width > maxWidth {
            nextSize -= 1
            font = themedFont(size: nextSize, weight: theme.font.weight)
        }
        return font
    }

    private func themedFont(size: CGFloat, weight: KeyTaoThemeFontWeight) -> UIFont {
        .keytaoThemeFont(family: theme.font.family, size: size, weight: weight)
    }

    private static func loadLogoImage() -> UIImage? {
        for bundle in KeyTaoIOSBundle.resourceBundles {
            if let image = UIImage(named: "keytao-logo", in: bundle, compatibleWith: nil) {
                return image
            }
            if let url = bundle.url(forResource: "keytao-logo", withExtension: "png"),
               let image = UIImage(contentsOfFile: url.path) {
                return image
            }
        }
        return nil
    }

    private func textWidth(_ text: String, size: CGFloat) -> CGFloat {
        text.size(withAttributes: [.font: themedFont(size: size, weight: theme.font.weight)]).width
    }

    private func keyAccessibilityIdentifier(_ spec: KeyTaoKeySpec) -> String {
        if let action = spec.action {
            return commandAccessibilityIdentifier(action, prefix: "keytao-key")
        }
        let value = spec.value ?? spec.rimeValue ?? spec.label
        return "keytao-key-\(asciiSlug(value))"
    }

    private func commandAccessibilityIdentifier(_ command: KeyTaoKeyCommand, prefix: String) -> String {
        if let value = command.value, !value.isEmpty {
            return "\(prefix)-\(asciiSlug(command.type))-\(asciiSlug(value))"
        }
        return "\(prefix)-\(asciiSlug(command.type))"
    }

    private func asciiSlug(_ value: String) -> String {
        let scalars = value.unicodeScalars.map { scalar -> Character in
            if scalar.value >= 48 && scalar.value <= 57
                || scalar.value >= 65 && scalar.value <= 90
                || scalar.value >= 97 && scalar.value <= 122 {
                return Character(scalar)
            }
            return "-"
        }
        let slug = String(scalars)
            .split(separator: "-")
            .joined(separator: "-")
            .lowercased()
        return slug.isEmpty ? "unknown" : slug
    }

    private var pixel: CGFloat {
        1 / max(UIScreen.main.scale, 1)
    }

    private static let longPressDelayMs = 420
    private static let backspaceRepeatIntervalMs = 72
    private static let expandedCandidateLoadDelayMs = 180
    private static let emojiChoices = [
        "😀", "😁", "😂", "🤣", "😊", "😍", "😘", "😎",
        "🥰", "😇", "🙂", "😉", "😋", "🤔", "😭", "😡",
        "👍", "👎", "👌", "🙏", "👏", "💪", "🔥", "✨",
        "🎉", "❤️", "💔", "⭐", "🌟", "✅", "❌", "❓",
        "☕", "🍵", "🍻", "🍚", "🍜", "🌙", "☀️", "🌧️",
    ]
}

private extension Int {
    var clampedColor: Int {
        Swift.min(Swift.max(self, 0), 255)
    }
}
