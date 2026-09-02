import UIKit

protocol KeyTaoIOSKeyboardViewDelegate: AnyObject {
    func keyboardView(_ view: KeyTaoIOSKeyboardView, didTrigger command: KeyTaoKeyCommand)
    func keyboardView(_ view: KeyTaoIOSKeyboardView, didSelectCandidate index: Int, global: Bool)
    func keyboardView(_ view: KeyTaoIOSKeyboardView, requestExpandedCandidates completion: @escaping ([KeyTaoCandidate]) -> Void)
    func keyboardView(_ view: KeyTaoIOSKeyboardView, requestClipboardHistory completion: @escaping ([String]) -> Void)
    func keyboardView(_ view: KeyTaoIOSKeyboardView, deleteClipboardEntry text: String)
    func keyboardViewClearClipboardHistory(_ view: KeyTaoIOSKeyboardView)
    func keyboardViewCanUndo(_ view: KeyTaoIOSKeyboardView) -> Bool
}

private final class KeyTaoActivatingAccessibilityElement: UIAccessibilityElement {
    var activation: (() -> Bool)?

    override func accessibilityActivate() -> Bool {
        activation?() ?? false
    }
}

private enum KeyTaoFunctionPanelMode {
    case rime
    case clipboard
}

private enum KeyTaoPanelItemStyle {
    case standard
    case section
    case schema
    case option
}

private enum KeyTaoToolbarIcon {
    case function
    case selection
    case clipboard
    case emoji
    case layout
    case back
    case settings
}

final class KeyTaoIOSKeyboardView: UIView {
    weak var delegate: KeyTaoIOSKeyboardViewDelegate?

    private struct KeyRect {
        var spec: KeyTaoKeySpec
        var rect: CGRect
        var sticky: Bool = false
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
        var drawingRect: CGRect? = nil
    }

    private struct ClipboardDeleteRect {
        var text: String
        var rect: CGRect
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
        var clipboardText: String? = nil
        var style: KeyTaoPanelItemStyle = .standard
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
    private var layoutPresentation = KeyTaoIOSKeyboardLayoutPresentation(
        mode: .full,
        alternativeMode: .oneHanded,
        side: .right
    )
    private var availabilityMessage: String?
    private var layerMode: KeyTaoKeyboardLayer = .letters
    private var shiftState: KeyTaoShiftState = .off
    private var showsInputModeSwitchKey = true
    private var capabilities = KeyTaoEngineCapabilities.current
    private var hostTraits = KeyTaoHostTraits.default
    private var hapticsAvailable = true
    private var lastShiftTap = Date.distantPast
    private var functionPanelActive = false
    private var functionPanelMode: KeyTaoFunctionPanelMode = .rime
    private var rimeOptionsState = KeyTaoRimeOptionsState.empty
    private var rimeOptionsLoading = false
    private var expandedCandidates: [KeyTaoCandidate] = []
    private var expandedCandidatesLoading = false
    private var clipboardItemsLoading = false
    private var clipboardItems: [String] = []
    private var clipboardClearConfirmationPending = false
    private var expandedCandidateItemsCacheSignature = ""
    private var expandedCandidateItemsCache: [CandidateDrawItem] = []
    private var expandedCandidateScrollY: CGFloat = 0
    private var expandedCandidateContentHeight: CGFloat = 0
    private var keyboardScrollY: CGFloat = 0
    private var keyboardTouchStartY: CGFloat = 0
    private var keyboardTouchStartScrollY: CGFloat = 0
    private var keyboardDragging = false
    private var keyboardScrollTouchActive = false
    private var keyboardScrollContentHeight: CGFloat = 0
    private var keyboardScrollViewportHeight: CGFloat = 0
    private var keyboardScrollViewportTop: CGFloat = 0
    private var keyboardScrollViewportBottom: CGFloat = 0
    private var expandRequestToken = 0

    private var keyRects: [KeyRect] = []
    private var inlineCandidateRects: [CandidateRect] = []
    private var expandedCandidateRects: [CandidateRect] = []
    private var expandedSectionRects: [Int: CGRect] = [:]
    private var clipboardDeleteRects: [ClipboardDeleteRect] = []
    private var candidateRects: [CandidateRect] {
        inlineCandidateRects + expandedCandidateRects
    }
    private var toolbarRects: [ToolbarRect] = []
    private var candidateExpandRect: CGRect?
    private var candidatePanelExpanded = false
    private var pressedKey: KeyRect?
    private var pressedToolbar: ToolbarRect?
    private var pressedCandidate: CandidateRect?
    private var pressedClipboardDelete: ClipboardDeleteRect?
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
    private var cachedLogoImage: UIImage?
    private var logoImage: UIImage? {
        if let cachedLogoImage {
            return cachedLogoImage
        }
        cachedLogoImage = Self.loadLogoImage()
        return cachedLogoImage
    }

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
        resetKeyboardScroll()
        invalidateLayoutAndDisplay()
    }

    func currentConfig() -> KeyTaoIOSImeConfig {
        config
    }

    func update(theme: KeyTaoImeTheme) {
        self.theme = theme
        invalidateLayoutAndDisplay()
    }

    func updateLayoutPresentation(_ presentation: KeyTaoIOSKeyboardLayoutPresentation) {
        guard layoutPresentation != presentation else {
            return
        }
        layoutPresentation = presentation
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
        guard showsInputModeSwitchKey != visible else {
            return
        }
        showsInputModeSwitchKey = visible
        invalidateLayoutAndDisplay()
    }

    func update(rimeOptions: KeyTaoRimeOptionsState) {
        rimeOptionsState = rimeOptions
        rimeOptionsLoading = false
        resetExpandedCandidateScroll()
        expandedCandidateItemsCacheSignature = ""
        expandedCandidateItemsCache = []
        invalidateLayoutAndDisplay()
    }

    /// Controls that need a librime entry point the running build does not
    /// export are dropped instead of being drawn and then synthesizing a key
    /// stroke: paging keys leave the layout, candidates stop taking taps.
    func update(engineCapabilities: KeyTaoEngineCapabilities) {
        guard capabilities != engineCapabilities else {
            return
        }
        capabilities = engineCapabilities
        invalidateLayoutAndDisplay()
    }

    func update(hostTraits: KeyTaoHostTraits) {
        guard self.hostTraits != hostTraits else {
            return
        }
        self.hostTraits = hostTraits
        if state.asciiMode, hostTraits.autocapitalizationType == .allCharacters {
            shiftState = .locked
        }
        invalidateLayoutAndDisplay()
    }

    /// Called when the keyboard enters a fresh input context so that a host
    /// asking for capitalisation starts with the one-shot shift armed. Only
    /// meaningful in English mode: shifted letters are not part of any Rime
    /// speller alphabet, so Chinese input must never start capitalised.
    func resetShiftForNewContext() {
        guard state.asciiMode else {
            if shiftState == .once {
                shiftState = .off
                invalidateLayoutAndDisplay()
            }
            return
        }
        let next: KeyTaoShiftState = hostTraits.autocapitalizationType == .allCharacters
            ? .locked
            : (hostTraits.wantsAutoShift ? .once : .off)
        // Never stomp a caps-lock the user set by hand.
        guard shiftState != .locked, shiftState != next else {
            return
        }
        shiftState = next
        lastShiftTap = .distantPast
        invalidateLayoutAndDisplay()
    }

    /// Frame of the "switch keyboard" key in this view's coordinate space, so
    /// that the controller can park its handleInputModeList overlay on it.
    func inputModeSwitchKeyFrame() -> CGRect? {
        guard !candidatePanelExpanded, !functionPanelActive else {
            return nil
        }
        return keyRects.first { $0.spec.action?.type == KeyTaoCommandType.keyboardPicker }?.rect
    }

    func releaseCaches() {
        cancelExpandedCandidateRequest()
        expandedCandidates = []
        expandedCandidatesLoading = false
        clipboardItems = []
        clipboardClearConfirmationPending = false
        expandedCandidateItemsCacheSignature = ""
        expandedCandidateItemsCache = []
        cachedLogoImage = nil
        invalidateLayoutAndDisplay()
    }

