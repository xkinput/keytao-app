import Cocoa
import InputMethodKit
import Carbon
import CKeytaoCore
import QuartzCore

private let rimeModifierShift: UInt32 = 0x0001
private let rimeModifierLock: UInt32 = 0x0002
private let rimeModifierControl: UInt32 = 0x0004
private let rimeModifierAlt: UInt32 = 0x0008
private let rimeModifierSuper: UInt32 = 1 << 26
private let rimeReleaseMask: UInt32 = 1 << 30
private let rimeKeyReturn: UInt32 = 0xff0d
private let rimeKeyKeypadEnter: UInt32 = 0xff8d
private let rimeKeyF4: UInt32 = 0xffc1

private let chordModifiers: NSEvent.ModifierFlags = [.command, .control, .option]

/// KeyTao's IMKInputController subclass.
/// macOS creates one controller per client context and routes key events here.
final class KeyTaoInputController: IMKInputController {

    private static let controllers = NSHashTable<KeyTaoInputController>.weakObjects()
    private static var keyHandlingDepth = 0
    private static var shuttingDown = false

    private var session: UnsafeMutableRawPointer?
    private var candidatePanel: CandidatePanel?
    private var modeIndicatorPanel: ModeIndicatorPanel?
    private var lastModifierFlags: NSEvent.ModifierFlags = []
    private var modifierTap = KeyTaoModifierTap()
    private var hasComposition = false
    private var isActive = false
    private var englishMode = false
    private var asciiMode = false
    private var chordComposer = false
    private var chordReleases = KeyTaoChordReleaseTracker()
    private var chordReleaseTimer: Timer?
    private let chordEventSource = CGEventSource(stateID: .combinedSessionState)
    private var hasMarkedText = false
    private var lastPreeditCursor = 0
    private var lastCursorRect = NSRect.zero
    private var hasLoggedMissingCursorRect = false
    private var keyPanelQueries: PanelQueries?
    private var reloadObserver: NSObjectProtocol?
    private var deploymentObserver: NSObjectProtocol?
    private var showingDeploymentHint = false
    private var normalCandidateModel: KeyTaoPanelModel?
    private let inputContextGeneration = KeyTaoInputContextGeneration()
    private var keyTiming: KeyTiming?

    private struct KeyTiming {
        var clientIPC: UInt64 = 0
        var panelUpdate: UInt64 = 0
    }

    private final class PanelQueries {
        let generation: UInt64
        let client: IMKTextInput?
        var level: NSWindow.Level?
        var caret: NSRect?

        func invalidate() {
            level = nil
            caret = nil
        }

        init(generation: UInt64, client: IMKTextInput?) {
            self.generation = generation
            self.client = client
        }
    }

    // MARK: Lifecycle

    static func prepareForShutdown() {
        shuttingDown = true
        for controller in controllers.allObjects {
            if controller.hasComposition || controller.hasMarkedText {
                controller.endComposition(commit: true, client: controller.client())
            }
            controller.hidePalettes()
        }
        keytao_log_drain_key_stats()
    }

    static func resumeAfterCancelledShutdown() {
        shuttingDown = false
    }

    override init!(server: IMKServer!, delegate: Any!, client: Any!) {
        super.init(server: server, delegate: delegate, client: client)
        Self.controllers.add(self)
        KeyTaoRuntime.shared.start()
        session = KeyTaoRuntime.shared.takeSession()
        reloadObserver = NotificationCenter.default.addObserver(
            forName: KeyTaoRuntime.didReloadNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            self?.runtimeDidReload()
        }
        deploymentObserver = NotificationCenter.default.addObserver(
            forName: KeyTaoRuntime.deploymentDidChangeNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            guard let self, self.isActive else { return }
            self.showDeploymentIndicator(client: self.client())
        }
    }

    deinit {
        chordReleaseTimer?.invalidate()
        if let reloadObserver {
            NotificationCenter.default.removeObserver(reloadObserver)
        }
        if let deploymentObserver {
            NotificationCenter.default.removeObserver(deploymentObserver)
        }
        if let session {
            KeyTaoRuntime.shared.releaseSession(session)
        }
    }

    // MARK: IMKStateSetting

    override func recognizedEvents(_ sender: Any!) -> Int {
        Int(
            NSEvent.EventTypeMask.keyDown.rawValue
                | NSEvent.EventTypeMask.keyUp.rawValue
                | NSEvent.EventTypeMask.flagsChanged.rawValue
        )
    }

    override func activateServer(_ sender: Any!) {
        inputContextGeneration.invalidate()
        resetDeploymentInput()
        clearPendingChordReleases()
        let client = sender as? IMKTextInput
        isActive = true
        lastModifierFlags = NSEvent.modifierFlags.intersection(.deviceIndependentFlagsMask)
        modifierTap.clear()
        KeyTaoRuntime.shared.checkForReload()
        let hadSession = session != nil
        ensureSession()
        if hadSession { KeyTaoRuntime.shared.requestPrewarm() }
        refreshSessionState(from: client)
    }

