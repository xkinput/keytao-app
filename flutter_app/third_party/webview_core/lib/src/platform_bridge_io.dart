import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'platform_bridge.dart';
import 'types.dart';

/// Native `MethodChannel` bridge instance for `Android`, `iOS`, `Linux`,
/// `macOS`, and `Windows`.
final WebViewPlatformBridge platformBridgeInstance = IoWebViewBridge();

bool _isRenderSubtreeActive(RenderObject object) {
  RenderObject? branch = object;
  var parent = object.parent;
  while (parent != null) {
    if (parent is RenderOffstage && parent.offstage) {
      return false;
    }
    if (parent is RenderAnimatedOpacity && parent.opacity.value == 0.0) {
      return false;
    }
    if (parent is RenderOpacity && parent.opacity == 0.0) {
      return false;
    }
    if (parent is RenderIndexedStack) {
      final activeIndex = parent.index;
      if (activeIndex == null) {
        return false;
      }
      var child = parent.firstChild;
      var index = 0;
      var matchedActive = false;
      while (child != null) {
        if (identical(child, branch)) {
          matchedActive = index == activeIndex;
          break;
        }
        child = parent.childAfter(child);
        index += 1;
      }
      if (!matchedActive) {
        return false;
      }
    }
    branch = parent;
    parent = parent.parent;
  }
  return true;
}

/// Keys the Windows host never swallows so Flutter focus traversal, function
/// key shortcuts, and modifier-only presses keep working while the WebView
/// holds focus.
final Set<LogicalKeyboardKey> _passThroughKeys = <LogicalKeyboardKey>{
  LogicalKeyboardKey.escape,
  LogicalKeyboardKey.tab,
  LogicalKeyboardKey.f1,
  LogicalKeyboardKey.f2,
  LogicalKeyboardKey.f3,
  LogicalKeyboardKey.f4,
  LogicalKeyboardKey.f5,
  LogicalKeyboardKey.f6,
  LogicalKeyboardKey.f7,
  LogicalKeyboardKey.f8,
  LogicalKeyboardKey.f9,
  LogicalKeyboardKey.f10,
  LogicalKeyboardKey.f11,
  LogicalKeyboardKey.f12,
  LogicalKeyboardKey.shiftLeft,
  LogicalKeyboardKey.shiftRight,
  LogicalKeyboardKey.controlLeft,
  LogicalKeyboardKey.controlRight,
  LogicalKeyboardKey.altLeft,
  LogicalKeyboardKey.altRight,
  LogicalKeyboardKey.metaLeft,
  LogicalKeyboardKey.metaRight,
  LogicalKeyboardKey.capsLock,
};

bool _isPassThroughKey(LogicalKeyboardKey key) =>
    _passThroughKeys.contains(key);

/// Native platform implementation of [WebViewPlatformBridge].
final class IoWebViewBridge extends WebViewPlatformBridge {
  /// Method channel shared across native WebView instances.
  static const MethodChannel channel = MethodChannel(
    'com.infradise.webview_core',
  );

  /// Platform view identifier registered on Android, iOS, and macOS.
  static const String viewType = 'com.infradise.webview_core/view';

  final Map<int, WebViewPlatformCallbacks> _controllers =
      <int, WebViewPlatformCallbacks>{};
  bool _handlerRegistered = false;

  void _ensureHandlerRegistered() {
    if (_handlerRegistered) {
      return;
    }
    if (kDebugMode && BindingBase.debugBindingType() == null) {
      return;
    }
    channel.setMethodCallHandler(_handleMethodCall);
    _handlerRegistered = true;
  }

  Future<Object?> _handleMethodCall(MethodCall call) async {
    final rawArgs = call.arguments;
    if (rawArgs is! Map<Object?, Object?>) {
      return null;
    }
    final controllerId = rawArgs['controllerId'];
    if (controllerId is! int) {
      return null;
    }
    final callbacks = _controllers[controllerId];
    if (callbacks == null) {
      return null;
    }

    switch (call.method) {
      case 'onViewCreated':
        callbacks.handleViewCreated(controllerId);
        return null;
      case 'onPageStarted':
        final url = rawArgs['url'] as String? ?? '';
        callbacks.handlePageStarted(controllerId, url);
        return null;
      case 'onProgress':
        final progress = (rawArgs['progress'] as num?)?.toInt() ?? 0;
        callbacks.handleProgress(controllerId, progress.clamp(0, 100));
        return null;
      case 'onPageFinished':
        final url = rawArgs['url'] as String? ?? '';
        callbacks.handlePageFinished(controllerId, url);
        return null;
      case 'onUrlChange':
        final url = rawArgs['url'] as String?;
        callbacks.handleUrlChange(controllerId, url);
        return null;
      case 'onScrollChanged':
        final x = rawArgs['x'];
        final y = rawArgs['y'];
        if (x is num && y is num && x.isFinite && y.isFinite) {
          callbacks.handleScrollChanged(
            controllerId,
            Offset(x.toDouble(), y.toDouble()),
          );
        }
        return null;
      case 'onWebResourceError':
        final errorCode = (rawArgs['errorCode'] as num?)?.toInt() ?? -1;
        final description = rawArgs['description'] as String? ?? 'Unknown';
        final rawType = rawArgs['errorType'] as String?;
        final isMain = rawArgs['isForMainFrame'] as bool?;
        final url = rawArgs['url'] as String?;
        callbacks.handleWebResourceError(
          controllerId,
          WebResourceError(
            errorCode: errorCode,
            description: description,
            errorType: _parseErrorType(rawType),
            isForMainFrame: isMain,
            url: url,
          ),
        );
        return null;
      case 'onNavigationRequest':
        final url = rawArgs['url'] as String? ?? '';
        final isMainFrame = rawArgs['isMainFrame'] as bool? ?? true;
        final allow = await callbacks.handleNavigationRequest(
          controllerId,
          NavigationRequest(url: url, isMainFrame: isMainFrame),
        );
        return allow;
      case 'onJavaScriptMessage':
        final channelName = rawArgs['channel'] as String? ?? '';
        final message = rawArgs['message'] as String? ?? '';
        callbacks.handleJavaScriptMessage(controllerId, channelName, message);
        return null;
      default:
        return null;
    }
  }

