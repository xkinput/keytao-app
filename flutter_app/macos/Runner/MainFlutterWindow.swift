import Cocoa
import CoreText
import FlutterMacOS

class MainFlutterWindow: NSWindow {
  private var themeSharePicker: NSSharingServicePicker?
  private let fontQueue = DispatchQueue(label: "ink.rea.keytao.system-fonts", qos: .userInitiated)

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
      switch call.method {
      case "setTitle":
        guard let title = call.arguments as? String else {
          result(FlutterMethodNotImplemented)
          return
        }
        self?.title = title
        result(nil)
      case "shareThemeFile":
        self?.shareThemeFile(call.arguments, result: result)
      case "resolveSystemFontFamily":
        guard let self, let arguments = call.arguments as? [String: String],
          let family = arguments["family"], !family.isEmpty
        else {
          result(false)
          return
        }
        self.fontQueue.async {
          let resolved = Self.resolveSystemFontFamily(family, path: arguments["path"])
          DispatchQueue.main.async { result(resolved) }
        }
      default:
        result(FlutterMethodNotImplemented)
      }
    }

    super.awakeFromNib()
  }

  private static func resolveSystemFontFamily(_ family: String, path: String?) -> Bool {
    let key = kCTFontFamilyNameAttribute
    let descriptor = CTFontDescriptorCreateWithAttributes([key: family] as CFDictionary)
    let mandatory = NSSet(object: key) as CFSet
    func matches() -> Bool {
      guard let match = CTFontDescriptorCreateMatchingFontDescriptor(descriptor, mandatory) else {
        return false
      }
      return (CTFontDescriptorCopyAttribute(match, key) as? String) == family
    }
    if matches() { return true }
    guard let path else { return false }
    CTFontManagerRegisterFontsForURL(URL(fileURLWithPath: path) as CFURL, .process, nil)
    return matches()
  }

  private func shareThemeFile(_ arguments: Any?, result: FlutterResult) {
    guard let arguments = arguments as? [String: Any],
      let fileName = arguments["fileName"] as? String,
      let json = arguments["json"] as? String,
      fileName.hasSuffix(".keytao-theme.json")
    else {
      result(FlutterError(code: "INVALID_THEME_FILE", message: "主题文件无效", details: nil))
      return
    }
    let id = String(fileName.dropLast(".keytao-theme.json".count))
    let data = Data(json.utf8)
    guard (3...64).contains(id.count),
      id.range(of: "\\A[a-z0-9][a-z0-9_-]*(\\.[a-z0-9_-]+)*\\z", options: .regularExpression) != nil,
      data.count <= 64 * 1024, let view = contentView
    else {
      result(FlutterError(code: "INVALID_THEME_FILE", message: "主题文件无效或超过 64 KiB", details: nil))
      return
    }
    let manager = FileManager.default
    let directory = manager.temporaryDirectory
      .appendingPathComponent("theme-share-\(UUID().uuidString)", isDirectory: true)
    do {
      try manager.createDirectory(at: directory, withIntermediateDirectories: false)
      let file = directory.appendingPathComponent(fileName)
      try data.write(to: file, options: .atomic)
      let picker = NSSharingServicePicker(items: [file])
      themeSharePicker = picker
      picker.show(relativeTo: NSRect(x: view.bounds.midX, y: view.bounds.midY, width: 1, height: 1),
        of: view, preferredEdge: .minY)
      result(nil)
    } catch {
      try? manager.removeItem(at: directory)
      result(FlutterError(code: "THEME_SHARE_FAILED", message: error.localizedDescription, details: nil))
    }
  }
}
