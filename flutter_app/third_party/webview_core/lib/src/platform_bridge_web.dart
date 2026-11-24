import 'dart:async';
import 'dart:js_interop';
import 'dart:js_interop_unsafe';
import 'dart:ui_web' as ui_web;

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'html_decorator.dart';
import 'platform_bridge.dart';
import 'types.dart';

@JS('window')
external _JSWindow get _jsWindow;

@JS('document')
external _JSDocument get _jsDocument;

@JS('Object.is')
external bool _jsObjectIs(JSAny? a, JSAny? b);

extension type const _JSDocument(JSObject _raw) implements JSObject {
  external JSObject createElement(String tagName);
  external String get title;
  external set cookie(String value);
}

extension type const _JSStyleDeclaration(JSObject _raw) implements JSObject {
  external set border(String value);
  external set width(String value);
  external set height(String value);
  external set margin(String value);
  external set padding(String value);
  external set backgroundColor(String value);
}

extension type const _JSWindow(JSObject _raw) implements JSObject {
  external void addEventListener(String type, JSFunction listener);
  external void removeEventListener(String type, JSFunction listener);
  external void scrollTo(int x, int y);
  external void scrollBy(int x, int y);
  external double get scrollX;
  external double get scrollY;
  external void stop();
  @JS('eval')
  external JSAny? evalScript(String script);
}

extension type const _JSIFrameElement(JSObject _raw) implements JSObject {
  external _JSStyleDeclaration get style;
  external set src(String value);
  external String get src;
  external set srcdoc(String value);
  external String get srcdoc;
  external void setAttribute(String name, String value);
  external void removeAttribute(String name);
  external void addEventListener(String type, JSFunction listener);
  external void removeEventListener(String type, JSFunction listener);
  external void remove();
  external _JSWindow? get contentWindow;
  external _JSDocument? get contentDocument;
}

extension type const _JSMessageEvent(JSObject _raw) implements JSObject {
  external JSAny? get data;
  external JSAny? get source;
}

/// Web and WebAssembly (`dart2wasm`) bridge instance using pure
/// `dart:js_interop` and `dart:ui_web` with zero external packages.
final WebViewPlatformBridge platformBridgeInstance = WebWebViewBridge();

final class _WebViewEntry {
  _WebViewEntry({
    required this.controllerId,
    required this.viewType,
    required this.iframe,
    required this.loadListener,
  });

  final int controllerId;
  final String viewType;
  final _JSIFrameElement iframe;
  final JSFunction loadListener;
  bool javaScriptEnabled = true;
  Color backgroundColor = const Color(0x00000000);
  final Set<String> channelNames = <String>{};
  String? currentUrl;
  String? lastHtml;
  String? lastBaseUrl;
  final List<String> history = <String>[];
  int historyIndex = -1;
}

/// Pure `dart:js_interop` Web implementation of [WebViewPlatformBridge].
final class WebWebViewBridge extends WebViewPlatformBridge {
  final Map<int, WebViewPlatformCallbacks> _callbacks =
      <int, WebViewPlatformCallbacks>{};
  final Map<int, _WebViewEntry> _entries = <int, _WebViewEntry>{};
  JSFunction? _windowMessageListener;

  /// Initializes global `postMessage` reception for JavaScript channels.
  void ensureInitialized() {
    if (_windowMessageListener != null) {
      return;
    }
    final listener = ((JSObject rawEvent) {
      final event = _JSMessageEvent(rawEvent);
      final data = event.data;
      if (data == null || !data.isA<JSObject>()) {
        return;
      }
      final payload = data as JSObject;
      final marker = payload.getProperty<JSAny?>('__webview_core_id'.toJS);
      final chProp = payload.getProperty<JSAny?>('__webview_core_channel'.toJS);
      final msgProp = payload.getProperty<JSAny?>('message'.toJS);
      if (marker == null ||
          chProp == null ||
          !marker.isA<JSNumber>() ||
          !chProp.isA<JSString>()) {
        return;
      }
      final controllerId = (marker as JSNumber).toDartInt;
      final channelName = (chProp as JSString).toDart;
      final entry = _entries[controllerId];
      if (entry == null || !entry.channelNames.contains(channelName)) {
        return;
      }
      final expectedWindow = entry.iframe.contentWindow as JSAny?;
      if (expectedWindow == null ||
          !_jsObjectIs(event.source, expectedWindow)) {
        return;
      }
      final message = msgProp != null && msgProp.isA<JSString>()
          ? (msgProp as JSString).toDart
          : '';
      _callbacks[controllerId]?.handleJavaScriptMessage(
        controllerId,
        channelName,
        message,
      );
    }).toJS;
    _jsWindow.addEventListener('message', listener);
    _windowMessageListener = listener;
  }

