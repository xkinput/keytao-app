import Flutter
import UIKit

@main
@objc class AppDelegate: FlutterAppDelegate, FlutterImplicitEngineDelegate {
  private var iosChannel: FlutterMethodChannel?
  private let pathsQueue = DispatchQueue(label: "ink.rea.keytao-app.paths")
  private var preparedPaths: [String: String]?

  override func application(
    _ application: UIApplication,
    didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?
  ) -> Bool {
    return super.application(application, didFinishLaunchingWithOptions: launchOptions)
  }

  func didInitializeImplicitFlutterEngine(_ engineBridge: FlutterImplicitEngineBridge) {
    GeneratedPluginRegistrant.register(with: engineBridge.pluginRegistry)
    let channel = FlutterMethodChannel(
      name: "keytao/ios",
      binaryMessenger: engineBridge.applicationRegistrar.messenger()
    )
    iosChannel = channel
    channel.setMethodCallHandler { [self] call, result in
      switch call.method {
      case "getPaths":
        pathsQueue.async {
          do {
            let paths = try self.preparedPaths ?? Self.preparePaths()
            self.preparedPaths = paths
            DispatchQueue.main.async { result(paths) }
          } catch {
            let failure = FlutterError(
              code: "GET_PATHS_FAILED", message: error.localizedDescription, details: nil
            )
            DispatchQueue.main.async { result(failure) }
          }
        }
      case "openSettings":
        UIApplication.shared.open(URL(string: UIApplication.openSettingsURLString)!, options: [:]) {
          opened in
          result(opened ? nil : FlutterError(
            code: "OPEN_SETTINGS_FAILED", message: "Cannot open app Settings.", details: nil
          ))
        }
      default:
        result(FlutterMethodNotImplemented)
      }
    }
  }

  private static func preparePaths() throws -> [String: String] {
    let manager = FileManager.default
    guard let group = manager.containerURL(
      forSecurityApplicationGroupIdentifier: "group.ink.rea.keytao-app"
    ) else {
      // A private fallback would break the frozen shared-root contract.
      throw NSError(domain: "keytao/ios", code: 1, userInfo: [
        NSLocalizedDescriptionKey: "App Group group.ink.rea.keytao-app is unavailable."
      ])
    }
    let data = try manager.url(
      for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true
    ).appendingPathComponent("ink.rea.keytao-app", isDirectory: true)
    let cache = try manager.url(
      for: .cachesDirectory, in: .userDomainMask, appropriateFor: nil, create: true
    ).appendingPathComponent("ink.rea.keytao-app", isDirectory: true)
    // Keep this suffix identical to KeyTaoIOSPaths.userRoot() in the keyboard package.
    let userRoot = group.appendingPathComponent("keytao", isDirectory: true)
    for directory in [data, cache, userRoot] {
      try manager.createDirectory(at: directory, withIntermediateDirectories: true)
    }

    // The three-key channel contract lets Dart use dataDir as resourceDir too.
    // Only bundled resources are refreshed; credentials and user schemas are untouched.
    for name in ["rime-data", "addon-schemas"] {
      guard let source = Bundle.main.resourceURL?.appendingPathComponent(name),
        manager.fileExists(atPath: source.path)
      else {
        throw NSError(domain: "keytao/ios", code: 2, userInfo: [
          NSLocalizedDescriptionKey: "Missing bundled resource: \(name)"
        ])
      }
      let destination = data.appendingPathComponent(name, isDirectory: true)
      if manager.fileExists(atPath: destination.path) {
        try manager.removeItem(at: destination)
      }
      try manager.copyItem(at: source, to: destination)
    }

    // Seed the shared fallback once, without replacing any installed user data.
    let shared = userRoot.appendingPathComponent("rime-data", isDirectory: true)
    if !manager.fileExists(atPath: shared.path) {
      let staging = userRoot.appendingPathComponent(".rime-data-\(UUID().uuidString)")
      defer { try? manager.removeItem(at: staging) }
      try manager.copyItem(at: data.appendingPathComponent("rime-data"), to: staging)
      try manager.moveItem(at: staging, to: shared)
    }
    return ["dataDir": data.path, "cacheDir": cache.path, "userRoot": userRoot.path]
  }
}