  static WebResourceErrorType _parseErrorType(String? value) {
    if (value == null) {
      return WebResourceErrorType.unknown;
    }
    for (final entry in WebResourceErrorType.values) {
      if (entry.name == value) {
        return entry;
      }
    }
    return WebResourceErrorType.unknown;
  }

  @override
  void registerController(
    int controllerId,
    WebViewPlatformCallbacks callbacks,
  ) {
    _controllers[controllerId] = callbacks;
    _ensureHandlerRegistered();
  }

  @override
  void unregisterController(int controllerId) {
    _controllers.remove(controllerId);
  }

  /// Notifies the registered controller that its platform view is ready.
  void notifyViewCreated(int controllerId) {
    _controllers[controllerId]?.handleViewCreated(controllerId);
  }

  @override
  Widget buildView({
    required BuildContext context,
    required WebViewCreationParams params,
    Key? key,
  }) {
    _ensureHandlerRegistered();
    switch (defaultTargetPlatform) {
      case TargetPlatform.android:
        return ColoredBox(
          key: key,
          color: params.backgroundColor,
          child: AndroidView(
            viewType: viewType,
            creationParams: params.toMap(),
            creationParamsCodec: const StandardMessageCodec(),
            gestureRecognizers: params.gestureRecognizers,
            onPlatformViewCreated: (_) =>
                notifyViewCreated(params.controllerId),
          ),
        );
      case TargetPlatform.iOS:
        return ColoredBox(
          key: key,
          color: params.backgroundColor,
          child: UiKitView(
            viewType: viewType,
            creationParams: params.toMap(),
            creationParamsCodec: const StandardMessageCodec(),
            gestureRecognizers: params.gestureRecognizers,
            onPlatformViewCreated: (_) =>
                notifyViewCreated(params.controllerId),
          ),
        );
      case TargetPlatform.macOS:
        return ColoredBox(
          key: key,
          color: params.backgroundColor,
          child: AppKitView(
            viewType: viewType,
            creationParams: params.toMap(),
            creationParamsCodec: const StandardMessageCodec(),
            onPlatformViewCreated: (_) =>
                notifyViewCreated(params.controllerId),
          ),
        );
      case TargetPlatform.linux:
        return _LinuxWebViewHost(key: key, bridge: this, params: params);
      case TargetPlatform.windows:
        return _WindowsWebViewHost(key: key, bridge: this, params: params);
      case TargetPlatform.fuchsia:
        return ColoredBox(
          key: key,
          color: params.backgroundColor,
          child: const SizedBox.expand(),
        );
    }
  }

  /// Creates the native `WebKitWebView` overlay instance on Linux.
  Future<void> createLinuxView(WebViewCreationParams params) async {
    _ensureHandlerRegistered();
    try {
      await channel.invokeMethod<Object?>('create', params.toMap());
      notifyViewCreated(params.controllerId);
    } on MissingPluginException {
      // Safe no-op in headless widget tests without a registered mock handler.
    }
  }

  /// Updates the visibility, pointer pass-through state, clipped viewport rect,
  /// and full layout rect of a Linux `GtkOverlay` WebView.
  Future<void> setLinuxGeometry({
    required int controllerId,
    required bool visible,
    required int x,
    required int y,
    required int width,
    required int height,
    int? fullX,
    int? fullY,
    int? fullWidth,
    int? fullHeight,
    bool passThrough = false,
  }) async {
    _ensureHandlerRegistered();
    try {
      await channel.invokeMethod<void>('setGeometry', <String, Object?>{
        'controllerId': controllerId,
        'visible': visible,
        'passThrough': passThrough,
        'x': x,
        'y': y,
        'width': width,
        'height': height,
        'fullX': fullX ?? x,
        'fullY': fullY ?? y,
        'fullWidth': fullWidth ?? width,
        'fullHeight': fullHeight ?? height,
      });
    } on MissingPluginException {
      // Safe no-op in headless widget tests.
    }
  }

  /// Creates the native `WebView2` composition texture instance on Windows.
  Future<int?> createWindowsView(WebViewCreationParams params) async {
    _ensureHandlerRegistered();
    try {
      final result = await channel.invokeMethod<int>('create', params.toMap());
      if (result != null && result >= 0) {
        notifyViewCreated(params.controllerId);
      }
      return result;
    } on MissingPluginException {
      return null;
    }
  }

  /// Resizes the off-screen composition surface on Windows.
  Future<void> setWindowsSize({
    required int controllerId,
    required int width,
    required int height,
    required double scaleFactor,
  }) async {
    try {
      await channel.invokeMethod<void>('setSize', <String, Object?>{
        'controllerId': controllerId,
        'width': width,
        'height': height,
        'scaleFactor': scaleFactor,
      });
    } on MissingPluginException {
      // Safe no-op in headless widget tests.
    }
  }

  /// Updates the visibility of the Windows `WebView2` composition controller.
  Future<void> setWindowsVisibility({
    required int controllerId,
    required bool visible,
  }) async {
    try {
      await channel.invokeMethod<void>('setVisibility', <String, Object?>{
        'controllerId': controllerId,
        'visible': visible,
      });
    } on MissingPluginException {
      // Safe no-op in headless widget tests.
    }
  }

  /// Forwards pointer or scroll wheel input to Windows `WebView2`.
  Future<void> sendWindowsPointerEvent({
    required int controllerId,
    required String kind,
    required int button,
    required int buttons,
    required double x,
    required double y,
    double deltaX = 0.0,
    double deltaY = 0.0,
    bool shift = false,
    bool control = false,
    bool alt = false,
  }) async {
    try {
      await channel.invokeMethod<void>('pointerEvent', <String, Object?>{
        'controllerId': controllerId,
        'kind': kind,
        'button': button,
        'buttons': buttons,
        'x': x,
        'y': y,
        'deltaX': deltaX,
        'deltaY': deltaY,
        'shift': shift,
        'control': control,
        'alt': alt,
      });
    } on MissingPluginException {
      // Safe no-op in headless widget tests.
    }
  }

