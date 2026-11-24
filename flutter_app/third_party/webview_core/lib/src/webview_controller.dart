import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';

import 'platform_bridge.dart';
import 'types.dart';

sealed class _InitialLoadSpec {
  const _InitialLoadSpec();
}

final class _PendingHtmlLoad extends _InitialLoadSpec {
  const _PendingHtmlLoad({required this.html, this.baseUrl});

  final String html;
  final String? baseUrl;
}

final class _PendingRequestLoad extends _InitialLoadSpec {
  const _PendingRequestLoad({
    required this.url,
    required this.method,
    required this.headers,
    this.body,
  });

  final String url;
  final String method;
  final Map<String, String> headers;
  final Uint8List? body;
}

final class _PendingFileLoad extends _InitialLoadSpec {
  const _PendingFileLoad({required this.path});

  final String path;
}

final class _PendingAssetLoad extends _InitialLoadSpec {
  const _PendingAssetLoad({required this.assetKey});

  final String assetKey;
}

/// Controller for a cross-platform `WebViewWidget` across Android, iOS, Linux,
/// macOS, Windows, and Web / WebAssembly.
///
/// Commands invoked prior to mounting a `WebViewWidget` are captured and
/// applied atomically during native/DOM view initialization without throwing
/// uninitialized exceptions or causing a flash of unstyled white background.
class WebViewController implements WebViewPlatformCallbacks {
  /// Creates a [WebViewController].
  WebViewController() : controllerId = ++_nextControllerId {
    WebViewPlatformBridge.active.registerController(controllerId, this);
  }

  static int _nextControllerId = 0;

  /// Unique identifier binding this controller to its platform view instance.
  final int controllerId;

  final ValueNotifier<Color> _backgroundColorNotifier = ValueNotifier<Color>(
    const Color(0x00000000),
  );

  bool _javaScriptEnabled = true;
  bool _zoomEnabled = true;
  String? _userAgent;
  String? _cachedCurrentUrl;
  NavigationDelegate? _navigationDelegate;
  final Map<String, void Function(JavaScriptMessage)> _channels =
      <String, void Function(JavaScriptMessage)>{};

  bool _creationParamsConsumed = false;
  bool _isViewReady = false;
  bool _isDisposed = false;
  _InitialLoadSpec? _lastLoadSpec;
  bool _deferredMountLoadQueued = false;
  final List<Future<void> Function()> _pendingPostCreateOps =
      <Future<void> Function()>[];

  /// Listenable tracking the active background color for backdrop widgets.
  ValueListenable<Color> get backgroundColorListenable =>
      _backgroundColorNotifier;

  /// Whether this controller has been disposed.
  bool get isDisposed => _isDisposed;

  /// Builds the initial creation parameter snapshot when mounting a view.
  WebViewCreationParams buildCreationParams({
    Set<Factory<OneSequenceGestureRecognizer>> gestureRecognizers =
        const <Factory<OneSequenceGestureRecognizer>>{},
  }) {
    _creationParamsConsumed = true;
    final loadSpec = _lastLoadSpec;

    String? initialHtml;
    String? initialBaseUrl;
    String? initialUrl;
    var initialMethod = 'GET';
    var initialHeaders = const <String, String>{};
    Uint8List? initialBody;

    if (loadSpec is _PendingHtmlLoad) {
      initialHtml = loadSpec.html;
      initialBaseUrl = loadSpec.baseUrl;
      _cachedCurrentUrl = loadSpec.baseUrl ?? 'about:blank';
    } else if (loadSpec is _PendingRequestLoad) {
      initialUrl = loadSpec.url;
      initialMethod = loadSpec.method;
      initialHeaders = loadSpec.headers;
      initialBody = loadSpec.body;
      _cachedCurrentUrl = loadSpec.url;
    } else if (loadSpec is _PendingFileLoad && !_deferredMountLoadQueued) {
      _deferredMountLoadQueued = true;
      _pendingPostCreateOps.insert(
        0,
        () =>
            WebViewPlatformBridge.active.loadFile(controllerId, loadSpec.path),
      );
    } else if (loadSpec is _PendingAssetLoad && !_deferredMountLoadQueued) {
      _deferredMountLoadQueued = true;
      _pendingPostCreateOps.insert(
        0,
        () => WebViewPlatformBridge.active.loadFlutterAsset(
          controllerId,
          loadSpec.assetKey,
        ),
      );
    }

    return WebViewCreationParams(
      controllerId: controllerId,
      javaScriptEnabled: _javaScriptEnabled,
      backgroundColor: _backgroundColorNotifier.value,
      zoomEnabled: _zoomEnabled,
      hasNavigationDecision: _navigationDelegate?.onNavigationRequest != null,
      channelNames: _channels.keys.toList(growable: false),
      gestureRecognizers: gestureRecognizers,
      userAgent: _userAgent,
      initialHtml: initialHtml,
      initialBaseUrl: initialBaseUrl,
      initialUrl: initialUrl,
      initialMethod: initialMethod,
      initialHeaders: initialHeaders,
      initialBody: initialBody,
    );
  }

