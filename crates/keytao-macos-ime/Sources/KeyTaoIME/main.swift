import Cocoa
import InputMethodKit

let installer = KeyTaoInputSourceInstaller()
if CommandLine.arguments.count > 1 {
  switch CommandLine.arguments[1] {
  case "--register-input-source":
    installer.register()
    exit(0)
  case "--enable-input-source":
    installer.enablePrimary()
    exit(0)
  case "--select-input-source":
    installer.selectPrimary()
    exit(0)
  case "--disable-legacy-input-sources":
    installer.disableLegacySources()
    exit(0)
  case "--list-input-sources":
    installer.printKeyTaoSources()
    exit(0)
  default:
    break
  }
}

let connectionName =
  Bundle.main.infoDictionary?["InputMethodConnectionName"] as? String
  ?? "KeyTao_Connection"

let imkServer = IMKServer(
  name: connectionName,
  bundleIdentifier: Bundle.main.bundleIdentifier ?? KeyTaoInputSource.bundleID
)
if imkServer == nil {
  NSLog("KeyTao: IMKServer returned nil — TIS may not have registered yet")
}
_ = imkServer

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
app.run()
