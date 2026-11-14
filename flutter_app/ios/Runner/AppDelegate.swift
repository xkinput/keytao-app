import Flutter
import CryptoKit
import UIKit
import UniformTypeIdentifiers

enum KeyTaoIOSBundledData {
  // Every version of default-keyboard.yaml shipped since iOS layout seeding began.
  static let legacyDefaultKeyboardHashes: Set<String> = [
    "34475509153894fcd8e53f5d21b1f6d0852b800731d670e9cfe7694cdf64df2c",
    "25ae1b176617c64fec16a55b73c9faae8abd17c3eba12d1fc67ec9f66364b854",
    "3ebe95295376bfeffb79c0106f86bc7e3d8631311dae0c595d330ed4c1b2805c",
    "e4d7aa7445ac138286941d095017ee7d9e397ecc5501cfb744482835538e5329",
    "cea5f21a618e959d230a637aa6350506dbeb6e7c22bcd2ecca5147adb004cc7a",
    "169d17974591052bd9e4481f45d1f442acfb6c0d376fb6a8aa283dc37520fb78",
    "046288ffa66825452485199a9c5d5fba0f1de5c6d6e50d9ea1c21acc09aa90ab",
    "7168cea0f730954a7d93f748275fd7f6c3f82f83ced681e9aff4fa60d870a9a4",
    "eeb40d355a81a536b3b3bb2d11b5ea889f0ea656220b956bda9e45f1e11caf31",
    "6f34882fe10e215dd0351c5e8e990e9b2ed40afb351668ca2104ab49e8da2549",
    "3fc4b50a4ddf2a37f24d63671f788414e32776f78334ae741444eaaade513a66",
  ]

  static func shouldRefreshRimeData(
    storedStamp: String?, bundleStamp: String, hasDefaultYaml: Bool
  ) -> Bool {
    !hasDefaultYaml || storedStamp != bundleStamp
  }

  static func shouldRefreshKeyboard(
    existingHash: String?, bundledHash: String, seededHash: String?
  ) -> Bool {
    guard let existingHash else { return true }
    if existingHash == bundledHash { return false }
    let seed = seededHash?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    return seed.isEmpty
      ? legacyDefaultKeyboardHashes.contains(existingHash)
      : existingHash == seed
  }

  static func sha256(_ value: String) -> String {
    SHA256.hash(data: Data(value.utf8)).map { String(format: "%02x", $0) }.joined()
  }

  static func refreshRimeData(from source: URL, to destination: URL, bundleStamp: String) throws {
    let manager = FileManager.default
    let parent = destination.deletingLastPathComponent()
    let backup = parent.appendingPathComponent(".rime-data-previous", isDirectory: true)
    let staging = parent.appendingPathComponent(".rime-data-staging", isDirectory: true)
    if !manager.fileExists(atPath: destination.path), manager.fileExists(atPath: backup.path) {
      try manager.moveItem(at: backup, to: destination)
    }
    if manager.fileExists(atPath: staging.path) {
      try manager.removeItem(at: staging)
    }
    let stampURL = destination.appendingPathComponent(".bundled-version")
    let storedStamp = try? String(contentsOf: stampURL, encoding: .utf8)
    guard shouldRefreshRimeData(
      storedStamp: storedStamp, bundleStamp: bundleStamp,
      hasDefaultYaml: manager.fileExists(atPath: destination.appendingPathComponent("default.yaml").path)
    ) else {
      try? manager.removeItem(at: backup)
      return
    }
    defer { try? manager.removeItem(at: staging) }
    try manager.copyItem(at: source, to: staging)
    let stagedStamp = staging.appendingPathComponent(".bundled-version")
    if manager.fileExists(atPath: stagedStamp.path) {
      try manager.removeItem(at: stagedStamp)
    }
    let replacing = manager.fileExists(atPath: destination.path)
    if manager.fileExists(atPath: backup.path) {
      try manager.removeItem(at: backup)
    }
    do {
      if replacing {
        _ = try manager.replaceItemAt(
          destination, withItemAt: staging, backupItemName: backup.lastPathComponent,
          options: [.withoutDeletingBackupItem]
        )
      } else {
        try manager.moveItem(at: staging, to: destination)
      }
      // Publishing the stamp last makes an interrupted replacement retryable.
      try bundleStamp.write(to: stampURL, atomically: true, encoding: .utf8)
    } catch {
      if manager.fileExists(atPath: backup.path) {
        if manager.fileExists(atPath: destination.path) {
          _ = try? manager.replaceItemAt(destination, withItemAt: backup)
        } else {
          try? manager.moveItem(at: backup, to: destination)
        }
      }
      throw error
    }
    try? manager.removeItem(at: backup)
  }

