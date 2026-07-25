import UIKit
import os

private let rimeKeySpace: UInt32 = 0x0020
private let rimeKeyBackspace: UInt32 = 0xff08
private let rimeKeyReturn: UInt32 = 0xff0d
private let rimeKeyEscape: UInt32 = 0xff1b
private let rimeKeyF4: UInt32 = 0xffc1
private let keyTaoKeyboardLog = Logger(subsystem: "ink.rea.keytao-app.keyboard", category: "Keyboard")

open class KeyTaoKeyboardViewController: UIInputViewController, KeyTaoIOSKeyboardViewDelegate {
    private static let clipboardHistoryLimit = 24
    private static let expandedCandidateLimit = 96

    private let engine = KeyTaoIOSEngine()
    private let candidateQueue = DispatchQueue(label: "ink.rea.keytao-app.keyboard.candidates", qos: .userInitiated)
    private let layoutStateStore = KeyTaoIOSKeyboardLayoutStateStore()
    private var keyboardView: KeyTaoIOSKeyboardView?
    private var keyboardContainer: KeyTaoResizableKeyboardContainerView?
    private var layoutSideButton: UIButton?
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
    private var inputAvailable = false
    private var unavailableMessage = "请先在 KeyTao App 安装键道方案"
    private var clipboardHistory: [String] = []
    private var backspaceRestoreStack: [String] = []

    public override init(nibName nibNameOrNil: String?, bundle nibBundleOrNil: Bundle?) {
        keyTaoKeyboardLog.info("KeyTao keyboard init")
        super.init(nibName: nibNameOrNil, bundle: nibBundleOrNil)
    }

    public required init?(coder: NSCoder) {
        keyTaoKeyboardLog.info("KeyTao keyboard init coder")
        super.init(coder: coder)
    }

    public override func loadView() {
        let inputView = UIInputView(frame: .zero, inputViewStyle: .keyboard)
        inputView.allowsSelfSizing = true
        self.inputView = inputView
        self.view = inputView
    }

    public override func viewDidLoad() {
        super.viewDidLoad()
        keyTaoKeyboardLog.info("KeyTao keyboard viewDidLoad")

        let systemColorScheme = currentSystemColorScheme()
        let theme = engine.resolveTheme(systemColorScheme: systemColorScheme)
        let config = engine.loadConfig(systemColorScheme: systemColorScheme)
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
        view.delegate = self
        view.translatesAutoresizingMaskIntoConstraints = false
        self.view.backgroundColor = .clear
        self.view.isOpaque = false
        applyInterfaceStyle(for: theme)
        view.updateInputModeSwitchKey(visible: needsInputModeSwitchKey)
        self.view.addSubview(container)
        self.view.addSubview(sideButton)
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
        keyboardView = view
        applyKeyboardPresentation(config: config, force: true)
        refreshInputAvailability()
    }

