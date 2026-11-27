import Foundation
import CKeytaoCore
import Darwin
import ImageIO
import UIKit
import os

enum KeyTaoLog {
    private struct PendingEvent {
        let category: String
        let name: String
        let duration: Double
        let fields: [String: Any]
    }

    private static let lock = NSLock()
    private static var awaitingNativeInit = true
    private static var pending: [PendingEvent] = []

    static func now() -> TimeInterval { ProcessInfo.processInfo.systemUptime }

    static func flush(_ timeoutMs: UInt32) {
        keytao_log_flush(timeoutMs)
    }

    /// Refresh only at coarse main-thread boundaries; hot paths keep a local Bool.
    static func collectionEnabled() -> Bool {
        lock.lock()
        let waiting = awaitingNativeInit
        lock.unlock()
        return waiting || keytao_log_enabled(1)
    }

    /// Only coarse events call the C bridge. Startup metadata waits for the
    /// existing keytao_init entry point to initialize the ios-ime logger.
    static func event(
        _ category: String,
        _ name: String,
        duration: Double = .nan,
        fields: () -> [String: Any] = { [:] }
    ) {
        lock.lock()
        if awaitingNativeInit {
            if pending.count < 32 {
                var values = fields()
                values["event_uptime"] = now()
                pending.append(PendingEvent(category: category, name: name, duration: duration, fields: values))
            }
            lock.unlock()
            return
        }
        lock.unlock()
        guard keytao_log_enabled(1) else { return }
        write(category, name, duration: duration, fields: fields())
    }

    static func nativeDidInitialize() {
        lock.lock()
        guard awaitingNativeInit else {
            lock.unlock()
            return
        }
        awaitingNativeInit = false
        let buffered = pending
        pending.removeAll(keepingCapacity: false)
        if keytao_log_enabled(1) {
            for event in buffered {
                write(event.category, event.name, duration: event.duration, fields: event.fields)
            }
        }
        lock.unlock()
    }

    private static func write(_ category: String, _ name: String, duration: Double, fields: [String: Any]) {
        guard let data = try? JSONSerialization.data(withJSONObject: fields),
              let json = String(data: data, encoding: .utf8) else { return }
        category.withCString { categoryPointer in
            name.withCString { namePointer in
                json.withCString { fieldsPointer in
                    keytao_log_event(1, categoryPointer, namePointer, duration, fieldsPointer)
                }
            }
        }
    }

    static func configWriteFailed(_ operation: String) {
        event("error", "config_write") { ["operation": operation, "msg": "config_write_failed"] }
    }

    static func memoryFootprint() -> UInt64? {
        var info = task_vm_info_data_t()
        var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<integer_t>.size)
        let capacity = Int(count)
        let result = withUnsafeMutablePointer(to: &info) { pointer in
            pointer.withMemoryRebound(to: integer_t.self, capacity: capacity) {
                task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
            }
        }
        return result == KERN_SUCCESS ? info.phys_footprint : nil
    }

    static func memorySnapshot(_ moment: String) {
        event("memory", "footprint") {
            guard let footprint = memoryFootprint() else { return ["moment": moment, "available": false] }
            let megabytes = Double(footprint) / (1024 * 1024)
            let available: UInt64?
            if #available(iOS 13.0, *) {
                let bytes = UInt64(os_proc_available_memory())
                available = bytes > 0 ? bytes : nil
            } else {
                available = nil
            }
            let limit = available.map { megabytes + Double($0) / (1024 * 1024) } ?? 60
            return [
                "moment": moment, "available": true, "mem_mb": megabytes,
                "limit_pct": megabytes / max(limit, 1) * 100, "limit_mb": limit,
                "limit_estimated": available == nil,
            ]
        }
    }

    static func commandType(_ type: String) -> String {
        switch type {
        case KeyTaoCommandType.input, KeyTaoCommandType.directInput, KeyTaoCommandType.rimeInput,
             KeyTaoCommandType.backspace, KeyTaoCommandType.backspaceGesture, KeyTaoCommandType.enter,
             KeyTaoCommandType.space, KeyTaoCommandType.shift, KeyTaoCommandType.mode,
             KeyTaoCommandType.openPage, KeyTaoCommandType.keyboardPicker, KeyTaoCommandType.nextInputMethod,
             KeyTaoCommandType.keyboardMode, KeyTaoCommandType.nextCandidatePage,
             KeyTaoCommandType.previousCandidatePage, KeyTaoCommandType.reset, KeyTaoCommandType.rimeMenu,
             KeyTaoCommandType.rimeSchema, KeyTaoCommandType.rimeOption, KeyTaoCommandType.edit,
             KeyTaoCommandType.panel, KeyTaoCommandType.floating, KeyTaoCommandType.setting:
            return type
        default:
            return "other"
        }
    }
}

