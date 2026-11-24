import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';

/// Controls whether JavaScript execution is allowed inside a WebView.
enum JavaScriptMode {
  /// JavaScript execution is disabled.
  disabled,

  /// JavaScript execution is enabled without restriction.
  unrestricted,
}

/// HTTP request method used by `WebViewController.loadRequest`.
enum LoadRequestMethod {
  /// HTTP `GET` request.
  get,

  /// HTTP `POST` request.
  post,

  /// HTTP `PUT` request.
  put,

  /// HTTP `DELETE` request.
  delete,

  /// HTTP `PATCH` request.
  patch,

  /// HTTP `HEAD` request.
  head,
}

/// Policy decision returned by a [NavigationRequestCallback].
enum NavigationDecision {
  /// Prevents the navigation from proceeding.
  prevent,

  /// Allows the navigation to proceed.
  navigate,
}

/// Categorized error types reported by [WebResourceError].
enum WebResourceErrorType {
  /// Authentication failed on the server.
  authentication,

  /// Malformed URL.
  badUrl,

  /// Failed to connect to the server.
  connect,

  /// Failed to perform TLS/SSL handshake.
  failedSslHandshake,

  /// Generic file error.
  file,

  /// File not found.
  fileNotFound,

  /// Server or proxy hostname lookup failed.
  hostLookup,

  /// Failed to read or write to the server.
  io,

  /// User authentication failed on a proxy.
  proxyAuthentication,

  /// Too many redirects occurred.
  redirectLoop,

  /// Connection timed out.
  timeout,

  /// Too many requests during this load.
  tooManyRequests,

  /// Unknown or uncategorized platform error.
  unknown,

  /// Resource load was cancelled or flagged as unsafe.
  unsafeResource,

  /// Unsupported authentication scheme.
  unsupportedAuthScheme,

  /// Unsupported URI scheme.
  unsupportedScheme,

  /// Web content process terminated unexpectedly.
  webContentProcessTerminated,

  /// WebView rendering process crashed or became unresponsive.
  webViewInvalidated,

  /// JavaScript exception occurred during execution.
  javaScriptExceptionOccurred,

  /// JavaScript result type is unsupported.
  javaScriptResultTypeIsUnsupported,
}

/// Details describing a navigation action intercepted by [NavigationDelegate].
@immutable
class NavigationRequest {
  /// Creates a [NavigationRequest].
  const NavigationRequest({required this.url, required this.isMainFrame});

  /// Target URL of the navigation request.
  final String url;

  /// Whether the navigation targets the main frame rather than an iframe.
  final bool isMainFrame;

  @override
  String toString() =>
      'NavigationRequest(url: $url, isMainFrame: $isMainFrame)';
}

/// Notification payload delivered when the active URL of a WebView changes.
@immutable
class UrlChange {
  /// Creates a [UrlChange].
  const UrlChange({required this.url});

  /// Updated active URL, or `null` when unavailable.
  final String? url;
}

/// Structured resource loading error reported by a native or web engine.
@immutable
class WebResourceError {
  /// Creates a [WebResourceError].
  const WebResourceError({
    required this.errorCode,
    required this.description,
    this.errorType,
    this.isForMainFrame,
    this.url,
  });

  /// Platform-specific numeric error code.
  final int errorCode;

  /// Human-readable description of the failure.
  final String description;

  /// Normalized category of the error when known.
  final WebResourceErrorType? errorType;

  /// Whether the failed resource belonged to the main frame.
  final bool? isForMainFrame;

  /// URL of the failing resource when available.
  final String? url;

  @override
  String toString() =>
      'WebResourceError(errorCode: $errorCode, description: $description, '
      'errorType: $errorType, isForMainFrame: $isForMainFrame, url: $url)';
}

/// Message received from JavaScript over a named channel registered via
/// `WebViewController.addJavaScriptChannel`.
@immutable
class JavaScriptMessage {
  /// Creates a [JavaScriptMessage].
  const JavaScriptMessage({required this.message});

  /// String payload sent by JavaScript `postMessage`.
  final String message;
}

/// Signature for page lifecycle callbacks receiving the current [url].
typedef PageEventCallback = void Function(String url);

/// Signature for loading progress callbacks receiving [progress] (`0..100`).
typedef ProgressCallback = void Function(int progress);

/// Signature for resource failure callbacks receiving [error].
typedef WebResourceErrorCallback = void Function(WebResourceError error);

/// Signature for navigation policy callbacks returning a [NavigationDecision].
typedef NavigationRequestCallback =
    FutureOr<NavigationDecision> Function(NavigationRequest request);

/// Signature for URL change callbacks receiving [change].
typedef UrlChangeCallback = void Function(UrlChange change);

