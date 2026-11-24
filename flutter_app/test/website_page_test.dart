import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/app/app.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/rust/frb_generated.dart';

import 'view_fixture.dart';

class _WebsiteCore implements RustLibApi {
  final settings = <(String, bool)>[];

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

void main() {
  testWidgets(
    'website changes domain using the native view and can be hidden',
    (tester) async {
      final core = _WebsiteCore();
      RustLib.initMock(api: core);
      addTearDown(RustLib.dispose);
      final controller = ViewController(BridgePlatform.linux);
      addTearDown(controller.dispose);
      const channel = MethodChannel('com.infradise.webview_core');
      final creations = <Map<Object?, Object?>>[];
      final messenger = tester.binding.defaultBinaryMessenger;
      messenger.setMockMethodCallHandler(channel, (call) async {
        if (call.method == 'create') {
          creations.add(call.arguments as Map<Object?, Object?>);
        }
        return null;
      });
      addTearDown(() => messenger.setMockMethodCallHandler(channel, null));

      Future<void> finishLoading() async {
        final view = creations.last;
        await messenger.handlePlatformMessage(
          channel.name,
          channel.codec.encodeMethodCall(
            MethodCall('onPageFinished', {
              'controllerId': view['controllerId'],
              'url': view['initialUrl'],
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
      expect(creations.single['initialUrl'], 'https://keytao.rea.ink');
      expect(creations.single['initialHeaders'], isEmpty);
      await finishLoading();

      await tester.tap(find.text('keytao.rea.ink'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('keytao.vercel.app'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 300));
      expect(creations.last['initialUrl'], 'https://keytao.vercel.app');
      expect(creations.last['initialHeaders'], isEmpty);
      await finishLoading();

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