    /// Haptics need Full Access; without it `UIFeedbackGenerator` silently does
    /// nothing, so the keyboard stops asking for it.
    func update(hapticsAvailable: Bool) {
        self.hapticsAvailable = hapticsAvailable
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
        functionPanelMode = .rime
        rimeOptionsState = .empty
        rimeOptionsLoading = false
        clipboardClearConfirmationPending = false
        expandedCandidates = []
        cancelExpandedCandidateRequest()
        clipboardItemsLoading = false
        resetExpandedCandidateScroll()
        shiftState = .off
        pressedKey = nil
        pressedToolbar = nil
        resetKeyboardScroll()
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
        drawLayoutInteractionHints()
    }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        guard let point = touches.first?.location(in: self) else {
            return
        }
        stopLongPressAndRepeat()
        touchStart = point
        currentTouchPoint = point
        touchStartScrollY = expandedCandidateScrollY
        keyboardTouchStartY = point.y
        keyboardTouchStartScrollY = keyboardScrollY
        expandedDragging = false
        keyboardDragging = false
        longPressConsumed = false
        backspaceGestureUnits = 0
        backspaceGestureConsumed = false
        candidateExpandPressed = !functionPanelActive
            && !state.candidatePanel.candidates.isEmpty
            && point.y < config.candidateBarHeightDp
            && candidateExpandRect?.contains(point) == true
        pressedToolbar = point.y < config.candidateBarHeightDp ? toolbarRects.first { $0.rect.contains(point) } : nil
        pressedCandidate = nil
        pressedClipboardDelete = nil
        expandedTouchActive = false
        if pressedToolbar == nil && !candidateExpandPressed && point.y < config.candidateBarHeightDp {
            pressedCandidate = inlineCandidateRects.first { $0.rect.contains(point) }
        } else if pressedToolbar == nil && !candidateExpandPressed && candidatePanelExpanded && point.y >= config.candidateBarHeightDp {
            expandedTouchActive = true
            pressedClipboardDelete = clipboardDeleteRects.first { $0.rect.contains(point) }
            if pressedClipboardDelete == nil {
                pressedCandidate = expandedCandidateRects.first { $0.rect.contains(point) }
            }
        }
        keyboardScrollTouchActive = pressedToolbar == nil
            && pressedCandidate == nil
            && !candidateExpandPressed
            && !expandedTouchActive
            && usesCategorizedSymbolKeyboard()
            && maxKeyboardScroll() > 0
            && point.y >= keyboardScrollViewportTop
            && point.y < keyboardScrollViewportBottom
        if pressedToolbar == nil && pressedCandidate == nil && !candidateExpandPressed && !expandedTouchActive {
            pressedKey = keyRects.first { isVisibleKey($0, at: point) && $0.rect.contains(point) }
            if let pressedKey, isUnsupportedEditKey(pressedKey.spec) {
                self.pressedKey = nil
                setNeedsDisplay()
                return
            }
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
                pressedClipboardDelete = nil
            }
            if expandedDragging {
                expandedCandidateScrollY = max(0, min(maxExpandedCandidateScroll(), touchStartScrollY - deltaY))
                invalidateLayoutAndDisplay()
            }
            return
        }
        if keyboardScrollTouchActive {
            let deltaY = point.y - keyboardTouchStartY
            if !keyboardDragging && abs(deltaY) > 6 {
                keyboardDragging = true
                stopLongPressAndRepeat()
                pressedKey = nil
            }
            if keyboardDragging {
                keyboardScrollY = max(0, min(maxKeyboardScroll(), keyboardTouchStartScrollY - deltaY))
                invalidateLayoutAndDisplay()
                return
            }
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

        if let clipboardDelete = pressedClipboardDelete,
           !expandedDragging,
           clipboardDelete.rect.contains(point) {
            clearPressedState()
            deleteClipboardEntry(clipboardDelete.text)
            return
        }

        if let candidate = pressedCandidate, !expandedDragging, candidate.rect.contains(point) {
            clearPressedState()
            if let command = candidate.command {
                handlePanelCommand(command)
            } else if !isSelectable(candidate) {
                // The controller refuses the selection and says why. Nothing is
                // committed, so the panel must not collapse and the tap must not
                // feel like it worked.
                delegate?.keyboardView(self, didSelectCandidate: candidate.selectIndex, global: candidate.global)
            } else {
                closeCandidatePanelIfNeeded(afterCandidateSelection: candidate.global)
                performConfiguredHaptic()
                delegate?.keyboardView(self, didSelectCandidate: candidate.selectIndex, global: candidate.global)
            }
            return
        }

        if keyboardScrollTouchActive {
            let wasDragging = keyboardDragging
            keyboardScrollTouchActive = false
            keyboardDragging = false
            if wasDragging {
                clearPressedState()
                invalidateLayoutAndDisplay()
                return
            }
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
        pressedClipboardDelete = nil
        candidateExpandPressed = false
        expandedTouchActive = false
        expandedDragging = false
        keyboardScrollTouchActive = false
        keyboardDragging = false
        backspaceGestureUnits = 0
        backspaceGestureConsumed = false
        setNeedsDisplay()
    }

    private func rebuildInteractiveRects() {
        keyRects = keyboardLayout()
        inlineCandidateRects = inlineCandidateLayout()
        if candidatePanelExpanded {
            expandedCandidateRects = expandedCandidateLayout()
        } else {
            expandedCandidateRects = []
            expandedSectionRects = [:]
            clipboardDeleteRects = []
        }
        toolbarRects = toolbarLayout()
        candidateExpandRect = expandButtonRect()
    }

    private func rebuildAccessibilityElements() {
        var elements: [UIAccessibilityElement] = []
        for key in keyRects {
            let element = UIAccessibilityElement(accessibilityContainer: self)
            element.accessibilityFrameInContainerSpace = key.rect
            element.accessibilityTraits = isUnsupportedEditKey(key.spec) ? [.button, .notEnabled] : .button
            element.accessibilityIdentifier = keyAccessibilityIdentifier(key.spec)
            element.accessibilityLabel = displayLabel(key.spec)
            elements.append(element)
        }
        for candidate in candidateRects {
            let element = UIAccessibilityElement(accessibilityContainer: self)
            element.accessibilityFrameInContainerSpace = candidate.rect
            // A candidate the runtime cannot select is still worth reading out,
            // but announcing it as a button would promise an action that the
            // controller is about to refuse.
            element.accessibilityTraits = isSelectable(candidate) ? .button : .staticText
            element.accessibilityIdentifier = "keytao-candidate-\(candidate.identifierIndex)"
            elements.append(element)
        }
        for (index, delete) in clipboardDeleteRects.enumerated() {
            let element = KeyTaoActivatingAccessibilityElement(accessibilityContainer: self)
            element.accessibilityFrameInContainerSpace = delete.rect
            element.accessibilityTraits = .button
            element.accessibilityIdentifier = "keytao-clipboard-delete-\(index)"
            element.accessibilityLabel = "删除剪贴板历史：\(delete.text)"
            element.activation = { [weak self] in
                self?.deleteClipboardEntry(delete.text)
                return self != nil
            }
            elements.append(element)
        }
        for toolbar in toolbarRects {
            let element = KeyTaoActivatingAccessibilityElement(accessibilityContainer: self)
            element.accessibilityFrameInContainerSpace = toolbar.rect
            element.accessibilityTraits = .button
            element.accessibilityIdentifier = commandAccessibilityIdentifier(toolbar.action.command, prefix: "keytao-toolbar")
            element.accessibilityLabel = toolbar.action.label
            element.activation = { [weak self] in
                self?.handleToolbarCommand(toolbar.action.command)
                return self != nil
            }
            elements.append(element)
        }
        accessibilityElements = elements
    }

    private func drawBackground() {
        guard layoutPresentation.isCompact else {
            // The root UIInputView(.keyboard) supplies the system keyboard material.
            return
        }
        let borderWidth = max(0.5, theme.panel.borderWidth)
        let inset = borderWidth / 2
        let panelRect = bounds.insetBy(dx: inset, dy: inset)
        let path = UIBezierPath(
            roundedRect: panelRect,
            cornerRadius: theme.panel.cornerRadius
        )
        theme.panel.background.uiColor.setFill()
        path.fill()
        theme.panel.borderColor.uiColor.setStroke()
        path.lineWidth = borderWidth
        path.stroke()
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

        if usesFullHeightSymbolKeyboard() {
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
        if usesCategorizedSymbolKeyboard(), let context = UIGraphicsGetCurrentContext() {
            context.saveGState()
            UIBezierPath(rect: CGRect(
                x: 0,
                y: keyboardScrollViewportTop,
                width: bounds.width,
                height: max(0, keyboardScrollViewportBottom - keyboardScrollViewportTop)
            )).addClip()
            for key in keyRects where !key.sticky {
                let pressed = pressedKey?.spec == key.spec
                drawKey(key.spec, rect: key.rect, pressed: pressed, pressedStackIndex: pressedStackIndex(for: key))
            }
            context.restoreGState()
            for key in keyRects where key.sticky {
                let pressed = pressedKey?.spec == key.spec
                drawKey(key.spec, rect: key.rect, pressed: pressed, pressedStackIndex: pressedStackIndex(for: key))
            }
            return
        }
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
            let drawingRect = expandedCandidateRects
                .first(where: { $0.identifierIndex == item.identifierIndex })
                .map { $0.drawingRect ?? $0.rect }
                ?? expandedSectionRects[item.identifierIndex]
            guard let drawingRect else {
                continue
            }
            drawCandidateOption(item, rect: drawingRect)
        }
    }

    private func drawFunctionPanelBar() {
        for toolbar in toolbarRects {
            drawToolbarChip(toolbar)
        }
        if functionPanelMode != .clipboard || clipboardItems.isEmpty {
            drawText(
                functionPanelTitle(),
                in: CGRect(x: 0, y: 0, width: bounds.width, height: config.candidateBarHeightDp),
                color: theme.candidate.commentColor.uiColor,
                size: theme.font.labelSize,
                weight: theme.font.weight,
                alignment: .center
            )
        }

        if expandedCandidatesLoading || clipboardItemsLoading || rimeOptionsLoading {
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
        if item.style == .section {
            drawRimeSectionHeader(item, rect: rect)
            return
        }
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

        switch item.style {
        case .schema:
            drawRimeSchemaRow(item, rect: rect)
        case .option:
            drawRimeOptionPill(item, rect: rect)
        default:
            switch panelColumns(for: functionPanelActive ? functionPanelMode : .rime) {
            case 4:
                drawCandidateGridCell(item, rect: rect)
            case 1:
                drawClipboardCandidateRow(item, rect: rect)
            default:
                drawInlineCandidateOption(item, rect: rect)
            }
        }
    }

    private func drawRimeSectionHeader(_ item: CandidateDrawItem, rect: CGRect) {
        let labelRect = CGRect(x: rect.minX + 4, y: rect.minY, width: rect.width - 4, height: rect.height)
        let font = themedFont(size: candidateLabelSize(), weight: theme.font.weight)
        let labelWidth = item.label.size(withAttributes: [.font: font]).width
        drawText(
            item.label,
            in: labelRect,
            color: theme.candidate.commentColor.uiColor,
            font: font,
            alignment: .left
        )
        let lineLeft = labelRect.minX + labelWidth + 10
        if lineLeft < rect.maxX {
            let line = UIBezierPath()
            line.move(to: CGPoint(x: lineLeft, y: rect.midY))
            line.addLine(to: CGPoint(x: rect.maxX, y: rect.midY))
            line.lineWidth = max(pixel, theme.candidate.borderWidth)
            theme.panel.borderColor.uiColor.setStroke()
            line.stroke()
        }
    }

    private func drawRimeSchemaRow(_ item: CandidateDrawItem, rect: CGRect) {
        let center = CGPoint(x: rect.minX + 18, y: rect.midY)
        let radio = UIBezierPath(arcCenter: center, radius: 7, startAngle: 0, endAngle: .pi * 2, clockwise: true)
        radio.lineWidth = item.selected ? 2 : 1.4
        (item.selected ? theme.candidate.selectedForeground.uiColor : theme.candidate.commentColor.uiColor).setStroke()
        radio.stroke()
        if item.selected {
            theme.candidate.selectedForeground.uiColor.setFill()
            UIBezierPath(arcCenter: center, radius: 3.5, startAngle: 0, endAngle: .pi * 2, clockwise: true).fill()
        }

        let textLeft = rect.minX + 36
        let textWidth = max(0, rect.maxX - 10 - textLeft)
        drawTruncatedText(
            item.label,
            in: CGRect(x: textLeft, y: rect.minY + 3, width: textWidth, height: 23),
            color: item.selected ? theme.candidate.selectedForeground.uiColor : theme.candidate.foreground.uiColor,
            size: candidateTextSize(),
            alignment: .left
        )
        drawTruncatedText(
            item.text,
            in: CGRect(x: textLeft, y: rect.midY, width: textWidth, height: 19),
            color: item.selected ? theme.candidate.selectedCommentColor.uiColor : theme.candidate.commentColor.uiColor,
            size: candidateCommentSize(),
            alignment: .left
        )
    }

    private func drawRimeOptionPill(_ item: CandidateDrawItem, rect: CGRect) {
        let statusRect = CGRect(x: rect.maxX - 50, y: rect.midY - 11, width: 42, height: 22)
        let textLeft = rect.minX + 10
        let textWidth = max(0, statusRect.minX - 8 - textLeft)
        drawTruncatedText(
            item.label,
            in: CGRect(x: textLeft, y: rect.minY + 3, width: textWidth, height: 23),
            color: item.selected ? theme.candidate.selectedForeground.uiColor : theme.candidate.foreground.uiColor,
            size: candidateLabelSize(),
            alignment: .left
        )
        drawTruncatedText(
            item.text,
            in: CGRect(x: textLeft, y: rect.midY, width: textWidth, height: 19),
            color: item.selected ? theme.candidate.selectedCommentColor.uiColor : theme.candidate.commentColor.uiColor,
            size: candidateCommentSize(),
            alignment: .left
        )
        (item.selected ? theme.candidate.selectedForeground.uiColor : theme.candidate.borderColor.uiColor).setFill()
        UIBezierPath(roundedRect: statusRect, cornerRadius: 11).fill()
        drawText(
            item.selected ? "ON" : "OFF",
            in: statusRect,
            color: item.selected ? theme.candidate.selectedBackground.uiColor : theme.candidate.commentColor.uiColor,
            size: 10,
            weight: theme.font.weight,
            alignment: .center
        )
    }

    private func drawCandidateGridCell(_ item: CandidateDrawItem, rect: CGRect) {
        let selected = item.selected
        let contentRect = rect.insetBy(dx: 6, dy: 0)
        let labelRect = CGRect(x: contentRect.minX, y: rect.midY - 20, width: contentRect.width, height: 20)
        let captionRect = CGRect(x: contentRect.minX, y: rect.midY, width: contentRect.width, height: 20)
        drawTruncatedText(
            item.label,
            in: labelRect,
            color: selected ? theme.candidate.selectedForeground.uiColor : theme.candidate.foreground.uiColor,
            size: candidateTextSize(),
            alignment: .center
        )
        let caption = [item.text, item.comment]
            .compactMap { value in value?.isEmpty == false ? value : nil }
            .joined(separator: " ")
        drawTruncatedText(
            caption,
            in: captionRect,
            color: selected ? theme.candidate.selectedCommentColor.uiColor : theme.candidate.commentColor.uiColor,
            size: candidateCommentSize(),
            alignment: .center
        )
    }

    private func drawClipboardCandidateRow(_ item: CandidateDrawItem, rect: CGRect) {
        let selected = item.selected
        let paddingX = candidatePaddingX()
        let inlineGap = candidateInlineGap()
        let deleteWidth = config.clipboardDeleteHitWidthDp
        let deleteRect = CGRect(x: rect.maxX - deleteWidth, y: rect.minY, width: deleteWidth, height: rect.height)
        var x = rect.minX + paddingX
        if !item.label.isEmpty {
            x += drawInlineText(
                item.label,
                x: x,
                centerY: rect.midY,
                color: selected ? theme.candidate.selectedLabelColor.uiColor : theme.candidate.labelColor.uiColor,
                size: candidateLabelSize(),
                weight: theme.font.weight
            ) + inlineGap
        }
        drawTruncatedText(
            item.text,
            in: CGRect(x: x, y: rect.minY, width: max(0, deleteRect.minX - paddingX - x), height: rect.height),
            color: selected ? theme.candidate.selectedForeground.uiColor : theme.candidate.foreground.uiColor,
            size: candidateTextSize(),
            alignment: .left
        )

        let divider = UIBezierPath()
        divider.move(to: CGPoint(x: deleteRect.minX, y: rect.minY + 7))
        divider.addLine(to: CGPoint(x: deleteRect.minX, y: rect.maxY - 7))
        divider.lineWidth = max(pixel, 0.7)
        theme.candidate.borderColor.uiColor.setStroke()
        divider.stroke()
        drawText(
            "✕",
            in: deleteRect,
            color: theme.candidate.commentColor.uiColor,
            size: 18,
            weight: theme.font.weight,
            alignment: .center
        )
    }

    private func drawInlineCandidateOption(_ item: CandidateDrawItem, rect: CGRect) {
        let selected = item.selected
        let paddingX = candidatePaddingX()
        let inlineGap = candidateInlineGap()
        var x = rect.minX + paddingX
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
        let gap: CGFloat = rect.width < 44 ? 3 : 5
        let total = primaryWidth + secondaryWidth + gap
        let maxWidth = max(1, rect.width - 10)
        if total > maxWidth {
            let scale = max(0.6, maxWidth / total)
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
        case .layout:
            drawLayoutIcon(in: iconRect)
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

    private func drawLayoutIcon(in rect: CGRect) {
        let window = CGRect(
            x: rect.minX + rect.width * 0.06,
            y: rect.minY + rect.height * 0.14,
            width: rect.width * 0.88,
            height: rect.height * 0.72
        )
        UIBezierPath(roundedRect: window, cornerRadius: rect.width * 0.10).stroke()
        switch layoutPresentation.displayedMode {
        case .oneHanded:
            let width = window.width * 0.54
            let x = layoutPresentation.side == .left ? window.minX : window.maxX - width
            UIBezierPath(
                roundedRect: CGRect(x: x, y: window.minY, width: width, height: window.height),
                cornerRadius: rect.width * 0.07
            ).stroke()
        case .split:
            let gap = window.width * 0.18
            let width = (window.width - gap) / 2
            UIBezierPath(
                roundedRect: CGRect(x: window.minX, y: window.minY, width: width, height: window.height),
                cornerRadius: rect.width * 0.07
            ).stroke()
            UIBezierPath(
                roundedRect: CGRect(x: window.maxX - width, y: window.minY, width: width, height: window.height),
                cornerRadius: rect.width * 0.07
            ).stroke()
        case .full:
            break
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
        let unsupported = isUnsupportedEditKey(key)
        if unsupported {
            UIGraphicsGetCurrentContext()?.saveGState()
            UIGraphicsGetCurrentContext()?.setAlpha(0.38)
        }
        defer {
            if unsupported {
                UIGraphicsGetCurrentContext()?.restoreGState()
            }
        }
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
        shadow.origin.y += pressed ? 0.8 : 1.6
        UIColor(
            red: 26 / 255,
            green: 34 / 255,
            blue: 44 / 255,
            alpha: CGFloat(pressed ? 18 : 28) / 255
        ).setFill()
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

    private func drawTruncatedText(
        _ text: String,
        in rect: CGRect,
        color: UIColor,
        size: CGFloat,
        alignment: NSTextAlignment
    ) {
        let paragraph = NSMutableParagraphStyle()
        paragraph.alignment = alignment
        paragraph.lineBreakMode = .byTruncatingTail
        let font = themedFont(size: size, weight: theme.font.weight)
        let attributes: [NSAttributedString.Key: Any] = [
            .font: font,
            .foregroundColor: color,
            .paragraphStyle: paragraph,
        ]
        let lineRect = CGRect(x: rect.minX, y: rect.midY - font.lineHeight / 2, width: rect.width, height: font.lineHeight)
        text.draw(
            with: lineRect,
            options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine],
            attributes: attributes,
            context: nil
        )
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
        var next: [KeyRect] = []
        let maximumRowWidth = max(1, bounds.width - keyboardOuterInset() * 2)
        let referenceUnitWidth = keyboardReferenceUnitWidth(rows: rows, horizontalGap: horizontalGap)

        func appendSplitRow(
            _ row: [KeyTaoKeySpec],
            rowIndex: Int,
            y: CGFloat,
            rowHeight: CGFloat,
            sticky: Bool,
            hasLeadingSpans: Bool
        ) -> Bool {
            guard layoutPresentation.mode == .split,
                  layerMode == .letters,
                  rowIndex < rows.count - 1,
                  row.count >= 7,
                  !hasLeadingSpans,
                  row.allSatisfy({ keyRowSpan($0) == 1 }) else {
                return false
            }
            let splitIndex = (row.count + 1) / 2
            let leftKeys = Array(row[..<splitIndex])
            let rightKeys = Array(row[splitIndex...])
            let centerGap = min(96, max(52, bounds.width * 0.10))
            let groupAvailableWidth = max(1, (maximumRowWidth - centerGap) / 2)
            let referenceWeight = max(rowWeight(leftKeys), rowWeight(rightKeys))
            let referenceCount = max(leftKeys.count, rightKeys.count)
            let referenceGapWidth = horizontalGap * CGFloat(max(0, referenceCount - 1))
            let unitWidth = max(1, (groupAvailableWidth - referenceGapWidth) / referenceWeight)

            func appendGroup(_ keys: [KeyTaoKeySpec], startX: CGFloat) {
                var x = startX
                for key in keys {
                    let width = unitWidth * keyWeight(key)
                    let rect = CGRect(x: x, y: y, width: width, height: rowHeight)
                    next.append(KeyRect(spec: key, rect: rect, sticky: sticky))
                    x = rect.maxX + horizontalGap
                }
            }

            let rightWidth = unitWidth * rowWeight(rightKeys)
                + horizontalGap * CGFloat(max(0, rightKeys.count - 1))
            appendGroup(leftKeys, startX: keyboardOuterInset())
            appendGroup(rightKeys, startX: bounds.width - keyboardOuterInset() - rightWidth)
            return true
        }

        func appendRows(
            _ layoutRows: [[KeyTaoKeySpec]],
            rowIndexOffset: Int,
            startY: CGFloat,
            rowHeight: CGFloat,
            verticalGap: CGFloat,
            sticky: Bool
        ) {
            var y = startY
            var activeLeadingSpans: [ActiveRowSpan] = []
            for (localRowIndex, row) in layoutRows.enumerated() {
                let rowIndex = rowIndexOffset + localRowIndex
                guard !row.isEmpty else {
                    activeLeadingSpans = advanceRowSpans(activeLeadingSpans)
                    y += rowHeight + verticalGap
                    continue
                }
                if appendSplitRow(
                    row,
                    rowIndex: rowIndex,
                    y: y,
                    rowHeight: rowHeight,
                    sticky: sticky,
                    hasLeadingSpans: !activeLeadingSpans.isEmpty
                ) {
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
                    next.append(KeyRect(spec: key, rect: rect, sticky: sticky))
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
        }

        if usesCategorizedSymbolKeyboard(rows) {
            let targetVisibleRows = CGFloat(min(5, rows.count))
            let rowHeight = min(
                max(40, (availableHeight - verticalGapFloor * (targetVisibleRows + 1)) / targetVisibleRows),
                keyboardMaxKeyHeight()
            )
            let verticalGap = verticalGapFloor
            let headerRow = Array(rows.prefix(1))
            let bodyRows = Array(rows.dropFirst().dropLast())
            let footerRow = Array(rows.suffix(1))
            let headerTop = top + verticalGap
            let footerTop = bottom - verticalGap - rowHeight
            keyboardScrollViewportTop = headerTop + rowHeight + verticalGap
            keyboardScrollViewportBottom = max(keyboardScrollViewportTop, footerTop - verticalGap)
            keyboardScrollViewportHeight = max(0, keyboardScrollViewportBottom - keyboardScrollViewportTop)
            keyboardScrollContentHeight = max(
                0,
                CGFloat(bodyRows.count) * rowHeight + CGFloat(max(0, bodyRows.count - 1)) * verticalGap
            )
            keyboardScrollY = max(0, min(maxKeyboardScroll(), keyboardScrollY))
            appendRows(
                headerRow,
                rowIndexOffset: 0,
                startY: headerTop,
                rowHeight: rowHeight,
                verticalGap: verticalGap,
                sticky: true
            )
            appendRows(
                bodyRows,
                rowIndexOffset: 1,
                startY: keyboardScrollViewportTop - keyboardScrollY,
                rowHeight: rowHeight,
                verticalGap: verticalGap,
                sticky: false
            )
            appendRows(
                footerRow,
                rowIndexOffset: rows.count - 1,
                startY: footerTop,
                rowHeight: rowHeight,
                verticalGap: verticalGap,
                sticky: true
            )
        } else {
            let naturalRowHeight = max(36, (availableHeight - verticalGapFloor * (rowCount + 1)) / rowCount)
            let rowHeight = min(naturalRowHeight, keyboardMaxKeyHeight())
            let verticalGap = max(verticalGapFloor, (availableHeight - rowHeight * rowCount) / (rowCount + 1))
            keyboardScrollY = 0
            keyboardScrollContentHeight = availableHeight
            keyboardScrollViewportHeight = availableHeight
            keyboardScrollViewportTop = top
            keyboardScrollViewportBottom = bottom
            appendRows(
                rows,
                rowIndexOffset: 0,
                startY: top + verticalGap,
                rowHeight: rowHeight,
                verticalGap: verticalGap,
                sticky: false
            )
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
        let columns = panelColumns(for: functionPanelActive ? functionPanelMode : .rime)
        let defaultRowHeight: CGFloat
        switch columns {
        case 4:
            defaultRowHeight = 52
        case 1:
            defaultRowHeight = config.clipboardRowHeightDp
        default:
            defaultRowHeight = 36
        }
        let cellWidth = columns.map { columnCount in
            (right - left - gap * CGFloat(columnCount - 1)) / CGFloat(columnCount)
        }
        var x = left
        var y = top + gap - expandedCandidateScrollY
        var contentBottom = top + gap
        var rects: [CandidateRect] = []
        var sectionRects: [Int: CGRect] = [:]
        var deleteRects: [ClipboardDeleteRect] = []
        let structuredRime = functionPanelActive && functionPanelMode == .rime
        for (index, item) in expandedCandidateItems().enumerated() {
            let width: CGFloat
            let itemRowHeight: CGFloat
            if structuredRime {
                switch item.style {
                case .section:
                    if x > left {
                        x = left
                        y += 44 + gap
                    }
                    width = right - left
                    itemRowHeight = 28
                case .schema:
                    if x > left {
                        x = left
                        y += 44 + gap
                    }
                    width = right - left
                    itemRowHeight = 44
                case .option:
                    width = (right - left - gap) / 2
                    itemRowHeight = 44
                case .standard:
                    width = right - left
                    itemRowHeight = defaultRowHeight
                }
            } else if let columns, let cellWidth {
                let column = index % columns
                let row = index / columns
                x = left + CGFloat(column) * (cellWidth + gap)
                y = top + gap + CGFloat(row) * (defaultRowHeight + gap) - expandedCandidateScrollY
                width = cellWidth
                itemRowHeight = defaultRowHeight
            } else {
                width = min(max(candidateWidth(item), 56), right - left)
                if x + width > right && x > left {
                    x = left
                    y += defaultRowHeight + gap
                }
                itemRowHeight = defaultRowHeight
            }
            let drawingRect = CGRect(x: x, y: y, width: width, height: itemRowHeight)
            if drawingRect.maxY >= top && drawingRect.minY <= bottom {
                let hitRect: CGRect
                if let clipboardText = item.clipboardText {
                    hitRect = CGRect(
                        x: drawingRect.minX,
                        y: drawingRect.minY,
                        width: drawingRect.width - config.clipboardDeleteHitWidthDp,
                        height: drawingRect.height
                    )
                    deleteRects.append(
                        ClipboardDeleteRect(
                            text: clipboardText,
                            rect: CGRect(
                                x: hitRect.maxX,
                                y: drawingRect.minY,
                                width: config.clipboardDeleteHitWidthDp,
                                height: drawingRect.height
                            )
                        )
                    )
                } else {
                    hitRect = drawingRect
                }
                if item.style == .section {
                    sectionRects[item.identifierIndex] = drawingRect
                } else {
                    rects.append(
                        CandidateRect(
                            identifierIndex: item.identifierIndex,
                            selectIndex: item.selectIndex,
                            rect: hitRect,
                            global: item.global,
                            command: item.command,
                            drawingRect: drawingRect
                        )
                    )
                }
            }
            contentBottom = max(contentBottom, drawingRect.maxY + expandedCandidateScrollY)
            if structuredRime {
                switch item.style {
                case .section, .schema, .standard:
                    x = left
                    y = drawingRect.maxY + gap
                case .option:
                    if x > left {
                        x = left
                        y = drawingRect.maxY + gap
                    } else {
                        x = drawingRect.maxX + gap
                    }
                }
            } else if columns == nil {
                x += width + gap
            }
        }
        expandedCandidateContentHeight = max(contentBottom - top + gap, expandedCandidatePanelHeight())
        expandedSectionRects = sectionRects
        clipboardDeleteRects = deleteRects
        coerceExpandedCandidateScroll()
        return rects
    }

    private func panelColumns(for mode: KeyTaoFunctionPanelMode) -> Int? {
        switch mode {
        case .clipboard:
            return 1
        case .rime:
            return nil
        }
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
            let pasteAction = ToolbarAction(label: "粘贴", command: .edit("paste"))
            let clearAction = ToolbarAction(
                label: clipboardClearConfirmationPending ? "确认清空" : "清空",
                command: .panel("clearClipboardHistory")
            )
            let settingsAction = ToolbarAction(
                label: "设置",
                command: KeyTaoKeyCommand(type: KeyTaoCommandType.openPage, value: "settings", fallbackValue: nil),
                icon: .settings
            )
            let backWidth = toolbarChipWidth(backAction)
            let pasteWidth = toolbarChipWidth(pasteAction)
            let clearWidth = toolbarChipWidth(clearAction)
            let settingsWidth = toolbarChipWidth(settingsAction)
            var rects = [
                ToolbarRect(
                    action: backAction,
                    rect: CGRect(x: leftPadding, y: top, width: backWidth, height: chipHeight)
                ),
            ]
            if functionPanelMode == .clipboard {
                rects.append(
                    ToolbarRect(
                        action: pasteAction,
                        rect: CGRect(x: leftPadding + backWidth + 6, y: top, width: pasteWidth, height: chipHeight)
                    )
                )
                if !clipboardItems.isEmpty {
                    rects.append(
                        ToolbarRect(
                            action: clearAction,
                            rect: CGRect(
                                x: leftPadding + backWidth + 6 + pasteWidth + 6,
                                y: top,
                                width: clearWidth,
                                height: chipHeight
                            )
                        )
                    )
                }
            }
            rects.append(
                ToolbarRect(
                    action: settingsAction,
                    rect: CGRect(x: bounds.width - leftPadding - settingsWidth, y: top, width: settingsWidth, height: chipHeight)
                )
            )
            return rects
        }
        if usesFullHeightSymbolKeyboard() {
            return []
        }
        guard state.candidatePanel.candidates.isEmpty, (state.candidatePanel.preedit ?? state.preedit).isEmpty else {
            return []
        }
        let barHeight = config.candidateBarHeightDp
        let leftPadding = theme.panel.gap * 1.5
        let compactToolbar = bounds.width < 300
        let logoLeft = logoRect().minX
        let maxRight = logoLeft - (compactToolbar ? 4 : 8)
        let chipHeight = min(34, barHeight - 12)
        let top = (barHeight - chipHeight) / 2
        let actions = toolbarActions()
        let gap = toolbarGap(for: actions, availableWidth: max(0, maxRight - leftPadding))
        let widths = toolbarChipWidths(for: actions, availableWidth: max(0, maxRight - leftPadding), gap: gap)
        var x = leftPadding
        var rects: [ToolbarRect] = []
        for (action, width) in zip(actions, widths) {
            let remainingWidth = maxRight - x
            if remainingWidth <= 0 {
                break
            }
            let rect = CGRect(x: x, y: top, width: min(width, remainingWidth), height: chipHeight)
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
        let size: CGFloat = bounds.width < 300 ? 24 : 30
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
        let items: [CandidateDrawItem]
        if functionPanelActive {
            switch functionPanelMode {
            case .rime:
                items = rimePanelItems()
            case .clipboard:
                items = clipboardPanelItems()
            }
        } else {
            items = rimePanelItems()
        }
        let signature = expandedCandidateItemsSignature(items)
        if signature == expandedCandidateItemsCacheSignature {
            return expandedCandidateItemsCache
        }
        expandedCandidateItemsCacheSignature = signature
        expandedCandidateItemsCache = items
        return items
    }

    private func expandedCandidateItemsSignature(_ items: [CandidateDrawItem]) -> String {
        let mode = functionPanelActive ? functionPanelMode : .rime
        let itemSignature = items.map { item in
            [
                String(item.identifierIndex),
                String(item.selectIndex),
                item.label,
                item.text,
                item.comment ?? "",
                item.selected ? "1" : "0",
                item.global ? "1" : "0",
                item.command?.type ?? "",
                item.command?.value ?? "",
                item.command?.fallbackValue ?? "",
                String(describing: item.style),
            ].joined(separator: "\u{0}")
        }.joined(separator: "\u{1}")
        return [
            functionPanelActive ? "1" : "0",
            functionPanelModeName(mode),
            panelColumns(for: mode).map { String($0) } ?? "flow",
            itemSignature,
        ].joined(separator: "|")
    }

    private func rimePanelItems() -> [CandidateDrawItem] {
        if functionPanelActive && functionPanelMode == .rime {
            if rimeOptionsLoading && rimeOptionsState == .empty {
                return []
            }
            var schemas = rimeOptionsState.schemas
            if let current = rimeOptionsState.currentSchema,
               !schemas.contains(where: { $0.id == current.id }) {
                schemas.append(current)
            }
            var items = [
                CandidateDrawItem(
                    identifierIndex: -2000,
                    selectIndex: -2000,
                    label: "输入方案",
                    text: "",
                    comment: nil,
                    selected: false,
                    global: false,
                    command: nil,
                    style: .section
                ),
            ]
            items.append(contentsOf: schemas.enumerated().map { index, schema in
                CandidateDrawItem(
                    identifierIndex: -2100 - index,
                    selectIndex: -2100 - index,
                    label: schema.name,
                    text: schema.id,
                    comment: nil,
                    selected: schema.id == rimeOptionsState.currentSchema?.id,
                    global: false,
                    command: KeyTaoKeyCommand(
                        type: KeyTaoCommandType.rimeSchema,
                        value: schema.id,
                        fallbackValue: nil
                    ),
                    style: .schema
                )
            })
            items.append(
                CandidateDrawItem(
                    identifierIndex: -3000,
                    selectIndex: -3000,
                    label: "选项",
                    text: "",
                    comment: nil,
                    selected: false,
                    global: false,
                    command: nil,
                    style: .section
                )
            )
            items.append(contentsOf: Self.rimeOptionSpecs.enumerated().map { index, spec in
                let enabled = rimeOptionsState.options[spec.name] == true
                return CandidateDrawItem(
                    identifierIndex: -3100 - index,
                    selectIndex: -3100 - index,
                    label: spec.label,
                    text: enabled ? spec.onLabel : spec.offLabel,
                    comment: nil,
                    selected: enabled,
                    global: false,
                    command: KeyTaoKeyCommand(
                        type: KeyTaoCommandType.rimeOption,
                        value: spec.name,
                        fallbackValue: String(!enabled)
                    ),
                    style: .option
                )
            })
            return items
        }
        let source = !expandedCandidates.isEmpty
            ? expandedCandidates
            : (!state.candidates.isEmpty
                ? state.candidates
                : state.candidatePanel.candidates.map {
                    KeyTaoCandidate(
                        text: $0.text,
                        comment: $0.comment,
                        index: panelCandidateGlobalIndex($0.index)
                    )
                })
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

    private func clipboardPanelItems() -> [CandidateDrawItem] {
        clipboardItems.enumerated().map { index, text in
            CandidateDrawItem(
                identifierIndex: -1000 - index,
                selectIndex: -1000 - index,
                label: "剪贴 \(index + 1)",
                text: String(text.prefix(120)),
                comment: nil,
                selected: false,
                global: false,
                command: .directInput(text),
                clipboardText: text
            )
        }
    }

    private struct PanelItem {
        var label: String
        var text: String
        var command: KeyTaoKeyCommand
        var comment: String?
    }

    private struct RimeOptionSpec {
        var name: String
        var label: String
        var onLabel: String
        var offLabel: String
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
        return labelWidth + bodyWidth + commentWidth + gaps + candidatePaddingX() * 2
    }

    private func toolbarChipWidth(_ action: ToolbarAction, horizontalPadding: CGFloat = 22, minimumWidth: CGFloat? = nil) -> CGFloat {
        if action.icon != nil && (action.secondaryLabel?.isEmpty ?? true) {
            return minimumWidth ?? 46
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
        return naturalTotal <= availableWidth ? naturalGap : 3
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

        let compact = actions.map { toolbarChipWidth($0, horizontalPadding: 12, minimumWidth: 38) }
        let compactTotal = compact.reduce(0, +) + gap * CGFloat(max(0, actions.count - 1))
        if compactTotal <= availableWidth {
            return compact
        }

        let minimums = actions.map { toolbarMinimumChipWidth($0) }
        let gapTotal = gap * CGFloat(max(0, actions.count - 1))
        let minimumContentWidth = minimums.reduce(0, +)
        let availableContentWidth = max(0, availableWidth - gapTotal)
        guard minimumContentWidth > 0 else {
            return minimums
        }
        if minimumContentWidth >= availableContentWidth {
            let scale = availableContentWidth / minimumContentWidth
            return minimums.map { $0 * scale }
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
            return 34
        }
        let labelWidth = textWidth(action.label, size: max(11, theme.font.labelSize * 0.72))
        let secondaryWidth = action.secondaryLabel.map { textWidth($0, size: max(10, theme.font.commentSize * 0.72)) } ?? 0
        let inlineGap: CGFloat = secondaryWidth > 0 ? 3 : 0
        return max(labelWidth + secondaryWidth + inlineGap + 6, secondaryWidth > 0 ? 38 : 34)
    }

    private func activeRows() -> [[KeyTaoKeySpec]] {
        var rows = config.rows(for: layerMode)
        if layerMode == .letters, shouldUseInlineNumberRow() {
            rows = rows.enumerated().map { index, row in
                index == 0 ? inlineNumberRow(row) : row
            }
        }
        rows = applyEngineCapabilities(to: rows)
        return applyInputModeSwitchKey(to: rows)
    }

    /// Without `RimeChangePage` the common layer degrades paging to a
    /// synthesized `-`/`=`. A schema that does not import the default
    /// `paging_with_minus_equal` bindings types those characters into the
    /// composition instead of turning the page, so the key is removed from the
    /// layout rather than offered as a control that corrupts the code (D4).
    /// The runtime is asked for the bit; nothing here assumes an OS.
    private func applyEngineCapabilities(to rows: [[KeyTaoKeySpec]]) -> [[KeyTaoKeySpec]] {
        guard !capabilities.nativePaging else {
            return rows
        }
        let filtered = rows
            .map { row in row.filter { !$0.isCandidatePagingKey } }
            .filter { !$0.isEmpty }
        // A layout made of nothing but paging keys would collapse the keyboard
        // and take Apple's mandatory switch key with it, so keep it drawn and
        // let the controller refuse the command instead.
        return filtered.isEmpty ? rows : filtered
    }

    /// Apple requires every custom keyboard to offer a way out to another
    /// keyboard whenever `needsInputModeSwitchKey` is true. The layout comes
    /// from user-editable YAML, so the key is enforced here instead of trusting
    /// whatever `keyboard.yaml` happens to contain.
    private func applyInputModeSwitchKey(to rows: [[KeyTaoKeySpec]]) -> [[KeyTaoKeySpec]] {
        guard !rows.isEmpty else {
            return rows
        }
        let hasSwitchKey = rows.contains { row in
            row.contains { $0.action?.type == KeyTaoCommandType.keyboardPicker }
        }
        if showsInputModeSwitchKey {
            guard !hasSwitchKey else {
                return rows
            }
            var next = rows
            let index = next.count - 1
            next[index] = insertingInputModeSwitchKey(into: next[index])
            return next
        }
        guard hasSwitchKey else {
            return rows
        }
        return rows.map { row in
            row.filter { $0.action?.type != KeyTaoCommandType.keyboardPicker }
        }
    }

    private func insertingInputModeSwitchKey(into row: [KeyTaoKeySpec]) -> [KeyTaoKeySpec] {
        let switchKey = KeyTaoKeySpec(
            label: "🌐",
            weight: Self.inputModeSwitchKeyWeight,
            action: KeyTaoKeyCommand(type: KeyTaoCommandType.keyboardPicker, value: nil, fallbackValue: nil)
        )
        var next = row
        // Keep the row's total weight stable by taking the width out of the
        // widest key (the space bar in every shipped layout).
        if let widest = next.indices.max(by: { keyWeight(next[$0]) < keyWeight(next[$1]) }),
           keyWeight(next[widest]) - Self.inputModeSwitchKeyWeight >= 1 {
            next[widest].weight = keyWeight(next[widest]) - Self.inputModeSwitchKeyWeight
        }
        next.insert(switchKey, at: 0)
        return next
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
            label: "Rime",
            command: KeyTaoKeyCommand(type: KeyTaoCommandType.panel, value: "rime", fallbackValue: nil),
            icon: .function
        )
        let languageToggle = languageToggleAction()
        let layoutModeName = layoutPresentation.displayedMode == .split ? "分栏" : "单手"
        let layout = ToolbarAction(
            label: layoutPresentation.isEnabled ? "退出\(layoutModeName)" : layoutModeName,
            command: KeyTaoKeyCommand(type: KeyTaoCommandType.floating, value: nil, fallbackValue: nil),
            selected: layoutPresentation.isEnabled,
            icon: .layout
        )
        if layerMode == .symbols {
            return [
                function,
                ToolbarAction(label: "中", command: KeyTaoKeyCommand(type: KeyTaoCommandType.mode, value: "chinese", fallbackValue: nil), selected: !state.asciiMode),
                ToolbarAction(label: "En", command: KeyTaoKeyCommand(type: KeyTaoCommandType.mode, value: "ascii", fallbackValue: nil), selected: state.asciiMode),
                ToolbarAction(label: "123", command: KeyTaoKeyCommand(type: KeyTaoCommandType.keyboardMode, value: "numbers", fallbackValue: nil)),
                ToolbarAction(label: "ABC", command: KeyTaoKeyCommand(type: KeyTaoCommandType.keyboardMode, value: "letters", fallbackValue: nil)),
                layout,
            ]
        } else {
            return [
                function,
                languageToggle,
                ToolbarAction(label: "选择", command: KeyTaoKeyCommand(type: KeyTaoCommandType.keyboardMode, value: "editor", fallbackValue: nil), icon: .selection),
                ToolbarAction(label: "剪贴板", command: KeyTaoKeyCommand(type: KeyTaoCommandType.panel, value: "clipboard", fallbackValue: nil), icon: .clipboard),
                ToolbarAction(label: "Emoji", command: KeyTaoKeyCommand(type: KeyTaoCommandType.keyboardMode, value: "symbols_emoji_face", fallbackValue: nil), icon: .emoji),
                layout,
            ]
        }
    }

    private func drawLayoutInteractionHints() {
        guard layoutPresentation.isCompact else {
            return
        }
        let color = theme.candidate.commentColor.uiColor.withAlphaComponent(0.6)
        color.setFill()
        let handleWidth = max(2, theme.candidate.borderWidth)
        let handleHeight = min(30, bounds.height * 0.12)
        let x = layoutPresentation.side == .left
            ? bounds.maxX - handleWidth - 2
            : bounds.minX + 2
        UIBezierPath(
            roundedRect: CGRect(
                x: x,
                y: bounds.midY - handleHeight / 2,
                width: handleWidth,
                height: handleHeight
            ),
            cornerRadius: handleWidth / 2
        ).fill()
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
            case "rime":
                openFunctionPanel(.rime)
                delegate?.keyboardView(self, didTrigger: KeyTaoKeyCommand(type: KeyTaoCommandType.rimeMenu, value: nil, fallbackValue: nil))
            case "clipboard":
                openFunctionPanel(.clipboard)
            case "clearClipboardHistory":
                handleClearClipboardHistory()
            default:
                setLayer("letters")
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
        functionPanelMode = .rime
        clipboardClearConfirmationPending = false
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
        functionPanelMode = .rime
        layerMode = .letters
        rimeOptionsState = .empty
        rimeOptionsLoading = false
        clipboardClearConfirmationPending = false
        expandedCandidates = []
        expandedCandidatesLoading = false
        clipboardItemsLoading = false
        cancelExpandedCandidateRequest()
        resetExpandedCandidateScroll()
        resetKeyboardScroll()
        expandedCandidateItemsCacheSignature = ""
        expandedCandidateItemsCache = []
    }

    private func closeCandidatePanelIfNeeded(afterCandidateSelection global: Bool) {
        guard global, candidatePanelExpanded, !functionPanelActive else {
            return
        }
        closeCandidatePanel()
    }

    private func openFunctionPanel(_ mode: KeyTaoFunctionPanelMode) {
        if mode != .clipboard || functionPanelMode != mode {
            clipboardClearConfirmationPending = false
        }
        functionPanelActive = true
        candidatePanelExpanded = true
        functionPanelMode = mode
        expandedCandidates = []
        cancelExpandedCandidateRequest()
        clipboardItemsLoading = mode == .clipboard
        rimeOptionsLoading = mode == .rime
        if rimeOptionsLoading {
            rimeOptionsState = .empty
        }
        resetExpandedCandidateScroll()
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
                if items.isEmpty {
                    self.clipboardClearConfirmationPending = false
                }
                self.clipboardItemsLoading = false
                self.resetExpandedCandidateScroll()
                self.invalidateLayoutAndDisplay()
            }
        })
    }

    private func deleteClipboardEntry(_ text: String) {
        guard functionPanelActive, functionPanelMode == .clipboard else {
            return
        }
        clipboardClearConfirmationPending = false
        performConfiguredHaptic()
        delegate?.keyboardView(self, deleteClipboardEntry: text)
        requestClipboardItemsAsync()
        invalidateLayoutAndDisplay()
    }

    private func handleClearClipboardHistory() {
        guard functionPanelActive, functionPanelMode == .clipboard, !clipboardItems.isEmpty else {
            clipboardClearConfirmationPending = false
            return
        }
        if !clipboardClearConfirmationPending {
            clipboardClearConfirmationPending = true
            return
        }
        clipboardClearConfirmationPending = false
        delegate?.keyboardViewClearClipboardHistory(self)
        requestClipboardItemsAsync()
    }

    private func canRequestExpandedCandidates() -> Bool {
        guard candidatePanelExpanded, !state.candidatePanel.candidates.isEmpty else {
            return false
        }
        return !functionPanelActive
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

    private func resetKeyboardScroll() {
        keyboardScrollY = 0
        keyboardTouchStartY = 0
        keyboardTouchStartScrollY = 0
        keyboardDragging = false
        keyboardScrollTouchActive = false
        keyboardScrollContentHeight = 0
        keyboardScrollViewportHeight = 0
        keyboardScrollViewportTop = keyboardTop()
        keyboardScrollViewportBottom = keyboardBottom()
    }

    private func maxExpandedCandidateScroll() -> CGFloat {
        max(0, expandedCandidateContentHeight - expandedCandidatePanelHeight())
    }

    private func maxKeyboardScroll() -> CGFloat {
        max(0, keyboardScrollContentHeight - keyboardScrollViewportHeight)
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
        case .rime:
            return "Rime 选项"
        case .clipboard:
            return "剪贴板"
        }
    }

    private func functionPanelModeName(_ mode: KeyTaoFunctionPanelMode) -> String {
        switch mode {
        case .rime:
            return "rime"
        case .clipboard:
            return "clipboard"
        }
    }

    private func expandedPanelEmptyMessage() -> String {
        if clipboardItemsLoading {
            return "正在读取剪贴板"
        }
        if rimeOptionsLoading && functionPanelMode == .rime {
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
        if layerMode.id.isSymbolLayer && key.isTextInputKey {
            return .directInput(valueForMode(key))
        }
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

    private var unsupportedEditVerbs: Set<String> {
        var verbs = Self.alwaysUnsupportedEditVerbs
        if delegate?.keyboardViewCanUndo(self) != true {
            verbs.insert("undo")
        }
        return verbs
    }

    private func isUnsupportedEditKey(_ key: KeyTaoKeySpec) -> Bool {
        let command = actionForMode(key)
        guard command.type == KeyTaoCommandType.edit, let value = command.value else {
            return false
        }
        return unsupportedEditVerbs.contains(value)
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
        if layerMode.id.isSymbolLayer && item.isTextInputItem {
            return .directInput(valueForMode(item))
        }
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
        key.longPress != nil || key.asciiLongPress != nil || key.hint?.isEmpty == false || isRepeatableKey(key)
    }

    private func isRepeatableKey(_ key: KeyTaoKeySpec) -> Bool {
        let command = actionForMode(key)
        return command.type == KeyTaoCommandType.backspace ||
            (command.type == KeyTaoCommandType.edit && Self.repeatableEditVerbs.contains(command.value ?? ""))
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
        if key.action?.type == KeyTaoCommandType.enter, let label = hostTraits.returnKeyLabel {
            return label
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

    private func valueForMode(_ key: KeyTaoKeySpec) -> String {
        if state.asciiMode {
            return key.asciiValue ?? key.value ?? key.label
        }
        return key.value ?? key.label
    }

    private func valueForMode(_ item: KeyTaoKeyStackItem) -> String {
        let value = item.value ?? item.label
        if state.asciiMode {
            return item.asciiValue ?? value
        }
        return value
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

    /// Whether tapping this cell can do anything. Panel rows carry their own
    /// command and never reach librime, so only real candidates are gated: a
    /// tap on one needs `RimeSelectCandidate`/`RimeSelectCandidateOnCurrentPage`
    /// or the common layer falls back to sending the schema's select key, which
    /// a schema without `menu/select_keys` types into the composition (D4).
    private func isSelectable(_ candidate: CandidateRect) -> Bool {
        if candidate.command != nil {
            return true
        }
        return candidate.global
            ? capabilities.globalCandidateSelection
            : capabilities.candidateSelection
    }

    private func panelCandidateGlobalIndex(_ localIndex: Int) -> Int {
        let pageSize = state.pageSize > 0 ? state.pageSize : max(state.candidatePanel.candidates.count, 1)
        return state.page * pageSize + localIndex
    }

    /// The highlight drawn on a candidate is librime's own
    /// `highlighted_candidate_index` — what Space is about to commit — so it
    /// stays truthful on a runtime without `RimeHighlightCandidateOnCurrentPage`
    /// and is deliberately not gated on `capabilities.candidateHighlight`.
    /// That bit buys moving the highlight from the frontend (hover, arrow keys);
    /// this keyboard offers no such gesture and draws no hover state, so there
    /// is nothing for it to switch off. Anything added here that would call
    /// `keytao_session_highlight_candidate` has to check it first, or the
    /// common layer degrades the move to a no-op the user cannot see.
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
        if usesFullHeightSymbolKeyboard() {
            return 0
        }
        return config.candidateBarHeightDp
    }

    private func keyboardBottom() -> CGFloat {
        bounds.height
    }

    private func usesFullHeightSymbolKeyboard() -> Bool {
        layerMode.id.isSymbolLayer && !candidatePanelExpanded && !functionPanelActive
    }

    private func usesCategorizedSymbolKeyboard(_ rows: [[KeyTaoKeySpec]]? = nil) -> Bool {
        usesFullHeightSymbolKeyboard() && (rows ?? activeRows()).count >= 3
    }

    private func usesScrollableSymbolKeyboard(_ rows: [[KeyTaoKeySpec]]? = nil) -> Bool {
        usesCategorizedSymbolKeyboard(rows) && (rows ?? activeRows()).count > 5
    }

    private func isVisibleKey(_ key: KeyRect, at point: CGPoint) -> Bool {
        key.sticky
            || !usesCategorizedSymbolKeyboard()
            || (point.y >= keyboardScrollViewportTop && point.y < keyboardScrollViewportBottom)
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

    private func keyLabelSize(for label: String) -> CGFloat {
        if layerMode.id.isSymbolLayer && !containsCJK(label) && label.count <= 2 {
            return max(theme.font.size, 22)
        }
        if label.count > 2 || containsCJK(label) {
            return max(12, min(theme.font.labelSize, theme.font.size - 4, 16))
        }
        return theme.font.size
    }

    private func keyHintSize() -> CGFloat {
        max(9, min(theme.font.commentSize - 2, keyLabelSize(for: "中") - 2, 12))
    }

    private func containsCJK(_ text: String) -> Bool {
        text.unicodeScalars.contains { scalar in
            (0x4E00...0x9FFF).contains(scalar.value) ||
                (0x3400...0x4DBF).contains(scalar.value) ||
                (0x20000...0x2A6DF).contains(scalar.value) ||
                (0xF900...0xFAFF).contains(scalar.value)
        }
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
        min(max(7, min(theme.candidate.cornerRadius + 1, 10)), rect.width * 0.28, rect.height * 0.28)
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
        if action.command.type == KeyTaoCommandType.mode ||
            action.command.type == KeyTaoCommandType.openPage ||
            action.command.type == KeyTaoCommandType.keyboardMode ||
            action.command.type == KeyTaoCommandType.keyboardPicker {
            return true
        }
        if action.command.type == KeyTaoCommandType.panel,
           ["rime", "clipboard", "close", "dismissClipboard"].contains(action.command.value ?? "") {
            return true
        }
        return false
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

    /// Standard key feedback: the system click sound is always requested (iOS
    /// honours the user's "Keyboard Clicks" setting), haptics only when the
    /// config asks for them and Full Access makes them work at all.
    private func performConfiguredHaptic(strong: Bool = false) {
        UIDevice.current.playInputClick()
        guard config.hapticsEnabled, hapticsAvailable else {
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
        1 / max(traitCollection.displayScale, 1)
    }

    private static let inputModeSwitchKeyWeight: CGFloat = 1.05
    private static let longPressDelayMs = 420
    private static let backspaceRepeatIntervalMs = 72
    private static let expandedCandidateLoadDelayMs = 180
    private static let rimeOptionSpecs = [
        RimeOptionSpec(name: "ascii_mode", label: "英文模式", onLabel: "英文", offLabel: "中文"),
        RimeOptionSpec(name: "ascii_punct", label: "标点", onLabel: "英文标点", offLabel: "中文标点"),
        RimeOptionSpec(name: "full_shape", label: "全角模式", onLabel: "全角", offLabel: "半角"),
        RimeOptionSpec(name: "simplification", label: "简体输出", onLabel: "简体", offLabel: "繁体"),
    ]
    private static let repeatableEditVerbs: Set<String> = [
        "cursorLeft",
        "cursorRight",
        "cursorUp",
        "cursorDown",
        "forwardDelete",
    ]
    private static let alwaysUnsupportedEditVerbs: Set<String> = [
        "selectAll",
        "toggleSelection",
        "selectLeft",
        "selectRight",
        "redo",
        "clearAll",
    ]
}

extension KeyTaoIOSKeyboardView: UIInputViewAudioFeedback {
    var enableInputClicksWhenVisible: Bool { true }
}

private extension Int {
    var clampedColor: Int {
        Swift.min(Swift.max(self, 0), 255)
    }
}

private extension String {
    var isSymbolLayer: Bool {
        self == "symbols" || hasPrefix("symbols_")
    }
}

private extension KeyTaoKeyCommand {
    var isTextInputCommand: Bool {
        type == KeyTaoCommandType.input ||
            type == KeyTaoCommandType.rimeInput ||
            type == KeyTaoCommandType.directInput
    }
}

private extension KeyTaoKeySpec {
    var isTextInputKey: Bool {
        action?.isTextInputCommand ?? true
    }

    var isCandidatePagingKey: Bool {
        guard let type = action?.type else {
            return false
        }
        return type == KeyTaoCommandType.nextCandidatePage
            || type == KeyTaoCommandType.previousCandidatePage
    }
}

private extension KeyTaoKeyStackItem {
    var isTextInputItem: Bool {
        action?.isTextInputCommand ?? true
    }
}
