import Cocoa
import InputMethodKit

enum KeyTaoInputSource {
  static let appURL = Bundle.main.bundleURL
  static let installedAppURL = URL(fileURLWithPath: "/Library/Input Methods/KeyTao.app")
  static let bundleID = "ink.rea.inputmethod.keytao"
  static let primaryModeID = "ink.rea.inputmethod.keytao.Hans"
  static let legacyModeIDs = [
    "ink.rea.keytao-ime.Hans",
    "ink.rea.keytao-ime",
    "ink.rea.keytao.inputmethod.Hans",
    "ink.rea.keytao.inputmethod",
  ]
  static let managedIDs = [primaryModeID] + legacyModeIDs + [bundleID]
}

final class KeyTaoInputSourceInstaller {
  func register() {
    let status = TISRegisterInputSource(KeyTaoInputSource.appURL as CFURL)
    print("register=\(status)")
  }

  @discardableResult
  func enablePrimary() -> Bool {
    if let parent = inputSource(id: KeyTaoInputSource.bundleID),
       boolProperty(parent, key: kTISPropertyInputSourceIsEnableCapable),
       !boolProperty(parent, key: kTISPropertyInputSourceIsEnabled) {
      _ = enable(id: KeyTaoInputSource.bundleID)
    }
    return enable(id: KeyTaoInputSource.primaryModeID)
  }

  func selectPrimary() {
    guard let source = inputSource(id: KeyTaoInputSource.primaryModeID) else {
      print("select=missing:\(KeyTaoInputSource.primaryModeID)")
      return
    }
    guard enablePrimary() else { return }
    let status = TISSelectInputSource(source)
    print("select=\(status):\(KeyTaoInputSource.primaryModeID)")
  }

  func disableLegacySources() {
    for id in KeyTaoInputSource.legacyModeIDs {
      _ = disable(id: id)
    }
  }

  func managesSystemProfile(_ root: String) -> Bool {
    let home = FileManager.default.homeDirectoryForCurrentUser.path
    let defaultRoot = (home as NSString).appendingPathComponent("Library/keytao")
    let requested = URL(fileURLWithPath: root).resolvingSymlinksInPath().path
    return requested == URL(fileURLWithPath: defaultRoot).resolvingSymlinksInPath().path
      && requested == URL(fileURLWithPath: resolveUserDataDir(home: home))
        .resolvingSymlinksInPath().path
      && KeyTaoInputSource.appURL.resolvingSymlinksInPath().path
        == KeyTaoInputSource.installedAppURL.resolvingSymlinksInPath().path
  }

  func disableKeyTaoSources() -> Bool {
    if let current = TISCopyCurrentKeyboardInputSource()?.takeRetainedValue(),
       KeyTaoInputSource.managedIDs.contains(stringProperty(current, key: kTISPropertyInputSourceID)) {
      guard let fallback = TISCopyCurrentASCIICapableKeyboardLayoutInputSource()?.takeRetainedValue(),
            TISSelectInputSource(fallback) == noErr else {
        NSLog("KeyTao: unable to select a keyboard layout before shutdown")
        return false
      }
    }
    var disabled = true
    for id in KeyTaoInputSource.managedIDs {
      if !disable(id: id) { disabled = false }
    }
    return disabled
  }

  func stopInputMethod() -> Bool {
    guard !inputEngineEnabled(), disableKeyTaoSources() else { return false }
    let started = ProcessInfo.processInfo.systemUptime
    var requestedTermination = Set<pid_t>()
    repeat {
      guard !inputEngineEnabled() else { return false }
      let current = runningInputMethods()
      if current.isEmpty { return true }

      // Already running versions may predate the reload-driven shutdown.
      if ProcessInfo.processInfo.systemUptime - started >= 2 {
        for app in current where requestedTermination.insert(app.processIdentifier).inserted {
          _ = app.terminate()
        }
      }
      let nextPoll = Date().addingTimeInterval(0.05)
      if !RunLoop.current.run(mode: .default, before: nextPoll) {
        Thread.sleep(until: nextPoll)
      }
    } while ProcessInfo.processInfo.systemUptime - started < 5
    NSLog("KeyTao: input method did not terminate")
    return false
  }

  private func runningInputMethods() -> [NSRunningApplication] {
    let path = KeyTaoInputSource.installedAppURL.resolvingSymlinksInPath().path
    return ([KeyTaoInputSource.bundleID] + KeyTaoInputSource.legacyModeIDs)
      .flatMap { NSRunningApplication.runningApplications(withBundleIdentifier: $0) }
      .filter {
        $0.processIdentifier != getpid() && !$0.isTerminated
          && $0.bundleURL?.resolvingSymlinksInPath().path == path
      }
  }

  func printKeyTaoSources() {
    let sources = allInputSources()
    for source in sources {
      let sourceID = stringProperty(source, key: kTISPropertyInputSourceID)
      if sourceID.contains("keytao") || sourceID.contains("KeyTao") {
        let name = stringProperty(source, key: kTISPropertyLocalizedName)
        let enabled = boolProperty(source, key: kTISPropertyInputSourceIsEnabled)
        let selectable = boolProperty(source, key: kTISPropertyInputSourceIsSelectCapable)
        print("source=\(sourceID) name=\(name) enabled=\(enabled) selectable=\(selectable)")
      }
    }
  }

  private func enable(id: String) -> Bool {
    guard let source = inputSource(id: id) else {
      print("enable=missing:\(id)")
      return false
    }
    let enabled = boolProperty(source, key: kTISPropertyInputSourceIsEnabled)
    let status = enabled ? noErr : TISEnableInputSource(source)
    print("enable=\(status):\(id)")
    return status == noErr
  }

  private func disable(id: String) -> Bool {
    guard let source = inputSource(id: id),
          boolProperty(source, key: kTISPropertyInputSourceIsEnabled) else {
      return true
    }
    if !boolProperty(source, key: kTISPropertyInputSourceIsEnableCapable),
       !boolProperty(source, key: kTISPropertyInputSourceIsSelectCapable) {
      return true
    }
    let status = TISDisableInputSource(source)
    print("disable=\(status):\(id)")
    return status == noErr
  }

  private func inputSource(id: String) -> TISInputSource? {
    for source in allInputSources() {
      if stringProperty(source, key: kTISPropertyInputSourceID) == id {
        return source
      }
    }
    return nil
  }

  private func allInputSources() -> [TISInputSource] {
    TISCreateInputSourceList(nil, true).takeRetainedValue() as! [TISInputSource]
  }

  private func stringProperty(_ source: TISInputSource, key: CFString) -> String {
    guard let value = TISGetInputSourceProperty(source, key) else {
      return ""
    }
    return unsafeBitCast(value, to: CFString?.self) as String? ?? ""
  }

  private func boolProperty(_ source: TISInputSource, key: CFString) -> Bool {
    guard let value = TISGetInputSourceProperty(source, key) else {
      return false
    }
    return unsafeBitCast(value, to: CFBoolean?.self).map { CFBooleanGetValue($0) } ?? false
  }
}
