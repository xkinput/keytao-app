import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:keytao/src/app/app.dart';
import 'package:keytao/src/app/options.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/ui/onboarding.dart';

import 'unified_gate_cases.dart';
import 'view_fixture.dart';

class _SetupController extends ViewController {
  _SetupController(super.platform) {
    local = const LocalSchemaDto(
      installed: false,
      deployed: false,
      schemas: [],
    );
  }

  int finishCalls = 0;
  int iosChecks = 0;
  bool nativeFullAccess = false;

  void detectedAndroid({
    required bool enabled,
    required bool selected,
    required bool writable,
  }) {
    ime = {'enabled': enabled, 'selected': selected, 'canShowPicker': enabled};
    storage = {
      'granted': writable,
      'writable': writable,
      'canOpenSettings': true,
    };
    notifyListeners();
  }

  void detectedIos({required bool enabled, required bool fullAccess}) {
    iosKeyboardEnabled = enabled;
    iosKeyboardFullAccess = fullAccess;
    notifyListeners();
  }

  @override
  Future<bool> finishOnboarding() async {
    finishCalls++;
    onboardingCompleted = setupReady;
    return onboardingCompleted;
  }

  @override
  Future<void> refreshIosKeyboardStatus() async {
    iosChecks++;
    detectedIos(enabled: true, fullAccess: nativeFullAccess);
  }
}

void main() {
  unifiedGateCases(BridgePlatform.android);

  testWidgets('Android detected setup advances and opens schemes', (
    tester,
  ) async {
    final c = _SetupController(BridgePlatform.android)
      ..detectedAndroid(enabled: false, selected: false, writable: false);
    addTearDown(c.dispose);
    await tester.pumpWidget(
      KeyTaoApp(
        controller: c,
        initialOnboarding: const OnboardingDto(completed: false),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('setup-enable')), findsOneWidget);

    c.detectedAndroid(enabled: true, selected: false, writable: false);
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('setup-select')), findsOneWidget);

    await tester.tap(find.text('返回'));
    await tester.pumpAndSettle();
    c.detectedAndroid(enabled: true, selected: false, writable: false);
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('setup-enable')), findsOneWidget);

    c.detectedAndroid(enabled: true, selected: true, writable: false);
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('setup-storage')), findsOneWidget);
    expect(find.byKey(const ValueKey('setup-nav-install')), findsNothing);
    expect(find.byKey(const ValueKey('setup-nav-deploy')), findsNothing);
    expect(c.finishCalls, 0);

    c.detectedAndroid(enabled: true, selected: true, writable: true);
    await tester.pumpAndSettle();
    expect(find.byType(OnboardingPage), findsNothing);
    expect(find.byKey(const PageStorageKey('scheme')), findsOneWidget);
    expect(c.activePage, AppPage.scheme);
    expect(c.finishCalls, 1);
    expect(c.local!.installed, isFalse);
  }, variant: TargetPlatformVariant.only(TargetPlatform.android));

  testWidgets('iOS detected keyboard access opens schemes', (tester) async {
    final c = _SetupController(BridgePlatform.ios)
      ..detectedIos(enabled: false, fullAccess: false);
    addTearDown(c.dispose);
    await tester.pumpWidget(
      KeyTaoApp(
        controller: c,
        initialOnboarding: const OnboardingDto(completed: false),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('setup-enable')), findsOneWidget);

    c.detectedIos(enabled: true, fullAccess: false);
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('setup-fullAccess')), findsOneWidget);
    expect(find.byKey(const ValueKey('setup-nav-install')), findsNothing);
    expect(find.byKey(const ValueKey('setup-nav-deploy')), findsNothing);

    final input = find.byType(EditableText);
    await tester.ensureVisible(input);
    await tester.pumpAndSettle();
    await tester.tap(input);
    await tester.pump();
    c.nativeFullAccess = true;
    await tester.pump(const Duration(milliseconds: 500));
    await tester.pumpAndSettle();
    expect(find.byType(OnboardingPage), findsNothing);
    expect(find.byKey(const PageStorageKey('scheme')), findsOneWidget);
    expect(c.activePage, AppPage.scheme);
    expect(c.finishCalls, 1);
    await tester.pump(const Duration(seconds: 1));
    expect(c.iosChecks, 1);
    expect(c.local!.installed, isFalse);
  }, variant: TargetPlatformVariant.only(TargetPlatform.iOS));
}