  /// Forwards keyboard input and clipboard/editing shortcuts to Windows
  /// `WebView2` when Flutter's host window holds Win32 focus.
  Future<void> sendWindowsKeyEvent({
    required int controllerId,
    required String logicalKey,
    required int keyId,
    String? character,
    bool shift = false,
    bool control = false,
    bool alt = false,
    bool meta = false,
  }) async {
    try {
      await channel.invokeMethod<void>('keyEvent', <String, Object?>{
        'controllerId': controllerId,
        'logicalKey': logicalKey,
        'keyId': keyId,
        'character': character,
        'shift': shift,
        'control': control,
        'alt': alt,
        'meta': meta,
      });
    } on MissingPluginException {
      // Safe no-op in headless widget tests.
    }
  }

  /// Forwards keyboard/focus state changes to Windows `WebView2`.
  Future<void> setWindowsFocus({
    required int controllerId,
    required bool focused,
  }) async {
    try {
      await channel.invokeMethod<void>('setFocus', <String, Object?>{
        'controllerId': controllerId,
        'focused': focused,
      });
    } on MissingPluginException {
      // Safe no-op in headless widget tests.
    }
  }

  Future<T?> _invoke<T>(String method, Map<String, Object?> args) async {
    _ensureHandlerRegistered();
    try {
      return await channel.invokeMethod<T>(method, args);
    } on MissingPluginException {
      return null;
    }
  }

  @override
  Future<void> loadHtmlString(
    int controllerId,
    String html, {
    String? baseUrl,
  }) async {
    await _invoke<void>('loadHtmlString', <String, Object?>{
      'controllerId': controllerId,
      'html': html,
      'baseUrl': baseUrl,
    });
  }

  @override
  Future<void> loadRequest(
    int controllerId, {
    required String url,
    required String method,
    required Map<String, String> headers,
    Uint8List? body,
  }) async {
    await _invoke<void>('loadRequest', <String, Object?>{
      'controllerId': controllerId,
      'url': url,
      'method': method,
      'headers': headers,
      'body': body,
    });
  }

  @override
  Future<void> loadFile(int controllerId, String path) async {
    await _invoke<void>('loadFile', <String, Object?>{
      'controllerId': controllerId,
      'path': path,
    });
  }

  @override
  Future<void> loadFlutterAsset(int controllerId, String assetKey) async {
    final resolved = await _invoke<bool>('loadFlutterAsset', <String, Object?>{
      'controllerId': controllerId,
      'assetKey': assetKey,
    });
    if (resolved == true) {
      return;
    }
    final html = await rootBundle.loadString(assetKey);
    await loadHtmlString(controllerId, html);
  }

  @override
  Future<void> setJavaScriptEnabled(int controllerId, bool enabled) async {
    await _invoke<void>('setJavaScriptEnabled', <String, Object?>{
      'controllerId': controllerId,
      'enabled': enabled,
    });
  }

  @override
  Future<void> setBackgroundColor(int controllerId, Color color) async {
    await _invoke<void>('setBackgroundColor', <String, Object?>{
      'controllerId': controllerId,
      'color': color.toARGB32(),
    });
  }

  @override
  Future<void> setUserAgent(int controllerId, String? userAgent) async {
    await _invoke<void>('setUserAgent', <String, Object?>{
      'controllerId': controllerId,
      'userAgent': userAgent,
    });
  }

  @override
  Future<void> enableZoom(int controllerId, bool enabled) async {
    await _invoke<void>('enableZoom', <String, Object?>{
      'controllerId': controllerId,
      'enabled': enabled,
    });
  }

  @override
  Future<void> setHasNavigationDecision(int controllerId, bool enabled) async {
    await _invoke<void>('setHasNavigationDecision', <String, Object?>{
      'controllerId': controllerId,
      'enabled': enabled,
    });
  }

  @override
  Future<void> runJavaScript(int controllerId, String script) async {
    await _invoke<void>('runJavaScript', <String, Object?>{
      'controllerId': controllerId,
      'script': script,
    });
  }

  @override
  Future<Object?> runJavaScriptReturningResult(
    int controllerId,
    String script,
  ) async {
    return _invoke<Object?>('runJavaScriptReturningResult', <String, Object?>{
      'controllerId': controllerId,
      'script': script,
    });
  }

  @override
  Future<void> addJavaScriptChannel(int controllerId, String name) async {
    await _invoke<void>('addJavaScriptChannel', <String, Object?>{
      'controllerId': controllerId,
      'name': name,
    });
  }

  @override
  Future<void> removeJavaScriptChannel(int controllerId, String name) async {
    await _invoke<void>('removeJavaScriptChannel', <String, Object?>{
      'controllerId': controllerId,
      'name': name,
    });
  }

  @override
  Future<void> reload(int controllerId) async {
    await _invoke<void>('reload', <String, Object?>{
      'controllerId': controllerId,
    });
  }

  @override
  Future<void> stopLoading(int controllerId) async {
    await _invoke<void>('stopLoading', <String, Object?>{
      'controllerId': controllerId,
    });
  }

  @override
  Future<bool> canGoBack(int controllerId) async {
    final result = await _invoke<bool>('canGoBack', <String, Object?>{
      'controllerId': controllerId,
    });
    return result ?? false;
  }

  @override
  Future<bool> canGoForward(int controllerId) async {
    final result = await _invoke<bool>('canGoForward', <String, Object?>{
      'controllerId': controllerId,
    });
    return result ?? false;
  }

  @override
  Future<void> goBack(int controllerId) async {
    await _invoke<void>('goBack', <String, Object?>{
      'controllerId': controllerId,
    });
  }

  @override
  Future<void> goForward(int controllerId) async {
    await _invoke<void>('goForward', <String, Object?>{
      'controllerId': controllerId,
    });
  }

  @override
  Future<String?> currentUrl(int controllerId) async {
    return _invoke<String?>('currentUrl', <String, Object?>{
      'controllerId': controllerId,
    });
  }

  @override
  Future<String?> getTitle(int controllerId) async {
    return _invoke<String?>('getTitle', <String, Object?>{
      'controllerId': controllerId,
    });
  }

