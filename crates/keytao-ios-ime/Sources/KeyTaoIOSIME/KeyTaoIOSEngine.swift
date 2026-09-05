import Foundation
import CryptoKit
import CKeytaoCore

struct KeyTaoRimeSchema: Codable, Equatable {
    var id: String
    var name: String
}

struct KeyTaoRimeSchemaSwitch: Codable, Equatable {
    var name: String?
    var options: [String]
    var states: [String]
    var reset: Int?

    var optionNames: [String] {
        options.isEmpty ? name.map { [$0] } ?? [] : options
    }
}

struct KeyTaoRimeOptionsState: Equatable {
    var schemas: [KeyTaoRimeSchema]
    var currentSchema: KeyTaoRimeSchema?
    var englishSchemaID: String?
    var switches: [KeyTaoRimeSchemaSwitch]
    var options: [String: Bool]

    static let empty = KeyTaoRimeOptionsState(
        schemas: [],
        currentSchema: nil,
        englishSchemaID: nil,
        switches: [],
        options: [:]
    )
}

enum KeyTaoIOSPaths {
    static let appGroupIdentifier = "group.ink.rea.keytao-app"
    private static let keyboardSeedFileName = ".keyboard.seed"
    private static let legacyDefaultKeyboardHashes: Set<String> = [
        "e4d7aa7445ac138286941d095017ee7d9e397ecc5501cfb744482835538e5329",
        "3ebe95295376bfeffb79c0106f86bc7e3d8631311dae0c595d330ed4c1b2805c",
        "25ae1b176617c64fec16a55b73c9faae8abd17c3eba12d1fc67ec9f66364b854",
        "34475509153894fcd8e53f5d21b1f6d0852b800731d670e9cfe7694cdf64df2c",
    ]

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

    static func hasInstalledSchema(userRoot: URL) -> Bool {
        let schemas = configuredSchemas(userRoot: userRoot)
        return !schemas.isEmpty && schemas.allSatisfy {
            FileManager.default.fileExists(atPath: userRoot.appendingPathComponent("\($0).schema.yaml").path)
        }
    }

    static func hasDeployedSchema(userRoot: URL) -> Bool {
        let schemas = configuredSchemas(userRoot: userRoot)
        let build = userRoot.appendingPathComponent("build", isDirectory: true)
        return !schemas.isEmpty
            && schemas.allSatisfy {
                FileManager.default.fileExists(atPath: userRoot.appendingPathComponent("\($0).schema.yaml").path)
            }
            && schemas.allSatisfy {
                FileManager.default.fileExists(atPath: build.appendingPathComponent("\($0).schema.yaml").path)
            }
    }

    private static func configuredSchemas(userRoot: URL) -> [String] {
        let config = ["default.custom.yaml", "default-custom.yaml"]
            .map { userRoot.appendingPathComponent($0) }
            .first { FileManager.default.fileExists(atPath: $0.path) }
        guard let config, let content = try? String(contentsOf: config, encoding: .utf8) else {
            return []
        }

        return content.split(separator: "\n").compactMap { rawLine in
            let line = rawLine.split(separator: "#", maxSplits: 1).first?
                .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            guard line.hasPrefix("- schema:") else {
                return nil
            }
            let schema = String(line.dropFirst("- schema:".count))
                .trimmingCharacters(in: .whitespacesAndNewlines)
                .trimmingCharacters(in: CharacterSet(charactersIn: "\"'"))
            guard !schema.isEmpty,
                  schema.unicodeScalars.allSatisfy({
                      CharacterSet.alphanumerics.contains($0) || "_-.".unicodeScalars.contains($0)
                  }) else {
                return nil
            }
            return schema
        }.filter { schema in
            ["keytao", "txjx", "xmjd6", "keydo"].contains { schema.hasPrefix($0) }
        }
    }

    static func ensureUserRoot(_ url: URL) {
        try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    }

