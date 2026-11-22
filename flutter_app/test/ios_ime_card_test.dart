import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/app/options.dart';
import 'package:keytao/src/platform/ios_host.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/ui/input_page.dart';
import 'package:keytao/src/ui/onboarding.dart';
import 'package:keytao/src/ui/theme.dart';

import 'view_fixture.dart';

void main() {
  for (final state in [
    (name: 'not enabled', enabled: false, fullAccess: null),
    (name: 'full access unconfirmed', enabled: true, fullAccess: null),
    (name: 'full access denied', enabled: true, fullAccess: false),
    (name: 'ready', enabled: true, fullAccess: true),
  ]) {
    testWidgets('iOS card shows only the ${state.name} guidance', (
      tester,
    ) async {
      final c = ViewController(BridgePlatform.ios)
        ..iosKeyboardEnabled = state.enabled
        ..iosKeyboardFullAccess = state.fullAccess;
      addTearDown(c.dispose);
      final calls = <String>[];
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        IosHost.channel,
        (call) async {
          calls.add(call.method);
          return null;
        },
      );
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          IosHost.channel,
          null,
        ),
      );
      await tester.pumpWidget(
        KeyTaoTheme(
          platform: BridgePlatform.ios,
          home: FScaffold(child: IosImeCard(controller: c)),
        ),
      );
      await tester.pumpAndSettle();

      final ready = state.enabled && state.fullAccess == true;
      final visible = {
        if (!state.enabled) ...[
          AppStrings.iosAddKeyboard,
          AppStrings.iosChooseKeyboard,
        ] else if (!ready) ...[
          AppStrings.iosEnableFullAccess,
          AppStrings.iosFullAccessDescription,
          AppStrings.iosKeyboardStatusRefresh,
        ] else ...[
          AppStrings.iosKeyboardReady,
          AppStrings.iosGlobe,
        ],
      };
      expect(
        find.text(AppStrings.iosOnboarding),
        ready ? findsNothing : findsOneWidget,
      );
      for (final text in [
        AppStrings.iosAddKeyboard,
        AppStrings.iosChooseKeyboard,
        AppStrings.iosEnableFullAccess,
        AppStrings.iosFullAccessDescription,
        AppStrings.iosKeyboardStatusRefresh,
        AppStrings.iosKeyboardReady,
        AppStrings.iosGlobe,
        AppStrings.iosInstallDeploy,
      ]) {
        expect(
          find.text(text),
          visible.contains(text) ? findsOneWidget : findsNothing,
        );
      }
      final settings = find.text(AppStrings.openSystemSettings);
      expect(settings, ready ? findsNothing : findsOneWidget);
      if (!ready) {
        await tester.tap(settings);
        await tester.pumpAndSettle();
        expect(calls, ['openSettings']);
      }
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    }, variant: TargetPlatformVariant.only(TargetPlatform.iOS));
  }

  testWidgets('iOS onboarding marks detected keyboard steps complete', (
    tester,
  ) async {
    final c = ViewController(BridgePlatform.ios)
      ..iosKeyboardEnabled = true
      ..iosKeyboardFullAccess = false;
    addTearDown(c.dispose);
    await tester.pumpWidget(
      KeyTaoTheme(
        platform: BridgePlatform.ios,
        home: OnboardingPage(controller: c, onFinished: () {}),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.byKey(const ValueKey('setup-fullAccess')), findsOneWidget);
    expect(find.text(AppStrings.testInput), findsOneWidget);
    for (final (step, done) in [
      (SetupStep.enable, true),
      (SetupStep.fullAccess, false),
    ]) {
      final nav = find.byKey(ValueKey('setup-nav-${step.name}'));
      expect(
        tester.widget<FButton>(nav).semanticsLabel,
        contains(done ? '已完成' : '待完成'),
      );
      expect(
        find.descendant(of: nav, matching: find.byIcon(FLucideIcons.check)),
        done ? findsOneWidget : findsNothing,
      );
      await tester.tap(nav);
      await tester.pumpAndSettle();
      expect(find.text(done ? '已完成' : '待完成'), findsOneWidget);
    }
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  }, variant: TargetPlatformVariant.only(TargetPlatform.iOS));
}
