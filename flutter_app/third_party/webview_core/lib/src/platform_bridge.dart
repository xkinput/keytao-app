import 'dart:async';
import 'dart:typed_data';

import 'package:flutter/widgets.dart';

import 'platform_bridge_stub.dart'
    if (dart.library.io) 'platform_bridge_io.dart'
    if (dart.library.js_interop) 'platform_bridge_web.dart'
    as impl;
import 'types.dart';

/// Callback sink through which a [WebViewPlatformBridge] delivers native/web
/// lifecycle events to its owning `WebViewController`.
abstract interface class WebViewPlatformCallbacks {
  /// Invoked when the underlying native or DOM view is ready to receive calls.
  void handleViewCreated(int controllerId);

  /// Invoked when a main-frame load starts.
  void handlePageStarted(int controllerId, String url);

  /// Invoked when load progress updates (`0..100`).
  void handleProgress(int controllerId, int progress);

  /// Invoked when a main-frame load finishes.
  void handlePageFinished(int controllerId, String url);

  /// Invoked when the active URL changes.
  void handleUrlChange(int controllerId, String? url);

  /// Invoked when the main document scrolls on Android or iOS.
  void handleScrollChanged(int controllerId, Offset offset);

  /// Invoked when a load error occurs.
  void handleWebResourceError(int controllerId, WebResourceError error);

  /// Invoked to decide whether a navigation request should proceed.
  FutureOr<bool> handleNavigationRequest(
    int controllerId,
    NavigationRequest request,
  );

  /// Invoked when JavaScript posts a message to a named channel.
  void handleJavaScriptMessage(
    int controllerId,
    String channelName,
    String message,
  );
}

/// Platform-neutral bridge contract implemented separately for IO (`Android`,
/// `iOS`, `Linux`, `macOS`, `Windows`) and Web/WebAssembly (`dart:js_interop`).
abstract class WebViewPlatformBridge {
  /// Returns the singleton [WebViewPlatformBridge] for the active runtime.
  static WebViewPlatformBridge get instance => impl.platformBridgeInstance;

  /// Allows tests to override the active bridge implementation.
  @visibleForTesting
  static set debugOverrideInstance(WebViewPlatformBridge? override) {
    _override = override;
  }

  static WebViewPlatformBridge? _override;

  /// Resolves either the test override or the active runtime singleton.
  static WebViewPlatformBridge get active => _override ?? instance;

  /// Registers a controller callback sink by [controllerId].
  void registerController(int controllerId, WebViewPlatformCallbacks callbacks);

  /// Unregisters a controller callback sink by [controllerId].
  void unregisterController(int controllerId);

  /// Builds the platform-specific widget hosting the WebView for [params].
  Widget buildView({
    required BuildContext context,
    required WebViewCreationParams params,
    Key? key,
  });

  /// Loads [html] with an optional [baseUrl].
  Future<void> loadHtmlString(int controllerId, String html, {String? baseUrl});

  /// Loads an HTTP/HTTPS/custom-scheme [url] request.
  Future<void> loadRequest(
    int controllerId, {
    required String url,
    required String method,
    required Map<String, String> headers,
    Uint8List? body,
  });

  /// Loads a local filesystem [path].
  Future<void> loadFile(int controllerId, String path);

  /// Loads a bundled Flutter [assetKey].
  Future<void> loadFlutterAsset(int controllerId, String assetKey);

  /// Enables or disables JavaScript execution.
  Future<void> setJavaScriptEnabled(int controllerId, bool enabled);

  /// Updates the native or DOM background [color].
  Future<void> setBackgroundColor(int controllerId, Color color);

  /// Sets an optional custom [userAgent].
  Future<void> setUserAgent(int controllerId, String? userAgent);

  /// Enables or disables pinch/page zoom.
  Future<void> enableZoom(int controllerId, bool enabled);

  /// Informs the native side whether navigation interception is active.
  Future<void> setHasNavigationDecision(int controllerId, bool enabled);

  /// Evaluates [script] without awaiting a serialized return value.
  Future<void> runJavaScript(int controllerId, String script);

  /// Evaluates [script] and returns the decoded result.
  Future<Object?> runJavaScriptReturningResult(int controllerId, String script);

  /// Registers a named JavaScript message channel.
  Future<void> addJavaScriptChannel(int controllerId, String name);

  /// Removes a named JavaScript message channel.
  Future<void> removeJavaScriptChannel(int controllerId, String name);

  /// Reloads the current page.
  Future<void> reload(int controllerId);

  /// Stops an in-progress page load.
  Future<void> stopLoading(int controllerId);

  /// Returns `true` if backward history navigation is available.
  Future<bool> canGoBack(int controllerId);

  /// Returns `true` if forward history navigation is available.
  Future<bool> canGoForward(int controllerId);

  /// Navigates one entry backward in history.
  Future<void> goBack(int controllerId);

  /// Navigates one entry forward in history.
  Future<void> goForward(int controllerId);

  /// Returns the current URL, or `null` if none is loaded.
  Future<String?> currentUrl(int controllerId);

  /// Returns the current document title, or `null` if unavailable.
  Future<String?> getTitle(int controllerId);

  /// Scrolls the page to (`[x]`, `[y]`).
  Future<void> scrollTo(int controllerId, int x, int y);

  /// Scrolls the page relative to its current offset by (`[x]`, `[y]`).
  Future<void> scrollBy(int controllerId, int x, int y);

  /// Returns the current scroll position in logical pixels.
  Future<Offset> getScrollPosition(int controllerId);

  /// Clears cached network and disk resources for the WebView.
  Future<void> clearCache(int controllerId);

  /// Clears DOM `localStorage` data for the WebView.
  Future<void> clearLocalStorage(int controllerId);

  /// Clears all cookies in the platform cookie store.
  Future<bool> clearCookies();

  /// Sets [cookie] in the platform cookie store.
  Future<void> setCookie(WebViewCookie cookie);

  /// Disposes native/DOM resources owned by [controllerId].
  Future<void> dispose(int controllerId);
}
