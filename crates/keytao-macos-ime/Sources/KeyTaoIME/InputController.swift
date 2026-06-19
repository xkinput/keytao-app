import Cocoa
import InputMethodKit
import Carbon
import CKeytaoCore

private let rimeModifierShift: UInt32 = 0x0001
private let rimeModifierControl: UInt32 = 0x0004
private let rimeModifierAlt: UInt32 = 0x0008
private let rimeReleaseMask: UInt32 = 1 << 30

/// KeyTao's IMKInputController subclass.
/// macOS creates one controller per client context and routes key events here.
final class KeyTaoInputController: IMKInputController {

    private var session: UnsafeMutableRawPointer?
    private var candidatePanel: CandidatePanel?
    private var modeIndicatorPanel: ModeIndicatorPanel?
    private var lastModifierFlags: NSEvent.ModifierFlags = []
    private var shiftPressedWithoutKey = false
    private var hasComposition = false
    private var asciiMode = false

    // MARK: Lifecycle

    override init!(server: IMKServer!, delegate: Any!, client: Any!) {
        super.init(server: server, delegate: delegate, client: client)
        ensureEngineReady()
        session = keytao_create_session()
        if session == nil {
            NSLog("KeyTao: failed to create Rime session")
        }
    }

    deinit {
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
        reloadSessionIfNeeded(client: client)
        ensureSession()
        refreshSessionState(from: client)
    }

    override func deactivateServer(_ sender: Any!) {
        resetSession()
        hideCandidates()
        hideModeIndicator()
    }

    // MARK: Key handling

    /// Called for key events in the client app. Return true only when librime consumes it.
    override func handle(_ event: NSEvent!, client sender: Any!) -> Bool {
        guard let event else { return false }
        let client = sender as? IMKTextInput
        reloadSessionIfNeeded(client: client)

        if event.type == .flagsChanged {
            return handleFlagsChanged(event, client: client)
        }

        guard event.type == .keyDown else { return false }
        if event.modifierFlags.contains(.command) {
            return false
        }
        if event.modifierFlags.contains(.shift) {
            shiftPressedWithoutKey = false
        }
        if asciiMode && !hasComposition {
            return false
        }

        guard let session = ensureSession() else { return false }

        let keyval = rimeKeyValue(from: event)
        if keyval == 0 {
            return false
        }

        let modifiers = rimeModifiers(from: event.modifierFlags)
        if shouldBypassWithoutComposition(keyval: keyval, modifiers: modifiers) {
            return false
        }

        guard let statePtr = keytao_session_process_key(session, keyval, modifiers) else {
            return false
        }
        defer { keytao_free_state(statePtr) }

        let state = KeyTaoStateView(statePtr.pointee)
        apply(state, to: sender)
        return state.accepted
    }

    // MARK: Commit / cancel

    override func commitComposition(_ sender: Any!) {
        guard let session = ensureSession() else {
            hideCandidates()
            return
        }

        if let statePtr = keytao_session_process_key(session, UInt32(kVK_Return), 0) {
            defer { keytao_free_state(statePtr) }
            apply(KeyTaoStateView(statePtr.pointee), to: sender)
        }
        hideCandidates()
    }

    override func cancelComposition() {
        resetSession()
        hideCandidates()
    }

    override func mouseDown(
        onCharacterIndex index: Int,
        coordinate point: NSPoint,
        withModifier flags: Int,
        continueTracking keepTracking: UnsafeMutablePointer<ObjCBool>!,
        client sender: Any!
    ) -> Bool {
        keepTracking.pointee = false
        if hasComposition {
            commitComposition(sender)
        }
        return false
    }

    // MARK: State application

