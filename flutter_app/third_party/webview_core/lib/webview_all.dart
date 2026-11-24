/// Drop-in compatibility barrel for `package:webview_all/webview_all.dart`.
///
/// Replace `import 'package:webview_all/webview_all.dart';` with
/// `import 'package:webview_core/webview_all.dart';` to eliminate federated
/// transitive packages while preserving `WebViewController`, `WebViewWidget`,
/// `JavaScriptMode`, and `NavigationDelegate`.
library;

export 'webview_core.dart';