  /// Resets mount tracking when a `WebViewWidget` unmounts and later remounts.
  void handleWidgetUnmounted() {
    _isViewReady = false;
    _creationParamsConsumed = false;
    _deferredMountLoadQueued = false;
  }

  Future<void> _runOrQueue(Future<void> Function() action) async {
    if (_isDisposed) {
      return;
    }
    if (_isViewReady) {
      await action();
      return;
    }
    _pendingPostCreateOps.add(action);
  }

  @override
  void handleViewCreated(int id) {
    if (_isDisposed || id != controllerId || _isViewReady) {
      return;
    }
    _isViewReady = true;
    if (_pendingPostCreateOps.isEmpty) {
      return;
    }
    final queued = List<Future<void> Function()>.of(_pendingPostCreateOps);
    _pendingPostCreateOps.clear();
    unawaited(_flushQueuedOps(queued));
  }

  Future<void> _flushQueuedOps(List<Future<void> Function()> ops) async {
    for (final op in ops) {
      if (_isDisposed) {
        return;
      }
      await op();
    }
  }

  @override
  void handlePageStarted(int id, String url) {
    if (_isDisposed || id != controllerId) {
      return;
    }
    _cachedCurrentUrl = url;
    _navigationDelegate?.onPageStarted?.call(url);
  }

  @override
  void handleProgress(int id, int progress) {
    if (_isDisposed || id != controllerId) {
      return;
    }
    _navigationDelegate?.onProgress?.call(progress);
  }

  @override
  void handlePageFinished(int id, String url) {
    if (_isDisposed || id != controllerId) {
      return;
    }
    _cachedCurrentUrl = url;
    _navigationDelegate?.onPageFinished?.call(url);
  }

  @override
  void handleUrlChange(int id, String? url) {
    if (_isDisposed || id != controllerId) {
      return;
    }
    _cachedCurrentUrl = url;
    _navigationDelegate?.onUrlChange?.call(UrlChange(url: url));
  }

  @override
  void handleScrollChanged(int id, Offset offset) {
    if (_isDisposed || id != controllerId) {
      return;
    }
    _navigationDelegate?.onScrollChanged?.call(offset);
  }

  @override
  void handleWebResourceError(int id, WebResourceError error) {
    if (_isDisposed || id != controllerId) {
      return;
    }
    _navigationDelegate?.onWebResourceError?.call(error);
  }

  @override
  FutureOr<bool> handleNavigationRequest(int id, NavigationRequest request) {
    if (_isDisposed || id != controllerId) {
      return false;
    }
    final callback = _navigationDelegate?.onNavigationRequest;
    if (callback == null) {
      return true;
    }
    final decision = callback(request);
    if (decision is NavigationDecision) {
      return decision == NavigationDecision.navigate;
    }
    return decision.then((NavigationDecision d) {
      return d == NavigationDecision.navigate;
    });
  }