/// Main-thread local samples; no bridge calls or per-sample JSON allocation.
struct KeyTaoDurationHistogram {
    private static let bounds: [Double] = [0.25, 0.5, 1, 2, 4, 8, 16, 32, 64, 128, .infinity]
    private var buckets = [UInt64](repeating: 0, count: 11)
    private var count: UInt64 = 0
    private var maximum = 0.0
    private var over16ms: UInt64 = 0

    mutating func record(_ milliseconds: Double) {
        guard milliseconds.isFinite, milliseconds >= 0 else { return }
        let index = Self.bounds.firstIndex { milliseconds <= $0 } ?? 10
        buckets[index] += 1
        count += 1
        maximum = max(maximum, milliseconds)
        if milliseconds > 16 { over16ms += 1 }
    }

    mutating func drain() -> [String: Any]? {
        guard count > 0 else { return nil }
        func percentile(_ percent: UInt64) -> Double {
            let target = (count * percent + 99) / 100
            var cumulative: UInt64 = 0
            for (index, samples) in buckets.enumerated() {
                cumulative += samples
                if cumulative >= target { return min(Self.bounds[index], maximum) }
            }
            return maximum
        }
        let fields: [String: Any] = [
            "n": count, "p50": percentile(50), "p95": percentile(95),
            "max": maximum, "jank_gt_16ms": over16ms,
        ]
        self = Self()
        return fields
    }
}

struct KeyTaoRimeSchema: Codable, Equatable {
    var id: String
    var name: String
}

struct KeyTaoRimePackage: Decodable, Equatable {
    var packageKey: String
    var displayName: String
    var active: Bool
    var ready: Bool
    var schemas: [KeyTaoRimeSchema]
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

struct KeyTaoKeyboardOption: Equatable {
    var id: String
    var name: String
    var current: Bool
    var recommended: Bool
}

struct KeyTaoRimeOptionsState: Equatable {
    var schemas: [KeyTaoRimeSchema]
    var currentSchema: KeyTaoRimeSchema?
    var englishSchemaID: String?
    var switches: [KeyTaoRimeSchemaSwitch]
    var options: [String: Bool]
    var packages: [KeyTaoRimePackage]
    var keyboards: [KeyTaoKeyboardOption] = []

    static let empty = KeyTaoRimeOptionsState(
        schemas: [],
        currentSchema: nil,
        englishSchemaID: nil,
        switches: [],
        options: [:],
        packages: [],
        keyboards: []
    )
}

enum KeyTaoIOSPaths {
    static let appGroupIdentifier = "group.ink.rea.keytao-app"

    static func userRoot() -> URL {
        if let override = ProcessInfo.processInfo.environment["KEYTAO_IOS_USER_DATA_DIR"], !override.isEmpty {
            return URL(fileURLWithPath: (override as NSString).expandingTildeInPath)
        }
        if let appGroup = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: appGroupIdentifier) {
            return appGroup.appendingPathComponent("keytao", isDirectory: true)
        }
        return applicationSupportRoot().appendingPathComponent("keytao", isDirectory: true)
    }