  @override
  Future<void> scrollTo(int controllerId, int x, int y) async {
    await _invoke<void>('scrollTo', <String, Object?>{
      'controllerId': controllerId,
      'x': x,
      'y': y,
    });
  }

  @override
  Future<void> scrollBy(int controllerId, int x, int y) async {
    await _invoke<void>('scrollBy', <String, Object?>{
      'controllerId': controllerId,
      'x': x,
      'y': y,
    });
  }

  @override
  Future<Offset> getScrollPosition(int controllerId) async {
    final map = await _invoke<Map<Object?, Object?>>(
      'getScrollPosition',
      <String, Object?>{'controllerId': controllerId},
    );
    if (map == null) {
      return Offset.zero;
    }
    final x = (map['x'] as num?)?.toDouble() ?? 0.0;
    final y = (map['y'] as num?)?.toDouble() ?? 0.0;
    return Offset(x, y);
  }

  @override
  Future<void> clearCache(int controllerId) async {
    await _invoke<void>('clearCache', <String, Object?>{
      'controllerId': controllerId,
    });
  }

  @override
  Future<void> clearLocalStorage(int controllerId) async {
    await _invoke<void>('clearLocalStorage', <String, Object?>{
      'controllerId': controllerId,
    });
  }

  @override
  Future<bool> clearCookies() async {
    final cleared = await _invoke<bool>('clearCookies', const <String, Object?>{
      'controllerId': 0,
    });
    return cleared ?? false;
  }

  @override
  Future<void> setCookie(WebViewCookie cookie) async {
    await _invoke<void>('setCookie', <String, Object?>{
      'controllerId': 0,
      ...cookie.toMap(),
    });
  }

  @override
  Future<void> dispose(int controllerId) async {
    await _invoke<void>('dispose', <String, Object?>{
      'controllerId': controllerId,
    });
  }
}

final Set<_LinuxWebViewHostState> _mountedLinuxHosts =
    <_LinuxWebViewHostState>{};
bool _linuxPersistentCallbackInstalled = false;

void _ensureLinuxPersistentCallback() {
  if (_linuxPersistentCallbackInstalled) {
    return;
  }
  _linuxPersistentCallbackInstalled = true;
  SchedulerBinding.instance.addPersistentFrameCallback((_) {
    if (_mountedLinuxHosts.isEmpty) {
      return;
    }
    for (final host in _mountedLinuxHosts.toList(growable: false)) {
      host._syncAfterFrame();
    }
  });
}

/// Event-driven Linux `GtkOverlay` host widget that synchronizes visibility,
/// `IndexedStack` / `Offstage` / popup & modal occlusion, Flutter drag
/// pass-through, and clipped viewport geometry without idle frame polling.
class _LinuxWebViewHost extends StatefulWidget {
  const _LinuxWebViewHost({
    required this.bridge,
    required this.params,
    super.key,
  });

  final IoWebViewBridge bridge;
  final WebViewCreationParams params;

  @override
  State<_LinuxWebViewHost> createState() => _LinuxWebViewHostState();
}

class _LinuxWebViewHostState extends State<_LinuxWebViewHost>
    with WidgetsBindingObserver {
  final GlobalKey _surfaceKey = GlobalKey();
  final Set<int> _activeFlutterPointers = <int>{};
  ScrollPosition? _scrollPosition;
  Animation<double>? _secondaryRouteAnimation;
  bool _syncScheduled = false;

  @override
  void initState() {
    super.initState();
    _ensureLinuxPersistentCallback();
    _mountedLinuxHosts.add(this);
    WidgetsBinding.instance.addObserver(this);
    FocusManager.instance.addListener(_scheduleGeometrySync);
    GestureBinding.instance.pointerRouter.addGlobalRoute(
      _handleGlobalPointerEvent,
    );
    unawaited(widget.bridge.createLinuxView(widget.params));
  }

  void _handleGlobalPointerEvent(PointerEvent event) {
    final wasGrabbing = _activeFlutterPointers.isNotEmpty;
    if (event is PointerDownEvent) {
      _activeFlutterPointers.add(event.pointer);
    } else if (event is PointerUpEvent || event is PointerCancelEvent) {
      _activeFlutterPointers.remove(event.pointer);
    } else {
      return;
    }
    final isGrabbing = _activeFlutterPointers.isNotEmpty;
    if (wasGrabbing == isGrabbing) {
      return;
    }
    final renderObject = _surfaceKey.currentContext?.findRenderObject();
    if (renderObject is _RenderLinuxWebViewSurface) {
      renderObject.flutterPointerGrabActive = isGrabbing;
    }
  }

  void _syncAfterFrame() {
    if (!mounted) {
      return;
    }
    final renderObject = _surfaceKey.currentContext?.findRenderObject();
    if (renderObject is _RenderLinuxWebViewSurface) {
      renderObject.syncGeometryNow();
    }
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final nextPosition = Scrollable.maybeOf(context)?.position;
    if (!identical(_scrollPosition, nextPosition)) {
      _scrollPosition?.removeListener(_scheduleGeometrySync);
      _scrollPosition = nextPosition;
      _scrollPosition?.addListener(_scheduleGeometrySync);
    }
    final nextSecondary = ModalRoute.of(context)?.secondaryAnimation;
    if (!identical(_secondaryRouteAnimation, nextSecondary)) {
      _secondaryRouteAnimation?.removeListener(_scheduleGeometrySync);
      _secondaryRouteAnimation = nextSecondary;
      _secondaryRouteAnimation?.addListener(_scheduleGeometrySync);
    }
    _scheduleGeometrySync();
  }

  @override
  void didUpdateWidget(covariant _LinuxWebViewHost oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.params.controllerId != widget.params.controllerId) {
      unawaited(
        widget.bridge.setLinuxGeometry(
          controllerId: oldWidget.params.controllerId,
          visible: false,
          x: 0,
          y: 0,
          width: 0,
          height: 0,
        ),
      );
      unawaited(widget.bridge.createLinuxView(widget.params));
    }
    _scheduleGeometrySync();
  }

  @override
  void didChangeMetrics() {
    _scheduleGeometrySync();
  }

  void _scheduleGeometrySync() {
    if (_syncScheduled || !mounted) {
      return;
    }
    _syncScheduled = true;
    SchedulerBinding.instance.addPostFrameCallback((_) {
      _syncScheduled = false;
      _syncAfterFrame();
    });
  }

  @override
  void deactivate() {
    _activeFlutterPointers.clear();
    final renderObject = _surfaceKey.currentContext?.findRenderObject();
    if (renderObject is _RenderLinuxWebViewSurface) {
      renderObject.hideImmediately();
    } else {
      unawaited(
        widget.bridge.setLinuxGeometry(
          controllerId: widget.params.controllerId,
          visible: false,
          x: 0,
          y: 0,
          width: 0,
          height: 0,
        ),
      );
    }
    super.deactivate();
  }

  @override
  void dispose() {
    _mountedLinuxHosts.remove(this);
    _activeFlutterPointers.clear();
    GestureBinding.instance.pointerRouter.removeGlobalRoute(
      _handleGlobalPointerEvent,
    );
    _scrollPosition?.removeListener(_scheduleGeometrySync);
    _secondaryRouteAnimation?.removeListener(_scheduleGeometrySync);
    FocusManager.instance.removeListener(_scheduleGeometrySync);
    WidgetsBinding.instance.removeObserver(this);
    unawaited(
      widget.bridge.setLinuxGeometry(
        controllerId: widget.params.controllerId,
        visible: false,
        x: 0,
        y: 0,
        width: 0,
        height: 0,
      ),
    );
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final isVisible = Visibility.of(context);
    final tickerEnabled = TickerMode.valuesOf(context).enabled;
    final isCurrentRoute = ModalRoute.isCurrentOf(context) ?? true;
    final subtreeVisible = isVisible && tickerEnabled && isCurrentRoute;
    final viewId = View.maybeOf(context)?.viewId;

    return _LinuxWebViewSurface(
      key: _surfaceKey,
      bridge: widget.bridge,
      controllerId: widget.params.controllerId,
      subtreeVisible: subtreeVisible,
      flutterPointerGrabActive: _activeFlutterPointers.isNotEmpty,
      viewId: viewId,
      onPaintNeedsSync: _scheduleGeometrySync,
      child: ColoredBox(
        color: widget.params.backgroundColor,
        child: const SizedBox.expand(),
      ),
    );
  }
}

