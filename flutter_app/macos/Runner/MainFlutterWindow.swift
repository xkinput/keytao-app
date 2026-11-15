import Cocoa
import FlutterMacOS

class MainFlutterWindow: NSWindow {
  override func awakeFromNib() {
    let windowController = FlutterViewController()
    self.contentViewController = windowController
    self.setContentSize(NSSize(width: 900, height: 680))
    self.contentMinSize = NSSize(width: 720, height: 520)
    self.center()

    RegisterGeneratedPlugins(registry: windowController)

    let channel = FlutterMethodChannel(
      name: "ink.rea.keytao/window",
      binaryMessenger: windowController.engine.binaryMessenger
    )
    channel.setMethodCallHandler { [weak self] call, result in
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
