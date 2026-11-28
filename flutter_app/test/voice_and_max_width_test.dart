import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/app/options.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/ui/input_page.dart';
import 'package:keytao/src/ui/theme.dart';
import 'package:keytao/src/ui/widgets.dart';

import 'view_fixture.dart';

class _SettingsFeatureController extends ViewController {
  _SettingsFeatureController(super.platform);

  final calls = <String>[];
  final savedMaxWidths = <double?>[];

  @override
  Future<bool> saveCandidatePanelMaxWidth(double? value) async {
    savedMaxWidths.add(value);
    final current = uiSettings!;
    uiSettings = ImeUiSettingsDto(
      colorScheme: current.colorScheme,
      effectiveColorScheme: current.effectiveColorScheme,
      orientation: current.orientation,
      embeddedComposition: current.embeddedComposition,
      hideCandidatePanel: current.hideCandidatePanel,
      showTrayModeIndicator: current.showTrayModeIndicator,
      candidatePanelMaxWidth: value,
      effectiveEmbeddedComposition: current.effectiveEmbeddedComposition,
      accentColor: current.accentColor,
      cornerRadius: current.cornerRadius,
      backgroundColor: current.backgroundColor,
      fontSize: current.fontSize,
      themeExists: current.themeExists,
      themePath: current.themePath,
      message: current.message,
    );
    notifyListeners();
    return true;
  }

  @override
  Future<bool> setVoiceInputEnabled(bool enabled) async {
    calls.add('enabled:$enabled');
    voiceInput = {...?voiceInput, 'enabled': enabled};
    notifyListeners();
    return true;
  }

  @override
  Future<bool> requestVoicePermission() async {
    calls.add('requestPermission');
    return true;
  }

  @override
  Future<bool> openVoiceInputSettings() async {
    calls.add('openVoiceInputSettings');
    return true;
  }

  @override
  Future<bool> openVoiceModelSettings() async {
    calls.add('openVoiceModelSettings');
    return true;
  }

  @override
  Future<bool> refreshVoiceInputStatus() async {
    calls.add('refreshVoiceInputStatus');
    return true;
  }
}

Future<void> _pumpInputPage(
  WidgetTester tester,
  ViewController controller,
) async {
  tester.view.devicePixelRatio = 1;
  tester.view.physicalSize = const Size(720, 1200);
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  addTearDown(controller.dispose);
  await tester.pumpWidget(
    KeyTaoTheme(
      platform: controller.info.platform,
      home: FScaffold(
        child: ListenableBuilder(
          listenable: controller,
          builder: (_, _) => InputPage(controller: controller),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

void main() {
  testWidgets(
    'desktop shows candidate panel max width setting and supports reset to auto',
    (tester) async {
      final controller = _SettingsFeatureController(BridgePlatform.macOs);
      await _pumpInputPage(tester, controller);

      final sliderFinder = find.byKey(
        const ValueKey('pc-candidate-panel-max-width'),
      );
      expect(sliderFinder, findsOneWidget);
      expect(find.text(AppStrings.candidatePanelMaxWidth), findsOneWidget);
      expect(find.text(AppStrings.candidatePanelMaxWidthAuto), findsOneWidget);

      await controller.saveCandidatePanelMaxWidth(560);
      await tester.pumpAndSettle();
      expect(find.text('560px'), findsOneWidget);

      final reset = find.bySemanticsLabel('候选条最大宽度：恢复自动');
      expect(reset, findsOneWidget);
      await tester.ensureVisible(reset);
      await tester.tap(reset);
      await tester.pumpAndSettle();

      expect(controller.savedMaxWidths, [560.0, null]);
      expect(controller.uiSettings!.candidatePanelMaxWidth, isNull);
      expect(find.text(AppStrings.candidatePanelMaxWidthAuto), findsOneWidget);
      await tester.pumpWidget(const SizedBox.shrink());
    },
    variant: TargetPlatformVariant.only(TargetPlatform.macOS),
  );

  testWidgets(
    'android voice input section shows engine and permission guidance actions',
    (tester) async {
      final controller = _SettingsFeatureController(BridgePlatform.android)
        ..voiceInput = {
          'enabled': true,
          'ready': false,
          'recognitionAvailable': true,
          'engineInstalled': true,
          'engineSelected': true,
          'usesSystemDefault': false,
          'selectionInvalid': false,
          'selectedEngineName': 'Google Speech',
          'hasModelSettings': true,
          'permissionGranted': false,
          'permissionPermanentlyDenied': false,
        };
      await _pumpInputPage(tester, controller);

      expect(find.text(AppStrings.voiceInput), findsOneWidget);
      expect(find.text('Google Speech'), findsOneWidget);
      expect(find.text(AppStrings.voicePermissionMissing), findsOneWidget);

      final permissionButton = find.widgetWithText(
        ActionButton,
        AppStrings.voiceRequestPermission,
      );
      await tester.ensureVisible(permissionButton);
      await tester.tap(permissionButton);
      await tester.pumpAndSettle();

      final engineButton = find.widgetWithText(
        ActionButton,
        AppStrings.voiceChooseEngine,
      );
      await tester.ensureVisible(engineButton);
      await tester.tap(engineButton);
      await tester.pumpAndSettle();

      final modelButton = find.widgetWithText(
        ActionButton,
        AppStrings.voiceChooseModel,
      );
      await tester.ensureVisible(modelButton);
      await tester.tap(modelButton);
      await tester.pumpAndSettle();

      expect(controller.calls, [
        'requestPermission',
        'openVoiceInputSettings',
        'openVoiceModelSettings',
      ]);
      await tester.pumpWidget(const SizedBox.shrink());
    },
    variant: TargetPlatformVariant.only(TargetPlatform.android),
  );

  testWidgets(
    'ios shows voice input guidance without android permission controls',
    (tester) async {
      final controller = _SettingsFeatureController(BridgePlatform.ios);
      await _pumpInputPage(tester, controller);

      expect(find.text(AppStrings.voiceInput), findsOneWidget);
      expect(find.text(AppStrings.voiceIosHint), findsOneWidget);
      expect(find.text(AppStrings.voiceRequestPermission), findsNothing);
      await tester.pumpWidget(const SizedBox.shrink());
    },
    variant: TargetPlatformVariant.only(TargetPlatform.iOS),
  );
}
