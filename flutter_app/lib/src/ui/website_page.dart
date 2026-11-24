import 'dart:async';

import 'package:flutter/widgets.dart';
import 'package:forui/forui.dart';
import 'package:webview_core/webview_core.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../rust/api/core.dart' as core;
import 'widgets.dart';

const _sessionPath = '/api/auth/app-webview-session';
const _sessionHeader = 'X-KeyTao-App-Session';

class WebsitePage extends StatefulWidget {
  const WebsitePage({
    super.key,
    required this.controller,
    required this.active,
  });

  final AppController controller;
  final bool active;

  @override
  State<WebsitePage> createState() => _WebsitePageState();
}

class _WebsitePageState extends State<WebsitePage> {
  late bool _opened = widget.active;

  @override
  void didUpdateWidget(covariant WebsitePage oldWidget) {
    super.didUpdateWidget(oldWidget);
    _opened = _opened || widget.active;
  }

  @override
  Widget build(BuildContext context) {
    if (!_opened) return const SizedBox.shrink();
    final controller = widget.controller;
    final origin = controller.websiteOrigin;
    final account = controller.accountUser;
    return TickerMode(
      enabled: widget.active,
      child: _WebsiteBrowser(
        key: ValueKey((origin, account?.id)),
        controller: controller,
        origin: origin,
      ),
    );
  }
}

class _WebsiteBrowser extends StatefulWidget {
  const _WebsiteBrowser({
    super.key,
    required this.controller,
    required this.origin,
  });

  final AppController controller;
  final String origin;

  @override
  State<_WebsiteBrowser> createState() => _WebsiteBrowserState();
}

class _WebsiteBrowserState extends State<_WebsiteBrowser> {
  static const _loadTimeout = Duration(seconds: 30);
  late WebViewController _browser;
  late String _currentUrl = widget.origin;
  Timer? _timeout;
  bool _loading = true;
  bool _failed = false;
  bool _initializingSession = true;

  @override
  void initState() {
    super.initState();
    _startBrowser();
  }

  void _startBrowser() {
    final browser = WebViewController();
    _browser = browser;
    _currentUrl = widget.origin;
    _initializingSession = true;
    unawaited(_initialize(browser));
    _trackLoad();
  }

  Future<void> _initialize(WebViewController browser) async {
    try {
      final delegate = browser.setNavigationDelegate(
        NavigationDelegate(
          onNavigationRequest: (request) {
            final uri = Uri.tryParse(request.url);
            if (!request.isMainFrame) return NavigationDecision.navigate;
            if (!_isHttps(uri) ||
                (_initializingSession && uri!.origin != widget.origin)) {
              return NavigationDecision.prevent;
            }
            return NavigationDecision.navigate;
          },
          onPageStarted: (_) {
            if (mounted && !_failed && identical(browser, _browser)) {
              _trackLoad();
            }
          },
          onPageFinished: (url) {
            if (!mounted || _failed || !identical(browser, _browser)) return;
            final uri = Uri.tryParse(url);
            if (!_isHttps(uri) || uri!.path == _sessionPath) return;
            if (_initializingSession &&
                (uri.origin != widget.origin ||
                    (uri.path.isNotEmpty && uri.path != '/'))) {
              return;
            }
            _currentUrl = url;
            _timeout?.cancel();
            setState(() {
              _initializingSession = false;
              _loading = false;
            });
          },
          onWebResourceError: (error) {
            if (error.isForMainFrame == true) _showFailure(browser);
          },
        ),
      );
      await Future.wait([delegate, _applySession(browser)]);
    } catch (_) {
      _showFailure(browser);
    }
  }

  Future<void> _applySession(WebViewController browser) async {
    final accountId = widget.controller.accountUser?.id;
    final grant = accountId == null
        ? 'clear'
        : await core.issueWebsiteSession(
            origin: widget.origin,
            accountId: accountId,
          );
    if (!mounted ||
        _failed ||
        !identical(browser, _browser) ||
        accountId != widget.controller.accountUser?.id ||
        widget.origin != widget.controller.websiteOrigin) {
      return;
    }
    await browser.loadRequest(
      Uri.parse(widget.origin).replace(path: _sessionPath),
      headers: {_sessionHeader: grant},
    );
  }

