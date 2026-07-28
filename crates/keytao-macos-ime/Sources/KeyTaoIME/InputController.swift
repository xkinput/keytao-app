import Cocoa
import InputMethodKit
import Carbon
import CKeytaoCore

private let rimeModifierShift: UInt32 = 0x0001
private let rimeModifierControl: UInt32 = 0x0004
private let rimeModifierAlt: UInt32 = 0x0008
private let rimeModifierSuper: UInt32 = 1 << 26
private let rimeReleaseMask: UInt32 = 1 << 30
private let rimeKeyReturn: UInt32 = 0xff0d
private let rimeKeyKeypadEnter: UInt32 = 0xff8d
private let rimeKeyF4: UInt32 = 0xffc1
private let rimeKeyShiftLeft: UInt32 = 0xffe1
private let rimeKeyShiftRight: UInt32 = 0xffe2

/// Modifiers whose presence means the key press belongs to a chord, not to a
/// solo `Shift` tap.
private let chordModifiers: NSEvent.ModifierFlags = [.command, .control, .option]

/// KeyTao's IMKInputController subclass.
/// macOS creates one controller per client context and routes key events here.
final class KeyTaoInputController: IMKInputController {

    private var session: UnsafeMutableRawPointer?
    private var candidatePanel: CandidatePanel?
    private var modeIndicatorPanel: ModeIndicatorPanel?
    private var lastModifierFlags: NSEvent.ModifierFlags = []
    private var shiftPressedWithoutKey = false
    private var hasComposition = false
    private var isActive = false
    private var asciiMode = false
    private var lastPreeditCursor = 0
    private var lastCursorRect = NSRect.zero
    private var reloadObserver: NSObjectProtocol?

    // MARK: Lifecycle

    override init!(server: IMKServer!, delegate: Any!, client: Any!) {
        super.init(server: server, delegate: delegate, client: client)
        KeyTaoRuntime.shared.start()
        // Null until the runtime is up; ensureSession() picks it up later
        // instead of blocking this callback on librime initialization.
        session = keytao_create_session()
        reloadObserver = NotificationCenter.default.addObserver(
            forName: KeyTaoRuntime.didReloadNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            self?.runtimeDidReload()
        }
    }

    deinit {
        if let reloadObserver {
            NotificationCenter.default.removeObserver(reloadObserver)
        }
        if let session {
            keytao_destroy_session(session)
        }
    }

    // MARK: IMKStateSetting

    override func recognizedEvents(_ sender: Any!) -> Int {
        Int(
            NSEvent.EventTypeMask.keyDown.rawValue
                | NSEvent.EventTypeMask.flagsChanged.rawValue
        )
    }

    override func activateServer(_ sender: Any!) {
        let client = sender as? IMKTextInput
        isActive = true
        KeyTaoRuntime.shared.checkForReload()
        ensureSession()
        refreshSessionState(from: client)
    }

    override func deactivateServer(_ sender: Any!) {
        // The composition belongs to the client that is going away, so it has to
        // be handed back before the session is dropped; otherwise the client
        // keeps underlined marked text nobody owns any more.
        if hasComposition {
            endComposition(commit: true, client: sender as? IMKTextInput)
        }
        hideCandidates()
        hideModeIndicator()
        isActive = false
        lastModifierFlags = []
        shiftPressedWithoutKey = false
    }

    /// The system asks for every visible piece of input method UI to go away.
    override func hidePalettes() {
        hideCandidates()
        hideModeIndicator()
        super.hidePalettes()
    }

    // MARK: Key handling

