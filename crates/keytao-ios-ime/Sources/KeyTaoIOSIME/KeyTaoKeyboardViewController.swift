import UIKit
import ImageIO
import UniformTypeIdentifiers
import CryptoKit
import CKeytaoCore
import Darwin
import os

private let rimeKeySpace: UInt32 = 0x0020
private let rimeKeyBackspace: UInt32 = 0xff08
private let rimeKeyEscape: UInt32 = 0xff1b
private let keyTaoKeyboardLog = Logger(subsystem: "ink.rea.keytao-app.keyboard", category: "Keyboard")
private let keyTaoKeyboardTraitsLog = OSLog(subsystem: "ink.rea.keytao-app.keyboard", category: "Keyboard")

/// `UIInputView` subclass that opts the keyboard into the standard system click
/// sound; whether it is audible is the user's "Keyboard Clicks" preference.
private final class KeyTaoInputView: UIInputView, UIInputViewAudioFeedback {
    var enableInputClicksWhenVisible: Bool { true }
}

/// Cancels unfinished snapshots when the input context changes.
private final class KeyTaoClipboardMediaLifetime {
    private let lock = NSLock()
    private var generation = 0

    func currentGeneration() -> Int {
        lock.lock()
        defer { lock.unlock() }
        return generation
    }

    func write(_ data: Data, to file: URL, generation expected: Int) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard generation == expected else { return false }
        do {
            try FileManager.default.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
            try data.write(to: file, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
            return true
        } catch {
            try? FileManager.default.removeItem(at: file)
            return false
        }
    }

    func cancel() {
        lock.lock()
        defer { lock.unlock() }
        generation += 1
    }
}

open class KeyTaoKeyboardViewController: UIInputViewController, KeyTaoIOSKeyboardViewDelegate {
    private static let clipboardMediaMaxBytes = 8 * 1024 * 1024
    private static let clipboardMediaTotalBytes = 48 * 1024 * 1024
    private static let clipboardThumbPx = 176
    private static let clipboardMaxPixels = 20_000_000
    private struct ClipboardMedia {
        var id: String
        var file: URL
        var pasteboardType: String
        var mime: String
        var name: String
        var size: Int
        var width: Int
        var height: Int
        var timestamp: TimeInterval
        var thumb: UIImage?

        var entry: ClipboardEntry {
            ClipboardEntry(key: "media:\(id)", mime: mime, name: name, size: size, width: width, height: height, thumb: thumb)
        }
    }
    private static let expandedCandidateLimit = 96
    private static let queuedCommandLimit = 32
    private static let deleteAllBatchLimit = 4096
    private static let rimeOptionChoicePrefix = "choice:"

    private let extensionStarted = ProcessInfo.processInfo.systemUptime
    private var startupLatencyRecorded = false
    private var commandCounts: [String: UInt64] = [:]
    private var applyHistogram = KeyTaoDurationHistogram()
    private var pendingKeystrokeStarted: TimeInterval?
    private var resettingInputContext = false
    private var runtimeMetricsEnabled = true
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
    private var currentThemeCacheKey: String?
    private var rimeOptionsReloadStamp: String?
    private var lastPresentationLandscape: Bool?
    private var currentState = KeyTaoImeState.empty
    private var bypassAsciiActive = false
    private var asciiModeBeforeBypass = false
    private var englishSchemaID: String?
    private var currentRimeSchemaID: String?
    private var lastChineseSchemaID: String?
    private var chineseSwitchSnapshot: [String: Bool] = [:]
    private var inputAvailable = false
    private var unavailableMessage = "请先在 KeyTao App 安装键道方案"
    private var clipboardHistory = KeyTaoClipboardHistory()
    private var clipboardHistoryLoaded = false
    private var clipboardHistoryNeedsSave = false
    private var clipboardSuppression: (text: String, changeCount: Int)?
    private var clipboardMedia: [ClipboardMedia] = []
    private var clipboardLastReadChangeCount: Int?
    private var clipboardObservedChangeCount: Int?
    private var clipboardObservedAt: TimeInterval = 0
    private var clipboardChangeObserver: NSObjectProtocol?
    private var clipboardMediaSuppressionChangeCount: Int?
    private var clipboardMediaCapturePending = false
    private var clipboardMediaCompletions: [([ClipboardEntry]) -> Void] = []
    private let clipboardMediaLifetime = KeyTaoClipboardMediaLifetime()
    private var backspaceRestoreStack: [String] = [] {
        didSet { keyboardView?.setNeedsDisplay() }
    }
    private var backspaceGestureRestoreStart = 0
    private struct BackspaceSelectionSession {
        var documentIdentifier: UUID?
        var originalBefore: String
        var selectedText: String
    }
    private var backspaceSelectionSession: BackspaceSelectionSession?
    private var lastCommittedText: String?
    private let doubleSpacePeriodTracker = KeyTaoDoubleSpacePeriodTracker()
    private var hostTraits = KeyTaoHostTraits.default
    private var inputDocumentIdentifier: UUID?
    private var inputContextGeneration: UInt64 = 0
    private var markedTextOwnership = KeyTaoMarkedTextOwnership()
    private var hostTextMutationDepth = 0
    private var runtimeStarting = false
    private var runtimeReleased = false
    private var runtimeResumeFailed = false
    private var keyboardVisible = false
    private var engineControlWatcher: DispatchSourceFileSystemObject?
    private var memoryReclamation = KeyTaoMemoryReclamationPolicy()
    private var hiddenMemoryReleaseWorkItem: DispatchWorkItem?
    private var queuedCommands: [KeyTaoKeyCommand] = []
    /// Starts at the ABI-level answer, which is already available while the
    /// runtime boots, then follows the live session's own mask.
    private var engineCapabilities = KeyTaoEngineCapabilities.current

    public override init(nibName nibNameOrNil: String?, bundle nibBundleOrNil: Bundle?) {
        keyTaoKeyboardLog.info("KeyTao keyboard init")
        super.init(nibName: nibNameOrNil, bundle: nibBundleOrNil)
        KeyTaoLog.event("lifecycle", "ext_init") { ["t0_uptime": extensionStarted] }
    }

    public required init?(coder: NSCoder) {
        keyTaoKeyboardLog.info("KeyTao keyboard init coder")
        super.init(coder: coder)
        KeyTaoLog.event("lifecycle", "ext_init") { ["t0_uptime": extensionStarted] }
    }

    deinit {
        hiddenMemoryReleaseWorkItem?.cancel()
        engineControlWatcher?.cancel()
        if let clipboardChangeObserver { NotificationCenter.default.removeObserver(clipboardChangeObserver) }
        clipboardMediaLifetime.cancel()
    }

    public override func loadView() {
        let inputView = KeyTaoInputView(frame: .zero, inputViewStyle: .keyboard)
        inputView.allowsSelfSizing = true
        self.inputView = inputView
        self.view = inputView
    }