    public override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        reloadIfNeeded()
        let systemColorScheme = currentSystemColorScheme()
        keyboardView?.updateInputModeSwitchKey(visible: needsInputModeSwitchKey)
        let config = engine.loadConfig(systemColorScheme: systemColorScheme)
        applyKeyboardPresentation(config: config, force: true)
        updateThemeForCurrentAppearance(systemColorScheme: systemColorScheme)
        refreshInputAvailability()
        keyboardView?.update(state: currentState)
    }

    public override func textDidChange(_ textInput: UITextInput?) {
        super.textDidChange(textInput)
        reloadIfNeeded()
        keyboardView?.updateInputModeSwitchKey(visible: needsInputModeSwitchKey)
        updateThemeForCurrentAppearance()
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

    public override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        applyKeyboardPresentationIfOrientationChanged()
        updateKeyboardPositionConstraints()
        updateLayoutSideButton()
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
        if currentState.hasComposition {
            apply(engine.reset())
        }
    }

    public override func selectionWillChange(_ textInput: UITextInput?) {
        super.selectionWillChange(textInput)
        if currentState.hasComposition {
            apply(engine.reset())
        }
    }

    func keyboardView(_ view: KeyTaoIOSKeyboardView, didTrigger command: KeyTaoKeyCommand) {
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
            handleBackspaceGesture(command.value.orEmpty)
        case KeyTaoCommandType.enter:
            handleEnter()
        case KeyTaoCommandType.space:
            handleSpace()
        case KeyTaoCommandType.shift:
            keyboardView?.toggleShift()
        case KeyTaoCommandType.mode:
            handleMode(command.value)
        case KeyTaoCommandType.keyboardPicker:
            advanceToNextInputMode()
        case KeyTaoCommandType.keyboardMode:
            keyboardView?.setLayer(command.value)
        case KeyTaoCommandType.nextCandidatePage:
            apply(engine.changePage(backward: false))
        case KeyTaoCommandType.previousCandidatePage:
            apply(engine.changePage(backward: true))
        case KeyTaoCommandType.reset:
            apply(engine.reset())
        case KeyTaoCommandType.rimeMenu:
            apply(engine.processKey(rimeKeyF4))
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
        apply(global ? engine.selectCandidateGlobal(index) : engine.selectCandidate(index))
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

    private func handleTextInput(_ text: String, fallbackValue: String?) {
        guard !text.isEmpty else {
            return
        }
        let fallbackText = fallbackValue ?? text
        if currentState.asciiMode {
            commitDirect(fallbackText)
            return
        }
        guard let key = rimeKey(from: text) else {
            commitDirect(fallbackText)
            return
        }
        let result = engine.processKey(key)
        if !result.accepted && !result.hasComposition {
            commitDirect(fallbackText)
        } else {
            apply(result)
        }
    }

    private func handleRimeInput(_ sequence: String, fallbackValue: String?) {
        guard !sequence.isEmpty else {
            return
        }
        let fallbackText = fallbackValue ?? sequence
        if currentState.asciiMode {
            commitDirect(fallbackText)
            return
        }

        var latest: KeyTaoImeState?
        for scalar in sequence.unicodeScalars {
            guard scalar.value >= 0x20 && scalar.value < 0x7f else {
                _ = engine.reset()
                commitDirect(fallbackText)
                return
            }
            let result = engine.processKey(scalar.value)
            if !result.accepted && !result.hasComposition {
                _ = engine.reset()
                commitDirect(fallbackText)
                return
            }
            latest = result
        }
        if let latest {
            apply(latest)
        }
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
    }

    private func handleBackspaceGesture(_ action: String) {
        switch action {
        case "delete":
            _ = deleteOneBeforeCursorForRestore()
        case "restore":
            _ = restoreOneBackspaceText()
        case "deleteAll":
            deleteAllBeforeCursorForRestore()
        case "restoreAll":
            restoreAllBackspaceText()
        default:
            break
        }
    }

    private func deleteOneBeforeCursorForRestore() -> Bool {
        resetCompositionBeforeBackspaceGesture()
        guard let before = textDocumentProxy.documentContextBeforeInput,
              let deleted = before.last else {
            textDocumentProxy.deleteBackward()
            return false
        }
        textDocumentProxy.deleteBackward()
        backspaceRestoreStack.append(String(deleted))
        return true
    }

    private func deleteAllBeforeCursorForRestore() {
        resetCompositionBeforeBackspaceGesture()
        guard let before = textDocumentProxy.documentContextBeforeInput, !before.isEmpty else {
            return
        }
        for _ in before {
            textDocumentProxy.deleteBackward()
        }
        backspaceRestoreStack.append(contentsOf: before.reversed().map { String($0) })
    }

    private func restoreOneBackspaceText() -> Bool {
        guard let text = backspaceRestoreStack.popLast() else {
            return false
        }
        textDocumentProxy.insertText(text)
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
        textDocumentProxy.insertText(restored)
    }

    private func resetCompositionBeforeBackspaceGesture() {
        guard currentState.hasComposition else {
            return
        }
        _ = engine.reset()
        currentState = engine.nativeReady ? engine.state().withoutTransientCommit() : currentState.withoutTransientCommit()
        keyboardView?.update(state: currentState)
    }

    private func handleEnter() {
        if currentState.hasComposition {
            apply(engine.processKey(rimeKeyReturn))
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
        textDocumentProxy.insertText("\n")
    }

    private func performSystemEnter() {
        // Custom keyboards expose text insertion only; the host control interprets Return semantics.
        textDocumentProxy.insertText("\n")
    }

    private func handleSpace() {
        if currentState.hasComposition {
            apply(engine.processKey(rimeKeySpace))
        } else {
            commitDirect(" ")
        }
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
        case "copy", "cut", "selectAll", "toggleSelection", "selectLeft", "selectRight":
            showMessage("当前输入框不支持此编辑操作")
        default:
            break
        }
    }

    private func pasteClipboard() {
        clearCompositionBeforeEdit()
        guard let text = currentClipboardText() else {
            showMessage("剪贴板为空")
            return
        }
        commitDirect(text)
    }

    private func moveToLineBoundary(start: Bool) {
        clearCompositionBeforeEdit()
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

    private func clearCompositionBeforeEdit() {
        if !currentState.hasComposition {
            return
        }
        _ = engine.reset()
        currentState = engine.nativeReady ? engine.state().withoutTransientCommit() : currentState.withoutTransientCommit()
        keyboardView?.update(state: currentState)
    }

    private func currentClipboardText() -> String? {
        UIPasteboard.general.string?.takeIfNotEmpty
    }

    private func rememberCurrentClipboard() {
        guard let text = currentClipboardText() else {
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
        backspaceRestoreStack.removeAll()
        if currentState.hasComposition {
            _ = engine.reset()
        }
        textDocumentProxy.insertText(text)
        currentState = engine.nativeReady ? engine.state().withoutTransientCommit() : currentState.withoutTransientCommit()
        keyboardView?.update(state: currentState)
    }

    private func apply(_ state: KeyTaoImeState) {
        if !state.committed.isEmpty {
            backspaceRestoreStack.removeAll()
            textDocumentProxy.insertText(state.committed)
        }
        currentState = state.withoutTransientCommit()
        keyboardView?.update(state: currentState)
    }

    private func refreshInputAvailability() {
        let message: String
        let installed = engine.hasInstalledSchema()
        let deployed = engine.hasDeployedSchema()
        let ready = installed && deployed && engine.ensureReady()
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
        currentState = engine.state().withoutTransientCommit()
        let systemColorScheme = currentSystemColorScheme()
        updateThemeForCurrentAppearance(systemColorScheme: systemColorScheme)
        applyKeyboardPresentation(
            config: engine.loadConfig(systemColorScheme: systemColorScheme),
            force: true
        )
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
           keyboardView.currentConfig() == config.scaledForFloating(profile) {
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
        let presentedConfig = config.scaledForFloating(profile)
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
            equalToConstant: presentedConfig.keyboardHeightDp + presentedConfig.candidateBarHeightDp
        )
        keyboardLeadingConstraint = container.leadingAnchor.constraint(
            equalTo: self.view.leadingAnchor
        )
        keyboardTopConstraint = container.topAnchor.constraint(
            equalTo: self.view.topAnchor
        )
        let margin = presentation.isCompact ? config.floating.marginDp : 0
        heightConstraint = self.view.heightAnchor.constraint(
            equalToConstant: presentedConfig.keyboardHeightDp
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
        layoutState = normalized
        if scaleChanged {
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
            side: layoutState.side
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
            side: layoutState.side.opposite
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
            scale: state.scale
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

    private func currentInterfaceIsLandscape() -> Bool {
        if let orientation = view.window?.windowScene?.interfaceOrientation,
           orientation != .unknown {
            return orientation.isLandscape
        }
        if traitCollection.verticalSizeClass == .compact {
            return true
        }
        let screenBounds = UIScreen.main.bounds
        return screenBounds.width > screenBounds.height
    }

    private func currentSystemColorScheme() -> KeyTaoEffectiveColorScheme {
        if let simulatorScheme = simulatorSystemColorScheme() {
            return simulatorScheme
        }

        let systemStyles: [UIUserInterfaceStyle?] = [
            view.window?.windowScene?.traitCollection.userInterfaceStyle,
            UIScreen.main.traitCollection.userInterfaceStyle,
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

        switch textDocumentProxy.keyboardAppearance {
        case .dark, .alert:
            return .dark
        default:
            return .light
        }
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

    private func rimeKey(from text: String) -> UInt32? {
        let scalars = Array(text.unicodeScalars)
        guard scalars.count == 1 else {
            return nil
        }
        let value = scalars[0].value
        guard value >= 0x20 && value < 0x7f else {
            return nil
        }
        return value
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