    /// Drops the bundled layout into the App Group so that users have a
    /// `keyboard.yaml` to edit. A reseed happens only when the sidecar hash
    /// proves the existing file is still the last bundled layout: the extension
    /// must never overwrite a layout the user has customised.
    static func seedDefaultKeyboardIfNeeded(userRoot: URL) {
        let url = keyboardFile(userRoot: userRoot)
        let seedURL = userRoot.appendingPathComponent(keyboardSeedFileName)
        guard let yaml = KeyTaoIOSKeyboardConfigResolver.defaultKeyboardYaml() else {
            return
        }
        let bundledHash = sha256(yaml)
        if FileManager.default.fileExists(atPath: url.path) {
            guard let existing = try? String(contentsOf: url, encoding: .utf8) else {
                return
            }
            if !existing.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                let existingHash = sha256(existing)
                let seededHash = try? String(contentsOf: seedURL, encoding: .utf8)
                    .trimmingCharacters(in: .whitespacesAndNewlines)
                if existingHash == bundledHash {
                    if seededHash != bundledHash {
                        try? bundledHash.write(to: seedURL, atomically: true, encoding: .utf8)
                    }
                    return
                }
                let canReseed: Bool
                if let seededHash, !seededHash.isEmpty {
                    canReseed = existingHash == seededHash
                } else {
                    canReseed = legacyDefaultKeyboardHashes.contains(existingHash)
                }
                guard canReseed else {
                    return
                }
            }
        }
        do {
            try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            try yaml.write(to: url, atomically: true, encoding: .utf8)
            try bundledHash.write(to: seedURL, atomically: true, encoding: .utf8)
        } catch {
            return
        }
    }

    private static func sha256(_ value: String) -> String {
        SHA256.hash(data: Data(value.utf8)).map { String(format: "%02x", $0) }.joined()
    }

    private static func hasDefaultYaml(at url: URL) -> Bool {
        FileManager.default.fileExists(atPath: url.appendingPathComponent("default.yaml").path)
    }

    private static func applicationSupportRoot() -> URL {
        let fileManager = FileManager.default
        if let url = fileManager.urls(for: .applicationSupportDirectory, in: .userDomainMask).first {
            return url
        }
        return fileManager.temporaryDirectory
    }
}

/// What the linked librime can do through its own entry points, decoded from
/// the `KEYTAO_CAP_*` mask.
///
/// A missing capability makes the common layer synthesize a key stroke whose
/// meaning depends on the schema, so a control that needs one has to be hidden
/// instead of quietly typing characters into the composition (D4). Nothing here
/// is hard-coded per platform: the vendored iOS librime happens to be 1.8.5
/// today, but every consumer asks the mask rather than the OS.
struct KeyTaoEngineCapabilities: Equatable {
    var candidateSelection: Bool
    var globalCandidateSelection: Bool
    var candidateHighlight: Bool
    var candidateDeletion: Bool
    var nativePaging: Bool

    /// The ABI-level answer, available before `keytao_init()` and constant for
    /// the life of the process.
    static let current = KeyTaoEngineCapabilities(mask: keytao_engine_capabilities())

    init(mask: UInt32) {
        func has(_ bit: Int32) -> Bool {
            mask & UInt32(bitPattern: bit) != 0
        }
        candidateSelection = has(KEYTAO_CAP_CANDIDATE_SELECTION)
        globalCandidateSelection = has(KEYTAO_CAP_GLOBAL_CANDIDATE_SELECTION)
        candidateHighlight = has(KEYTAO_CAP_CANDIDATE_HIGHLIGHT)
        candidateDeletion = has(KEYTAO_CAP_CANDIDATE_DELETION)
        nativePaging = has(KEYTAO_CAP_NATIVE_PAGING)
    }
}

