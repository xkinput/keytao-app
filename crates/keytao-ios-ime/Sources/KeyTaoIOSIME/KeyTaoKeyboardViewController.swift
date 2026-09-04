import UIKit
import os

private let rimeKeySpace: UInt32 = 0x0020
private let rimeKeyBackspace: UInt32 = 0xff08
private let keyTaoKeyboardLog = Logger(subsystem: "ink.rea.keytao-app.keyboard", category: "Keyboard")
private let keyTaoKeyboardTraitsLog = OSLog(subsystem: "ink.rea.keytao-app.keyboard", category: "Keyboard")

/// `UIInputView` subclass that opts the keyboard into the standard system click
/// sound; whether it is audible is the user's "Keyboard Clicks" preference.
private final class KeyTaoInputView: UIInputView, UIInputViewAudioFeedback {
    var enableInputClicksWhenVisible: Bool { true }
}

open class KeyTaoKeyboardViewController: UIInputViewController, KeyTaoIOSKeyboardViewDelegate {
    private static let clipboardHistoryLimit = 24
    private static let expandedCandidateLimit = 96
    private static let queuedCommandLimit = 32
    private static let deleteAllBatchLimit = 4096
    private static let rimeOptionChoicePrefix = "choice:"
    private static let englishCompletionLexicon = [
        "about", "after", "again", "because", "before", "between", "could", "different",
        "example", "first", "from", "good", "great", "have", "hello", "help", "important",
        "keyboard", "know", "language", "little", "make", "more", "people", "please", "right",
        "should", "something", "thanks", "their", "there", "these", "thing", "think", "through",
        "today", "under", "want", "where", "which", "with", "work", "would", "your",
    ]

    private let engine = KeyTaoIOSEngine()
    private let candidateQueue = DispatchQueue(label: "ink.rea.keytao-app.keyboard.candidates", qos: .userInitiated)
    private let layoutStateStore = KeyTaoIOSKeyboardLayoutStateStore()
    private var keyboardView: KeyTaoIOSKeyboardView?
    private var keyboardContainer: KeyTaoResizableKeyboardContainerView?
    private var layoutSideButton: UIButton?
    private var inputModeSwitchButton: UIButton?
    private var heightConstraint: NSLayoutConstraint?
    private var keyboardWidthConstraint: NSLayoutConstraint?
    private var keyboardHeightConstraint: NSLayoutConstraint?
    private var keyboardLeadingConstraint: NSLayoutConstraint?
    private var keyboardTopConstraint: NSLayoutConstraint?
    private var baseKeyboardConfig: KeyTaoIOSImeConfig?
    private var layoutState = KeyTaoIOSKeyboardLayoutState(enabled: false, scale: 0.88)
    private var layoutPresentation = KeyTaoIOSKeyboardLayoutPresentation(
        mode: .full,
        alternativeMode: .oneHanded,
        side: .right
    )
    private var currentTheme: KeyTaoImeTheme = .fallback
    private var lastPresentationLandscape: Bool?
    private var currentState = KeyTaoImeState.empty
    private var bypassAsciiActive = false
    private var asciiModeBeforeBypass = false
    private var inputAvailable = false
    private var unavailableMessage = "请先在 KeyTao App 安装键道方案"
    private var clipboardHistory: [String] = []
    private var clipboardSuppression: (text: String, changeCount: Int)?
    private var backspaceRestoreStack: [String] = [] {
        didSet { keyboardView?.setNeedsDisplay() }
    }
    private var backspaceGestureRestoreStart = 0
    private struct BackspaceSelectionSession {
        var originalBefore: String
        var selectedText: String
    }
    private var backspaceSelectionSession: BackspaceSelectionSession?
    private var lastCommittedText: String?
    private let doubleSpacePeriodTracker = KeyTaoDoubleSpacePeriodTracker()
    private let englishTextChecker = UITextChecker()
    private var hostTraits = KeyTaoHostTraits.default
    private var markedTextActive = false
    private var hostTextMutationDepth = 0
    private var runtimeStarting = false
    private var queuedCommands: [KeyTaoKeyCommand] = []
    /// Starts at the ABI-level answer, which is already available while the
    /// runtime boots, then follows the live session's own mask.
    private var engineCapabilities = KeyTaoEngineCapabilities.current

    public override init(nibName nibNameOrNil: String?, bundle nibBundleOrNil: Bundle?) {
        keyTaoKeyboardLog.info("KeyTao keyboard init")
        super.init(nibName: nibNameOrNil, bundle: nibBundleOrNil)
    }

    public required init?(coder: NSCoder) {
        keyTaoKeyboardLog.info("KeyTao keyboard init coder")
        super.init(coder: coder)
    }

    public override func loadView() {
        let inputView = KeyTaoInputView(frame: .zero, inputViewStyle: .keyboard)
        inputView.allowsSelfSizing = true
        self.inputView = inputView
        self.view = inputView
    }

    public override func viewDidLoad() {
        super.viewDidLoad()
        keyTaoKeyboardLog.info("KeyTao keyboard viewDidLoad")

        let systemColorScheme = currentSystemColorScheme()
        let theme = engine.resolveTheme(systemColorScheme: systemColorScheme)
        let config = engine.loadConfig()
        let view = KeyTaoIOSKeyboardView(
            config: config,
            theme: theme,
            state: currentState
        )
        let container = KeyTaoResizableKeyboardContainerView(frame: .zero)
        container.backgroundColor = .clear
        container.translatesAutoresizingMaskIntoConstraints = false
        container.onStateChanged = { [weak self] state, finished in
            self?.handleLayoutStateChanged(state, finished: finished)
        }
        let sideButton = UIButton(type: .system)
        sideButton.isHidden = true
        sideButton.accessibilityIdentifier = "keytao-layout-side-toggle"
        sideButton.addTarget(self, action: #selector(switchOneHandedSide), for: .touchUpInside)
        // Transparent overlay on top of the drawn globe key. Binding
        // handleInputModeList(from:with:) to .allTouchEvents is Apple's
        // documented way to get "tap advances, long press opens the picker".
        let switchButton = UIButton(type: .custom)
        switchButton.isHidden = true
        switchButton.backgroundColor = .clear
        switchButton.accessibilityIdentifier = "keytao-key-keyboardpicker"
        switchButton.accessibilityLabel = "切换键盘"
        switchButton.addTarget(self, action: #selector(handleInputModeList(from:with:)), for: .allTouchEvents)
        view.delegate = self
        view.translatesAutoresizingMaskIntoConstraints = false
        self.view.backgroundColor = .clear
        self.view.isOpaque = false
        applyInterfaceStyle(for: theme)
        view.update(engineCapabilities: engineCapabilities)
        view.updateInputModeSwitchKey(visible: needsInputModeSwitchKey)
        self.view.addSubview(container)
        self.view.addSubview(sideButton)
        self.view.addSubview(switchButton)
        container.addSubview(view)
        NSLayoutConstraint.activate([
            view.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            view.trailingAnchor.constraint(equalTo: container.trailingAnchor),
            view.topAnchor.constraint(equalTo: container.topAnchor),
            view.bottomAnchor.constraint(equalTo: container.bottomAnchor),
        ])
        currentTheme = theme
        keyboardContainer = container
        layoutSideButton = sideButton
        inputModeSwitchButton = switchButton
        keyboardView = view
        view.update(hapticsAvailable: hasFullAccess)
        applyKeyboardPresentation(config: config, force: true)
        applyHostTraits()
        startRuntimeIfNeeded()
    }

    public override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        // The keyboard is coming back for a (possibly different) input context:
        // start from a clean composition instead of resurrecting whatever the
        // librime session still held from the previous one.
        resetInputContextCaches()
        reloadIfNeeded()
        let systemColorScheme = currentSystemColorScheme()
        keyboardView?.update(hapticsAvailable: hasFullAccess)
        keyboardView?.updateInputModeSwitchKey(visible: needsInputModeSwitchKey)
        let config = engine.loadConfig()
        applyKeyboardPresentation(config: config, force: true)
        updateThemeForCurrentAppearance(systemColorScheme: systemColorScheme)
        applyHostTraits()
        startRuntimeIfNeeded()
        keyboardView?.update(state: currentState)
        refreshPredictionSuggestions()
    }

    public override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        // Equivalent of macOS deactivateServer / Android onFinishInput: the
        // input context is over, so nothing may survive into the next one.
        resetInputContextCaches()
    }

