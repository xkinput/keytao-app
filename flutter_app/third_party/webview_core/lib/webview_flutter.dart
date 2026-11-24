/// Drop-in compatibility barrel for `package:webview_flutter/webview_flutter.dart`.
///
/// Replace `import 'package:webview_flutter/webview_flutter.dart';` with
/// `import 'package:webview_core/webview_flutter.dart';` to gain Linux,
/// Windows, macOS, Android, iOS, and WebAssembly support with zero external
/// hosted dependencies.
library;

export 'webview_core.dart';
