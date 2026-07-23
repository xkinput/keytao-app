import Cocoa
import InputMethodKit
import CKeytaoCore

/// Called once when the OS first launches (or reactivates) the input method process.
func initializeEngine() {
    let home = FileManager.default.homeDirectoryForCurrentUser.path
    let userDir = resolveUserDataDir(home: home)
    let sharedDir = resolveSharedDataDir()

    let ok = keytao_init(userDir, sharedDir)
    if ok {
        NSLog("KeyTao: engine initialized (user=%@, shared=%@)", userDir, sharedDir)
    } else {
        NSLog("KeyTao: engine initialization FAILED")
    }
}

/// Prefer an explicit development override, otherwise use KeyTao's own profile.
func resolveUserDataDir(home: String) -> String {
    let environment = ProcessInfo.processInfo.environment
    if let override = environment["KEYTAO_RIME_USER_DATA_DIR"], hasKeyTaoSchema(at: override) {
        return override
    }

    return (home as NSString).appendingPathComponent("Library/keytao")
}

func hasKeyTaoSchema(at path: String) -> Bool {
    let fileManager = FileManager.default
    return fileManager.fileExists(atPath: (path as NSString).appendingPathComponent("keytao.schema.yaml")) ||
        fileManager.fileExists(atPath: (path as NSString).appendingPathComponent("build/keytao.schema.yaml"))
}

/// Finds the best shared rime-data directory available on this machine.
func resolveSharedDataDir() -> String {
    let environment = ProcessInfo.processInfo.environment
    for key in ["KEYTAO_RIME_SHARED_DATA_DIR", "RIME_SHARED_DATA_DIR", "RIME_DATA_DIR"] {
        if let value = environment[key], hasDefaultYaml(at: value) {
            return value
        }
    }

    let candidates = [
        "/Applications/KeyTao.app/Contents/Resources/rime-data",
        "/Applications/KeyTao.app/Contents/SharedSupport",
        "/Library/Input Methods/KeyTao.app/Contents/Resources/rime-data",
        "/Library/Input Methods/KeyTao.app/Contents/SharedSupport",
        "/Library/Input Methods/Squirrel.app/Contents/SharedSupport",
        "/opt/homebrew/share/rime-data",
        "/usr/local/share/rime-data",
    ]
    for path in candidates {
        if hasDefaultYaml(at: path) {
            return path
        }
    }
    return ""
}

func hasDefaultYaml(at path: String) -> Bool {
    FileManager.default.fileExists(atPath: (path as NSString).appendingPathComponent("default.yaml"))
}

func reloadStampPath() -> String {
    let home = FileManager.default.homeDirectoryForCurrentUser.path
    return (home as NSString)
        .appendingPathComponent("Library/keytao/keytao-ime.reload")
}

private let reloadStampLock = NSLock()
private var lastSeenReloadStamp: String? = currentReloadStamp()

private func currentReloadStamp() -> String? {
    let path = reloadStampPath()
    return try? String(contentsOfFile: path, encoding: .utf8)
        .trimmingCharacters(in: .whitespacesAndNewlines)
}

func consumeExternalDeployReloadRequest() -> Bool {
    reloadStampLock.lock()
    defer { reloadStampLock.unlock() }

    let current = currentReloadStamp()
    guard current != lastSeenReloadStamp else {
        return false
    }

    lastSeenReloadStamp = current
    return current != nil
}

private let engineInitLock = NSLock()

func ensureEngineReady() {
    engineInitLock.lock()
    defer { engineInitLock.unlock() }
    if !keytao_is_initialized() {
        initializeEngine()
    }
}

func reloadEngine() -> Bool {
    ensureEngineReady()
    if !keytao_is_initialized() {
        return false
    }
    return keytao_reload()
}