  @override
  void handleJavaScriptMessage(int id, String channelName, String message) {
    if (_isDisposed || id != controllerId) {
      return;
    }
    final handler = _channels[channelName];
    handler?.call(JavaScriptMessage(message: message));
  }

  /// Loads an inline [html] document string with an optional [baseUrl].
  Future<void> loadHtmlString(String html, {String? baseUrl}) async {
    if (_isDisposed) {
      return;
    }
    _lastLoadSpec = _PendingHtmlLoad(html: html, baseUrl: baseUrl);
    _cachedCurrentUrl = baseUrl ?? 'about:blank';
    if (!_creationParamsConsumed && !_isViewReady) {
      return;
    }
    await _runOrQueue(
      () => WebViewPlatformBridge.active.loadHtmlString(
        controllerId,
        html,
        baseUrl: baseUrl,
      ),
    );
  }

  /// Loads the specified [uri] using [method], [headers], and optional [body].
  Future<void> loadRequest(
    Uri uri, {
    LoadRequestMethod method = LoadRequestMethod.get,
    Map<String, String> headers = const <String, String>{},
    Uint8List? body,
  }) async {
    if (_isDisposed) {
      return;
    }
    final urlString = uri.toString();
    final methodName = method.name.toUpperCase();
    final copiedHeaders = Map<String, String>.unmodifiable(headers);
    final copiedBody = body != null ? Uint8List.fromList(body) : null;
    _lastLoadSpec = _PendingRequestLoad(
      url: urlString,
      method: methodName,
      headers: copiedHeaders,
      body: copiedBody,
    );
    _cachedCurrentUrl = urlString;

    if (!_creationParamsConsumed && !_isViewReady) {
      return;
    }
    await _runOrQueue(
      () => WebViewPlatformBridge.active.loadRequest(
        controllerId,
        url: urlString,
        method: methodName,
        headers: copiedHeaders,
        body: copiedBody,
      ),
    );
  }

  /// Convenience helper loading [url] via HTTP `GET` with optional [headers].
  Future<void> loadUrl(String url, {Map<String, String>? headers}) {
    return loadRequest(
      Uri.parse(url),
      headers: headers ?? const <String, String>{},
    );
  }

  /// Loads a local HTML file located at [absoluteFilePath].
  Future<void> loadFile(String absoluteFilePath) async {
    if (_isDisposed) {
      return;
    }
    _lastLoadSpec = _PendingFileLoad(path: absoluteFilePath);
    _cachedCurrentUrl = Uri.file(absoluteFilePath).toString();
    if (!_creationParamsConsumed && !_isViewReady) {
      return;
    }
    await _runOrQueue(
      () =>
          WebViewPlatformBridge.active.loadFile(controllerId, absoluteFilePath),
    );
  }

  /// Loads a Flutter asset identified by [key] from the asset bundle.
  Future<void> loadFlutterAsset(String key) async {
    if (_isDisposed) {
      return;
    }
    _lastLoadSpec = _PendingAssetLoad(assetKey: key);
    if (!_creationParamsConsumed && !_isViewReady) {
      return;
    }
    await _runOrQueue(
      () => WebViewPlatformBridge.active.loadFlutterAsset(controllerId, key),
    );
  }

  /// Sets the [JavaScriptMode] (`disabled` or `unrestricted`) of the WebView.
  Future<void> setJavaScriptMode(JavaScriptMode javaScriptMode) {
    return setJavaScriptEnabled(javaScriptMode == JavaScriptMode.unrestricted);
  }

  /// Enables or disables JavaScript execution inside the WebView.
  Future<void> setJavaScriptEnabled(bool enabled) async {
    if (_isDisposed) {
      return;
    }
    _javaScriptEnabled = enabled;
    if (!_creationParamsConsumed && !_isViewReady) {
      return;
    }
    await _runOrQueue(
      () => WebViewPlatformBridge.active.setJavaScriptEnabled(
        controllerId,
        enabled,
      ),
    );
  }