  static String _colorToCssRgba(Color color) {
    final argb = color.toARGB32();
    final a = ((argb >> 24) & 0xFF) / 255.0;
    final r = (argb >> 16) & 0xFF;
    final g = (argb >> 8) & 0xFF;
    final b = argb & 0xFF;
    return 'rgba($r, $g, $b, ${a.toStringAsFixed(4)})';
  }

  static String _decorateHtml({
    required int controllerId,
    required String html,
    required Set<String> channels,
    String? baseUrl,
  }) {
    return decorateHtmlDocument(
      controllerId: controllerId,
      html: html,
      channels: channels,
      baseUrl: baseUrl,
    );
  }

  static void _applySandbox(_JSIFrameElement iframe, bool jsEnabled) {
    if (jsEnabled) {
      iframe.removeAttribute('sandbox');
    } else {
      iframe.setAttribute(
        'sandbox',
        'allow-forms allow-popups allow-same-origin',
      );
    }
  }

  _WebViewEntry _ensureEntry(WebViewCreationParams params) {
    ensureInitialized();
    final existing = _entries[params.controllerId];
    if (existing != null) {
      return existing;
    }

    final viewType = 'com.infradise.webview_core/iframe_${params.controllerId}';
    final iframe = _JSIFrameElement(_jsDocument.createElement('iframe'));
    iframe.style
      ..border = '0'
      ..margin = '0'
      ..padding = '0'
      ..width = '100%'
      ..height = '100%'
      ..backgroundColor = _colorToCssRgba(params.backgroundColor);

    _applySandbox(iframe, params.javaScriptEnabled);

    late final _WebViewEntry entry;
    final onLoad = ((JSObject _) {
      final url = entry.currentUrl ?? 'about:blank';
      _callbacks[params.controllerId]?.handleProgress(params.controllerId, 100);
      _callbacks[params.controllerId]?.handlePageFinished(
        params.controllerId,
        url,
      );
    }).toJS;
    iframe.addEventListener('load', onLoad);

    entry =
        _WebViewEntry(
            controllerId: params.controllerId,
            viewType: viewType,
            iframe: iframe,
            loadListener: onLoad,
          )
          ..javaScriptEnabled = params.javaScriptEnabled
          ..backgroundColor = params.backgroundColor
          ..channelNames.addAll(params.channelNames);

    ui_web.platformViewRegistry.registerViewFactory(
      viewType,
      (int _) => iframe,
    );

    _entries[params.controllerId] = entry;

    if (params.initialHtml != null) {
      entry
        ..lastHtml = params.initialHtml
        ..lastBaseUrl = params.initialBaseUrl
        ..currentUrl = params.initialBaseUrl ?? 'about:srcdoc';
      iframe.srcdoc = _decorateHtml(
        controllerId: params.controllerId,
        html: params.initialHtml!,
        channels: entry.channelNames,
        baseUrl: params.initialBaseUrl,
      );
    } else if (params.initialUrl != null) {
      entry
        ..currentUrl = params.initialUrl
        ..history.add(params.initialUrl!)
        ..historyIndex = 0;
      iframe.src = params.initialUrl!;
    }

    scheduleMicrotask(() {
      _callbacks[params.controllerId]?.handleViewCreated(params.controllerId);
    });
    return entry;
  }

  @override
  void registerController(
    int controllerId,
    WebViewPlatformCallbacks callbacks,
  ) {
    _callbacks[controllerId] = callbacks;
  }

  @override
  void unregisterController(int controllerId) {
    _callbacks.remove(controllerId);
  }

  @override
  Widget buildView({
    required BuildContext context,
    required WebViewCreationParams params,
    Key? key,
  }) {
    final entry = _ensureEntry(params);
    return ColoredBox(
      key: key,
      color: params.backgroundColor,
      child: HtmlElementView(viewType: entry.viewType),
    );
  }