    private func apply(_ state: KeyTaoStateView, to sender: Any?) {
        let client = sender as? IMKTextInput

        if !state.committed.isEmpty {
            if hasComposition {
                clearMarkedText(client: client)
            }
            client?.insertText(
                state.committed,
                replacementRange: NSRange(location: NSNotFound, length: 0)
            )
        }

        updateMarkedText(state.preedit, cursor: state.cursor, client: client)
        updateCompositionFlag(state)
        asciiMode = state.asciiMode

        if state.candidates.isEmpty {
            hideCandidates()
        } else {
            showCandidates(state, client: client)
        }
    }

    private func clearMarkedText(client: IMKTextInput?) {
        defer { hasComposition = false }
        guard let client else { return }
        client.setMarkedText(
            "",
            selectionRange: NSRange(location: 0, length: 0),
            replacementRange: NSRange(location: NSNotFound, length: 0)
        )
    }

    private func updateMarkedText(_ preedit: String, cursor: Int, client: IMKTextInput?) {
        guard let client else { return }

        if preedit.isEmpty {
            if hasComposition {
                clearMarkedText(client: client)
            }
            return
        }

        let markedRange = NSRange(location: 0, length: preedit.utf16.count)
        let selection = NSRange(
            location: min(max(cursor, 0), preedit.utf16.count),
            length: 0
        )
        let attrs = mark(forStyle: kTSMHiliteSelectedRawText, at: markedRange)
        let marked = NSAttributedString(
            string: preedit,
            attributes: attrs as? [NSAttributedString.Key: Any]
        )
        client.setMarkedText(
            marked,
            selectionRange: selection,
            replacementRange: NSRange(location: NSNotFound, length: 0)
        )
    }

    private func updateCompositionFlag(_ state: KeyTaoStateView) {
        hasComposition = !state.preedit.isEmpty || !state.candidates.isEmpty
    }

    // MARK: Candidate window helpers

    private func showCandidates(_ state: KeyTaoStateView, client: IMKTextInput?) {
        let panel = candidatePanel ?? CandidatePanel()
        candidatePanel = panel

        panel.onSelect = { [weak self, weak client] index in
            self?.handleCandidateSelection(index: index, client: client)
        }
        panel.onPageChange = { [weak self, weak client] backward in
            self?.handlePageChange(backward: backward, client: client)
        }

        panel.update(
            texts: state.candidates.map(\.text),
            comments: state.candidates.map(\.comment),
            highlightedIndex: state.highlightedCandidateIndex,
            page: state.page,
            isLastPage: state.isLastPage,
            selectKeys: state.selectKeys,
            near: cursorRect(for: client)
        )
    }

    private func hideCandidates() {
        candidatePanel?.orderOut(nil)
    }

    private func showModeIndicator(asciiMode: Bool, client: IMKTextInput?) {
        let panel = modeIndicatorPanel ?? ModeIndicatorPanel()
        modeIndicatorPanel = panel
        panel.show(asciiMode: asciiMode, near: cursorRect(for: client))
    }

    private func hideModeIndicator() {
        modeIndicatorPanel?.orderOut(nil)
    }

    private func handleCandidateSelection(index: Int, client: IMKTextInput?) {
        guard let session = ensureSession() else { return }
        guard let statePtr = keytao_session_select_candidate(session, UInt32(index)) else {
            return
        }
        defer { keytao_free_state(statePtr) }
        apply(KeyTaoStateView(statePtr.pointee), to: client)
    }

    private func handlePageChange(backward: Bool, client: IMKTextInput?) {
        guard let session = ensureSession() else { return }
        guard let statePtr = keytao_session_change_page(session, backward) else {
            return
        }
        defer { keytao_free_state(statePtr) }
        apply(KeyTaoStateView(statePtr.pointee), to: client)
    }

