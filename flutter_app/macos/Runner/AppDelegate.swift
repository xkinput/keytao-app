import Cocoa
import FlutterMacOS

@main
class AppDelegate: FlutterAppDelegate {
  static func main() {
    let commands = ["--deploy", "--disable-engine", "--enable-engine"]
    guard CommandLine.arguments.dropFirst().contains(where: commands.contains) else {
      _ = NSApplicationMain(CommandLine.argc, CommandLine.unsafeArgv)
      return
    }

    let app = NSApplication.shared
    app.setActivationPolicy(.prohibited)
    let engine = FlutterEngine(name: "KeyTao command", project: nil)
    guard engine.run(withEntrypoint: nil) else { exit(EXIT_FAILURE) }
    withExtendedLifetime(engine) { app.run() }
    exit(EXIT_FAILURE)
  }

  override func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
    return true
  }

  override func applicationSupportsSecureRestorableState(_ app: NSApplication) -> Bool {
    return true
  }
}
