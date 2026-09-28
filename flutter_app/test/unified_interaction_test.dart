import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/app/app.dart';
import 'package:keytao/src/app/options.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/ui/widgets.dart';

import 'view_fixture.dart';

class _EditingController extends ViewController {
  _EditingController(super.platform);
  String? savedAccent;
  final keyboardPatches = <Map<String, Object>>[];
  @override
  Future<bool> saveUi({
    UiColorSchemeDto? colorScheme,
    PanelOrientationDto? orientation,
    String? accentColor,
    double? fontSize,
  }) async {
    savedAccent = accentColor;
    return true;
  }

  @override
  Future<bool> saveAndroid(Map<String, Object> patch) async {
    keyboardPatches.add(patch);
    return true;
  }
}

Future<void> _pump(WidgetTester tester, ViewController c, Size size) async {
  tester.view.devicePixelRatio = 1;
  tester.view.physicalSize = size;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  addTearDown(c.dispose);
  const channel = MethodChannel('ink.rea.keytao/window');
  tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
    channel,
    (_) async => null,
  );
  addTearDown(
    () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      channel,
      null,
    ),
  );
  await tester.pumpWidget(
    KeyTaoApp(
      controller: c,
      initialOnboarding: const OnboardingDto(completed: true),
    ),
  );
  await tester.pumpAndSettle();
}

void main() {
  for (final size in [const Size(400, 860), const Size(1000, 760)]) {
    testWidgets('changelog and operation log dialogs at $size', (tester) async {
      final c = ViewController(BridgePlatform.macOs)
        ..activePage = AppPage.scheme;
      await _pump(tester, c, size);
      await tester.ensureVisible(find.text(AppStrings.changelog));
      await tester.tap(find.text(AppStrings.changelog));
      await tester.pumpAndSettle();
      expect(find.byType(FDialog), findsOneWidget);
      expect(find.text(c.changelogBody!), findsOneWidget);
      await tester.tap(find.text(AppStrings.close));
      await tester.pumpAndSettle();
      final logs = find.text(AppStrings.operationLogCount(c.operationLogCount));
      await tester.ensureVisible(logs);
      await tester.tap(logs);
      await tester.pumpAndSettle();
      expect(find.text(AppStrings.operationLogs), findsOneWidget);
      await tester.tap(find.text(AppStrings.clearOperationLogs));
      await tester.pumpAndSettle();
      expect(c.operationLogs, isEmpty);
      expect(find.text(AppStrings.noLogs), findsOneWidget);
      await tester.tap(find.text(AppStrings.close));
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));
  }

  testWidgets('custom color rejects invalid hex and applies uppercase RGB', (
    tester,
  ) async {
    final c = _EditingController(BridgePlatform.macOs);
    await _pump(tester, c, const Size(720, 520));
    final picker = find.byIcon(FLucideIcons.pipette);
    await tester.ensureVisible(picker);
    await tester.tap(picker);
    await tester.pumpAndSettle();
    final field = find.descendant(
      of: find.byType(FDialog),
      matching: find.byType(EditableText),
    );
    await tester.enterText(field, '#bad');
    await tester.pump();
    expect(
      tester
          .widget<ActionButton>(
            find.widgetWithText(ActionButton, AppStrings.apply),
          )
          .onPress,
      isNull,
    );
    await tester.enterText(field, '#ab12ef');
    await tester.pump();
    await tester.tap(find.text(AppStrings.apply));
    await tester.pumpAndSettle();
    expect(c.savedAccent, '#AB12EF');
    expect(find.byType(FDialog), findsNothing);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets(
    'mobile keyboard height commits a five percent step after dragging',
    (tester) async {
      final c = _EditingController(BridgePlatform.android);
      await _pump(tester, c, const Size(400, 860));
      final row = find.widgetWithText(SliderSetting, AppStrings.keyboardHeight);
      await tester.ensureVisible(row);
      await tester.pumpAndSettle();
      final slider = find.descendant(of: row, matching: find.byType(FSlider));
      final rect = tester.getRect(slider);
      final gesture = await tester.startGesture(
        Offset(rect.left + rect.width / 3, rect.center.dy),
      );
      await gesture.moveTo(
        Offset(rect.left + rect.width * .73, rect.center.dy),
      );
      await tester.pump();
      expect(c.keyboardPatches, isEmpty);
      await gesture.up();
      await tester.pumpAndSettle();
      final value = c.keyboardPatches.single['keyboardHeightScale'] as double;
      expect(value, inInclusiveRange(85, 130));
      expect(value % 5, 0);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
    variant: TargetPlatformVariant.only(TargetPlatform.android),
  );
}
