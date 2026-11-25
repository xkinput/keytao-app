import 'dart:async';

import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/app/app.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/rust/frb_generated.dart';
import 'package:keytao/src/ui/theme.dart';
import 'package:keytao/src/ui/website_page.dart';
import 'package:keytao/src/ui/widgets.dart';
import 'package:webview_core/webview_core.dart' show WebViewWidget;

import 'view_fixture.dart';

class _WebsiteCore implements RustLibApi {
  final settings = <(String, bool)>[];
  final grants = <(String, PlatformInt64, Completer<String>)>[];

  @override
  Future<String> crateApiCoreIssueWebsiteSession({
    required String origin,
    required PlatformInt64 accountId,
  }) {
    final result = Completer<String>();
    grants.add((origin, accountId, result));
    return result.future;
  }

  @override
  Future<void> crateApiCoreSetWebsiteSettings({
    required String origin,
    required bool hidden,
  }) async {
    settings.add((origin, hidden));
  }

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

class _WebsiteController extends ViewController {
  _WebsiteController() : super(BridgePlatform.linux);

  void changeAccount(AccountUserDto? user) {
    accountUser = user;
    notifyListeners();
  }
}

void main() {
  testWidgets('mobile toolbar fits at 320 and reports the active view offset', (
    tester,
  ) async {
    final core = _WebsiteCore();
    RustLib.initMock(api: core);
    addTearDown(RustLib.dispose);
    final controller = ViewController(BridgePlatform.android);
    final toolbarVisible = ValueNotifier(true);
    final toolbarFraction = ValueNotifier(1.0);
    addTearDown(controller.dispose);
    addTearDown(toolbarVisible.dispose);
    addTearDown(toolbarFraction.dispose);
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(320, 700);
    addTearDown(tester.view.resetDevicePixelRatio);
    addTearDown(tester.view.resetPhysicalSize);

    const channel = MethodChannel('com.infradise.webview_core');
    final messenger = tester.binding.defaultBinaryMessenger;
    final ids = <int>[];
    messenger.setMockMethodCallHandler(channel, (call) async {
      if (call.method == 'create') {
        ids.add((call.arguments as Map)['controllerId'] as int);
      }
      return null;
    });
    addTearDown(() => messenger.setMockMethodCallHandler(channel, null));

    Future<void> event(int id, String name, Map<String, Object?> payload) =>
        messenger.handlePlatformMessage(
          channel.name,
          channel.codec.encodeMethodCall(
            MethodCall(name, {'controllerId': id, ...payload}),
          ),
          null,
        );

    final offsets = <double>[];
    final account = find.byKey(const ValueKey('website-account'));
    await tester.pumpWidget(
      KeyTaoTheme(
        platform: BridgePlatform.android,
        home: FScaffold(
          child: MediaQuery(
            data: const MediaQueryData(
              size: Size(320, 700),
              padding: EdgeInsets.only(top: 32),
              disableAnimations: true,
            ),
            child: ListenableBuilder(
              listenable: Listenable.merge([controller, toolbarVisible]),
              builder: (_, _) => WebsitePage(
                controller: controller,
                active: true,
                mobileAccount: const Icon(
                  FLucideIcons.user,
                  key: ValueKey('website-account'),
                ),
                mobileToolbarVisible: toolbarVisible.value,
                mobileToolbarFraction: toolbarFraction,
                onScrollOffset: offsets.add,
              ),
            ),
          ),
        ),
      ),
    );
    await tester.pump();

    final firstId = ids.single;
    await event(firstId, 'onPageFinished', {'url': 'https://keytao.rea.ink'});
    await tester.pumpAndSettle();
    expect(offsets, [0]);
    final domain = find.byWidgetPredicate(
      (widget) => widget is FSelect<String>,
    );
    final actions = find.byType(FButton);
    expect(tester.takeException(), isNull);
    expect(tester.getCenter(domain).dy, tester.getCenter(account).dy);
    expect(actions, findsNWidgets(3));
    for (var i = 0; i < 3; i++) {
      expect(tester.getSize(actions.at(i)), const Size.square(44));
      expect(tester.getCenter(actions.at(i)).dy, tester.getCenter(account).dy);
    }

    final page = find.byKey(const PageStorageKey('website'));
    expect(tester.getRect(page).top, 32);
    final webView = find.byType(WebViewWidget);
    expect(
      tester
          .widget<WebViewWidget>(webView)
          .gestureRecognizers
          .map((f) => f.type),
      unorderedEquals([
        VerticalDragGestureRecognizer,
        TapGestureRecognizer,
        LongPressGestureRecognizer,
      ]),
    );
    final expandedTop = tester.getTopLeft(webView).dy;
    toolbarFraction.value = .6;
    await tester.pump();
    final partialTop = tester.getTopLeft(webView).dy;
    expect(partialTop, lessThan(expandedTop));
    toolbarFraction.value = .3;
    await tester.pump();
    expect(tester.getTopLeft(webView).dy, lessThan(partialTop));
    toolbarVisible.value = false;
    await tester.pump();
    expect(domain, findsNothing);
    expect(actions, findsNothing);
    expect(account, findsNothing);
    expect(tester.getRect(page).top, 32);

    await event(firstId, 'onScrollChanged', {'x': 0, 'y': 61.5});
    await event(firstId, 'onScrollChanged', {'x': 0, 'y': 61.5});
    expect(offsets, [0, 61.5]);
    await event(firstId, 'onPageStarted', {
      'url': 'https://keytao.rea.ink/docs',
    });
    expect(offsets, [0, 61.5, 0]);

    await controller.saveWebsiteSettings(origin: 'https://keytao.vercel.app');
    await tester.pump();
    await tester.pump();
    final secondId = ids.last;
    expect(secondId, isNot(firstId));
    expect(offsets.last, 0);
    await event(firstId, 'onScrollChanged', {'x': 0, 'y': 190.0});
    expect(offsets.last, 0);
    await event(secondId, 'onPageFinished', {
      'url': 'https://keytao.vercel.app',
    });
    await event(secondId, 'onScrollChanged', {'x': 0, 'y': 23.0});
    expect(offsets.last, 23);
  }, variant: TargetPlatformVariant.only(TargetPlatform.linux));

  testWidgets(
    'website aligns its toolbar and isolates account and domain changes',
    (tester) async {
      final core = _WebsiteCore();
      RustLib.initMock(api: core);
      addTearDown(RustLib.dispose);
      final controller = _WebsiteController();
      addTearDown(controller.dispose);
      const channel = MethodChannel('com.infradise.webview_core');
      const sessionPath = '/api/auth/app-webview-session';
      const sessionHeader = 'X-KeyTao-App-Session';
      final creations = <Map<Object?, Object?>>[];
      final disposals = <Object?>[];
      final requests = <Map<Object?, Object?>>[];
      final messenger = tester.binding.defaultBinaryMessenger;
      messenger.setMockMethodCallHandler(channel, (call) async {
        if (call.method == 'create') {
          creations.add(call.arguments as Map<Object?, Object?>);
        }
        if (call.method == 'dispose') {
          disposals.add((call.arguments as Map)['controllerId']);
        }
        if (call.method == 'loadRequest') {
          requests.add(call.arguments as Map<Object?, Object?>);
        }
        return null;
      });
      addTearDown(() => messenger.setMockMethodCallHandler(channel, null));

      Future<void> finishLoading(String origin) async {
        final view = creations.last;
        await messenger.handlePlatformMessage(
          channel.name,
          channel.codec.encodeMethodCall(
            MethodCall('onPageFinished', {
              'controllerId': view['controllerId'],
              'url': origin,
            }),
          ),
          null,
        );
        await tester.pumpAndSettle();
      }

      await tester.pumpWidget(
        KeyTaoApp(
          controller: controller,
          initialOnboarding: const OnboardingDto(completed: true),
        ),
      );
      await tester.pumpAndSettle();
      expect(creations, isEmpty);
      final website = find.byKey(const ValueKey('nav-website'));
      await tester.tap(website);
      await tester.pump();
      expect(
        creations.single['initialUrl'],
        'https://keytao.rea.ink$sessionPath',
      );
      expect(creations.single['initialHeaders'], {sessionHeader: 'clear'});
      await finishLoading('https://keytao.rea.ink');
      final firstId = creations.single['controllerId'];
      final about = find.byKey(const ValueKey('nav-about'));
      await tester.tap(about);
      await tester.pump();
      await tester.pump(const Duration(minutes: 4));
      await tester.tap(website);
      await tester.pumpAndSettle();
      expect(creations, hasLength(1));
      expect(disposals, isEmpty);

      await tester.tap(about);
      await tester.pump();
      await tester.pump(const Duration(minutes: 4, seconds: 59));
      expect(disposals, isEmpty);
      await tester.pump(const Duration(seconds: 1));
      expect(disposals, [firstId]);
      expect(creations, hasLength(1));

      await tester.tap(website);
      await tester.pump();
      expect(creations, hasLength(2));
      expect(creations.last['controllerId'], isNot(firstId));
      await finishLoading('https://keytao.rea.ink');

      final external = find.ancestor(
        of: find.byWidgetPredicate(
          (widget) => widget is Icon && widget.semanticLabel == '在浏览器中打开',
        ),
        matching: find.byType(FButton),
      );
      final page = find.byKey(const PageStorageKey('website'));
      expect(tester.getRect(external).right, tester.getRect(page).right - 18);

      await tester.tap(find.text('keytao.rea.ink'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('keytao.vercel.app'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 300));
      expect(
        creations.last['initialUrl'],
        'https://keytao.vercel.app$sessionPath',
      );
      expect(creations.last['initialHeaders'], {sessionHeader: 'clear'});
      await finishLoading('https://keytao.vercel.app');

      controller.changeAccount(const AccountUserDto(id: 7));
      await tester.pump();
      controller.changeAccount(const AccountUserDto(id: 8));
      await tester.pump();
      expect(core.grants.map((grant) => (grant.$1, grant.$2)), [
        ('https://keytao.vercel.app', 7),
        ('https://keytao.vercel.app', 8),
      ]);
      core.grants.first.$3.complete('expired.web.grant');
      await tester.pump();
      expect(requests, isEmpty);
      core.grants.last.$3.complete('current.web.grant');
      await tester.pump();
      expect(requests.single['url'], 'https://keytao.vercel.app$sessionPath');
      expect(requests.single['headers'], {sessionHeader: 'current.web.grant'});
      await finishLoading('https://keytao.vercel.app');
      controller.changeAccount(null);
      await tester.pump();
      expect(creations.last['initialHeaders'], {sessionHeader: 'clear'});
      await finishLoading('https://keytao.vercel.app');

      await tester.tap(find.byKey(const ValueKey('nav-ime')));
      await tester.pumpAndSettle();
      final toggle = find.descendant(
        of: find.byWidgetPredicate(
          (widget) => widget is SwitchSetting && widget.label == '显示网站',
        ),
        matching: find.byType(FSwitch),
      );
      await tester.ensureVisible(toggle);
      await tester.tap(toggle);
      await tester.pumpAndSettle();
      expect(website, findsNothing);
      await tester.tap(toggle);
      await tester.pumpAndSettle();
      expect(website, findsOneWidget);
      expect(core.settings, [
        ('https://keytao.vercel.app', false),
        ('https://keytao.vercel.app', true),
        ('https://keytao.vercel.app', false),
      ]);
    },
    variant: TargetPlatformVariant.only(TargetPlatform.linux),
  );
}