  @override
  Future<void> loadHtmlString(
    int controllerId,
    String html, {
    String? baseUrl,
  }) async {
    final entry = _entries[controllerId];
    if (entry == null) {
      return;
    }
    final url = baseUrl ?? 'about:srcdoc';
    entry
      ..lastHtml = html
      ..lastBaseUrl = baseUrl
      ..currentUrl = url;
    _callbacks[controllerId]?.handlePageStarted(controllerId, url);
    _callbacks[controllerId]?.handleUrlChange(controllerId, url);
    entry.iframe.removeAttribute('src');
    entry.iframe.srcdoc = _decorateHtml(
      controllerId: controllerId,
      html: html,
      channels: entry.channelNames,
      baseUrl: baseUrl,
    );
  }

  @override
  Future<void> loadRequest(
    int controllerId, {
    required String url,
    required String method,
    required Map<String, String> headers,
    Uint8List? body,
  }) async {
    final entry = _entries[controllerId];
    if (entry == null) {
      return;
    }
    final callbacks = _callbacks[controllerId];
    if (callbacks != null) {
      final allowed = await callbacks.handleNavigationRequest(
        controllerId,
        NavigationRequest(url: url, isMainFrame: true),
      );
      if (!allowed) {
        return;
      }
    }
    entry
      ..lastHtml = null
      ..lastBaseUrl = null
      ..currentUrl = url;
    if (entry.historyIndex < entry.history.length - 1) {
      entry.history.removeRange(entry.historyIndex + 1, entry.history.length);
    }
    entry.history.add(url);
    entry.historyIndex = entry.history.length - 1;
    callbacks?.handlePageStarted(controllerId, url);
    callbacks?.handleUrlChange(controllerId, url);
    entry.iframe.removeAttribute('srcdoc');
    entry.iframe.src = url;
  }

  @override
  Future<void> loadFile(int controllerId, String path) async {
    await loadRequest(
      controllerId,
      url: path,
      method: 'GET',
      headers: const <String, String>{},
    );
  }

  @override
  Future<void> loadFlutterAsset(int controllerId, String assetKey) async {
    final html = await rootBundle.loadString(assetKey);
    await loadHtmlString(controllerId, html);
  }

  @override
  Future<void> setJavaScriptEnabled(int controllerId, bool enabled) async {
    final entry = _entries[controllerId];
    if (entry == null) {
      return;
    }
    entry.javaScriptEnabled = enabled;
    _applySandbox(entry.iframe, enabled);
    if (entry.lastHtml != null) {
      entry.iframe.srcdoc = _decorateHtml(
        controllerId: controllerId,
        html: entry.lastHtml!,
        channels: entry.channelNames,
        baseUrl: entry.lastBaseUrl,
      );
    }
  }

  @override
  Future<void> setBackgroundColor(int controllerId, Color color) async {
    final entry = _entries[controllerId];
    if (entry == null) {
      return;
    }
    entry.backgroundColor = color;
    entry.iframe.style.backgroundColor = _colorToCssRgba(color);
  }

  @override
  Future<void> setUserAgent(int controllerId, String? userAgent) async {}

  @override
  Future<void> enableZoom(int controllerId, bool enabled) async {}

  @override
  Future<void> setHasNavigationDecision(int controllerId, bool enabled) async {}

  @override
  Future<void> runJavaScript(int controllerId, String script) async {
    await runJavaScriptReturningResult(controllerId, script);
  }

  @override
  Future<Object?> runJavaScriptReturningResult(
    int controllerId,
    String script,
  ) async {
    final entry = _entries[controllerId];
    if (entry == null || !entry.javaScriptEnabled) {
      return null;
    }
    try {
      final win = entry.iframe.contentWindow;
      if (win == null) {
        return null;
      }
      final raw = win.evalScript(script);
      return raw.dartify();
    } on Object {
      return null;
    }
  }

  @override
  Future<void> addJavaScriptChannel(int controllerId, String name) async {
    final entry = _entries[controllerId];
    if (entry == null) {
      return;
    }
    entry.channelNames.add(name);
    if (entry.lastHtml != null) {
      entry.iframe.srcdoc = _decorateHtml(
        controllerId: controllerId,
        html: entry.lastHtml!,
        channels: entry.channelNames,
        baseUrl: entry.lastBaseUrl,
      );
    }
  }