    public override func textDidChange(_ textInput: UITextInput?) {
        super.textDidChange(textInput)
        guard hostTextMutationDepth == 0 else {
            return
        }
        reloadIfNeeded()
        keyboardView?.updateInputModeSwitchKey(visible: needsInputModeSwitchKey)
        applyHostTraits()
        updateThemeForCurrentAppearance()
    }

    public override func didReceiveMemoryWarning() {
        super.didReceiveMemoryWarning()
        clipboardHistory.removeAll()
        clipboardSuppression = nil
        queuedCommands.removeAll()
        engine.releaseCaches()
        keyboardView?.releaseCaches()
    }

    public override func traitCollectionDidChange(_ previousTraitCollection: UITraitCollection?) {
        super.traitCollectionDidChange(previousTraitCollection)
        if previousTraitCollection?.userInterfaceStyle != traitCollection.userInterfaceStyle {
            updateThemeForCurrentAppearance()
        }
        applyKeyboardPresentationIfOrientationChanged()
    }

    public override func viewWillTransition(
        to size: CGSize,
        with coordinator: UIViewControllerTransitionCoordinator
    ) {
        super.viewWillTransition(to: size, with: coordinator)
        coordinator.animate(alongsideTransition: nil) { [weak self] _ in
            self?.applyKeyboardPresentationIfOrientationChanged(force: true)
        }
    }

    public override func viewWillLayoutSubviews() {
        super.viewWillLayoutSubviews()
        keyboardView?.updateInputModeSwitchKey(visible: needsInputModeSwitchKey)
    }