    private func cursorRect(for client: IMKTextInput?) -> NSRect {
        guard let client else { return .zero }

        var lineRect = NSRect.zero
        _ = client.attributes(forCharacterIndex: 0, lineHeightRectangle: &lineRect)
        let normalizedLineRect = normalizeTextInputRect(lineRect, source: "lineHeight")
        if normalizedLineRect.isUsableTextInputRect {
            return normalizedLineRect
        }

        var actualRange = NSRange(location: NSNotFound, length: 0)
        let firstRect = client.firstRect(
            forCharacterRange: NSRange(location: 0, length: 0),
            actualRange: &actualRange
        )
        let normalizedFirstRect = normalizeTextInputRect(firstRect, source: "firstRect")
        if normalizedFirstRect.isUsableTextInputRect {
            return normalizedFirstRect
        }

        NSLog("KeyTao: no usable client cursor rect, line=%@ first=%@", NSStringFromRect(lineRect), NSStringFromRect(firstRect))
        return .zero
    }

    private func normalizeTextInputRect(_ rect: NSRect, source: String) -> NSRect {
        guard rect.isUsableTextInputRect else { return .zero }

        if NSScreen.screen(containing: rect) != nil {
            NSLog("KeyTao: cursor rect %@ %@", source, NSStringFromRect(rect))
            return rect
        }

        guard let windowFrame = frontmostWindowFrame() else {
            NSLog("KeyTao: cursor rect %@ %@ without front window", source, NSStringFromRect(rect))
            return rect
        }

        let bottomLeftConverted = NSRect(
            x: windowFrame.minX + rect.minX,
            y: windowFrame.minY + rect.minY,
            width: rect.width,
            height: rect.height
        )
        if NSScreen.screen(containing: bottomLeftConverted) != nil {
            NSLog("KeyTao: cursor rect %@ %@ -> %@", source, NSStringFromRect(rect), NSStringFromRect(bottomLeftConverted))
            return bottomLeftConverted
        }

        let topLeftConverted = NSRect(
            x: windowFrame.minX + rect.minX,
            y: windowFrame.maxY - rect.maxY,
            width: rect.width,
            height: rect.height
        )
        if NSScreen.screen(containing: topLeftConverted) != nil {
            NSLog("KeyTao: cursor rect %@ %@ -> %@", source, NSStringFromRect(rect), NSStringFromRect(topLeftConverted))
            return topLeftConverted
        }

        NSLog("KeyTao: cursor rect %@ %@ not normalized with window %@", source, NSStringFromRect(rect), NSStringFromRect(windowFrame))
        return rect
    }

    private func frontmostWindowFrame() -> NSRect? {
        guard let pid = NSWorkspace.shared.frontmostApplication?.processIdentifier else {
            return nil
        }
        guard let windowList = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] else {
            return nil
        }

        for info in windowList {
            guard (info[kCGWindowOwnerPID as String] as? pid_t) == pid else { continue }
            guard (info[kCGWindowLayer as String] as? Int) == 0 else { continue }
            guard let bounds = info[kCGWindowBounds as String] as? [String: CGFloat] else { continue }
            guard let x = bounds["X"], let y = bounds["Y"], let width = bounds["Width"], let height = bounds["Height"] else {
                continue
            }
            guard width > 0, height > 0 else { continue }

            let cgRect = NSRect(x: x, y: y, width: width, height: height)
            return cgWindowRectToAppKitRect(cgRect)
        }

