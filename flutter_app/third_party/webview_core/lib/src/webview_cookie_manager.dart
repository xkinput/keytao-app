import 'platform_bridge.dart';
import 'types.dart';

/// Manages HTTP cookies shared by WebView instances across supported platforms.
class WebViewCookieManager {
  /// Creates a [WebViewCookieManager].
  WebViewCookieManager();

  /// Clears all cookies stored in the underlying browser cookie store.
  ///
  /// Returns `true` when cookies were cleared by the platform store, or `false`
  /// when no cookies existed or the active runtime restricts global cookie
  /// deletion (such as browser iframe sandboxes on Web).
  Future<bool> clearCookies() {
    return WebViewPlatformBridge.active.clearCookies();
  }

  /// Sets [cookie] in the underlying browser cookie store.
  Future<void> setCookie(WebViewCookie cookie) {
    return WebViewPlatformBridge.active.setCookie(cookie);
  }
}