    override func deactivateServer(_ sender: Any!) {
        defer { keytao_log_drain_key_stats() }
        let generation = inputContextGeneration.invalidate()
        clearPendingChordReleases()
        isActive = false
        resetDeploymentInput()
        // The composition belongs to the client that is going away, so it has to
        // be handed back before the session is dropped; otherwise the client
        // keeps underlined marked text nobody owns any more.
        if hasComposition {
            endComposition(commit: true, client: sender as? IMKTextInput)
        }
        guard inputContextGeneration.isCurrent(generation) else { return }
        hideCandidates()
        hideModeIndicator()
        lastModifierFlags = []
        modifierTap.clear()
        // Wait until IMKit's callback returns, including any reentrant key handling.
        DispatchQueue.main.async {
            guard Self.keyHandlingDepth == 0,
                  !Self.controllers.allObjects.contains(where: {
                      $0.isActive || $0.hasComposition || $0.hasMarkedText
                  }),
                  KeyTaoExecutableUpdateGuard.shared.hasChanged else { return }
            NSLog("KeyTao: executable replaced; exiting for relaunch")
            NSApp.terminate(nil)
        }
    }

    /// The system asks for every visible piece of input method UI to go away.
    override func hidePalettes() {
        resetDeploymentInput()
        hideCandidates()
        hideModeIndicator()
        super.hidePalettes()
    }

    // MARK: Key handling

    /// Called for key events in the client app. Return true only when librime consumes it.
    override func handle(_ event: NSEvent!, client sender: Any!) -> Bool {
        let recordsTiming = event?.type == .keyDown && keytao_log_enabled(1)
        let keyStarted = recordsTiming ? DispatchTime.now().uptimeNanoseconds : 0
        guard !Self.shuttingDown else { return false }
        Self.keyHandlingDepth += 1
        defer { Self.keyHandlingDepth -= 1 }
        guard let event else { return false }

        let previousPanelQueries = keyPanelQueries
        let previousKeyTiming = keyTiming
        keyPanelQueries = PanelQueries(generation: inputContextGeneration.current, client: sender as? IMKTextInput)
        keyTiming = recordsTiming ? KeyTiming() : nil
        defer {
            previousPanelQueries?.invalidate()
            keyPanelQueries = previousPanelQueries
            keyTiming = previousKeyTiming
        }

        if event.type == .flagsChanged {
            return handleFlagsChanged(event, client: sender as? IMKTextInput)
        }
        if event.type == .keyUp {
            guard chordComposer else { return false }
            let pressedKeyval = chordReleases.takeRelease(keyCode: event.keyCode)
            let keyval = pressedKeyval ?? rimeKeyValue(from: event)
            if chordReleases.isEmpty { clearPendingChordReleases() }
            let modifiers = pressedKeyval == nil ? rimeModifiers(from: event.modifierFlags) : 0
            guard KeyTaoChordKeyPolicy.shouldForwardRelease(
                chordComposer: chordComposer, asciiMode: asciiMode,
                keyval: keyval, modifiers: modifiers
            ), let session = ensureSession(),
               let state = KeyTaoImeState.consuming(keytao_session_process_key_json(
                   session, keyval, modifiers | rimeReleaseMask
               )) else { return false }
            apply(state, to: sender)
            return state.accepted
        }
        guard event.type == .keyDown else { return false }
        let generation = inputContextGeneration.current
        let deploymentTheme = deploymentKeyPressed(client: sender as? IMKTextInput)
        guard inputContextGeneration.isCurrent(generation) else { return false }

        // Any ordinary key makes a pending modifier press part of a shortcut.
        modifierTap.clear()

        let modifiers = rimeModifiers(from: event.modifierFlags)
        if modifiers & rimeModifierSuper != 0 {
            // Command chords are reserved by the window system; librime never
            // binds them on macOS.
            return false
        }
        guard !keytao_deployment_blocks_input() else {
            // Nothing usable yet: keep asking, so a missed reload event cannot
            // leave every key swallowed until the next focus change.
            KeyTaoRuntime.shared.requestInitialization()
            return true
        }

        guard let session = ensureSession() else { return false }

        let keyval = rimeKeyValue(from: event)
        if keyval == 0 {
            // Nothing librime can be told about this key. Handing it to the app
            // while marked text is still on screen would interleave the two, so
            // the composition is closed first.
            if hasComposition {
                commitComposition(sender)
            }
            return false
        }
        // The key set lives in keytao-core::key_policy, not here. The cached
        // flag only skips the call in the case the shared rule already answers
        // "never bypass while something is composing".
        if !hasComposition, keytao_key_policy_should_bypass(session, keyval, modifiers) {
            return false
        }

        let usesEnterPath = keytao_key_policy_is_enter(keyval)
            && modifiers & (rimeModifierControl | rimeModifierAlt) == 0
        let theme = deploymentTheme ?? ImeThemeManager.shared.theme()
        let ffiStarted = recordsTiming ? DispatchTime.now().uptimeNanoseconds : 0
        let json = usesEnterPath
            ? keytao_session_process_enter_json(session)
            : keytao_session_process_key_with_panel_json(
                session, keyval, modifiers,
                candidatePanel?.isVisible == true
                    && (!theme.ui.hideCandidatePanel || normalCandidateModel?.isPackageSwitcher == true),
                theme.panel.orientation == .vertical
            )
        let decodeStarted = recordsTiming ? DispatchTime.now().uptimeNanoseconds : 0
        guard let state = KeyTaoImeState.consuming(json) else { return false }
        let decoded = recordsTiming ? DispatchTime.now().uptimeNanoseconds : 0

        chordReleases.record(
            keyCode: event.keyCode, keyval: keyval,
            chordComposer: state.chordComposer, asciiMode: state.asciiMode,
            accepted: state.accepted, now: ProcessInfo.processInfo.systemUptime,
            isPressed: chordEventSource != nil
                && CGEventSource.keyState(.combinedSessionState, key: event.keyCode)
        )
        if !chordReleases.isEmpty && chordReleaseTimer == nil {
            // IMKit does not reliably deliver printable keyUp events.
            let timer = Timer(timeInterval: 0.008, repeats: true) { [weak self] _ in
                self?.pollPendingChordReleases()
            }
            chordReleaseTimer = timer
            RunLoop.main.add(timer, forMode: .common)
        }
        apply(state, to: sender, theme: theme)
        if let timing = keyTiming {
            let elapsed = DispatchTime.now().uptimeNanoseconds - keyStarted
            state.schemaId.withCString { schema in
                keytao_log_keystroke_stages(
                    schema, Double(elapsed) / 1_000_000,
                    Double(decodeStarted - ffiStarted) / 1_000_000,
                    Double(decoded - decodeStarted) / 1_000_000,
                    timing.clientIPC == 0 ? .nan : Double(timing.clientIPC) / 1_000_000,
                    timing.panelUpdate == 0 ? .nan : Double(timing.panelUpdate) / 1_000_000
                )
            }
        }
        return state.accepted
    }