  bool _isHttps(Uri? uri) =>
      uri != null &&
      uri.isScheme('https') &&
      uri.host.isNotEmpty &&
      uri.userInfo.isEmpty;

  void _trackLoad() {
    if (!_loading) setState(() => _loading = true);
    _timeout?.cancel();
    final browser = _browser;
    _timeout = Timer(_loadTimeout, () => _showFailure(browser));
  }

  void _showFailure(WebViewController browser) {
    if (!mounted || !identical(browser, _browser)) return;
    _timeout?.cancel();
    setState(() {
      _loading = false;
      _failed = true;
    });
  }

  Future<void> _reload() async {
    final browser = _browser;
    if (_failed) {
      setState(() {
        _loading = true;
        _failed = false;
        _startBrowser();
      });
      await browser.dispose();
      return;
    }
    if (_initializingSession) return;
    _trackLoad();
    try {
      await browser.reload();
    } catch (_) {
      _showFailure(browser);
    }
  }

  Future<void> _goBack() async {
    if (_failed || _initializingSession) return;
    final browser = _browser;
    try {
      if (await browser.canGoBack()) await browser.goBack();
    } catch (_) {
      _showFailure(browser);
    }
  }

  @override
  void dispose() {
    _timeout?.cancel();
    unawaited(_browser.dispose());
    super.dispose();
  }

  Widget _action(String label, IconData icon, VoidCallback action) => FTooltip(
    tipBuilder: (_, _) => Text(label),
    child: FButton.icon(
      variant: FButtonVariant.ghost,
      onPress: action,
      child: Icon(icon, size: 18, semanticLabel: label),
    ),
  );

  @override
  Widget build(BuildContext context) {
    final controller = widget.controller;
    return Column(
      key: const PageStorageKey('website'),
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (controller.appUpdate?.hasUpdate == true)
          AppUpdateBanner(controller: controller),
        LayoutBuilder(
          builder: (context, constraints) {
            final wide = constraints.maxWidth >= 720;
            return Padding(
              padding: EdgeInsets.symmetric(
                horizontal: wide ? 18 : 16,
                vertical: 8,
              ),
              child: Align(
                child: ConstrainedBox(
                  constraints: BoxConstraints(maxWidth: wide ? 1392 : 760),
                  child: _toolbar(controller),
                ),
              ),
            );
          },
        ),
        if (_loading) const FProgress(),
        Expanded(
          child: _failed
              ? const Center(child: Text('无法在 App 内加载网站'))
              : Opacity(
                  opacity: _initializingSession ? 0 : 1,
                  child: ClipRect(child: WebViewWidget(controller: _browser)),
                ),
        ),
      ],
    );
  }

  Widget _toolbar(AppController controller) => Row(
    spacing: 8,
    children: [
      Expanded(
        child: Align(
          alignment: AlignmentDirectional.centerStart,
          child: SizedBox(
            width: 230,
            child: Semantics(
              label: '网站域名',
              child: FSelect<String>(
                size: FTextFieldSizeVariant.sm,
                enabled: !controller.busy,
                control: FSelectControl.lifted(
                  value: widget.origin,
                  onChange: (origin) {
                    if (origin != null && origin != widget.origin) {
                      unawaited(controller.saveWebsiteSettings(origin: origin));
                    }
                  },
                ),
                items: {
                  for (final origin in keytaoWebsiteOrigins)
                    Uri.parse(origin).host: origin,
                },
              ),
            ),
          ),
        ),
      ),
      _action('返回上一页', FLucideIcons.arrowLeft, _goBack),
      _action('刷新网站', FLucideIcons.refreshCw, _reload),
      _action('在浏览器中打开', FLucideIcons.externalLink, () {
        controller.open(_currentUrl);
      }),
    ],
  );
}
