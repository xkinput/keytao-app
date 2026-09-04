import UIKit

protocol KeyTaoIOSKeyboardViewDelegate: AnyObject {
    func keyboardView(_ view: KeyTaoIOSKeyboardView, didTrigger command: KeyTaoKeyCommand)
    func keyboardView(_ view: KeyTaoIOSKeyboardView, didSelectCandidate index: Int, global: Bool)
    func keyboardView(_ view: KeyTaoIOSKeyboardView, candidateIsUserPhrase index: Int) -> Bool
    func keyboardView(_ view: KeyTaoIOSKeyboardView, deleteCandidate index: Int) -> Bool
    func keyboardView(_ view: KeyTaoIOSKeyboardView, requestExpandedCandidates completion: @escaping ([KeyTaoCandidate]) -> Void)
    func keyboardView(_ view: KeyTaoIOSKeyboardView, requestClipboardHistory completion: @escaping ([String]) -> Void)
    func keyboardView(_ view: KeyTaoIOSKeyboardView, deleteClipboardEntry text: String)
    func keyboardViewClearClipboardHistory(_ view: KeyTaoIOSKeyboardView)
    func keyboardView(_ view: KeyTaoIOSKeyboardView, persistToolbarOrder order: [String], pinnedCount: Int)
    func keyboardViewCanUndo(_ view: KeyTaoIOSKeyboardView) -> Bool
    func keyboardViewNeedsFullAccessForHaptics(_ view: KeyTaoIOSKeyboardView)
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
    case empty
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

    private struct ActiveTouch {
        var key: KeyRect
        var keyIndex: Int
        let originKey: KeyRect
        let originKeyIndex: Int
        var touchStart: CGPoint
        var currentTouchPoint: CGPoint
        let touchStartUptime: TimeInterval
        var longPressConsumed = false
        var backspaceGestureUnits = 0
        var backspaceGestureConsumed = false
        var pressedStackIndex: Int?
        var alternatePanel: AlternatePanel?
        var longPressWorkItem: DispatchWorkItem?
        var repeatTimer: Timer?
        var cursorGesture: KeyTaoCursorGestureTracker?
    }

    private struct AlternateOption {
        var label: String
        var command: KeyTaoKeyCommand
    }

    private struct AlternatePanel {
        var options: [AlternateOption]
        var rect: CGRect
        var selectionRect: CGRect
        var selectionTracker: KeyTaoAlternateSelectionTracker
        var selectedIndex: Int?
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
        var pageIndex: Int = 0
        var label: String = ""
        var comment: String?
        var clipboardText: String? = nil
    }

    private struct CandidateMenuState {
        var pageIndex: Int
        var text: String
        var code: String
        var deletionUnavailable: Bool = false
    }

    private struct CandidateMenuActionRect {
        var action: String
        var rect: CGRect
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
        var statusLabel: String? = nil
    }

    private struct CompletionSuggestion: Equatable {
        var word: String
        var insertion: String
    }

    private struct ToolbarAction {
        var label: String
        var command: KeyTaoKeyCommand
        var selected: Bool = false
        var secondaryLabel: String?
        var icon: KeyTaoToolbarIcon?
        var longPressCommand: KeyTaoKeyCommand?
        var id: String?
        var customizable: Bool { id != nil && id != Self.pinnedBoundaryID }

        private static let pinnedBoundaryID = "__toolbar_pinned_boundary__"
    }

    private struct ToolbarRect {
        var action: ToolbarAction
        var rect: CGRect
        var drawingRect: CGRect? = nil
    }

    private struct KeyPressAnimationState {
        var progress: CGFloat
        var target: CGFloat
    }

    private enum VerticalScrollSurface {
        case expandedPanel
        case symbolKeyboard
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
    private var hapticsAccessMessageShown = false
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
    private var completionSuggestions: [CompletionSuggestion] = []
    private var expandedCandidateItemsCacheSignature = ""
    private var expandedCandidateItemsCache: [CandidateDrawItem] = []
    private var expandedCandidateScrollY: CGFloat = 0
    private var expandedCandidateOverscrollY: CGFloat = 0
    private var expandedCandidateContentHeight: CGFloat = 0
    private var keyboardScrollY: CGFloat = 0
    private var keyboardOverscrollY: CGFloat = 0
    private var keyboardTouchStartY: CGFloat = 0
    private var keyboardTouchStartScrollY: CGFloat = 0
    private var keyboardDragging = false
    private var keyboardScrollTouchActive = false
    private var keyboardScrollContentHeight: CGFloat = 0
    private var keyboardScrollViewportHeight: CGFloat = 0
    private var keyboardScrollViewportTop: CGFloat = 0
    private var keyboardScrollViewportBottom: CGFloat = 0
    private var expandRequestToken = 0

    private let keyHitLayout = KeyTaoImmediateHitLayout<KeyRect>()
    private var keyRects: [KeyRect] {
        get { keyHitLayout.items }
        set { keyHitLayout.rebuild(newValue) }
    }
    private var inlineCandidateRects: [CandidateRect] = []
    private var expandedCandidateRects: [CandidateRect] = []
    private var expandedSectionRects: [Int: CGRect] = [:]
    private var clipboardDeleteRects: [ClipboardDeleteRect] = []
    private var candidateRects: [CandidateRect] {
        inlineCandidateRects + expandedCandidateRects
    }
    private var toolbarRects: [ToolbarRect] = []
    private var candidateExpandRect: CGRect?
    private var candidateScrollX: CGFloat = 0
    private var candidateContentWidth: CGFloat = 0
    private var candidateViewportWidth: CGFloat = 0
    private var candidateTouchStartScrollX: CGFloat = 0
    private var toolbarScrollX: CGFloat = 0
    private var toolbarContentWidth: CGFloat = 0
    private var toolbarViewportWidth: CGFloat = 0
    private var toolbarTouchStartScrollX: CGFloat = 0
    private var toolbarTouchActive = false
    private var toolbarDragging = false
    private var toolbarLongPressWorkItem: DispatchWorkItem?
    private var toolbarLongPressConsumed = false
    private var toolbarMoreExpanded = false
    private var toolbarEditMode = false
    private var toolbarActionOrderOverride: [String]?
    private var toolbarPinnedCountOverride: Int?
    private var toolbarDragActionID: String?
    private var toolbarInactiveActionIDs: [String] = []
    private var inlineCandidateTouchActive = false
    private var candidateDragging = false
    private var candidatePagingConsumed = false
    private var candidateMenu: CandidateMenuState?
    private var candidateMenuActionRects: [CandidateMenuActionRect] = []
    private var pressedCandidateMenuAction: CandidateMenuActionRect?
    private var candidateLongPressWorkItem: DispatchWorkItem?
    private var candidateLongPressConsumed = false
    private var candidatePanelExpanded = false
    private var keyPressAnimations: [Int: KeyPressAnimationState] = [:]
    private var keyPressDisplayLink: CADisplayLink?
    private var keyPressLastTimestamp: CFTimeInterval = 0
    private var verticalScrollSurface: VerticalScrollSurface?
    private var verticalScrollVelocityY: CGFloat = 0
    private var verticalScrollDisplayLink: CADisplayLink?
    private var verticalScrollLastTimestamp: CFTimeInterval = 0
    private var scrollGestureLastY: CGFloat = 0
    private var scrollGestureLastTimestamp: CFTimeInterval = 0
    private var scrollGestureVelocityY: CGFloat = 0
    private var scrollIndicatorSurface: VerticalScrollSurface?
    private var scrollIndicatorAlpha: CGFloat = 0
    private var scrollIndicatorHoldRemaining: TimeInterval = 0
    private var activeTouches = KeyTaoTouchRolloverStateMachine<ActiveTouch>()
    private let touchBounceTracker = KeyTaoPerPointerBounceTracker<Int>()
    private var pointerSlotsByTouchIdentifier: [ObjectIdentifier: Int] = [:]
    private var candidateGestureTouchIdentifier: ObjectIdentifier?
    private var keyboardScrollTouchIdentifier: ObjectIdentifier?
    private var expandedScrollWasAnimatingAtDown = false
    private var keyboardScrollWasAnimatingAtDown = false
    private var pressedToolbar: ToolbarRect?
    private var pressedCandidate: CandidateRect?
    private var pressedClipboardDelete: ClipboardDeleteRect?
    private var expandedTouchActive = false
    private var expandedDragging = false
    private var candidateExpandPressed = false
    private var gestureTouchStart: CGPoint = .zero
    private var backspacePreviewText: String?
    private var backspacePreviewRect: CGRect?
    private var backspacePreviewPressed = false
    private var backspacePreviewPendingSelection = false
    private var backspacePreviewHideWorkItem: DispatchWorkItem?
    private let emojiPreferences = UserDefaults(suiteName: KeyTaoIOSPaths.appGroupIdentifier) ?? .standard
    private lazy var recentEmojis: [String] = loadRecentEmojis()
    private var touchStartScrollY: CGFloat = 0
    private var pendingExpandedCandidateWorkItem: DispatchWorkItem?
    private let lightHapticGenerator = UIImpactFeedbackGenerator(style: .light)
    private let mediumHapticGenerator = UIImpactFeedbackGenerator(style: .medium)
    private let selectionHapticGenerator = UISelectionFeedbackGenerator()
    private let notificationHapticGenerator = UINotificationFeedbackGenerator()
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
        config.effectiveKeyboardHeightDp + config.candidateBarHeightDp
    }

    func update(config: KeyTaoIOSImeConfig) {
        if self.config.keyboardHeightScale != config.keyboardHeightScale {
            clearActiveTouches()
            keyRects = []
        }
        self.config = config
        if !config.predictionEnabled {
            completionSuggestions = []
        }
        if !toolbarEditMode {
            toolbarActionOrderOverride = config.toolbarActionOrder.isEmpty ? nil : config.toolbarActionOrder
            toolbarPinnedCountOverride = config.toolbarPinnedCount
        }
        if !config.hapticsEnabled {
            hapticsAccessMessageShown = false
        } else {
            showHapticsAccessMessageIfNeeded()
        }
        resetCandidateScroll()
        toolbarScrollX = 0
        resetExpandedCandidateScroll()
        resetKeyboardScroll()
        invalidateLayoutAndDisplay()
    }

    func currentConfig() -> KeyTaoIOSImeConfig {
        config
    }

    func updatePredictionSuggestions(prefix: String?, suggestions: [String]) {
        let normalizedPrefix = prefix ?? ""
        let next: [CompletionSuggestion]
        if config.predictionEnabled, !normalizedPrefix.isEmpty {
            next = suggestions.reduce(into: []) { result, word in
                guard word.count > normalizedPrefix.count,
                      word.lowercased().hasPrefix(normalizedPrefix.lowercased()),
                      !result.contains(where: { $0.word == word }) else {
                    return
                }
                result.append(
                    CompletionSuggestion(
                        word: word,
                        insertion: String(word.dropFirst(normalizedPrefix.count))
                    )
                )
            }
        } else {
            next = []
        }
        guard next != completionSuggestions else { return }
        completionSuggestions = next
        resetCandidateScroll()
        invalidateLayoutAndDisplay()
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
            candidateMenu = nil
            candidateMenuActionRects = []
            candidateLongPressWorkItem?.cancel()
            candidateLongPressWorkItem = nil
            cancelExpandedCandidateRequest()
            expandedCandidates = []
            resetCandidateScroll()
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

    func showBackspaceDeletionPreview(_ text: String, pendingSelection: Bool = false) {
        backspacePreviewHideWorkItem?.cancel()
        backspacePreviewHideWorkItem = nil
        backspacePreviewText = text.isEmpty ? nil : text
        backspacePreviewPendingSelection = pendingSelection && !text.isEmpty
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
        if hapticsAvailable {
            hapticsAccessMessageShown = false
        } else {
            showHapticsAccessMessageIfNeeded()
        }
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
        clearActiveTouches()
        pressedToolbar = nil
        resetKeyboardScroll()
        invalidateLayoutAndDisplay()
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        rebuildInteractiveRects()
        rebuildAccessibilityElements()
    }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        if window == nil {
            clearActiveTouches()
            verticalScrollDisplayLink?.invalidate()
            verticalScrollDisplayLink = nil
            verticalScrollSurface = nil
            keyPressDisplayLink?.invalidate()
            keyPressDisplayLink = nil
            keyPressAnimations.removeAll()
            keyPressLastTimestamp = 0
        }
    }