    private func measureKeyStage<T>(_ stage: WritableKeyPath<KeyTiming, UInt64>, _ work: () -> T) -> T {
        guard var timing = keyTiming else { return work() }
        keyTiming = nil
        let started = DispatchTime.now().uptimeNanoseconds
        let result = work()
        timing[keyPath: stage] += DispatchTime.now().uptimeNanoseconds - started
        keyTiming = timing
        return result
    }

    private func pollPendingChordReleases() {
        guard Self.keyHandlingDepth == 0 else { return }
        Self.keyHandlingDepth += 1
        defer { Self.keyHandlingDepth -= 1 }
        guard isActive, chordComposer, !asciiMode, let session, let client = client() else {
            clearPendingChordReleases()
            return
        }
        let generation = inputContextGeneration.current
        let released = chordReleases.poll(now: ProcessInfo.processInfo.systemUptime) { keyCode in
            guard chordEventSource != nil else { return nil }
            return CGEventSource.keyState(.combinedSessionState, key: keyCode)
        }
        if chordReleases.isEmpty { clearPendingChordReleases() }
        for keyval in released {
            guard inputContextGeneration.isCurrent(generation), isActive, chordComposer, !asciiMode else { return }
            guard let state = KeyTaoImeState.consuming(
                keytao_session_process_key_json(session, keyval, rimeReleaseMask)
            ) else { continue }
            keyPanelQueries = PanelQueries(generation: generation, client: client)
            defer { keyPanelQueries = nil }
            apply(state, to: client)
        }
    }

    private func clearPendingChordReleases() {
        chordReleaseTimer?.invalidate()
        chordReleaseTimer = nil
        chordReleases.clear()
    }

    // MARK: Commit / cancel

    override func commitComposition(_ sender: Any!) {
        let generation = inputContextGeneration.invalidate()
        resetDeploymentInput()
        clearPendingChordReleases()
        guard hasComposition else {
            hideCandidates()
            hideModeIndicator()
            return
        }
        endComposition(commit: true, client: sender as? IMKTextInput)
        guard inputContextGeneration.isCurrent(generation) else { return }
        hideCandidates()
        hideModeIndicator()
    }

    override func cancelComposition() {
        let generation = inputContextGeneration.invalidate()
        resetDeploymentInput()
        endComposition(commit: false, client: client())
        guard inputContextGeneration.isCurrent(generation) else { return }
        hideCandidates()
        hideModeIndicator()
    }

