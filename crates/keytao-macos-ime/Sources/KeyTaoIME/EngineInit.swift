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

/// Path of the reload signal, as keytao-core spells it. The IME must not build
/// this path (or its signature) itself: the app writes the file through the
/// same shared implementation.
func reloadStampPath() -> String? {
    let userDir = resolveUserDataDir(home: FileManager.default.homeDirectoryForCurrentUser.path)
    guard let ptr = userDir.withCString({ keytao_reload_stamp_path_at($0) }) else {
        return nil
    }
    defer { keytao_free_string(ptr) }
    return String(cString: ptr)
}

/// Everything that happens once per input method process: bringing the shared
/// runtime up, declaring what this frontend can draw, and noticing when the app
/// has deployed new data.
///
/// None of it runs on the IMKit callback thread. `handleEvent:client:` is a
/// synchronous round trip from the client application, so a blocking
/// `keytao_init()` there would stall the foreground app's event loop, and a
/// stat of the reload stamp there would put file I/O on every keystroke.
final class KeyTaoRuntime {
    static let shared = KeyTaoRuntime()

    /// Posted on the main queue after the runtime became usable or reloaded.
    static let didReloadNotification = Notification.Name("ink.rea.keytao.runtimeDidReload")

    /// How long to wait before retrying an initialization that failed, so a
    /// machine without deployed data does not rescan on every activation.
    private static let initializationRetryInterval: TimeInterval = 2
    /// How long to wait before looking for a user directory that does not exist yet.
    private static let watchRetryInterval: TimeInterval = 5

    private let queue = DispatchQueue(label: "ink.rea.keytao.runtime")
    private let startLock = NSLock()
    private var started = false

    private var lastInitializationAttempt: Date?
    private var watchSource: DispatchSourceFileSystemObject?

    private init() {}

    /// Safe to call from every controller: only the first call does work.
    func start() {
        startLock.lock()
        let alreadyStarted = started
        started = true
        startLock.unlock()
        guard !alreadyStarted else { return }

        declareUiCapabilities()
        ImeThemeManager.shared.installThemeSources()

        queue.async {
            self.initializeIfNeeded()
            self.startWatchingReloadStamp()
        }
    }

    /// Ask for a retry without blocking the caller. Key handling uses this when
    /// there is still no session to talk to.
    func requestInitialization() {
        queue.async { self.initializeIfNeeded() }
    }

    /// Focus changes are a good moment to notice a deployment, and the check is
    /// cheap because it happens off the caller's thread.
    func checkForReload() {
        queue.async { self.consumeReloadRequest() }
    }

    /// The input menu's explicit redeploy, which must reload even when the app
    /// wrote no stamp.
    func reloadNow() -> Bool {
        guard keytao_is_initialized() else {
            requestInitialization()
            return false
        }
        return keytao_reload()
    }

    // MARK: - Private

    private func declareUiCapabilities() {
        // An IMKit panel draws its own colors, can stack candidates vertically,
        // tracks the mouse and casts a shadow. Without this the shared theme
        // layer assumes the soft-keyboard candidate bar shape.
        keytao_set_ui_capabilities(true, true, true, true, true, false)
    }

    private func initializeIfNeeded() {
        if keytao_is_initialized() {
            return
        }
        if let last = lastInitializationAttempt,
           Date().timeIntervalSince(last) < Self.initializationRetryInterval {
            return
        }
        lastInitializationAttempt = Date()
        initializeEngine()
        if keytao_is_initialized() {
            postDidReload()
        }
    }

    private func consumeReloadRequest() {
        guard keytao_is_initialized() else {
            initializeIfNeeded()
            return
        }
        if keytao_reload_if_stamp_changed() {
            postDidReload()
        }
    }

    private func startWatchingReloadStamp() {
        watchSource?.cancel()
        watchSource = nil

        guard let stampPath = reloadStampPath() else {
            return
        }
        // Watch the stamp itself once it exists: the app rewrites it in place,
        // which leaves the directory entry untouched and would never reach a
        // watcher on the directory. Before the first deployment the file is
        // missing, so the directory is watched until it shows up.
        let stampExists = FileManager.default.fileExists(atPath: stampPath)
        let watchedPath = stampExists
            ? stampPath
            : (stampPath as NSString).deletingLastPathComponent
        let descriptor = open(watchedPath, O_EVTONLY)
        guard descriptor >= 0 else {
            queue.asyncAfter(deadline: .now() + Self.watchRetryInterval) { [weak self] in
                self?.startWatchingReloadStamp()
            }
            return
        }

        let source = DispatchSource.makeFileSystemObjectSource(
            fileDescriptor: descriptor,
            eventMask: [.write, .extend, .delete, .rename, .revoke],
            queue: queue
        )
        source.setEventHandler { [weak self] in
            guard let self else { return }
            self.consumeReloadRequest()
            // The stamp may have just been created, replaced by a rename, or
            // unlinked, so the watch is rebuilt against whatever is there now.
            self.startWatchingReloadStamp()
        }
        source.setCancelHandler {
            close(descriptor)
        }
        watchSource = source
        source.resume()

        consumeReloadRequest()
    }

    private func postDidReload() {
        DispatchQueue.main.async {
            NotificationCenter.default.post(name: Self.didReloadNotification, object: nil)
        }
    }
}