    /// Called for key events in the client app. Return true only when librime consumes it.
    override func handle(_ event: NSEvent!, client sender: Any!) -> Bool {
        guard let event else { return false }

        if event.type == .flagsChanged {
            return handleFlagsChanged(event, client: sender as? IMKTextInput)
        }
        guard event.type == .keyDown else { return false }

        // Every key press ends the solo-Shift window, chords included. Deciding
        // this from lastModifierFlags at release time loses Cmd+Shift+key, whose
        // key down the system may never route here.
        shiftPressedWithoutKey = false

        let modifiers = rimeModifiers(from: event.modifierFlags)
        if modifiers & rimeModifierSuper != 0 {
            // Command chords are reserved by the window system; librime never
            // binds them on macOS.
            return false
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
        let json = usesEnterPath
            ? keytao_session_process_enter_json(session)
            : keytao_session_process_key_json(session, keyval, modifiers)
        guard let state = KeyTaoImeState.consuming(json) else { return false }

        apply(state, to: sender)
        return state.accepted
    }

    // MARK: Commit / cancel

    override func commitComposition(_ sender: Any!) {
        guard hasComposition else {
            hideCandidates()
            hideModeIndicator()
            return
        }
        endComposition(commit: true, client: sender as? IMKTextInput)
        hideCandidates()
        hideModeIndicator()
    }

    override func cancelComposition() {
        endComposition(commit: false, client: client())
        hideCandidates()
        hideModeIndicator()
    }

    /// Ends the composition session in both directions: librime forgets it and
    /// the client is left without marked text.
    private func endComposition(commit: Bool, client: IMKTextInput?) {
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

        if hasComposition {
            // librime kept something composing; the session is over regardless.
            if let state = KeyTaoImeState.consuming(keytao_session_clear_composition_json(session)) {
                asciiMode = state.asciiMode
            }
            clearMarkedText(client: client)
        }
    }

    // MARK: State application

    private func apply(_ state: KeyTaoImeState, to sender: Any?) {
        let client = sender as? IMKTextInput
        rememberCursorRect(for: client, reason: "beforeApply")

        if !state.committed.isEmpty {
            if hasComposition {
                clearMarkedText(client: client)
            }
            client?.insertText(
                state.committed,
                replacementRange: NSRange(location: NSNotFound, length: 0)
            )
        }

        updateMarkedText(state, client: client)
        hasComposition = state.hasComposition
        asciiMode = state.asciiMode

        if state.candidatePanel.candidates.isEmpty {
            hideCandidates()
        } else {
            showCandidates(state.candidatePanel, client: client)
        }
    }

    private func clearMarkedText(client: IMKTextInput?) {
        defer {
            hasComposition = false
            lastPreeditCursor = 0
        }
        guard let client else { return }
        client.setMarkedText(
            "",
            selectionRange: NSRange(location: 0, length: 0),
            replacementRange: NSRange(location: NSNotFound, length: 0)
        )
    }

    private func updateMarkedText(_ state: KeyTaoImeState, client: IMKTextInput?) {
        let preedit = state.preedit
        let length = preedit.utf16.count
        // ImeState counts in Unicode scalars, IMKit's NSRange counts UTF-16.
        lastPreeditCursor = preedit.keytaoUtf16Offset(fromCharacterOffset: state.cursor)
        guard let client else { return }

        if preedit.isEmpty {
            if hasComposition {
                clearMarkedText(client: client)
            }
            return
        }

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
        mark(marked, NSRange(location: 0, length: selectionStart), style: kTSMHiliteConvertedText)
        if selectionEnd > selectionStart {
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
            mark(
                marked,
                NSRange(location: selectionStart, length: length - selectionStart),
                style: kTSMHiliteSelectedRawText
            )
        }

        client.setMarkedText(
            marked,
            selectionRange: NSRange(location: min(lastPreeditCursor, length), length: 0),
            replacementRange: NSRange(location: NSNotFound, length: 0)
        )
    }

    private func mark(_ text: NSMutableAttributedString, _ range: NSRange, style: Int) {
        guard range.length > 0, range.location >= 0 else { return }
        guard let attributes = mark(forStyle: style, at: range) as? [NSAttributedString.Key: Any] else {
            return
        }
        text.addAttributes(attributes, range: range)
    }

    // MARK: Candidate window helpers

    private func showCandidates(_ model: KeyTaoPanelModel, client: IMKTextInput?) {
        let panel = candidatePanel ?? CandidatePanel()
        candidatePanel = panel

        panel.onSelect = { [weak self, weak client] index in
            self?.handleCandidateSelection(index: index, client: client)
        }
        panel.onPageChange = { [weak self, weak client] backward in
            self?.handlePageChange(backward: backward, client: client)
        }

        panel.update(
            model: model,
            windowLevel: panelLevel(for: client),
            near: cursorRect(for: client)
        )
    }

    private func hideCandidates() {
        candidatePanel?.orderOut(nil)
    }

    private func showModeIndicator(_ modeHint: KeyTaoModeHintModel, client: IMKTextInput?) {
        let panel = modeIndicatorPanel ?? ModeIndicatorPanel()
        modeIndicatorPanel = panel
        panel.show(
            modeHint: modeHint,
            windowLevel: panelLevel(for: client),
            near: cursorRect(for: client)
        )
    }

    private func hideModeIndicator() {
        modeIndicatorPanel?.orderOut(nil)
    }

    /// IMKit tells the input method which level the client's window sits at, and
    /// expects self-drawn candidate windows one level above it.
    private func panelLevel(for client: IMKTextInput?) -> NSWindow.Level {
        guard let client else { return .popUpMenu }
        let clientLevel = Int(client.windowLevel())
        guard clientLevel > 0 else { return .popUpMenu }
        return NSWindow.Level(rawValue: max(clientLevel + 1, NSWindow.Level.popUpMenu.rawValue))
    }

    private func handleCandidateSelection(index: Int, client: IMKTextInput?) {
        guard let session = ensureSession() else { return }
        guard let state = KeyTaoImeState.consuming(
            keytao_session_select_candidate_json(session, UInt32(index))
        ) else { return }
        apply(state, to: client)
    }

    private func handlePageChange(backward: Bool, client: IMKTextInput?) {
        guard let session = ensureSession() else { return }
        guard let state = KeyTaoImeState.consuming(
            keytao_session_change_page_json(session, backward)
        ) else { return }
        apply(state, to: client)
    }

    private func cursorRect(for client: IMKTextInput?) -> NSRect {
        if let rect = resolveCursorRect(for: client) {
            lastCursorRect = rect
            return rect
        }

        if lastCursorRect.isUsableTextInputRect {
            NSLog("KeyTao: using last cursor rect %@", NSStringFromRect(lastCursorRect))
            return lastCursorRect
        }

        return .zero
    }

    private func rememberCursorRect(for client: IMKTextInput?, reason: String) {
        guard let rect = resolveCursorRect(for: client) else { return }
        lastCursorRect = rect
        NSLog("KeyTao: remembered cursor rect %@ %@", reason, NSStringFromRect(rect))
    }

    private func resolveCursorRect(for client: IMKTextInput?) -> NSRect? {
        guard let client else { return nil }

        var attemptedRects: [String] = []
        var lineRect = NSRect.zero
        _ = client.attributes(forCharacterIndex: lastPreeditCursor, lineHeightRectangle: &lineRect)
        attemptedRects.append("lineHeight=\(NSStringFromRect(lineRect))")
        if let normalizedLineRect = normalizeTextInputRect(lineRect, source: "lineHeight") {
            return normalizedLineRect
        }

        for query in cursorRectQueries(for: client) {
            var actualRange = NSRange(location: NSNotFound, length: 0)
            let rect = client.firstRect(forCharacterRange: query.range, actualRange: &actualRange)
            attemptedRects.append("\(query.source)=\(NSStringFromRect(rect)) actual=\(NSStringFromRange(actualRange))")
            if let normalizedRect = normalizeTextInputRect(rect, source: query.source) {
                return normalizedRect
            }
        }

        NSLog("KeyTao: no usable client cursor rect, tried=%@", attemptedRects.joined(separator: " | "))
        return nil
    }

    private func cursorRectQueries(for client: IMKTextInput) -> [(source: String, range: NSRange)] {
        var queries: [(source: String, range: NSRange)] = []

        let markedRange = client.markedRange()
        if markedRange.location != NSNotFound {
            let cursor = min(lastPreeditCursor, max(markedRange.length, 0))
            queries.append((
                source: "markedRange",
                range: NSRange(location: markedRange.location + cursor, length: 0)
            ))
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

    private func normalizeTextInputRect(_ rect: NSRect, source: String) -> NSRect? {
        guard rect.isUsableTextInputRect else { return nil }
        guard NSScreen.screen(containing: rect) != nil else {
            NSLog("KeyTao: rejected cursor rect %@ %@ outside screens", source, NSStringFromRect(rect))
            return nil
        }
        guard !rect.isLikelyMissingTextInputRect else {
            NSLog("KeyTao: rejected cursor rect %@ %@ near screen corner", source, NSStringFromRect(rect))
            return nil
        }

        NSLog("KeyTao: cursor rect %@ %@", source, NSStringFromRect(rect))
        return rect
    }

    // MARK: Session helpers

    @discardableResult
    private func ensureSession() -> UnsafeMutableRawPointer? {
        if let session {
            return session
        }
        session = keytao_create_session()
        if session == nil {
            // The runtime is not up yet; retrying happens in the background so
            // this callback stays cheap and keys keep reaching the application.
            KeyTaoRuntime.shared.requestInitialization()
        }
        return session
    }

    private func runtimeDidReload() {
        guard isActive else {
            hasComposition = false
            hideCandidates()
            hideModeIndicator()
            return
        }

        let client = client()
        if hasComposition {
            clearMarkedText(client: client)
        }
        hideCandidates()
        hideModeIndicator()
        ensureSession()
        refreshSessionState(from: client)
    }

    private func refreshSessionState(from client: IMKTextInput?) {
        guard let session = ensureSession() else { return }
        guard let state = KeyTaoImeState.consuming(keytao_session_state_json(session)) else { return }
        hasComposition = state.hasComposition
        asciiMode = state.asciiMode
        if state.candidatePanel.candidates.isEmpty {
            hideCandidates()
        } else {
            showCandidates(state.candidatePanel, client: client)
        }
    }

    // MARK: Modifier handling

    private func handleFlagsChanged(_ event: NSEvent, client: IMKTextInput?) -> Bool {
        let newFlags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        let changedFlags = lastModifierFlags.symmetricDifference(newFlags)
        defer { lastModifierFlags = newFlags }

        if !newFlags.isDisjoint(with: chordModifiers) {
            shiftPressedWithoutKey = false
        }

        guard changedFlags.contains(.shift) else {
            return false
        }

        if newFlags.contains(.shift) {
            shiftPressedWithoutKey = newFlags.isDisjoint(with: chordModifiers)
            return false
        }

        let wasSoloShift = shiftPressedWithoutKey
            && lastModifierFlags.isDisjoint(with: chordModifiers)
            && newFlags.isDisjoint(with: chordModifiers)
        shiftPressedWithoutKey = false

        guard wasSoloShift else {
            return false
        }
        guard let session = ensureSession() else { return false }

        let keyval: UInt32 = Int(event.keyCode) == kVK_RightShift ? rimeKeyShiftRight : rimeKeyShiftLeft
        guard let state = KeyTaoImeState.consuming(
            keytao_session_process_key_json(session, keyval, rimeReleaseMask)
        ) else {
            return toggleAsciiMode(client: client)
        }

        apply(state, to: client)
        if state.accepted {
            showModeIndicator(state.modeHint, client: client)
            return true
        }
        return toggleAsciiMode(client: client)
    }

    private func toggleAsciiMode(client: IMKTextInput?) -> Bool {
        guard let session = ensureSession() else { return false }
        if hasComposition {
            endComposition(commit: false, client: client)
            hideCandidates()
        }

        guard let state = KeyTaoImeState.consuming(
            keytao_session_set_ascii_mode_json(session, !asciiMode)
        ) else { return false }
        apply(state, to: client)
        showModeIndicator(state.modeHint, client: client)
        return true
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
        guard KeyTaoRuntime.shared.reloadNow() else {
            NSLog("KeyTao: manual runtime reload failed")
            return
        }
        hasComposition = false
        hideCandidates()
        refreshSessionState(from: client())
        NSSound(named: NSSound.Name("Glass"))?.play()
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