    /// Ends the composition session in both directions: librime forgets it and
    /// the client is left without marked text.
    private func endComposition(commit: Bool, client: IMKTextInput?) {
        clearPendingChordReleases()
        let generation = inputContextGeneration.current
        guard let session else {
            clearMarkedText(client: client)
            return
        }

        let json = commit
            ? keytao_session_commit_composition_json(session)
            : keytao_session_clear_composition_json(session)
        if let state = KeyTaoImeState.consuming(json) {
            apply(state, to: client)
        }
        guard inputContextGeneration.isCurrent(generation) else { return }

        if hasComposition {
            // librime kept something composing; the session is over regardless.
            if let state = KeyTaoImeState.consuming(keytao_session_clear_composition_json(session)) {
                englishMode = state.englishMode
            }
            clearMarkedText(client: client)
        }
    }

    // MARK: State application

    private func apply(_ state: KeyTaoImeState, to sender: Any?, theme: ImeTheme = ImeThemeManager.shared.theme()) {
        let client = sender as? IMKTextInput
        let generation = inputContextGeneration.current

        if !state.committed.isEmpty {
            if hasMarkedText {
                guard inputContextGeneration.perform(generation, {
                    clearMarkedText(client: client)
                }) else { return }
            }
            guard inputContextGeneration.perform(generation, {
                keyPanelQueries?.invalidate()
                defer { keyPanelQueries?.invalidate() }
                measureKeyStage(\.clientIPC) {
                    _ = client?.insertText(
                        state.committed,
                        replacementRange: NSRange(location: NSNotFound, length: 0)
                    )
                }
            }) else { return }
        }

        // Publish ownership before the host call, so a reentrant commit/cancel
        // sees the composition being installed and can end it immediately.
        hasComposition = state.hasComposition
        guard inputContextGeneration.perform(generation, {
            updateMarkedText(state, client: client, theme: theme)
        }) else { return }
        hasComposition = state.hasComposition
        englishMode = state.englishMode
        asciiMode = state.asciiMode
        chordComposer = state.chordComposer
        if !chordComposer || asciiMode { clearPendingChordReleases() }

        if state.candidatePanel.candidates.isEmpty && (state.preedit.isEmpty || theme.ui.effectiveEmbeddedComposition) {
            normalCandidateModel = nil
        } else {
            normalCandidateModel = panelModel(for: state, theme: theme)
        }
        KeyTaoRuntime.shared.refreshDeploymentHint()
        showDeploymentIndicator(client: client, theme: theme)
        if let hint = state.switchHint, inputContextGeneration.isCurrent(generation) {
            showModeIndicator(
                KeyTaoModeHintModel(asciiMode: state.englishMode, text: hint),
                client: client,
                theme: theme
            )
        }
    }

    private func clearMarkedText(client: IMKTextInput?) {
        // Client callbacks can synchronously reenter the controller. Release
        // ownership before asking the client to clear our marked text.
        let ownedMarkedText = hasMarkedText
        hasComposition = false
        lastPreeditCursor = 0
        hasMarkedText = false
        guard ownedMarkedText, let client else { return }
        setMarkedText(
            "",
            client: client,
            selectionRange: NSRange(location: 0, length: 0),
            replacementRange: NSRange(location: NSNotFound, length: 0)
        )
    }

    private func updateMarkedText(_ state: KeyTaoImeState, client: IMKTextInput?, theme: ImeTheme) {
        let selectedRange = !hasMarkedText && state.hasComposition && state.preedit.isEmpty
            ? measureKeyStage(\.clientIPC) { client?.selectedRange() ?? NSRange(location: NSNotFound, length: 0) }
            : NSRange(location: NSNotFound, length: 0)
        let decision = KeyTaoMarkedTextPolicy.decide(
            preedit: state.preedit,
            commitPreview: state.commitPreview,
            hasComposition: state.hasComposition,
            isPackageSwitcher: state.isPackageSwitcher,
            embeddedComposition: theme.ui.effectiveEmbeddedComposition,
            hideCandidatePanel: theme.ui.hideCandidatePanel,
            hadMarkedText: hasMarkedText,
            selectedRange: selectedRange
        )
        switch decision {
        case .none:
            if lastPreeditCursor != 0 { keyPanelQueries?.caret = nil }
            lastPreeditCursor = 0
        case .clear:
            clearMarkedText(client: client)
        case let .placeholder(replacementRange):
            lastPreeditCursor = 0
            guard let client else { return }
            // Keep a non-empty marked range so clients suppress default key actions.
            hasMarkedText = true
            setMarkedText(
                KeyTaoMarkedTextPolicy.placeholderText,
                client: client,
                selectionRange: NSRange(location: 0, length: 0),
                replacementRange: replacementRange
            )
        case let .inline(preedit, hidden):
            let length = preedit.utf16.count
            // ImeState counts in Unicode scalars, IMKit's NSRange counts UTF-16.
            lastPreeditCursor = hidden ? length : preedit.keytaoUtf16Offset(fromCharacterOffset: state.cursor)
            guard let client else { return }

            let selectionStart = min(
                preedit.keytaoUtf16Offset(fromCharacterOffset: state.selStart),
                length
            )
            let selectionEnd = min(
                max(preedit.keytaoUtf16Offset(fromCharacterOffset: state.selEnd), selectionStart),
                length
            )

            let marked = NSMutableAttributedString(string: preedit)
            // What Rime already converted, what it is converting now, and what is
            // still raw input each get the style macOS input methods use for it.
            if hidden {
                mark(marked, NSRange(location: 0, length: length), style: kTSMHiliteConvertedText)
            } else if selectionEnd > selectionStart {
                mark(marked, NSRange(location: 0, length: selectionStart), style: kTSMHiliteConvertedText)
                mark(
                    marked,
                    NSRange(location: selectionStart, length: selectionEnd - selectionStart),
                    style: kTSMHiliteSelectedRawText
                )
                mark(
                    marked,
                    NSRange(location: selectionEnd, length: length - selectionEnd),
                    style: kTSMHiliteRawText
                )
            } else {
                mark(marked, NSRange(location: 0, length: selectionStart), style: kTSMHiliteConvertedText)
                mark(
                    marked,
                    NSRange(location: selectionStart, length: length - selectionStart),
                    style: kTSMHiliteSelectedRawText
                )
            }

            hasMarkedText = true
            setMarkedText(
                marked,
                client: client,
                selectionRange: NSRange(location: min(lastPreeditCursor, length), length: 0),
                replacementRange: NSRange(location: NSNotFound, length: 0)
            )
        }
    }