        return nil
    }

    private func cgWindowRectToAppKitRect(_ rect: NSRect) -> NSRect {
        let screen = NSScreen.screens.first { screen in
            let cgScreenRect = NSRect(
                x: screen.frame.minX,
                y: NSScreen.globalMaxY - screen.frame.maxY,
                width: screen.frame.width,
                height: screen.frame.height
            )
            return cgScreenRect.intersects(rect)
        } ?? NSScreen.main

        guard let screen else { return rect }
        return NSRect(
            x: rect.minX,
            y: screen.frame.maxY - (rect.minY - (NSScreen.globalMaxY - screen.frame.maxY)) - rect.height,
            width: rect.width,
            height: rect.height
        )
    }

    // MARK: Session helpers

    @discardableResult
    private func ensureSession() -> UnsafeMutableRawPointer? {
        if let session {
            return session
        }
        ensureEngineReady()
        session = keytao_create_session()
        if session == nil {
            NSLog("KeyTao: failed to create Rime session")
        }
        return session
    }

    @discardableResult
    private func reloadSessionIfNeeded(client: IMKTextInput?) -> Bool {
        guard consumeExternalDeployReloadRequest() else {
            return false
        }

        NSLog("KeyTao: external deploy detected, reloading runtime and session")
        if hasComposition {
            clearMarkedText(client: client)
        }
        hideCandidates()
        hideModeIndicator()
        guard reloadEngine() else {
            NSLog("KeyTao: runtime reload failed")
            return false
        }
        hasComposition = false

        refreshSessionState(from: client)
        return true
    }

    private func resetSession() {
        guard let session else {
            hasComposition = false
            return
        }
        if let statePtr = keytao_session_reset(session) {
            let state = KeyTaoStateView(statePtr.pointee)
            asciiMode = state.asciiMode
            keytao_free_state(statePtr)
        }
        hasComposition = false
    }

    private func refreshSessionState(from client: IMKTextInput?) {
        guard let session = ensureSession() else { return }
        guard let statePtr = keytao_session_state(session) else { return }
        defer { keytao_free_state(statePtr) }
        let state = KeyTaoStateView(statePtr.pointee)
        updateCompositionFlag(state)
        asciiMode = state.asciiMode
        if state.candidates.isEmpty {
            hideCandidates()
        } else {
            showCandidates(state, client: client)
        }
    }

    // MARK: Modifier handling

    private func handleFlagsChanged(_ event: NSEvent, client: IMKTextInput?) -> Bool {
        let newFlags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        let changedFlags = lastModifierFlags.symmetricDifference(newFlags)
        defer { lastModifierFlags = newFlags }

        guard changedFlags.contains(.shift) else {
            return false
        }

        if newFlags.contains(.shift) {
            shiftPressedWithoutKey = true
            return false
        }

        let wasSoloShift = shiftPressedWithoutKey
            && !lastModifierFlags.contains(.command)
            && !lastModifierFlags.contains(.control)
            && !lastModifierFlags.contains(.option)
            && !newFlags.contains(.command)
            && !newFlags.contains(.control)
            && !newFlags.contains(.option)
        shiftPressedWithoutKey = false

        guard wasSoloShift else {
            return false
        }
        guard let session = ensureSession() else { return false }

        let keyval: UInt32 = Int(event.keyCode) == kVK_RightShift ? 0xffe2 : 0xffe1
        guard let statePtr = keytao_session_process_key(session, keyval, rimeReleaseMask) else {
            return toggleAsciiMode(client: client)
        }
        defer { keytao_free_state(statePtr) }

        let state = KeyTaoStateView(statePtr.pointee)
        apply(state, to: client)
        if state.accepted {
            showModeIndicator(asciiMode: state.asciiMode, client: client)
            return true
        }
        return toggleAsciiMode(client: client)
    }

    private func toggleAsciiMode(client: IMKTextInput?) -> Bool {
        guard let session = ensureSession() else { return false }
        if hasComposition {
            resetSession()
            updateMarkedText("", cursor: 0, client: client)
            hideCandidates()
        }

        guard let statePtr = keytao_session_set_ascii_mode(session, !asciiMode) else {
            return false
        }
        defer { keytao_free_state(statePtr) }
        let state = KeyTaoStateView(statePtr.pointee)
        apply(state, to: client)
        showModeIndicator(asciiMode: state.asciiMode, client: client)
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
        guard reloadEngine() else {
            NSLog("KeyTao: manual runtime reload failed")
            return
        }
        hasComposition = false
        hideCandidates()
        refreshSessionState(from: nil)
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
        case kVK_Return:        return 0xff0d
        case kVK_Delete:        return 0xff08
        case kVK_ForwardDelete: return 0xffff
        case kVK_Escape:        return 0xff1b
        case kVK_Space:         return 0x0020
        case kVK_LeftArrow:     return 0xff51
        case kVK_RightArrow:    return 0xff53
        case kVK_UpArrow:       return 0xff52
        case kVK_DownArrow:     return 0xff54
        case kVK_Home:          return 0xff50
        case kVK_End:           return 0xff57
        case kVK_PageUp:        return 0xff55
        case kVK_PageDown:      return 0xff56
        case kVK_Tab:           return 0xff09
        default:
            return printableAsciiKeyValue(from: event)
        }
    }

    private func printableAsciiKeyValue(from event: NSEvent) -> UInt32 {
        let text = printableAsciiText(from: event)
        guard let scalar = text.unicodeScalars.first else { return 0 }

        if scalar.value >= 0x20 && scalar.value < 0x7f {
            return scalar.value
        }
        return 0
    }

    private func printableAsciiText(from event: NSEvent) -> String {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        if !flags.contains(.command),
           !flags.contains(.control),
           !flags.contains(.option),
           let text = event.characters,
           let scalar = text.unicodeScalars.first,
           scalar.value >= 0x20,
           scalar.value < 0x7f {
            return text
        }
        return event.charactersIgnoringModifiers ?? event.characters ?? ""
    }

    private func rimeModifiers(from flags: NSEvent.ModifierFlags) -> UInt32 {
        var mask: UInt32 = 0
        if flags.contains(.shift) { mask |= rimeModifierShift }
        if flags.contains(.control) { mask |= rimeModifierControl }
        if flags.contains(.option) { mask |= rimeModifierAlt }
        return mask
    }

    private func shouldBypassWithoutComposition(keyval: UInt32, modifiers: UInt32) -> Bool {
        if hasComposition {
            return false
        }
        if modifiers & (rimeModifierControl | rimeModifierAlt) != 0 {
            return true
        }
        return keyval == 0x0020
            || keyval == 0xff08
            || keyval == 0xffff
            || keyval == 0xff09
            || keyval == 0xff0d
            || keyval == 0xff1b
            || (keyval >= 0xff50 && keyval <= 0xff58)
    }
}