  /// Sets the background [color] of the WebView across all 6 platforms,
  /// including macOS, Linux, Windows, iOS, Android, and Web.
  Future<void> setBackgroundColor(Color color) async {
    if (_isDisposed) {
      return;
    }
    _backgroundColorNotifier.value = color;
    if (!_creationParamsConsumed && !_isViewReady) {
      return;
    }
    await _runOrQueue(
      () =>
          WebViewPlatformBridge.active.setBackgroundColor(controllerId, color),
    );
  }

  /// Overrides the HTTP `User-Agent` header with [userAgent], or resets to the
  /// platform default when `null`.
  Future<void> setUserAgent(String? userAgent) async {
    if (_isDisposed) {
      return;
    }
    _userAgent = userAgent;
    if (!_creationParamsConsumed && !_isViewReady) {
      return;
    }
    await _runOrQueue(
      () => WebViewPlatformBridge.active.setUserAgent(controllerId, userAgent),
    );
  }

  /// Returns the configured custom `User-Agent`, or queries `navigator.userAgent`
  /// when mounted.
  Future<String?> getUserAgent() async {
    if (_userAgent != null) {
      return _userAgent;
    }
    if (!_isViewReady || _isDisposed) {
      return null;
    }
    return evaluateJavaScript('navigator.userAgent');
  }

  /// Enables or disables pinch/page zoom gestures.
  Future<void> enableZoom(bool enabled) async {
    if (_isDisposed) {
      return;
    }
    _zoomEnabled = enabled;
    if (!_creationParamsConsumed && !_isViewReady) {
      return;
    }
    await _runOrQueue(
      () => WebViewPlatformBridge.active.enableZoom(controllerId, enabled),
    );
  }

  /// Attaches a [NavigationDelegate] receiving lifecycle and policy callbacks.
  Future<void> setNavigationDelegate(NavigationDelegate delegate) async {
    if (_isDisposed) {
      return;
    }
    _navigationDelegate = delegate;
    final hasDecision = delegate.onNavigationRequest != null;
    if (!_creationParamsConsumed && !_isViewReady) {
      return;
    }
    await _runOrQueue(
      () => WebViewPlatformBridge.active.setHasNavigationDecision(
        controllerId,
        hasDecision,
      ),
    );
  }

  /// Evaluates [javaScript] in the context of the current page.
  Future<void> runJavaScript(String javaScript) async {
    await _runOrQueue(
      () =>
          WebViewPlatformBridge.active.runJavaScript(controllerId, javaScript),
    );
  }

  /// Evaluates [javaScript] and returns the resulting value.
  Future<Object> runJavaScriptReturningResult(String javaScript) async {
    if (_isDisposed || !_isViewReady) {
      return '';
    }
    final result = await WebViewPlatformBridge.active
        .runJavaScriptReturningResult(controllerId, javaScript);
    return result ?? '';
  }

  /// Evaluates [script] and returns its result formatted as a nullable `String`.
  Future<String?> evaluateJavaScript(String script) async {
    if (_isDisposed || !_isViewReady) {
      return null;
    }
    final raw = await WebViewPlatformBridge.active.runJavaScriptReturningResult(
      controllerId,
      script,
    );
    if (raw == null) {
      return null;
    }
    if (raw is String) {
      return raw;
    }
    if (raw is num || raw is bool) {
      return raw.toString();
    }
    return jsonEncode(raw);
  }

  /// Registers a JavaScript channel named [name] exposed to page scripts as
  /// `window.<name>.postMessage(message)`.
  Future<void> addJavaScriptChannel(
    String name, {
    required void Function(JavaScriptMessage) onMessageReceived,
  }) async {
    if (name.isEmpty) {
      throw ArgumentError.value(name, 'name', 'Channel name must not be empty');
    }
    if (_isDisposed) {
      return;
    }
    _channels[name] = onMessageReceived;
    if (!_creationParamsConsumed && !_isViewReady) {
      return;
    }
    await _runOrQueue(
      () =>
          WebViewPlatformBridge.active.addJavaScriptChannel(controllerId, name),
    );
  }