    private func setMarkedText(_ text: Any, client: IMKTextInput, selectionRange: NSRange, replacementRange: NSRange) {
        keyPanelQueries?.invalidate()
        defer { keyPanelQueries?.invalidate() }
        measureKeyStage(\.clientIPC) {
            client.setMarkedText(text, selectionRange: selectionRange, replacementRange: replacementRange)
        }
    }

    private func mark(_ text: NSMutableAttributedString, _ range: NSRange, style: Int) {
        guard range.length > 0, range.location >= 0 else { return }
        guard let attributes = mark(forStyle: style, at: range) as? [NSAttributedString.Key: Any] else {
            return
        }
        text.addAttributes(attributes, range: range)
    }

    // MARK: Candidate window helpers

    private func panelModel(for state: KeyTaoImeState, theme: ImeTheme) -> KeyTaoPanelModel {
        var model = state.candidatePanel
        model.isPackageSwitcher = state.isPackageSwitcher
        if theme.ui.effectiveEmbeddedComposition || state.preedit.isEmpty {
            model.preedit = nil
            model.preeditCaretRange = nil
        } else {
            var scalars = Array(state.preedit.unicodeScalars)
            let cursor = min(max(state.cursor, 0), scalars.count)
            scalars.insert("|", at: cursor)
            model.preedit = String(String.UnicodeScalarView(scalars))
            model.preeditCaretRange = NSRange(
                location: state.preedit.keytaoUtf16Offset(fromCharacterOffset: cursor),
                length: 1
            )
        }
        return model
    }

    private func showCandidates(_ model: KeyTaoPanelModel, client: IMKTextInput?, theme: ImeTheme) {
        let generation = inputContextGeneration.current
        guard isActive, !theme.ui.hideCandidatePanel || model.isPackageSwitcher || showingDeploymentHint else {
            hideCandidates()
            return
        }
        let windowLevel = measureKeyStage(\.clientIPC) { panelLevel(for: client) }
        guard inputContextGeneration.isCurrent(generation), isActive else { return }
        let rect = measureKeyStage(\.clientIPC) { cursorRect(for: client) }
        guard inputContextGeneration.isCurrent(generation), isActive else { return }
        let panel = candidatePanel ?? CandidatePanel()
        candidatePanel = panel

        panel.onSelect = { [weak self, weak client] index in
            guard let self, let client, self.isActive, self.inputContextGeneration.isCurrent(generation) else { return }
            self.handleCandidateSelection(index: index, client: client)
        }
        panel.onPageChange = { [weak self, weak client] backward in
            guard let self, let client, self.isActive, self.inputContextGeneration.isCurrent(generation) else { return }
            self.handlePageChange(backward: backward, client: client)
        }

        measureKeyStage(\.panelUpdate) {
            panel.update(model: model, theme: theme, windowLevel: windowLevel, near: rect)
        }
    }

    private func hideCandidates() {
        candidatePanel?.onSelect = nil
        candidatePanel?.onPageChange = nil
        candidatePanel?.orderOut(nil)
    }

    private func showModeIndicator(_ modeHint: KeyTaoModeHintModel, client: IMKTextInput?, theme: ImeTheme) {
        let generation = inputContextGeneration.current
        guard isActive, !showingDeploymentHint else { return }
        let windowLevel = measureKeyStage(\.clientIPC) { panelLevel(for: client) }
        guard inputContextGeneration.isCurrent(generation), isActive else { return }
        let rect = measureKeyStage(\.clientIPC) { cursorRect(for: client) }
        guard inputContextGeneration.isCurrent(generation), isActive else { return }
        let panel = modeIndicatorPanel ?? ModeIndicatorPanel()
        modeIndicatorPanel = panel
        measureKeyStage(\.panelUpdate) {
            panel.show(modeHint: modeHint, theme: theme, windowLevel: windowLevel, near: rect)
        }
    }

