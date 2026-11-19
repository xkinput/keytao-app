import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/app/options.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/ui/candidate_panel_preview.dart';
import 'package:keytao/src/ui/color_picker.dart';
import 'package:keytao/src/ui/settings.dart';
import 'package:keytao/src/ui/theme.dart';
import 'package:keytao/src/ui/widgets.dart';

import 'view_fixture.dart';

class _PanelController extends ViewController {
  _PanelController(super.platform);

  final savedValues = <bool>[];
  final savedOrientations = <PanelOrientationDto>[];

  @override
  Future<bool> saveUi({
    UiColorSchemeDto? colorScheme,
    PanelOrientationDto? orientation,
    bool? hideCandidatePanel,
    String? accentColor,
    double? cornerRadius,
    bool resetCornerRadius = false,
    String? backgroundColor,
    double? fontSize,
  }) async {
    if (orientation != null) savedOrientations.add(orientation);
    _updateSettings(
      hide: hideCandidatePanel ?? uiSettings!.hideCandidatePanel,
      orientation: orientation,
    );
    return true;
  }

  @override
  Future<bool> saveHideCandidatePanel(bool value) async {
    savedValues.add(value);
    _updateSettings(hide: value);
    return true;
  }

  void _updateSettings({required bool hide, PanelOrientationDto? orientation}) {
    final current = uiSettings!;
    uiSettings = ImeUiSettingsDto(
      colorScheme: current.colorScheme,
      effectiveColorScheme: current.effectiveColorScheme,
      orientation: orientation ?? current.orientation,
      embeddedComposition: current.embeddedComposition,
      hideCandidatePanel: hide,
      showTrayModeIndicator: current.showTrayModeIndicator,
      effectiveEmbeddedComposition: hide || current.embeddedComposition,
      accentColor: current.accentColor,
      cornerRadius: current.cornerRadius,
      backgroundColor: current.backgroundColor,
      fontSize: current.fontSize,
      themeExists: current.themeExists,
      themePath: current.themePath,
      message: current.message,
    );
    notifyListeners();
  }
}

Future<void> _pumpSettings(
  WidgetTester tester,
  ViewController controller,
) async {
  tester.view.devicePixelRatio = 1;
  tester.view.physicalSize = const Size(720, 1000);
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  addTearDown(controller.dispose);
  await tester.pumpWidget(
    KeyTaoTheme(
      platform: controller.info.platform,
      home: FScaffold(
        child: SingleChildScrollView(
          child: ListenableBuilder(
            listenable: controller,
            builder: (_, _) => SettingsCard(controller: controller),
          ),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

void main() {
  for (final entry in {
    BridgePlatform.macOs: TargetPlatform.macOS,
    BridgePlatform.windows: TargetPlatform.windows,
    BridgePlatform.linux: TargetPlatform.linux,
  }.entries) {
    testWidgets(
      '${entry.key.name} hidden panel keeps theme controls and embedded preference',
      (tester) async {
        final controller = _PanelController(entry.key)
          .._updateSettings(
            hide: false,
            orientation: PanelOrientationDto.vertical,
          );
        await _pumpSettings(tester, controller);
        final mode = find.byWidgetPredicate(
          (widget) => widget is ChoiceSetting<String> && widget.label == '方向',
        );
        final embedded = find.widgetWithText(
          SwitchSetting,
          AppStrings.embedded,
        );
        final font = find.widgetWithText(
          SliderSetting,
          AppStrings.candidateFontSize,
        );
        final hidden = find.widgetWithText(SwitchSetting, '隐藏候选面板');
        final hideSwitch = find.descendant(
          of: hidden,
          matching: find.byType(FSwitch),
        );

        if (entry.key == BridgePlatform.linux) {
          expect(embedded, findsNothing);
        } else {
          expect(tester.widget<SwitchSetting>(embedded).value, isFalse);
          expect(tester.widget<SwitchSetting>(embedded).onChanged, isNotNull);
        }
        expect(font, findsOneWidget);
        expect(find.byType(AccentPicker), findsOneWidget);
        expect(find.byType(CandidatePanelPreview), findsOneWidget);
        expect(mode, findsOneWidget);
        expect(tester.widget<ChoiceSetting<String>>(mode).value, 'vertical');
        expect(find.byType(ChoiceSetting<UiColorSchemeDto>), findsOneWidget);

        await tester.ensureVisible(hideSwitch);
        await tester.tap(hideSwitch);
        await tester.pumpAndSettle();

        expect(controller.uiSettings!.hideCandidatePanel, isTrue);
        expect(
          controller.uiSettings!.orientation,
          PanelOrientationDto.vertical,
        );
        expect(controller.savedOrientations, isEmpty);
        expect(controller.uiSettings!.embeddedComposition, isFalse);
        if (entry.key == BridgePlatform.linux) {
          expect(embedded, findsNothing);
        } else {
          expect(tester.widget<SwitchSetting>(embedded).value, isTrue);
          expect(tester.widget<SwitchSetting>(embedded).onChanged, isNull);
        }
        expect(font, findsOneWidget);
        expect(find.byType(AccentPicker), findsOneWidget);
        expect(find.byType(CandidatePanelPreview), findsOneWidget);
        expect(mode, findsOneWidget);
        expect(tester.widget<ChoiceSetting<String>>(mode).value, 'vertical');
        expect(find.byType(ChoiceSetting<UiColorSchemeDto>), findsOneWidget);
        expect(find.text(AppStrings.englishMode), findsOneWidget);

        await tester.ensureVisible(hideSwitch);
        await tester.tap(hideSwitch);
        await tester.pumpAndSettle();

        expect(controller.savedValues, [true, false]);
        expect(controller.savedOrientations, isEmpty);
        expect(controller.uiSettings!.hideCandidatePanel, isFalse);
        expect(
          controller.uiSettings!.orientation,
          PanelOrientationDto.vertical,
        );
        expect(controller.uiSettings!.embeddedComposition, isFalse);
        if (entry.key == BridgePlatform.linux) {
          expect(embedded, findsNothing);
        } else {
          expect(tester.widget<SwitchSetting>(embedded).value, isFalse);
          expect(tester.widget<SwitchSetting>(embedded).onChanged, isNotNull);
        }
        expect(font, findsOneWidget);
        expect(find.byType(AccentPicker), findsOneWidget);
        expect(find.byType(CandidatePanelPreview), findsOneWidget);
        expect(mode, findsOneWidget);
        expect(tester.widget<ChoiceSetting<String>>(mode).value, 'vertical');
        expect(tester.takeException(), isNull);
        await tester.pumpWidget(const SizedBox.shrink());
      },
      variant: TargetPlatformVariant.only(entry.value),
    );
  }

  for (final entry in {
    BridgePlatform.android: TargetPlatform.android,
    BridgePlatform.ios: TargetPlatform.iOS,
  }.entries) {
    testWidgets('${entry.key.name} omits no candidate panel mode', (
      tester,
    ) async {
      await _pumpSettings(tester, ViewController(entry.key));
      expect(find.text('候选模式'), findsNothing);
      expect(find.text(AppStrings.embedded), findsNothing);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    }, variant: TargetPlatformVariant.only(entry.value));
  }
}