  @override
  Future<void> removeJavaScriptChannel(int controllerId, String name) async {
    final entry = _entries[controllerId];
    if (entry == null) {
      return;
    }
    entry.channelNames.remove(name);
  }

  @override
  Future<void> reload(int controllerId) async {
    final entry = _entries[controllerId];
    if (entry == null) {
      return;
    }
    if (entry.lastHtml != null) {
      await loadHtmlString(
        controllerId,
        entry.lastHtml!,
        baseUrl: entry.lastBaseUrl,
      );
    } else if (entry.currentUrl != null) {
      entry.iframe.src = entry.currentUrl!;
    }
  }

  @override
  Future<void> stopLoading(int controllerId) async {
    final entry = _entries[controllerId];
    entry?.iframe.contentWindow?.stop();
  }

  @override
  Future<bool> canGoBack(int controllerId) async {
    final entry = _entries[controllerId];
    return entry != null && entry.historyIndex > 0;
  }

  @override
  Future<bool> canGoForward(int controllerId) async {
    final entry = _entries[controllerId];
    return entry != null && entry.historyIndex < entry.history.length - 1;
  }

  @override
  Future<void> goBack(int controllerId) async {
    final entry = _entries[controllerId];
    if (entry == null || entry.historyIndex <= 0) {
      return;
    }
    entry.historyIndex -= 1;
    final target = entry.history[entry.historyIndex];
    entry.currentUrl = target;
    entry.iframe.src = target;
  }

  @override
  Future<void> goForward(int controllerId) async {
    final entry = _entries[controllerId];
    if (entry == null || entry.historyIndex >= entry.history.length - 1) {
      return;
    }
    entry.historyIndex += 1;
    final target = entry.history[entry.historyIndex];
    entry.currentUrl = target;
    entry.iframe.src = target;
  }

  @override
  Future<String?> currentUrl(int controllerId) async {
    return _entries[controllerId]?.currentUrl;
  }

  @override
  Future<String?> getTitle(int controllerId) async {
    final entry = _entries[controllerId];
    if (entry == null) {
      return null;
    }
    try {
      return entry.iframe.contentDocument?.title;
    } on Object {
      return null;
    }
  }

  @override
  Future<void> scrollTo(int controllerId, int x, int y) async {
    try {
      _entries[controllerId]?.iframe.contentWindow?.scrollTo(x, y);
    } on Object {
      // Cross-origin iframe scroll access may be restricted by browser policy.
    }
  }

  @override
  Future<void> scrollBy(int controllerId, int x, int y) async {
    try {
      _entries[controllerId]?.iframe.contentWindow?.scrollBy(x, y);
    } on Object {
      // Cross-origin iframe scroll access may be restricted by browser policy.
    }
  }

  @override
  Future<Offset> getScrollPosition(int controllerId) async {
    try {
      final win = _entries[controllerId]?.iframe.contentWindow;
      if (win == null) {
        return Offset.zero;
      }
      return Offset(win.scrollX, win.scrollY);
    } on Object {
      return Offset.zero;
    }
  }

  @override
  Future<void> clearCache(int controllerId) async {}

  @override
  Future<void> clearLocalStorage(int controllerId) async {
    await runJavaScript(controllerId, 'window.localStorage.clear();');
  }

  @override
  Future<bool> clearCookies() async {
    return false;
  }

  @override
  Future<void> setCookie(WebViewCookie cookie) async {
    try {
      final encoded =
          '${cookie.name}=${cookie.value}; domain=${cookie.domain}; path=${cookie.path}';
      _jsDocument.cookie = encoded;
    } on Object {
      // Cross-domain cookie assignment may be rejected by browser policy.
    }
  }

  @override
  Future<void> dispose(int controllerId) async {
    final entry = _entries.remove(controllerId);
    if (entry != null) {
      entry.iframe.removeEventListener('load', entry.loadListener);
      entry.iframe.removeAttribute('srcdoc');
      entry.iframe.src = 'about:blank';
      entry.iframe.remove();
    }
    if (_entries.isEmpty && _windowMessageListener != null) {
      _jsWindow.removeEventListener('message', _windowMessageListener!);
      _windowMessageListener = null;
    }
  }
}
