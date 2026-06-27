import Foundation
import CKeytaoCore

enum KeyTaoIOSPaths {
    static let appGroupIdentifier = "group.ink.rea.keytao-app"
    static let reloadStampFileName = "keytao-ime.reload"

    static func userRoot() -> URL {
        if let override = ProcessInfo.processInfo.environment["KEYTAO_IOS_USER_DATA_DIR"], !override.isEmpty {
            return URL(fileURLWithPath: (override as NSString).expandingTildeInPath)
        }
        if let appGroup = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: appGroupIdentifier) {
            return appGroup.appendingPathComponent("keytao", isDirectory: true)
        }
        return applicationSupportRoot().appendingPathComponent("keytao", isDirectory: true)
    }

    static func sharedDataDir(userRoot: URL) -> URL? {
        if let override = ProcessInfo.processInfo.environment["KEYTAO_RIME_SHARED_DATA_DIR"], !override.isEmpty {
            let url = URL(fileURLWithPath: (override as NSString).expandingTildeInPath)
            if hasDefaultYaml(at: url) {
                return url
            }
        }

        let candidates = [
            userRoot,
            userRoot.appendingPathComponent("rime-data", isDirectory: true),
            userRoot.appendingPathComponent("shared", isDirectory: true),
            Bundle.main.resourceURL?.appendingPathComponent("rime-data", isDirectory: true),
            KeyTaoIOSBundle.url(forResource: "rime-data"),
        ].compactMap { $0 }

        return candidates.first { hasDefaultYaml(at: $0) }
    }

    static func themeFile(userRoot: URL) -> URL {
        userRoot.appendingPathComponent("theme.yaml")
    }

    static func keyboardFile(userRoot: URL) -> URL {
        userRoot.appendingPathComponent("keyboard.yaml")
    }

    static func configFile(userRoot: URL) -> URL {
        userRoot.appendingPathComponent("ios_ime.json")
    }

    static func reloadStampFile(userRoot: URL) -> URL {
        userRoot.appendingPathComponent(reloadStampFileName)
    }

    static func hasInstalledSchema(userRoot: URL) -> Bool {
        if hasInstalledSchemaFiles(at: userRoot) {
            return true
        }
        if let sharedData = sharedDataDir(userRoot: userRoot), hasInstalledSchemaFiles(at: sharedData) {
            return true
        }
        return false
    }

    static func seedPackagedRimeDataIfNeeded(userRoot: URL) {
        guard !hasInstalledSchemaFiles(at: userRoot) else {
            return
        }
        guard let packagedData = packagedRimeDataDir(), hasInstalledSchemaFiles(at: packagedData) else {
            return
        }
        copyDirectoryContents(from: packagedData, to: userRoot)
    }

    private static func hasInstalledSchemaFiles(at root: URL) -> Bool {
        let fileManager = FileManager.default
        return fileManager.fileExists(atPath: root.appendingPathComponent("keytao.schema.yaml").path)
            || fileManager.fileExists(atPath: root.appendingPathComponent("build/keytao.schema.yaml").path)
            || fileManager.fileExists(atPath: root.appendingPathComponent("build/keytao.table.bin").path)
    }

    static func ensureUserRoot(_ url: URL) {
        try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    }

    static func seedDefaultKeyboardIfNeeded(userRoot: URL) {
        let url = keyboardFile(userRoot: userRoot)
        guard let yaml = KeyTaoIOSKeyboardConfigResolver.defaultKeyboardYaml() else {
            return
        }
        if FileManager.default.fileExists(atPath: url.path),
           let existing = try? String(contentsOf: url, encoding: .utf8),
           !shouldRefreshDefaultKeyboard(existing: existing, bundled: yaml) {
            return
        }
        do {
            try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            try yaml.write(to: url, atomically: true, encoding: .utf8)
        } catch {
            return
        }
    }

    private static func shouldRefreshDefaultKeyboard(existing: String, bundled: String) -> Bool {
        let trimmed = existing.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, trimmed != bundled else {
            return trimmed.isEmpty
        }
        return existing.contains("# KeyTao IME default keyboard layout.")
            && existing.contains("layers: {}")
            && !existing.contains("symbols_en:")
            && !existing.contains("label: \"英文\"")
            && bundled.contains("symbols_en:")
    }

    private static func hasDefaultYaml(at url: URL) -> Bool {
        FileManager.default.fileExists(atPath: url.appendingPathComponent("default.yaml").path)
    }

    private static func packagedRimeDataDir() -> URL? {
        if let url = Bundle.main.resourceURL?.appendingPathComponent("rime-data", isDirectory: true),
           hasDefaultYaml(at: url) {
            return url
        }
        if let url = KeyTaoIOSBundle.url(forResource: "rime-data"), hasDefaultYaml(at: url) {
            return url
        }
        return nil
    }

    private static func copyDirectoryContents(from source: URL, to destination: URL) {
        let fileManager = FileManager.default
        guard let enumerator = fileManager.enumerator(
            at: source,
            includingPropertiesForKeys: [.isDirectoryKey],
            options: [.skipsHiddenFiles]
        ) else {
            return
        }

        for case let sourceURL as URL in enumerator {
            let relativePath = sourceURL.path.replacingOccurrences(of: source.path + "/", with: "")
            guard !relativePath.isEmpty else {
                continue
            }
            let destinationURL = destination.appendingPathComponent(relativePath)
            let isDirectory = (try? sourceURL.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) ?? false
            do {
                if isDirectory {
                    try fileManager.createDirectory(at: destinationURL, withIntermediateDirectories: true)
                } else {
                    try fileManager.createDirectory(at: destinationURL.deletingLastPathComponent(), withIntermediateDirectories: true)
                    if fileManager.fileExists(atPath: destinationURL.path) {
                        try fileManager.removeItem(at: destinationURL)
                    }
                    try fileManager.copyItem(at: sourceURL, to: destinationURL)
                }
            } catch {
                continue
            }
        }
    }

    private static func applicationSupportRoot() -> URL {
        let fileManager = FileManager.default
        if let url = fileManager.urls(for: .applicationSupportDirectory, in: .userDomainMask).first {
            return url
        }
        return fileManager.temporaryDirectory
    }
}