    public override func viewDidLoad() {
        let started = KeyTaoLog.now()
        defer { KeyTaoLog.event("lifecycle", "view_did_load", duration: (KeyTaoLog.now() - started) * 1000) }
        super.viewDidLoad()
        keyTaoKeyboardLog.info("KeyTao keyboard viewDidLoad")

        let systemColorScheme = currentSystemColorScheme()
        let resolvedTheme = engine.resolveTheme(systemColorScheme: systemColorScheme)
        let theme = resolvedTheme.theme
        let config = engine.loadConfig()
        let view = KeyTaoIOSKeyboardView(
            engine: engine,
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
        currentThemeCacheKey = resolvedTheme.cacheKey
        keyboardContainer = container
        layoutSideButton = sideButton
        inputModeSwitchButton = switchButton
        keyboardView = view
        refreshRuntimeLogCollection()
        view.update(hapticsAvailable: hasFullAccess)
        applyKeyboardPresentation(config: config)
        applyHostTraits()
        startRuntimeIfNeeded()
    }

    public override func viewWillAppear(_ animated: Bool) {
        let started = KeyTaoLog.now()
        defer {
            KeyTaoLog.event("lifecycle", "view_will_appear", duration: (KeyTaoLog.now() - started) * 1000)
            if !startupLatencyRecorded {
                startupLatencyRecorded = true
                KeyTaoLog.event("lifecycle", "startup_latency", duration: (KeyTaoLog.now() - extensionStarted) * 1000)
            }
            KeyTaoLog.memorySnapshot("view_will_appear")
        }
        super.viewWillAppear(animated)
        keyboardVisible = true
        watchAlgorithmControl()
        hiddenMemoryReleaseWorkItem?.cancel()
        hiddenMemoryReleaseWorkItem = nil
        releaseMemoryIfNeeded()
        memoryReclamation.hiddenSince = nil
        let statusDefaults = UserDefaults(suiteName: KeyTaoIOSPaths.appGroupIdentifier)
        statusDefaults?.set(hasFullAccess, forKey: "keyboardFullAccess")
        statusDefaults?.set(Date().timeIntervalSince1970, forKey: "keyboardStatusUpdatedAt")
        refreshRuntimeLogCollection()
        // The keyboard is coming back for a (possibly different) input context:
        // start from a clean composition instead of resurrecting whatever the
        // librime session still held from the previous one.
        resetInputContextCaches(resetNavigation: true)
        reloadIfNeeded()
        let systemColorScheme = currentSystemColorScheme()
        keyboardView?.update(hapticsAvailable: hasFullAccess)
        keyboardView?.updateInputModeSwitchKey(visible: needsInputModeSwitchKey)
        let config = engine.loadConfig()
        applyKeyboardPresentation(config: config)
        updateThemeForCurrentAppearance(systemColorScheme: systemColorScheme)
        applyHostTraits()
        startRuntimeIfNeeded()
        keyboardView?.update(state: currentState)
    }

    public override func viewWillDisappear(_ animated: Bool) {
        let started = KeyTaoLog.now()
        defer {
            KeyTaoLog.event("lifecycle", "view_will_disappear", duration: (KeyTaoLog.now() - started) * 1000)
            KeyTaoLog.memorySnapshot("view_will_disappear")
            flushRuntimeMetrics()
        }
        super.viewWillDisappear(animated)
        keyboardVisible = false
        if let clipboardChangeObserver { NotificationCenter.default.removeObserver(clipboardChangeObserver) }
        clipboardChangeObserver = nil
        keyboardView?.clearActiveTouches()
        // Equivalent of macOS deactivateServer / Android onFinishInput: the
        // input context is over, so nothing may survive into the next one.
        resetInputContextCaches(resetNavigation: true)
    }

    public override func viewDidAppear(_ animated: Bool) {
        let started = KeyTaoLog.now()
        super.viewDidAppear(animated)
        refreshRuntimeLogCollection()
        if clipboardChangeObserver == nil {
            clipboardChangeObserver = NotificationCenter.default.addObserver(
                forName: UIPasteboard.changedNotification, object: nil, queue: .main
            ) { [weak self] _ in
                self?.refreshCurrentClipboardText()
            }
        }
        refreshCurrentClipboardText()
        KeyTaoLog.event("lifecycle", "view_did_appear", duration: (KeyTaoLog.now() - started) * 1000)
    }

    public override func viewDidDisappear(_ animated: Bool) {
        let started = KeyTaoLog.now()
        super.viewDidDisappear(animated)
        memoryReclamation.hiddenSince = KeyTaoLog.now()
        hiddenMemoryReleaseWorkItem?.cancel()
        let workItem = DispatchWorkItem { [weak self] in self?.releaseMemoryIfNeeded() }
        hiddenMemoryReleaseWorkItem = workItem
        DispatchQueue.main.asyncAfter(deadline: .now() + 60.001, execute: workItem)
        KeyTaoLog.event("lifecycle", "view_did_disappear", duration: (KeyTaoLog.now() - started) * 1000)
        KeyTaoLog.flush(200)
    }

    private func flushRuntimeMetrics() {
        let counts = commandCounts
        commandCounts.removeAll(keepingCapacity: true)
        if !counts.isEmpty {
            KeyTaoLog.event("input", "command_summary") { ["counts": counts] }
        }
        if let fields = applyHistogram.drain() {
            KeyTaoLog.event("input", "apply_summary") { fields }
        }
        keyboardView?.flushRuntimeMetrics()
        engine.flushRuntimeMetrics()
        keytao_log_drain_key_stats()
        // viewDidDisappear flushes the writer after these summaries are queued.
    }

    private func refreshRuntimeLogCollection() {
        runtimeMetricsEnabled = engine.refreshRuntimeLogCollection()
        keyboardView?.runtimeMetricsEnabled = runtimeMetricsEnabled
    }

    public override func textDidChange(_ textInput: UITextInput?) {
        super.textDidChange(textInput)
        guard hostTextMutationDepth == 0 else {
            return
        }
        _ = refreshInputDocumentIfNeeded()
        reloadIfNeeded()
        keyboardView?.updateInputModeSwitchKey(visible: needsInputModeSwitchKey)
        applyHostTraits()
        updateThemeForCurrentAppearance()
    }

    public override func didReceiveMemoryWarning() {
        super.didReceiveMemoryWarning()
        KeyTaoLog.event("memory", "mem_warning")
        KeyTaoLog.memorySnapshot("memory_warning")
        autoreleasepool {
            resetClipboardState()
            for index in clipboardMedia.indices { clipboardMedia[index].thumb = nil }
            engine.releaseCaches()
            currentThemeCacheKey = nil
            keyboardView?.releaseCaches()
        }
        memoryReclamation.memoryWarningPending = true
        releaseMemoryIfNeeded()
    }

    private func releaseMemoryIfNeeded() {
        guard let reason = memoryReclamation.releaseReason(
            now: KeyTaoLog.now(), composing: currentState.hasComposition,
            busy: runtimeStarting || !queuedCommands.isEmpty
        ) else { return }
        let before = KeyTaoLog.memoryFootprint()
        // Candidate enumeration holds the same native handle off the main queue.
        candidateQueue.sync {}
        guard engine.releaseIdleSession() else { return }
        autoreleasepool {
            clipboardMediaLifetime.cancel()
            clipboardMediaCapturePending = false
            let completions = clipboardMediaCompletions
            clipboardMediaCompletions.removeAll()
            for index in clipboardMedia.indices { clipboardMedia[index].thumb = nil }
            keyboardView?.releaseCaches()
            completions.forEach { $0([]) }
            currentThemeCacheKey = nil
        }
        inputAvailable = false
        runtimeReleased = true
        memoryReclamation.didRelease()
        let after = KeyTaoLog.memoryFootprint()
        KeyTaoLog.event("memory", "release_idle_resources") {
            ["reason": reason, "before_bytes": before.map { $0 as Any } ?? NSNull(),
             "after_bytes": after.map { $0 as Any } ?? NSNull()]
        }
        KeyTaoLog.flush(200)
    }

    public override func traitCollectionDidChange(_ previousTraitCollection: UITraitCollection?) {
        super.traitCollectionDidChange(previousTraitCollection)
        if previousTraitCollection?.userInterfaceStyle != traitCollection.userInterfaceStyle {
            updateThemeForCurrentAppearance()
        }
        applyKeyboardPresentationIfOrientationChanged(
            force: previousTraitCollection?.verticalSizeClass != traitCollection.verticalSizeClass
        )
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
        resetInputContextCaches(resetNavigation: currentDocumentIdentifier() != inputDocumentIdentifier)
    }

    public override func selectionWillChange(_ textInput: UITextInput?) {
        super.selectionWillChange(textInput)
        guard hostTextMutationDepth == 0 else {
            return
        }
        resetInputContextCaches(resetNavigation: currentDocumentIdentifier() != inputDocumentIdentifier)
    }

    public override func selectionDidChange(_ textInput: UITextInput?) {
        super.selectionDidChange(textInput)
        guard hostTextMutationDepth == 0 else { return }
        _ = refreshInputDocumentIfNeeded()
        applyHostTraits()
    }

    private func currentDocumentIdentifier() -> UUID? {
        // Some hosts return nil before attaching a document, despite Swift's non-optional import.
        let identifier = (textDocumentProxy as AnyObject)
            .perform(#selector(getter: UITextDocumentProxy.documentIdentifier))?
            .takeUnretainedValue() as? NSUUID
        return identifier.map { $0 as UUID }
    }

    /// Everything bound to "the text field the user is in right now": the Rime
    /// composition, the host marked text, the backspace restore stack and the
    /// clipboard snapshot. Called whenever the input context ends or changes.
    private func resetInputContextCaches(resetNavigation: Bool = false) {
        resettingInputContext = true
        defer {
            resettingInputContext = false
            releaseMemoryIfNeeded()
        }
        keyboardView?.releaseChordTouches()
        inputContextGeneration &+= 1
        inputDocumentIdentifier = currentDocumentIdentifier()
        // Cold-start keystrokes belong to the context where they were queued.
        // Runtime initialization may finish after a field change or dismissal.
        queuedCommands.removeAll()
        cancelBackspaceSelection()
        backspaceRestoreStack.removeAll()
        doubleSpacePeriodTracker.reset()
        lastCommittedText = nil
        resetClipboardState()
        clearHostMarkedText()
        if resetNavigation {
            keyboardView?.resetNavigation(forcedLayer: KeyTaoHostTraits(proxy: textDocumentProxy).forcedLayer)
        }
        guard engine.nativeReady else {
            currentState = currentState.withoutTransientCommit()
            keyboardView?.update(state: currentState)
            return
        }
        currentState = engine.reset().withoutTransientCommit()
        keyboardView?.update(state: currentState)
    }

    /// Host notifications can arrive after the proxy has already switched.
    /// Check identity at input entry points before using undo, queued input or
    /// candidate state cached for the preceding document.
    @discardableResult
    private func refreshInputDocumentIfNeeded() -> Bool {
        // A host that never attaches an identifier keeps reporting nil; treating
        // nil as a field change would reset the composition on every keystroke.
        guard currentDocumentIdentifier() == inputDocumentIdentifier else {
            resetInputContextCaches(resetNavigation: true)
            applyHostTraits()
            return false
        }
        return true
    }

    private func inputContextIsCurrent(_ generation: UInt64, documentIdentifier: UUID?) -> Bool {
        generation == inputContextGeneration && currentDocumentIdentifier() == documentIdentifier
    }

    func keyboardView(_ view: KeyTaoIOSKeyboardView, didTrigger command: KeyTaoKeyCommand) {
        let started = KeyTaoLog.now()
        let previousKeystrokeStarted = pendingKeystrokeStarted
        pendingKeystrokeStarted = started
        defer { pendingKeystrokeStarted = previousKeystrokeStarted }
        if runtimeMetricsEnabled {
            commandCounts[KeyTaoLog.commandType(command.type), default: 0] += 1
        }
        performCommand(command)
    }

    func keyboardViewWillBeginChord(_ view: KeyTaoIOSKeyboardView) -> Bool {
        _ = refreshInputDocumentIfNeeded()
        return inputAvailable && engine.nativeReady && !runtimeStarting
            && currentState.chordComposer && !currentState.asciiMode && !hostTraits.bypassesRime
    }

    func keyboardView(_ view: KeyTaoIOSKeyboardView, didTriggerChordKey event: KeyTaoChordKeyEvent) {
        // Release before a context reset: the ending touch has already left the view's held-key store.
        let result = engine.processKey(event.key, modifiers: event.modifiers)
        if !resettingInputContext, refreshInputDocumentIfNeeded() {
            if !event.released, !result.accepted, let character = UnicodeScalar(event.key) {
                applyRejectedKeyFallback(result, text: String(character))
            } else {
                apply(result)
            }
        }
    }

    private func performCommand(_ command: KeyTaoKeyCommand) {
        refreshInputDocumentIfNeeded()
        defer { releaseMemoryIfNeeded() }
        if runtimeReleased && (command.requiresInstalledSchema || command.type == KeyTaoCommandType.rimeMenu) {
            startRuntimeIfNeeded()
        }
        // The runtime is still coming up on the startup queue. Queue the key
        // instead of committing it raw, otherwise the first few characters of
        // every cold start would land in the host as latin letters. Once text
        // is pending, later deletes, direct text and document edits must wait
        // behind it rather than changing the host before those keys arrive.
        if engine.algorithmEnabled && runtimeStarting && (command.requiresInstalledSchema
            || (!queuedCommands.isEmpty && command.editsHostDocument)) {
            if queuedCommands.count < Self.queuedCommandLimit {
                queuedCommands.append(command)
            }
            return
        }
        let directsWhileDisabled = !engine.algorithmEnabled
            && [KeyTaoCommandType.enter, KeyTaoCommandType.space].contains(command.type)
        if !inputAvailable && command.requiresInstalledSchema && !directsWhileDisabled {
            if runtimeResumeFailed {
                showMessage(unavailableMessage)
                return
            }
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
            KeyTaoLog.event("ui", "input_mode_switch")
            advanceToNextInputMode()
        case KeyTaoCommandType.nextInputMethod:
            KeyTaoLog.event("ui", "input_mode_switch")
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
            guard inputAvailable else {
                keyboardView?.update(rimeOptions: .empty)
                keyboardView?.updateSchemeOperationProgress(engine.schemeOperationProgress())
                return
            }
            if currentState.isPackageSwitcher {
                apply(engine.processKey(rimeKeyEscape))
            }
            refreshRimeOptions()
        case KeyTaoCommandType.rimeSchema:
            if let view = keyboardView, view.isRimeMenuOpen {
                let generation = inputContextGeneration
                let document = currentDocumentIdentifier()
                view.selectRimeSchemaAfterFrame(command) { [weak self] in
                    guard let self, self.keyboardVisible, self.inputAvailable,
                          self.inputContextIsCurrent(generation, documentIdentifier: document) else { return }
                    self.selectRimeSchema(command.value, packageKey: command.fallbackValue)
                }
            } else {
                selectRimeSchema(command.value, packageKey: command.fallbackValue)
            }
        case KeyTaoCommandType.rimeOption:
            if keyboardView?.rimeSchemaSelectionPending != true {
                setRimeOption(command.value, action: command.fallbackValue)
            }
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

    func keyboardViewWillEnterDirectInputPage(_ view: KeyTaoIOSKeyboardView) -> Bool {
        defer { releaseMemoryIfNeeded() }
        guard refreshInputDocumentIfNeeded() else { return false }
        return commitPendingComposition()
    }

    func keyboardView(_ view: KeyTaoIOSKeyboardView, commitEmojiSearchResult text: String) {
        defer { releaseMemoryIfNeeded() }
        guard refreshInputDocumentIfNeeded(), view.isEmojiSearching,
              commitPendingComposition() else { return }
        _ = insertHostText(text, intoSearch: false)
    }

    func keyboardViewEmojiSearchModeDidChange(_ view: KeyTaoIOSKeyboardView) {
        guard let config = baseKeyboardConfig else { return }
        applyKeyboardPresentation(config: config)
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
        defer { releaseMemoryIfNeeded() }
        guard refreshInputDocumentIfNeeded() else { return }
        if !inputAvailable {
            showUnavailableMessage()
            return
        }
        // Both entry points exist in the vendored librime; the guard keeps
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
        refreshInputDocumentIfNeeded() && inputAvailable && engineCapabilities.candidateDeletion && engine.candidateIsUserPhrase(index)
    }

    func keyboardView(_ view: KeyTaoIOSKeyboardView, deleteCandidate index: Int) -> Bool {
        defer { releaseMemoryIfNeeded() }
        guard refreshInputDocumentIfNeeded() else { return false }
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
        guard refreshInputDocumentIfNeeded() else { completion([]); return }
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

    func keyboardView(_ view: KeyTaoIOSKeyboardView, requestClipboardHistory completion: @escaping ([ClipboardEntry]) -> Void) {
        refreshInputDocumentIfNeeded()
        view.update(clipboardFullAccess: hasFullAccess)
        loadClipboardHistoryIfNeeded()
        guard clipboardAccessPolicy.canShowHistory else {
            resetClipboardState()
            completion([])
            return
        }
        refreshCurrentClipboardText()
        guard clipboardCaptureAllowed else {
            completion(clipboardEntries())
            return
        }
        if clipboardMediaCapturePending {
            clipboardMediaCompletions.append(completion)
            return
        }
        let changeCount = UIPasteboard.general.changeCount
        guard clipboardLastReadChangeCount != changeCount,
              clipboardMediaSuppressionChangeCount != changeCount else {
            completion(clipboardEntries())
            return
        }
        let lifetime = clipboardMediaLifetime
        let generation = lifetime.currentGeneration()
        guard let source = currentClipboardMedia(expectedChangeCount: changeCount) else {
            guard generation == lifetime.currentGeneration(), clipboardCaptureAllowed else {
                completion([])
                return
            }
            if rememberCurrentClipboard(expectedChangeCount: changeCount) {
                clipboardLastReadChangeCount = changeCount
            }
            completion(clipboardEntries())
            return
        }
        guard generation == lifetime.currentGeneration(), clipboardCaptureAllowed else {
            completion([])
            return
        }
        let digest = SHA256.hash(data: source.data).map { String(format: "%02x", $0) }.joined()
        let timestamp = Date().timeIntervalSince1970
        if let media = clipboardMedia.first(where: { $0.id.hasPrefix(digest + ".") }) {
            clipboardHistory.upsert(KeyTaoClipboardRecord(
                key: "media:\(media.id)", mime: media.mime, name: media.name, size: media.size,
                width: media.width, height: media.height, timestamp: timestamp
            ))
            persistClipboardHistory()
            clipboardLastReadChangeCount = changeCount
            completion(clipboardEntries())
            return
        }
        // Reserve the byte budget before the worker writes its snapshot.
        while clipboardMedia.reduce(0, { $0 + $1.size }) + source.data.count > Self.clipboardMediaTotalBytes {
            guard let evicted = clipboardHistory.records.last(where: { $0.mime != nil && !$0.pinned }) else {
                completion(clipboardEntries())
                return
            }
            clipboardHistory.remove(evicted.key)
            reconcileClipboardMedia()
            persistClipboardHistory()
        }
        clipboardMediaCapturePending = true
        clipboardMediaCompletions.append(completion)
        // Unique snapshots prevent a cancelled worker from deleting a newer capture.
        let id = digest + "." + UUID().uuidString + "." + source.fileExtension
        let directory = clipboardHistoryFile?.deletingLastPathComponent() ?? KeyTaoIOSPaths.clipboardDir()
        let file = directory.appendingPathComponent(id)
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let media: ClipboardMedia? = autoreleasepool {
                guard lifetime.write(source.data, to: file, generation: generation) else { return nil }
                let preview = Self.clipboardThumbnail(file: file, mime: source.mime)
                return ClipboardMedia(id: id, file: file, pasteboardType: source.type, mime: source.mime,
                                      name: source.name, size: source.data.count, width: preview.width,
                                      height: preview.height, timestamp: timestamp, thumb: preview.image)
            }
            DispatchQueue.main.async { [weak self] in
                guard let self,
                      generation == lifetime.currentGeneration() else {
                    try? FileManager.default.removeItem(at: file)
                    return
                }
                guard self.clipboardCaptureAllowed else {
                    try? FileManager.default.removeItem(at: file)
                    self.resetClipboardState()
                    return
                }
                if let media {
                    self.clipboardMedia.insert(media, at: 0)
                    self.clipboardHistory.upsert(KeyTaoClipboardRecord(
                        key: "media:\(media.id)", mime: media.mime, name: media.name, size: media.size,
                        width: media.width, height: media.height, timestamp: media.timestamp
                    ))
                    self.reconcileClipboardMedia()
                    self.persistClipboardHistory()
                    self.clipboardLastReadChangeCount = changeCount
                    self.logClipboardMedia()
                }
                self.clipboardMediaCapturePending = false
                let completions = self.clipboardMediaCompletions
                self.clipboardMediaCompletions.removeAll()
                let entries = self.clipboardEntries()
                completions.forEach { $0(entries) }
            }
        }
    }

    func keyboardView(_ view: KeyTaoIOSKeyboardView, deleteClipboardEntry key: String, media: Bool) {
        guard clipboardAccessPolicy.canShowHistory else { return }
        clipboardHistory.remove(key)
        reconcileClipboardMedia()
        persistClipboardHistory()
        if !media, currentClipboardText() == key {
            clipboardSuppression = (text: key, changeCount: UIPasteboard.general.changeCount)
        }
    }

    func keyboardView(_ view: KeyTaoIOSKeyboardView, toggleClipboardPin key: String) {
        guard clipboardAccessPolicy.canShowHistory else { return }
        clipboardHistory.togglePin(key)
        reconcileClipboardMedia()
        persistClipboardHistory()
    }

    func keyboardViewClearClipboardHistory(_ view: KeyTaoIOSKeyboardView) {
        guard clipboardAccessPolicy.canShowHistory else { return }
        let changeCount = hasFullAccess ? UIPasteboard.general.changeCount : nil
        resetClipboardState()
        clipboardHistory.clearUnpinned()
        reconcileClipboardMedia()
        persistClipboardHistory()
        clipboardLastReadChangeCount = changeCount
        clipboardMediaSuppressionChangeCount = changeCount
    }

    func keyboardView(_ view: KeyTaoIOSKeyboardView, commitClipboardMedia key: String) {
        guard refreshInputDocumentIfNeeded() else { return }
        guard clipboardReadAllowed else {
            resetClipboardState()
            return
        }
        guard let item = clipboardMedia.first(where: { "media:\($0.id)" == key }) else { return }
        let generation = clipboardMediaLifetime.currentGeneration()
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let data = try? Data(contentsOf: item.file, options: .mappedIfSafe)
            DispatchQueue.main.async { [weak self] in
                guard let self, generation == self.clipboardMediaLifetime.currentGeneration() else { return }
                guard self.clipboardReadAllowed else {
                    self.resetClipboardState()
                    return
                }
                guard self.clipboardMedia.contains(where: { $0.id == item.id }),
                      let data, data.count <= Self.clipboardMediaMaxBytes else { return }
                UIPasteboard.general.setData(data, forPasteboardType: item.pasteboardType)
                let changeCount = UIPasteboard.general.changeCount
                self.clipboardMediaSuppressionChangeCount = changeCount
                self.clipboardLastReadChangeCount = changeCount
                self.showMessage("已复制到剪贴板，请在输入框长按粘贴")
            }
        }
    }

    func keyboardView(_ view: KeyTaoIOSKeyboardView, previewSetting key: String, value: String) {
        guard let current = baseKeyboardConfig,
              let next = applySetting(to: current, key: key, value: value) else { return }
        previewKeyboardPresentation(config: next)
    }

    func keyboardView(_ view: KeyTaoIOSKeyboardView, persistSetting key: String, value: String) {
        if key == "accentColor" || key == "colorScheme" {
            persistThemeSetting(key: key, value: value)
            return
        }
        if key == "reset" {
            let patch = defaultPanelSettingsPatch()
            var next = baseKeyboardConfig ?? engine.loadConfig()
            for (settingKey, settingValue) in patch {
                next = applySetting(to: next, key: settingKey, value: String(describing: settingValue)) ?? next
            }
            baseKeyboardConfig = next
            applyKeyboardPresentation(config: next)
            let configWritten = engine.persistSettings(patch: patch)
            let themeWritten = engine.persistThemeUi(colorScheme: "auto", accentHex: "", resetAppearance: true)
            engine.releaseCaches()
            updateThemeForCurrentAppearance()
            if !configWritten || !themeWritten { showMessage("键盘内设置恢复失败") }
            return
        }
        guard let patchValue = settingPatch(key: key, value: value) else { return }
        if let current = baseKeyboardConfig {
            applyKeyboardPresentation(config: current)
        }
        if !engine.persistSettings(patch: [key: patchValue]) {
            showMessage("设置保存失败")
        }
    }

    private func persistThemeSetting(key: String, value: String) {
        let currentScheme = keyboardView?.currentThemeColorScheme() ?? "auto"
        let scheme = key == "colorScheme" ? value : currentScheme
        let accent: String? = key == "accentColor" ? value : nil
        guard engine.persistThemeUi(colorScheme: scheme, accentHex: accent) else {
            showMessage("主题设置保存失败")
            return
        }
        engine.releaseCaches()
        updateThemeForCurrentAppearance()
    }

    private func settingPatch(key: String, value: String) -> Any? {
        switch key {
        case "keyboardHeightDp", "candidateBarHeightDp", "haptics.intensity":
            return Double(value).map { Int($0.rounded()) }
        case "candidateFontScale":
            return Double(value)
        case "keyHintVisible", "mnemonicHintsEnabled", "numberRowEnabled", "haptics.enabled", "keyPreviewEnabled":
            return Bool(value)
        default:
            return nil
        }
    }

    private func applySetting(
        to current: KeyTaoIOSImeConfig,
        key: String,
        value: String
    ) -> KeyTaoIOSImeConfig? {
        var next = current
        switch key {
        case "keyboardHeightDp":
            guard let number = Double(value) else { return nil }
            next.keyboardHeightDp = max(160, min(420, CGFloat(number.rounded())))
        case "candidateBarHeightDp":
            guard let number = Double(value) else { return nil }
            next.candidateBarHeightDp = max(36, min(96, CGFloat(number.rounded())))
        case "candidateFontScale":
            guard let number = Double(value) else { return nil }
            next.candidateFontScale = max(0.8, min(1.4, CGFloat(number)))
        case "keyHintVisible":
            guard let enabled = Bool(value) else { return nil }
            next.keyHintVisible = enabled
        case "mnemonicHintsEnabled":
            guard let enabled = Bool(value) else { return nil }
            next.mnemonicHintsEnabled = enabled
        case "numberRowEnabled":
            guard let enabled = Bool(value) else { return nil }
            next.numberRowEnabled = enabled
        case "haptics.enabled":
            guard let enabled = Bool(value) else { return nil }
            next.hapticsEnabled = enabled
        case "haptics.intensity":
            guard let number = Double(value) else { return nil }
            next.hapticIntensity = max(1, min(100, Int(number.rounded())))
        case "keyPreviewEnabled":
            guard let enabled = Bool(value) else { return nil }
            next.keyPreviewEnabled = enabled
        default:
            return nil
        }
        return next
    }

    private func previewKeyboardPresentation(config: KeyTaoIOSImeConfig) {
        guard let keyboardView else { return }
        baseKeyboardConfig = config
        let normalized = layoutState.normalized()
        let isLandscape = currentInterfaceIsLandscape()
        let presentation = keyboardLayoutPresentation(state: normalized, isLandscape: isLandscape)
        let presented = presentedKeyboardConfig(config, state: normalized, presentation: presentation)
        keyboardView.update(settingsConfig: config)
        keyboardView.update(config: presented)
        let keyboardHeight = presented.effectiveKeyboardHeightDp + presented.candidateBarHeightDp
            + keyboardView.emojiSearchExtraHeight
        keyboardHeightConstraint?.constant = keyboardHeight
        let margin = presentation.isCompact ? config.floating.marginDp : 0
        heightConstraint?.constant = keyboardHeight + (presentation.isCompact ? margin * 2 : 0)
        self.view.layoutIfNeeded()
    }

    private func defaultPanelSettingsPatch() -> [String: Any] {
        [
            "candidateFontScale": 1.0,
            "keyHintVisible": true,
            "mnemonicHintsEnabled": false,
            "keyboardHeightDp": 266,
            "candidateBarHeightDp": 52,
            "numberRowEnabled": false,
            "haptics.enabled": true,
            "haptics.intensity": 42,
            "keyPreviewEnabled": true,
        ]
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
        let generation = inputContextGeneration
        let documentIdentifier = currentDocumentIdentifier()
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
            guard inputContextIsCurrent(generation, documentIdentifier: documentIdentifier) else { return }
            let result = engine.processKey(key)
            if result.accepted {
                apply(result)
                guard inputContextIsCurrent(generation, documentIdentifier: documentIdentifier) else { return }
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
        let generation = inputContextGeneration
        let documentIdentifier = currentDocumentIdentifier()
        apply(result)
        guard inputContextIsCurrent(generation, documentIdentifier: documentIdentifier) else { return }
        if currentState.hasComposition {
            apply(engine.commitComposition())
            guard inputContextIsCurrent(generation, documentIdentifier: documentIdentifier) else { return }
        }
        insertHostText(text)
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
        if keyboardView?.deleteEmojiSearchText() == true { return }
        _ = deleteOneBeforeCursorForRestore()
    }

    private func handleBackspaceGesture(_ action: String, count: Int) {
        if keyboardView?.isEmojiSearching == true {
            if ["delete", "deleteSegment", "deleteAll"].contains(action) { handleBackspace() }
            return
        }
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
                guard restoreOneBackspaceText() else { break }
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
    }

    private func updateBackspaceSelection(count: Int) {
        guard !hostTraits.isSensitive else { return }
        let generation = inputContextGeneration
        let documentIdentifier = currentDocumentIdentifier()
        resetCompositionBeforeBackspaceGesture()
        guard inputContextIsCurrent(generation, documentIdentifier: documentIdentifier) else { return }
        let originalBefore = backspaceSelectionSession.flatMap {
            $0.documentIdentifier == documentIdentifier ? $0.originalBefore : nil
        }
            ?? textDocumentProxy.documentContextBeforeInput
            ?? ""
        restoreBackspaceSelectionMarkup()
        guard inputContextIsCurrent(generation, documentIdentifier: documentIdentifier) else { return }
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
                guard inputContextIsCurrent(generation, documentIdentifier: documentIdentifier) else { return }
                textDocumentProxy.deleteBackward()
            }
            guard inputContextIsCurrent(generation, documentIdentifier: documentIdentifier) else { return }
            markedTextOwnership.claim(documentIdentifier)
            backspaceSelectionSession = BackspaceSelectionSession(
                documentIdentifier: documentIdentifier,
                originalBefore: originalBefore,
                selectedText: selectedText
            )
            textDocumentProxy.setMarkedText(
                selectedText,
                selectedRange: NSRange(location: 0, length: selectedText.utf16.count)
            )
        }
        guard inputContextIsCurrent(generation, documentIdentifier: documentIdentifier) else { return }
        keyboardView?.showBackspaceDeletionPreview(selectedText, pendingSelection: true)
    }

    private func restoreBackspaceSelectionMarkup() {
        guard let session = backspaceSelectionSession else { return }
        backspaceSelectionSession = nil
        let generation = inputContextGeneration
        let documentIdentifier = currentDocumentIdentifier()
        _ = markedTextOwnership.release(for: documentIdentifier)
        guard let documentIdentifier, session.documentIdentifier == documentIdentifier else { return }
        withHostTextMutation {
            textDocumentProxy.insertText(session.selectedText)
            guard inputContextIsCurrent(generation, documentIdentifier: documentIdentifier) else { return }
            textDocumentProxy.unmarkText()
        }
    }

    private func cancelBackspaceSelection() {
        restoreBackspaceSelectionMarkup()
        keyboardView?.showBackspaceDeletionPreview("")
    }

    private func commitBackspaceSelection() {
        guard let session = backspaceSelectionSession else { return }
        backspaceSelectionSession = nil
        let generation = inputContextGeneration
        let documentIdentifier = currentDocumentIdentifier()
        guard let documentIdentifier, session.documentIdentifier == documentIdentifier else {
            _ = markedTextOwnership.release(for: documentIdentifier)
            keyboardView?.showBackspaceDeletionPreview("")
            return
        }
        let selectedText = session.selectedText
        backspaceRestoreStack.removeAll()
        backspaceGestureRestoreStart = 0
        clearHostMarkedText()
        guard inputContextIsCurrent(generation, documentIdentifier: documentIdentifier) else { return }
        backspaceRestoreStack.append(contentsOf: selectedText.reversed().map(String.init))
        keyboardView?.showBackspaceDeletionPreview(selectedText)
    }

    private func deleteTrailingSegmentBeforeCursorForRestore() {
        let generation = inputContextGeneration
        let documentIdentifier = currentDocumentIdentifier()
        let before = textDocumentProxy.documentContextBeforeInput ?? ""
        let count = keyTaoTrailingDeletionSegmentLength(before)
        for _ in 0..<count {
            guard inputContextIsCurrent(generation, documentIdentifier: documentIdentifier) else { return }
            guard deleteOneBeforeCursorForRestore() else {
                break
            }
        }
    }

    private func deleteOneBeforeCursorForRestore() -> Bool {
        let generation = inputContextGeneration
        let documentIdentifier = currentDocumentIdentifier()
        resetCompositionBeforeBackspaceGesture()
        guard inputContextIsCurrent(generation, documentIdentifier: documentIdentifier) else { return false }
        return withHostTextMutation {
            guard let before = textDocumentProxy.documentContextBeforeInput,
                  let deleted = before.last else {
                textDocumentProxy.deleteBackward()
                return false
            }
            textDocumentProxy.deleteBackward()
            guard inputContextIsCurrent(generation, documentIdentifier: documentIdentifier) else { return false }
            backspaceRestoreStack.append(String(deleted))
            return true
        }
    }

    /// `documentContextBeforeInput` only returns the slice the host chose to
    /// hand over, so "delete everything before the caret" has to re-read it
    /// after every batch until the host stops giving text back.
    private func deleteAllBeforeCursorForRestore() {
        let generation = inputContextGeneration
        let documentIdentifier = currentDocumentIdentifier()
        resetCompositionBeforeBackspaceGesture()
        guard inputContextIsCurrent(generation, documentIdentifier: documentIdentifier) else { return }
        withHostTextMutation {
            var deleted = 0
            while deleted < Self.deleteAllBatchLimit {
                guard inputContextIsCurrent(generation, documentIdentifier: documentIdentifier) else { return }
                guard let before = textDocumentProxy.documentContextBeforeInput, !before.isEmpty else {
                    return
                }
                let batch = keyTaoBackwardDeletionBatch(
                    before,
                    remainingLimit: Self.deleteAllBatchLimit - deleted
                )
                for removedText in batch {
                    guard inputContextIsCurrent(generation, documentIdentifier: documentIdentifier) else { return }
                    textDocumentProxy.deleteBackward()
                    guard inputContextIsCurrent(generation, documentIdentifier: documentIdentifier) else { return }
                    backspaceRestoreStack.append(removedText)
                    deleted += 1
                }
            }
        }
    }

    private func restoreOneBackspaceText() -> Bool {
        let generation = inputContextGeneration
        let documentIdentifier = currentDocumentIdentifier()
        guard let text = backspaceRestoreStack.popLast() else {
            return false
        }
        clearHostMarkedText()
        guard inputContextIsCurrent(generation, documentIdentifier: documentIdentifier) else { return false }
        withHostTextMutation {
            textDocumentProxy.insertText(text)
        }
        return inputContextIsCurrent(generation, documentIdentifier: documentIdentifier)
    }

    private func restoreAllBackspaceText() {
        let generation = inputContextGeneration
        let documentIdentifier = currentDocumentIdentifier()
        guard !backspaceRestoreStack.isEmpty else {
            return
        }
        var restored = ""
        while let text = backspaceRestoreStack.popLast() {
            restored.append(text)
        }
        clearHostMarkedText()
        guard inputContextIsCurrent(generation, documentIdentifier: documentIdentifier) else { return }
        withHostTextMutation {
            textDocumentProxy.insertText(restored)
        }
    }

    private func resetCompositionBeforeBackspaceGesture() {
        guard currentState.hasComposition else {
            return
        }
        let generation = inputContextGeneration
        let documentIdentifier = currentDocumentIdentifier()
        _ = engine.reset()
        clearHostMarkedText()
        guard inputContextIsCurrent(generation, documentIdentifier: documentIdentifier) else { return }
        currentState = engine.nativeReady ? engine.state().withoutTransientCommit() : currentState.withoutTransientCommit()
        keyboardView?.update(state: currentState)
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
            enabled: config.doubleSpacePeriodEnabled && keyboardView?.isEmojiSearching != true,
            hasComposition: currentState.hasComposition
        )
        if replace {
            _ = deleteOneBeforeCursorForRestore()
            commitDirect(isEnglishMode ? ". " : "。")
            return
        }
        if !engine.algorithmEnabled || hostTraits.bypassesRime {
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
        let decision = keyTaoLanguageModeDecision(
            englishMode: configuredEnglishMode,
            englishSchemaID: englishSchemaID,
            value: value,
            currentSchemaID: currentRimeSchemaID,
            asciiMode: currentState.asciiMode
        )
        guard decision.usesEnglishSchema else {
            apply(engine.setAsciiMode(decision.targetEnglish))
            return
        }
        guard let englishSchemaID else {
            return
        }
        if (currentRimeSchemaID == englishSchemaID) == decision.targetEnglish {
            return
        }

        let schemas = engine.listSchemas()
        let targetSchemaID: String?
        if decision.targetEnglish {
            targetSchemaID = englishSchemaID
        } else if let lastChineseSchemaID,
                  schemas.contains(where: { $0.id == lastChineseSchemaID && $0.id != englishSchemaID }) {
            targetSchemaID = lastChineseSchemaID
        } else {
            targetSchemaID = schemas.first(where: { $0.id != englishSchemaID })?.id
        }
        guard let targetSchemaID else {
            showMessage("没有可用的中文输入方案")
            refreshRimeOptions()
            return
        }

        if currentState.hasComposition || markedTextOwnership.belongs(to: currentDocumentIdentifier()) {
            clearHostMarkedText()
            apply(engine.clearComposition())
        }
        if decision.targetEnglish,
           let current = engine.currentSchema(),
           current.id != englishSchemaID {
            lastChineseSchemaID = current.id
            chineseSwitchSnapshot = keyTaoChineseSwitchSnapshot(
                names: engine.schemaSwitches().flatMap(\.optionNames) + ["ascii_punct"],
                optionValue: engine.getOption
            )
        }

        guard var state = engine.selectSchema(targetSchemaID) else {
            showMessage("无法切换输入方案")
            refreshRimeOptions()
            return
        }
        var failedOptions: [String] = []
        if !decision.targetEnglish {
            for (optionName, enabled) in chineseSwitchSnapshot {
                guard let restoredState = engine.setOption(optionName, enabled: enabled) else {
                    failedOptions.append(optionName)
                    continue
                }
                state = restoredState
            }
        }
        apply(state)
        if !failedOptions.isEmpty {
            showMessage("无法恢复 Rime 选项：\(failedOptions.joined(separator: "、"))")
        }
        refreshRimeOptions()
    }

    private func selectRimeSchema(_ schemaID: String?, packageKey: String?) {
        guard let schemaID, !schemaID.isEmpty else {
            return
        }
        if let packageKey {
            let state = engine.selectPackageSchema(packageKey, schemaID: schemaID)
            if let state {
                if state.accepted {
                    lastChineseSchemaID = nil
                    chineseSwitchSnapshot = [:]
                    refreshEngineCapabilities()
                }
                apply(state)
            }
            if state?.accepted != true { showMessage("无法切换输入方案") }
            refreshRimeOptions()
            return
        }
        if usesEnglishSchema, schemaID == englishSchemaID {
            handleMode("ascii")
            return
        }
        if usesEnglishSchema, currentRimeSchemaID == englishSchemaID {
            lastChineseSchemaID = schemaID
            handleMode("chinese")
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
            state = optionName == "ascii_mode" && !usesEnglishSchema
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
        let optionsOpen = keyboardView?.isRimeMenuOpen == true
        let progress = optionsOpen ? engine.schemeOperationProgress() : nil
        if optionsOpen { rimeOptionsReloadStamp = engine.loadedReloadStampSignature() }
        let schemas = engine.listSchemas()
        let currentSchema = engine.currentSchema()
        englishSchemaID = keyTaoEnglishSchemaID(schemas: schemas.map { ($0.id, $0.name) })
        currentRimeSchemaID = currentSchema?.id
        if let currentSchema, currentSchema.id != englishSchemaID {
            lastChineseSchemaID = currentSchema.id
        }
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
            schemas: schemas,
            currentSchema: currentSchema,
            englishSchemaID: usesEnglishSchema ? englishSchemaID : nil,
            switches: switches,
            options: optionStates,
            packages: optionsOpen ? engine.listSwitcherPackages() : []
        ))
        if optionsOpen {
            keyboardView?.updateSchemeOperationProgress(progress)
        }
    }

    private func handleEditAction(_ action: String, value: String?) {
        if keyboardView?.isEmojiSearching == true,
           !["paste", "pasteText", "tab", "repeatCommit"].contains(action) { return }
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
        guard clipboardAccessPolicy.canShowHistory else {
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
        refreshCurrentClipboardText()
    }

    private func pasteClipboard() {
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
    }

    /// Reading `UIPasteboard.general.string` needs Full Access, and since iOS 16
    /// it also raises the system paste prompt. `hasStrings` is metadata only, so
    /// it lets an empty clipboard be reported without bothering the user.
    private func currentClipboardText() -> String? {
        guard clipboardReadAllowed,
              !UIPasteboard.general.hasImages, UIPasteboard.general.hasStrings else {
            return nil
        }
        return UIPasteboard.general.string?.takeIfNotEmpty
    }

    /// Called only by the explicit clipboard-panel request. Inspect metadata
    /// first, then retain one original representation without UIImage encoding.
    private func currentClipboardMedia(expectedChangeCount: Int) -> (data: Data, type: String, mime: String, name: String, fileExtension: String)? {
        guard clipboardCaptureAllowed else { return nil }
        let pasteboard = UIPasteboard.general
        let hasImages = pasteboard.hasImages
        // Rich text may also advertise flat RTFD/webarchive data; preserve its
        // directly insertable string unless an actual image is advertised.
        guard hasImages || !pasteboard.hasStrings else { return nil }
        let types = pasteboard.types.compactMap { identifier -> UTType? in
            guard let type = UTType(identifier), type.conforms(to: .data),
                  !type.conforms(to: .text), !type.conforms(to: .url) else { return nil }
            if hasImages { return type.conforms(to: .image) ? type : nil }
            // File URLs are not copied: an extension does not own their grants.
            return type
        }
        for type in types {
            guard let data = pasteboard.data(forPasteboardType: type.identifier) else { continue }
            guard pasteboard.changeCount == expectedChangeCount,
                  !data.isEmpty, data.count <= Self.clipboardMediaMaxBytes else { return nil }
            let fileExtension = type.preferredFilenameExtension ?? "bin"
            let mime = type.preferredMIMEType ?? (type.conforms(to: .image) ? "image/*" : "application/octet-stream")
            return (data, type.identifier, mime,
                    "剪贴板文件.\(fileExtension)", fileExtension)
        }
        return nil
    }

    private static func clipboardThumbnail(file: URL, mime: String) -> (image: UIImage?, width: Int, height: Int) {
        guard mime.hasPrefix("image/"),
              let source = CGImageSourceCreateWithURL(file as CFURL, [kCGImageSourceShouldCache: false] as CFDictionary),
              let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any] else {
            return (nil, 0, 0)
        }
        let width = (properties[kCGImagePropertyPixelWidth] as? NSNumber)?.intValue ?? 0
        let height = (properties[kCGImagePropertyPixelHeight] as? NSNumber)?.intValue ?? 0
        guard width > 0, height > 0, width <= clipboardMaxPixels / height else {
            return (nil, width, height)
        }
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: clipboardThumbPx,
            kCGImageSourceShouldCacheImmediately: true,
        ]
        let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary).map { UIImage(cgImage: $0) }
        return (image, width, height)
    }

    private func clipboardEntries() -> [ClipboardEntry] {
        guard clipboardAccessPolicy.canShowHistory else { return [] }
        return clipboardHistory.records.compactMap { record in
            guard clipboardConfig.clipboardHistoryEnabled || record.pinned else { return nil }
            if let text = record.text {
                return ClipboardEntry(key: record.key, text: text, pinned: record.pinned)
            }
            guard let index = clipboardMedia.firstIndex(where: { "media:\($0.id)" == record.key }) else { return nil }
            if clipboardMedia[index].thumb == nil {
                clipboardMedia[index].thumb = Self.clipboardThumbnail(
                    file: clipboardMedia[index].file, mime: clipboardMedia[index].mime
                ).image
            }
            var entry = clipboardMedia[index].entry
            entry.pinned = record.pinned
            return entry
        }
    }

    private var clipboardHistoryFile: URL? {
        guard hasFullAccess else { return nil }
        return FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: KeyTaoIOSPaths.appGroupIdentifier)?
            .appendingPathComponent("keytao/clipboard/history.json")
    }

    private var clipboardConfig: KeyTaoIOSImeConfig {
        baseKeyboardConfig ?? .fallback
    }

    private var clipboardConfigLoaded: Bool {
        baseKeyboardConfig != nil && engine.clipboardConfigLoaded
    }

    private var clipboardAccessPolicy: KeyTaoClipboardAccessPolicy {
        KeyTaoClipboardAccessPolicy(
            historyEnabled: clipboardConfig.clipboardHistoryEnabled,
            hideSensitive: clipboardConfig.clipboardHideSensitive,
            isSensitive: KeyTaoHostTraits(proxy: textDocumentProxy).isSensitive
        )
    }

    private var clipboardReadAllowed: Bool {
        hasFullAccess && clipboardAccessPolicy.canShowHistory
    }

    private var clipboardCaptureAllowed: Bool {
        hasFullAccess && clipboardConfigLoaded && clipboardAccessPolicy.canRecord
    }

    private func loadClipboardHistoryIfNeeded() {
        if clipboardHistory.setLimit(clipboardConfigLoaded ? clipboardConfig.clipboardHistoryLimit : nil) {
            reconcileClipboardMedia()
            persistClipboardHistory()
        }
        guard !clipboardHistoryLoaded, let file = clipboardHistoryFile else { return }
        guard FileManager.default.fileExists(atPath: file.path) else {
            clipboardHistoryLoaded = true
            return
        }
        do {
            let saved = try KeyTaoClipboardHistory.readRecords(contentsOf: file)
            var loadedMediaIDs = Set<String>()
            let records = saved.filter { record in
                if record.text != nil { return true }
                guard record.key.hasPrefix("media:"), let mime = record.mime else { return false }
                let id = String(record.key.dropFirst("media:".count))
                if loadedMediaIDs.contains(id) { return true }
                let url = file.deletingLastPathComponent().appendingPathComponent(id)
                let components = id.split(separator: ".")
                let digest = components.first.map(String.init) ?? ""
                guard id == url.lastPathComponent,
                      digest.count == 64, digest.allSatisfy(\.isHexDigit),
                      components.count == 3, UUID(uuidString: String(components[1])) != nil,
                      let type = UTType(filenameExtension: url.pathExtension) ?? UTType(mimeType: mime),
                      let attributes = try? FileManager.default.attributesOfItem(atPath: url.path),
                      let size = attributes[.size] as? NSNumber,
                      size.intValue > 0, size.intValue <= Self.clipboardMediaMaxBytes else { return false }
                clipboardMedia.append(ClipboardMedia(
                    id: id, file: url, pasteboardType: type.identifier, mime: mime,
                    name: record.name ?? "剪贴板文件", size: size.intValue,
                    width: record.width, height: record.height, timestamp: record.timestamp, thumb: nil
                ))
                loadedMediaIDs.insert(id)
                return true
            }
            clipboardHistory = KeyTaoClipboardHistory(records: clipboardHistory.records + records, limit: clipboardHistory.limit)
            reconcileClipboardMedia()
            clipboardHistoryLoaded = true
            if clipboardHistory.records != saved {
                persistClipboardHistory()
            }
        } catch {
            keyTaoKeyboardLog.error("Clipboard history could not be loaded")
        }
    }

    private func persistClipboardHistory() {
        clipboardHistoryNeedsSave = true
        guard clipboardHistoryLoaded, let file = clipboardHistoryFile else { return }
        do {
            let data = try clipboardHistory.encoded()
            try FileManager.default.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
            try data.write(to: file, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
            clipboardHistoryNeedsSave = false
        } catch {
            showMessage("剪贴板历史保存失败")
        }
    }

    private func reconcileClipboardMedia() {
        let retained = Set(clipboardHistory.records.filter { $0.mime != nil }.map(\.key))
        clipboardMedia.removeAll { media in
            guard !retained.contains("media:\(media.id)") else { return false }
            if hasFullAccess { try? FileManager.default.removeItem(at: media.file) }
            return true
        }
    }

    private func resetClipboardState() {
        clipboardMediaLifetime.cancel()
        clipboardMediaCapturePending = false
        let completions = clipboardMediaCompletions
        clipboardMediaCompletions.removeAll()
        keyboardView?.clearClipboardItems()
        keyboardView?.update(clipboardSuggestion: nil, copiedAt: 0)
        completions.forEach { $0([]) }
        logClipboardMedia()
    }

    private func logClipboardMedia() {
        KeyTaoLog.event("ui", "clipboard_media") {
            ["count": clipboardMedia.count, "bytes": clipboardMedia.reduce(0, { $0 + $1.size })]
        }
    }

    private func refreshCurrentClipboardText() {
        keyboardView?.update(clipboardFullAccess: hasFullAccess)
        loadClipboardHistoryIfNeeded()
        guard clipboardReadAllowed else {
            keyboardView?.update(clipboardSuggestion: nil, copiedAt: 0)
            return
        }
        let changeCount = UIPasteboard.general.changeCount
        if rememberCurrentClipboard(expectedChangeCount: changeCount) {
            clipboardLastReadChangeCount = changeCount
        } else {
            keyboardView?.update(clipboardSuggestion: nil, copiedAt: 0)
        }
    }

    private func rememberCurrentClipboard(expectedChangeCount: Int) -> Bool {
        guard clipboardCaptureAllowed,
              clipboardMediaSuppressionChangeCount != expectedChangeCount else {
            return false
        }
        let generation = clipboardMediaLifetime.currentGeneration()
        guard let text = currentClipboardText(), text.utf8.count <= KeyTaoClipboardHistory.maxTextBytes else {
            return false
        }
        guard generation == clipboardMediaLifetime.currentGeneration(), clipboardCaptureAllowed,
              UIPasteboard.general.changeCount == expectedChangeCount else { return false }
        if let clipboardSuppression {
            guard text != clipboardSuppression.text
                    || UIPasteboard.general.changeCount != clipboardSuppression.changeCount else {
                return false
            }
            self.clipboardSuppression = nil
        }
        if clipboardObservedChangeCount != expectedChangeCount {
            clipboardObservedChangeCount = expectedChangeCount
            clipboardObservedAt = Date().timeIntervalSince1970
        }
        keyboardView?.update(
            clipboardSuggestion: clipboardAccessPolicy.canSuggest(enabled: clipboardConfig.clipboardSuggestionEnabled) ? text : nil,
            copiedAt: clipboardObservedAt
        )
        if clipboardCaptureAllowed {
            let alreadyRecorded = clipboardLastReadChangeCount == expectedChangeCount &&
                clipboardHistory.records.contains {
                    $0.key == text && $0.text == text && $0.timestamp == clipboardObservedAt
                }
            if !alreadyRecorded {
                clipboardHistory.upsert(KeyTaoClipboardRecord(key: text, text: text, timestamp: clipboardObservedAt))
                reconcileClipboardMedia()
            }
            if !alreadyRecorded || clipboardHistoryNeedsSave {
                persistClipboardHistory()
            }
        }
        return true
    }

    @discardableResult
    private func commitPendingComposition() -> Bool {
        guard !runtimeStarting || queuedCommands.isEmpty else { return false }
        guard currentState.hasComposition else { return true }
        guard engine.nativeReady else { return false }
        let generation = inputContextGeneration
        let documentIdentifier = currentDocumentIdentifier()
        if currentState.isPackageSwitcher {
            apply(engine.processKey(rimeKeyEscape))
            guard inputContextIsCurrent(generation, documentIdentifier: documentIdentifier),
                  !currentState.isPackageSwitcher else { return false }
        }
        if !currentState.candidates.isEmpty || !currentState.candidatePanel.candidates.isEmpty {
            if engineCapabilities.candidateSelection {
                apply(engine.selectCandidate(0))
            } else if engineCapabilities.globalCandidateSelection {
                let index = currentState.candidates.first?.index
                    ?? currentState.candidatePanel.candidates.first?.index
                    ?? currentState.page * currentState.pageSize
                apply(engine.selectCandidateGlobal(index))
            } else {
                showMessage("当前 RIME 运行库不支持提交首选")
                return false
            }
            guard inputContextIsCurrent(generation, documentIdentifier: documentIdentifier) else { return false }
            if currentState.hasComposition {
                apply(engine.commitComposition())
                guard inputContextIsCurrent(generation, documentIdentifier: documentIdentifier) else { return false }
            }
        }
        if currentState.hasComposition {
            // Enter uses the core's raw-input fallback; display preedit is not raw code.
            apply(engine.processEnter())
            guard inputContextIsCurrent(generation, documentIdentifier: documentIdentifier) else { return false }
        }
        return !currentState.hasComposition
    }

    private func commitDirect(_ text: String) {
        guard !text.isEmpty, commitPendingComposition() else { return }
        guard insertHostText(text) else { return }
        if keyboardView?.isEmojiSearching != true {
            lastCommittedText = hostTraits.isSensitive ? nil : text
        }
        currentState = engine.nativeReady ? engine.state().withoutTransientCommit() : currentState.withoutTransientCommit()
        keyboardView?.update(state: currentState)
        keyboardView?.didCommitDirectInput()
    }

    private func apply(_ state: KeyTaoImeState) {
        let keystrokeStarted = pendingKeystrokeStarted
        pendingKeystrokeStarted = nil
        let started = runtimeMetricsEnabled ? KeyTaoLog.now() : nil
        defer {
            if let keystrokeStarted {
                keytao_log_keystroke((KeyTaoLog.now() - keystrokeStarted) * 1000)
            }
            if let started { applyHistogram.record((KeyTaoLog.now() - started) * 1000) }
        }
        let generation = inputContextGeneration
        let documentIdentifier = currentDocumentIdentifier()
        if !state.committed.isEmpty {
            guard insertHostText(state.committed) else { return }
        }
        guard inputContextIsCurrent(generation, documentIdentifier: documentIdentifier) else { return }
        currentState = state.withoutTransientCommit()
        updateHostMarkedText()
        guard inputContextIsCurrent(generation, documentIdentifier: documentIdentifier) else { return }
        keyboardView?.update(state: currentState)
        if let hint = state.switchHint, !hint.isEmpty {
            keyboardView?.showSwitchHint(hint)
        }
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

    @discardableResult
    private func insertHostText(_ text: String, intoSearch: Bool = true) -> Bool {
        guard !text.isEmpty else {
            return true
        }
        if intoSearch, keyboardView?.appendEmojiSearchText(text) == true { return true }
        let generation = inputContextGeneration
        let documentIdentifier = currentDocumentIdentifier()
        backspaceRestoreStack.removeAll()
        clearHostMarkedText()
        guard inputContextIsCurrent(generation, documentIdentifier: documentIdentifier) else { return false }
        withHostTextMutation {
            textDocumentProxy.insertText(text)
        }
        return inputContextIsCurrent(generation, documentIdentifier: documentIdentifier)
    }

    /// iOS 13+ lets a keyboard extension show the pending composition inside the
    /// host field via marked text, which is the standard CJK channel: the host
    /// suppresses autocorrect over it and groups undo around it. When host marked
    /// text is disabled, the candidate bar renders the preedit instead.
    private func updateHostMarkedText() {
        guard !currentState.isPackageSwitcher else { return }
        let preedit = currentState.candidatePanel.preedit ?? currentState.preedit
        guard keyboardView?.isEmojiSearching != true, hostMarkedTextEnabled, !preedit.isEmpty,
              let documentIdentifier = currentDocumentIdentifier() else {
            clearHostMarkedText()
            return
        }
        let selected = markedTextSelectedRange(in: preedit)
        markedTextOwnership.claim(documentIdentifier)
        withHostTextMutation {
            textDocumentProxy.setMarkedText(preedit, selectedRange: selected)
        }
    }

    /// `unmarkText()` only finalises the marked text, it does not remove it, so
    /// the composition has to be replaced with an empty string first. Otherwise
    /// the preedit would stay in the document next to the text Rime commits.
    private func clearHostMarkedText() {
        let generation = inputContextGeneration
        let documentIdentifier = currentDocumentIdentifier()
        guard markedTextOwnership.release(for: documentIdentifier) else {
            return
        }
        withHostTextMutation {
            textDocumentProxy.setMarkedText("", selectedRange: NSRange(location: 0, length: 0))
            guard inputContextIsCurrent(generation, documentIdentifier: documentIdentifier) else { return }
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
        guard engine.algorithmEnabled else {
            refreshInputAvailability()
            return
        }
        if engine.nativeReady {
            refreshInputAvailability()
            return
        }
        guard !runtimeStarting else {
            return
        }
        let resuming = runtimeReleased || runtimeResumeFailed
        runtimeReleased = false
        runtimeStarting = true
        keyboardView?.updateAvailability(message: "正在加载键道方案…")
        engine.ensureReadyAsync { [weak self] ready in
            guard let self else {
                return
            }
            self.runtimeStarting = false
            self.refreshAlgorithmControl()
            self.runtimeResumeFailed = self.engine.algorithmEnabled && resuming && !ready
            self.refreshRuntimeLogCollection()
            self.refreshInputAvailability()
            self.applyInputPolicyForHost()
            self.flushQueuedCommands()
            self.releaseMemoryIfNeeded()
        }
    }

    private func flushQueuedCommands() {
        guard !runtimeStarting, !queuedCommands.isEmpty else {
            return
        }
        guard refreshInputDocumentIfNeeded() else { return }
        let generation = inputContextGeneration
        let documentIdentifier = currentDocumentIdentifier()
        let pending = queuedCommands
        queuedCommands.removeAll()
        guard keyboardView != nil else {
            return
        }
        keyTaoReplayPendingCommands(
            pending,
            isCurrent: { self.inputContextIsCurrent(generation, documentIdentifier: documentIdentifier) },
            perform: { self.performCommand($0) }
        )
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
        let (installed, deployed, deploying) = engine.algorithmEnabled ? engine.schemaAvailability() : (false, false, false)
        let ready = deployed && engine.nativeReady
        if !engine.algorithmEnabled {
            message = "算法引擎已关闭"
        } else if ready {
            message = ""
        } else if deploying {
            message = "正在部署方案…"
        } else if !installed && !hasFullAccess {
            message = "请在系统设置中允许 KeyTao 完全访问"
        } else if !installed {
            message = "请先在 KeyTao App 安装键道方案"
        } else if !deployed {
            message = "请打开 KeyTao App 完成升级"
        } else {
            message = "RIME 运行库未就绪，请重新安装 KeyTao"
        }

        inputAvailable = message.isEmpty
        unavailableMessage = message.isEmpty ? "请先在 KeyTao App 安装键道方案" : message
        if inputAvailable {
            currentState = engine.state().withoutTransientCommit()
            keyboardView?.updateAvailability(message: nil)
            refreshRimeOptions()
        } else {
            currentState = .empty
            if !engine.algorithmEnabled { currentState.asciiMode = true }
            keyboardView?.updateAvailability(message: unavailableMessage)
        }
        keyboardView?.update(state: currentState)
    }

    private var configuredEnglishMode: String {
        baseKeyboardConfig?.englishMode ?? keyboardView?.currentConfig().englishMode ?? "ascii"
    }

    private var usesEnglishSchema: Bool {
        configuredEnglishMode == "schema" && englishSchemaID != nil
    }

    private var isEnglishMode: Bool {
        usesEnglishSchema ? currentRimeSchemaID == englishSchemaID : currentState.asciiMode
    }

    private func reloadIfNeeded() {
        refreshAlgorithmControl()
        guard engine.reloadIfNeeded() else {
            if inputAvailable, keyboardView?.isRimeMenuOpen == true,
               engine.loadedReloadStampSignature() != rimeOptionsReloadStamp {
                refreshRimeOptions()
            }
            return
        }
        refreshEngineCapabilities()
        currentState = engine.state().withoutTransientCommit()
        let systemColorScheme = currentSystemColorScheme()
        updateThemeForCurrentAppearance(systemColorScheme: systemColorScheme)
        applyKeyboardPresentation(config: engine.loadConfig())
        if keyboardView?.isRimeMenuOpen == true { refreshRimeOptions() }
    }

    private func watchAlgorithmControl() {
        guard engineControlWatcher == nil else { return }
        let descriptor = open(engine.userRoot.path, O_EVTONLY)
        guard descriptor >= 0 else { return }
        let source = DispatchSource.makeFileSystemObjectSource(
            fileDescriptor: descriptor,
            eventMask: [.write, .rename, .delete, .revoke],
            queue: .main
        )
        source.setEventHandler { [weak self] in
            guard let self else { return }
            if self.keyboardVisible, self.keyboardView?.isRimeMenuOpen == true {
                self.reloadIfNeeded()
                let progress = self.engine.schemeOperationProgress()
                let schemeWorkFinished = self.keyboardView?.updateSchemeOperationProgress(progress) == true
                if schemeWorkFinished, self.inputAvailable {
                    self.refreshRimeOptions()
                } else if progress == nil, self.engine.algorithmEnabled, !self.engine.nativeReady, !self.runtimeStarting,
                   self.engine.hasDeployedSchema() {
                    self.startRuntimeIfNeeded()
                }
            } else {
                self.refreshAlgorithmControl()
            }
        }
        source.setCancelHandler { close(descriptor) }
        engineControlWatcher = source
        source.resume()
    }

    private func refreshAlgorithmControl() {
        guard engine.refreshAlgorithmEnabled() else { return }
        if engine.algorithmEnabled {
            if keyboardVisible { startRuntimeIfNeeded() }
            return
        }
        if keyboardVisible {
            if refreshInputDocumentIfNeeded() { _ = commitPendingComposition() }
            resetInputContextCaches(resetNavigation: true)
        }
        candidateQueue.sync {}
        engine.suspendAlgorithm()
        runtimeReleased = false
        runtimeResumeFailed = false
        bypassAsciiActive = false
        englishSchemaID = nil
        currentRimeSchemaID = nil
        lastChineseSchemaID = nil
        chineseSwitchSnapshot = [:]
        keyboardView?.update(rimeOptions: .empty)
        refreshInputAvailability()
    }

    // MARK: - Host traits

    /// `textDocumentProxy` inherits `UITextInputTraits`; it is the only channel
    /// through which an iOS host declares what it expects from the keyboard.
    private func applyHostTraits() {
        let traits = KeyTaoHostTraits(proxy: textDocumentProxy)
        keyboardView?.update(clipboardFullAccess: hasFullAccess)
        if !hasFullAccess && clipboardMediaCapturePending {
            resetClipboardState()
        }
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
        let previousForcedLayer = hostTraits.forcedLayer
        hostTraits = traits
        if traits.isSensitive && !previouslySensitive {
            if clipboardConfig.clipboardHideSensitive { resetClipboardState() }
            backspaceRestoreStack.removeAll()
            lastCommittedText = nil
        }
        applyInputPolicyForHost()
        keyboardView?.update(hostTraits: traits)
        if traits.forcedLayer != previousForcedLayer {
            keyboardView?.resetNavigation(forcedLayer: traits.forcedLayer)
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
        let resolved = engine.resolveTheme(systemColorScheme: systemColorScheme ?? currentSystemColorScheme())
        guard resolved.cacheKey == nil || resolved.cacheKey != currentThemeCacheKey else { return }
        let theme = resolved.theme
        currentTheme = theme
        currentThemeCacheKey = resolved.cacheKey
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
        applyKeyboardPresentation(config: config)
    }

    private func applyKeyboardPresentation(config: KeyTaoIOSImeConfig) {
        guard let keyboardView else {
            return
        }
        baseKeyboardConfig = config
        if clipboardHistory.setLimit(clipboardConfigLoaded ? config.clipboardHistoryLimit : nil) {
            reconcileClipboardMedia()
            persistClipboardHistory()
        }
        if !clipboardCaptureAllowed || !config.clipboardSuggestionEnabled {
            keyboardView.update(clipboardSuggestion: nil, copiedAt: 0)
        }
        let isLandscape = currentInterfaceIsLandscape()
        let fallbackProfile = config.floating.profile(isLandscape: isLandscape)
        let state = layoutStateStore.load(isLandscape: isLandscape, fallback: fallbackProfile)
        lastPresentationLandscape = isLandscape
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
        layoutState = normalized
        layoutPresentation = presentation
        let presentedConfig = presentedKeyboardConfig(config, state: normalized, presentation: presentation)
        keyboardView.update(settingsConfig: config)
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
                + keyboardView.emojiSearchExtraHeight
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
                + keyboardView.emojiSearchExtraHeight
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

    private func presentedKeyboardConfig(
        _ config: KeyTaoIOSImeConfig,
        state: KeyTaoIOSKeyboardLayoutState,
        presentation: KeyTaoIOSKeyboardLayoutPresentation
    ) -> KeyTaoIOSImeConfig {
        let profile = keyboardLayoutProfile(state: state, presentation: presentation)
        let presented = config.scaledForFloating(profile, heightScale: state.heightScale)
        guard presentation.mode == .full,
              traitCollection.userInterfaceIdiom == .phone,
              traitCollection.verticalSizeClass == .compact,
              currentInterfaceIsLandscape() else {
            return presented
        }
        let bounds = view.window?.windowScene?.screen.bounds ?? UIScreen.main.bounds
        return presented.fittedToCompactLandscape(screenHeight: bounds.height)
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
        button.tintColor = currentTheme.mobile.keyboard.key.foreground.uiColor
        button.backgroundColor = currentTheme.mobile.keyboard.key.background.uiColor
        button.layer.cornerRadius = currentTheme.mobile.keyboard.key.cornerRadius
        button.layer.borderWidth = currentTheme.mobile.keyboard.key.borderWidth
        button.layer.borderColor = currentTheme.mobile.keyboard.key.borderColor.uiColor.cgColor
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
        container.layer.shadowColor = currentTheme.mobile.keyboard.floating.shadowColor.uiColor.cgColor
        container.layer.shadowOpacity = compact ? 1 : 0
        container.layer.shadowRadius = 0
        container.layer.shadowOffset = compact ? CGSize(width: 0, height: currentTheme.mobile.keyboard.key.shadowOffsetY) : .zero
        container.layer.shadowPath = compact
            ? UIBezierPath(roundedRect: container.bounds, cornerRadius: radius).cgPath
            : nil
    }

    /// Keyboard extensions have no scene of their own, and on iPad the host
    /// window can be a narrow column while the device is landscape, so the
    /// keyboard's own bounds and size classes decide the layout.
    private func currentInterfaceIsLandscape() -> Bool {
        // The keyboard view is always wider than tall, so only the screen tells orientation.
        let bounds = view.window?.windowScene?.screen.bounds ?? UIScreen.main.bounds
        if bounds.width > 0, bounds.height > 0 {
            return bounds.width > bounds.height
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
    var editsHostDocument: Bool {
        switch type {
        case KeyTaoCommandType.backspace,
            KeyTaoCommandType.backspaceGesture,
            KeyTaoCommandType.directInput,
            KeyTaoCommandType.edit:
            return true
        default:
            return false
        }
    }

    var requiresInstalledSchema: Bool {
        switch type {
        case KeyTaoCommandType.openPage,
            KeyTaoCommandType.rimeMenu,
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