private struct KeyTaoCandidate {
    let text: String
    let comment: String
}

private struct KeyTaoStateView {
    let preedit: String
    let cursor: Int
    let candidates: [KeyTaoCandidate]
    let highlightedCandidateIndex: Int
    let page: Int
    let isLastPage: Bool
    let committed: String
    let selectKeys: String
    let asciiMode: Bool
    let accepted: Bool

    init(_ state: KeytaoState) {
        preedit = state.preedit.map { String(cString: $0) } ?? ""
        cursor = Int(state.cursor)
        highlightedCandidateIndex = Int(state.highlighted_candidate_index)
        page = Int(state.page)
        isLastPage = state.is_last_page
        committed = state.committed.map { String(cString: $0) } ?? ""
        selectKeys = state.select_keys.map { String(cString: $0) } ?? ""
        asciiMode = state.ascii_mode
        accepted = state.accepted

        let count = Int(state.candidate_count)
        var parsedCandidates: [KeyTaoCandidate] = []
        parsedCandidates.reserveCapacity(count)
        for index in 0..<count {
            let text = state.candidate_texts?[index].map { String(cString: $0) } ?? ""
            let comment = state.candidate_comments?[index].map { String(cString: $0) } ?? ""
            parsedCandidates.append(KeyTaoCandidate(text: text, comment: comment))
        }
        candidates = parsedCandidates
    }
}

extension NSRect {
    var isUsableTextInputRect: Bool {
        !isNull
            && !isEmpty
            && origin.x.isFinite
            && origin.y.isFinite
            && size.width.isFinite
            && size.height.isFinite
            && abs(origin.x) < 100_000
            && abs(origin.y) < 100_000
    }
}
