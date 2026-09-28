import Cocoa
import FlutterMacOS
import macos_window_utils

class MainFlutterWindow: NSWindow {
  override func awakeFromNib() {
    let windowController = MacOSWindowUtilsViewController()
    self.contentViewController = windowController
    self.setContentSize(NSSize(width: 760, height: 620))
    self.contentMinSize = NSSize(width: 640, height: 480)
    self.center()

    MainFlutterWindowManipulator.start(mainFlutterWindow: self)
    RegisterGeneratedPlugins(registry: windowController.flutterViewController)

    let channel = FlutterMethodChannel(
      name: "ink.rea.keytao/window",
      binaryMessenger: windowController.flutterViewController.engine.binaryMessenger
    )
    channel.setMethodCallHandler { [weak self] call, result in
      if call.method == "pickDirectory" {
        guard let self = self else {
          result(nil)
          return
        }
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.canCreateDirectories = true
        panel.allowsMultipleSelection = false
        panel.prompt = "选择"
        panel.beginSheetModal(for: self) { response in
          result(response == .OK ? panel.url?.path : nil)
        }
        return
      }
      guard call.method == "setTitle", let title = call.arguments as? String else {
        result(FlutterMethodNotImplemented)
        return
      }
      self?.title = title
      result(nil)
    }

    super.awakeFromNib()
  }
}
