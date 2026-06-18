import Foundation
import InputMethodKit

enum KeyTaoInputSource {
  static let appURL = Bundle.main.bundleURL
  static let bundleID = "ink.rea.inputmethod.keytao"
  static let primaryModeID = "ink.rea.inputmethod.keytao.Hans"
  static let legacyModeIDs = [
    "ink.rea.keytao-ime.Hans",
    "ink.rea.keytao-ime",
    "ink.rea.keytao.inputmethod.Hans",
    "ink.rea.keytao.inputmethod",
  ]
}

final class KeyTaoInputSourceInstaller {
  func register() {
    let status = TISRegisterInputSource(KeyTaoInputSource.appURL as CFURL)
    print("register=\(status)")
  }

  func enablePrimary() {
    enable(id: KeyTaoInputSource.primaryModeID)
  }

  func selectPrimary() {
    guard let source = inputSource(id: KeyTaoInputSource.primaryModeID) else {
      print("select=missing:\(KeyTaoInputSource.primaryModeID)")
      return
    }
    enable(id: KeyTaoInputSource.primaryModeID)
    let status = TISSelectInputSource(source)
    print("select=\(status):\(KeyTaoInputSource.primaryModeID)")
  }

  func disableLegacySources() {
    for id in KeyTaoInputSource.legacyModeIDs {
      disable(id: id)
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

  private func enable(id: String) {
    guard let source = inputSource(id: id) else {
      print("enable=missing:\(id)")
      return
    }
    let status = TISEnableInputSource(source)
    print("enable=\(status):\(id)")
  }

  private func disable(id: String) {
    guard let source = inputSource(id: id) else {
      return
    }
    let status = TISDisableInputSource(source)
    print("disable=\(status):\(id)")
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