class _LinuxWebViewSurface extends SingleChildRenderObjectWidget {
  const _LinuxWebViewSurface({
    required this.bridge,
    required this.controllerId,
    required this.subtreeVisible,
    required this.flutterPointerGrabActive,
    required this.viewId,
    required this.onPaintNeedsSync,
    super.key,
    super.child,
  });

  final IoWebViewBridge bridge;
  final int controllerId;
  final bool subtreeVisible;
  final bool flutterPointerGrabActive;
  final int? viewId;
  final VoidCallback onPaintNeedsSync;

  @override
  RenderObject createRenderObject(BuildContext context) {
    return _RenderLinuxWebViewSurface(
      bridge: bridge,
      controllerId: controllerId,
      subtreeVisible: subtreeVisible,
      flutterPointerGrabActive: flutterPointerGrabActive,
      viewId: viewId,
      onPaintNeedsSync: onPaintNeedsSync,
    );
  }

  @override
  void updateRenderObject(
    BuildContext context,
    covariant _RenderLinuxWebViewSurface renderObject,
  ) {
    renderObject
      ..bridge = bridge
      ..controllerId = controllerId
      ..viewId = viewId
      ..onPaintNeedsSync = onPaintNeedsSync
      ..flutterPointerGrabActive = flutterPointerGrabActive
      ..subtreeVisible = subtreeVisible;
  }
}

@immutable
class _LinuxGeometrySnapshot {
  const _LinuxGeometrySnapshot({
    required this.visible,
    required this.passThrough,
    required this.x,
    required this.y,
    required this.width,
    required this.height,
    required this.fullX,
    required this.fullY,
    required this.fullWidth,
    required this.fullHeight,
  });

  static const _LinuxGeometrySnapshot hidden = _LinuxGeometrySnapshot(
    visible: false,
    passThrough: true,
    x: 0,
    y: 0,
    width: 0,
    height: 0,
    fullX: 0,
    fullY: 0,
    fullWidth: 0,
    fullHeight: 0,
  );

  final bool visible;
  final bool passThrough;
  final int x;
  final int y;
  final int width;
  final int height;
  final int fullX;
  final int fullY;
  final int fullWidth;
  final int fullHeight;

  @override
  bool operator ==(Object other) {
    return other is _LinuxGeometrySnapshot &&
        other.visible == visible &&
        other.passThrough == passThrough &&
        other.x == x &&
        other.y == y &&
        other.width == width &&
        other.height == height &&
        other.fullX == fullX &&
        other.fullY == fullY &&
        other.fullWidth == fullWidth &&
        other.fullHeight == fullHeight;
  }

  @override
  int get hashCode => Object.hash(
    visible,
    passThrough,
    x,
    y,
    width,
    height,
    fullX,
    fullY,
    fullWidth,
    fullHeight,
  );
}

class _RenderLinuxWebViewSurface extends RenderProxyBox {
  _RenderLinuxWebViewSurface({
    required IoWebViewBridge bridge,
    required int controllerId,
    required bool subtreeVisible,
    required bool flutterPointerGrabActive,
    required int? viewId,
    required VoidCallback onPaintNeedsSync,
  }) : _bridge = bridge,
       _controllerId = controllerId,
       _subtreeVisible = subtreeVisible,
       _flutterPointerGrabActive = flutterPointerGrabActive,
       _viewId = viewId,
       _onPaintNeedsSync = onPaintNeedsSync;

  IoWebViewBridge _bridge;
  int _controllerId;
  bool _subtreeVisible;
  bool _flutterPointerGrabActive;
  int? _viewId;
  VoidCallback _onPaintNeedsSync;
  _LinuxGeometrySnapshot? _lastSent;

  set bridge(IoWebViewBridge value) {
    _bridge = value;
  }

  set viewId(int? value) {
    _viewId = value;
  }

  set onPaintNeedsSync(VoidCallback value) {
    _onPaintNeedsSync = value;
  }

