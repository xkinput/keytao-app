import 'dart:typed_data';

import 'package:flutter/widgets.dart';

import 'platform_bridge.dart';
import 'types.dart';

/// Fallback stub bridge instance used only when neither `dart:io` nor
/// `dart:js_interop` is available.
final WebViewPlatformBridge platformBridgeInstance = _StubWebViewBridge();

final class _StubWebViewBridge extends WebViewPlatformBridge {
  @override
  void registerController(
    int controllerId,
    WebViewPlatformCallbacks callbacks,
  ) {}

  @override
  void unregisterController(int controllerId) {}

  @override
  Widget buildView({
    required BuildContext context,
    required WebViewCreationParams params,
    Key? key,
  }) {
    return SizedBox.expand(key: key);
  }

  @override
  Future<void> loadHtmlString(
    int controllerId,
    String html, {
    String? baseUrl,
  }) async {}

  @override
  Future<void> loadRequest(
    int controllerId, {
    required String url,
    required String method,
    required Map<String, String> headers,
    Uint8List? body,
  }) async {}

  @override
  Future<void> loadFile(int controllerId, String path) async {}

  @override
  Future<void> loadFlutterAsset(int controllerId, String assetKey) async {}

  @override
  Future<void> setJavaScriptEnabled(int controllerId, bool enabled) async {}

  @override
  Future<void> setBackgroundColor(int controllerId, Color color) async {}

  @override
  Future<void> setUserAgent(int controllerId, String? userAgent) async {}

  @override
  Future<void> enableZoom(int controllerId, bool enabled) async {}

  @override
  Future<void> setHasNavigationDecision(int controllerId, bool enabled) async {}

  @override
  Future<void> runJavaScript(int controllerId, String script) async {}

  @override
  Future<Object?> runJavaScriptReturningResult(
    int controllerId,
    String script,
  ) async => null;

  @override
  Future<void> addJavaScriptChannel(int controllerId, String name) async {}

  @override
  Future<void> removeJavaScriptChannel(int controllerId, String name) async {}

  @override
  Future<void> reload(int controllerId) async {}

  @override
  Future<void> stopLoading(int controllerId) async {}

  @override
  Future<bool> canGoBack(int controllerId) async => false;

  @override
  Future<bool> canGoForward(int controllerId) async => false;

  @override
  Future<void> goBack(int controllerId) async {}

  @override
  Future<void> goForward(int controllerId) async {}

  @override
  Future<String?> currentUrl(int controllerId) async => null;

  @override
  Future<String?> getTitle(int controllerId) async => null;

  @override
  Future<void> scrollTo(int controllerId, int x, int y) async {}

  @override
  Future<void> scrollBy(int controllerId, int x, int y) async {}

  @override
  Future<Offset> getScrollPosition(int controllerId) async => Offset.zero;

  @override
  Future<void> clearCache(int controllerId) async {}

  @override
  Future<void> clearLocalStorage(int controllerId) async {}

  @override
  Future<bool> clearCookies() async => false;

  @override
  Future<void> setCookie(WebViewCookie cookie) async {}

  @override
  Future<void> dispose(int controllerId) async {}
}
