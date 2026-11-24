import Cocoa
import FlutterMacOS
import ObjectiveC.runtime
import WebKit

/// Guards `FlutterViewController` against unstarted or menu-tracking trackpad
/// gesture phases (`NSEvent.Phase`).
///
/// When `WKWebView` opens a native context menu (`NSMenuTrackingSession` above
/// WebKit's `-fno-exceptions` C++ frames) or consumes `NSEvent.Phase.began`
/// before the pointer crosses onto `FlutterView`, subsequent `.changed`,
/// `.ended`, or `.stationary` events arriving on `FlutterViewController` hit an
/// active `NSAssert` in `FlutterViewController.mm` (shipped in Debug, Profile,
/// and Release builds of Flutter 3.50.0), causing `objc_exception_rethrow` ->
/// `std::__terminate()` (`SIGABRT`).
///
/// Note: AppKit's `NSView` owned by an `NSViewController` routes responder
/// `forwardMethod` calls directly to `_viewController`, bypassing custom
/// `view.nextResponder` assignments. Wrapping the public `NSResponder` gesture
/// selectors on `FlutterViewController` guarantees every incoming trackpad
/// gesture event is validated before `dispatchGestureEvent:` runs, without
/// requiring any changes in the host application's `MainFlutterWindow`.
private final class FlutterGesturePhaseGuard {
  private static var associatedStateKey: UInt8 = 0
  private static var installed = false

  private var isPanZoomStarted = false
  private var panGesturePhase: NSEvent.Phase = []
  private var scaleGesturePhase: NSEvent.Phase = []
  private var rotateGesturePhase: NSEvent.Phase = []