  set controllerId(int value) {
    if (_controllerId == value) {
      return;
    }
    _controllerId = value;
    _lastSent = null;
    markNeedsPaint();
  }

  set flutterPointerGrabActive(bool value) {
    if (_flutterPointerGrabActive == value) {
      return;
    }
    _flutterPointerGrabActive = value;
    syncGeometryNow();
  }

  set subtreeVisible(bool value) {
    if (_subtreeVisible == value) {
      return;
    }
    _subtreeVisible = value;
    if (!value) {
      hideImmediately();
    } else {
      markNeedsPaint();
    }
  }

  void hideImmediately() {
    _emitGeometry(_LinuxGeometrySnapshot.hidden);
  }

  void syncGeometryNow() {
    if (!attached || !hasSize || !_subtreeVisible) {
      hideImmediately();
      return;
    }
    final snapshot = _computeVisibleGeometry(checkOcclusion: true);
    _emitGeometry(snapshot);
  }

  /// Returns `true` when any interior sample point of [visibleRect] is covered
  /// by a hit-absorbing Flutter widget above this surface (e.g. modal barrier,
  /// dropdown menu, popup menu, floating dialog, or in-stack overlay).
  bool _isOccludedInView(Rect visibleRect) {
    final targetViewId = _viewId;
    if (targetViewId == null || debugNeedsLayout) {
      return false;
    }
    final List<Offset> samplePoints;
    if (visibleRect.width < 24.0 || visibleRect.height < 24.0) {
      samplePoints = <Offset>[visibleRect.center];
    } else {
      final l = visibleRect.left;
      final t = visibleRect.top;
      final w = visibleRect.width;
      final h = visibleRect.height;
      samplePoints = <Offset>[
        visibleRect.center,
        Offset(l + w * 0.2, t + h * 0.2),
        Offset(l + w * 0.5, t + h * 0.2),
        Offset(l + w * 0.8, t + h * 0.2),
        Offset(l + w * 0.2, t + h * 0.5),
        Offset(l + w * 0.8, t + h * 0.5),
        Offset(l + w * 0.2, t + h * 0.8),
        Offset(l + w * 0.5, t + h * 0.8),
        Offset(l + w * 0.8, t + h * 0.8),
        Offset(l + w * 0.35, t + h * 0.1),
        Offset(l + w * 0.5, t + h * 0.1),
        Offset(l + w * 0.65, t + h * 0.1),
        Offset(l + w * 0.35, t + h * 0.9),
        Offset(l + w * 0.5, t + h * 0.9),
        Offset(l + w * 0.65, t + h * 0.9),
      ];
    }
    for (final point in samplePoints) {
      final hitResult = BoxHitTestResult();
      RendererBinding.instance.hitTestInView(hitResult, point, targetViewId);
      var hitSelf = false;
      for (final entry in hitResult.path) {
        if (identical(entry.target, this)) {
          hitSelf = true;
          break;
        }
      }
      if (!hitSelf) {
        return true;
      }
    }
    return false;
  }

  _LinuxGeometrySnapshot _computeVisibleGeometry({
    required bool checkOcclusion,
  }) {
    if (!attached ||
        !hasSize ||
        !_subtreeVisible ||
        size.isEmpty ||
        !_isRenderSubtreeActive(this)) {
      return _LinuxGeometrySnapshot.hidden;
    }

    final topLeft = localToGlobal(Offset.zero);
    final bottomRight = localToGlobal(size.bottomRight(Offset.zero));
    final fullRect = Rect.fromPoints(topLeft, bottomRight);
    var visibleRect = fullRect;

    var node = parent;
    while (node != null) {
      if (node is RenderBox && node.hasSize && _clipsDescendants(node)) {
        final clipTopLeft = node.localToGlobal(Offset.zero);
        final clipBottomRight = node.localToGlobal(
          node.size.bottomRight(Offset.zero),
        );
        visibleRect = visibleRect.intersect(
          Rect.fromPoints(clipTopLeft, clipBottomRight),
        );
        if (visibleRect.isEmpty ||
            visibleRect.width <= 0.5 ||
            visibleRect.height <= 0.5) {
          return _LinuxGeometrySnapshot.hidden;
        }
      }
      node = node.parent;
    }

    final left = visibleRect.left.round();
    final top = visibleRect.top.round();
    final width = visibleRect.width.round();
    final height = visibleRect.height.round();
    final fullLeft = fullRect.left.round();
    final fullTop = fullRect.top.round();
    final fullWidth = fullRect.width.round();
    final fullHeight = fullRect.height.round();
    if (width <= 0 || height <= 0 || fullWidth <= 0 || fullHeight <= 0) {
      return _LinuxGeometrySnapshot.hidden;
    }

    if (checkOcclusion && _isOccludedInView(visibleRect)) {
      return _LinuxGeometrySnapshot.hidden;
    }

    return _LinuxGeometrySnapshot(
      visible: true,
      passThrough: _flutterPointerGrabActive,
      x: left,
      y: top,
      width: width,
      height: height,
      fullX: fullLeft,
      fullY: fullTop,
      fullWidth: fullWidth,
      fullHeight: fullHeight,
    );
  }

  static bool _clipsDescendants(RenderBox box) {
    if (box is RenderClipRect) {
      return box.clipBehavior != Clip.none;
    }
    if (box is RenderClipRRect) {
      return box.clipBehavior != Clip.none;
    }
    if (box is RenderClipOval) {
      return box.clipBehavior != Clip.none;
    }
    if (box is RenderClipPath) {
      return box.clipBehavior != Clip.none;
    }
    if (box is RenderAbstractViewport) {
      return true;
    }
    return false;
  }

  void _emitGeometry(_LinuxGeometrySnapshot next) {
    if (_lastSent == next) {
      return;
    }
    _lastSent = next;
    unawaited(
      _bridge.setLinuxGeometry(
        controllerId: _controllerId,
        visible: next.visible,
        passThrough: next.passThrough,
        x: next.x,
        y: next.y,
        width: next.width,
        height: next.height,
        fullX: next.fullX,
        fullY: next.fullY,
        fullWidth: next.fullWidth,
        fullHeight: next.fullHeight,
      ),
    );
  }

