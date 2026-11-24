import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/app/app.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/rust/frb_generated.dart';

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
      final requests = <Map<Object?, Object?>>[];
      final messenger = tester.binding.defaultBinaryMessenger;
      messenger.setMockMethodCallHandler(channel, (call) async {
        if (call.method == 'create') {
          creations.add(call.arguments as Map<Object?, Object?>);
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

      await tester.tap(find.byKey(const ValueKey('nav-about')));
      await tester.pumpAndSettle();
      final toggle = find.byType(FSwitch);
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