  static func installIfNeeded() {
    guard !installed else { return }
    installed = true
    let cls: AnyClass = FlutterViewController.self
    hookGestureSelector(#selector(NSResponder.scrollWheel(with:)), on: cls)
    hookGestureSelector(#selector(NSResponder.magnify(with:)), on: cls)
    hookGestureSelector(#selector(NSResponder.rotate(with:)), on: cls)
  }

  private static func hookGestureSelector(_ selector: Selector, on cls: AnyClass) {
    guard let method = class_getInstanceMethod(cls, selector) else { return }
    let originalImp = method_getImplementation(method)
    typealias OriginalFn = @convention(c) (AnyObject, Selector, NSEvent) -> Void
    let original = unsafeBitCast(originalImp, to: OriginalFn.self)

    let block: @convention(block) (AnyObject, NSEvent) -> Void = { receiver, event in
      guard let controller = receiver as? FlutterViewController else {
        original(receiver, selector, event)
        return
      }
      let state = guardState(for: controller)
      if state.shouldForward(event) {
        original(receiver, selector, event)
      }
    }

    let newImp = imp_implementationWithBlock(block)
    if !class_addMethod(cls, selector, newImp, method_getTypeEncoding(method)) {
      method_setImplementation(method, newImp)
    }
  }

  private static func guardState(for controller: FlutterViewController) -> FlutterGesturePhaseGuard {
    if let existing = objc_getAssociatedObject(controller, &associatedStateKey)
      as? FlutterGesturePhaseGuard
    {
      return existing
    }
    let created = FlutterGesturePhaseGuard()
    objc_setAssociatedObject(
      controller,
      &associatedStateKey,
      created,
      .OBJC_ASSOCIATION_RETAIN_NONATOMIC
    )
    return created
  }

  private func resetGestureState() {
    isPanZoomStarted = false
    panGesturePhase = []
    scaleGesturePhase = []
    rotateGesturePhase = []
  }

  private func recordPhase(_ phase: NSEvent.Phase, for type: NSEvent.EventType) {
    switch type {
    case .scrollWheel:
      panGesturePhase = phase
    case .magnify:
      scaleGesturePhase = phase
    case .rotate:
      rotateGesturePhase = phase
    default:
      break
    }
  }

  private func shouldForward(_ event: NSEvent) -> Bool {
    let phase = event.phase
    let momentum = event.momentumPhase
    let inMenuTracking = RunLoop.current.currentMode == .eventTracking

    if inMenuTracking && !isPanZoomStarted {
      return false
    }

    if phase == .began || phase == .mayBegin {
      recordPhase(phase, for: event.type)
      isPanZoomStarted = true
      return true
    }

    if phase == .changed {
      return isPanZoomStarted
    }

    if phase == .ended || phase == .cancelled {
      guard isPanZoomStarted else {
        resetGestureState()
        return false
      }
      recordPhase(phase, for: event.type)
      let allPhases = panGesturePhase.union(scaleGesturePhase).union(rotateGesturePhase)
      let activeMask: NSEvent.Phase = [.began, .changed]
      if allPhases.intersection(activeMask).isEmpty {
        resetGestureState()
      }
      return true
    }

    if phase.isEmpty && momentum.isEmpty {
      return true
    }

    if phase.isEmpty && !momentum.isEmpty {
      return true
    }

    return false
  }
}

private final class WeakScriptMessageHandler: NSObject, WKScriptMessageHandler {
  weak var target: WKScriptMessageHandler?

  init(target: WKScriptMessageHandler) {
    self.target = target
  }

  func userContentController(
    _ userContentController: WKUserContentController,
    didReceive message: WKScriptMessage
  ) {
    target?.userContentController(userContentController, didReceive: message)
  }
}

private final class WeakInstanceBox {
  weak var value: MacOSWebViewInstance?

  init(_ value: MacOSWebViewInstance) {
    self.value = value
  }
}

private final class InteractiveWKWebView: WKWebView {
  /// Strongly retains the native bridge for exactly as long as this view lives.
  ///
  /// The Flutter engine owns the view (and detaches / re-attaches it freely while
  /// a `WebViewWidget` is hidden by `IndexedStack`, `Offstage`, panel toggles,
  /// etc.). The bridge therefore must NOT be torn down when the view merely
  /// leaves its superview; it is released only when the engine releases the view.
  var bridge: MacOSWebViewInstance?

  override func acceptsFirstMouse(for event: NSEvent?) -> Bool {
    return true
  }
}

public class WebviewCorePlugin: NSObject, FlutterPlugin {
  private weak var registrar: FlutterPluginRegistrar?
  private let channel: FlutterMethodChannel
  private var instances: [Int: WeakInstanceBox] = [:]

  init(registrar: FlutterPluginRegistrar, channel: FlutterMethodChannel) {
    self.registrar = registrar
    self.channel = channel
    super.init()
  }

  public static func register(with registrar: FlutterPluginRegistrar) {
    FlutterGesturePhaseGuard.installIfNeeded()
    let channel = FlutterMethodChannel(
      name: "com.infradise.webview_core",
      binaryMessenger: registrar.messenger
    )
    let plugin = WebviewCorePlugin(registrar: registrar, channel: channel)
    registrar.addMethodCallDelegate(plugin, channel: channel)
    let factory = MacOSWebViewFactory(plugin: plugin)
    registrar.register(factory, withId: "com.infradise.webview_core/view")
  }

  fileprivate func createView(arguments: Any?) -> NSView {
    let map = arguments as? [String: Any] ?? [:]
    let controllerId = (map["controllerId"] as? NSNumber)?.intValue ?? 0
    instances.removeValue(forKey: controllerId)?.value?.dispose()
    let instance = MacOSWebViewInstance(
      controllerId: controllerId,
      creationArgs: map,
      plugin: self
    )
    instances[controllerId] = WeakInstanceBox(instance)
    return instance.takeView()
  }

  fileprivate func removeInstance(controllerId: Int, instance: MacOSWebViewInstance) {
    if let box = instances[controllerId], box.value === nil || box.value === instance {
      instances.removeValue(forKey: controllerId)
    }
  }

  fileprivate func sendEvent(_ method: String, arguments: [String: Any?]) {
    let payload: [String: Any] = arguments.reduce(into: [:]) { acc, pair in
      acc[pair.key] = pair.value ?? NSNull()
    }
    if Thread.isMainThread {
      channel.invokeMethod(method, arguments: payload)
    } else {
      DispatchQueue.main.async { [weak self] in
        self?.channel.invokeMethod(method, arguments: payload)
      }
    }
  }

  fileprivate func requestNavigationDecision(
    controllerId: Int,
    url: String,
    isMainFrame: Bool,
    completion: @escaping (Bool) -> Void
  ) {
    let payload: [String: Any] = [
      "controllerId": controllerId,
      "url": url,
      "isMainFrame": isMainFrame,
    ]
    let invokeBlock = { [weak self] in
      guard let self = self else {
        completion(true)
        return
      }
      self.channel.invokeMethod("onNavigationRequest", arguments: payload) { response in
        if let allow = response as? Bool {
          completion(allow)
        } else {
          completion(true)
        }
      }
    }
    if Thread.isMainThread {
      invokeBlock()
    } else {
      DispatchQueue.main.async(execute: invokeBlock)
    }
  }

  public func handle(_ call: FlutterMethodCall, result: @escaping FlutterResult) {
    let args = call.arguments as? [String: Any] ?? [:]
    if call.method == "clearCookies" {
      let store = WKWebsiteDataStore.default()
      store.httpCookieStore.getAllCookies { cookies in
        let hadCookies = !cookies.isEmpty
        let types: Set<String> = [WKWebsiteDataTypeCookies]
        store.removeData(ofTypes: types, modifiedSince: .distantPast) {
          result(hadCookies)
        }
      }
      return
    }
    if call.method == "setCookie" {
      let name = args["name"] as? String ?? ""
      let value = args["value"] as? String ?? ""
      let domain = args["domain"] as? String ?? ""
      let path = args["path"] as? String ?? "/"
      if !name.isEmpty,
         !domain.isEmpty,
         let cookie = HTTPCookie(properties: [
           .name: name,
           .value: value,
           .domain: domain,
           .path: path,
         ])
      {
        WKWebsiteDataStore.default().httpCookieStore.setCookie(cookie) {
          result(nil)
        }
      } else {
        result(nil)
      }
      return
    }

    guard let controllerId = (args["controllerId"] as? NSNumber)?.intValue else {
      result(FlutterError(code: "INVALID_ARGS", message: "Missing controllerId", details: nil))
      return
    }

    if call.method == "dispose" {
      instances.removeValue(forKey: controllerId)?.value?.dispose()
      result(nil)
      return
    }

    guard let instance = instances[controllerId]?.value else {
      instances.removeValue(forKey: controllerId)
      result(nil)
      return
    }
    instance.handle(call: call, args: args, registrar: registrar, result: result)
  }
}

private final class MacOSWebViewFactory: NSObject, FlutterPlatformViewFactory {
  private weak var plugin: WebviewCorePlugin?

  init(plugin: WebviewCorePlugin) {
    self.plugin = plugin
  }

  func createArgsCodec() -> (FlutterMessageCodec & NSObjectProtocol)? {
    return FlutterStandardMessageCodec.sharedInstance()
  }

  func create(withViewIdentifier viewId: Int64, arguments args: Any?) -> NSView {
    guard let plugin = plugin else {
      return NSView(frame: .zero)
    }
    return plugin.createView(arguments: args)
  }
}

private final class MacOSWebViewInstance: NSObject,
  WKNavigationDelegate,
  WKUIDelegate,
  WKScriptMessageHandler
{
  /// The native view is owned by the Flutter engine; this bridge only observes it.
  private weak var weakWebView: InteractiveWKWebView?
  /// Strong hand-off slot used exactly once by `takeView()` during creation.
  private var pendingView: InteractiveWKWebView?
  private let controllerId: Int
  private weak var plugin: WebviewCorePlugin?
  private var javaScriptEnabled: Bool
  private var zoomEnabled: Bool
  private var hasNavigationDecision: Bool
  private var registeredChannels: Set<String> = []
  private var progressObservation: NSKeyValueObservation?
  private var urlObservation: NSKeyValueObservation?
  private var isDisposed = false

  init(
    controllerId: Int,
    creationArgs: [String: Any],
    plugin: WebviewCorePlugin
  ) {
    self.controllerId = controllerId
    self.plugin = plugin
    self.javaScriptEnabled = creationArgs["javaScriptEnabled"] as? Bool ?? true
    self.zoomEnabled = creationArgs["zoomEnabled"] as? Bool ?? true
    self.hasNavigationDecision = creationArgs["hasNavigationDecision"] as? Bool ?? false

    let configuration = WKWebViewConfiguration()
    configuration.defaultWebpagePreferences.allowsContentJavaScript = self.javaScriptEnabled

    let webView = InteractiveWKWebView(frame: .zero, configuration: configuration)
    super.init()

    self.weakWebView = webView
    self.pendingView = webView
    webView.bridge = self
    webView.navigationDelegate = self
    webView.uiDelegate = self
    webView.autoresizesSubviews = true
    webView.wantsLayer = true
    webView.setValue(false, forKey: "drawsBackground")

    let rawColor = (creationArgs["backgroundColor"] as? NSNumber)?.uint64Value ?? 0
    applyBackgroundColor(UInt32(truncatingIfNeeded: rawColor))

    if let ua = creationArgs["userAgent"] as? String, !ua.isEmpty {
      webView.customUserAgent = ua
    }

    webView.allowsMagnification = zoomEnabled

    if let channels = creationArgs["channelNames"] as? [Any] {
      for item in channels {
        if let name = item as? String, !name.isEmpty {
          registeredChannels.insert(name)
          webView.configuration.userContentController.add(
            WeakScriptMessageHandler(target: self),
            name: name
          )
        }
      }
    }
    rebuildChannelUserScripts()

    progressObservation = webView.observe(\.estimatedProgress, options: [.new]) {
      [weak self] view, _ in
      guard let self = self, !self.isDisposed else { return }
      let pct = Int((view.estimatedProgress * 100.0).rounded())
      self.plugin?.sendEvent(
        "onProgress",
        arguments: ["controllerId": self.controllerId, "progress": min(100, max(0, pct))]
      )
    }

    urlObservation = webView.observe(\.url, options: [.new]) { [weak self] view, _ in
      guard let self = self, !self.isDisposed else { return }
      self.plugin?.sendEvent(
        "onUrlChange",
        arguments: ["controllerId": self.controllerId, "url": view.url?.absoluteString]
      )
    }

    if let initialHtml = creationArgs["initialHtml"] as? String {
      let baseUrlStr = creationArgs["initialBaseUrl"] as? String
      let baseUrl = baseUrlStr.flatMap { URL(string: $0) }
      webView.loadHTMLString(initialHtml, baseURL: baseUrl)
    } else if let initialUrl = creationArgs["initialUrl"] as? String,
              let url = URL(string: initialUrl)
    {
      let method = (creationArgs["initialMethod"] as? String ?? "GET").uppercased()
      let headers = extractHeaders(creationArgs["initialHeaders"])
      let body = (creationArgs["initialBody"] as? FlutterStandardTypedData)?.data
      loadRequestInternal(url: url, method: method, headers: headers, body: body)
    }

    DispatchQueue.main.async { [weak self] in
      guard let self = self, !self.isDisposed else { return }
      self.plugin?.sendEvent("onViewCreated", arguments: ["controllerId": self.controllerId])
    }
  }

  /// Hands the freshly created view to the engine (called exactly once).
  func takeView() -> NSView {
    defer { pendingView = nil }
    return pendingView ?? NSView(frame: .zero)
  }

  deinit {
    // Reached only after the engine released the view (the view retains this
    // bridge through `bridge`), so there is nothing left to detach.
    releaseResources(view: nil)
  }

  /// Explicit teardown requested by Dart (`WebViewController.dispose`).
  func dispose() {
    releaseResources(view: weakWebView)
  }

  private func releaseResources(view: InteractiveWKWebView?) {
    if isDisposed {
      return
    }
    isDisposed = true
    progressObservation?.invalidate()
    progressObservation = nil
    urlObservation?.invalidate()
    urlObservation = nil
    if let view = view {
      let controller = view.configuration.userContentController
      for name in registeredChannels {
        controller.removeScriptMessageHandler(forName: name)
      }
      controller.removeAllUserScripts()
      view.navigationDelegate = nil
      view.uiDelegate = nil
      view.stopLoading()
      view.removeFromSuperview()
      view.bridge = nil
    }
    registeredChannels.removeAll()
    plugin?.removeInstance(controllerId: controllerId, instance: self)
  }

  private func applyBackgroundColor(_ argb: UInt32) {
    guard let webView = weakWebView else { return }
    let a = CGFloat((argb >> 24) & 0xFF) / 255.0
    let r = CGFloat((argb >> 16) & 0xFF) / 255.0
    let g = CGFloat((argb >> 8) & 0xFF) / 255.0
    let b = CGFloat(argb & 0xFF) / 255.0
    let color = NSColor(srgbRed: r, green: g, blue: b, alpha: a)
    webView.wantsLayer = true
    webView.layer?.backgroundColor = color.cgColor
    if #available(macOS 12.0, *) {
      webView.underPageBackgroundColor = color
    }
  }

  private func rebuildChannelUserScripts() {
    guard let webView = weakWebView else { return }
    let controller = webView.configuration.userContentController
    controller.removeAllUserScripts()
    if !zoomEnabled {
      let noZoom = WKUserScript(
        source: "window.addEventListener('wheel',function(e){if(e.ctrlKey)e.preventDefault();},"
          + "{passive:false});",
        injectionTime: .atDocumentStart,
        forMainFrameOnly: false
      )
      controller.addUserScript(noZoom)
    }
    for name in registeredChannels {
      guard let nameData = try? JSONSerialization.data(withJSONObject: [name]),
            let jsonArr = String(data: nameData, encoding: .utf8),
            jsonArr.count >= 2
      else {
        continue
      }
      let encodedName = String(jsonArr.dropFirst().dropLast())
      let source = "window[\(encodedName)]={postMessage:function(m){"
        + "window.webkit.messageHandlers[\(encodedName)].postMessage(String(m));}};"
      let script = WKUserScript(
        source: source,
        injectionTime: .atDocumentStart,
        forMainFrameOnly: false
      )
      controller.addUserScript(script)
    }
  }

  private func addChannelInternal(_ name: String) {
    guard let webView = weakWebView, !registeredChannels.contains(name) else { return }
    registeredChannels.insert(name)
    webView.configuration.userContentController.add(
      WeakScriptMessageHandler(target: self),
      name: name
    )
    rebuildChannelUserScripts()
  }

  private func removeChannelInternal(_ name: String) {
    guard let webView = weakWebView, registeredChannels.remove(name) != nil else { return }
    webView.configuration.userContentController.removeScriptMessageHandler(forName: name)
    rebuildChannelUserScripts()
  }

  private func extractHeaders(_ raw: Any?) -> [String: String] {
    guard let dict = raw as? [AnyHashable: Any] else { return [:] }
    var out: [String: String] = [:]
    for (k, v) in dict {
      if let ks = k as? String, let vs = v as? String {
        out[ks] = vs
      }
    }
    return out
  }

  private func loadRequestInternal(
    url: URL,
    method: String,
    headers: [String: String],
    body: Data?
  ) {
    var request = URLRequest(url: url)
    request.httpMethod = method
    for (k, v) in headers {
      request.setValue(v, forHTTPHeaderField: k)
    }
    request.httpBody = body
    weakWebView?.load(request)
  }

  func userContentController(
    _ userContentController: WKUserContentController,
    didReceive message: WKScriptMessage
  ) {
    guard !isDisposed else { return }
    let text: String
    if let str = message.body as? String {
      text = str
    } else {
      text = "\(message.body)"
    }
    plugin?.sendEvent(
      "onJavaScriptMessage",
      arguments: [
        "controllerId": controllerId,
        "channel": message.name,
        "message": text,
      ]
    )
  }

  func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) {
    guard !isDisposed else { return }
    plugin?.sendEvent(
      "onPageStarted",
      arguments: [
        "controllerId": controllerId,
        "url": webView.url?.absoluteString ?? "",
      ]
    )
  }

  func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
    guard !isDisposed else { return }
    plugin?.sendEvent(
      "onPageFinished",
      arguments: [
        "controllerId": controllerId,
        "url": webView.url?.absoluteString ?? "",
      ]
    )
  }