    override func draw(_ rect: CGRect) {
        drawBackground()
        drawCandidateBar()
        drawBackspaceDeletionPreview()
        if candidatePanelExpanded {
            drawExpandedCandidatePanel()
        } else {
            drawKeyboard()
            drawKeyFeedbackOverlays()
        }
        drawLayoutInteractionHints()
    }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        for touch in touches {
            beginTouch(touch)
        }
        setNeedsDisplay()
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) {
        for touch in touches {
            let identifier = ObjectIdentifier(touch)
            let point = touch.location(in: self)
            if identifier == candidateGestureTouchIdentifier {
                updateCandidateOrPanelTouchMove(at: point)
                continue
            }
            if identifier == keyboardScrollTouchIdentifier {
                updateKeyboardScrollTouchMove(identifier: identifier, at: point)
                if keyboardDragging {
                    continue
                }
            }
            updateKeyTouchMove(identifier: identifier, point: point)
        }
    }

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
        for touch in touches {
            let identifier = ObjectIdentifier(touch)
            let point = touch.location(in: self)
            let pointerSlot = pointerSlotsByTouchIdentifier.removeValue(forKey: identifier)
            if pointerSlot.map({ touchBounceTracker.recordUp(
                pointerID: $0,
                eventTimeMs: touch.timestamp * 1_000,
                x: point.x,
                y: point.y
            ) }) == true {
                continue
            }
            if identifier == candidateGestureTouchIdentifier {
                finishCandidateOrPanelTouch(at: point)
                continue
            }
            if identifier == keyboardScrollTouchIdentifier {
                let wasDragging = keyboardDragging
                let wasBrakingScroll = keyboardScrollWasAnimatingAtDown
                if wasDragging {
                    startVerticalScrollAnimation(surface: .symbolKeyboard)
                }
                clearKeyboardScrollTouchState()
                if wasDragging {
                    cancelActiveTouch(identifier)
                    continue
                }
                if wasBrakingScroll {
                    cancelActiveTouch(identifier)
                    continue
                }
                finishKeyTouch(identifier: identifier, point: point)
                continue
            }
            finishKeyTouch(identifier: identifier, point: point)
        }
        setNeedsDisplay()
    }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) {
        for touch in touches {
            let identifier = ObjectIdentifier(touch)
            if let pointerSlot = pointerSlotsByTouchIdentifier.removeValue(forKey: identifier) {
                touchBounceTracker.cancel(pointerID: pointerSlot)
            }
            if identifier == candidateGestureTouchIdentifier {
                settleVerticalScrollAfterCancellation()
                clearCandidateOrPanelTouchState()
                continue
            }
            cancelActiveTouch(identifier)
            if identifier == keyboardScrollTouchIdentifier {
                settleVerticalScrollAfterCancellation()
                clearKeyboardScrollTouchState()
            }
        }
        setNeedsDisplay()
    }

    private func setup() {
        isOpaque = false
        backgroundColor = .clear
        isAccessibilityElement = false
        isMultipleTouchEnabled = true
        contentMode = .redraw
        lightHapticGenerator.prepare()
        mediumHapticGenerator.prepare()
        selectionHapticGenerator.prepare()
        notificationHapticGenerator.prepare()
    }

    private func invalidateLayoutAndDisplay() {
        backgroundColor = .clear
        rebuildInteractiveRects()
        rebuildAccessibilityElements()
        setNeedsDisplay()
        invalidateIntrinsicContentSize()
    }

    private func clearCandidateOrPanelTouchState() {
        candidateLongPressWorkItem?.cancel()
        candidateLongPressWorkItem = nil
        toolbarLongPressWorkItem?.cancel()
        toolbarLongPressWorkItem = nil
        pressedToolbar = nil
        pressedCandidate = nil
        pressedClipboardDelete = nil
        candidateExpandPressed = false
        expandedTouchActive = false
        expandedDragging = false
        inlineCandidateTouchActive = false
        candidateDragging = false
        candidatePagingConsumed = false
        candidateLongPressConsumed = false
        pressedCandidateMenuAction = nil
        backspacePreviewPressed = false
        toolbarTouchActive = false
        toolbarDragging = false
        toolbarLongPressConsumed = false
        toolbarDragActionID = nil
        expandedScrollWasAnimatingAtDown = false
        candidateGestureTouchIdentifier = nil
        setNeedsDisplay()
    }

    private func clearKeyboardScrollTouchState() {
        keyboardScrollTouchActive = false
        keyboardDragging = false
        keyboardScrollWasAnimatingAtDown = false
        keyboardScrollTouchIdentifier = nil
        setNeedsDisplay()
    }

    private func beginTouch(_ touch: UITouch) {
        let identifier = ObjectIdentifier(touch)
        let point = touch.location(in: self)
        let pointerSlot = allocatePointerSlot(for: identifier)
        if touchBounceTracker.isBounceDown(
            pointerID: pointerSlot,
            eventTimeMs: touch.timestamp * 1_000,
            x: point.x,
            y: point.y
        ) {
            return
        }
        let beginsCandidateOrPanelTouch = (!usesFullHeightSymbolKeyboard() && point.y < config.candidateBarHeightDp) ||
            (candidatePanelExpanded && point.y >= config.candidateBarHeightDp && point.y < keyboardBottom())
        let beginsKeyboardScrollTouch = usesCategorizedSymbolKeyboard() &&
            maxKeyboardScroll() > 0 &&
            point.y >= keyboardScrollViewportTop &&
            point.y < keyboardScrollViewportBottom
        if beginsCandidateOrPanelTouch {
            guard candidateGestureTouchIdentifier == nil else { return }
            if beginAuxiliaryTouch(at: point) {
                candidateGestureTouchIdentifier = identifier
            }
            return
        }
        if beginsKeyboardScrollTouch, keyboardDragging {
            return
        }
        if beginsKeyboardScrollTouch, keyboardScrollTouchIdentifier == nil {
            keyboardScrollTouchIdentifier = identifier
            beginKeyboardScrollTouch(identifier: identifier, at: point)
            return
        }
        guard let keyIndex = keyHitLayout.firstIndex(where: { isVisibleKey($0, at: point) && $0.rect.contains(point) }) else {
            return
        }
        let key = keyRects[keyIndex]
        guard !isUnsupportedEditKey(key.spec) else {
            performWarningFeedback()
            return
        }
        beginKeyTouch(identifier: identifier, key: key, keyIndex: keyIndex, point: point)
    }

    private func beginAuxiliaryTouch(at point: CGPoint) -> Bool {
        gestureTouchStart = point
        if !backspacePreviewPendingSelection, backspacePreviewRect?.contains(point) == true {
            backspacePreviewPressed = true
            return true
        }
        if candidateMenu != nil {
            pressedCandidateMenuAction = candidateMenuActionRects.first { $0.rect.contains(point) }
            return true
        }
        candidateTouchStartScrollX = candidateScrollX
        toolbarTouchStartScrollX = toolbarScrollX
        expandedDragging = false
        candidateDragging = false
        toolbarDragging = false
        candidatePagingConsumed = false
        inlineCandidateTouchActive = false
        let hasInlineCandidates = !state.candidatePanel.candidates.isEmpty || !completionSuggestions.isEmpty
        candidateExpandPressed = !functionPanelActive
            && !state.candidatePanel.candidates.isEmpty
            && point.y < config.candidateBarHeightDp
            && candidateExpandRect?.contains(point) == true
        pressedToolbar = point.y < config.candidateBarHeightDp ? toolbarRects.first { $0.rect.contains(point) } : nil
        toolbarTouchActive = !functionPanelActive
            && !hasInlineCandidates
            && !usesFullHeightSymbolKeyboard()
            && point.y < config.candidateBarHeightDp
        pressedCandidate = nil
        toolbarLongPressConsumed = false
        if pressedToolbar?.action.longPressCommand != nil {
            scheduleToolbarLongPress()
        }
        pressedClipboardDelete = nil
        expandedTouchActive = false
        if pressedToolbar == nil,
           !candidateExpandPressed,
           hasInlineCandidates,
           point.y < config.candidateBarHeightDp {
            inlineCandidateTouchActive = true
            pressedCandidate = inlineCandidateRects.first { $0.rect.contains(point) }
            if pressedCandidate != nil {
                scheduleCandidateLongPress()
            }
        } else if pressedToolbar == nil && !candidateExpandPressed && candidatePanelExpanded && point.y >= config.candidateBarHeightDp {
            expandedTouchActive = true
            expandedScrollWasAnimatingAtDown = beginVerticalScrollGesture(at: point.y) == .expandedPanel
            touchStartScrollY = expandedCandidateScrollY
            pressedClipboardDelete = clipboardDeleteRects.first { $0.rect.contains(point) }
            if pressedClipboardDelete == nil {
                pressedCandidate = expandedCandidateRects.first { $0.rect.contains(point) }
            }
        }
        return candidateExpandPressed ||
            pressedToolbar != nil ||
            pressedCandidate != nil ||
            pressedClipboardDelete != nil ||
            expandedTouchActive ||
            inlineCandidateTouchActive ||
            toolbarTouchActive
    }

    private func beginKeyboardScrollTouch(identifier: ObjectIdentifier, at point: CGPoint) {
        keyboardScrollWasAnimatingAtDown = beginVerticalScrollGesture(at: point.y) == .symbolKeyboard
        keyboardTouchStartY = point.y
        keyboardTouchStartScrollY = keyboardScrollY
        keyboardDragging = false
        keyboardScrollTouchActive = true
        if let keyIndex = keyHitLayout.firstIndex(where: { isVisibleKey($0, at: point) && $0.rect.contains(point) }) {
            let key = keyRects[keyIndex]
            if !isUnsupportedEditKey(key.spec) {
                beginKeyTouch(identifier: identifier, key: key, keyIndex: keyIndex, point: point)
            }
        }
    }

    private func updateCandidateOrPanelTouchMove(at point: CGPoint) {
        if backspacePreviewPressed {
            backspacePreviewPressed = backspacePreviewRect?.contains(point) == true
            setNeedsDisplay()
            return
        }
        if candidateMenu != nil {
            if let pressed = pressedCandidateMenuAction, !pressed.rect.contains(point) {
                pressedCandidateMenuAction = nil
                setNeedsDisplay()
            }
            return
        }
        if toolbarTouchActive {
            let toolbar = pressedToolbar
            let dragActionID = toolbarDragActionID ?? toolbar?.action.id
            let deltaX = point.x - gestureTouchStart.x
            let deltaY = point.y - gestureTouchStart.y
            let dragSlop = KeyTaoIMEInteractionTuning.candidateDragSlop
            if !toolbarDragging, abs(deltaX) > dragSlop, abs(deltaX) > abs(deltaY) {
                toolbarDragging = true
                toolbarLongPressWorkItem?.cancel()
                toolbarLongPressWorkItem = nil
                if !toolbarEditMode || toolbar?.action.customizable != true {
                    pressedToolbar = nil
                }
            }
            if toolbarDragging {
                if toolbarEditMode, let dragActionID, toolbar?.action.customizable == true {
                    toolbarDragActionID = dragActionID
                    reorderToolbarAction(dragActionID, at: point.x)
                } else {
                    toolbarScrollX = max(0, min(maxToolbarScroll(), toolbarTouchStartScrollX - deltaX))
                }
            } else if let toolbar = pressedToolbar, !toolbar.rect.contains(point) {
                toolbarLongPressWorkItem?.cancel()
                toolbarLongPressWorkItem = nil
                pressedToolbar = nil
            }
            invalidateLayoutAndDisplay()
            return
        }
        if inlineCandidateTouchActive {
            let deltaX = point.x - gestureTouchStart.x
            let deltaY = point.y - gestureTouchStart.y
            let dragSlop = KeyTaoIMEInteractionTuning.candidateDragSlop
            if !candidateDragging, abs(deltaX) > dragSlop || abs(deltaY) > dragSlop {
                candidateDragging = true
                pressedCandidate = nil
                candidateLongPressWorkItem?.cancel()
                candidateLongPressWorkItem = nil
            }
            if candidateDragging {
                let maximum = maxCandidateScroll()
                let requested = candidateTouchStartScrollX - deltaX
                candidateScrollX = max(0, min(maximum, requested))
                if !candidatePagingConsumed {
                    let navigation = state.candidatePanel.navigation
                    let command: KeyTaoKeyCommand?
                    if requested > maximum + dragSlop, navigation.canGoNext {
                        command = KeyTaoKeyCommand(type: KeyTaoCommandType.nextCandidatePage, value: nil, fallbackValue: nil)
                    } else if requested < -dragSlop, navigation.canGoPrevious {
                        command = KeyTaoKeyCommand(type: KeyTaoCommandType.previousCandidatePage, value: nil, fallbackValue: nil)
                    } else {
                        command = nil
                    }
                    if let command {
                        candidatePagingConsumed = true
                        candidateScrollX = 0
                        candidateTouchStartScrollX = 0
                        gestureTouchStart.x = point.x
                        performSelectionFeedback(playSound: false)
                        delegate?.keyboardView(self, didTrigger: command)
                    }
                }
                invalidateLayoutAndDisplay()
            }
            return
        }
        if expandedTouchActive {
            let deltaY = point.y - gestureTouchStart.y
            if !expandedDragging && abs(deltaY) > 6 {
                expandedDragging = true
                pressedCandidate = nil
                pressedClipboardDelete = nil
            }
            if expandedDragging {
                updateVerticalScrollGesture(at: point.y)
                setExpandedCandidateScroll(touchStartScrollY - deltaY, rubberBand: true)
                showScrollIndicator(.expandedPanel)
                refreshScrollLayoutAndDisplay()
            }
        }
    }

    private func updateKeyboardScrollTouchMove(identifier: ObjectIdentifier, at point: CGPoint) {
        let deltaY = point.y - keyboardTouchStartY
        if !keyboardDragging && abs(deltaY) > 6 {
            keyboardDragging = true
            cancelActiveTouch(identifier)
        }
        if keyboardDragging {
            updateVerticalScrollGesture(at: point.y)
            setKeyboardScroll(keyboardTouchStartScrollY - deltaY, rubberBand: true)
            showScrollIndicator(.symbolKeyboard)
            refreshScrollLayoutAndDisplay()
        }
    }

    private func finishCandidateOrPanelTouch(at point: CGPoint) {
        if backspacePreviewPressed || backspacePreviewRect?.contains(gestureTouchStart) == true {
            let activate = backspacePreviewPressed && backspacePreviewRect?.contains(point) == true
            clearCandidateOrPanelTouchState()
            if activate {
                delegate?.keyboardView(
                    self,
                    didTrigger: KeyTaoKeyCommand(
                        type: KeyTaoCommandType.backspaceGesture,
                        value: "restoreGesture",
                        fallbackValue: nil
                    )
                )
                hideBackspaceDeletionPreview()
            }
            return
        }
        if var menu = candidateMenu {
            let action = pressedCandidateMenuAction
            pressedCandidateMenuAction = nil
            if let action, action.rect.contains(point) {
                switch action.action {
                case "delete":
                    if delegate?.keyboardView(self, deleteCandidate: menu.pageIndex) == true {
                        candidateMenu = nil
                        candidateMenuActionRects = []
                    } else {
                        menu.deletionUnavailable = true
                        candidateMenu = menu
                    }
                    performSelectionFeedback(playSound: false)
                case "close":
                    candidateMenu = nil
                default:
                    break
                }
            }
            candidateGestureTouchIdentifier = nil
            invalidateLayoutAndDisplay()
            return
        }
        if toolbarTouchActive {
            let toolbar = pressedToolbar
            let wasDragging = toolbarDragging
            let longPressConsumed = toolbarLongPressConsumed
            clearCandidateOrPanelTouchState()
            if wasDragging, toolbarEditMode {
                persistToolbarCustomization()
            } else if !longPressConsumed, let toolbar, toolbar.rect.contains(point) {
                handleToolbarCommand(toolbar.action.command)
            }
            return
        }
        if candidateExpandPressed,
           let expand = candidateExpandRect,
           expand.contains(point),
           expand.contains(gestureTouchStart) {
            clearCandidateOrPanelTouchState()
            toggleCandidatePanel()
            performSelectionFeedback()
            invalidateLayoutAndDisplay()
            return
        }
        if let toolbar = pressedToolbar, toolbar.rect.contains(point) {
            clearCandidateOrPanelTouchState()
            handleToolbarCommand(toolbar.action.command)
            return
        }
        let wasBrakingScroll = expandedScrollWasAnimatingAtDown
        if let clipboardDelete = pressedClipboardDelete,
           !expandedDragging,
           !wasBrakingScroll,
           clipboardDelete.rect.contains(point) {
            clearCandidateOrPanelTouchState()
            deleteClipboardEntry(clipboardDelete.text)
            return
        }
        if let candidate = pressedCandidate, !candidateLongPressConsumed, !expandedDragging,
           !candidateDragging, !wasBrakingScroll, candidate.rect.contains(point) {
            clearCandidateOrPanelTouchState()
            if let command = candidate.command {
                handlePanelCommand(command)
                if candidate.clipboardText != nil {
                    closeCandidatePanel()
                    invalidateLayoutAndDisplay()
                }
            } else if !isSelectable(candidate) {
                performWarningFeedback()
                delegate?.keyboardView(self, didSelectCandidate: candidate.selectIndex, global: candidate.global)
            } else {
                closeCandidatePanelIfNeeded(afterCandidateSelection: candidate.global)
                performSelectionFeedback()
                delegate?.keyboardView(self, didSelectCandidate: candidate.selectIndex, global: candidate.global)
            }
            return
        }
        if expandedDragging {
            startVerticalScrollAnimation(surface: .expandedPanel)
        }
        clearCandidateOrPanelTouchState()
    }

    private func scheduleCandidateLongPress() {
        candidateLongPressWorkItem?.cancel()
        let workItem = DispatchWorkItem { [weak self] in
            guard let self,
                  self.inlineCandidateTouchActive,
                  !self.candidateDragging,
                  let candidate = self.pressedCandidate,
                  candidate.command == nil else {
                return
            }
            self.candidateLongPressConsumed = true
            self.candidateMenu = CandidateMenuState(
                pageIndex: candidate.pageIndex,
                text: candidate.label,
                code: candidate.comment?.isEmpty == false
                    ? candidate.comment!
                    : ((self.state.candidatePanel.preedit ?? self.state.preedit).isEmpty
                        ? "暂无编码"
                        : (self.state.candidatePanel.preedit ?? self.state.preedit)),
                deletionUnavailable: self.delegate?.keyboardView(
                    self,
                    candidateIsUserPhrase: candidate.pageIndex
                ) != true
            )
            self.performMediumFeedback(playSound: false)
            self.invalidateLayoutAndDisplay()
        }
        candidateLongPressWorkItem = workItem
        DispatchQueue.main.asyncAfter(
            deadline: .now() + Double(config.longPressDelayMs) / 1_000,
            execute: workItem
        )
    }

    private func scheduleToolbarLongPress() {
        toolbarLongPressWorkItem?.cancel()
        let workItem = DispatchWorkItem { [weak self] in
            guard let self,
                  self.toolbarTouchActive,
                  !self.toolbarDragging,
                  let command = self.pressedToolbar?.action.longPressCommand else {
                return
            }
            self.toolbarLongPressConsumed = true
            self.performMediumFeedback(playSound: false)
            self.handleToolbarCommand(command)
            self.setNeedsDisplay()
        }
        toolbarLongPressWorkItem = workItem
        DispatchQueue.main.asyncAfter(
            deadline: .now() + Double(config.longPressDelayMs) / 1_000,
            execute: workItem
        )
    }

    private func beginKeyTouch(
        identifier: ObjectIdentifier,
        key: KeyRect,
        keyIndex: Int,
        point: CGPoint
    ) {
        let pressedStackIndex = key.spec.stack?.isEmpty == false && key.rect.contains(point)
            ? stackIndex(in: key.rect, count: key.spec.stack?.count ?? 0, y: point.y)
            : nil
        let isBackspace = isBackspaceKey(key.spec)
        let selectionBackspace = isBackspace && usesSelectionBackspaceGesture()
        activeTouches.begin(
            ActiveTouch(
                key: key,
                keyIndex: keyIndex,
                originKey: key,
                originKeyIndex: keyIndex,
                touchStart: point,
                currentTouchPoint: point,
                touchStartUptime: ProcessInfo.processInfo.systemUptime,
                longPressConsumed: isBackspace && !selectionBackspace,
                backspaceGestureUnits: isBackspace && !selectionBackspace ? 1 : 0,
                pressedStackIndex: pressedStackIndex,
                cursorGesture: isSpaceKey(key.spec) && !hasActiveComposition
                    ? KeyTaoCursorGestureTracker(startX: point.x)
                    : nil
            ),
            for: identifier
        )
        animateKeyPress(keyIndex: keyIndex, pressed: true)
        performPressFeedback()
        if isBackspace {
            if !selectionBackspace {
                delegate?.keyboardView(self, didTrigger: backspaceGestureCommand("begin"))
                delegate?.keyboardView(self, didTrigger: actionForMode(key.spec))
            }
            scheduleBackspaceRepeat(for: identifier)
        } else {
            scheduleLongPressIfNeeded(for: identifier)
        }
    }

    private func updateKeyTouchMove(identifier: ObjectIdentifier, point: CGPoint) {
        guard let currentTouch = activeTouches[identifier] else {
            return
        }
        activeTouches.move(identifier) { touch in
            touch.currentTouchPoint = point
            touch.pressedStackIndex = touch.key.spec.stack?.isEmpty == false && touch.key.rect.contains(point)
                ? stackIndex(in: touch.key.rect, count: touch.key.spec.stack?.count ?? 0, y: point.y)
                : nil
        }
        if activeTouches[identifier]?.alternatePanel != nil {
            updateAlternateSelection(identifier: identifier, at: point)
            setNeedsDisplay()
            return
        }
        if handleBackspaceDrag(identifier: identifier, at: point) {
            return
        }
        if handleSpaceCursorDrag(identifier: identifier, at: point) {
            return
        }
        let deltaY = point.y - currentTouch.touchStart.y
        if abs(deltaY) >= config.swipeThresholdDp {
            if currentTouch.keyIndex != currentTouch.originKeyIndex {
                stopLongPressAndRepeat(for: identifier)
                activeTouches.move(identifier) { touch in
                    touch.key = touch.originKey
                    touch.keyIndex = touch.originKeyIndex
                }
            } else {
                stopLongPressAndRepeat(for: identifier)
            }
            setNeedsDisplay()
            return
        }
        if retargetKeyIfNeeded(identifier: identifier, at: point) {
            setNeedsDisplay()
            return
        }
        guard let updatedTouch = activeTouches[identifier] else {
            return
        }
        let holdRect = isBackspaceKey(updatedTouch.key.spec)
            ? updatedTouch.key.rect.insetBy(
                dx: -KeyTaoIMEInteractionTuning.backspaceHoldTolerance,
                dy: -KeyTaoIMEInteractionTuning.backspaceHoldTolerance
            )
            : updatedTouch.key.rect
        if !holdRect.contains(point) {
            stopLongPressAndRepeat(for: identifier)
        }
        setNeedsDisplay()
    }

    private func finishKeyTouch(identifier: ObjectIdentifier, point: CGPoint) {
        // Do not reintroduce release-time duration filtering; bounce suppression happens only on DOWN.
        guard activeTouches[identifier] != nil else { return }
        activeTouches.move(identifier) { $0.currentTouchPoint = point }
        if activeTouches[identifier]?.backspaceGestureConsumed == true {
            _ = handleBackspaceDrag(identifier: identifier, at: point)
        }
        stopLongPressAndRepeat(for: identifier)
        guard let finishedTouch = activeTouches.finish(identifier, resolving: { $0 }).first else {
            return
        }
        if !activeTouches.values.contains(where: { $0.keyIndex == finishedTouch.keyIndex }) {
            animateKeyPress(keyIndex: finishedTouch.keyIndex, pressed: false)
        }
        if var panel = finishedTouch.alternatePanel {
            panel.selectedIndex = alternateIndex(in: &panel, at: point)
            if let selectedIndex = panel.selectedIndex,
               panel.selectionRect.contains(point) {
                let command = panel.options[selectedIndex].command
                performSelectionFeedback(playSound: false)
                rememberRecentEmoji(command)
                delegate?.keyboardView(self, didTrigger: command)
                clearOneShotShift(after: command)
            }
            setNeedsDisplay()
            return
        }
        if finishedTouch.backspaceGestureConsumed {
            if let command = KeyTaoBackspaceGesturePolicy.releaseCommand(
                mode: backspaceGestureMode(),
                selectedUnits: finishedTouch.backspaceGestureUnits
            ) {
                delegate?.keyboardView(self, didTrigger: backspaceGestureCommand(command.action, count: command.count))
            }
            settleBackspaceDeletionPreview()
            setNeedsDisplay()
            return
        }
        if handleBackspaceRelease(for: finishedTouch, at: point) {
            settleBackspaceDeletionPreview()
            setNeedsDisplay()
            return
        }
        guard !finishedTouch.longPressConsumed else {
            setNeedsDisplay()
            return
        }
        guard shouldAcceptKeyRelease(finishedTouch, at: point) else {
            setNeedsDisplay()
            return
        }
        let composingSpace = isSpaceKey(finishedTouch.key.spec) && hasActiveComposition
        let command = resolveCommand(
            finishedTouch.key.spec,
            deltaY: composingSpace ? 0 : point.y - finishedTouch.touchStart.y,
            rect: finishedTouch.key.rect,
            releaseY: composingSpace ? finishedTouch.key.rect.midY : point.y
        )
        if isConfirmationCommand(command) {
            performMediumFeedback(playSound: false)
        }
        rememberRecentEmoji(command)
        delegate?.keyboardView(self, didTrigger: command)
        clearOneShotShift(after: command)
    }

    private func settleBackspaceDeletionPreview() {
        guard backspacePreviewText?.isEmpty == false else {
            return
        }
        backspacePreviewHideWorkItem?.cancel()
        let workItem = DispatchWorkItem { [weak self] in
            self?.hideBackspaceDeletionPreview()
        }
        backspacePreviewHideWorkItem = workItem
        DispatchQueue.main.asyncAfter(deadline: .now() + Self.backspacePreviewDuration, execute: workItem)
    }

    private func hideBackspaceDeletionPreview() {
        backspacePreviewHideWorkItem?.cancel()
        backspacePreviewHideWorkItem = nil
        backspacePreviewText = nil
        backspacePreviewRect = nil
        backspacePreviewPressed = false
        backspacePreviewPendingSelection = false
        setNeedsDisplay()
    }

    private func retargetKeyIfNeeded(identifier: ObjectIdentifier, at point: CGPoint) -> Bool {
        guard let touch = activeTouches[identifier],
              isCharacterKey(touch.originKey.spec),
              isCharacterKey(touch.key.spec) else {
            return false
        }
        let retainedRect = touch.key.rect.insetBy(
            dx: -KeyTaoIMEInteractionTuning.slideRetargetHysteresis,
            dy: -KeyTaoIMEInteractionTuning.slideRetargetHysteresis
        )
        guard !retainedRect.contains(point),
              let targetIndex = keyHitLayout.firstIndex(where: { isVisibleKey($0, at: point) && $0.rect.contains(point) }),
              targetIndex != touch.keyIndex,
              isCharacterKey(keyRects[targetIndex].spec) else {
            return false
        }
        let previousIndex = touch.keyIndex
        stopLongPressAndRepeat(for: identifier)
        activeTouches.move(identifier) { activeTouch in
            activeTouch.key = keyRects[targetIndex]
            activeTouch.keyIndex = targetIndex
            activeTouch.pressedStackIndex = nil
            activeTouch.alternatePanel = nil
        }
        if !activeTouches.values.contains(where: { $0.keyIndex == previousIndex && $0.keyIndex != targetIndex }) {
            animateKeyPress(keyIndex: previousIndex, pressed: false)
        }
        animateKeyPress(keyIndex: targetIndex, pressed: true)
        scheduleLongPressIfNeeded(for: identifier)
        return true
    }

    private func handleSpaceCursorDrag(identifier: ObjectIdentifier, at point: CGPoint) -> Bool {
        guard var touch = activeTouches[identifier], let tracker = touch.cursorGesture else {
            return false
        }
        if hasActiveComposition {
            touch.cursorGesture = nil
            activeTouches[identifier] = touch
            return false
        }
        let update = tracker.update(x: point.x)
        guard update.active else {
            return false
        }
        touch.longPressConsumed = true
        activeTouches[identifier] = touch
        stopLongPressAndRepeat(for: identifier)
        let command = KeyTaoKeyCommand.edit(update.stepDelta < 0 ? "cursorLeft" : "cursorRight")
        for _ in 0..<abs(update.stepDelta) {
            delegate?.keyboardView(self, didTrigger: command)
        }
        setNeedsDisplay()
        return true
    }

    private func shouldAcceptKeyRelease(_ touch: ActiveTouch, at point: CGPoint) -> Bool {
        if isSpaceKey(touch.key.spec), hasActiveComposition {
            return true
        }
        if touch.key.rect.contains(point) {
            return true
        }
        let deltaY = point.y - touch.touchStart.y
        guard abs(deltaY) >= config.swipeThresholdDp else {
            return false
        }
        let horizontalLimit = max(CGFloat(16), touch.key.rect.width * 0.65)
        return abs(point.x - touch.touchStart.x) <= horizontalLimit
    }

    private func isCharacterKey(_ key: KeyTaoKeySpec) -> Bool {
        guard key.stack?.isEmpty != false else {
            return false
        }
        return [
            KeyTaoCommandType.input,
            KeyTaoCommandType.directInput,
            KeyTaoCommandType.rimeInput,
        ].contains(actionForMode(key).type)
    }

    private func explicitAlternates(_ key: KeyTaoKeySpec) -> [AlternateOption] {
        let source: [KeyTaoKeyAlternate]
        if state.asciiMode, let asciiAlternates = key.asciiAlternates, !asciiAlternates.isEmpty {
            source = asciiAlternates
        } else {
            source = key.alternates ?? []
        }
        if source.isEmpty {
            let hasLegacyLongPress = key.longPress != nil ||
                (state.asciiMode && key.asciiLongPress != nil) ||
                key.hint?.isEmpty == false
            guard hasLegacyLongPress else {
                return []
            }
            let command = resolveLongPressCommand(key)
            return [
                AlternateOption(
                    label: command.value?.isEmpty == false
                        ? command.value!
                        : (key.hint ?? displayLabel(key)),
                    command: command
                ),
            ]
        }
        return source.map { alternate in
            let command: KeyTaoKeyCommand
            if let action = alternate.action {
                command = action
            } else if !state.asciiMode, let rimeValue = alternate.rimeValue {
                command = KeyTaoKeyCommand(
                    type: KeyTaoCommandType.rimeInput,
                    value: rimeValue,
                    fallbackValue: alternate.value ?? alternate.label
                )
            } else if layerMode.id.isSymbolLayer {
                command = .directInput(alternate.value ?? alternate.rimeValue ?? alternate.label)
            } else {
                command = .input(alternate.value ?? alternate.rimeValue ?? alternate.label)
            }
            return AlternateOption(label: alternate.label, command: applyShift(command))
        }
    }

    private func createAlternatePanel(
        for key: KeyRect,
        options: [AlternateOption],
        currentX: CGFloat
    ) -> AlternatePanel {
        let margin = Self.alternatePanelMargin
        let availableWidth = max(CGFloat(1), bounds.width - margin * 2)
        let desiredItemWidth = max(Self.alternatePanelMinimumItemWidth, key.rect.width)
        let panelWidth = min(availableWidth, desiredItemWidth * CGFloat(options.count))
        let itemWidth = panelWidth / CGFloat(options.count)
        let left = max(margin, min(bounds.width - margin - panelWidth, key.rect.midX - itemWidth / 2))
        let panelHeight = max(
            Self.alternatePanelMinimumHeight,
            min(Self.alternatePanelMaximumHeight, key.rect.height)
        )
        let top = max(margin, key.rect.minY - panelHeight - Self.alternatePanelGap)
        let rect = CGRect(x: left, y: top, width: panelWidth, height: panelHeight)
        let selectionRect = CGRect(
            x: rect.minX,
            y: rect.minY,
            width: rect.width,
            height: max(rect.height, key.rect.maxY - rect.minY)
        )
        return AlternatePanel(
            options: options,
            rect: rect,
            selectionRect: selectionRect,
            selectionTracker: KeyTaoAlternateSelectionTracker(
                startX: currentX,
                movementThreshold: KeyTaoIMEInteractionTuning.candidateDragSlop
            ),
            selectedIndex: 0
        )
    }

    private func updateAlternateSelection(identifier: ObjectIdentifier, at point: CGPoint) {
        activeTouches.move(identifier) { touch in
            guard var panel = touch.alternatePanel else {
                return
            }
            panel.selectedIndex = alternateIndex(in: &panel, at: point)
            touch.alternatePanel = panel
        }
    }

    private func alternateIndex(in panel: inout AlternatePanel, at point: CGPoint) -> Int? {
        let itemWidth = panel.rect.width / CGFloat(panel.options.count)
        return panel.selectionTracker.selectedIndex(
            x: point.x,
            insideSelection: panel.selectionRect.contains(point),
            panelLeft: panel.rect.minX,
            itemWidth: itemWidth,
            itemCount: panel.options.count
        )
    }

    private func cancelActiveTouch(_ identifier: ObjectIdentifier) {
        stopLongPressAndRepeat(for: identifier)
        guard let touch = activeTouches.cancel(identifier) else { return }
        if touch.backspaceGestureConsumed, usesSelectionBackspaceGesture() {
            delegate?.keyboardView(self, didTrigger: backspaceGestureCommand("cancelSelection"))
        }
        if !activeTouches.values.contains(where: { $0.keyIndex == touch.keyIndex }) {
            animateKeyPress(keyIndex: touch.keyIndex, pressed: false)
        }
    }

    private func clearActiveTouches() {
        let removedTouches = activeTouches.removeAll()
        if usesSelectionBackspaceGesture(), removedTouches.contains(where: { $0.backspaceGestureConsumed }) {
            delegate?.keyboardView(self, didTrigger: backspaceGestureCommand("cancelSelection"))
        }
        for touch in removedTouches {
            touch.longPressWorkItem?.cancel()
            touch.repeatTimer?.invalidate()
        }
        for keyIndex in Set(removedTouches.map(\.keyIndex)) {
            animateKeyPress(keyIndex: keyIndex, pressed: false)
        }
        touchBounceTracker.reset()
        pointerSlotsByTouchIdentifier.removeAll()
        candidateGestureTouchIdentifier = nil
        keyboardScrollTouchIdentifier = nil
        expandedScrollWasAnimatingAtDown = false
        keyboardScrollWasAnimatingAtDown = false
        keyboardScrollTouchActive = false
        keyboardDragging = false
    }

    private func allocatePointerSlot(for identifier: ObjectIdentifier) -> Int {
        if let existing = pointerSlotsByTouchIdentifier[identifier] {
            return existing
        }
        let usedSlots = Set(pointerSlotsByTouchIdentifier.values)
        var pointerSlot = 0
        while usedSlots.contains(pointerSlot) {
            pointerSlot += 1
        }
        pointerSlotsByTouchIdentifier[identifier] = pointerSlot
        return pointerSlot
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
        candidateMenuActionRects = candidateMenuLayout()
        backspacePreviewRect = backspacePreviewLayout()
    }

    private func candidateMenuLayout() -> [CandidateMenuActionRect] {
        guard let menu = candidateMenu else { return [] }
        let gap = max(4, theme.panel.gap)
        let leftPadding = theme.panel.gap * 1.5
        let top: CGFloat = 7
        let bottom = config.candidateBarHeightDp - 7
        let codeRight = menu.deletionUnavailable ? bounds.width - leftPadding : bounds.width * 0.62
        let codeRect = CGRect(
            x: leftPadding,
            y: top,
            width: max(0, codeRight - gap / 2 - leftPadding),
            height: max(0, bottom - top)
        )
        var actions = [CandidateMenuActionRect(action: "close", rect: codeRect)]
        if !menu.deletionUnavailable {
            actions.append(
                CandidateMenuActionRect(
                    action: "delete",
                    rect: CGRect(
                        x: codeRight + gap / 2,
                        y: top,
                        width: max(0, bounds.width - leftPadding - codeRight - gap / 2),
                        height: max(0, bottom - top)
                    )
                )
            )
        }
        return actions
    }

    private func backspacePreviewLayout() -> CGRect? {
        guard backspacePreviewText?.isEmpty == false else { return nil }
        let gap = max(4, theme.panel.gap)
        return CGRect(
            x: gap,
            y: 6,
            width: max(0, bounds.width - gap * 2),
            height: max(0, config.candidateBarHeightDp - 12)
        )
    }

    private func rebuildAccessibilityElements() {
        var elements: [UIAccessibilityElement] = []
        for key in keyRects {
            let element = KeyTaoActivatingAccessibilityElement(accessibilityContainer: self)
            element.accessibilityFrameInContainerSpace = key.rect
            element.accessibilityTraits = isUnsupportedEditKey(key.spec)
                ? [.button, .keyboardKey, .notEnabled]
                : [.button, .keyboardKey]
            element.accessibilityIdentifier = keyAccessibilityIdentifier(key.spec)
            element.accessibilityLabel = displayLabel(key.spec)
            element.activation = { [weak self] in
                guard let self, !self.isUnsupportedEditKey(key.spec) else {
                    return false
                }
                self.activateAccessibilityKey(key)
                return true
            }
            elements.append(element)
        }
        for candidate in candidateRects {
            let element = KeyTaoActivatingAccessibilityElement(accessibilityContainer: self)
            element.accessibilityFrameInContainerSpace = candidate.rect
            // A candidate the runtime cannot select is still worth reading out,
            // but announcing it as a button would promise an action that the
            // controller is about to refuse.
            element.accessibilityTraits = isSelectable(candidate) ? .button : .staticText
            element.accessibilityIdentifier = "keytao-candidate-\(candidate.identifierIndex)"
            element.accessibilityLabel = [candidate.label, candidate.comment]
                .compactMap { $0 }
                .filter { !$0.isEmpty }
                .joined(separator: "，")
            element.activation = { [weak self] in
                guard let self, self.isSelectable(candidate) else {
                    return false
                }
                if let command = candidate.command {
                    self.handlePanelCommand(command)
                    if candidate.clipboardText != nil {
                        self.closeCandidatePanel()
                        self.invalidateLayoutAndDisplay()
                    }
                } else {
                    self.closeCandidatePanelIfNeeded(afterCandidateSelection: candidate.global)
                    self.performSelectionFeedback()
                    self.delegate?.keyboardView(
                        self,
                        didSelectCandidate: candidate.selectIndex,
                        global: candidate.global
                    )
                }
                return true
            }
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

    private func activateAccessibilityKey(_ key: KeyRect) {
        let command = resolveCommand(
            key.spec,
            deltaY: 0,
            rect: key.rect,
            releaseY: key.rect.midY
        )
        performPressFeedback()
        rememberRecentEmoji(command)
        delegate?.keyboardView(self, didTrigger: command)
        clearOneShotShift(after: command)
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
        if let candidateMenu {
            drawCandidateMenu(candidateMenu)
            return
        }
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

        if !state.candidatePanel.candidates.isEmpty || !completionSuggestions.isEmpty {
            if let context = UIGraphicsGetCurrentContext() {
                context.saveGState()
                UIBezierPath(rect: inlineCandidateViewportRect()).addClip()
                for candidate in candidateDrawItems(inlineOnly: true) {
                    guard let layout = inlineCandidateRects.first(where: { $0.identifierIndex == candidate.identifierIndex }) else {
                        continue
                    }
                    let pressed = pressedCandidate?.pageIndex == candidate.identifierIndex &&
                        inlineCandidateTouchActive && !candidateDragging && !candidateLongPressConsumed
                    drawCandidateOption(candidate, rect: layout.drawingRect ?? layout.rect, pressed: pressed)
                }
                context.restoreGState()
            }
            if let expand = candidateExpandRect {
                drawExpandButton(expand)
            }
            return
        }

        let preedit = state.candidatePanel.preedit ?? state.preedit
        if !config.hostMarkedTextEnabled, !preedit.isEmpty {
            let logo = logoRect()
            drawText(
                preedit,
                in: CGRect(
                    x: leftPadding,
                    y: 0,
                    width: max(0, logo.minX - leftPadding - theme.panel.gap),
                    height: barHeight
                ),
                color: theme.candidate.labelColor.uiColor,
                size: theme.font.preeditSize,
                weight: theme.font.weight,
                alignment: .left
            )
            drawLogo(in: logo)
            return
        }

        if let context = UIGraphicsGetCurrentContext() {
            context.saveGState()
            let compactToolbar = bounds.width < 300
            let viewport = CGRect(
                x: leftPadding,
                y: 0,
                width: max(0, logoRect().minX - (compactToolbar ? 4 : 8) - leftPadding),
                height: barHeight
            )
            UIBezierPath(rect: viewport).addClip()
            for toolbar in toolbarRects {
                drawToolbarChip(toolbar)
            }
            context.restoreGState()
        }
        drawLogo(in: logoRect())
    }

    private func drawCandidateMenu(_ menu: CandidateMenuState) {
        let codeText = "\(menu.text) · 编码 \(menu.code)"
        guard let codeRect = candidateMenuActionRects.first(where: { $0.action == "close" })?.rect else {
            return
        }
        keyBackgroundColor().setFill()
        UIBezierPath(roundedRect: codeRect, cornerRadius: candidateCornerRadius()).fill()
        drawText(
            menu.deletionUnavailable ? "\(codeText) · 系统词不可删除" : codeText,
            in: codeRect,
            color: theme.candidate.foreground.uiColor,
            size: candidateLabelSize(),
            weight: theme.font.weight,
            alignment: .center
        )
        if !menu.deletionUnavailable {
            guard let deleteRect = candidateMenuActionRects.first(where: { $0.action == "delete" })?.rect else {
                return
            }
            let pressed = pressedCandidateMenuAction?.action == "delete"
            drawSurfaceShadow(deleteRect, pressed: pressed)
            (pressed ? theme.candidate.selectedBackground.uiColor : keyBackgroundColor()).setFill()
            UIBezierPath(roundedRect: deleteRect, cornerRadius: candidateCornerRadius()).fill()
            drawText(
                "删除该词",
                in: deleteRect,
                color: pressed ? theme.candidate.selectedForeground.uiColor : theme.candidate.foreground.uiColor,
                size: candidateLabelSize(),
                weight: theme.font.weight,
                alignment: .center
            )
        }
    }

    private func drawBackspaceDeletionPreview() {
        guard let deleted = backspacePreviewText, !deleted.isEmpty, let rect = backspacePreviewRect else {
            return
        }
        drawSurfaceShadow(rect, pressed: backspacePreviewPressed)
        (backspacePreviewPressed ? theme.candidate.selectedBackground.uiColor : keyBackgroundColor()).setFill()
        UIBezierPath(roundedRect: rect, cornerRadius: candidateCornerRadius()).fill()
        drawText(
            backspacePreviewPendingSelection
                ? "将删除 \(deleted.count) 字：\(String(deleted.suffix(18))) · 抬手删除"
                : "已删除 \(deleted.count) 字：\(String(deleted.suffix(18))) · 点按恢复",
            in: rect,
            color: backspacePreviewPressed ? theme.candidate.selectedForeground.uiColor : theme.candidate.foreground.uiColor,
            size: candidateLabelSize(),
            weight: theme.font.weight,
            alignment: .center
        )
    }

    private func drawKeyboard() {
        let activeTouchSnapshot = activeTouches.values
        if usesCategorizedSymbolKeyboard(), let context = UIGraphicsGetCurrentContext() {
            context.saveGState()
            UIBezierPath(rect: CGRect(
                x: 0,
                y: keyboardScrollViewportTop,
                width: bounds.width,
                height: max(0, keyboardScrollViewportBottom - keyboardScrollViewportTop)
            )).addClip()
            for (index, key) in keyRects.enumerated() where !key.sticky {
                let touchState = activeTouchState(forKeyAt: index, in: activeTouchSnapshot)
                drawKey(
                    key.spec,
                    rect: key.rect,
                    pressed: touchState.pressed,
                    pressProgress: keyPressProgress(for: index, pressed: touchState.pressed),
                    pressedStackIndices: touchState.pressedStackIndices
                )
            }
            context.restoreGState()
            for (index, key) in keyRects.enumerated() where key.sticky {
                let touchState = activeTouchState(forKeyAt: index, in: activeTouchSnapshot)
                drawKey(
                    key.spec,
                    rect: key.rect,
                    pressed: touchState.pressed,
                    pressProgress: keyPressProgress(for: index, pressed: touchState.pressed),
                    pressedStackIndices: touchState.pressedStackIndices
                )
            }
            drawVerticalScrollIndicator(
                viewportTop: keyboardScrollViewportTop,
                viewportBottom: keyboardScrollViewportBottom,
                contentHeight: keyboardScrollContentHeight,
                scrollY: keyboardScrollY,
                surface: .symbolKeyboard
            )
            return
        }
        for (index, key) in keyRects.enumerated() {
            let touchState = activeTouchState(forKeyAt: index, in: activeTouchSnapshot)
            drawKey(
                key.spec,
                rect: key.rect,
                pressed: touchState.pressed,
                pressProgress: keyPressProgress(for: index, pressed: touchState.pressed),
                pressedStackIndices: touchState.pressedStackIndices
            )
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
        drawVerticalScrollIndicator(
            viewportTop: top,
            viewportBottom: keyboardBottom(),
            contentHeight: expandedCandidateContentHeight,
            scrollY: expandedCandidateScrollY,
            surface: .expandedPanel
        )
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

    private func drawCandidateOption(_ item: CandidateDrawItem, rect: CGRect, pressed: Bool = false) {
        if item.style == .section {
            drawRimeSectionHeader(item, rect: rect)
            return
        }
        if item.style == .empty {
            drawRimeEmptyState(item, rect: rect)
            return
        }
        let selected = item.selected || pressed
        if selected {
            drawSurfaceShadow(rect, pressed: pressed, cornerRadius: candidateCornerRadius())
        }
        (selected ? theme.candidate.selectedBackground.uiColor : keyBackgroundColor()).setFill()
        UIBezierPath(roundedRect: rect, cornerRadius: candidateCornerRadius()).fill()

        let borderWidth = selected
            ? max(theme.candidate.borderWidth, 1)
            : KeyTaoIMEInteractionTuning.accentBorderWidth
        if borderWidth > 0 {
            let path = UIBezierPath(roundedRect: rect.insetBy(dx: borderWidth / 2, dy: borderWidth / 2), cornerRadius: candidateCornerRadius())
            path.lineWidth = borderWidth
            (selected
                ? theme.candidate.selectedBorderColor.uiColor
                : accentBorderColor(KeyTaoIMEInteractionTuning.candidateBorderAlpha)
            ).setStroke()
            path.stroke()
        }

        var displayItem = item
        displayItem.selected = selected
        switch item.style {
        case .schema:
            drawRimeSchemaRow(displayItem, rect: rect)
        case .option:
            drawRimeOptionPill(displayItem, rect: rect)
        default:
            switch panelColumns(for: functionPanelActive ? functionPanelMode : .rime) {
            case 4:
                drawCandidateGridCell(displayItem, rect: rect)
            case 1:
                drawClipboardCandidateRow(displayItem, rect: rect)
            default:
                drawInlineCandidateOption(displayItem, rect: rect)
            }
        }
    }

    private func drawRimeEmptyState(_ item: CandidateDrawItem, rect: CGRect) {
        drawText(
            item.label,
            in: rect,
            color: theme.candidate.commentColor.uiColor,
            size: candidateLabelSize(),
            weight: theme.font.weight,
            alignment: .center
        )
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
            item.statusLabel ?? (item.selected ? "ON" : "OFF"),
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
        let useAccent = item.action.selected || isSoftAccentToolbar(item.action)
        let rect = item.drawingRect ?? item.rect
        drawSurfaceShadow(rect, pressed: pressed)
        toolbarBackgroundColor(item.action, pressed: pressed).setFill()
        UIBezierPath(roundedRect: rect, cornerRadius: keyCornerRadius(for: rect)).fill()

        if useAccent {
            let borderWidth = KeyTaoIMEInteractionTuning.accentBorderWidth
            let path = UIBezierPath(
                roundedRect: rect.insetBy(dx: borderWidth / 2, dy: borderWidth / 2),
                cornerRadius: keyCornerRadius(for: rect)
            )
            path.lineWidth = borderWidth
            accentBorderColor(KeyTaoIMEInteractionTuning.accentToolbarBorderAlpha).setStroke()
            path.stroke()
        }

        if let secondary = item.action.secondaryLabel, !secondary.isEmpty {
            drawToolbarPair(primary: item.action.label, secondary: secondary, rect: rect, pressed: pressed)
        } else if let icon = item.action.icon {
            let color = pressed || item.action.selected
                ? theme.candidate.selectedForeground.uiColor
                : theme.candidate.foreground.uiColor
            drawToolbarIcon(icon, in: rect, color: color)
        } else {
            drawText(
                item.action.label,
                in: rect,
                color: pressed ? theme.candidate.selectedForeground.uiColor : theme.candidate.foreground.uiColor,
                font: fittedFont(for: item.action.label, size: theme.font.labelSize, maxWidth: rect.width - 10),
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

    private func drawKey(
        _ key: KeyTaoKeySpec,
        rect: CGRect,
        pressed: Bool,
        pressProgress: CGFloat,
        pressedStackIndices: Set<Int> = []
    ) {
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
            drawStackKey(
                stack,
                key: key,
                rect: rect,
                pressProgress: pressProgress,
                pressedStackIndices: pressedStackIndices
            )
            return
        }

        var keyRect = rect
        if pressProgress > 0 {
            keyRect.origin.y += pressProgress
        }
        let selected = isActiveKey(key)
        drawSurfaceShadow(keyRect, pressed: pressProgress > 0.5)
        keySurfaceColor(key, pressProgress: pressProgress).setFill()
        UIBezierPath(roundedRect: keyRect, cornerRadius: keyCornerRadius(for: keyRect)).fill()
        drawKeyOutline(key, rect: keyRect, pressed: pressProgress > 0.01)
        drawShiftStateDecoration(key, rect: keyRect)

        let label = displayLabel(key)
        let baseSize = keyLabelSize(for: label)
        let font = fittedFont(for: label, size: baseSize, maxWidth: keyRect.width - 10)
        let color = keyForegroundColor(key, selected: selected, pressProgress: pressProgress)
        drawText(label, in: keyRect, color: color, font: font, alignment: .center)

        if config.keyHintVisible, let hint = key.hint, !hint.isEmpty {
            let hintFont = themedFont(size: keyHintSize(keyHeight: keyRect.height), weight: .regular)
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

    private func drawStackKey(
        _ stack: [KeyTaoKeyStackItem],
        key: KeyTaoKeySpec,
        rect: CGRect,
        pressProgress: CGFloat,
        pressedStackIndices: Set<Int>
    ) {
        let itemRects = stackItemRects(in: rect, count: stack.count)
        for (index, item) in stack.enumerated() {
            let pressed = pressedStackIndices.contains(index)
            let itemPressProgress = pressed || pressedStackIndices.isEmpty ? pressProgress : 0
            var itemRect = itemRects[index]
            if itemPressProgress > 0 {
                itemRect.origin.y += itemPressProgress
            }
            let selected = isActiveKey(key)
            drawSurfaceShadow(itemRect, pressed: itemPressProgress > 0.5)
            keySurfaceColor(key, pressProgress: itemPressProgress).setFill()
            UIBezierPath(roundedRect: itemRect, cornerRadius: keyCornerRadius(for: itemRect)).fill()
            drawKeyOutline(key, rect: itemRect, pressed: itemPressProgress > 0.01)

            let label = stackLabelForMode(item)
            let baseSize = keyLabelSize(for: label)
            let font = fittedFont(for: label, size: baseSize, maxWidth: itemRect.width - 10)
            let color = keyForegroundColor(key, selected: selected, pressProgress: itemPressProgress)
            drawText(label, in: itemRect, color: color, font: font, alignment: .center)
        }
    }

    private func keySurfaceColor(_ key: KeyTaoKeySpec, pressProgress: CGFloat) -> UIColor {
        let active = isActiveKey(key)
        let normal = active && shiftState == .locked
            ? theme.candidate.selectedBackground.uiColor
            : keyBackgroundColor(key)
        guard pressProgress > 0 else { return normal }
        let pressed: UIColor
        if active {
            pressed = blend(
                foreground: theme.candidate.pressedBackground.uiColor,
                background: theme.candidate.selectedBackground.uiColor,
                amount: 0.52
            )
        } else if isSoftAccentKey(key) {
            pressed = blend(
                foreground: theme.candidate.pressedBackground.uiColor,
                background: softenedAccentSurfaceColor(KeyTaoIMEInteractionTuning.softAccentKeyFillAmount),
                amount: 0.72
            )
        } else {
            pressed = theme.candidate.pressedBackground.uiColor
        }
        return blend(foreground: pressed, background: normal, amount: pressProgress)
    }

    private func drawShiftStateDecoration(_ key: KeyTaoKeySpec, rect: CGRect) {
        guard key.action?.type == KeyTaoCommandType.shift, shiftState != .off else {
            return
        }
        let color = theme.candidate.selectedBorderColor.uiColor
        if shiftState == .once {
            color.setStroke()
            let outline = UIBezierPath(
                roundedRect: rect.insetBy(dx: 1.5, dy: 1.5),
                cornerRadius: keyCornerRadius(for: rect)
            )
            outline.lineWidth = 2
            outline.stroke()
        } else {
            color.setFill()
            let barWidth = rect.width * 0.36
            let barHeight: CGFloat = 2.5
            let bar = CGRect(
                x: rect.midX - barWidth / 2,
                y: rect.maxY - 7,
                width: barWidth,
                height: barHeight
            )
            UIBezierPath(roundedRect: bar, cornerRadius: barHeight / 2).fill()
        }
    }

    private func drawKeyFeedbackOverlays() {
        for touch in activeTouches.values {
            if let panel = touch.alternatePanel {
                drawAlternatePanel(panel)
            } else if let text = previewText(for: touch) {
                drawKeyPreview(for: touch.key, text: text)
            }
        }
    }

    private func previewText(for touch: ActiveTouch) -> String? {
        guard config.keyPreviewEnabled,
              !touch.longPressConsumed,
              !keyboardDragging,
              !functionPanelActive else {
            return nil
        }
        let deltaY = touch.currentTouchPoint.y - touch.touchStart.y
        let key = abs(deltaY) >= config.swipeThresholdDp ? touch.originKey : touch.key
        guard isCharacterKey(key.spec) else {
            return nil
        }
        if abs(deltaY) < config.swipeThresholdDp {
            let retainedRect = key.rect.insetBy(
                dx: -KeyTaoIMEInteractionTuning.slideRetargetHysteresis,
                dy: -KeyTaoIMEInteractionTuning.slideRetargetHysteresis
            )
            guard retainedRect.contains(touch.currentTouchPoint) else {
                return nil
            }
        }
        let command = resolveCommand(
            key.spec,
            deltaY: deltaY,
            rect: key.rect,
            releaseY: touch.currentTouchPoint.y
        )
        guard [
            KeyTaoCommandType.input,
            KeyTaoCommandType.directInput,
            KeyTaoCommandType.rimeInput,
        ].contains(command.type) else {
            return nil
        }
        return command.value?.isEmpty == false ? command.value : displayLabel(key.spec)
    }

    private func drawKeyPreview(for key: KeyRect, text: String) {
        let margin = Self.keyPreviewMargin
        let bubbleWidth = min(
            max(Self.keyPreviewMinimumWidth, key.rect.width * 1.08),
            max(CGFloat(1), bounds.width - margin * 2)
        )
        let bubbleHeight = max(
            Self.keyPreviewMinimumHeight,
            min(Self.keyPreviewMaximumHeight, key.rect.height * 1.12)
        )
        let left = max(margin, min(bounds.width - margin - bubbleWidth, key.rect.midX - bubbleWidth / 2))
        let top = max(margin, key.rect.minY - bubbleHeight + Self.keyPreviewKeyOverlap)
        let bubble = CGRect(x: left, y: top, width: bubbleWidth, height: bubbleHeight)
        drawSurfaceShadow(bubble, pressed: false, cornerRadius: keyCornerRadius(for: bubble) + 3)
        theme.candidate.pressedBackground.uiColor.setFill()
        UIBezierPath(roundedRect: bubble, cornerRadius: keyCornerRadius(for: bubble) + 3).fill()
        theme.candidate.selectedBorderColor.uiColor.setStroke()
        let border = UIBezierPath(roundedRect: bubble, cornerRadius: keyCornerRadius(for: bubble) + 3)
        border.lineWidth = max(1, pixel)
        border.stroke()
        drawText(
            text,
            in: bubble,
            color: theme.candidate.selectedForeground.uiColor,
            size: Self.keyPreviewTextSize,
            weight: theme.font.weight,
            alignment: .center
        )
    }

    private func drawAlternatePanel(_ panel: AlternatePanel) {
        drawSurfaceShadow(panel.rect, pressed: false, cornerRadius: keyCornerRadius(for: panel.rect) + 3)
        panelBackgroundColor().setFill()
        UIBezierPath(roundedRect: panel.rect, cornerRadius: keyCornerRadius(for: panel.rect) + 3).fill()
        let itemWidth = panel.rect.width / CGFloat(panel.options.count)
        for (index, option) in panel.options.enumerated() {
            let item = CGRect(
                x: panel.rect.minX + itemWidth * CGFloat(index),
                y: panel.rect.minY,
                width: itemWidth,
                height: panel.rect.height
            )
            if panel.selectedIndex == index {
                theme.candidate.pressedBackground.uiColor.setFill()
                UIBezierPath(roundedRect: item, cornerRadius: keyCornerRadius(for: item)).fill()
            }
            drawText(
                option.label,
                in: item,
                color: panel.selectedIndex == index
                    ? theme.candidate.selectedForeground.uiColor
                    : theme.candidate.foreground.uiColor,
                size: Self.alternatePanelTextSize,
                weight: theme.font.weight,
                alignment: .center
            )
        }
        theme.candidate.selectedBorderColor.uiColor.setStroke()
        let border = UIBezierPath(roundedRect: panel.rect, cornerRadius: keyCornerRadius(for: panel.rect) + 3)
        border.lineWidth = max(1, pixel)
        border.stroke()
    }

    private func drawKeyOutline(_ key: KeyTaoKeySpec, rect: CGRect, pressed: Bool) {
        let softAccent = isSoftAccentKey(key)
        guard !pressed || softAccent else {
            return
        }
        let outline = rect.insetBy(dx: 1, dy: 1)
        let path = UIBezierPath(roundedRect: outline, cornerRadius: max(0, keyCornerRadius(for: rect) - 1))
        path.lineWidth = softAccent ? KeyTaoIMEInteractionTuning.accentBorderWidth : max(1, 0.7)
        if softAccent {
            accentBorderColor(pressed ? 1 : KeyTaoIMEInteractionTuning.softAccentKeyBorderAlpha).setStroke()
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
            let footerTop = bottom - verticalGap - rowHeight
            let emojiTabsAtBottom = isEmojiLayer
            let headerTop = emojiTabsAtBottom
                ? footerTop - verticalGap - rowHeight
                : top + verticalGap
            keyboardScrollViewportTop = emojiTabsAtBottom
                ? top + verticalGap
                : headerTop + rowHeight + verticalGap
            keyboardScrollViewportBottom = emojiTabsAtBottom
                ? max(keyboardScrollViewportTop, headerTop - verticalGap)
                : max(keyboardScrollViewportTop, footerTop - verticalGap)
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
                startY: keyboardScrollViewportTop - keyboardVisualScrollY(),
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
        guard !state.candidatePanel.candidates.isEmpty || !completionSuggestions.isEmpty else {
            candidateContentWidth = 0
            candidateViewportWidth = 0
            return []
        }
        let barHeight = config.candidateBarHeightDp
        let gap = theme.panel.gap
        let viewport = inlineCandidateViewportRect()
        candidateViewportWidth = viewport.width
        let candidateHeight = min(38, barHeight - gap * 1.8)
        let top = (barHeight - candidateHeight) / 2
        let items = candidateDrawItems(inlineOnly: true)
        let itemWidths = items.map(candidateWidth)
        candidateContentWidth = itemWidths.reduce(0, +) + gap * CGFloat(max(0, itemWidths.count - 1))
        coerceCandidateScroll()
        var x = viewport.minX - candidateScrollX
        var rects: [CandidateRect] = []
        for (item, width) in zip(items, itemWidths) {
            let drawingRect = CGRect(x: x, y: top, width: width, height: candidateHeight)
            let hitRect = drawingRect.intersection(viewport)
            if !hitRect.isNull, hitRect.width > 0 {
                rects.append(
                    CandidateRect(
                        identifierIndex: item.identifierIndex,
                        selectIndex: item.selectIndex,
                        rect: hitRect,
                        global: item.global,
                        command: item.command,
                        drawingRect: drawingRect,
                        pageIndex: item.identifierIndex,
                        label: item.text,
                        comment: item.comment,
                        clipboardText: item.clipboardText
                    )
                )
            }
            x = drawingRect.maxX + gap
        }
        return rects
    }

    private func inlineCandidateViewportRect() -> CGRect {
        let leftPadding = theme.panel.gap * 1.5
        let left = leftPadding
        let right = (expandButtonRect()?.minX ?? bounds.width - leftPadding) - theme.panel.gap
        return CGRect(x: left, y: 0, width: max(0, right - left), height: config.candidateBarHeightDp)
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
        let visualScrollY = expandedCandidateVisualScrollY()
        var y = top + gap - visualScrollY
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
                case .standard, .empty:
                    width = right - left
                    itemRowHeight = defaultRowHeight
                }
            } else if let columns, let cellWidth {
                let column = index % columns
                let row = index / columns
                x = left + CGFloat(column) * (cellWidth + gap)
                y = top + gap + CGFloat(row) * (defaultRowHeight + gap) - visualScrollY
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
                if item.style == .section || item.style == .empty {
                    sectionRects[item.identifierIndex] = drawingRect
                } else {
                    rects.append(
                        CandidateRect(
                            identifierIndex: item.identifierIndex,
                            selectIndex: item.selectIndex,
                            rect: hitRect,
                            global: item.global,
                            command: item.command,
                            drawingRect: drawingRect,
                            pageIndex: item.identifierIndex,
                            label: item.text,
                            comment: item.comment,
                            clipboardText: item.clipboardText
                        )
                    )
                }
            }
            contentBottom = max(contentBottom, drawingRect.maxY + visualScrollY)
            if structuredRime {
                switch item.style {
                case .section, .schema, .standard, .empty:
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
        let previousScrollY = expandedCandidateScrollY
        coerceExpandedCandidateScroll()
        if expandedCandidateScrollY != previousScrollY {
            return expandedCandidateLayout()
        }
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
        let barHeight = config.candidateBarHeightDp
        let leftPadding = theme.panel.gap * 1.5
        if !state.candidatePanel.candidates.isEmpty || !completionSuggestions.isEmpty {
            return []
        }
        let preedit = state.candidatePanel.preedit ?? state.preedit
        if !config.hostMarkedTextEnabled, !preedit.isEmpty {
            return []
        }
        let chipHeight = min(34, barHeight - 12)
        let top = (barHeight - chipHeight) / 2
        let compactToolbar = bounds.width < 300
        let maxRight = logoRect().minX - (compactToolbar ? 4 : 8)
        let actions = toolbarActions()
        let availableWidth = max(0, maxRight - leftPadding)
        let naturalWidths = actions.map { toolbarChipWidth($0) }
        let naturalGap: CGFloat = 6
        let naturalTotal = naturalWidths.reduce(0, +) + naturalGap * CGFloat(max(0, actions.count - 1))
        let compression = naturalTotal > 0 ? max(0.85, min(1, availableWidth / naturalTotal)) : 1
        let gap = naturalGap * compression
        let widths = naturalWidths.map { $0 * compression }
        toolbarViewportWidth = availableWidth
        toolbarContentWidth = widths.reduce(0, +) + gap * CGFloat(max(0, actions.count - 1))
        coerceToolbarScroll()
        let viewport = CGRect(x: leftPadding, y: 0, width: availableWidth, height: barHeight)
        var x = leftPadding - toolbarScrollX
        var rects: [ToolbarRect] = []
        for (action, width) in zip(actions, widths) {
            let drawingRect = CGRect(x: x, y: top, width: width, height: chipHeight)
            let hitRect = drawingRect.intersection(viewport)
            if !hitRect.isNull, hitRect.width > 0 {
                rects.append(ToolbarRect(action: action, rect: hitRect, drawingRect: drawingRect))
            }
            x = drawingRect.maxX + gap
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
        if state.candidatePanel.candidates.isEmpty {
            return completionSuggestions.enumerated().map { index, suggestion in
                CandidateDrawItem(
                    identifierIndex: index,
                    selectIndex: index,
                    label: "补全",
                    text: suggestion.word,
                    comment: nil,
                    selected: false,
                    global: false,
                    command: .directInput(suggestion.insertion)
                )
            }
        }
        return state.candidatePanel.candidates.map { candidate in
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
                item.statusLabel ?? "",
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
            if rimeOptionsState.switches.isEmpty {
                items.append(
                    CandidateDrawItem(
                        identifierIndex: -3100,
                        selectIndex: -3100,
                        label: "当前方案没有可用选项",
                        text: "",
                        comment: nil,
                        selected: false,
                        global: false,
                        command: nil,
                        style: .empty
                    )
                )
            } else {
                items.append(contentsOf: rimeOptionsState.switches.enumerated().map { index, schemaSwitch in
                    rimeSwitchItem(index: index, schemaSwitch: schemaSwitch)
                })
            }
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

    private func rimeSwitchItem(index: Int, schemaSwitch: KeyTaoRimeSchemaSwitch) -> CandidateDrawItem {
        if !schemaSwitch.options.isEmpty {
            let activeIndex = schemaSwitch.options.firstIndex { rimeOptionsState.options[$0] == true }
            let currentOption = activeIndex.map { schemaSwitch.options[$0] }
            let nextIndex: Int
            if let activeIndex {
                nextIndex = (activeIndex + 1) % schemaSwitch.options.count
            } else {
                nextIndex = min(max(schemaSwitch.reset ?? 0, 0), schemaSwitch.options.count - 1)
            }
            let labels = schemaSwitch.states.isEmpty ? schemaSwitch.options : schemaSwitch.states
            return CandidateDrawItem(
                identifierIndex: -3100 - index,
                selectIndex: -3100 - index,
                label: labels.joined(separator: " / "),
                text: activeIndex.map {
                    schemaSwitch.states.indices.contains($0)
                        ? schemaSwitch.states[$0]
                        : schemaSwitch.options[$0]
                } ?? "未选择",
                comment: nil,
                selected: activeIndex != nil,
                global: false,
                command: KeyTaoKeyCommand(
                    type: KeyTaoCommandType.rimeOption,
                    value: schemaSwitch.options[nextIndex],
                    fallbackValue: "choice:\(currentOption ?? "")"
                ),
                style: .option,
                statusLabel: "切换"
            )
        }

        let name = schemaSwitch.name ?? ""
        let enabled = rimeOptionsState.options[name] == true
        let stateIndex = enabled ? 1 : 0
        let stateLabel = schemaSwitch.states.indices.contains(stateIndex)
            ? schemaSwitch.states[stateIndex]
            : (enabled ? "开" : "关")
        return CandidateDrawItem(
            identifierIndex: -3100 - index,
            selectIndex: -3100 - index,
            label: schemaSwitch.states.count >= 2
                ? schemaSwitch.states.prefix(2).joined(separator: " / ")
                : name,
            text: stateLabel,
            comment: nil,
            selected: enabled,
            global: false,
            command: KeyTaoKeyCommand(
                type: KeyTaoCommandType.rimeOption,
                value: name,
                fallbackValue: String(!enabled)
            ),
            style: .option
        )
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
        if !hostTraits.isSensitive,
           layerMode.id == Self.emojiRecentLayer,
           !recentEmojis.isEmpty,
           rows.count >= 3 {
            let recentRows = stride(from: 0, to: recentEmojis.count, by: 8).map { start -> [KeyTaoKeySpec] in
                let end = min(start + 8, recentEmojis.count)
                return recentEmojis[start..<end].map { emoji in
                    KeyTaoKeySpec(label: emoji, value: emoji, action: .directInput(emoji))
                }
            }
            rows.insert(contentsOf: recentRows, at: 1)
        }
        if layerMode.id == "editor" {
            rows = replacingUnsupportedEditorKeys(in: rows)
        }
        if layerMode == .letters {
            if config.numberRowEnabled, let firstRow = rows.first {
                rows.insert(persistentNumberRow(firstRow), at: 0)
            } else if shouldUseInlineNumberRow() {
                rows = rows.enumerated().map { index, row in
                    index == 0 ? inlineNumberRow(row) : row
                }
            }
        }
        rows = applyEngineCapabilities(to: rows)
        return applyInputModeSwitchKey(to: rows)
    }

    private func replacingUnsupportedEditorKeys(in rows: [[KeyTaoKeySpec]]) -> [[KeyTaoKeySpec]] {
        rows.map { row in
            row.map { key in
                guard key.action?.type == KeyTaoCommandType.edit,
                      let verb = key.action?.value else {
                    return key
                }
                var replacement = key
                switch verb {
                case "selectAll":
                    replacement.label = "删词"
                    replacement.action = .edit("deleteWord")
                case "clearAll":
                    replacement.label = "括号"
                    replacement.action = .edit("insertPair", fallbackValue: "（）")
                case "redo":
                    replacement.label = "引号"
                    replacement.action = .edit("insertPair", fallbackValue: "“”")
                case "toggleSelection":
                    replacement.label = "短语"
                    replacement.action = .directInput("谢谢")
                default:
                    return key
                }
                replacement.asciiAction = replacement.action
                replacement.asciiLabel = replacement.label
                replacement.hint = nil
                replacement.longPress = nil
                replacement.asciiLongPress = nil
                replacement.alternates = nil
                replacement.asciiAlternates = nil
                return replacement
            }
        }
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

    private func persistentNumberRow(_ row: [KeyTaoKeySpec]) -> [KeyTaoKeySpec] {
        let symbols = Array("!@#$%^&*()")
        return inlineNumberRow(row).enumerated().map { index, key in
            guard index < symbols.count else {
                return key
            }
            let symbol = String(symbols[index])
            var next = key
            next.hint = symbol
            next.longPress = .input(symbol)
            next.asciiLongPress = .input(symbol)
            next.alternates = [KeyTaoKeyAlternate(label: symbol, value: symbol)]
            next.asciiAlternates = [KeyTaoKeyAlternate(label: symbol, value: symbol)]
            return next
        }
    }

    private func toolbarActions() -> [ToolbarAction] {
        let base = baseToolbarActions()
        let byID = Dictionary(uniqueKeysWithValues: base.compactMap { action in
            action.id.map { ($0, action) }
        })
        let configuredOrder = toolbarActionOrderOverride ?? config.toolbarActionOrder
        let orderedIDs = (configuredOrder + base.compactMap(\.id)).reduce(into: [String]()) { result, id in
            if !result.contains(id) { result.append(id) }
        }
        let ordered = orderedIDs.compactMap { byID[$0] }
        let pinnedCount = max(1, min(toolbarPinnedCountOverride ?? config.toolbarPinnedCount, max(1, ordered.count)))
        let more = ToolbarAction(
            label: toolbarMoreExpanded ? "收起" : "更多",
            command: .panel("toolbarMore"),
            longPressCommand: .panel("toolbarEdit")
        )
        if toolbarEditMode {
            var actions = Array(ordered.prefix(pinnedCount))
            actions.append(
                ToolbarAction(
                    label: "置顶｜更多",
                    command: .panel("toolbarPinnedBoundary"),
                    id: Self.toolbarPinnedBoundaryID
                )
            )
            actions.append(contentsOf: ordered.dropFirst(pinnedCount))
            actions.append(ToolbarAction(label: "完成", command: .panel("toolbarDone")))
            return actions
        }
        if toolbarMoreExpanded {
            return ordered + [ToolbarAction(label: "编辑", command: .panel("toolbarEdit"))]
        }
        return Array(ordered.prefix(pinnedCount)) + [more]
    }

    private func baseToolbarActions() -> [ToolbarAction] {
        let function = ToolbarAction(
            label: "Rime",
            command: KeyTaoKeyCommand(type: KeyTaoCommandType.panel, value: "rime", fallbackValue: nil),
            icon: .function,
            id: "rime"
        )
        let languageToggle = languageToggleAction()
        let layoutModeName = layoutPresentation.displayedMode == .split ? "分栏" : "单手"
        let layout = ToolbarAction(
            label: layoutPresentation.isEnabled ? "退出\(layoutModeName)" : layoutModeName,
            command: KeyTaoKeyCommand(type: KeyTaoCommandType.floating, value: nil, fallbackValue: nil),
            selected: layoutPresentation.isEnabled,
            icon: .layout,
            id: "layout"
        )
        let settings = ToolbarAction(
            label: "设置",
            command: KeyTaoKeyCommand(type: KeyTaoCommandType.openPage, value: "settings", fallbackValue: nil),
            icon: .settings,
            id: "settings"
        )
        if layerMode == .symbols {
            return [
                function,
                ToolbarAction(label: "中", command: KeyTaoKeyCommand(type: KeyTaoCommandType.mode, value: "chinese", fallbackValue: nil), selected: !state.asciiMode, id: "chinese"),
                ToolbarAction(label: "En", command: KeyTaoKeyCommand(type: KeyTaoCommandType.mode, value: "ascii", fallbackValue: nil), selected: state.asciiMode, id: "english"),
                ToolbarAction(label: "123", command: KeyTaoKeyCommand(type: KeyTaoCommandType.keyboardMode, value: "numbers", fallbackValue: nil), id: "numbers"),
                ToolbarAction(label: "ABC", command: KeyTaoKeyCommand(type: KeyTaoCommandType.keyboardMode, value: "letters", fallbackValue: nil), id: "letters"),
                layout,
                settings,
            ]
        } else {
            return [
                function,
                languageToggle,
                ToolbarAction(label: "选择", command: KeyTaoKeyCommand(type: KeyTaoCommandType.keyboardMode, value: "editor", fallbackValue: nil), icon: .selection, id: "editor"),
                ToolbarAction(
                    label: "剪贴板",
                    command: KeyTaoKeyCommand(type: KeyTaoCommandType.panel, value: "clipboard", fallbackValue: nil),
                    icon: .clipboard,
                    longPressCommand: .panel("clearClipboardHistoryNow"),
                    id: "clipboard"
                ),
                ToolbarAction(
                    label: "Emoji",
                    command: KeyTaoKeyCommand(type: KeyTaoCommandType.keyboardMode, value: Self.emojiRecentLayer, fallbackValue: nil),
                    icon: .emoji,
                    id: "emoji"
                ),
                layout,
                settings,
            ]
        }
    }

    private func currentToolbarOrder() -> [String] {
        let available = baseToolbarActions().compactMap(\.id)
        return ((toolbarActionOrderOverride ?? config.toolbarActionOrder) + available).reduce(into: []) { result, id in
            if available.contains(id), !result.contains(id) {
                result.append(id)
            }
        }
    }

    private func currentToolbarPinnedCount() -> Int {
        max(1, min(toolbarPinnedCountOverride ?? config.toolbarPinnedCount, max(1, currentToolbarOrder().count)))
    }

    private func reorderToolbarAction(_ actionID: String, at x: CGFloat) {
        var order = currentToolbarOrder()
        guard let sourceIndex = order.firstIndex(of: actionID),
              let targetID = toolbarRects
                .filter({ $0.action.customizable })
                .min(by: {
                    abs(($0.drawingRect ?? $0.rect).midX - x) < abs(($1.drawingRect ?? $1.rect).midX - x)
                })?
                .action.id,
              var targetIndex = order.firstIndex(of: targetID) else {
            return
        }
        let oldOrder = order
        let oldPinnedCount = currentToolbarPinnedCount()
        var pinnedIDs = Set(order.prefix(oldPinnedCount))
        let boundaryMidX = toolbarRects
            .first(where: { $0.action.id == Self.toolbarPinnedBoundaryID })
            .map { ($0.drawingRect ?? $0.rect).midX }
        let movingToPinned = boundaryMidX.map { x < $0 } ?? (targetIndex < oldPinnedCount)
        order.remove(at: sourceIndex)
        if sourceIndex < targetIndex { targetIndex -= 1 }
        order.insert(actionID, at: max(0, min(targetIndex, order.count)))
        if movingToPinned { pinnedIDs.insert(actionID) } else { pinnedIDs.remove(actionID) }
        let pinnedCount = max(1, min(pinnedIDs.count, max(1, order.count)))
        guard order != oldOrder || pinnedCount != oldPinnedCount else { return }
        toolbarActionOrderOverride = order
        toolbarPinnedCountOverride = pinnedCount
        performSelectionFeedback(playSound: false)
    }

    private func persistToolbarCustomization() {
        let persistedOrder = (currentToolbarOrder() + toolbarInactiveActionIDs).reduce(into: [String]()) { result, id in
            if !result.contains(id) { result.append(id) }
        }
        toolbarActionOrderOverride = persistedOrder
        delegate?.keyboardView(
            self,
            persistToolbarOrder: persistedOrder,
            pinnedCount: currentToolbarPinnedCount()
        )
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
                secondaryLabel: "中",
                longPressCommand: KeyTaoKeyCommand(type: KeyTaoCommandType.keyboardMode, value: "symbols", fallbackValue: nil),
                id: "language"
            )
        }
        return ToolbarAction(
            label: "中",
            command: KeyTaoKeyCommand(type: KeyTaoCommandType.mode, value: nil, fallbackValue: nil),
            secondaryLabel: "En",
            longPressCommand: KeyTaoKeyCommand(type: KeyTaoCommandType.keyboardMode, value: "symbols", fallbackValue: nil),
            id: "language"
        )
    }

    private func handleToolbarCommand(_ command: KeyTaoKeyCommand) {
        if handlePanelCommand(command) {
            return
        }
        performSelectionFeedback()
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
            case "clearClipboardHistoryNow":
                clipboardClearConfirmationPending = false
                clipboardItems = []
                delegate?.keyboardViewClearClipboardHistory(self)
            case "toolbarMore":
                toolbarMoreExpanded.toggle()
                toolbarScrollX = 0
                rebuildInteractiveRects()
            case "toolbarEdit":
                let visibleOrder = currentToolbarOrder()
                let configuredOrder = toolbarActionOrderOverride ?? config.toolbarActionOrder
                toolbarInactiveActionIDs = configuredOrder.filter { !visibleOrder.contains($0) }
                toolbarEditMode = true
                toolbarMoreExpanded = true
                toolbarScrollX = 0
                toolbarActionOrderOverride = visibleOrder
                toolbarPinnedCountOverride = currentToolbarPinnedCount()
                rebuildInteractiveRects()
            case "toolbarDone":
                toolbarEditMode = false
                toolbarMoreExpanded = false
                toolbarScrollX = 0
                persistToolbarCustomization()
                rebuildInteractiveRects()
            case "toolbarPinnedBoundary":
                break
            default:
                setLayer("letters")
            }
            performSelectionFeedback()
            invalidateLayoutAndDisplay()
            return true
        }
        performSelectionFeedback()
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
        invalidateLayoutAndDisplay()
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
        invalidateLayoutAndDisplay()
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
        invalidateLayoutAndDisplay()
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
        performSelectionFeedback()
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
        if verticalScrollSurface == .expandedPanel {
            stopVerticalScrollAnimation()
        }
        expandedCandidateScrollY = 0
        expandedCandidateOverscrollY = 0
        expandedCandidateContentHeight = expandedCandidatePanelHeight()
    }

    private func resetCandidateScroll() {
        candidateScrollX = 0
        candidateContentWidth = bounds.width
        candidateViewportWidth = bounds.width
    }

    private func resetKeyboardScroll() {
        if verticalScrollSurface == .symbolKeyboard {
            stopVerticalScrollAnimation()
        }
        keyboardScrollTouchIdentifier = nil
        keyboardScrollY = 0
        keyboardOverscrollY = 0
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

    private func maxCandidateScroll() -> CGFloat {
        max(0, candidateContentWidth - candidateViewportWidth)
    }

    private func maxToolbarScroll() -> CGFloat {
        max(0, toolbarContentWidth - toolbarViewportWidth)
    }

    private func coerceToolbarScroll() {
        toolbarScrollX = max(0, min(maxToolbarScroll(), toolbarScrollX))
    }

    private func coerceCandidateScroll() {
        candidateScrollX = max(0, min(maxCandidateScroll(), candidateScrollX))
    }

    private func maxKeyboardScroll() -> CGFloat {
        max(0, keyboardScrollContentHeight - keyboardScrollViewportHeight)
    }

    private func coerceExpandedCandidateScroll() {
        expandedCandidateScrollY = max(0, min(maxExpandedCandidateScroll(), expandedCandidateScrollY))
    }

    private func expandedCandidateVisualScrollY() -> CGFloat {
        expandedCandidateScrollY + expandedCandidateOverscrollY
    }

    private func keyboardVisualScrollY() -> CGFloat {
        keyboardScrollY + keyboardOverscrollY
    }

    private func setExpandedCandidateScroll(_ rawScrollY: CGFloat, rubberBand: Bool) {
        let maximum = maxExpandedCandidateScroll()
        let clamped = max(0, min(maximum, rawScrollY))
        expandedCandidateScrollY = clamped
        let overscroll = rawScrollY - clamped
        expandedCandidateOverscrollY = max(
            -Self.scrollOverscrollDistance,
            min(
                Self.scrollOverscrollDistance,
                overscroll * (rubberBand ? Self.scrollRubberBandFactor : 1)
            )
        )
    }

    private func setKeyboardScroll(_ rawScrollY: CGFloat, rubberBand: Bool) {
        let maximum = maxKeyboardScroll()
        let clamped = max(0, min(maximum, rawScrollY))
        keyboardScrollY = clamped
        let overscroll = rawScrollY - clamped
        keyboardOverscrollY = max(
            -Self.scrollOverscrollDistance,
            min(
                Self.scrollOverscrollDistance,
                overscroll * (rubberBand ? Self.scrollRubberBandFactor : 1)
            )
        )
    }

    private func beginVerticalScrollGesture(at y: CGFloat) -> VerticalScrollSurface? {
        let interruptedSurface = verticalScrollSurface
        stopVerticalScrollAnimation(settleAtBoundary: true)
        scrollGestureLastY = y
        scrollGestureLastTimestamp = CACurrentMediaTime()
        scrollGestureVelocityY = 0
        return interruptedSurface
    }

    private func updateVerticalScrollGesture(at y: CGFloat) {
        let now = CACurrentMediaTime()
        let elapsed = now - scrollGestureLastTimestamp
        if elapsed > 0 {
            let instantVelocity = -(y - scrollGestureLastY) / CGFloat(elapsed)
            scrollGestureVelocityY = scrollGestureVelocityY * 0.72 + instantVelocity * 0.28
        }
        scrollGestureLastY = y
        scrollGestureLastTimestamp = now
    }

    private func startVerticalScrollAnimation(surface: VerticalScrollSurface) {
        verticalScrollSurface = surface
        verticalScrollVelocityY = abs(scrollGestureVelocityY) >= Self.minimumScrollVelocity
            ? max(-Self.maximumScrollVelocity, min(Self.maximumScrollVelocity, scrollGestureVelocityY))
            : 0
        showScrollIndicator(surface)
        ensureVerticalScrollDisplayLink()
    }

    private func settleVerticalScrollAfterCancellation() {
        let surface: VerticalScrollSurface?
        if expandedCandidateOverscrollY != 0 {
            surface = .expandedPanel
        } else if keyboardOverscrollY != 0 {
            surface = .symbolKeyboard
        } else {
            surface = nil
        }
        guard let surface else { return }
        verticalScrollSurface = surface
        verticalScrollVelocityY = 0
        showScrollIndicator(surface)
        ensureVerticalScrollDisplayLink()
    }

    private func showScrollIndicator(_ surface: VerticalScrollSurface) {
        scrollIndicatorSurface = surface
        scrollIndicatorAlpha = 1
        scrollIndicatorHoldRemaining = Self.scrollIndicatorHoldDuration
    }

    private func ensureVerticalScrollDisplayLink() {
        guard verticalScrollDisplayLink == nil else { return }
        verticalScrollLastTimestamp = 0
        let displayLink = CADisplayLink(target: self, selector: #selector(stepVerticalScroll(_:)))
        displayLink.add(to: .main, forMode: .common)
        verticalScrollDisplayLink = displayLink
    }

    private func stopVerticalScrollAnimation(settleAtBoundary: Bool = false) {
        verticalScrollDisplayLink?.invalidate()
        verticalScrollDisplayLink = nil
        verticalScrollLastTimestamp = 0
        verticalScrollSurface = nil
        verticalScrollVelocityY = 0
        if settleAtBoundary {
            scrollIndicatorAlpha = 0
            scrollIndicatorSurface = nil
            scrollIndicatorHoldRemaining = 0
            expandedCandidateOverscrollY = 0
            keyboardOverscrollY = 0
            coerceExpandedCandidateScroll()
            keyboardScrollY = max(0, min(maxKeyboardScroll(), keyboardScrollY))
            refreshScrollLayoutAndDisplay()
        }
    }

    @objc private func stepVerticalScroll(_ displayLink: CADisplayLink) {
        let previousTimestamp = verticalScrollLastTimestamp
        verticalScrollLastTimestamp = displayLink.timestamp
        guard previousTimestamp > 0 else { return }
        let elapsed = min(1.0 / 30.0, displayLink.timestamp - previousTimestamp)
        if let surface = verticalScrollSurface {
            let current = surface == .expandedPanel
                ? expandedCandidateVisualScrollY()
                : keyboardVisualScrollY()
            let maximum = surface == .expandedPanel
                ? maxExpandedCandidateScroll()
                : maxKeyboardScroll()
            var velocity = verticalScrollVelocityY
            var next = current + velocity * CGFloat(elapsed)
            if next < 0 || next > maximum {
                let boundary = max(0, min(maximum, next))
                velocity += (boundary - next) * Self.scrollSpringStrength * CGFloat(elapsed)
                velocity *= CGFloat(pow(Self.scrollBoundaryDamping, elapsed * 60))
            } else {
                velocity *= CGFloat(pow(Self.scrollFrictionPerFrame, elapsed * 60))
            }
            verticalScrollVelocityY = velocity
            if surface == .expandedPanel {
                setExpandedCandidateScroll(next, rubberBand: false)
                next = expandedCandidateVisualScrollY()
            } else {
                setKeyboardScroll(next, rubberBand: false)
                next = keyboardVisualScrollY()
            }
            let overscroll = next - max(0, min(maximum, next))
            if abs(velocity) < Self.scrollStopVelocity && abs(overscroll) < 0.5 {
                if surface == .expandedPanel {
                    setExpandedCandidateScroll(next, rubberBand: false)
                    expandedCandidateOverscrollY = 0
                } else {
                    setKeyboardScroll(next, rubberBand: false)
                    keyboardOverscrollY = 0
                }
                verticalScrollSurface = nil
                verticalScrollVelocityY = 0
            }
            refreshScrollLayoutAndDisplay()
        } else if scrollIndicatorHoldRemaining > 0 {
            scrollIndicatorHoldRemaining = max(0, scrollIndicatorHoldRemaining - elapsed)
        } else if scrollIndicatorAlpha > 0 {
            scrollIndicatorAlpha = max(0, scrollIndicatorAlpha - CGFloat(elapsed / Self.scrollIndicatorFadeDuration))
            setNeedsDisplay()
        }
        if verticalScrollSurface == nil && scrollIndicatorAlpha <= 0 {
            stopVerticalScrollAnimation()
        }
    }

    private func refreshScrollLayoutAndDisplay() {
        rebuildInteractiveRects()
        setNeedsDisplay()
    }

    private func drawVerticalScrollIndicator(
        viewportTop: CGFloat,
        viewportBottom: CGFloat,
        contentHeight: CGFloat,
        scrollY: CGFloat,
        surface: VerticalScrollSurface
    ) {
        guard scrollIndicatorSurface == surface, scrollIndicatorAlpha > 0 else { return }
        let viewportHeight = viewportBottom - viewportTop
        guard viewportHeight > 0, contentHeight > viewportHeight else { return }
        let maximum = contentHeight - viewportHeight
        let thumbHeight = max(Self.scrollIndicatorMinimumThumb, viewportHeight * viewportHeight / contentHeight)
        let travel = max(0, viewportHeight - thumbHeight - 4)
        let thumbTop = viewportTop + 2 + travel * (max(0, min(maximum, scrollY)) / maximum)
        theme.candidate.commentColor.uiColor
            .withAlphaComponent(scrollIndicatorAlpha * Self.scrollIndicatorMaximumAlpha)
            .setFill()
        UIBezierPath(
            roundedRect: CGRect(
                x: bounds.width - Self.scrollIndicatorWidth - 2,
                y: thumbTop,
                width: Self.scrollIndicatorWidth,
                height: thumbHeight
            ),
            cornerRadius: Self.scrollIndicatorWidth / 2
        ).fill()
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

    private func handleBackspaceDrag(identifier: ObjectIdentifier, at point: CGPoint) -> Bool {
        guard var touch = activeTouches[identifier], isBackspaceKey(touch.key.spec) else {
            return false
        }
        let deltaX = point.x - touch.touchStart.x
        let deltaY = point.y - touch.touchStart.y
        let threshold = max(CGFloat(8), config.swipeThresholdDp * 0.65)
        if usesSelectionBackspaceGesture(), touch.backspaceGestureConsumed, abs(deltaX) <= threshold {
            if let command = KeyTaoBackspaceGesturePolicy.dragCommand(
                mode: .selectThenDelete,
                currentUnits: touch.backspaceGestureUnits,
                requestedUnits: 0,
                maximumUnits: Self.maxBackspaceGestureUnitsPerGesture
            ) {
                delegate?.keyboardView(self, didTrigger: backspaceGestureCommand(command.action, count: command.count))
            }
            touch.backspaceGestureUnits = 0
            activeTouches[identifier] = touch
            return true
        }
        guard abs(deltaX) > threshold, abs(deltaX) > abs(deltaY) * 0.75 else {
            return false
        }

        let firstDragUpdate = !touch.backspaceGestureConsumed
        stopLongPressAndRepeat(for: identifier)
        touch.longPressConsumed = true
        touch.backspaceGestureConsumed = true

        let stepWidth = max(CGFloat(8), touch.key.rect.width * 0.22)
        let moved = max(CGFloat(0), abs(deltaX) - threshold)
        let stepCount = max(1, Int(floor(moved / stepWidth)) + 1)
        let requestedUnits = deltaX < 0 ? stepCount : -stepCount
        let targetUnits = usesSelectionBackspaceGesture()
            ? max(0, min(Self.maxBackspaceGestureUnitsPerGesture, requestedUnits))
            : max(-Self.maxBackspaceGestureUnitsPerGesture, min(Self.maxBackspaceGestureUnitsPerGesture, requestedUnits))
        let command = KeyTaoBackspaceGesturePolicy.dragCommand(
            mode: backspaceGestureMode(),
            currentUnits: touch.backspaceGestureUnits,
            requestedUnits: targetUnits,
            maximumUnits: Self.maxBackspaceGestureUnitsPerGesture
        )
        touch.backspaceGestureUnits = targetUnits
        activeTouches[identifier] = touch
        guard let command else {
            if firstDragUpdate {
                delegate?.keyboardView(self, didTrigger: backspaceGestureCommand("preview"))
            }
            return true
        }
        delegate?.keyboardView(self, didTrigger: backspaceGestureCommand(command.action, count: command.count))
        performConfiguredHaptic()
        return true
    }

    private func handleBackspaceRelease(for touch: ActiveTouch, at point: CGPoint) -> Bool {
        guard isBackspaceKey(touch.key.spec), !touch.backspaceGestureConsumed else {
            return false
        }
        let deltaX = point.x - touch.touchStart.x
        let deltaY = point.y - touch.touchStart.y
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

    private func backspaceGestureCommand(_ action: String, count: Int = 1) -> KeyTaoKeyCommand {
        KeyTaoKeyCommand(
            type: KeyTaoCommandType.backspaceGesture,
            value: action,
            fallbackValue: String(max(1, count))
        )
    }

    private func backspaceGestureMode() -> KeyTaoBackspaceGestureMode {
        hostTraits.isSensitive
            ? .immediate
            : KeyTaoBackspaceGestureMode(setting: config.backspaceGestureMode)
    }

    private func usesSelectionBackspaceGesture() -> Bool {
        backspaceGestureMode() == .selectThenDelete
    }

    private func isBackspaceKey(_ key: KeyTaoKeySpec) -> Bool {
        actionForMode(key).type == KeyTaoCommandType.backspace
    }

    private func isSpaceKey(_ key: KeyTaoKeySpec) -> Bool {
        actionForMode(key).type == KeyTaoCommandType.space
    }

    private var hasActiveComposition: Bool {
        state.hasComposition || !(state.candidatePanel.preedit ?? "").isEmpty
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
            command = resolveSwipeDownCommand(key)
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

    private func resolveSwipeDownCommand(_ key: KeyTaoKeySpec) -> KeyTaoKeyCommand {
        if let swipeDown = key.swipeDown {
            return swipeDown
        }
        guard config.flickKeysEnabled else {
            return actionForMode(key)
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

    private func activeTouchState(
        forKeyAt keyIndex: Int,
        in activeTouchSnapshot: [ActiveTouch]
    ) -> (pressed: Bool, pressedStackIndices: Set<Int>) {
        var pressed = false
        var pressedStackIndices = Set<Int>()
        for touch in activeTouchSnapshot where touch.keyIndex == keyIndex {
            pressed = true
            if let pressedStackIndex = touch.pressedStackIndex {
                pressedStackIndices.insert(pressedStackIndex)
            }
        }
        return (pressed, pressedStackIndices)
    }

    private func keyPressProgress(for keyIndex: Int, pressed: Bool) -> CGFloat {
        keyPressAnimations[keyIndex]?.progress ?? (pressed ? 1 : 0)
    }

    private func animateKeyPress(keyIndex: Int, pressed: Bool) {
        guard keyRects.indices.contains(keyIndex) else { return }
        let rect = keyRects[keyIndex].rect
        let target: CGFloat = pressed ? 1 : 0
        let current = keyPressAnimations[keyIndex]?.progress ?? (1 - target)
        if UIAccessibility.isReduceMotionEnabled || abs(current - target) < 0.001 {
            if target == 0 {
                keyPressAnimations.removeValue(forKey: keyIndex)
            } else {
                keyPressAnimations[keyIndex] = KeyPressAnimationState(progress: target, target: target)
            }
            setNeedsDisplay(rect.insetBy(dx: -4, dy: -4))
            return
        }
        keyPressAnimations[keyIndex] = KeyPressAnimationState(progress: current, target: target)
        if keyPressDisplayLink == nil {
            keyPressLastTimestamp = 0
            let displayLink = CADisplayLink(target: self, selector: #selector(stepKeyPressAnimations(_:)))
            displayLink.add(to: .main, forMode: .common)
            keyPressDisplayLink = displayLink
        }
    }

    @objc private func stepKeyPressAnimations(_ displayLink: CADisplayLink) {
        let previousTimestamp = keyPressLastTimestamp
        keyPressLastTimestamp = displayLink.timestamp
        guard previousTimestamp > 0 else { return }
        let step = CGFloat(min(1, (displayLink.timestamp - previousTimestamp) / Self.keyPressAnimationDuration))
        var completed: [Int] = []
        for (keyIndex, var animation) in keyPressAnimations {
            let delta = animation.target - animation.progress
            if abs(delta) <= step {
                animation.progress = animation.target
                completed.append(keyIndex)
            } else {
                animation.progress += delta.sign == .minus ? -step : step
            }
            keyPressAnimations[keyIndex] = animation
            if keyRects.indices.contains(keyIndex) {
                setNeedsDisplay(keyRects[keyIndex].rect.insetBy(dx: -4, dy: -4))
            }
        }
        for keyIndex in completed where keyPressAnimations[keyIndex]?.target == 0 {
            keyPressAnimations.removeValue(forKey: keyIndex)
        }
        if keyPressAnimations.values.allSatisfy({ abs($0.progress - $0.target) < 0.001 }) {
            displayLink.invalidate()
            keyPressDisplayLink = nil
            keyPressLastTimestamp = 0
        }
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

    private func scheduleLongPressIfNeeded(for identifier: ObjectIdentifier) {
        guard var touch = activeTouches[identifier], keySupportsLongPress(touch.key.spec) else {
            return
        }
        let workItem = DispatchWorkItem { [weak self] in
            guard let self, var activeTouch = self.activeTouches[identifier], activeTouch.longPressWorkItem != nil else {
                return
            }
            activeTouch.longPressWorkItem = nil
            activeTouch.longPressConsumed = true
            let options = self.explicitAlternates(activeTouch.key.spec)
            if !options.isEmpty {
                activeTouch.alternatePanel = self.createAlternatePanel(
                    for: activeTouch.key,
                    options: options,
                    currentX: activeTouch.currentTouchPoint.x
                )
                self.activeTouches[identifier] = activeTouch
            } else if self.isRepeatableKey(activeTouch.key.spec) {
                self.activeTouches[identifier] = activeTouch
                self.startRepeating(identifier: identifier)
            } else {
                self.activeTouches[identifier] = activeTouch
                let command = self.resolveLongPressCommand(activeTouch.key.spec)
                self.rememberRecentEmoji(command)
                self.delegate?.keyboardView(self, didTrigger: command)
                self.clearOneShotShift(after: command)
            }
            self.performConfiguredHaptic(strong: true, playSound: false)
            self.setNeedsDisplay()
        }
        touch.longPressWorkItem = workItem
        activeTouches[identifier] = touch
        DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(config.longPressDelayMs), execute: workItem)
    }

    private func scheduleBackspaceRepeat(for identifier: ObjectIdentifier) {
        guard var touch = activeTouches[identifier] else {
            return
        }
        let profile = backspaceRepeatProfile()
        let workItem = DispatchWorkItem { [weak self] in
            guard let self, var activeTouch = self.activeTouches[identifier], activeTouch.longPressWorkItem != nil else {
                return
            }
            activeTouch.longPressWorkItem = nil
            self.activeTouches[identifier] = activeTouch
            self.startRepeating(identifier: identifier)
        }
        touch.longPressWorkItem = workItem
        activeTouches[identifier] = touch
        DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(profile.initialDelayMs), execute: workItem)
    }

    private func stopLongPressAndRepeat(for identifier: ObjectIdentifier) {
        guard var touch = activeTouches[identifier] else {
            return
        }
        touch.longPressWorkItem?.cancel()
        touch.longPressWorkItem = nil
        touch.repeatTimer?.invalidate()
        touch.repeatTimer = nil
        activeTouches[identifier] = touch
    }

    private func keySupportsLongPress(_ key: KeyTaoKeySpec) -> Bool {
        key.longPress != nil ||
            key.asciiLongPress != nil ||
            key.hint?.isEmpty == false ||
            !explicitAlternates(key).isEmpty ||
            isRepeatableKey(key)
    }

    private func isRepeatableKey(_ key: KeyTaoKeySpec) -> Bool {
        let command = actionForMode(key)
        return command.type == KeyTaoCommandType.backspace ||
            (command.type == KeyTaoCommandType.edit && Self.repeatableEditVerbs.contains(command.value ?? ""))
    }

    private func startRepeating(identifier: ObjectIdentifier) {
        guard let touch = activeTouches[identifier] else {
            return
        }
        let repeatingKeyIndex = touch.keyIndex
        dispatchRepeatedKey(identifier: identifier)
        guard var activeTouch = activeTouches[identifier] else {
            return
        }
        activeTouch.repeatTimer?.invalidate()
        let intervalMs = isBackspaceKey(activeTouch.key.spec)
            ? backspaceRepeatProfile().intervalMs
            : KeyTaoIMEInteractionTuning.repeatableEditIntervalMs
        let interval = TimeInterval(intervalMs) / 1000
        let timer = Timer(timeInterval: interval, repeats: true) { [weak self] timer in
            guard let self,
                  let currentTouch = self.activeTouches[identifier],
                  currentTouch.keyIndex == repeatingKeyIndex,
                  !currentTouch.backspaceGestureConsumed else {
                timer.invalidate()
                return
            }
            self.dispatchRepeatedKey(identifier: identifier)
        }
        timer.tolerance = min(
            interval * KeyTaoIMEInteractionTuning.repeatTimerToleranceFraction,
            KeyTaoIMEInteractionTuning.repeatTimerMaximumToleranceSeconds
        )
        RunLoop.main.add(timer, forMode: .common)
        activeTouch.repeatTimer = timer
        activeTouches[identifier] = activeTouch
    }

    private func dispatchRepeatedKey(identifier: ObjectIdentifier) {
        guard let touch = activeTouches[identifier] else {
            return
        }
        let holdDurationMs = Int(
            max(0, ProcessInfo.processInfo.systemUptime - touch.touchStartUptime) * 1000
        )
        let command: KeyTaoKeyCommand
        if isBackspaceKey(touch.key.spec),
           KeyTaoBackspaceRepeatPolicy(profile: backspaceRepeatProfile()).granularity(at: holdDurationMs) == .segment {
            command = backspaceGestureCommand("deleteSegment")
        } else {
            command = resolveCommand(
                touch.key.spec,
                deltaY: 0,
                rect: touch.key.rect,
                releaseY: touch.key.rect.midY
            )
        }
        performConfiguredHaptic()
        delegate?.keyboardView(self, didTrigger: command)
        clearOneShotShift(after: command)
    }

    private func backspaceRepeatProfile() -> KeyTaoBackspaceRepeatProfile {
        KeyTaoIMEInteractionTuning.backspaceProfile(for: KeyTaoDeleteSpeed(setting: config.deleteSpeed))
    }

    private func displayLabel(_ key: KeyTaoKeySpec) -> String {
        if key.action?.type == KeyTaoCommandType.shift {
            return shiftState == .locked ? "⇪" : key.label
        }
        if key.action?.type == KeyTaoCommandType.enter, let label = hostTraits.returnKeyLabel {
            return label
        }
        if key.action?.type == KeyTaoCommandType.space {
            let mode = state.asciiMode ? "En" : "中"
            return "\(state.schemaName.isEmpty ? key.label : state.schemaName) · \(mode)"
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
        return Self.softAccentPunctuation.contains(label) || Self.softAccentPunctuation.contains(value)
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

    private var isEmojiLayer: Bool {
        layerMode.id.hasPrefix("symbols_emoji_")
    }

    private func rememberRecentEmoji(_ command: KeyTaoKeyCommand) {
        guard !hostTraits.isSensitive,
              isEmojiLayer,
              [KeyTaoCommandType.input, KeyTaoCommandType.directInput, KeyTaoCommandType.rimeInput].contains(command.type),
              let emoji = command.value,
              !emoji.isEmpty else {
            return
        }
        recentEmojis = ([emoji] + recentEmojis.filter { $0 != emoji }).prefix(Self.maxRecentEmojiCount).map { $0 }
        emojiPreferences.set(recentEmojis, forKey: Self.recentEmojiPreferenceKey)
        invalidateLayoutAndDisplay()
    }

    private func loadRecentEmojis() -> [String] {
        let values = emojiPreferences.stringArray(forKey: Self.recentEmojiPreferenceKey) ?? []
        var seen = Set<String>()
        return values.filter { !$0.isEmpty && seen.insert($0).inserted }.prefix(Self.maxRecentEmojiCount).map { $0 }
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
        config.maxKeyHeightDp * config.keyboardHeightScaleFactor
    }

    private func candidateTextSize() -> CGFloat {
        let desired = max(13, min(theme.font.size - 2, 22)) * config.candidateFontScale
        return max(10, min(desired, candidateFontHeightLimit()))
    }

    private func candidateLabelSize() -> CGFloat {
        let desired = max(10, min(theme.font.labelSize - 1, 16)) * config.candidateFontScale
        return max(9, min(desired, candidateFontHeightLimit() * 0.76))
    }

    private func candidateCommentSize() -> CGFloat {
        let desired = max(10, min(theme.font.commentSize - 1, 14)) * config.candidateFontScale
        return max(9, min(desired, candidateFontHeightLimit() * 0.68))
    }

    private func candidateFontHeightLimit() -> CGFloat {
        let inlineHeight = max(24, min(38, config.candidateBarHeightDp - theme.panel.gap * 1.8))
        return inlineHeight * 0.72
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

    private func keyHintSize(keyHeight: CGFloat) -> CGFloat {
        let base = max(10, min(theme.font.commentSize - 1, keyLabelSize(for: "中") - 2, 13))
        return max(10, min(14, base * max(0.9, min(1.2, keyHeight / 54))))
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
            return softenedAccentSurfaceColor(KeyTaoIMEInteractionTuning.softAccentKeyFillAmount)
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

    private func keyForegroundColor(
        _ key: KeyTaoKeySpec,
        selected: Bool,
        pressProgress: CGFloat = 0
    ) -> UIColor {
        let normal = selected || key.style == "accent"
            ? theme.candidate.selectedForeground.uiColor
            : theme.candidate.foreground.uiColor
        guard pressProgress > 0 else { return normal }
        return blend(
            foreground: theme.candidate.pressedForeground.uiColor,
            background: normal,
            amount: pressProgress
        )
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

    private func accentBorderColor(_ alpha: CGFloat) -> UIColor {
        let effectiveAlpha = alpha * (isDarkPanel()
            ? KeyTaoIMEInteractionTuning.darkAccentBorderAlphaMultiplier
            : 1)
        return theme.candidate.selectedLabelColor.uiColor.withAlphaComponent(
            max(0, min(effectiveAlpha, 1))
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

    /// Sound remains owned by iOS' Keyboard Clicks setting. Haptics additionally
    /// respect the KeyTao switch and Full Access, which UIKit requires for a
    /// third-party keyboard's feedback generators.
    private func performConfiguredHaptic(strong: Bool = false, playSound: Bool = true) {
        if playSound {
            UIDevice.current.playInputClick()
        }
        guard configuredHapticsAreAvailable() else {
            return
        }
        let intensity = min(1, max(0.15, CGFloat(config.hapticIntensity) / 100))
        let generator = strong ? mediumHapticGenerator : lightHapticGenerator
        generator.impactOccurred(intensity: intensity)
        generator.prepare()
    }

    private func performPressFeedback() {
        performConfiguredHaptic()
    }

    private func performMediumFeedback(playSound: Bool = true) {
        performConfiguredHaptic(strong: true, playSound: playSound)
    }

    private func performSelectionFeedback(playSound: Bool = true) {
        if playSound {
            UIDevice.current.playInputClick()
        }
        guard configuredHapticsAreAvailable() else {
            return
        }
        selectionHapticGenerator.selectionChanged()
        selectionHapticGenerator.prepare()
    }

    private func performWarningFeedback() {
        guard configuredHapticsAreAvailable() else {
            return
        }
        notificationHapticGenerator.notificationOccurred(.warning)
        notificationHapticGenerator.prepare()
    }

    private func configuredHapticsAreAvailable() -> Bool {
        guard config.hapticsEnabled else {
            return false
        }
        guard hapticsAvailable else {
            showHapticsAccessMessageIfNeeded()
            return false
        }
        return true
    }

    private func showHapticsAccessMessageIfNeeded() {
        guard config.hapticsEnabled, !hapticsAvailable, !hapticsAccessMessageShown else {
            return
        }
        hapticsAccessMessageShown = true
        delegate?.keyboardViewNeedsFullAccessForHaptics(self)
    }

    private func isConfirmationCommand(_ command: KeyTaoKeyCommand) -> Bool {
        [
            KeyTaoCommandType.shift,
            KeyTaoCommandType.mode,
            KeyTaoCommandType.openPage,
            KeyTaoCommandType.keyboardPicker,
            KeyTaoCommandType.nextInputMethod,
            KeyTaoCommandType.keyboardMode,
            KeyTaoCommandType.nextCandidatePage,
            KeyTaoCommandType.previousCandidatePage,
            KeyTaoCommandType.reset,
            KeyTaoCommandType.rimeMenu,
            KeyTaoCommandType.rimeSchema,
            KeyTaoCommandType.rimeOption,
            KeyTaoCommandType.panel,
            KeyTaoCommandType.floating,
        ].contains(command.type)
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
    private static let keyPreviewMargin: CGFloat = 4
    private static let keyPreviewMinimumWidth: CGFloat = 48
    private static let keyPreviewMinimumHeight: CGFloat = 48
    private static let keyPreviewMaximumHeight: CGFloat = 64
    private static let keyPreviewKeyOverlap: CGFloat = 6
    private static let keyPreviewTextSize: CGFloat = 28
    private static let alternatePanelMargin: CGFloat = 4
    private static let alternatePanelGap: CGFloat = 6
    private static let alternatePanelMinimumItemWidth: CGFloat = 40
    private static let alternatePanelMinimumHeight: CGFloat = 44
    private static let alternatePanelMaximumHeight: CGFloat = 56
    private static let alternatePanelTextSize: CGFloat = 20
    private static let expandedCandidateLoadDelayMs = 180
    private static let backspacePreviewDuration: TimeInterval = 2
    private static let maxBackspaceGestureUnitsPerGesture = 96
    private static let emojiRecentLayer = "symbols_emoji_face"
    private static let toolbarPinnedBoundaryID = "__toolbar_pinned_boundary__"
    private static let recentEmojiPreferenceKey = "recent_emoji"
    private static let maxRecentEmojiCount = 32
    private static let softAccentPunctuation: Set<String> = ["，", "。", ",", "."]
    private static let keyPressAnimationDuration: TimeInterval = 0.08
    private static let scrollRubberBandFactor: CGFloat = 0.28
    private static let scrollOverscrollDistance: CGFloat = 18
    private static let minimumScrollVelocity: CGFloat = 120
    private static let maximumScrollVelocity: CGFloat = 8_000
    private static let scrollStopVelocity: CGFloat = 8
    private static let scrollFrictionPerFrame = 0.92
    private static let scrollBoundaryDamping = 0.78
    private static let scrollSpringStrength: CGFloat = 180
    private static let scrollIndicatorHoldDuration: TimeInterval = 0.32
    private static let scrollIndicatorFadeDuration: TimeInterval = 0.22
    private static let scrollIndicatorMinimumThumb: CGFloat = 18
    private static let scrollIndicatorWidth: CGFloat = 2.5
    private static let scrollIndicatorMaximumAlpha: CGFloat = 0.7
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