final class KeyTaoIOSEngine {
    let userRoot: URL
    private var session: UnsafeMutableRawPointer?
    private var lastState = KeyTaoImeState.empty
    private var lastDisplaySchemaName = ""
    private var lastTheme: KeyTaoImeTheme?
    private var lastThemeColorScheme: KeyTaoEffectiveColorScheme?
    private var lastConfig: KeyTaoIOSImeConfig?
    private let startupQueue = DispatchQueue(label: "ink.rea.keytao-app.keyboard.startup", qos: .userInitiated)
    private var startupInFlight = false
    private var startupCompletions: [(Bool) -> Void] = []

    private(set) var nativeReady = false

    /// What the librime behind the current session can do.
    ///
    /// `keytao_session_capabilities()` is the authoritative per-session answer,
    /// but it reports 0 — every control off — for a null or retired handle.
    /// That is the safe answer for a handle that cannot reach librime and the
    /// wrong one for a keyboard whose runtime is still starting: the layout
    /// would drop its paging keys and grow them back a frame later. So the
    /// ABI-level mask stands in until a session exists, and because both are
    /// derived from the same librime the swap is invisible in practice.
    private(set) var capabilities = KeyTaoEngineCapabilities.current

    init(userRoot: URL = KeyTaoIOSPaths.userRoot()) {
        self.userRoot = userRoot
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
        guard KeyTaoIOSPaths.hasDeployedSchema(userRoot: userRoot) else {
            return false
        }
        return initializeRuntime()
    }

    /// Keyboard extensions are launched on demand by the host and must show an
    /// interactive first frame straight away, so the App Group seeding plus
    /// `keytao_init` and the session handshake run on a private serial queue.
    /// Only the resulting session pointer crosses back to the main queue, where
    /// every other member of this class is read and written.
    func ensureReadyAsync(_ completion: @escaping (Bool) -> Void) {
        if nativeReady {
            completion(true)
            return
        }
        startupCompletions.append(completion)
        guard !startupInFlight else {
            return
        }
        startupInFlight = true
        let userRoot = self.userRoot
        startupQueue.async { [weak self] in
            KeyTaoIOSPaths.ensureUserRoot(userRoot)
            KeyTaoIOSPaths.seedDefaultKeyboardIfNeeded(userRoot: userRoot)
            let prepared = Self.prepareRuntime(userRoot: userRoot)
            DispatchQueue.main.async {
                guard let self else {
                    if let prepared {
                        keytao_destroy_session(prepared)
                    }
                    return
                }
                let ready = self.installPreparedSession(prepared)
                self.startupInFlight = false
                let pending = self.startupCompletions
                self.startupCompletions = []
                for completion in pending {
                    completion(ready)
                }
            }
        }
    }

    private static func prepareRuntime(userRoot: URL) -> UnsafeMutableRawPointer? {
        guard KeyTaoIOSPaths.hasInstalledSchema(userRoot: userRoot),
              KeyTaoIOSPaths.hasDeployedSchema(userRoot: userRoot),
              let sharedDir = KeyTaoIOSPaths.sharedDataDir(userRoot: userRoot) else {
            return nil
        }
        let ok = userRoot.path.withCString { userPtr in
            sharedDir.path.withCString { sharedPtr in
                keytao_init(userPtr, sharedPtr)
            }
        }
        guard ok else {
            return nil
        }
        return keytao_create_session()
    }

    private func installPreparedSession(_ prepared: UnsafeMutableRawPointer?) -> Bool {
        if nativeReady {
            // A synchronous ensureReady() won the race; drop the spare session.
            if let prepared {
                keytao_destroy_session(prepared)
            }
            return true
        }
        guard let prepared else {
            nativeReady = false
            return false
        }
        if let session {
            keytao_destroy_session(session)
        }
        session = prepared
        nativeReady = true
        refreshCapabilities()
        lastState = state().withoutTransientCommit()
        return true
    }

    func hasInstalledSchema() -> Bool {
        KeyTaoIOSPaths.hasInstalledSchema(userRoot: userRoot)
    }