  func webView(
    _ webView: WKWebView,
    didFail navigation: WKNavigation!,
    withError error: Error
  ) {
    reportError(error, isMainFrame: true)
  }

  func webView(
    _ webView: WKWebView,
    didFailProvisionalNavigation navigation: WKNavigation!,
    withError error: Error
  ) {
    reportError(error, isMainFrame: true)
  }

  /// Links with `target="_blank"` and `window.open(...)` request a new web view.
  /// Embedded views have no tab strip, so open `http(s)` targets in place
  /// instead of silently dropping the click.
  func webView(
    _ webView: WKWebView,
    createWebViewWith configuration: WKWebViewConfiguration,
    for navigationAction: WKNavigationAction,
    windowFeatures: WKWindowFeatures
  ) -> WKWebView? {
    guard !isDisposed,
          navigationAction.targetFrame == nil,
          let url = navigationAction.request.url,
          let scheme = url.scheme?.lowercased(),
          scheme == "http" || scheme == "https"
    else {
      return nil
    }
    webView.load(navigationAction.request)
    return nil
  }

  /// The WebContent process crashed or was killed by the system (memory
  /// pressure, sleep/wake). WebKit leaves a blank, input-dead view behind, so
  /// report it and reload automatically instead of leaving a frozen surface.
  func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
    guard !isDisposed else { return }
    plugin?.sendEvent(
      "onWebResourceError",
      arguments: [
        "controllerId": controllerId,
        "errorCode": -1,
        "description": "The web content process terminated unexpectedly.",
        "errorType": "webContentProcessTerminated",
        "isForMainFrame": true,
        "url": webView.url?.absoluteString,
      ]
    )
    webView.reload()
  }

  private func reportError(_ error: Error, isMainFrame: Bool) {
    guard !isDisposed else { return }
    let nsError = error as NSError
    if nsError.domain == NSURLErrorDomain && nsError.code == NSURLErrorCancelled {
      return
    }
    let failingUrl = (nsError.userInfo[NSURLErrorFailingURLStringErrorKey] as? String)
      ?? weakWebView?.url?.absoluteString
    plugin?.sendEvent(
      "onWebResourceError",
      arguments: [
        "controllerId": controllerId,
        "errorCode": nsError.code,
        "description": nsError.localizedDescription,
        "errorType": mapErrorCode(nsError),
        "isForMainFrame": isMainFrame,
        "url": failingUrl,
      ]
    )
  }

  func webView(
    _ webView: WKWebView,
    decidePolicyFor navigationAction: WKNavigationAction,
    preferences: WKWebpagePreferences,
    decisionHandler: @escaping (WKNavigationActionPolicy, WKWebpagePreferences) -> Void
  ) {
    preferences.allowsContentJavaScript = javaScriptEnabled
    guard !isDisposed,
          hasNavigationDecision,
          let targetUrl = navigationAction.request.url?.absoluteString,
          targetUrl != "about:blank"
    else {
      decisionHandler(.allow, preferences)
      return
    }
    let isMainFrame = navigationAction.targetFrame?.isMainFrame ?? true
    guard let plugin = plugin else {
      decisionHandler(.allow, preferences)
      return
    }
    plugin.requestNavigationDecision(
      controllerId: controllerId,
      url: targetUrl,
      isMainFrame: isMainFrame
    ) { allow in
      decisionHandler(allow ? .allow : .cancel, preferences)
    }
  }

  func handle(
    call: FlutterMethodCall,
    args: [String: Any],
    registrar: FlutterPluginRegistrar?,
    result: @escaping FlutterResult
  ) {
    guard !isDisposed, let webView = weakWebView else {
      result(nil)
      return
    }

    switch call.method {
    case "loadHtmlString":
      let html = args["html"] as? String ?? ""
      let baseUrlStr = args["baseUrl"] as? String
      let baseUrl = baseUrlStr.flatMap { URL(string: $0) }
      webView.loadHTMLString(html, baseURL: baseUrl)
      result(nil)

    case "loadRequest":
      let urlStr = args["url"] as? String ?? "about:blank"
      guard let url = URL(string: urlStr) else {
        result(FlutterError(code: "INVALID_URL", message: "Invalid URL: \(urlStr)", details: nil))
        return
      }
      let method = (args["method"] as? String ?? "GET").uppercased()
      let headers = extractHeaders(args["headers"])
      let body = (args["body"] as? FlutterStandardTypedData)?.data
      loadRequestInternal(url: url, method: method, headers: headers, body: body)
      result(nil)

    case "loadFile":
      let path = args["path"] as? String ?? ""
      let fileUrl = URL(fileURLWithPath: path)
      webView.loadFileURL(fileUrl, allowingReadAccessTo: fileUrl.deletingLastPathComponent())
      result(nil)

    case "loadFlutterAsset":
      guard let assetKey = args["assetKey"] as? String,
            let registrar = registrar
      else {
        result(false)
        return
      }
      let lookupKey = registrar.lookupKey(forAsset: assetKey)
      guard let assetPath = Bundle.main.path(forResource: lookupKey, ofType: nil) else {
        result(false)
        return
      }
      let fileUrl = URL(fileURLWithPath: assetPath)
      webView.loadFileURL(fileUrl, allowingReadAccessTo: fileUrl.deletingLastPathComponent())
      result(true)

    case "setJavaScriptEnabled":
      javaScriptEnabled = args["enabled"] as? Bool ?? true
      webView.configuration.defaultWebpagePreferences.allowsContentJavaScript = javaScriptEnabled
      result(nil)

    case "setBackgroundColor":
      let raw = (args["color"] as? NSNumber)?.uint64Value ?? 0
      applyBackgroundColor(UInt32(truncatingIfNeeded: raw))
      result(nil)

    case "setUserAgent":
      let ua = args["userAgent"] as? String
      webView.customUserAgent = (ua?.isEmpty == false) ? ua : nil
      result(nil)

    case "enableZoom":
      zoomEnabled = args["enabled"] as? Bool ?? true
      webView.allowsMagnification = zoomEnabled
      if !zoomEnabled {
        webView.magnification = 1.0
      }
      rebuildChannelUserScripts()
      result(nil)

    case "setHasNavigationDecision":
      hasNavigationDecision = args["enabled"] as? Bool ?? false
      result(nil)

    case "runJavaScript":
      let script = args["script"] as? String ?? ""
      webView.evaluateJavaScript(script) { _, _ in
        result(nil)
      }

    case "runJavaScriptReturningResult":
      let script = args["script"] as? String ?? ""
      webView.evaluateJavaScript(script) { value, error in
        if let error = error {
          result(
            FlutterError(
              code: "JS_ERROR",
              message: error.localizedDescription,
              details: nil
            )
          )
          return
        }
        result(Self.normalizeJsValue(value))
      }

    case "addJavaScriptChannel":
      if let name = args["name"] as? String, !name.isEmpty {
        addChannelInternal(name)
      }
      result(nil)

    case "removeJavaScriptChannel":
      if let name = args["name"] as? String, !name.isEmpty {
        removeChannelInternal(name)
      }
      result(nil)

    case "reload":
      webView.reload()
      result(nil)

    case "stopLoading":
      webView.stopLoading()
      result(nil)

    case "canGoBack":
      result(webView.canGoBack)

    case "canGoForward":
      result(webView.canGoForward)

    case "goBack":
      if webView.canGoBack {
        webView.goBack()
      }
      result(nil)

    case "goForward":
      if webView.canGoForward {
        webView.goForward()
      }
      result(nil)

    case "currentUrl":
      result(webView.url?.absoluteString)

    case "getTitle":
      result(webView.title)

    case "scrollTo":
      let x = (args["x"] as? NSNumber)?.intValue ?? 0
      let y = (args["y"] as? NSNumber)?.intValue ?? 0
      webView.evaluateJavaScript("window.scrollTo(\(x), \(y));") { _, _ in
        result(nil)
      }

    case "scrollBy":
      let dx = (args["x"] as? NSNumber)?.intValue ?? 0
      let dy = (args["y"] as? NSNumber)?.intValue ?? 0
      webView.evaluateJavaScript("window.scrollBy(\(dx), \(dy));") { _, _ in
        result(nil)
      }

    case "getScrollPosition":
      webView.evaluateJavaScript("[window.scrollX || 0, window.scrollY || 0]") { value, _ in
        if let arr = value as? [NSNumber], arr.count >= 2 {
          result(["x": arr[0].doubleValue, "y": arr[1].doubleValue])
        } else {
          result(["x": 0.0, "y": 0.0])
        }
      }

    case "clearCache":
      let types: Set<String> = [
        WKWebsiteDataTypeDiskCache,
        WKWebsiteDataTypeMemoryCache,
      ]
      WKWebsiteDataStore.default().removeData(ofTypes: types, modifiedSince: .distantPast) {
        result(nil)
      }

    case "clearLocalStorage":
      let types: Set<String> = [WKWebsiteDataTypeLocalStorage]
      WKWebsiteDataStore.default().removeData(ofTypes: types, modifiedSince: .distantPast) {
        result(nil)
      }

    default:
      result(FlutterMethodNotImplemented)
    }
  }

  private static func normalizeJsValue(_ value: Any?) -> Any? {
    guard let value = value else { return nil }
    if value is NSNull {
      return nil
    }
    if let num = value as? NSNumber {
      if CFGetTypeID(num) == CFBooleanGetTypeID() {
        return num.boolValue
      }
      let dbl = num.doubleValue
      if dbl.isFinite, dbl == dbl.rounded(), abs(dbl) <= 9_007_199_254_740_991.0 {
        return NSNumber(value: Int64(dbl))
      }
      return num
    }
    if let str = value as? String {
      return str
    }
    if JSONSerialization.isValidJSONObject(value),
       let data = try? JSONSerialization.data(withJSONObject: value),
       let jsonStr = String(data: data, encoding: .utf8)
    {
      return jsonStr
    }
    return "\(value)"
  }

  private func mapErrorCode(_ error: NSError) -> String {
    guard error.domain == NSURLErrorDomain else { return "unknown" }
    switch error.code {
    case NSURLErrorUserAuthenticationRequired:
      return "authentication"
    case NSURLErrorBadURL:
      return "badUrl"
    case NSURLErrorCannotConnectToHost, NSURLErrorNetworkConnectionLost:
      return "connect"
    case NSURLErrorSecureConnectionFailed,
      NSURLErrorServerCertificateUntrusted,
      NSURLErrorServerCertificateHasBadDate,
      NSURLErrorServerCertificateHasUnknownRoot:
      return "failedSslHandshake"
    case NSURLErrorFileDoesNotExist:
      return "fileNotFound"
    case NSURLErrorCannotFindHost, NSURLErrorDNSLookupFailed:
      return "hostLookup"
    case NSURLErrorHTTPTooManyRedirects:
      return "redirectLoop"
    case NSURLErrorTimedOut:
      return "timeout"
    case NSURLErrorUnsupportedURL:
      return "unsupportedScheme"
    default:
      return "unknown"
    }
  }
}
