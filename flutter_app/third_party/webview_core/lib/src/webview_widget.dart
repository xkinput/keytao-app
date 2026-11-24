import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';

import 'platform_bridge.dart';
import 'types.dart';
import 'webview_controller.dart';

/// Shorthand alias for [WebViewWidget].
typedef WebView = WebViewWidget;

/// Cross-platform widget rendering a native or browser WebView controlled by
/// [controller].
///
/// Built exclusively on `package:flutter/widgets.dart` so it embeds cleanly
/// inside `WidgetsApp`, `MaterialApp`, `CupertinoApp`, split panes, `Offstage`,
/// `Visibility`, and `IndexedStack` containers without ancestor theme
/// dependencies.
class WebViewWidget extends StatefulWidget {
  /// Creates a [WebViewWidget] bound to [controller].
  const WebViewWidget({
    required this.controller,
    this.gestureRecognizers = const <Factory<OneSequenceGestureRecognizer>>{},
    super.key,
  });

  /// Controller driving content, JavaScript channels, and navigation policy.
  final WebViewController controller;

  /// Gesture recognizers forwarded to mobile platform views (`AndroidView` and
  /// `UiKitView`) when nested inside scrollable Flutter parents.
  final Set<Factory<OneSequenceGestureRecognizer>> gestureRecognizers;

  @override
  State<WebViewWidget> createState() => _WebViewWidgetState();
}

class _WebViewWidgetState extends State<WebViewWidget> {
  late WebViewCreationParams _params;

  @override
  void initState() {
    super.initState();
    _params = widget.controller.buildCreationParams(
      gestureRecognizers: widget.gestureRecognizers,
    );
  }

  @override
  void didUpdateWidget(covariant WebViewWidget oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.controller, widget.controller)) {
      oldWidget.controller.handleWidgetUnmounted();
      _params = widget.controller.buildCreationParams(
        gestureRecognizers: widget.gestureRecognizers,
      );
    }
  }

  @override
  void dispose() {
    widget.controller.handleWidgetUnmounted();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    if (widget.controller.isDisposed) {
      return const SizedBox.shrink();
    }
    return ValueListenableBuilder<Color>(
      valueListenable: widget.controller.backgroundColorListenable,
      builder: (BuildContext context, Color bgColor, Widget? _) {
        final updatedParams = WebViewCreationParams(
          controllerId: _params.controllerId,
          javaScriptEnabled: _params.javaScriptEnabled,
          backgroundColor: bgColor,
          zoomEnabled: _params.zoomEnabled,
          hasNavigationDecision: _params.hasNavigationDecision,
          channelNames: _params.channelNames,
          gestureRecognizers: widget.gestureRecognizers,
          userAgent: _params.userAgent,
          initialHtml: _params.initialHtml,
          initialBaseUrl: _params.initialBaseUrl,
          initialUrl: _params.initialUrl,
          initialMethod: _params.initialMethod,
          initialHeaders: _params.initialHeaders,
          initialBody: _params.initialBody,
        );
        return WebViewPlatformBridge.active.buildView(
          context: context,
          params: updatedParams,
          key: ValueKey<int>(_params.controllerId),
        );
      },
    );
  }
}