    private func showDeploymentIndicator(
        client: IMKTextInput?, theme: @autoclosure () -> ImeTheme = ImeThemeManager.shared.theme()
    ) {
        if let hint = KeyTaoRuntime.shared.deploymentHint,
           keytao_deployment_blocks_input() || normalCandidateModel?.candidates.isEmpty != false {
            showingDeploymentHint = true
            hideModeIndicator()
            var model = KeyTaoPanelModel.empty
            model.preedit = hint
            showCandidates(model, client: client, theme: theme())
            candidatePanel?.onSelect = nil
            candidatePanel?.onPageChange = nil
            candidatePanel?.displayIfNeeded()
            CATransaction.flush()
        } else {
            showingDeploymentHint = false
            if let model = normalCandidateModel {
                showCandidates(model, client: client, theme: theme())
            } else {
                hideCandidates()
            }
        }
    }

    private func hideModeIndicator() {
        modeIndicatorPanel?.orderOut(nil)
    }

    private func deploymentKeyPressed(client: IMKTextInput?) -> ImeTheme? {
        keytao_deployment_key_pressed()
        KeyTaoRuntime.shared.refreshDeploymentHint()
        guard KeyTaoRuntime.shared.deploymentHint != nil else { return nil }
        let theme = ImeThemeManager.shared.theme()
        showDeploymentIndicator(client: client, theme: theme)
        return theme
    }

    private func resetDeploymentInput() {
        keytao_deployment_reset_input()
        normalCandidateModel = nil
        KeyTaoRuntime.shared.refreshDeploymentHint()
    }

    /// IMKit tells the input method which level the client's window sits at, and
    /// expects self-drawn candidate windows one level above it.
    private func panelLevel(for client: IMKTextInput?) -> NSWindow.Level {
        guard let client else { return .popUpMenu }
        let queries = panelQueries(for: client)
        if let level = queries?.level { return level }
        let clientLevel = Int(client.windowLevel())
        let level = clientLevel > 0
            ? NSWindow.Level(rawValue: max(clientLevel + 1, NSWindow.Level.popUpMenu.rawValue))
            : .popUpMenu
        queries?.level = level
        return level
    }

    private func panelQueries(for client: IMKTextInput?) -> PanelQueries? {
        guard let queries = keyPanelQueries, queries.client === client,
              inputContextGeneration.isCurrent(queries.generation) else { return nil }
        return queries
    }

    private func handleCandidateSelection(index: Int, client: IMKTextInput?) {
        guard isActive, candidatePanel?.isVisible == true, index >= 0 else { return }
        guard let session = ensureSession() else { return }
        guard let state = KeyTaoImeState.consuming(
            keytao_session_select_candidate_json(session, UInt32(index))
        ) else { return }
        apply(state, to: client)
    }

    private func handlePageChange(backward: Bool, client: IMKTextInput?) {
        guard isActive, candidatePanel?.isVisible == true else { return }
        guard let session = ensureSession() else { return }
        guard let state = KeyTaoImeState.consuming(
            keytao_session_change_page_json(session, backward)
        ) else { return }
        apply(state, to: client)
    }

    private func cursorRect(for client: IMKTextInput?) -> NSRect {
        let queries = panelQueries(for: client)
        if let rect = queries?.caret { return rect }
        if let rect = resolveCursorRect(for: client) {
            lastCursorRect = rect
        }
        let rect = lastCursorRect.isUsableTextInputRect ? lastCursorRect : .zero
        queries?.caret = rect
        return rect
    }

    private func resolveCursorRect(for client: IMKTextInput?) -> NSRect? {
        guard let client else { return nil }

        var lineRect = NSRect.zero
        // This IMKit API uses an inline-relative index. Without our own marked
        // text, zero requests the current selection, not document character 0.
        let inlineCursor = hasMarkedText ? lastPreeditCursor : 0
        _ = client.attributes(forCharacterIndex: inlineCursor, lineHeightRectangle: &lineRect)
        if let normalizedLineRect = normalizeTextInputRect(lineRect) {
            return normalizedLineRect
        }

        var attemptedRects: [String] = []
        if !hasLoggedMissingCursorRect {
            attemptedRects.append("lineHeight=\(NSStringFromRect(lineRect))")
        }
        var triedRanges: Set<NSRange> = []
        for query in cursorRectQueries(for: client) where triedRanges.insert(query.range).inserted {
            var actualRange = NSRange(location: NSNotFound, length: 0)
            let rect = client.firstRect(forCharacterRange: query.range, actualRange: &actualRange)
            if let normalizedRect = normalizeTextInputRect(rect) {
                return normalizedRect
            }
            if !hasLoggedMissingCursorRect {
                attemptedRects.append("\(query.source)=\(NSStringFromRect(rect)) actual=\(NSStringFromRange(actualRange))")
            }
        }

        if !hasLoggedMissingCursorRect {
            hasLoggedMissingCursorRect = true
            NSLog("KeyTao: no usable client cursor rect, tried=%@", attemptedRects.joined(separator: " | "))
        }
        return nil
    }