  @override
  void detach() {
    hideImmediately();
    super.detach();
  }

  @override
  void paint(PaintingContext context, Offset offset) {
    super.paint(context, offset);
    final snapshot = _computeVisibleGeometry(checkOcclusion: false);
    if (!snapshot.visible) {
      _emitGeometry(snapshot);
      return;
    }
    _onPaintNeedsSync();
  }
}

/// Windows `WebView2` `Texture` host widget forwarding HiDPI bounds,
/// visibility, focus, keyboard shortcuts, mouse pointer, and scroll wheel
/// events.
class _WindowsWebViewHost extends StatefulWidget {
  const _WindowsWebViewHost({
    required this.bridge,
    required this.params,
    super.key,
  });

  final IoWebViewBridge bridge;
  final WebViewCreationParams params;

  @override
  State<_WindowsWebViewHost> createState() => _WindowsWebViewHostState();
}

class _WindowsWebViewHostState extends State<_WindowsWebViewHost> {
  final FocusNode _focusNode = FocusNode(debugLabel: 'WebViewCoreWindows');
  final GlobalKey _probeKey = GlobalKey();
  int? _textureId;
  int _lastWidth = -1;
  int _lastHeight = -1;
  double _lastScale = -1.0;
  bool? _lastVisible;
  bool _postFrameCheckScheduled = false;

  @override
  void initState() {
    super.initState();
    _focusNode.addListener(_onFocusChanged);
    unawaited(_initView());
  }

  Future<void> _initView() async {
    final id = await widget.bridge.createWindowsView(widget.params);
    if (!mounted) {
      return;
    }
    setState(() {
      _textureId = id;
    });
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _scheduleSubtreeVisibilityCheck();
  }