    public override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        applyKeyboardPresentationIfOrientationChanged()
        updateKeyboardPositionConstraints()
        updateLayoutSideButton()
        updateInputModeSwitchButton()
        guard let container = keyboardContainer,
              container.layer.shadowOpacity > 0 else {
            return
        }
        container.layer.shadowPath = UIBezierPath(
            roundedRect: container.bounds,
            cornerRadius: currentTheme.panel.cornerRadius
        ).cgPath
    }

    public override func textWillChange(_ textInput: UITextInput?) {
        super.textWillChange(textInput)
        guard hostTextMutationDepth == 0 else {
            return
        }
        resetInputContextCaches()
    }

    public override func selectionWillChange(_ textInput: UITextInput?) {
        super.selectionWillChange(textInput)
        guard hostTextMutationDepth == 0 else {
            return
        }
        resetInputContextCaches()
    }

    /// Everything bound to "the text field the user is in right now": the Rime
    /// composition, the host marked text, the backspace restore stack and the
    /// clipboard snapshot. Called whenever the input context ends or changes.
    private func resetInputContextCaches() {
        cancelBackspaceSelection()
        backspaceRestoreStack.removeAll()
        doubleSpacePeriodTracker.reset()
        lastCommittedText = nil
        clipboardHistory.removeAll()
        clipboardSuppression = nil
        clearHostMarkedText()
        keyboardView?.resetShiftForNewContext()
        guard engine.nativeReady else {
            currentState = currentState.withoutTransientCommit()
            keyboardView?.update(state: currentState)
            return
        }
        currentState = engine.reset().withoutTransientCommit()
        keyboardView?.update(state: currentState)
    }

    func keyboardView(_ view: KeyTaoIOSKeyboardView, didTrigger command: KeyTaoKeyCommand) {
        // The runtime is still coming up on the startup queue. Queue the key
        // instead of committing it raw, otherwise the first few characters of
        // every cold start would land in the host as latin letters.
        if runtimeStarting && command.requiresInstalledSchema {
            if queuedCommands.count < Self.queuedCommandLimit {
                queuedCommands.append(command)
            }
            return
        }
        if !inputAvailable && command.requiresInstalledSchema {
            if commitFallbackInput(for: command) {
                return
            }
            showUnavailableMessage()
            return
        }

        switch command.type {
        case KeyTaoCommandType.input:
            handleTextInput(command.value.orEmpty, fallbackValue: command.fallbackValue)
        case KeyTaoCommandType.directInput:
            commitDirect(command.value.orEmpty)
        case KeyTaoCommandType.rimeInput:
            handleRimeInput(command.value.orEmpty, fallbackValue: command.fallbackValue)
        case KeyTaoCommandType.backspace:
            handleBackspace()
        case KeyTaoCommandType.backspaceGesture:
            handleBackspaceGesture(
                command.value.orEmpty,
                count: Int(command.fallbackValue ?? "") ?? 1
            )
        case KeyTaoCommandType.enter:
            handleEnter()
        case KeyTaoCommandType.space:
            handleSpace()
        case KeyTaoCommandType.shift:
            keyboardView?.toggleShift()
        case KeyTaoCommandType.mode:
            handleMode(command.value)
        case KeyTaoCommandType.keyboardPicker:
            // Reached only when the transparent handleInputModeList overlay is
            // not in place (expanded panel, custom layout position).
            advanceToNextInputMode()
        case KeyTaoCommandType.nextInputMethod:
            advanceToNextInputMode()
        case KeyTaoCommandType.keyboardMode:
            keyboardView?.setLayer(command.value)
        case KeyTaoCommandType.nextCandidatePage:
            changeCandidatePage(backward: false)
        case KeyTaoCommandType.previousCandidatePage:
            changeCandidatePage(backward: true)
        case KeyTaoCommandType.reset:
            apply(engine.reset())
        case KeyTaoCommandType.rimeMenu:
            refreshRimeOptions()
        case KeyTaoCommandType.rimeSchema:
            selectRimeSchema(command.value)
        case KeyTaoCommandType.rimeOption:
            setRimeOption(command.value, action: command.fallbackValue)
        case KeyTaoCommandType.openPage:
            openContainingApp(page: command.value)
        case KeyTaoCommandType.edit:
            handleEditAction(command.value.orEmpty, value: command.fallbackValue)
        case KeyTaoCommandType.floating:
            toggleKeyboardLayout()
        case KeyTaoCommandType.panel:
            break
        default:
            break
        }
    }

    func keyboardViewNeedsFullAccessForHaptics(_ view: KeyTaoIOSKeyboardView) {
        showMessage("请在系统设置中允许 KeyTao 完全访问后使用按键振动")
    }

    /// The layout drops paging keys when the runtime cannot turn a page
    /// natively, but swipes, long presses and key stacks can carry the same
    /// command, so the capability is checked here as well: without
    /// `RimeChangePage` the common layer synthesizes `-`/`=`, which a schema
    /// without the default paging bindings types into the composition (D4).
    private func changeCandidatePage(backward: Bool) {
        guard engineCapabilities.nativePaging else {
            showMessage("当前 RIME 运行库不支持候选翻页")
            return
        }
        apply(engine.changePage(backward: backward))
    }

    private func commitFallbackInput(for command: KeyTaoKeyCommand) -> Bool {
        switch command.type {
        case KeyTaoCommandType.input, KeyTaoCommandType.rimeInput:
            let text = command.fallbackValue ?? command.value.orEmpty
            guard !text.isEmpty else {
                return false
            }
            commitDirect(text)
            return true
        default:
            return false
        }
    }

    func keyboardView(_ view: KeyTaoIOSKeyboardView, didSelectCandidate index: Int, global: Bool) {
        if !inputAvailable {
            showUnavailableMessage()
            return
        }
        // Both entry points exist in the vendored librime 1.8.5; the guard keeps
        // a future runtime without them from falling back to sending a select
        // key, which lands in the composition when the schema has none (D4).
        let supported = global
            ? engineCapabilities.globalCandidateSelection
            : engineCapabilities.candidateSelection
        guard supported else {
            showMessage("当前 RIME 运行库不支持点击选词")
            return
        }
        apply(global ? engine.selectCandidateGlobal(index) : engine.selectCandidate(index))
    }

    func keyboardView(_ view: KeyTaoIOSKeyboardView, candidateIsUserPhrase index: Int) -> Bool {
        inputAvailable && engineCapabilities.candidateDeletion && engine.candidateIsUserPhrase(index)
    }

    func keyboardView(_ view: KeyTaoIOSKeyboardView, deleteCandidate index: Int) -> Bool {
        guard inputAvailable, engineCapabilities.candidateDeletion else {
            showMessage("系统词不可删除")
            return false
        }
        let result = engine.deleteCandidate(index)
        apply(result.state)
        showMessage(result.deleted ? "已删除用户词" : "系统词不可删除")
        return result.deleted
    }

    func keyboardView(_ view: KeyTaoIOSKeyboardView, requestExpandedCandidates completion: @escaping ([KeyTaoCandidate]) -> Void) {
        if !inputAvailable {
            showUnavailableMessage()
            completion([])
            return
        }
        candidateQueue.async { [weak self] in
            let candidates = self?.engine.allCandidates(limit: Self.expandedCandidateLimit) ?? []
            DispatchQueue.main.async {
                completion(candidates)
            }
        }
    }

    func keyboardView(_ view: KeyTaoIOSKeyboardView, requestClipboardHistory completion: @escaping ([String]) -> Void) {
        rememberCurrentClipboard()
        completion(clipboardHistory)
    }

    func keyboardView(_ view: KeyTaoIOSKeyboardView, deleteClipboardEntry text: String) {
        guard !hostTraits.isSensitive, hasFullAccess else {
            return
        }
        // History is de-duplicated on insert, so text identity is stable while
        // the asynchronously rendered panel index may already be stale.
        clipboardHistory.removeAll { $0 == text }
        if currentClipboardText() == text {
            clipboardSuppression = (text: text, changeCount: UIPasteboard.general.changeCount)
        }
    }

    func keyboardViewClearClipboardHistory(_ view: KeyTaoIOSKeyboardView) {
        guard !hostTraits.isSensitive, hasFullAccess else {
            return
        }
        clipboardHistory.removeAll()
        clipboardSuppression = currentClipboardText().map {
            (text: $0, changeCount: UIPasteboard.general.changeCount)
        }
    }

    func keyboardView(_ view: KeyTaoIOSKeyboardView, persistToolbarOrder order: [String], pinnedCount: Int) {
        if !engine.persistToolbarCustomization(order: order, pinnedCount: pinnedCount) {
            showMessage("工具栏顺序保存失败")
        }
    }

    func keyboardViewCanUndo(_ view: KeyTaoIOSKeyboardView) -> Bool {
        !backspaceRestoreStack.isEmpty
    }

    /// Every mode goes through Rime first (D6). English mode is a librime
    /// option, not a keyboard-side shortcut: with `ascii_mode` on the schema's
    /// punctuation, `ascii_composer` and `key_binder` rules still apply, and
    /// librime rejects whatever it does not want so the host path below runs.
    private func handleTextInput(_ text: String, fallbackValue: String?) {
        guard !text.isEmpty else {
            return
        }
        let fallbackText = fallbackValue ?? text
        // Secure and non-composing hosts (D9) are the only bypass: the session
        // policy already refuses to compose there, so do not even build a key.
        if hostTraits.bypassesRime {
            commitDirect(fallbackText)
            return
        }
        guard let key = KeyTaoIOSEngine.keysym(for: text) else {
            commitDirect(fallbackText)
            return
        }
        let result = engine.processKey(key)
        if result.accepted {
            apply(result)
            return
        }
        // The common layer maps every printable character to a keysym, so Rime
        // gets a say even for full-width punctuation. When it declines, the
        // character still has to reach the host or it would be dropped.
        applyRejectedKeyFallback(result, text: fallbackText)
    }

    private func handleRimeInput(_ sequence: String, fallbackValue: String?) {
        guard !sequence.isEmpty else {
            return
        }
        let fallbackText = fallbackValue ?? sequence
        if hostTraits.bypassesRime {
            commitDirect(fallbackText)
            return
        }

        // The key falls back as a whole only while nothing has been sent yet:
        // once a prefix is inside Rime, replaying the declared fallback text
        // would duplicate whatever that prefix already committed.
        var keys: [UInt32] = []
        keys.reserveCapacity(sequence.count)
        for character in sequence {
            guard let key = KeyTaoIOSEngine.keysym(for: String(character)) else {
                commitDirect(fallbackText)
                return
            }
            keys.append(key)
        }

        // Each keystroke of the sequence is applied before the next one is sent:
        // a mid-sequence auto-commit has to reach the host right away, otherwise
        // the following state would overwrite it and the text would be lost.
        var consumed = 0
        for key in keys {
            let result = engine.processKey(key)
            if result.accepted {
                apply(result)
                consumed += 1
                continue
            }
            // Rime declined this keystroke: apply what it still returned, then
            // hand the host only the part of the sequence Rime never consumed.
            let rejectedText = consumed == 0
                ? fallbackText
                : String(sequence.dropFirst(consumed))
            applyRejectedKeyFallback(result, text: rejectedText)
            return
        }
    }

    private func applyRejectedKeyFallback(_ result: KeyTaoImeState, text: String) {
        apply(result)
        if currentState.hasComposition {
            apply(engine.commitComposition())
        }
        insertHostText(text)
        refreshPredictionSuggestions()
    }

    private func handleBackspace() {
        if currentState.hasComposition {
            let result = engine.processKey(rimeKeyBackspace)
            if result.accepted || result.hasComposition {
                apply(result)
            } else {
                apply(engine.reset())
            }
            return
        }
        _ = deleteOneBeforeCursorForRestore()
        refreshPredictionSuggestions()
    }

    private func handleBackspaceGesture(_ action: String, count: Int) {
        switch action {
        case "begin":
            backspaceGestureRestoreStart = backspaceRestoreStack.count
        case "delete":
            _ = deleteOneBeforeCursorForRestore()
        case "deleteSegment":
            deleteTrailingSegmentBeforeCursorForRestore()
        case "restore":
            _ = restoreOneBackspaceText()
        case "deleteAll":
            deleteAllBeforeCursorForRestore()
            backspaceGestureRestoreStart = 0
        case "restoreAll":
            restoreAllBackspaceText()
            backspaceGestureRestoreStart = 0
        case "restoreGesture":
            let count = max(0, backspaceRestoreStack.count - backspaceGestureRestoreStart)
            for _ in 0..<count {
                _ = restoreOneBackspaceText()
            }
        case "select":
            updateBackspaceSelection(count: count)
        case "cancelSelection":
            cancelBackspaceSelection()
        case "commitSelection":
            commitBackspaceSelection()
        default:
            break
        }
        let preview = hostTraits.isSensitive
            ? ""
            : backspaceRestoreStack[
                min(backspaceGestureRestoreStart, backspaceRestoreStack.count)...
            ].reversed().joined()
        if !["select", "cancelSelection", "commitSelection"].contains(action) {
            keyboardView?.showBackspaceDeletionPreview(preview)
        }
        refreshPredictionSuggestions()
    }

    private func updateBackspaceSelection(count: Int) {
        guard !hostTraits.isSensitive else { return }
        resetCompositionBeforeBackspaceGesture()
        let originalBefore = backspaceSelectionSession?.originalBefore
            ?? textDocumentProxy.documentContextBeforeInput
            ?? ""
        restoreBackspaceSelectionMarkup()
        let selectionLength = keyTaoTrailingDeletionSegmentsLength(
            originalBefore,
            count: max(1, min(count, 96))
        )
        let selectedText = String(originalBefore.suffix(selectionLength))
        guard !selectedText.isEmpty else {
            backspaceSelectionSession = nil
            keyboardView?.showBackspaceDeletionPreview("")
            return
        }
        withHostTextMutation {
            for _ in selectedText {
                textDocumentProxy.deleteBackward()
            }
            textDocumentProxy.setMarkedText(
                selectedText,
                selectedRange: NSRange(location: 0, length: selectedText.utf16.count)
            )
        }
        markedTextActive = true
        backspaceSelectionSession = BackspaceSelectionSession(
            originalBefore: originalBefore,
            selectedText: selectedText
        )
        keyboardView?.showBackspaceDeletionPreview(selectedText, pendingSelection: true)
    }

    private func restoreBackspaceSelectionMarkup() {
        guard let session = backspaceSelectionSession else { return }
        withHostTextMutation {
            textDocumentProxy.insertText(session.selectedText)
            textDocumentProxy.unmarkText()
        }
        markedTextActive = false
        backspaceSelectionSession = nil
    }

    private func cancelBackspaceSelection() {
        restoreBackspaceSelectionMarkup()
        keyboardView?.showBackspaceDeletionPreview("")
    }

    private func commitBackspaceSelection() {
        guard let session = backspaceSelectionSession else { return }
        let selectedText = session.selectedText
        backspaceSelectionSession = nil
        backspaceRestoreStack.removeAll()
        backspaceGestureRestoreStart = 0
        clearHostMarkedText()
        backspaceRestoreStack.append(contentsOf: selectedText.reversed().map(String.init))
        keyboardView?.showBackspaceDeletionPreview(selectedText)
    }

    private func deleteTrailingSegmentBeforeCursorForRestore() {
        let before = textDocumentProxy.documentContextBeforeInput ?? ""
        let count = keyTaoTrailingDeletionSegmentLength(before)
        for _ in 0..<count {
            guard deleteOneBeforeCursorForRestore() else {
                break
            }
        }
    }

    private func deleteOneBeforeCursorForRestore() -> Bool {
        resetCompositionBeforeBackspaceGesture()
        return withHostTextMutation {
            guard let before = textDocumentProxy.documentContextBeforeInput,
                  let deleted = before.last else {
                textDocumentProxy.deleteBackward()
                return false
            }
            textDocumentProxy.deleteBackward()
            backspaceRestoreStack.append(String(deleted))
            return true
        }
    }

    /// `documentContextBeforeInput` only returns the slice the host chose to
    /// hand over, so "delete everything before the caret" has to re-read it
    /// after every batch until the host stops giving text back.
    private func deleteAllBeforeCursorForRestore() {
        resetCompositionBeforeBackspaceGesture()
        withHostTextMutation {
            var deleted = 0
            while deleted < Self.deleteAllBatchLimit {
                guard let before = textDocumentProxy.documentContextBeforeInput, !before.isEmpty else {
                    return
                }
                for _ in before {
                    textDocumentProxy.deleteBackward()
                    deleted += 1
                    if deleted >= Self.deleteAllBatchLimit {
                        break
                    }
                }
                backspaceRestoreStack.append(contentsOf: before.reversed().map { String($0) })
            }
        }
    }

    private func restoreOneBackspaceText() -> Bool {
        guard let text = backspaceRestoreStack.popLast() else {
            return false
        }
        clearHostMarkedText()
        withHostTextMutation {
            textDocumentProxy.insertText(text)
        }
        return true
    }

    private func restoreAllBackspaceText() {
        guard !backspaceRestoreStack.isEmpty else {
            return
        }
        var restored = ""
        while let text = backspaceRestoreStack.popLast() {
            restored.append(text)
        }
        clearHostMarkedText()
        withHostTextMutation {
            textDocumentProxy.insertText(restored)
        }
    }

    private func resetCompositionBeforeBackspaceGesture() {
        guard currentState.hasComposition else {
            return
        }
        _ = engine.reset()
        clearHostMarkedText()
        currentState = engine.nativeReady ? engine.state().withoutTransientCommit() : currentState.withoutTransientCommit()
        keyboardView?.update(state: currentState)
        refreshPredictionSuggestions()
    }

    private func handleEnter() {
        if currentState.hasComposition {
            // Single cross-platform Enter semantics: hand XK_Return to Rime and
            // let the common layer decide, never commit the preedit literally.
            apply(engine.processEnter())
            return
        }
        let behavior = keyboardView?.currentConfig().enterKeyBehavior ?? KeyTaoEnterKeyBehavior.system
        if behavior == KeyTaoEnterKeyBehavior.newline {
            commitLineBreak()
        } else {
            performSystemEnter()
        }
    }

    private func commitLineBreak() {
        insertHostText("\n")
    }

    private func performSystemEnter() {
        // Custom keyboards expose text insertion only; the host control interprets Return semantics.
        insertHostText("\n")
    }

    /// Space is a key like any other (D6): a schema may bind it in `key_binder`
    /// and `full_shape` turns it into U+3000, so Rime decides even when there is
    /// no composition to select from.
    private func handleSpace() {
        let config = keyboardView?.currentConfig() ?? .fallback
        let contextBefore = textDocumentProxy.documentContextBeforeInput ?? ""
        let replace = doubleSpacePeriodTracker.shouldReplaceSpace(
            nowMs: Int(ProcessInfo.processInfo.systemUptime * 1_000),
            contextBefore: contextBefore,
            enabled: config.doubleSpacePeriodEnabled,
            hasComposition: currentState.hasComposition
        )
        if replace {
            _ = deleteOneBeforeCursorForRestore()
            commitDirect(currentState.asciiMode ? ". " : "。")
            return
        }
        if hostTraits.bypassesRime {
            commitDirect(" ")
            return
        }
        let result = engine.processKey(rimeKeySpace)
        if result.accepted {
            apply(result)
            return
        }
        applyRejectedKeyFallback(result, text: " ")
    }

    private func handleMode(_ value: String?) {
        let target: Bool
        switch value {
        case "ascii", "english", "en":
            target = true
        case "chinese", "zh", "cn":
            target = false
        default:
            target = !currentState.asciiMode
        }
        apply(engine.setAsciiMode(target))
    }

    private func selectRimeSchema(_ schemaID: String?) {
        guard let schemaID, !schemaID.isEmpty else {
            return
        }
        guard let state = engine.selectSchema(schemaID) else {
            showMessage("无法切换输入方案")
            refreshRimeOptions()
            return
        }
        apply(state)
        refreshRimeOptions()
    }

    private func setRimeOption(_ optionName: String?, action: String?) {
        guard let optionName, !optionName.isEmpty, let action else {
            return
        }
        let state: KeyTaoImeState?
        if action == "true" || action == "false" {
            let enabled = action == "true"
            state = optionName == "ascii_mode"
                ? engine.setAsciiMode(enabled)
                : engine.setOption(optionName, enabled: enabled)
        } else if action.hasPrefix(Self.rimeOptionChoicePrefix) {
            let previous = String(action.dropFirst(Self.rimeOptionChoicePrefix.count))
            if !previous.isEmpty, previous != optionName,
               engine.setOption(previous, enabled: false) == nil {
                showMessage("无法更新 Rime 选项")
                return
            }
            state = engine.setOption(optionName, enabled: true)
        } else {
            return
        }
        guard let state else {
            showMessage("无法更新 Rime 选项")
            return
        }
        apply(state)
        refreshRimeOptions()
    }

    private func refreshRimeOptions() {
        var switches = engine.schemaSwitches()
        if !switches.contains(where: { $0.name == "ascii_punct" }) {
            switches.append(KeyTaoRimeSchemaSwitch(
                name: "ascii_punct",
                options: [],
                states: ["中文标点", "英文标点"],
                reset: nil
            ))
        }
        var optionStates: [String: Bool] = [:]
        for optionName in switches.flatMap(\.optionNames) {
            optionStates[optionName] = engine.getOption(optionName)
        }
        keyboardView?.update(rimeOptions: KeyTaoRimeOptionsState(
            schemas: engine.listSchemas(),
            currentSchema: engine.currentSchema(),
            switches: switches,
            options: optionStates
        ))
    }

    private func handleEditAction(_ action: String, value: String?) {
        switch action {
        case "paste":
            pasteClipboard()
        case "pasteText":
            if let text = value?.takeIfNotEmpty {
                commitDirect(text)
            }
        case "tab":
            commitDirect("\t")
        case "lineStart":
            moveToLineBoundary(start: true)
        case "lineEnd":
            moveToLineBoundary(start: false)
        case "cursorLeft":
            moveCursor(byCharacterOffset: -1)
        case "cursorRight":
            moveCursor(byCharacterOffset: 1)
        case "cursorUp":
            moveCursor(lineOffset: -1)
        case "cursorDown":
            moveCursor(lineOffset: 1)
        case "undo":
            clearCompositionBeforeEdit()
            _ = restoreOneBackspaceText()
        case "deleteWord":
            deleteTrailingSegmentBeforeCursorForRestore()
        case "insertPair":
            if let pair = value?.takeIfNotEmpty {
                commitDirect(pair)
                moveCursor(byCharacterOffset: -1)
            }
        case "redo", "clearAll":
            break
        case "forwardDelete":
            forwardDelete()
        case "copy":
            copySelection(cut: false)
        case "cut":
            copySelection(cut: true)
        case "selectAll", "toggleSelection", "selectLeft", "selectRight":
            break
        case "repeatCommit":
            if let lastCommittedText {
                commitDirect(lastCommittedText)
            }
        default:
            break
        }
    }

    private func copySelection(cut: Bool) {
        clearCompositionBeforeEdit()
        guard !hostTraits.isSensitive else {
            return
        }
        guard hasFullAccess else {
            showMessage("请在系统设置中允许 KeyTao 完全访问后使用剪贴板")
            return
        }
        guard let selectedText = textDocumentProxy.selectedText, !selectedText.isEmpty else {
            return
        }
        UIPasteboard.general.string = selectedText
        if cut {
            withHostTextMutation {
                textDocumentProxy.deleteBackward()
            }
        }
    }

    private func pasteClipboard() {
        clearCompositionBeforeEdit()
        guard hasFullAccess else {
            showMessage("请在系统设置中允许 KeyTao 完全访问后使用剪贴板")
            return
        }
        guard let text = currentClipboardText() else {
            showMessage("剪贴板为空")
            return
        }
        commitDirect(text)
    }

    private func moveToLineBoundary(start: Bool) {
        clearCompositionBeforeEdit()
        withHostTextMutation {
            if start {
                let before = textDocumentProxy.documentContextBeforeInput ?? ""
                let offset = -(before.split(separator: "\n", omittingEmptySubsequences: false).last?.count ?? before.count)
                if offset != 0 {
                    textDocumentProxy.adjustTextPosition(byCharacterOffset: offset)
                }
            } else {
                let after = textDocumentProxy.documentContextAfterInput ?? ""
                let offset = after.split(separator: "\n", omittingEmptySubsequences: false).first?.count ?? after.count
                if offset != 0 {
                    textDocumentProxy.adjustTextPosition(byCharacterOffset: offset)
                }
            }
        }
    }

    private func moveCursor(byCharacterOffset offset: Int) {
        clearCompositionBeforeEdit()
        withHostTextMutation {
            textDocumentProxy.adjustTextPosition(byCharacterOffset: offset)
        }
    }

    /// UIKit exposes only logical text around the caret, not wrapped visual
    /// lines, so vertical movement is best-effort within the host-provided slice.
    private func moveCursor(lineOffset: Int) {
        guard lineOffset == -1 || lineOffset == 1 else {
            return
        }
        clearCompositionBeforeEdit()
        withHostTextMutation {
            let before = textDocumentProxy.documentContextBeforeInput ?? ""
            let after = textDocumentProxy.documentContextAfterInput ?? ""
            let beforeLines = before.split(separator: "\n", omittingEmptySubsequences: false)
            let currentColumn = beforeLines.last?.utf16.count ?? 0
            let offset: Int
            if lineOffset < 0 {
                guard beforeLines.count > 1 else {
                    return
                }
                let previousLineLength = beforeLines[beforeLines.count - 2].utf16.count
                offset = -currentColumn - 1 - previousLineLength + min(currentColumn, previousLineLength)
            } else {
                let afterLines = after.split(separator: "\n", omittingEmptySubsequences: false)
                guard afterLines.count > 1 else {
                    return
                }
                let currentLineRemainder = afterLines[0].utf16.count
                let nextLineLength = afterLines[1].utf16.count
                offset = currentLineRemainder + 1 + min(currentColumn, nextLineLength)
            }
            if offset != 0 {
                textDocumentProxy.adjustTextPosition(byCharacterOffset: offset)
            }
        }
    }

    private func forwardDelete() {
        clearCompositionBeforeEdit()
        guard textDocumentProxy.documentContextAfterInput?.isEmpty == false else {
            return
        }
        withHostTextMutation {
            textDocumentProxy.adjustTextPosition(byCharacterOffset: 1)
            textDocumentProxy.deleteBackward()
        }
    }

    private func clearCompositionBeforeEdit() {
        if !currentState.hasComposition {
            return
        }
        _ = engine.reset()
        clearHostMarkedText()
        currentState = engine.nativeReady ? engine.state().withoutTransientCommit() : currentState.withoutTransientCommit()
        keyboardView?.update(state: currentState)
        refreshPredictionSuggestions()
    }

    /// Reading `UIPasteboard.general.string` needs Full Access, and since iOS 16
    /// it also raises the system paste prompt. `hasStrings` is metadata only, so
    /// it lets an empty clipboard be reported without bothering the user.
    private func currentClipboardText() -> String? {
        guard hasFullAccess, UIPasteboard.general.hasStrings else {
            return nil
        }
        return UIPasteboard.general.string?.takeIfNotEmpty
    }

    private func rememberCurrentClipboard() {
        guard !hostTraits.isSensitive else {
            return
        }
        let text = currentClipboardText()
        if let clipboardSuppression {
            guard text != clipboardSuppression.text
                    || UIPasteboard.general.changeCount != clipboardSuppression.changeCount else {
                return
            }
            self.clipboardSuppression = nil
        }
        guard let text else {
            return
        }
        clipboardHistory.removeAll { $0 == text }
        clipboardHistory.insert(text, at: 0)
        if clipboardHistory.count > Self.clipboardHistoryLimit {
            clipboardHistory.removeLast(clipboardHistory.count - Self.clipboardHistoryLimit)
        }
    }

    private func commitDirect(_ text: String) {
        guard !text.isEmpty else {
            return
        }
        if currentState.hasComposition {
            _ = engine.reset()
        }
        insertHostText(text)
        lastCommittedText = hostTraits.isSensitive ? nil : text
        currentState = engine.nativeReady ? engine.state().withoutTransientCommit() : currentState.withoutTransientCommit()
        keyboardView?.update(state: currentState)
        refreshPredictionSuggestions()
    }

    private func apply(_ state: KeyTaoImeState) {
        if !state.committed.isEmpty {
            insertHostText(state.committed)
        }
        currentState = state.withoutTransientCommit()
        updateHostMarkedText()
        keyboardView?.update(state: currentState)
        refreshPredictionSuggestions()
    }

    private func refreshPredictionSuggestions() {
        let config = keyboardView?.currentConfig() ?? baseKeyboardConfig ?? .fallback
        guard config.predictionEnabled,
              !hostTraits.isSensitive,
              currentState.asciiMode,
              !currentState.hasComposition,
              backspaceSelectionSession == nil,
              let before = textDocumentProxy.documentContextBeforeInput,
              let prefix = keyTaoEnglishCompletionPrefix(before) else {
            keyboardView?.updatePredictionSuggestions(prefix: nil, suggestions: [])
            return
        }
        let range = NSRange(location: 0, length: prefix.utf16.count)
        let systemCompletions = englishTextChecker.completions(
            forPartialWordRange: range,
            in: prefix,
            language: "en_US"
        ) ?? []
        let systemGuesses = englishTextChecker.guesses(
            forWordRange: range,
            in: prefix,
            language: "en_US"
        ) ?? []
        let suggestions = keyTaoCompleteEnglishPrefix(
            prefix,
            lexicon: systemCompletions + systemGuesses + Self.englishCompletionLexicon
        )
        keyboardView?.updatePredictionSuggestions(prefix: prefix, suggestions: suggestions)
    }

    // MARK: - Host text

    private var hostMarkedTextEnabled: Bool {
        baseKeyboardConfig?.hostMarkedTextEnabled ?? true
    }

    /// Every write into the host goes through here so that our own
    /// textWillChange / selectionWillChange callbacks do not look like the user
    /// moving to a different field. The guard is released on the next main-queue
    /// turn because the proxy notifies back asynchronously for some hosts.
    @discardableResult
    private func withHostTextMutation<Result>(_ body: () -> Result) -> Result {
        hostTextMutationDepth += 1
        let result = body()
        DispatchQueue.main.async { [weak self] in
            guard let self, self.hostTextMutationDepth > 0 else {
                return
            }
            self.hostTextMutationDepth -= 1
        }
        return result
    }

    private func insertHostText(_ text: String) {
        guard !text.isEmpty else {
            return
        }
        backspaceRestoreStack.removeAll()
        clearHostMarkedText()
        withHostTextMutation {
            textDocumentProxy.insertText(text)
        }
    }

    /// iOS 13+ lets a keyboard extension show the pending composition inside the
    /// host field via marked text, which is the standard CJK channel: the host
    /// suppresses autocorrect over it and groups undo around it. When host marked
    /// text is disabled, the candidate bar renders the preedit instead.
    private func updateHostMarkedText() {
        let preedit = currentState.candidatePanel.preedit ?? currentState.preedit
        guard hostMarkedTextEnabled, !preedit.isEmpty else {
            clearHostMarkedText()
            return
        }
        let selected = markedTextSelectedRange(in: preedit)
        withHostTextMutation {
            textDocumentProxy.setMarkedText(preedit, selectedRange: selected)
        }
        markedTextActive = true
    }

    /// `unmarkText()` only finalises the marked text, it does not remove it, so
    /// the composition has to be replaced with an empty string first. Otherwise
    /// the preedit would stay in the document next to the text Rime commits.
    private func clearHostMarkedText() {
        guard markedTextActive else {
            return
        }
        markedTextActive = false
        withHostTextMutation {
            textDocumentProxy.setMarkedText("", selectedRange: NSRange(location: 0, length: 0))
            textDocumentProxy.unmarkText()
        }
    }

    /// `ImeState` offsets are Unicode scalars (D8); UIKit wants UTF-16 units.
    private func markedTextSelectedRange(in preedit: String) -> NSRange {
        if currentState.selEnd > currentState.selStart {
            let start = KeyTaoIOSEngine.utf16Offset(in: preedit, charOffset: currentState.selStart)
            let end = KeyTaoIOSEngine.utf16Offset(in: preedit, charOffset: currentState.selEnd)
            if end > start, end <= preedit.utf16.count {
                return NSRange(location: start, length: end - start)
            }
        }
        let caret = KeyTaoIOSEngine.utf16Offset(in: preedit, charOffset: currentState.cursor)
        return NSRange(location: min(caret, preedit.utf16.count), length: 0)
    }

    /// `keytao_init` plus the librime session handshake are the only slow part
    /// of a cold keyboard start, so they run off the main thread and the UI
    /// shows a loading hint until they land.
    private func startRuntimeIfNeeded() {
        if engine.nativeReady {
            refreshInputAvailability()
            return
        }
        guard !runtimeStarting else {
            return
        }
        runtimeStarting = true
        keyboardView?.updateAvailability(message: "正在加载键道方案…")
        engine.ensureReadyAsync { [weak self] _ in
            guard let self else {
                return
            }
            self.runtimeStarting = false
            self.refreshInputAvailability()
            self.applyInputPolicyForHost()
            self.flushQueuedCommands()
        }
    }

    private func flushQueuedCommands() {
        guard !queuedCommands.isEmpty else {
            return
        }
        let pending = queuedCommands
        queuedCommands.removeAll()
        guard let keyboardView else {
            return
        }
        for command in pending {
            self.keyboardView(keyboardView, didTrigger: command)
        }
    }

    /// Re-reads what the session behind the engine can do and hands it to the
    /// view. Runs on every availability refresh, which covers the cold start,
    /// every `viewWillAppear` and every reload, so a session rebuilt against a
    /// different librime cannot leave the UI offering controls it lost.
    private func refreshEngineCapabilities() {
        let capabilities = engine.capabilities
        guard capabilities != engineCapabilities else {
            return
        }
        engineCapabilities = capabilities
        keyboardView?.update(engineCapabilities: capabilities)
    }

    private func refreshInputAvailability() {
        refreshEngineCapabilities()
        let message: String
        let installed = engine.hasInstalledSchema()
        let deployed = engine.hasDeployedSchema()
        let ready = installed && deployed && engine.nativeReady
        if ready {
            message = ""
        } else if !installed && !hasFullAccess {
            message = "请在系统设置中允许 KeyTao 完全访问"
        } else if !installed {
            message = "请先在 KeyTao App 安装键道方案"
        } else if !deployed {
            message = "请先在 KeyTao App 部署方案"
        } else {
            message = "RIME 运行库未就绪，请重新安装 KeyTao"
        }

        inputAvailable = message.isEmpty
        unavailableMessage = message.isEmpty ? "请先在 KeyTao App 安装键道方案" : message
        if inputAvailable {
            currentState = engine.state().withoutTransientCommit()
            keyboardView?.updateAvailability(message: nil)
        } else {
            currentState = .empty
            keyboardView?.updateAvailability(message: unavailableMessage)
        }
        keyboardView?.update(state: currentState)
    }

    private func reloadIfNeeded() {
        guard engine.reloadIfNeeded() else {
            return
        }
        refreshEngineCapabilities()
        currentState = engine.state().withoutTransientCommit()
        let systemColorScheme = currentSystemColorScheme()
        updateThemeForCurrentAppearance(systemColorScheme: systemColorScheme)
        applyKeyboardPresentation(
            config: engine.loadConfig(),
            force: true
        )
    }

    // MARK: - Host traits

    /// `textDocumentProxy` inherits `UITextInputTraits`; it is the only channel
    /// through which an iOS host declares what it expects from the keyboard.
        private func applyHostTraits() {
        let traits = KeyTaoHostTraits(proxy: textDocumentProxy)
        defer {
            os_log(
                "host traits kb=%{public}d secure=%{public}d bypass=%{public}d",
                log: keyTaoKeyboardTraitsLog,
                type: .info,
                traits.keyboardType.rawValue,
                traits.isSecureTextEntry ? 1 : 0,
                traits.bypassesRime ? 1 : 0
            )
        }
        guard traits != hostTraits else {
            return
        }
        let previouslySensitive = hostTraits.isSensitive
        hostTraits = traits
        if traits.isSensitive && !previouslySensitive {
            clipboardHistory.removeAll()
            clipboardSuppression = nil
            backspaceRestoreStack.removeAll()
            lastCommittedText = nil
        }
        applyInputPolicyForHost()
        keyboardView?.update(hostTraits: traits)
        if let layer = traits.forcedLayer {
            keyboardView?.setLayer(layer)
        }
    }

    /// Secure or non-composing host fields must never reach librime: no
    /// preedit, no candidates, and nothing that could be learnt into the user
    /// dictionary (D9).
    private func applyInputPolicyForHost() {
        guard engine.nativeReady else {
            return
        }
        let bypass = hostTraits.bypassesRime
        let composing = !bypass
        let state = engine.setInputPolicy(composing: composing, learning: composing)
        if !composing {
            clearHostMarkedText()
        }
        currentState = state.withoutTransientCommit()
        if bypass && !bypassAsciiActive {
            bypassAsciiActive = true
            asciiModeBeforeBypass = currentState.asciiMode
            currentState = engine.setAsciiMode(true).withoutTransientCommit()
        } else if !bypass && bypassAsciiActive {
            bypassAsciiActive = false
            currentState = engine.setAsciiMode(asciiModeBeforeBypass).withoutTransientCommit()
        }
        keyboardView?.update(state: currentState)
    }

    private func updateThemeForCurrentAppearance(systemColorScheme: KeyTaoEffectiveColorScheme? = nil) {
        let theme = engine.resolveTheme(systemColorScheme: systemColorScheme ?? currentSystemColorScheme())
        currentTheme = theme
        applyInterfaceStyle(for: theme)
        keyboardView?.update(theme: theme)
        updateLayoutAppearance()
        updateLayoutSideButton()
    }

    private func applyInterfaceStyle(for theme: KeyTaoImeTheme) {
        let style: UIUserInterfaceStyle = theme.ui.effectiveColorScheme == "dark" ? .dark : .light
        self.view.overrideUserInterfaceStyle = style
        keyboardView?.overrideUserInterfaceStyle = style
    }

    private func applyKeyboardPresentationIfOrientationChanged(force: Bool = false) {
        guard let config = baseKeyboardConfig else {
            return
        }
        let isLandscape = currentInterfaceIsLandscape()
        guard force || lastPresentationLandscape != isLandscape else {
            return
        }
        applyKeyboardPresentation(config: config, force: true)
    }

    private func applyKeyboardPresentation(config: KeyTaoIOSImeConfig, force: Bool = false) {
        guard let keyboardView else {
            return
        }
        baseKeyboardConfig = config
        let isLandscape = currentInterfaceIsLandscape()
        let fallbackProfile = config.floating.profile(isLandscape: isLandscape)
        let state = layoutStateStore.load(isLandscape: isLandscape, fallback: fallbackProfile)
        let presentation = keyboardLayoutPresentation(state: state, isLandscape: isLandscape)
        let profile = keyboardLayoutProfile(state: state, presentation: presentation)
        if !force, lastPresentationLandscape == isLandscape,
           layoutState == state,
           layoutPresentation == presentation,
           keyboardView.currentConfig() == config.scaledForFloating(profile, heightScale: state.heightScale) {
            return
        }
        lastPresentationLandscape = isLandscape
        layoutState = state
        layoutPresentation = presentation
        applyKeyboardPresentation(config: config, state: state)
    }

    private func applyKeyboardPresentation(
        config: KeyTaoIOSImeConfig,
        state: KeyTaoIOSKeyboardLayoutState
    ) {
        guard let keyboardView,
              let container = keyboardContainer else {
            return
        }
        let normalized = state.normalized()
        let isLandscape = currentInterfaceIsLandscape()
        let presentation = keyboardLayoutPresentation(state: normalized, isLandscape: isLandscape)
        let profile = keyboardLayoutProfile(state: normalized, presentation: presentation)
        layoutState = normalized
        layoutPresentation = presentation
        let presentedConfig = config.scaledForFloating(profile, heightScale: normalized.heightScale)
        keyboardView.update(config: presentedConfig)
        keyboardView.updateLayoutPresentation(presentation)

        NSLayoutConstraint.deactivate(
            [
                keyboardWidthConstraint,
                keyboardHeightConstraint,
                keyboardLeadingConstraint,
                keyboardTopConstraint,
                heightConstraint,
            ]
                .compactMap { $0 }
        )
        keyboardWidthConstraint = container.widthAnchor.constraint(
            equalTo: self.view.widthAnchor,
            multiplier: presentation.isCompact ? normalized.scale : 1
        )
        keyboardHeightConstraint = container.heightAnchor.constraint(
            equalToConstant: presentedConfig.effectiveKeyboardHeightDp + presentedConfig.candidateBarHeightDp
        )
        keyboardLeadingConstraint = container.leadingAnchor.constraint(
            equalTo: self.view.leadingAnchor
        )
        keyboardTopConstraint = container.topAnchor.constraint(
            equalTo: self.view.topAnchor
        )
        let margin = presentation.isCompact ? config.floating.marginDp : 0
        heightConstraint = self.view.heightAnchor.constraint(
            equalToConstant: presentedConfig.effectiveKeyboardHeightDp
                + presentedConfig.candidateBarHeightDp
                + (presentation.isCompact ? margin * 2 : 0)
        )
        heightConstraint?.priority = .defaultHigh
        NSLayoutConstraint.activate(
            [
                keyboardWidthConstraint,
                keyboardHeightConstraint,
                keyboardLeadingConstraint,
                keyboardTopConstraint,
                heightConstraint,
            ]
                .compactMap { $0 }
        )
        container.configure(state: normalized, presentation: presentation)
        self.view.layoutIfNeeded()
        updateKeyboardPositionConstraints()
        updateLayoutAppearance()
        updateLayoutSideButton()
        updateInputModeSwitchButton()
    }

    private func handleLayoutStateChanged(
        _ state: KeyTaoIOSKeyboardLayoutState,
        finished: Bool
    ) {
        guard layoutPresentation.isCompact,
              let config = baseKeyboardConfig else {
            return
        }
        let normalized = state.normalized()
        let scaleChanged = abs(normalized.scale - layoutState.scale) >= 0.001
        let heightChanged = abs(normalized.heightScale - layoutState.heightScale) >= 0.001
        layoutState = normalized
        if scaleChanged || heightChanged {
            applyKeyboardPresentation(config: config, state: normalized)
        } else {
            keyboardContainer?.configure(
                state: normalized,
                presentation: layoutPresentation
            )
            updateKeyboardPositionConstraints()
            self.view.layoutIfNeeded()
            updateLayoutSideButton()
        }
        if finished {
            layoutStateStore.save(
                isLandscape: currentInterfaceIsLandscape(),
                state: normalized
            )
        }
    }

    private func toggleKeyboardLayout() {
        guard let config = baseKeyboardConfig else {
            return
        }
        let next = KeyTaoIOSKeyboardLayoutState(
            enabled: !layoutState.enabled,
            scale: layoutState.scale,
            heightScale: layoutState.heightScale,
            side: layoutState.side,
            orientation: layoutState.orientation
        ).normalized()
        layoutStateStore.save(
            isLandscape: currentInterfaceIsLandscape(),
            state: next
        )
        applyKeyboardPresentation(config: config, state: next)
    }

    @objc private func switchOneHandedSide() {
        guard layoutPresentation.isCompact,
              let config = baseKeyboardConfig else {
            return
        }
        let next = KeyTaoIOSKeyboardLayoutState(
            enabled: true,
            scale: layoutState.scale,
            heightScale: layoutState.heightScale,
            side: layoutState.side.opposite,
            orientation: layoutState.orientation
        ).normalized()
        layoutStateStore.save(
            isLandscape: currentInterfaceIsLandscape(),
            state: next
        )
        applyKeyboardPresentation(config: config, state: next)
    }

    private func keyboardLayoutPresentation(
        state: KeyTaoIOSKeyboardLayoutState,
        isLandscape: Bool
    ) -> KeyTaoIOSKeyboardLayoutPresentation {
        let alternativeMode: KeyTaoIOSKeyboardLayoutMode = isLandscape ? .split : .oneHanded
        return KeyTaoIOSKeyboardLayoutPresentation(
            mode: state.enabled ? alternativeMode : .full,
            alternativeMode: alternativeMode,
            side: state.side
        )
    }

    private func keyboardLayoutProfile(
        state: KeyTaoIOSKeyboardLayoutState,
        presentation: KeyTaoIOSKeyboardLayoutPresentation
    ) -> KeyTaoFloatingKeyboardProfile {
        KeyTaoFloatingKeyboardProfile(
            enabled: presentation.isEnabled,
            scale: state.scale,
            orientation: state.orientation
        )
    }

    private func updateKeyboardPositionConstraints() {
        guard let config = baseKeyboardConfig,
              let leading = keyboardLeadingConstraint,
              let top = keyboardTopConstraint else {
            return
        }
        guard layoutPresentation.isCompact else {
            leading.constant = 0
            top.constant = 0
            return
        }
        let margin = config.floating.marginDp
        let hostWidth = self.view.bounds.width
        let keyboardWidth = hostWidth * layoutState.scale
        leading.constant = layoutState.side == .left
            ? margin
            : max(margin, hostWidth - margin - keyboardWidth)
        top.constant = margin
    }

    private func updateLayoutSideButton() {
        guard let button = layoutSideButton,
              let container = keyboardContainer,
              layoutPresentation.isCompact else {
            layoutSideButton?.isHidden = true
            return
        }
        let emptyRect: CGRect
        if layoutState.side == .left {
            emptyRect = CGRect(
                x: container.frame.maxX,
                y: 0,
                width: max(0, self.view.bounds.width - container.frame.maxX),
                height: self.view.bounds.height
            )
        } else {
            emptyRect = CGRect(
                x: 0,
                y: 0,
                width: max(0, container.frame.minX),
                height: self.view.bounds.height
            )
        }
        guard emptyRect.width >= 36 else {
            button.isHidden = true
            return
        }
        let size = min(44, emptyRect.width - 8, emptyRect.height - 16)
        button.frame = CGRect(
            x: emptyRect.midX - size / 2,
            y: emptyRect.midY - size / 2,
            width: size,
            height: size
        )
        let destination: KeyTaoIOSKeyboardSide = layoutState.side.opposite
        button.setImage(
            UIImage(systemName: destination == .left ? "chevron.left" : "chevron.right"),
            for: .normal
        )
        button.accessibilityLabel = destination == .left ? "切换到左侧单手键盘" : "切换到右侧单手键盘"
        button.tintColor = currentTheme.candidate.labelColor.uiColor
        button.backgroundColor = currentTheme.panel.background.uiColor.withAlphaComponent(0.82)
        button.layer.cornerRadius = min(8, size * 0.24)
        button.layer.borderWidth = max(0.5, currentTheme.panel.borderWidth)
        button.layer.borderColor = currentTheme.panel.borderColor.uiColor.cgColor
        button.isHidden = false
        self.view.bringSubviewToFront(button)
    }

    private func updateInputModeSwitchButton() {
        guard let button = inputModeSwitchButton else {
            return
        }
        guard needsInputModeSwitchKey,
              let keyboardView,
              let frame = keyboardView.inputModeSwitchKeyFrame() else {
            button.isHidden = true
            return
        }
        button.frame = keyboardView.convert(frame, to: self.view)
        button.isHidden = false
        self.view.bringSubviewToFront(button)
    }

    private func updateLayoutAppearance() {
        guard let keyboardView,
              let container = keyboardContainer else {
            return
        }
        let compact = layoutPresentation.isCompact
        let radius = compact ? currentTheme.panel.cornerRadius : 0
        keyboardView.layer.cornerRadius = radius
        keyboardView.layer.cornerCurve = .continuous
        keyboardView.layer.masksToBounds = compact
        container.layer.shadowColor = UIColor.black.cgColor
        container.layer.shadowOpacity = compact && currentTheme.panel.shadow ? 0.2 : 0
        container.layer.shadowRadius = compact ? 9 : 0
        container.layer.shadowOffset = compact ? CGSize(width: 0, height: 3) : .zero
        container.layer.shadowPath = compact
            ? UIBezierPath(roundedRect: container.bounds, cornerRadius: radius).cgPath
            : nil
    }

    /// Keyboard extensions have no scene of their own, and on iPad the host
    /// window can be a narrow column while the device is landscape, so the
    /// keyboard's own bounds and size classes decide the layout.
    private func currentInterfaceIsLandscape() -> Bool {
        let bounds = view.bounds
        if bounds.width > 0, bounds.height > 0 {
            return bounds.width > bounds.height * 1.2
        }
        if traitCollection.verticalSizeClass == .compact {
            return true
        }
        return traitCollection.horizontalSizeClass == .regular
    }

    private func currentSystemColorScheme() -> KeyTaoEffectiveColorScheme {
        if let simulatorScheme = simulatorSystemColorScheme() {
            return simulatorScheme
        }

        // The host's keyboardAppearance is an explicit request and outranks the
        // device style; a dark UI app on a light device asks for .dark here.
        switch textDocumentProxy.keyboardAppearance {
        case .dark, .alert:
            return .dark
        case .light:
            return .light
        default:
            break
        }

        let systemStyles: [UIUserInterfaceStyle?] = [
            view.window?.windowScene?.traitCollection.userInterfaceStyle,
            UITraitCollection.current.userInterfaceStyle,
            traitCollection.userInterfaceStyle,
            view.traitCollection.userInterfaceStyle,
        ]
        for style in systemStyles {
            switch style {
            case .dark:
                return .dark
            case .light:
                return .light
            default:
                continue
            }
        }

        return .light
    }

    private func simulatorSystemColorScheme() -> KeyTaoEffectiveColorScheme? {
        #if targetEnvironment(simulator)
        let domain = "com.apple.uikitservices.userInterfaceStyleMode"
        let key = "UserInterfaceStyleMode"
        let rawValue = UserDefaults.standard.persistentDomain(forName: domain)?[key]
            ?? UserDefaults(suiteName: domain)?.object(forKey: key)
        switch rawValue {
        case let value as Int:
            return value == 2 ? .dark : value == 1 ? .light : nil
        case let value as NSNumber:
            return value.intValue == 2 ? .dark : value.intValue == 1 ? .light : nil
        default:
            return nil
        }
        #else
        return nil
        #endif
    }

    private func showUnavailableMessage() {
        startRuntimeIfNeeded()
        guard !runtimeStarting else {
            return
        }
        refreshInputAvailability()
        showMessage(unavailableMessage)
    }

    private func showMessage(_ message: String) {
        keyboardView?.updateAvailability(message: message)
    }

    private func openContainingApp(page: String?) {
        let page = page ?? "home"
        guard let url = URL(string: "keytao://\(page)") else {
            showMessage("请打开 KeyTao App")
            return
        }
        extensionContext?.open(url) { [weak self] success in
            if !success {
                self?.showMessage("请打开 KeyTao App")
            }
        }
    }

}