    private func cursorRectQueries(for client: IMKTextInput) -> [(source: String, range: NSRange)] {
        var queries: [(source: String, range: NSRange)] = []

        if hasMarkedText {
            let markedRange = client.markedRange()
            if markedRange.location != NSNotFound {
                let cursor = min(lastPreeditCursor, max(markedRange.length, 0))
                queries.append((
                    source: "markedRange",
                    range: NSRange(location: markedRange.location + cursor, length: 0)
                ))
            }
        }

        let selectedRange = client.selectedRange()
        if selectedRange.location != NSNotFound {
            queries.append((
                source: "selectedRange",
                range: NSRange(location: selectedRange.location, length: 0)
            ))
        }

        queries.append((source: "firstRect", range: NSRange(location: 0, length: 0)))
        return queries
    }

    private func normalizeTextInputRect(_ rect: NSRect) -> NSRect? {
        guard rect.isUsableTextInputRect, NSScreen.screen(containing: rect) != nil,
              !rect.isLikelyMissingTextInputRect else { return nil }
        return rect
    }

    // MARK: Session helpers

    @discardableResult
    private func ensureSession() -> UnsafeMutableRawPointer? {
        if let session {
            if isActive { keytao_set_focused_session(session) }
            return session
        }
        session = KeyTaoRuntime.shared.takeSession() ?? keytao_create_session()
        if session == nil {
            // The runtime is not up yet; retrying happens in the background so
            // this callback stays cheap and keys keep reaching the application.
            KeyTaoRuntime.shared.requestInitialization()
        }
        if let session, isActive {
            keytao_set_focused_session(session)
            KeyTaoRuntime.shared.requestPrewarm()
        }
        return session
    }

    private func runtimeDidReload() {
        let generation = inputContextGeneration.invalidate()
        clearPendingChordReleases()
        modifierTap.clear()
        guard isActive else {
            hasComposition = false
            hideCandidates()
            hideModeIndicator()
            return
        }

        guard inputContextGeneration.isCurrent(generation), let session = ensureSession(),
              let state = KeyTaoImeState.consuming(keytao_session_state_json(session)) else { return }
        apply(state, to: client())
    }

    private func refreshSessionState(from client: IMKTextInput?) {
        guard let session = ensureSession() else {
            if KeyTaoRuntime.shared.deploymentHint != nil {
                showDeploymentIndicator(client: client)
            }
            return
        }
        guard let state = KeyTaoImeState.consuming(keytao_session_state_json(session)) else { return }
        apply(state, to: client)
    }

    // MARK: Modifier handling

    private func handleFlagsChanged(_ event: NSEvent, client: IMKTextInput?) -> Bool {
        let generation = inputContextGeneration.current
        let newFlags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        let previousFlags = lastModifierFlags
        // Install before calling the host. A reentrant focus reset must not
        // later be overwritten by a defer from the obsolete flags callback.
        lastModifierFlags = newFlags
        if Int(event.keyCode) == kVK_CapsLock {
            modifierTap.clear()
            return false
        }
        var deploymentTheme: ImeTheme?
        if !newFlags.subtracting(previousFlags).isEmpty {
            deploymentTheme = deploymentKeyPressed(client: client)
            guard inputContextGeneration.isCurrent(generation) else { return false }
        }
        guard !keytao_deployment_blocks_input() else {
            modifierTap.clear()
            return false
        }

        guard let keyval = modifierTap.update(
            keyCode: event.keyCode,
            previousFlags: previousFlags,
            currentFlags: newFlags,
            time: event.timestamp
        ), let session = ensureSession() else { return false }
        let wasEnglishMode = keytao_session_get_english_mode(session)
        guard let state = KeyTaoImeState.consuming(
            keytao_session_process_modifier_tap_json(session, keyval)
        ) else { return false }
        let theme = deploymentTheme ?? ImeThemeManager.shared.theme()
        apply(state, to: client, theme: theme)
        guard inputContextGeneration.isCurrent(generation) else { return state.accepted }
        let handled = state.accepted || englishMode != wasEnglishMode
        if handled {
            showModeIndicator(state.modeHint, client: client, theme: theme)
        }
        return handled
    }

    // MARK: Input menu

