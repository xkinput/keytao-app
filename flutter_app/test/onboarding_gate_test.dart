import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:keytao/src/app/app.dart';
import 'package:keytao/src/app/controller.dart';
import 'package:keytao/src/app/shell.dart';
import 'package:keytao/src/onboarding/onboarding_page.dart';
import 'package:keytao/src/rust/api/types.dart';

void main() {
  testWidgets(
    'injected completion selects onboarding or shell without native initialization',
    (tester) async {
      final controller = AppController(
        info: const BridgeInfo(
          platform: BridgePlatform.android,
          appVersion: 'test',
          userRoot: '/test/keytao',
        ),
      );
      addTearDown(controller.dispose);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      tester.view.devicePixelRatio = 1;
      for (final width in [360.0, 1000.0]) {
        tester.view.physicalSize = Size(width, 800);
        for (final completed in [false, true]) {
          await tester.pumpWidget(
            KeyTaoApp(
              key: ValueKey('$width-$completed'),
              controller: controller,
              initialOnboarding: OnboardingDto(completed: completed),
            ),
          );
          await tester.pumpAndSettle();
          expect(
            find.byType(OnboardingPage),
            completed ? findsNothing : findsOneWidget,
          );
          expect(
            find.byType(AppShell),
            completed ? findsOneWidget : findsNothing,
          );
          expect(find.text('输入法'), completed ? findsOneWidget : findsNothing);
          expect(
            find.text('文件访问权限'),
            completed ? findsNothing : findsOneWidget,
          );
          expect(tester.takeException(), isNull);
        }
      }
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );
}