    func hasDeployedSchema() -> Bool {
        KeyTaoIOSPaths.hasDeployedSchema(userRoot: userRoot)
    }

    /// Shared App Group files are written by the containing app while the
    /// extension may be reading them, so a resolve that comes back empty (a
    /// half-written YAML) keeps the last good result instead of snapping the UI
    /// back to the built-in defaults.
    func resolveTheme(systemColorScheme: KeyTaoEffectiveColorScheme?) -> KeyTaoImeTheme {
        let userTheme = KeyTaoIOSPaths.themeFile(userRoot: userRoot)
        let path = FileManager.default.fileExists(atPath: userTheme.path) ? userTheme.path : nil
        guard let json = KeyTaoIOSThemeResolver.resolveJson(
            userThemePath: path,
            systemColorScheme: systemColorScheme
        ), let theme = KeyTaoIOSThemeResolver.decode(json: json) else {
            if let lastTheme, lastThemeColorScheme == systemColorScheme {
                return lastTheme
            }
            return KeyTaoIOSThemeResolver.resolve(
                userThemePath: path,
                systemColorScheme: systemColorScheme
            )
        }
        lastTheme = theme
        lastThemeColorScheme = systemColorScheme
        return theme
    }

    func loadConfig() -> KeyTaoIOSImeConfig {
        let userKeyboard = KeyTaoIOSPaths.keyboardFile(userRoot: userRoot)
        let userConfig = KeyTaoIOSPaths.configFile(userRoot: userRoot)
        let resolvedKeyboardJson = KeyTaoIOSKeyboardConfigResolver.resolveJson(
            userKeyboardPath: FileManager.default.fileExists(atPath: userKeyboard.path) ? userKeyboard.path : nil
        )
        if resolvedKeyboardJson == nil, let lastConfig {
            return lastConfig
        }
        let config = KeyTaoIOSImeConfig.load(
            resolvedKeyboardJson: resolvedKeyboardJson,
            userConfigURL: FileManager.default.fileExists(atPath: userConfig.path) ? userConfig : nil
        )
        lastConfig = config
        return config
    }

    func persistToolbarCustomization(order: [String], pinnedCount: Int) -> Bool {
        let url = KeyTaoIOSPaths.configFile(userRoot: userRoot)
        return (try? {
            var root: [String: Any] = [:]
            if FileManager.default.fileExists(atPath: url.path) {
                let data = try Data(contentsOf: url)
                root = try JSONSerialization.jsonObject(with: data) as? [String: Any] ?? [:]
            }
            let normalizedOrder = order.map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
                .filter { !$0.isEmpty }
                .reduce(into: [String]()) { result, id in
                    if !result.contains(id) { result.append(id) }
                }
            root["toolbarActionOrder"] = normalizedOrder
            root["toolbarPinnedCount"] = max(1, min(pinnedCount, 12))
            try FileManager.default.createDirectory(
                at: url.deletingLastPathComponent(),
                withIntermediateDirectories: true
            )
            let data = try JSONSerialization.data(withJSONObject: root, options: [.prettyPrinted, .sortedKeys])
            try data.write(to: url, options: .atomic)
            lastConfig = nil
            return true
        }()) ?? false
    }

    func persistSettings(patch: [String: Any]) -> Bool {
        let url = KeyTaoIOSPaths.configFile(userRoot: userRoot)
        return (try? {
            var root: [String: Any] = [:]
            if FileManager.default.fileExists(atPath: url.path) {
                let data = try Data(contentsOf: url)
                root = try JSONSerialization.jsonObject(with: data) as? [String: Any] ?? [:]
            }
            for (key, value) in patch {
                if key == "haptics.enabled" || key == "haptics.intensity" {
                    var haptics = root["haptics"] as? [String: Any] ?? [:]
                    haptics[String(key.dropFirst("haptics.".count))] = value
                    root["haptics"] = haptics
                } else {
                    root[key] = value
                }
            }
            try FileManager.default.createDirectory(
                at: url.deletingLastPathComponent(),
                withIntermediateDirectories: true
            )
            let data = try JSONSerialization.data(withJSONObject: root, options: [.prettyPrinted, .sortedKeys])
            try data.write(to: url, options: .atomic)
            lastConfig = nil
            return true
        }()) ?? false
    }

