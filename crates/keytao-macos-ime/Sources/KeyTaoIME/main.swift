import Cocoa
import InputMethodKit
import CKeytaoCore

if let subcommand = CommandLine.arguments.dropFirst().first,
  subcommand == "--deploy" || subcommand == "--preview-candidates"
{
  if let resources = Bundle.main.resourceURL {
    for name in ["rime-data", "SharedSupport"] {
      let path = resources.appendingPathComponent(name).path
      if hasDefaultYaml(at: path) {
        setenv("KEYTAO_RIME_SHARED_DATA_DIR", path, 1)
        break
      }
    }
  }
  if subcommand == "--preview-candidates" {
    keytao_run_preview_child_main()
  } else {
    keytao_run_deploy_child_main()
  }
}

let installer = KeyTaoInputSourceInstaller()
let profile = resolveUserDataDir(home: FileManager.default.homeDirectoryForCurrentUser.path)
if CommandLine.arguments.count > 1 {
  switch CommandLine.arguments[1] {
  case "--register-input-source":
    installer.register()
    exit(0)
  case "--enable-input-source":
    guard let root = CommandLine.arguments.dropFirst(2).first else {
      installer.enablePrimary()
      exit(EXIT_SUCCESS)
    }
    guard installer.managesSystemProfile(root) else { exit(EXIT_SUCCESS) }
    guard inputEngineEnabled() else { exit(EXIT_FAILURE) }
    exit(installer.enablePrimary() ? EXIT_SUCCESS : EXIT_FAILURE)
  case "--stop-input-method":
    let root = CommandLine.arguments.dropFirst(2).first ?? profile
    guard installer.managesSystemProfile(root) else { exit(EXIT_SUCCESS) }
    exit(installer.stopInputMethod() ? EXIT_SUCCESS : EXIT_FAILURE)
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

guard inputEngineEnabled() else {
  if installer.managesSystemProfile(profile) {
    _ = installer.disableKeyTaoSources()
  }
  exit(EXIT_SUCCESS)
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
_ = KeyTaoExecutableUpdateGuard.shared
// Bring the shared runtime up before the first client connects, so librime
// initialization never happens inside a synchronous IMKit callback.
KeyTaoRuntime.shared.start()
app.run()
