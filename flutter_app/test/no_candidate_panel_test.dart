import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/app/options.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/ui/candidate_panel_preview.dart';
import 'package:keytao/src/ui/color_picker.dart';
import 'package:keytao/src/ui/input_page.dart';
import 'package:keytao/src/ui/skin_page.dart';
import 'package:keytao/src/ui/theme.dart';
import 'package:keytao/src/ui/widgets.dart';

import 'view_fixture.dart';

class _PanelController extends ViewController {
  _PanelController(super.platform);

  final savedValues = <bool>[];
  final savedOrientations = <PanelOrientationDto>[];
  final savedPageSizes = <int?>[];
  final savedEmbedded = <bool>[];

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

  @override
  Future<bool> saveCandidatePageSize(int? size) async {
    savedPageSizes.add(size);
    candidatePageSize = size;
    notifyListeners();
    return true;
  }

  @override
  Future<bool> saveEmbedded(bool value) async {
    savedEmbedded.add(value);
    _updateSettings(hide: uiSettings!.hideCandidatePanel, embedded: value);
    return true;
  }

  void _updateSettings({
    required bool hide,
    PanelOrientationDto? orientation,
    bool? embedded,
  }) {
    final current = uiSettings!;
    final embeddedComposition = embedded ?? current.embeddedComposition;
    uiSettings = ImeUiSettingsDto(
      colorScheme: current.colorScheme,
      effectiveColorScheme: current.effectiveColorScheme,
      orientation: orientation ?? current.orientation,
      embeddedComposition: embeddedComposition,
      hideCandidatePanel: hide,
      showTrayModeIndicator: current.showTrayModeIndicator,
      effectiveEmbeddedComposition: hide || embeddedComposition,
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
        child: ListenableBuilder(
          listenable: controller,
          builder: (_, _) => controller.isMobile
              ? InputPage(controller: controller)
              : Column(
                  children: [
                    Expanded(child: InputPage(controller: controller)),
                    Expanded(child: SkinPage(controller: controller)),
                  ],
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
      '${entry.key.name} no candidate panel mode restores panel controls',
      (tester) async {
        final controller = _PanelController(entry.key)
          .._updateSettings(
            hide: false,
            orientation: PanelOrientationDto.vertical,
          );
        await _pumpSettings(tester, controller);
        final mode = find.byType(BigTextRadioGroup<PanelOrientationDto?>);
        final embedded = find.widgetWithText(
          SwitchSetting,
          AppStrings.embedded,
        );
        final font = find.widgetWithText(
          SliderSetting,
          AppStrings.candidateFontSize,
        );
        final hidden = find.descendant(of: mode, matching: find.text('隐藏'));
        final editor = find.widgetWithText(FButton, '编辑主题');
        final pageSize = find.widgetWithText(SettingRow, '候选个数');
        final isMacOs = entry.key == BridgePlatform.macOs;
        final candidatePage = find.byType(isMacOs ? SkinPage : InputPage);
        final otherPage = find.byType(isMacOs ? InputPage : SkinPage);

        for (final control in [mode, pageSize, embedded]) {
          expect(
            find.descendant(of: candidatePage, matching: control),
            findsOneWidget,
          );
          expect(
            find.descendant(of: otherPage, matching: control),
            findsNothing,
          );
        }
        expect(
          tester.widget<BigTextRadioGroup<PanelOrientationDto?>>(mode).options,
          const {
            PanelOrientationDto.horizontal: '横排',
            PanelOrientationDto.vertical: '竖排',
            null: '隐藏',
          },
        );
        expect(tester.widget<SwitchSetting>(embedded).value, isFalse);
        expect(tester.widget<SwitchSetting>(embedded).onChanged, isNotNull);
        expect(font, findsOneWidget);
        expect(find.byType(AccentPicker), findsOneWidget);
        expect(
          find.byType(CandidatePanelPreview),
          findsNWidgets(entry.key == BridgePlatform.macOs ? 1 : 2),
        );
        expect(editor, findsOneWidget);
        expect(tester.widget<FButton>(editor).onPress, isNotNull);
        expect(mode, findsOneWidget);
        expect(
          tester.widget<BigTextRadioGroup<PanelOrientationDto?>>(mode).value,
          PanelOrientationDto.vertical,
        );
        expect(
          find.byType(BigTextRadioGroup<UiColorSchemeDto>),
          findsOneWidget,
        );

        await tester.ensureVisible(hidden);
        await tester.tap(hidden);
        await tester.pumpAndSettle();

        expect(controller.uiSettings!.hideCandidatePanel, isTrue);
        expect(
          controller.uiSettings!.orientation,
          PanelOrientationDto.vertical,
        );
        expect(controller.savedOrientations, isEmpty);
        expect(controller.uiSettings!.embeddedComposition, isFalse);
        expect(tester.widget<SwitchSetting>(embedded).value, isTrue);
        expect(tester.widget<SwitchSetting>(embedded).onChanged, isNull);
        expect(font, findsNothing);
        expect(find.byType(AccentPicker), findsNothing);
        expect(find.byType(CandidatePanelPreview), findsNothing);
        expect(editor, findsOneWidget);
        expect(tester.widget<FButton>(editor).onPress, isNotNull);
        expect(mode, findsOneWidget);
        expect(
          tester.widget<BigTextRadioGroup<PanelOrientationDto?>>(mode).value,
          isNull,
        );
        expect(
          find.byType(BigTextRadioGroup<UiColorSchemeDto>),
          findsOneWidget,
        );
        expect(find.text(AppStrings.englishMode), findsOneWidget);

        final horizontal = find.text('横排');
        await tester.ensureVisible(horizontal);
        await tester.tap(horizontal);
        await tester.pumpAndSettle();

        expect(controller.savedValues, [true]);
        expect(controller.savedOrientations, [PanelOrientationDto.horizontal]);
        expect(controller.uiSettings!.hideCandidatePanel, isFalse);
        expect(
          controller.uiSettings!.orientation,
          PanelOrientationDto.horizontal,
        );
        expect(controller.uiSettings!.embeddedComposition, isFalse);
        expect(tester.widget<SwitchSetting>(embedded).value, isFalse);
        expect(tester.widget<SwitchSetting>(embedded).onChanged, isNotNull);
        expect(font, findsOneWidget);
        expect(find.byType(AccentPicker), findsOneWidget);
        expect(
          find.byType(CandidatePanelPreview),
          findsNWidgets(entry.key == BridgePlatform.macOs ? 1 : 2),
        );
        expect(editor, findsOneWidget);
        expect(tester.widget<FButton>(editor).onPress, isNotNull);
        expect(mode, findsOneWidget);
        expect(
          tester.widget<BigTextRadioGroup<PanelOrientationDto?>>(mode).value,
          PanelOrientationDto.horizontal,
        );
        expect(tester.takeException(), isNull);
        await tester.pumpWidget(const SizedBox.shrink());
      },
      variant: TargetPlatformVariant.only(entry.value),
    );
  }

  testWidgets(
    'macOs skin saves candidate count and retains embedded preference when hidden',
    (tester) async {
      final controller = _PanelController(BridgePlatform.macOs);
      await _pumpSettings(tester, controller);
      final skin = find.byType(SkinPage);
      final pageSize = find.descendant(
        of: skin,
        matching: find.byWidgetPredicate((widget) => widget is FSelect<int>),
      );
      final embedded = find.descendant(
        of: skin,
        matching: find.widgetWithText(SwitchSetting, AppStrings.embedded),
      );
      final toggle = find.descendant(
        of: embedded,
        matching: find.byType(FSwitch),
      );

      for (final size in <int?>[10, 1, null]) {
        await tester.ensureVisible(pageSize);
        await tester.tap(pageSize);
        await tester.pumpAndSettle();
        final options = find.byWidgetPredicate(
          (widget) => widget is FSelectItem<int>,
        );
        expect(options, findsNWidgets(11));
        for (final label in ['跟随方案', ...List.generate(10, (i) => '${i + 1}')]) {
          expect(
            find.descendant(of: options, matching: find.text(label)),
            findsOneWidget,
          );
        }
        final option = find.descendant(
          of: options,
          matching: find.text(size?.toString() ?? '跟随方案'),
        );
        await tester.ensureVisible(option);
        await tester.tap(option);
        await tester.pumpAndSettle();
        expect(controller.candidatePageSize, size);
      }
      expect(controller.savedPageSizes, [10, 1, null]);

      await tester.ensureVisible(toggle);
      await tester.tap(toggle);
      await tester.pumpAndSettle();
      expect(controller.savedEmbedded, [true]);
      expect(controller.uiSettings!.embeddedComposition, isTrue);

      final hidden = find.descendant(of: skin, matching: find.text('隐藏'));
      await tester.ensureVisible(hidden);
      await tester.tap(hidden);
      await tester.pumpAndSettle();
      expect(controller.uiSettings!.hideCandidatePanel, isTrue);
      expect(tester.widget<SwitchSetting>(embedded).value, isTrue);
      expect(tester.widget<SwitchSetting>(embedded).onChanged, isNull);
      expect(pageSize, findsOneWidget);
      await tester.ensureVisible(toggle);
      await tester.tap(toggle);
      await tester.pumpAndSettle();
      expect(controller.savedEmbedded, [true]);

      await tester.ensureVisible(find.text('竖排'));
      await tester.tap(find.text('竖排'));
      await tester.pumpAndSettle();
      expect(controller.savedValues, [true]);
      expect(controller.savedOrientations, [PanelOrientationDto.vertical]);
      expect(controller.uiSettings!.hideCandidatePanel, isFalse);
      expect(controller.uiSettings!.embeddedComposition, isTrue);
      expect(tester.widget<SwitchSetting>(embedded).value, isTrue);
      expect(tester.widget<SwitchSetting>(embedded).onChanged, isNotNull);

      await tester.ensureVisible(toggle);
      await tester.tap(toggle);
      await tester.pumpAndSettle();
      expect(controller.savedEmbedded, [true, false]);
      expect(controller.uiSettings!.effectiveEmbeddedComposition, isFalse);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
    variant: TargetPlatformVariant.only(TargetPlatform.macOS),
  );

  for (final entry in {
    BridgePlatform.android: TargetPlatform.android,
    BridgePlatform.ios: TargetPlatform.iOS,
  }.entries) {
    testWidgets('${entry.key.name} omits no candidate panel mode', (
      tester,
    ) async {
      await _pumpSettings(tester, ViewController(entry.key));
      expect(
        find.byType(BigTextRadioGroup<PanelOrientationDto?>),
        findsNothing,
      );
      expect(find.text(AppStrings.embedded), findsNothing);
      expect(
        find.descendant(
          of: find.byType(InputPage),
          matching: find.widgetWithText(SettingRow, '候选个数'),
        ),
        findsOneWidget,
      );
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    }, variant: TargetPlatformVariant.only(entry.value));
  }
}