    func persistThemeUi(colorScheme: String, accentHex: String?) -> Bool {
        let path = KeyTaoIOSPaths.themeFile(userRoot: userRoot).path
        let written = path.withCString { pathPointer in
            colorScheme.withCString { schemePointer in
                if let accentHex {
                    return accentHex.withCString { accentPointer in
                        keytao_write_theme_ui(pathPointer, schemePointer, accentPointer)
                    }
                }
                return keytao_write_theme_ui(pathPointer, schemePointer, nil)
            }
        }
        if written {
            lastTheme = nil
            lastThemeColorScheme = nil
        }
        return written
    }

    /// Drops everything that can be rebuilt on demand; the session and the
    /// deployed schemas stay put so that typing keeps working.
    func releaseCaches() {
        lastTheme = nil
        lastThemeColorScheme = nil
        lastConfig = nil
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

    func deleteCandidate(_ index: Int) -> (state: KeyTaoImeState, deleted: Bool) {
        guard let session,
              let state = decodeState(keytao_session_delete_candidate_json(session, UInt32(max(index, 0)))) else {
            return (lastState.withoutTransientCommit(), false)
        }
        let stable = stableSchemaState(state)
        let deleted = stable.accepted
        lastState = stable.withoutTransientCommit()
        return (stable, deleted)
    }

    func candidateIsUserPhrase(_ index: Int) -> Bool {
        guard let session else { return false }
        return keytao_session_candidate_is_user_phrase(session, UInt32(max(index, 0)))
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

    func listSchemas() -> [KeyTaoRimeSchema] {
        guard let session,
              let json = ownedCString(keytao_session_list_schemas_json(session)),
              let data = json.data(using: .utf8) else {
            return []
        }
        return (try? JSONDecoder().decode([KeyTaoRimeSchema].self, from: data)) ?? []
    }

    func schemaSwitches() -> [KeyTaoRimeSchemaSwitch] {
        guard let session,
              let json = ownedCString(keytao_session_schema_switches_json(session)),
              let data = json.data(using: .utf8) else {
            return []
        }
        return (try? JSONDecoder().decode([KeyTaoRimeSchemaSwitch].self, from: data)) ?? []
    }

    func currentSchema() -> KeyTaoRimeSchema? {
        guard let session,
              let json = ownedCString(keytao_session_current_schema_json(session)),
              let data = json.data(using: .utf8) else {
            return nil
        }
        return try? JSONDecoder().decode(KeyTaoRimeSchema.self, from: data)
    }

    func selectSchema(_ schemaID: String) -> KeyTaoImeState? {
        guard let session else {
            return nil
        }
        let state = schemaID.withCString {
            decodeState(keytao_session_select_schema_json(session, $0))
        }
        guard let state else {
            return nil
        }
        let stable = stableSchemaState(state)
        lastState = stable.withoutTransientCommit()
        return stable
    }

    func getOption(_ optionName: String) -> Bool {
        guard let session else {
            return false
        }
        return optionName.withCString { keytao_session_get_option(session, $0) }
    }

    func setOption(_ optionName: String, enabled: Bool) -> KeyTaoImeState? {
        guard let session else {
            return nil
        }
        let state = optionName.withCString {
            decodeState(keytao_session_set_option_json(session, $0, enabled))
        }
        guard let state else {
            return nil
        }
        let stable = stableSchemaState(state)
        lastState = stable.withoutTransientCommit()
        return stable
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

    /// Drop the composition without committing it before changing schemas.
    func clearComposition() -> KeyTaoImeState {
        guard let session,
              let state = decodeState(keytao_session_clear_composition_json(session)) else {
            return lastState.withoutTransientCommit()
        }
        let stable = stableSchemaState(state)
        lastState = stable.withoutTransientCommit()
        return stable
    }

    /// The single Enter implementation shared by all five front ends: hand
    /// `XK_Return` to Rime and let the common layer fall back to committing the
    /// raw input when Rime declines it.
    func processEnter() -> KeyTaoImeState {
        guard let session, let state = decodeState(keytao_session_process_enter_json(session)) else {
            return lastState.withoutTransientCommit()
        }
        let stable = stableSchemaState(state)
        lastState = stable.withoutTransientCommit()
        return stable
    }

    /// Flush whatever Rime has converted so far, for the "input context ends,
    /// keep the text" paths.
    func commitComposition() -> KeyTaoImeState {
        guard let session, let state = decodeState(keytao_session_commit_composition_json(session)) else {
            return lastState.withoutTransientCommit()
        }
        let stable = stableSchemaState(state)
        lastState = stable.withoutTransientCommit()
        return stable
    }

    /// Sensitive host fields (D9): stop feeding keys to Rime entirely, so no
    /// preedit, no candidates and no user-dictionary learning can happen.
    @discardableResult
    func setInputPolicy(composing: Bool, learning: Bool) -> KeyTaoImeState {
        guard let session,
              let state = decodeState(keytao_session_set_input_policy_json(session, composing, learning)) else {
            return lastState.withoutTransientCommit()
        }
        let stable = stableSchemaState(state)
        lastState = stable.withoutTransientCommit()
        return stable
    }

    /// Maps a single character to an X11 keysym through the common layer; 0
    /// means "do not hand this to Rime, commit it as-is".
    static func keysym(for text: String) -> UInt32? {
        guard !text.isEmpty else {
            return nil
        }
        let keysym = text.withCString { keytao_text_to_keysym($0) }
        return keysym == 0 ? nil : keysym
    }

    /// Converts a Unicode scalar offset from `ImeState` into the UTF-16 unit
    /// offset UIKit expects.
    static func utf16Offset(in text: String, charOffset: Int) -> Int {
        guard charOffset > 0 else {
            return 0
        }
        return text.withCString { Int(keytao_utf16_offset_from_chars($0, UInt32(charOffset))) }
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
            refreshCapabilities()
            lastState = state().withoutTransientCommit()
        }
        return ok
    }

    /// Stamp detection lives in the common layer (`ReloadStamp`), so the
    /// signature the containing app reports and the one the keyboard compares
    /// come from the same function.
    func reloadIfNeeded() -> Bool {
        guard nativeReady else {
            return false
        }
        guard keytao_reload_if_stamp_changed() else {
            return false
        }
        refreshCapabilities()
        lastState = state().withoutTransientCommit()
        return true
    }

    func close() {
        if let session {
            keytao_destroy_session(session)
        }
        session = nil
        nativeReady = false
        refreshCapabilities()
    }

    private func initializeRuntime() -> Bool {
        guard let prepared = Self.prepareRuntime(userRoot: userRoot) else {
            nativeReady = false
            return false
        }
        if let session {
            keytao_destroy_session(session)
        }
        session = prepared
        nativeReady = true
        refreshCapabilities()
        lastState = state().withoutTransientCommit()
        return true
    }

    /// Re-reads the `KEYTAO_CAP_*` mask for whatever session is installed now.
    /// Called on every session install, drop and reload, so no caller ever has
    /// to reason about which handle a cached mask belonged to.
    private func refreshCapabilities() {
        guard let session else {
            capabilities = .current
            return
        }
        capabilities = KeyTaoEngineCapabilities(mask: keytao_session_capabilities(session))
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
}