  @override
  void didUpdateWidget(covariant _WindowsWebViewHost oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.params.controllerId != widget.params.controllerId) {
      _textureId = null;
      _lastWidth = -1;
      _lastHeight = -1;
      _lastScale = -1.0;
      _lastVisible = null;
      unawaited(_initView());
    }
    _scheduleSubtreeVisibilityCheck();
  }

  void _scheduleSubtreeVisibilityCheck() {
    if (_postFrameCheckScheduled || !mounted) {
      return;
    }
    _postFrameCheckScheduled = true;
    SchedulerBinding.instance.addPostFrameCallback((_) {
      _postFrameCheckScheduled = false;
      if (!mounted) {
        return;
      }
      final inheritedVisible =
          Visibility.of(context) && TickerMode.valuesOf(context).enabled;
      if (!inheritedVisible) {
        _syncVisibility(false);
        return;
      }
      final ro = _probeKey.currentContext?.findRenderObject();
      final renderActive = ro == null || _isRenderSubtreeActive(ro);
      _syncVisibility(renderActive);
    });
  }

  void _onFocusChanged() {
    unawaited(
      widget.bridge.setWindowsFocus(
        controllerId: widget.params.controllerId,
        focused: _focusNode.hasFocus,
      ),
    );
  }

  KeyEventResult _handleKeyEvent(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent && event is! KeyRepeatEvent) {
      return KeyEventResult.ignored;
    }
    final key = event.logicalKey;
    final kb = HardwareKeyboard.instance;
    if (_isPassThroughKey(key) || kb.isMetaPressed) {
      return KeyEventResult.ignored;
    }
    unawaited(
      widget.bridge.sendWindowsKeyEvent(
        controllerId: widget.params.controllerId,
        logicalKey: key.keyLabel,
        keyId: key.keyId,
        character: event.character,
        shift: kb.isShiftPressed,
        control: kb.isControlPressed,
        alt: kb.isAltPressed,
        meta: kb.isMetaPressed,
      ),
    );
    return KeyEventResult.handled;
  }

  void _syncVisibility(bool visible) {
    if (_lastVisible == visible) {
      return;
    }
    _lastVisible = visible;
    unawaited(
      widget.bridge.setWindowsVisibility(
        controllerId: widget.params.controllerId,
        visible: visible,
      ),
    );
  }

  void _syncSize(Size size, double scaleFactor) {
    if (size.isEmpty) {
      return;
    }
    final width = (size.width * scaleFactor).round();
    final height = (size.height * scaleFactor).round();
    if (width <= 0 || height <= 0) {
      return;
    }
    if (width == _lastWidth &&
        height == _lastHeight &&
        scaleFactor == _lastScale) {
      return;
    }
    _lastWidth = width;
    _lastHeight = height;
    _lastScale = scaleFactor;
    unawaited(
      widget.bridge.setWindowsSize(
        controllerId: widget.params.controllerId,
        width: width,
        height: height,
        scaleFactor: scaleFactor,
      ),
    );
  }

  static int _resolveChangedButton(int buttonsMask, int prevButtonsMask) {
    final diff = buttonsMask ^ prevButtonsMask;
    if ((diff & kPrimaryMouseButton) != 0) {
      return 1;
    }
    if ((diff & kSecondaryMouseButton) != 0) {
      return 2;
    }
    if ((diff & kMiddleMouseButton) != 0) {
      return 3;
    }
    return 1;
  }

  int _activeButtons = 0;

  @override
  void deactivate() {
    _syncVisibility(false);
    super.deactivate();
  }

  @override
  void dispose() {
    _focusNode
      ..removeListener(_onFocusChanged)
      ..dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final isVisible = Visibility.of(context);
    final tickerEnabled = TickerMode.valuesOf(context).enabled;
    final inheritedVisible = isVisible && tickerEnabled;
    if (!inheritedVisible) {
      _syncVisibility(false);
    } else {
      _scheduleSubtreeVisibilityCheck();
    }
    final dpr = MediaQuery.maybeDevicePixelRatioOf(context) ?? 1.0;

    return LayoutBuilder(
      key: _probeKey,
      builder: (BuildContext context, BoxConstraints constraints) {
        final width = constraints.hasBoundedWidth ? constraints.maxWidth : 0.0;
        final height = constraints.hasBoundedHeight
            ? constraints.maxHeight
            : 0.0;
        _syncSize(Size(width, height), dpr);

        final textureId = _textureId;
        return Focus(
          focusNode: _focusNode,
          onKeyEvent: _handleKeyEvent,
          child: MouseRegion(
            onHover: (PointerHoverEvent event) {
              final kb = HardwareKeyboard.instance;
              unawaited(
                widget.bridge.sendWindowsPointerEvent(
                  controllerId: widget.params.controllerId,
                  kind: 'move',
                  button: 0,
                  buttons: event.buttons,
                  x: event.localPosition.dx * dpr,
                  y: event.localPosition.dy * dpr,
                  shift: kb.isShiftPressed,
                  control: kb.isControlPressed,
                  alt: kb.isAltPressed,
                ),
              );
            },
            onExit: (PointerExitEvent event) {
              unawaited(
                widget.bridge.sendWindowsPointerEvent(
                  controllerId: widget.params.controllerId,
                  kind: 'leave',
                  button: 0,
                  buttons: 0,
                  x: -1.0,
                  y: -1.0,
                ),
              );
            },
            child: Listener(
              behavior: HitTestBehavior.opaque,
              onPointerDown: (PointerDownEvent event) {
                if (!_focusNode.hasFocus) {
                  _focusNode.requestFocus();
                }
                final button = _resolveChangedButton(
                  event.buttons,
                  _activeButtons,
                );
                _activeButtons = event.buttons;
                final kb = HardwareKeyboard.instance;
                unawaited(
                  widget.bridge.sendWindowsPointerEvent(
                    controllerId: widget.params.controllerId,
                    kind: 'down',
                    button: button,
                    buttons: event.buttons,
                    x: event.localPosition.dx * dpr,
                    y: event.localPosition.dy * dpr,
                    shift: kb.isShiftPressed,
                    control: kb.isControlPressed,
                    alt: kb.isAltPressed,
                  ),
                );
              },
              onPointerMove: (PointerMoveEvent event) {
                _activeButtons = event.buttons;
                final kb = HardwareKeyboard.instance;
                unawaited(
                  widget.bridge.sendWindowsPointerEvent(
                    controllerId: widget.params.controllerId,
                    kind: 'move',
                    button: 0,
                    buttons: event.buttons,
                    x: event.localPosition.dx * dpr,
                    y: event.localPosition.dy * dpr,
                    shift: kb.isShiftPressed,
                    control: kb.isControlPressed,
                    alt: kb.isAltPressed,
                  ),
                );
              },
              onPointerUp: (PointerUpEvent event) {
                final button = _resolveChangedButton(
                  event.buttons,
                  _activeButtons,
                );
                _activeButtons = event.buttons;
                final kb = HardwareKeyboard.instance;
                unawaited(
                  widget.bridge.sendWindowsPointerEvent(
                    controllerId: widget.params.controllerId,
                    kind: 'up',
                    button: button,
                    buttons: event.buttons,
                    x: event.localPosition.dx * dpr,
                    y: event.localPosition.dy * dpr,
                    shift: kb.isShiftPressed,
                    control: kb.isControlPressed,
                    alt: kb.isAltPressed,
                  ),
                );
              },
              onPointerCancel: (PointerCancelEvent event) {
                final prev = _activeButtons;
                _activeButtons = 0;
                final kb = HardwareKeyboard.instance;
                final x = event.localPosition.dx * dpr;
                final y = event.localPosition.dy * dpr;
                if ((prev & kPrimaryMouseButton) != 0) {
                  unawaited(
                    widget.bridge.sendWindowsPointerEvent(
                      controllerId: widget.params.controllerId,
                      kind: 'up',
                      button: 1,
                      buttons: 0,
                      x: x,
                      y: y,
                      shift: kb.isShiftPressed,
                      control: kb.isControlPressed,
                      alt: kb.isAltPressed,
                    ),
                  );
                }
                if ((prev & kSecondaryMouseButton) != 0) {
                  unawaited(
                    widget.bridge.sendWindowsPointerEvent(
                      controllerId: widget.params.controllerId,
                      kind: 'up',
                      button: 2,
                      buttons: 0,
                      x: x,
                      y: y,
                      shift: kb.isShiftPressed,
                      control: kb.isControlPressed,
                      alt: kb.isAltPressed,
                    ),
                  );
                }
                if ((prev & kMiddleMouseButton) != 0) {
                  unawaited(
                    widget.bridge.sendWindowsPointerEvent(
                      controllerId: widget.params.controllerId,
                      kind: 'up',
                      button: 3,
                      buttons: 0,
                      x: x,
                      y: y,
                      shift: kb.isShiftPressed,
                      control: kb.isControlPressed,
                      alt: kb.isAltPressed,
                    ),
                  );
                }
                unawaited(
                  widget.bridge.sendWindowsPointerEvent(
                    controllerId: widget.params.controllerId,
                    kind: 'leave',
                    button: 0,
                    buttons: 0,
                    x: -1.0,
                    y: -1.0,
                  ),
                );
              },
              onPointerSignal: (PointerSignalEvent event) {
                if (event is PointerScrollEvent) {
                  GestureBinding.instance.pointerSignalResolver.register(
                    event,
                    (PointerSignalEvent resolved) {
                      if (resolved is! PointerScrollEvent || !mounted) {
                        return;
                      }
                      final kb = HardwareKeyboard.instance;
                      unawaited(
                        widget.bridge.sendWindowsPointerEvent(
                          controllerId: widget.params.controllerId,
                          kind: 'wheel',
                          button: 0,
                          buttons: _activeButtons,
                          x: resolved.localPosition.dx * dpr,
                          y: resolved.localPosition.dy * dpr,
                          deltaX: -resolved.scrollDelta.dx,
                          deltaY: -resolved.scrollDelta.dy,
                          shift: kb.isShiftPressed,
                          control: kb.isControlPressed,
                          alt: kb.isAltPressed,
                        ),
                      );
                    },
                  );
                }
              },
              child: ColoredBox(
                color: widget.params.backgroundColor,
                child: textureId != null && textureId >= 0
                    ? Texture(textureId: textureId)
                    : const SizedBox.expand(),
              ),
            ),
          ),
        );
      },
    );
  }
}