/// The `UITextInputTraits` the host declares on its `textDocumentProxy`.
struct KeyTaoHostTraits: Equatable {
    var returnKeyType: UIReturnKeyType
    var keyboardType: UIKeyboardType
    var autocapitalizationType: UITextAutocapitalizationType
    var isSecureTextEntry: Bool

    static let `default` = KeyTaoHostTraits(
        returnKeyType: .default,
        keyboardType: .default,
        autocapitalizationType: .sentences,
        isSecureTextEntry: false
    )

    init(
        returnKeyType: UIReturnKeyType,
        keyboardType: UIKeyboardType,
        autocapitalizationType: UITextAutocapitalizationType,
        isSecureTextEntry: Bool
    ) {
        self.returnKeyType = returnKeyType
        self.keyboardType = keyboardType
        self.autocapitalizationType = autocapitalizationType
        self.isSecureTextEntry = isSecureTextEntry
    }

    init(proxy: UITextDocumentProxy) {
        self.init(
            returnKeyType: proxy.returnKeyType ?? .default,
            keyboardType: proxy.keyboardType ?? .default,
            autocapitalizationType: proxy.autocapitalizationType ?? .sentences,
            isSecureTextEntry: proxy.isSecureTextEntry ?? false
        )
    }

    /// Password-like fields: nothing may reach Rime, nothing may be cached.
    var isSensitive: Bool {
        isSecureTextEntry
    }

