import Flutter
import UIKit
import WebKit

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

private final class WeakPlatformViewBox {
  weak var value: IOSWebViewPlatformView?

  init(_ value: IOSWebViewPlatformView) {
    self.value = value
  }
}

public class WebviewCorePlugin: NSObject, FlutterPlugin {
  private weak var registrar: FlutterPluginRegistrar?
  private let channel: FlutterMethodChannel
  private var instances: [Int: WeakPlatformViewBox] = [:]

  init(registrar: FlutterPluginRegistrar, channel: FlutterMethodChannel) {
    self.registrar = registrar
    self.channel = channel
    super.init()
  }

  public static func register(with registrar: FlutterPluginRegistrar) {
    let channel = FlutterMethodChannel(
      name: "com.infradise.webview_core",
      binaryMessenger: registrar.messenger()
    )
    let plugin = WebviewCorePlugin(registrar: registrar, channel: channel)
    registrar.addMethodCallDelegate(plugin, channel: channel)
    let factory = IOSWebViewFactory(plugin: plugin)
    registrar.register(factory, withId: "com.infradise.webview_core/view")
  }

  fileprivate func createView(frame: CGRect, arguments: Any?) -> FlutterPlatformView {
    let map = arguments as? [String: Any] ?? [:]
    let controllerId = (map["controllerId"] as? NSNumber)?.intValue ?? 0
    instances.removeValue(forKey: controllerId)?.value?.dispose()
    let instance = IOSWebViewPlatformView(
      frame: frame,
      controllerId: controllerId,
      creationArgs: map,
      plugin: self
    )
    instances[controllerId] = WeakPlatformViewBox(instance)
    return instance
  }

  fileprivate func removeInstance(controllerId: Int, instance: IOSWebViewPlatformView) {
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

private final class IOSWebViewFactory: NSObject, FlutterPlatformViewFactory {
  private weak var plugin: WebviewCorePlugin?

  init(plugin: WebviewCorePlugin) {
    self.plugin = plugin
  }

  func createArgsCodec() -> FlutterMessageCodec & NSObjectProtocol {
    return FlutterStandardMessageCodec.sharedInstance()
  }

  func create(
    withFrame frame: CGRect,
    viewIdentifier viewId: Int64,
    arguments args: Any?
  ) -> FlutterPlatformView {
    guard let plugin = plugin else {
      return FallbackEmptyPlatformView(frame: frame)
    }
    return plugin.createView(frame: frame, arguments: args)
  }
}

private final class FallbackEmptyPlatformView: NSObject, FlutterPlatformView {
  private let emptyView: UIView

  init(frame: CGRect) {
    emptyView = UIView(frame: frame)
  }

  func view() -> UIView {
    return emptyView
  }
}

private final class IOSWebViewPlatformView: NSObject,
  FlutterPlatformView,
  WKNavigationDelegate,
  WKUIDelegate,
  WKScriptMessageHandler
{
  private let controllerId: Int
  private weak var plugin: WebviewCorePlugin?
  private let webView: WKWebView
  private var javaScriptEnabled: Bool
  private var zoomEnabled: Bool
  private var hasNavigationDecision: Bool
  private var registeredChannels: Set<String> = []
  private var progressObservation: NSKeyValueObservation?
  private var urlObservation: NSKeyValueObservation?
  private var isDisposed = false

  init(
    frame: CGRect,
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
    configuration.allowsInlineMediaPlayback = true

    self.webView = WKWebView(frame: frame, configuration: configuration)
    super.init()

    webView.navigationDelegate = self
    webView.uiDelegate = self
    webView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
    webView.isOpaque = false

    let rawColor = (creationArgs["backgroundColor"] as? NSNumber)?.uint64Value ?? 0
    applyBackgroundColor(UInt32(truncatingIfNeeded: rawColor))

    if let ua = creationArgs["userAgent"] as? String, !ua.isEmpty {
      webView.customUserAgent = ua
    }

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

    applyZoomSetting()
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

  deinit {
    dispose()
  }

  func view() -> UIView {
    return webView
  }

  func dispose() {
    if isDisposed {
      return
    }
    isDisposed = true
    progressObservation?.invalidate()
    progressObservation = nil
    urlObservation?.invalidate()
    urlObservation = nil
    let controller = webView.configuration.userContentController
    for name in registeredChannels {
      controller.removeScriptMessageHandler(forName: name)
    }
    registeredChannels.removeAll()
    controller.removeAllUserScripts()
    webView.navigationDelegate = nil
    webView.uiDelegate = nil
    webView.stopLoading()
    plugin?.removeInstance(controllerId: controllerId, instance: self)
  }

  private func applyBackgroundColor(_ argb: UInt32) {
    let a = CGFloat((argb >> 24) & 0xFF) / 255.0
    let r = CGFloat((argb >> 16) & 0xFF) / 255.0
    let g = CGFloat((argb >> 8) & 0xFF) / 255.0
    let b = CGFloat(argb & 0xFF) / 255.0
    let color = UIColor(red: r, green: g, blue: b, alpha: a)
    webView.isOpaque = false
    webView.backgroundColor = color
    webView.scrollView.backgroundColor = color
  }

  private func applyZoomSetting() {
    webView.scrollView.pinchGestureRecognizer?.isEnabled = zoomEnabled
  }

  private func rebuildChannelUserScripts() {
    let controller = webView.configuration.userContentController
    controller.removeAllUserScripts()
    if !zoomEnabled {
      let noZoom = WKUserScript(
        source: "var m=document.createElement('meta');m.name='viewport';"
          + "m.content='width=device-width,initial-scale=1.0,maximum-scale=1.0,user-scalable=no';"
          + "document.getElementsByTagName('head')[0]?.appendChild(m);",
        injectionTime: .atDocumentEnd,
        forMainFrameOnly: true
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
    guard !registeredChannels.contains(name) else { return }
    registeredChannels.insert(name)
    webView.configuration.userContentController.add(
      WeakScriptMessageHandler(target: self),
      name: name
    )
    rebuildChannelUserScripts()
  }

  private func removeChannelInternal(_ name: String) {
    guard registeredChannels.remove(name) != nil else { return }
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
    webView.load(request)
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
  /// instead of silently dropping the tap.
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

  /// The WebContent process crashed or was killed by the system. Report it and
  /// reload instead of leaving a blank, input-dead view.
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
      ?? webView.url?.absoluteString
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
    if isDisposed {
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
      applyZoomSetting()
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
      let x = (args["x"] as? NSNumber)?.doubleValue ?? 0.0
      let y = (args["y"] as? NSNumber)?.doubleValue ?? 0.0
      webView.scrollView.setContentOffset(CGPoint(x: x, y: y), animated: false)
      result(nil)

    case "scrollBy":
      let dx = (args["x"] as? NSNumber)?.doubleValue ?? 0.0
      let dy = (args["y"] as? NSNumber)?.doubleValue ?? 0.0
      let current = webView.scrollView.contentOffset
      webView.scrollView.setContentOffset(
        CGPoint(x: current.x + dx, y: current.y + dy),
        animated: false
      )
      result(nil)

    case "getScrollPosition":
      let offset = webView.scrollView.contentOffset
      result(["x": Double(offset.x), "y": Double(offset.y)])

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
