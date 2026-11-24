/// Web and WebAssembly (`dart2wasm`) plugin registration entrypoint for
/// `webview_core`.
library;

import 'package:flutter_web_plugins/flutter_web_plugins.dart';

import 'src/platform_bridge_web.dart';

/// Web platform registration class for `webview_core` using pure
/// `dart:js_interop` and `dart:ui_web` with zero external packages.
class WebviewCoreWeb {
  /// Creates a [WebviewCoreWeb] plugin instance.
  const WebviewCoreWeb();

  /// Registers Web `postMessage` listeners with Flutter's Web [registrar].
  static void registerWith(Registrar registrar) {
    final bridge = platformBridgeInstance;
    if (bridge is WebWebViewBridge) {
      bridge.ensureInitialized();
    }
  }
}