    /// Secure fields and hosts that require digits do not want Rime conversion;
    /// keys go straight to the document. Text-oriented keyboard types are only
    /// presentation hints and still allow Chinese composition.
    var bypassesRime: Bool {
        if isSensitive {
            return true
        }
        switch keyboardType {
        case .numberPad, .decimalPad, .phonePad, .asciiCapableNumberPad:
            return true
        default:
            return false
        }
    }

    /// Layer the host effectively requires, or nil when the user stays in
    /// control of the layer.
    var forcedLayer: String? {
        switch keyboardType {
        case .numberPad, .decimalPad, .phonePad, .asciiCapableNumberPad:
            return KeyTaoKeyboardLayer.numbers.id
        default:
            return nil
        }
    }

    /// Label the Return key must carry, following the host's declared action.
    var returnKeyLabel: String? {
        switch returnKeyType {
        case .go:
            return "前往"
        case .google, .yahoo, .search:
            return "搜索"
        case .join:
            return "加入"
        case .next:
            return "下一项"
        case .route:
            return "路线"
        case .send:
            return "发送"
        case .done:
            return "完成"
        case .emergencyCall:
            return "紧急呼叫"
        case .continue:
            return "继续"
        default:
            return nil
        }
    }

    /// Shift state the host expects at the start of a fresh composition.
    var wantsAutoShift: Bool {
        switch autocapitalizationType {
        case .words, .sentences, .allCharacters:
            return true
        default:
            return false
        }
    }
}

private extension Optional where Wrapped == String {
    var orEmpty: String {
        self ?? ""
    }
}

private extension String {
    var takeIfNotEmpty: String? {
        isEmpty ? nil : self
    }
}

private extension KeyTaoKeyCommand {
    var requiresInstalledSchema: Bool {
        switch type {
        case KeyTaoCommandType.openPage,
            KeyTaoCommandType.backspace,
            KeyTaoCommandType.backspaceGesture,
            KeyTaoCommandType.keyboardPicker,
            KeyTaoCommandType.nextInputMethod,
            KeyTaoCommandType.keyboardMode,
            KeyTaoCommandType.shift,
            KeyTaoCommandType.directInput,
            KeyTaoCommandType.edit,
            KeyTaoCommandType.floating,
            KeyTaoCommandType.panel:
            return false
        default:
            return true
        }
    }
}