  /// Unregisters a previously added JavaScript channel named
  /// [javaScriptChannelName].
  Future<void> removeJavaScriptChannel(String javaScriptChannelName) async {
    if (_isDisposed) {
      return;
    }
    _channels.remove(javaScriptChannelName);
    if (!_creationParamsConsumed && !_isViewReady) {
      return;
    }
    await _runOrQueue(
      () => WebViewPlatformBridge.active.removeJavaScriptChannel(
        controllerId,
        javaScriptChannelName,
      ),
    );
  }

  /// Reloads the current page.
  Future<void> reload() async {
    await _runOrQueue(() => WebViewPlatformBridge.active.reload(controllerId));
  }

  /// Stops loading the current page.
  Future<void> stopLoading() async {
    await _runOrQueue(
      () => WebViewPlatformBridge.active.stopLoading(controllerId),
    );
  }

  /// Returns `true` when there is a backward entry in navigation history.
  Future<bool> canGoBack() async {
    if (_isDisposed || !_isViewReady) {
      return false;
    }
    return WebViewPlatformBridge.active.canGoBack(controllerId);
  }

  /// Returns `true` when there is a forward entry in navigation history.
  Future<bool> canGoForward() async {
    if (_isDisposed || !_isViewReady) {
      return false;
    }
    return WebViewPlatformBridge.active.canGoForward(controllerId);
  }

  /// Navigates one step backward in the WebView history stack.
  Future<void> goBack() async {
    await _runOrQueue(() => WebViewPlatformBridge.active.goBack(controllerId));
  }

  /// Navigates one step forward in the WebView history stack.
  Future<void> goForward() async {
    await _runOrQueue(
      () => WebViewPlatformBridge.active.goForward(controllerId),
    );
  }

  /// Returns the currently loaded URL, or `null` if no page has been loaded.
  Future<String?> currentUrl() async {
    if (_isDisposed) {
      return null;
    }
    if (!_isViewReady) {
      return _cachedCurrentUrl;
    }
    return await WebViewPlatformBridge.active.currentUrl(controllerId) ??
        _cachedCurrentUrl;
  }

  /// Returns the title of the currently loaded document.
  Future<String?> getTitle() async {
    if (_isDisposed || !_isViewReady) {
      return null;
    }
    return WebViewPlatformBridge.active.getTitle(controllerId);
  }

  /// Scrolls the WebView document to (`[x]`, `[y]`).
  Future<void> scrollTo(int x, int y) async {
    await _runOrQueue(
      () => WebViewPlatformBridge.active.scrollTo(controllerId, x, y),
    );
  }

  /// Scrolls the WebView document by (`[x]`, `[y]`).
  Future<void> scrollBy(int x, int y) async {
    await _runOrQueue(
      () => WebViewPlatformBridge.active.scrollBy(controllerId, x, y),
    );
  }

  /// Returns the current scroll offset of the WebView document.
  Future<Offset> getScrollPosition() async {
    if (_isDisposed || !_isViewReady) {
      return Offset.zero;
    }
    return WebViewPlatformBridge.active.getScrollPosition(controllerId);
  }

  /// Clears cached HTTP/disk resources associated with this WebView.
  Future<void> clearCache() async {
    await _runOrQueue(
      () => WebViewPlatformBridge.active.clearCache(controllerId),
    );
  }

  /// Clears `window.localStorage` entries associated with this WebView.
  Future<void> clearLocalStorage() async {
    await _runOrQueue(
      () => WebViewPlatformBridge.active.clearLocalStorage(controllerId),
    );
  }

  /// Releases native or DOM resources held by this controller.
  Future<void> dispose() async {
    if (_isDisposed) {
      return;
    }
    _isDisposed = true;
    _lastLoadSpec = null;
    _pendingPostCreateOps.clear();
    WebViewPlatformBridge.active.unregisterController(controllerId);
    _backgroundColorNotifier.dispose();
    await WebViewPlatformBridge.active.dispose(controllerId);
  }
}