/// Delegate grouping page lifecycle, progress, error, and navigation callbacks
/// for a `WebViewController`.
@immutable
class NavigationDelegate {
  /// Creates a [NavigationDelegate].
  const NavigationDelegate({
    this.onProgress,
    this.onPageStarted,
    this.onPageFinished,
    this.onWebResourceError,
    this.onNavigationRequest,
    this.onUrlChange,
    this.onScrollChanged,
  });

  /// Invoked when page loading progress updates (`0` to `100`).
  final ProgressCallback? onProgress;

  /// Invoked when a main-frame page load begins.
  final PageEventCallback? onPageStarted;

  /// Invoked when a main-frame page load completes.
  final PageEventCallback? onPageFinished;

  /// Invoked when a resource or main-frame navigation fails to load.
  final WebResourceErrorCallback? onWebResourceError;

  /// Invoked before navigating so the caller can allow or prevent navigation.
  final NavigationRequestCallback? onNavigationRequest;

  /// Invoked whenever the active URL changes.
  final UrlChangeCallback? onUrlChange;

  /// Reports the main document's scroll offset in logical pixels on mobile.
  final ValueChanged<Offset>? onScrollChanged;
}

/// HTTP cookie descriptor managed through `WebViewCookieManager`.
@immutable
class WebViewCookie {
  /// Creates a [WebViewCookie].
  const WebViewCookie({
    required this.name,
    required this.value,
    required this.domain,
    this.path = '/',
  });

  /// Cookie name.
  final String name;

  /// Cookie value.
  final String value;

  /// Domain scope for which the cookie applies.
  final String domain;

  /// URL path scope for which the cookie applies. Defaults to `'/'`.
  final String path;

  /// Serializes this cookie for native channel transmission.
  Map<String, Object?> toMap() {
    return <String, Object?>{
      'name': name,
      'value': value,
      'domain': domain,
      'path': path,
    };
  }
}

/// Internal snapshot of controller configuration passed at view creation time.
@immutable
class WebViewCreationParams {
  /// Creates [WebViewCreationParams].
  const WebViewCreationParams({
    required this.controllerId,
    required this.javaScriptEnabled,
    required this.backgroundColor,
    required this.zoomEnabled,
    required this.hasNavigationDecision,
    required this.channelNames,
    this.gestureRecognizers = const <Factory<OneSequenceGestureRecognizer>>{},
    this.userAgent,
    this.initialHtml,
    this.initialBaseUrl,
    this.initialUrl,
    this.initialMethod = 'GET',
    this.initialHeaders = const <String, String>{},
    this.initialBody,
  });

  /// Unique numeric identifier of the owning `WebViewController`.
  final int controllerId;

  /// Whether JavaScript execution starts enabled.
  final bool javaScriptEnabled;

  /// Initial background color applied prior to first content paint.
  final Color backgroundColor;

  /// Whether pinch/page zoom gestures are enabled.
  final bool zoomEnabled;

  /// Whether Dart registered an [NavigationDelegate.onNavigationRequest] hook.
  final bool hasNavigationDecision;

  /// Names of JavaScript message channels registered before mount.
  final List<String> channelNames;

  /// Gesture recognizers forwarded to mobile platform views (`AndroidView` /
  /// `UiKitView`) when embedded inside scrollable Flutter containers.
  final Set<Factory<OneSequenceGestureRecognizer>> gestureRecognizers;

  /// Optional custom `User-Agent` string.
  final String? userAgent;

  /// Optional HTML string queued before widget mount.
  final String? initialHtml;

  /// Optional base URL paired with [initialHtml].
  final String? initialBaseUrl;

  /// Optional target URL queued before widget mount.
  final String? initialUrl;

  /// HTTP method paired with [initialUrl].
  final String initialMethod;

  /// HTTP headers paired with [initialUrl].
  final Map<String, String> initialHeaders;

  /// Optional request body paired with [initialUrl].
  final Uint8List? initialBody;

  /// Encodes this snapshot into a standard codec map for native channels.
  Map<String, Object?> toMap() {
    return <String, Object?>{
      'controllerId': controllerId,
      'javaScriptEnabled': javaScriptEnabled,
      'backgroundColor': backgroundColor.toARGB32(),
      'zoomEnabled': zoomEnabled,
      'hasNavigationDecision': hasNavigationDecision,
      'channelNames': channelNames,
      'userAgent': userAgent,
      'initialHtml': initialHtml,
      'initialBaseUrl': initialBaseUrl,
      'initialUrl': initialUrl,
      'initialMethod': initialMethod,
      'initialHeaders': initialHeaders,
      'initialBody': initialBody,
    };
  }
}