    override func menu() -> NSMenu! {
        let menu = NSMenu()

        let redeploy = NSMenuItem(
            title: NSLocalizedString("Redeploy KeyTao", comment: "Input menu item"),
            action: #selector(redeployKeyTao),
            keyEquivalent: ""
        )
        redeploy.target = self
        menu.addItem(redeploy)

        let openApp = NSMenuItem(
            title: NSLocalizedString("Open KeyTao App", comment: "Input menu item"),
            action: #selector(openKeyTaoApp),
            keyEquivalent: ""
        )
        openApp.target = self
        menu.addItem(openApp)

        return menu
    }

    @objc private func redeployKeyTao() {
        KeyTaoRuntime.shared.reloadNow { succeeded in
            guard succeeded else {
                NSLog("KeyTao: manual runtime redeployment failed")
                return
            }
            NSSound(named: NSSound.Name("Glass"))?.play()
        }
    }

    @objc private func openKeyTaoApp() {
        let workspace = NSWorkspace.shared
        let appURL = workspace.urlForApplication(withBundleIdentifier: "ink.rea.keytao-app")
            ?? URL(fileURLWithPath: "/Applications/KeyTao.app")

        let configuration = NSWorkspace.OpenConfiguration()
        workspace.openApplication(at: appURL, configuration: configuration) { _, error in
            if let error {
                NSLog("KeyTao: failed to open app: %@", error.localizedDescription)
            }
        }
    }

    // MARK: Key code conversion

    private func rimeKeyValue(from event: NSEvent) -> UInt32 {
        switch Int(event.keyCode) {
        case kVK_Return:           return rimeKeyReturn
        case kVK_ANSI_KeypadEnter: return rimeKeyKeypadEnter
        case kVK_Delete:           return 0xff08
        case kVK_ForwardDelete:    return 0xffff
        case kVK_Escape:           return 0xff1b
        case kVK_Space:            return 0x0020
        case kVK_LeftArrow:        return 0xff51
        case kVK_RightArrow:       return 0xff53
        case kVK_UpArrow:          return 0xff52
        case kVK_DownArrow:        return 0xff54
        case kVK_Home:             return 0xff50
        case kVK_End:              return 0xff57
        case kVK_PageUp:           return 0xff55
        case kVK_PageDown:         return 0xff56
        case kVK_Tab:              return 0xff09
        case kVK_F4:               return rimeKeyF4
        default:
            if let scalar = event.charactersIgnoringModifiers?.unicodeScalars.first?.value,
               (UInt32(NSF1FunctionKey)...UInt32(NSF35FunctionKey)).contains(scalar) {
                return 0xffbe + scalar - UInt32(NSF1FunctionKey)
            }
            return keysym(from: event)
        }
    }

    /// Text-to-keysym lives in keytao-core so that every frontend encodes
    /// non-Latin-1 characters the same way instead of colliding with the X11
    /// function key block. 0 means "not something librime can be told about".
    private func keysym(from event: NSEvent) -> UInt32 {
        let text = typedText(from: event)
        guard !text.isEmpty else { return 0 }
        return text.withCString { keytao_text_to_keysym($0) }
    }

    private func typedText(from event: NSEvent) -> String {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        if flags.isDisjoint(with: chordModifiers),
           let text = event.characters,
           !text.isEmpty {
            return text
        }
        return event.charactersIgnoringModifiers ?? event.characters ?? ""
    }

    private func rimeModifiers(from flags: NSEvent.ModifierFlags) -> UInt32 {
        var mask: UInt32 = 0
        if flags.contains(.shift) { mask |= rimeModifierShift }
        if flags.contains(.capsLock) { mask |= rimeModifierLock }
        if flags.contains(.control) { mask |= rimeModifierControl }
        if flags.contains(.option) { mask |= rimeModifierAlt }
        // X11 and Rime call the window system's own modifier Super; on macOS
        // that is Command.
        if flags.contains(.command) { mask |= rimeModifierSuper }
        return mask
    }
}

extension NSRect {
    var isUsableTextInputRect: Bool {
        !isNull
            && origin.x.isFinite
            && origin.y.isFinite
            && size.width.isFinite
            && size.height.isFinite
            && size.width >= 0
            && size.height > 0
            && abs(origin.x) < 100_000
            && abs(origin.y) < 100_000
    }

    var textInputLookupRect: NSRect {
        NSRect(
            x: minX,
            y: minY,
            width: max(width, 1),
            height: max(height, 1)
        )
    }

    var isLikelyMissingTextInputRect: Bool {
        guard let screen = NSScreen.screen(containing: self) else { return false }
        let frame = screen.frame
        let tolerance: CGFloat = 4
        let nearLeft = abs(minX - frame.minX) <= tolerance
        let nearRight = abs(maxX - frame.maxX) <= tolerance
        let nearBottom = abs(minY - frame.minY) <= tolerance
        let nearTop = abs(maxY - frame.maxY) <= tolerance || abs(minY - frame.maxY) <= tolerance
        return (nearLeft || nearRight) && (nearBottom || nearTop)
    }
}