  static func seedKeyboard(from source: URL, userRoot: URL) throws {
    let manager = FileManager.default
    let keyboard = userRoot.appendingPathComponent("keyboard.yaml")
    let seed = userRoot.appendingPathComponent(".keyboard.seed")
    let bundled = try String(contentsOf: source, encoding: .utf8)
    let bundledHash = sha256(bundled)
    var existingHash: String?
    if manager.fileExists(atPath: keyboard.path) {
      guard let existing = try? String(contentsOf: keyboard, encoding: .utf8) else { return }
      if !existing.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
        existingHash = sha256(existing)
      }
    }
    let seededHash = try? String(contentsOf: seed, encoding: .utf8)
    if shouldRefreshKeyboard(
      existingHash: existingHash, bundledHash: bundledHash, seededHash: seededHash
    ) {
      try bundled.write(to: keyboard, atomically: true, encoding: .utf8)
      try bundledHash.write(to: seed, atomically: true, encoding: .utf8)
    } else if existingHash == bundledHash,
      seededHash?.trimmingCharacters(in: .whitespacesAndNewlines) != bundledHash {
      try bundledHash.write(to: seed, atomically: true, encoding: .utf8)
    }
  }
}

@main
@objc class AppDelegate: FlutterAppDelegate, FlutterImplicitEngineDelegate, UIDocumentPickerDelegate {
  private var iosChannel: FlutterMethodChannel?
  private let pathsQueue = DispatchQueue(label: "ink.rea.keytao-app.paths")
  private var preparedPaths: [String: String]?
  private var pendingDirectoryImport: FlutterResult?

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
      case "openUrl":
        guard let arguments = call.arguments as? [String: Any],
          let value = arguments["url"] as? String,
          let url = URL(string: value), let scheme = url.scheme, !scheme.isEmpty
        else {
          result(FlutterError(
            code: "INVALID_URL", message: "Expected an absolute URL.", details: nil
          ))
          return
        }
        UIApplication.shared.open(url, options: [:]) { opened in
          result(opened ? nil : FlutterError(
            code: "OPEN_URL_FAILED", message: "Cannot open URL.", details: nil
          ))
        }
      case "openSettings":
        UIApplication.shared.open(URL(string: UIApplication.openSettingsURLString)!, options: [:]) {
          opened in
          result(opened ? nil : FlutterError(
            code: "OPEN_SETTINGS_FAILED", message: "Cannot open app Settings.", details: nil
          ))
        }
      case "importLocalSchemeDirectory":
        importLocalSchemeDirectory(result)
      default:
        result(FlutterMethodNotImplemented)
      }
    }
  }

  private func importLocalSchemeDirectory(_ result: @escaping FlutterResult) {
    guard pendingDirectoryImport == nil else {
      result(FlutterError(code: "DIRECTORY_PICKER_OPEN", message: "目录选择器已打开", details: nil))
      return
    }
    guard var presenter = UIApplication.shared.connectedScenes
      .compactMap({ $0 as? UIWindowScene })
      .filter({ $0.activationState == .foregroundActive })
      .flatMap({ $0.windows })
      .first(where: { $0.isKeyWindow })?.rootViewController
    else {
      result(FlutterError(code: "DIRECTORY_PICKER_UNAVAILABLE", message: "无法打开目录选择器", details: nil))
      return
    }
    while let presented = presenter.presentedViewController {
      presenter = presented
    }
    let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.folder])
    picker.delegate = self
    picker.allowsMultipleSelection = false
    pendingDirectoryImport = result
    presenter.present(picker, animated: true)
  }

  func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
    let result = pendingDirectoryImport
    pendingDirectoryImport = nil
    result?(nil)
  }

  func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
    guard let result = pendingDirectoryImport else { return }
    guard let source = urls.first else {
      pendingDirectoryImport = nil
      result(nil)
      return
    }
    pathsQueue.async {
      let response: Any
      do {
        response = try Self.copyLocalSchemeDirectory(source)
      } catch {
        response = FlutterError(
          code: "DIRECTORY_IMPORT_FAILED", message: error.localizedDescription, details: nil
        )
      }
      DispatchQueue.main.async {
        self.pendingDirectoryImport = nil
        result(response)
      }
    }
  }

  private static func copyLocalSchemeDirectory(_ source: URL) throws -> [String: String] {
    let scoped = source.startAccessingSecurityScopedResource()
    defer {
      if scoped { source.stopAccessingSecurityScopedResource() }
    }
    let manager = FileManager.default
    let name = source.lastPathComponent
    guard !name.isEmpty, name != ".", name != "..", !name.contains("/"),
      try source.resourceValues(forKeys: [.isDirectoryKey]).isDirectory == true
    else {
      throw NSError(domain: "keytao/ios", code: 3, userInfo: [
        NSLocalizedDescriptionKey: "无法读取所选目录"
      ])
    }
    let temporaryRoot = manager.temporaryDirectory
      .appendingPathComponent("local-scheme-\(UUID().uuidString)", isDirectory: true)
    let destination = temporaryRoot.appendingPathComponent(name, isDirectory: true)
    do {
      try manager.createDirectory(at: temporaryRoot, withIntermediateDirectories: false)
      var coordinationError: NSError?
      var copyError: Error?
      NSFileCoordinator().coordinate(readingItemAt: source, options: [], error: &coordinationError) {
        coordinatedSource in
        do {
          try manager.copyItem(at: coordinatedSource, to: destination)
        } catch {
          copyError = error
        }
      }
      if let error = coordinationError { throw error }
      if let error = copyError { throw error }
      return ["path": destination.path, "name": name, "temporaryDirectory": temporaryRoot.path]
    } catch {
      try? manager.removeItem(at: temporaryRoot)
      throw error
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

    guard let resources = Bundle.main.resourceURL,
      let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String,
      let build = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String,
      !version.isEmpty, !build.isEmpty
    else {
      throw NSError(domain: "keytao/ios", code: 2, userInfo: [
        NSLocalizedDescriptionKey: "缺少应用版本信息"
      ])
    }
    let source = resources.appendingPathComponent("rime-data", isDirectory: true)
    guard manager.fileExists(atPath: source.appendingPathComponent("default.yaml").path) else {
      throw NSError(domain: "keytao/ios", code: 2, userInfo: [
        NSLocalizedDescriptionKey: "缺少内置 RIME 数据"
      ])
    }
    let stamp = "\(version)/\(build)"
    // The app and extension must derive the same deployment fingerprint.
    for root in [data, userRoot] {
      try KeyTaoIOSBundledData.refreshRimeData(
        from: source, to: root.appendingPathComponent("rime-data", isDirectory: true),
        bundleStamp: stamp
      )
    }
    try KeyTaoIOSBundledData.seedKeyboard(
      from: resources.appendingPathComponent("default-keyboard.yaml"), userRoot: userRoot
    )
    return ["dataDir": data.path, "cacheDir": cache.path, "userRoot": userRoot.path]
  }
}