final class KeyTaoIOSEngine {
    let userRoot: URL
    private let reloadStamp: URL
    private var session: UnsafeMutableRawPointer?
    private var lastState = KeyTaoImeState.empty
    private var lastDisplaySchemaName = ""
    private var reloadStampSignature: String?

    private(set) var nativeReady = false

    init(userRoot: URL = KeyTaoIOSPaths.userRoot()) {
        self.userRoot = userRoot
        self.reloadStamp = KeyTaoIOSPaths.reloadStampFile(userRoot: userRoot)
        self.reloadStampSignature = Self.fileSignature(reloadStamp)
        KeyTaoIOSPaths.ensureUserRoot(userRoot)
        KeyTaoIOSPaths.seedDefaultKeyboardIfNeeded(userRoot: userRoot)
        KeyTaoIOSPaths.seedPackagedRimeDataIfNeeded(userRoot: userRoot)
    }

    deinit {
        close()
    }

    func ensureReady() -> Bool {
        if nativeReady {
            return true
        }
        guard KeyTaoIOSPaths.hasInstalledSchema(userRoot: userRoot) else {
            return false
        }
        return initializeRuntime()
    }

    func hasInstalledSchema() -> Bool {
        KeyTaoIOSPaths.hasInstalledSchema(userRoot: userRoot)
    }

    func resolveTheme(systemColorScheme: KeyTaoEffectiveColorScheme?) -> KeyTaoImeTheme {
        let userTheme = KeyTaoIOSPaths.themeFile(userRoot: userRoot)
        return KeyTaoIOSThemeResolver.resolve(
            userThemePath: FileManager.default.fileExists(atPath: userTheme.path) ? userTheme.path : nil,
            systemColorScheme: systemColorScheme
        )
    }

    func loadConfig(systemColorScheme: KeyTaoEffectiveColorScheme?) -> KeyTaoIOSImeConfig {
        let userKeyboard = KeyTaoIOSPaths.keyboardFile(userRoot: userRoot)
        let userConfig = KeyTaoIOSPaths.configFile(userRoot: userRoot)
        let userTheme = KeyTaoIOSPaths.themeFile(userRoot: userRoot)
        let resolvedThemeJson = KeyTaoIOSThemeResolver.resolveJson(
            userThemePath: FileManager.default.fileExists(atPath: userTheme.path) ? userTheme.path : nil,
            systemColorScheme: systemColorScheme
        )
        let resolvedKeyboardJson = KeyTaoIOSKeyboardConfigResolver.resolveJson(
            userKeyboardPath: FileManager.default.fileExists(atPath: userKeyboard.path) ? userKeyboard.path : nil
        )
        return KeyTaoIOSImeConfig.load(
            resolvedKeyboardJson: resolvedKeyboardJson,
            userConfigURL: FileManager.default.fileExists(atPath: userConfig.path) ? userConfig : nil,
            resolvedThemeJson: resolvedThemeJson
        )
    }

    func state() -> KeyTaoImeState {
        guard let session, let state = decodeState(keytao_session_state_json(session)) else {
            return lastState.withoutTransientCommit()
        }
        lastState = stableSchemaState(state).withoutTransientCommit()
        return lastState
    }

    func processKey(_ keyCode: UInt32, modifiers: UInt32 = 0) -> KeyTaoImeState {
        guard let session, let state = decodeState(keytao_session_process_key_json(session, keyCode, modifiers)) else {
            return KeyTaoImeState.empty
        }
        let stable = stableSchemaState(state)
        lastState = stable.withoutTransientCommit()
        return stable
    }