    static func clipboardDir() -> URL {
        userRoot().appendingPathComponent("clipboard", isDirectory: true)
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

    static func schemaAvailability(
        userRoot: URL,
        sharedDir: URL? = nil
    ) -> (installed: Bool, deployed: Bool, legacyDeploying: Bool?) {
        let state = userRoot.path.withCString { user in
            guard let shared = sharedDir ?? sharedDataDir(userRoot: userRoot) else {
                return keytao_isolated_schema_state_at(user, nil)
            }
            return shared.path.withCString { keytao_isolated_schema_state_at(user, $0) }
        }
        if state >= 0 { return (state > 0, state == 2, nil) }

        let schemas = configuredSchemas(userRoot: userRoot)
        let build = userRoot.appendingPathComponent("build", isDirectory: true)
        let deploying = FileManager.default.fileExists(atPath: build.appendingPathComponent(".keytao-deploying").path)
        let installed = !schemas.isEmpty && schemas.allSatisfy {
            FileManager.default.fileExists(atPath: userRoot.appendingPathComponent("\($0).schema.yaml").path)
        }
        guard installed, !deploying else { return (installed, false, deploying) }
        let bundle = Bundle.main
        let coreVersion = bundle.url(forResource: "keytao-core-version", withExtension: nil)
            .flatMap { try? String(contentsOf: $0, encoding: .utf8) }
        let version = bundle.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String
        let buildNumber = bundle.object(forInfoDictionaryKey: "CFBundleVersion") as? String
        let bundleStamp = version.flatMap { version in buildNumber.map { "\(version)/\($0)" } }
        let sharedStamp = try? String(
            contentsOf: userRoot.appendingPathComponent("rime-data/.bundled-version"), encoding: .utf8
        )
        let deployed = KeyTaoIOSDeploymentState.isCurrent(
            marker: try? Data(contentsOf: build.appendingPathComponent(".keytao-deployed")),
            coreVersion: coreVersion,
            bundleStamp: bundleStamp,
            sharedStamp: sharedStamp,
            schemas: schemas,
            deploying: deploying
        ) && schemas.allSatisfy {
            FileManager.default.fileExists(atPath: build.appendingPathComponent("\($0).schema.yaml").path)
        }
        return (installed, deployed, deploying)
    }

    private static func configuredSchemas(userRoot: URL) -> [String] {
        let config = ["default.custom.yaml", "default-custom.yaml"]
            .map { userRoot.appendingPathComponent($0) }
            .first { FileManager.default.fileExists(atPath: $0.path) }
        guard let config, let content = try? String(contentsOf: config, encoding: .utf8) else {
            return []
        }

        let schemas: [String] = content.split(separator: "\n").compactMap { rawLine in
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
        }
        let primarySchemas = schemas.filter { schema in
            ["keytao", "txjx", "xmjd6", "keydo"].contains { schema.hasPrefix($0) }
        }
        return primarySchemas.isEmpty
            ? schemas.filter { ["easy_en", "wanxiang"].contains($0) }
            : primarySchemas
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
/// is hard-coded per platform: every consumer asks the mask rather than the OS.
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
    private struct SchemaLogoKey: Equatable {
        var id: String
        var modificationTime: Date?
        var generation: UInt64
    }

    let userRoot: URL
    private var schemaLogoGeneration: UInt64 = 0
    private var schemaLogoKey: SchemaLogoKey?
    private var schemaLogoRequest: SchemaLogoKey?
    private var schemaLogoLastCheckUptime: TimeInterval = 0
    private(set) var schemaLogoImage: UIImage?
    var onSchemaLogoChanged: ((UIImage?) -> Void)?
    private var session: UnsafeMutableRawPointer?
    private var suspendedSession: (schemaID: String, options: [String: Bool], configurationSignature: [String])?
    private var lastState = KeyTaoImeState.empty
    private var lastDisplaySchemaName = ""
    private var lastDisplaySchemaId = ""
    private var lastTheme: KeyTaoImeTheme?
    private var lastThemeSignature: String?
    private var lastThemeColorScheme: KeyTaoEffectiveColorScheme?
    private var lastConfig: KeyTaoIOSImeConfig?
    private var lastConfigSignature: String?
    private(set) var clipboardConfigLoaded = false
    private let startupQueue = DispatchQueue(label: "ink.rea.keytao-app.keyboard.startup", qos: .userInitiated)
    private var startupInFlight = false
    private var algorithmGeneration: UInt64 = 0
    private var startupCompletions: [(Bool) -> Void] = []
    private var reloadCheckHistogram = KeyTaoDurationHistogram()
    private var runtimeMetricsEnabled = true

    private(set) var nativeReady = false
    private(set) var algorithmEnabled: Bool

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
        self.algorithmEnabled = Self.readAlgorithmEnabled(userRoot: userRoot)
    }

    deinit {
        close()
    }

    /// Keyboard extensions are launched on demand by the host and must show an
    /// interactive first frame straight away, so `keytao_init` and the session
    /// handshake run on a private serial queue. The app owns shared-data seeding.
    /// Only the resulting session pointer crosses back to the main queue, where
    /// every other member of this class is read and written.
    func ensureReadyAsync(_ completion: @escaping (Bool) -> Void) {
        let started = KeyTaoLog.now()
        guard algorithmEnabled else {
            completion(false)
            return
        }
        if nativeReady {
            KeyTaoLog.event("rime", "ensure_ready_async", duration: (KeyTaoLog.now() - started) * 1000) {
                ["ok": true, "cached": true]
            }
            completion(true)
            return
        }
        startupCompletions.append(completion)
        guard !startupInFlight else {
            return
        }
        startupInFlight = true
        let userRoot = self.userRoot
        let generation = algorithmGeneration
        startupQueue.async { [weak self] in
            let prepared = Self.prepareRuntime(userRoot: userRoot)
            DispatchQueue.main.async {
                guard let self else {
                    if let prepared {
                        keytao_destroy_session(prepared)
                    }
                    KeyTaoLog.event("rime", "ensure_ready_async", duration: (KeyTaoLog.now() - started) * 1000) {
                        ["ok": false, "cancelled": true]
                    }
                    return
                }
                let ready: Bool
                if generation != self.algorithmGeneration {
                    if let prepared { keytao_destroy_session(prepared) }
                    self.startupInFlight = false
                    if self.algorithmEnabled {
                        let pending = self.startupCompletions
                        self.startupCompletions = []
                        for completion in pending { self.ensureReadyAsync(completion) }
                        return
                    }
                    ready = false
                } else {
                    ready = self.installPreparedSession(prepared)
                }
                self.startupInFlight = false
                KeyTaoLog.event("rime", "ensure_ready_async", duration: (KeyTaoLog.now() - started) * 1000) {
                    ["ok": ready, "cancelled": false]
                }
                let pending = self.startupCompletions
                self.startupCompletions = []
                for completion in pending {
                    completion(ready)
                }
            }
        }
    }

    private static func prepareRuntime(userRoot: URL) -> UnsafeMutableRawPointer? {
        let started = KeyTaoLog.now()
        var ready = false
        defer {
            KeyTaoLog.event("rime", "prepare_runtime", duration: (KeyTaoLog.now() - started) * 1000) {
                ["ok": ready]
            }
        }
        guard readAlgorithmEnabled(userRoot: userRoot),
              let sharedDir = KeyTaoIOSPaths.sharedDataDir(userRoot: userRoot),
              KeyTaoIOSPaths.schemaAvailability(userRoot: userRoot, sharedDir: sharedDir).deployed else {
            return nil
        }
        let initStarted = KeyTaoLog.now()
        let ok = userRoot.path.withCString { userPtr in
            sharedDir.path.withCString { sharedPtr in
                keytao_init(userPtr, sharedPtr)
            }
        }
        let initDuration = (KeyTaoLog.now() - initStarted) * 1000
        KeyTaoLog.nativeDidInitialize()
        KeyTaoLog.event("rime", "keytao_init", duration: initDuration) { ["ok": ok] }
        guard ok else {
            return nil
        }
        let sessionStarted = KeyTaoLog.now()
        let prepared = keytao_create_session()
        ready = prepared != nil
        KeyTaoLog.event("rime", "keytao_create_session", duration: (KeyTaoLog.now() - sessionStarted) * 1000) {
            ["ok": ready]
        }
        return prepared
    }

    private func installPreparedSession(_ prepared: UnsafeMutableRawPointer?) -> Bool {
        refreshRuntimeLogCollection()
        guard Self.readAlgorithmEnabled(userRoot: userRoot) else {
            if let prepared { keytao_destroy_session(prepared) }
            return false
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
        let suspendedSession = self.suspendedSession
        self.suspendedSession = nil
        if let suspendedSession,
           suspendedSession.configurationSignature == sessionConfigurationSignature(),
           let restored = keytao_create_session() {
            // Keep the default session untouched until the entire restore succeeds.
            session = restored
            var restoredSuccessfully = selectSchema(suspendedSession.schemaID) != nil
            if restoredSuccessfully {
                // Clear disabled alternatives before restoring enabled radio choices.
                for (name, enabled) in suspendedSession.options.sorted(by: { !$0.value && $1.value }) {
                    if setOption(name, enabled: enabled) == nil {
                        restoredSuccessfully = false
                        break
                    }
                }
            }
            if restoredSuccessfully {
                keytao_destroy_session(prepared)
            } else {
                keytao_destroy_session(restored)
                session = prepared
            }
        }
        refreshCapabilities()
        lastState = state().withoutTransientCommit()
        return true
    }

    func schemaAvailability() -> (installed: Bool, deployed: Bool, deploying: Bool) {
        let availability = KeyTaoIOSPaths.schemaAvailability(userRoot: userRoot)
        let deploying = !(nativeReady && availability.deployed) && (
            availability.legacyDeploying ?? userRoot.path.withCString { keytao_deployment_in_progress_at($0) }
        )
        return (availability.installed, availability.deployed, deploying)
    }

    func hasDeployedSchema() -> Bool {
        KeyTaoIOSPaths.schemaAvailability(userRoot: userRoot).deployed
    }

    private static func readAlgorithmEnabled(userRoot: URL) -> Bool {
        userRoot.path.withCString { keytao_engine_enabled_at($0) }
    }

    @discardableResult
    func refreshAlgorithmEnabled() -> Bool {
        let enabled = Self.readAlgorithmEnabled(userRoot: userRoot)
        guard enabled != algorithmEnabled else { return false }
        algorithmEnabled = enabled
        algorithmGeneration &+= 1
        return true
    }

    func suspendAlgorithm() {
        close()
        suspendedSession = nil
        lastState = .empty
        let userRoot = self.userRoot
        startupQueue.async {
            if !Self.readAlgorithmEnabled(userRoot: userRoot) {
                _ = keytao_reload()
            }
        }
    }

    /// Shared App Group files are written by the containing app while the
    /// extension may be reading them, so a resolve that comes back empty (a
    /// half-written YAML) keeps the last good result instead of snapping the UI
    /// back to the built-in defaults.
    func resolveTheme(systemColorScheme: KeyTaoEffectiveColorScheme?) -> (theme: KeyTaoImeTheme, cacheKey: String?) {
        let userTheme = KeyTaoIOSPaths.themeFile(userRoot: userRoot)
        let themeSignature = fileSignature(userTheme)
        let signature = "\(themeSignature)|\(systemColorScheme?.rawValue ?? "auto")"
        if let lastTheme, lastThemeSignature == signature {
            return (lastTheme, signature)
        }
        let path = themeSignature == "missing" ? nil : userTheme.path
        guard let json = KeyTaoIOSThemeResolver.resolveJson(
            userThemePath: path,
            systemColorScheme: systemColorScheme
        ), let theme = KeyTaoIOSThemeResolver.decode(json: json) else {
            if let lastTheme, lastThemeColorScheme == systemColorScheme {
                return (lastTheme, lastThemeSignature)
            }
            return (KeyTaoIOSThemeResolver.resolve(
                userThemePath: path,
                systemColorScheme: systemColorScheme
            ), nil)
        }
        lastTheme = theme
        lastThemeSignature = signature
        lastThemeColorScheme = systemColorScheme
        return (theme, signature)
    }

    func loadConfig() -> KeyTaoIOSImeConfig {
        let userKeyboard = KeyTaoIOSPaths.keyboardFile(userRoot: userRoot)
        let userConfig = KeyTaoIOSPaths.configFile(userRoot: userRoot)
        let keyboardSignature = fileSignature(userKeyboard)
        let configSignature = fileSignature(userConfig)
        let signature = "\(keyboardSignature)|\(configSignature)"
        if let lastConfig, lastConfigSignature == signature {
            return lastConfig
        }
        let resolvedKeyboardJson = KeyTaoIOSKeyboardConfigResolver.resolveJson(
            userKeyboardPath: keyboardSignature == "missing" ? nil : userKeyboard.path
        )
        guard let config = KeyTaoIOSImeConfig.loadIfAvailable(
            resolvedKeyboardJson: resolvedKeyboardJson,
            userConfigURL: configSignature == "missing" ? nil : userConfig
        ) else {
            clipboardConfigLoaded = lastConfig != nil
            return lastConfig ?? KeyTaoIOSImeConfig.load(
                resolvedKeyboardJson: resolvedKeyboardJson,
                userConfigURL: configSignature == "missing" ? nil : userConfig
            )
        }
        clipboardConfigLoaded = true
        lastConfig = config
        lastConfigSignature = signature
        return config
    }

    private func fileSignature(_ url: URL) -> String {
        guard let attributes = try? FileManager.default.attributesOfItem(atPath: url.path),
              let modified = attributes[.modificationDate] as? Date,
              let size = attributes[.size] as? NSNumber else {
            return "missing"
        }
        return "\(modified.timeIntervalSince1970):\(size.uint64Value)"
    }

    private func sessionConfigurationSignature() -> [String] {
        var files = [userRoot.appendingPathComponent("build/.keytao-deployed")]
        if let stampPath = userRoot.path.withCString({ ownedCString(keytao_reload_stamp_path_at($0)) }) {
            files.append(URL(fileURLWithPath: stampPath))
        }
        return files.map { url in
            "\(fileSignature(url)):\((try? Data(contentsOf: url))?.base64EncodedString() ?? "")"
        }
    }

    func persistSettings(patch: [String: Any]) -> Bool {
        let url = KeyTaoIOSPaths.configFile(userRoot: userRoot)
        do {
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
            lastConfigSignature = nil
            return true
        } catch {
            KeyTaoLog.configWriteFailed("settings")
            return false
        }
    }

    func listKeyboards(recommendedFor schemaID: String?) -> [KeyTaoKeyboardOption] {
        let path = KeyTaoIOSPaths.keyboardFile(userRoot: userRoot).path
        guard let ptr = path.withCString({ keytao_list_keyboards_json($0) }) else {
            return []
        }
        defer { keytao_free_string(ptr) }
        guard let data = String(cString: ptr).data(using: .utf8),
              let arr = try? JSONSerialization.jsonObject(with: data) as? [[String: Any]] else {
            return []
        }
        let normalized = schemaID?.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() ?? ""
        struct RawItem {
            let id: String
            let name: String
            let builtIn: Bool
            let current: Bool
            let recommendedSchemas: [String]
        }
        let items: [RawItem] = arr.compactMap { entry in
            guard let id = entry["id"] as? String, !id.isEmpty,
                  let name = entry["name"] as? String, !name.isEmpty else { return nil }
            return RawItem(
                id: id,
                name: name,
                builtIn: entry["builtIn"] as? Bool ?? false,
                current: entry["current"] as? Bool ?? false,
                recommendedSchemas: entry["recommendedSchemas"] as? [String] ?? []
            )
        }
        var recommendedID: String?
        if !normalized.isEmpty {
            let ordered = items.filter { !$0.builtIn } + items.filter { $0.builtIn }
            recommendedID = ordered.first { item in
                item.recommendedSchemas.contains { $0.lowercased() == normalized }
            }?.id
            if recommendedID == nil,
               normalized.contains("pinyin")
                || normalized.contains("shuangpin")
                || normalized.contains("wanxiang")
                || normalized.contains("rime_ice")
                || normalized.contains("rime_mint")
                || normalized.contains("rime_frost") {
                recommendedID = items.first { $0.id == "keytao.qwerty26" }?.id
            }
        }
        return items.map {
            KeyTaoKeyboardOption(
                id: $0.id,
                name: $0.name,
                current: $0.current,
                recommended: $0.id == recommendedID
            )
        }
    }

    func listKeyboardIds() -> [String] {
        listKeyboards(recommendedFor: nil).map(\.id)
    }

    func applyKeyboard(id: String) -> Bool {
        let path = KeyTaoIOSPaths.keyboardFile(userRoot: userRoot).path
        return path.withCString { pathPtr in
            id.withCString { idPtr in
                keytao_apply_keyboard(pathPtr, idPtr)
            }
        }
    }

    func persistThemeUi(colorScheme: String, accentHex: String?, resetAppearance: Bool = false) -> Bool {
        let path = KeyTaoIOSPaths.themeFile(userRoot: userRoot).path
        let written = path.withCString { pathPointer in
            colorScheme.withCString { schemePointer in
                if resetAppearance {
                    return "".withCString { clearPointer in
                        keytao_write_theme_appearance(pathPointer, schemePointer, clearPointer, -1, clearPointer)
                    }
                }
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
            lastThemeSignature = nil
            lastThemeColorScheme = nil
        } else {
            KeyTaoLog.configWriteFailed("theme_ui")
        }
        return written
    }

    /// Drops everything that can be rebuilt on demand; the session and the
    /// deployed schemas stay put so that typing keeps working.
    func releaseCaches() {
        lastTheme = nil
        lastThemeSignature = nil
        lastThemeColorScheme = nil
        lastConfig = nil
        lastConfigSignature = nil
    }

    @discardableResult
    func releaseIdleSession() -> Bool {
        guard !startupInFlight else { return false }
        if let session {
            guard let state = decodeState(keytao_session_state_json(session)), !state.hasComposition,
                  let schema = currentSchema() else { return false }
            var options: [String: Bool] = [:]
            for name in schemaSwitches().flatMap(\.optionNames) + ["ascii_mode", "ascii_punct"] {
                options[name] = getOption(name)
            }
            suspendedSession = (schema.id, options, sessionConfigurationSignature())
        }
        close()
        releaseCaches()
        lastState = .empty
        return true
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

    func listSwitcherPackages() -> [KeyTaoRimePackage] {
        guard let session,
              let json = ownedCString(keytao_session_list_switcher_packages_json(session)) else { return [] }
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return (try? decoder.decode([KeyTaoRimePackage].self, from: Data(json.utf8))) ?? []
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
        let started = KeyTaoLog.now()
        var selected = false
        defer {
            KeyTaoLog.event("rime", "schema_switch", duration: (KeyTaoLog.now() - started) * 1000) {
                ["ok": selected]
            }
        }
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
        selected = true
        return stable
    }

    func selectPackageSchema(_ packageKey: String, schemaID: String) -> KeyTaoImeState? {
        guard let session else { return nil }
        let state = packageKey.withCString { packageKey in
            schemaID.withCString { schemaID in
                decodeState(keytao_session_select_package_schema_json(session, packageKey, schemaID))
            }
        }
        guard let state else { return nil }
        if state.accepted {
            schemaLogoGeneration &+= 1
            refreshCapabilities()
        }
        let stable = stableSchemaState(state)
        lastState = stable.withoutTransientCommit()
        return stable
    }

    func schemeOperationProgress() -> String? {
        userRoot.path.withCString { ownedCString(keytao_scheme_operation_progress_at($0)) }
    }

    func loadedReloadStampSignature() -> String? {
        ownedCString(keytao_loaded_reload_stamp_signature())
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

    /// Stamp detection lives in the common layer (`ReloadStamp`), so the
    /// signature the containing app reports and the one the keyboard compares
    /// come from the same function.
    func reloadIfNeeded() -> Bool {
        let started = runtimeMetricsEnabled ? KeyTaoLog.now() : nil
        defer {
            if let started { reloadCheckHistogram.record((KeyTaoLog.now() - started) * 1000) }
        }
        guard algorithmEnabled, nativeReady else {
            return false
        }
        guard keytao_reload_if_stamp_changed() else {
            return false
        }
        schemaLogoGeneration &+= 1
        refreshCapabilities()
        lastState = state().withoutTransientCommit()
        KeyTaoLog.event("rime", "reload_if_needed", duration: started.map { (KeyTaoLog.now() - $0) * 1000 } ?? .nan) {
            ["reloaded": true]
        }
        return true
    }

    func flushRuntimeMetrics() {
        if let fields = reloadCheckHistogram.drain() {
            KeyTaoLog.event("rime", "reload_if_needed_summary") { fields }
        }
    }

    @discardableResult
    func refreshRuntimeLogCollection() -> Bool {
        runtimeMetricsEnabled = KeyTaoLog.collectionEnabled()
        return runtimeMetricsEnabled
    }

    func close() {
        schemaLogoGeneration &+= 1
        schemaLogoKey = nil
        schemaLogoImage = nil
        onSchemaLogoChanged?(nil)
        if let session {
            keytao_destroy_session(session)
        }
        session = nil
        nativeReady = false
        refreshCapabilities()
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
        let id = state.schemaId
        guard !name.isEmpty, !name.hasPrefix("."), !id.isEmpty, !id.hasPrefix(".") else {
            var next = state
            if !lastDisplaySchemaName.isEmpty { next.schemaName = lastDisplaySchemaName }
            if !state.isPackageSwitcher, !lastDisplaySchemaId.isEmpty { next.schemaId = lastDisplaySchemaId }
            refreshSchemaLogo(next)
            return next
        }
        lastDisplaySchemaName = name
        lastDisplaySchemaId = id
        refreshSchemaLogo(state)
        return state
    }

    private func refreshSchemaLogo(_ state: KeyTaoImeState) {
        let id = state.schemaId
        guard !id.isEmpty, !id.hasPrefix(".") else { return }
        let key = SchemaLogoKey(id: id, modificationTime: nil, generation: schemaLogoGeneration)
        guard schemaLogoRequest != key else { return }
        if schemaLogoKey?.id == id, schemaLogoKey?.generation == key.generation,
           KeyTaoLog.now() - schemaLogoLastCheckUptime < 1 {
            schemaLogoRequest = nil
            return
        }
        schemaLogoRequest = key
        let cachedKey = schemaLogoKey
        let cachedImage = schemaLogoImage
        let logoRoot = userRoot.appendingPathComponent(".keytao-logo", isDirectory: true)
        startupQueue.async { [weak self] in
            let (image, modificationTime) = Self.loadSchemaLogo(root: logoRoot, key: key, cachedKey: cachedKey, cachedImage: cachedImage)
            DispatchQueue.main.async {
                guard let self, self.schemaLogoRequest == key, self.schemaLogoGeneration == key.generation else { return }
                self.schemaLogoRequest = nil
                self.schemaLogoLastCheckUptime = KeyTaoLog.now()
                let loadedKey = SchemaLogoKey(id: key.id, modificationTime: modificationTime, generation: key.generation)
                guard self.schemaLogoKey != loadedKey else { return }
                self.schemaLogoKey = loadedKey
                self.schemaLogoImage = image
                self.onSchemaLogoChanged?(image)
            }
        }
    }

    private static func loadSchemaLogo(root: URL, key: SchemaLogoKey, cachedKey: SchemaLogoKey?, cachedImage: UIImage?) -> (UIImage?, Date?) {
        let id = key.id
        guard !id.isEmpty, id != ".", id != "..", !id.contains("/"), !id.contains("\\"), !id.contains("\0") else {
            return (nil, nil)
        }
        let url = root.appendingPathComponent("\(id).png")
        guard let attributes = try? FileManager.default.attributesOfItem(atPath: url.path) else {
            return (nil, nil)
        }
        let modificationTime = attributes[.modificationDate] as? Date
        if cachedKey == SchemaLogoKey(id: id, modificationTime: modificationTime, generation: key.generation) {
            return (cachedImage, modificationTime)
        }
        let maxBytes = 1024 * 1024
        guard let size = attributes[.size] as? NSNumber, size.int64Value <= Int64(maxBytes),
              let handle = try? FileHandle(forReadingFrom: url) else {
            return (nil, modificationTime)
        }
        defer { try? handle.close() }
        guard let data = try? handle.read(upToCount: maxBytes + 1), data.count <= maxBytes,
              data.starts(with: [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
              let source = CGImageSourceCreateWithData(data as CFData, [kCGImageSourceShouldCache: false] as CFDictionary),
              let sourceType = CGImageSourceGetType(source), sourceType as String == "public.png",
              let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any],
              let width = properties[kCGImagePropertyPixelWidth] as? NSNumber,
              let height = properties[kCGImagePropertyPixelHeight] as? NSNumber,
              width.intValue > 0, height.intValue > 0, width.intValue <= 512, height.intValue <= 512,
              let image = CGImageSourceCreateImageAtIndex(source, 0, [kCGImageSourceShouldCacheImmediately: true] as CFDictionary) else {
            return (nil, modificationTime)
        }
        return (UIImage(cgImage: image), modificationTime)
    }
}