    func selectCandidate(_ index: Int) -> KeyTaoImeState {
        guard let session, let state = decodeState(keytao_session_select_candidate_json(session, UInt32(max(index, 0)))) else {
            return lastState.withoutTransientCommit()
        }
        let stable = stableSchemaState(state)
        lastState = stable.withoutTransientCommit()
        return stable
    }

    func selectCandidateGlobal(_ index: Int) -> KeyTaoImeState {
        guard let session, let state = decodeState(keytao_session_select_candidate_global_json(session, UInt32(max(index, 0)))) else {
            return lastState.withoutTransientCommit()
        }
        let stable = stableSchemaState(state)
        lastState = stable.withoutTransientCommit()
        return stable
    }

    func allCandidates(limit: Int) -> [KeyTaoCandidate] {
        guard let session, let json = ownedCString(keytao_session_all_candidates_json(session, UInt32(max(limit, 0)))) else {
            return []
        }
        guard let data = json.data(using: .utf8) else {
            return []
        }
        return (try? JSONDecoder().decode([KeyTaoCandidate].self, from: data)) ?? []
    }

    func changePage(backward: Bool) -> KeyTaoImeState {
        guard let session, let state = decodeState(keytao_session_change_page_json(session, backward)) else {
            return lastState.withoutTransientCommit()
        }
        let stable = stableSchemaState(state)
        lastState = stable.withoutTransientCommit()
        return stable
    }

    func reset() -> KeyTaoImeState {
        guard let session, let state = decodeState(keytao_session_reset_json(session)) else {
            return KeyTaoImeState.empty
        }
        let stable = stableSchemaState(state)
        lastState = stable.withoutTransientCommit()
        return stable
    }

    func setAsciiMode(_ enabled: Bool) -> KeyTaoImeState {
        guard let session, let state = decodeState(keytao_session_set_ascii_mode_json(session, enabled)) else {
            var empty = KeyTaoImeState.empty
            empty.asciiMode = enabled
            return empty
        }
        let stable = stableSchemaState(state)
        lastState = stable.withoutTransientCommit()
        return stable
    }

    func reload() -> Bool {
        if !nativeReady && !ensureReady() {
            return false
        }
        let ok = keytao_reload()
        if ok {
            reloadStampSignature = Self.fileSignature(reloadStamp)
            lastState = state().withoutTransientCommit()
        }
        return ok
    }

    func reloadIfNeeded() -> Bool {
        if !nativeReady {
            return ensureReady()
        }
        guard let signature = Self.fileSignature(reloadStamp), signature != reloadStampSignature else {
            return false
        }
        return reload()
    }

    func close() {
        if let session {
            keytao_destroy_session(session)
        }
        session = nil
        nativeReady = false
    }

    private func initializeRuntime() -> Bool {
        guard let sharedDir = KeyTaoIOSPaths.sharedDataDir(userRoot: userRoot) else {
            nativeReady = false
            return false
        }

        let ok = userRoot.path.withCString { userPtr in
            sharedDir.path.withCString { sharedPtr in
                keytao_init(userPtr, sharedPtr)
            }
        }
        guard ok else {
            nativeReady = false
            return false
        }

        if let session {
            keytao_destroy_session(session)
        }
        session = keytao_create_session()
        nativeReady = session != nil
        if nativeReady {
            lastState = state().withoutTransientCommit()
            reloadStampSignature = Self.fileSignature(reloadStamp)
        }
        return nativeReady
    }

    private func decodeState(_ ptr: UnsafeMutablePointer<CChar>?) -> KeyTaoImeState? {
        KeyTaoImeState.decode(json: ownedCString(ptr))
    }

    private func ownedCString(_ ptr: UnsafeMutablePointer<CChar>?) -> String? {
        guard let ptr else {
            return nil
        }
        defer { keytao_free_string(ptr) }
        return String(cString: ptr)
    }

    private func stableSchemaState(_ state: KeyTaoImeState) -> KeyTaoImeState {
        let name = state.schemaName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty, !name.hasPrefix(".") else {
            if lastDisplaySchemaName.isEmpty {
                return state
            }
            var next = state
            next.schemaName = lastDisplaySchemaName
            return next
        }
        lastDisplaySchemaName = name
        return state
    }

    private static func fileSignature(_ url: URL) -> String? {
        guard let attrs = try? FileManager.default.attributesOfItem(atPath: url.path) else {
            return nil
        }
        let size = (attrs[.size] as? NSNumber)?.int64Value ?? 0
        let modified = (attrs[.modificationDate] as? Date)?.timeIntervalSince1970 ?? 0
        return "\(size):\(modified)"
    }
}
