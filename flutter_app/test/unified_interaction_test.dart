import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/app/app.dart';
import 'package:keytao/src/app/options.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/rust/frb_generated.dart';
import 'package:keytao/src/settings/android_settings.dart';
import 'package:keytao/src/ui/color_picker.dart';
import 'package:keytao/src/ui/theme.dart';
import 'package:keytao/src/ui/theme_library.dart';
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
    bool? hideCandidatePanel,
    String? accentColor,
    double? cornerRadius,
    bool resetCornerRadius = false,
    String? backgroundColor,
    double? fontSize,
  }) async {
    savedAccent = accentColor;
    return true;
  }

  @override
  Future<bool> saveAndroid(Map<String, Object> patch) async {
    keyboardPatches.add(patch);
    androidSettings = patchAndroidSettings(androidSettings!, patch);
    notifyListeners();
    return true;
  }
}

class _ThemePresetApi implements RustLibApi {
  String current = 'default';
  final motionChanges = <bool>[];

  @override
  Future<void> crateApiCoreSetReduceMotion({required bool enabled}) async {
    motionChanges.add(enabled);
  }

  @override
  Future<List<ThemeSummaryDto>> crateApiCoreGetThemeLibrary() async => [
    for (final (id, name) in [('default', '默认'), ('mint', '薄荷')])
      ThemeSummaryDto(
        id: id,
        name: name,
        builtIn: true,
        current: id == current,
        light: const ThemePreviewColorsDto(
          background: '#FFFFFFFF',
          foreground: '#101010FF',
          accent: '#178E73FF',
        ),
        dark: const ThemePreviewColorsDto(
          background: '#101010FF',
          foreground: '#FFFFFFFF',
          accent: '#178E73FF',
        ),
      ),
  ];

  @override
  Future<void> crateApiCoreApplyTheme({required String id}) async {
    current = id;
  }

  @override
  Future<ImeUiSettingsDto> crateApiCoreGetImeUiSettings() async =>
      const ImeUiSettingsDto(
        colorScheme: UiColorSchemeDto.dark,
        effectiveColorScheme: EffectiveColorSchemeDto.dark,
        orientation: PanelOrientationDto.horizontal,
        embeddedComposition: false,
        hideCandidatePanel: false,
        showTrayModeIndicator: true,
        effectiveEmbeddedComposition: false,
        accentColor: '#178E73',
        fontSize: 18,
        backgroundColor: '',
        themeExists: true,
        message: '',
      );

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
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
  testWidgets(
    'home motion setting keeps the navigator and honors system preference',
    (tester) async {
      final api = _ThemePresetApi();
      RustLib.initMock(api: api);
      addTearDown(RustLib.dispose);
      final c = ViewController(BridgePlatform.ios);
      await _pump(tester, c, const Size(390, 844));
      final navigator = find.byType(Navigator);
      final original = tester.state<NavigatorState>(navigator);
      bool effective() =>
          MediaQuery.disableAnimationsOf(tester.element(navigator));
      final setting = find.ancestor(
        of: find.text('减少动画'),
        matching: find.byType(SwitchSetting),
      );
      final toggle = find.descendant(
        of: setting,
        matching: find.byType(FSwitch),
      );

      expect(find.text('显示网站'), findsOneWidget);
      expect(c.reduceMotion, isFalse);
      expect(effective(), isFalse);
      await tester.ensureVisible(toggle);
      await tester.tap(toggle);
      await tester.pumpAndSettle();
      expect(c.reduceMotion, isTrue);
      expect(effective(), isTrue);
      expect(tester.state<NavigatorState>(navigator), same(original));

      final dispatcher = tester.binding.platformDispatcher;
      dispatcher.accessibilityFeaturesTestValue =
          const FakeAccessibilityFeatures(disableAnimations: true);
      addTearDown(dispatcher.clearAccessibilityFeaturesTestValue);
      await tester.pumpAndSettle();
      await tester.tap(toggle);
      await tester.pumpAndSettle();
      expect(c.reduceMotion, isFalse);
      expect(effective(), isTrue);
      expect(api.motionChanges, [true, false]);
      expect(tester.state<NavigatorState>(navigator), same(original));
      await tester.pumpWidget(const SizedBox.shrink());
    },
    variant: TargetPlatformVariant.only(TargetPlatform.iOS),
  );

  testWidgets('applying a keyboard theme on the mobile home keeps app colors', (
    tester,
  ) async {
    final api = _ThemePresetApi();
    RustLib.initMock(api: api);
    addTearDown(RustLib.dispose);
    final controller = ViewController(BridgePlatform.ios);
    await _pump(tester, controller, const Size(390, 844));
    final library = find.byType(ThemeLibrarySection);
    final mint = find.descendant(of: library, matching: find.text('薄荷'));
    await tester.ensureVisible(mint);
    await tester.tap(mint);
    await tester.pumpAndSettle();
    expect(api.current, 'mint');
    expect(controller.uiSettings?.accentColor, '#178E73');
    final context = tester.element(library);
    expect(ChromeAccent.of(context), ChromeAccent.defaultColor);
    expect(Theme.of(context).brightness, Brightness.light);
    await tester.pumpWidget(const SizedBox.shrink());
  }, variant: TargetPlatformVariant.only(TargetPlatform.iOS));

  for (final size in [const Size(400, 860), const Size(1000, 760)]) {
    testWidgets('changelog and operation log dialogs at $size', (tester) async {
      final c = ViewController(BridgePlatform.macOs)
        ..activePage = AppPage.scheme;
      await _pump(tester, c, size);
      final more = find.descendant(
        of: find.byKey(const ValueKey('scheme-package-keytao')),
        matching: find.byIcon(FLucideIcons.ellipsis),
      );
      await tester.ensureVisible(more);
      await tester.tap(more);
      await tester.pumpAndSettle();
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
    final picker = find.descendant(
      of: find.byType(AccentPicker),
      matching: find.byIcon(FLucideIcons.pipette),
    );
    await tester.ensureVisible(picker);
    await tester.tap(picker);
    await tester.pumpAndSettle();
    final field = find.descendant(
      of: find.byType(FDialog),
      matching: find.byType(EditableText),
    );
    await tester.enterText(field, '#GGGGGG');
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

  testWidgets('mobile phrases editor adds removes and reorders before saving', (
    tester,
  ) async {
    final c = _EditingController(BridgePlatform.android);
    await _pump(tester, c, const Size(400, 860));
    final row = find.widgetWithText(SettingRow, AppStrings.phrases);
    await tester.ensureVisible(row);
    await tester.pumpAndSettle();
    await tester.tap(find.descendant(of: row, matching: find.text('编辑')));
    await tester.pumpAndSettle();
    final field = find.descendant(
      of: find.byType(FDialog),
      matching: find.byType(EditableText),
    );
    await tester.enterText(field, '自定义');
    await tester.tap(find.text('添加'));
    await tester.pumpAndSettle();
    tester
        .widget<ReorderableListView>(find.byType(ReorderableListView))
        .onReorderItem!(5, 0);
    await tester.pumpAndSettle();
    await tester.tap(find.bySemanticsLabel('移除短语').first);
    await tester.pumpAndSettle();
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();
    expect(c.keyboardPatches.single['phrases'], [
      '谢谢',
      '好的',
      '收到',
      '稍等',
      '我在路上',
    ]);
    expect(find.byType(FDialog), findsNothing);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  }, variant: TargetPlatformVariant.only(TargetPlatform.android));

  testWidgets('clipboard controls follow history and suggestion switches', (
    tester,
  ) async {
    final c = _EditingController(BridgePlatform.android);
    await _pump(tester, c, const Size(400, 860));
    SwitchSetting toggle(String label) =>
        tester.widget<SwitchSetting>(find.widgetWithText(SwitchSetting, label));
    SliderSetting slider(String label) =>
        tester.widget<SliderSetting>(find.widgetWithText(SliderSetting, label));
    for (final label in [
      AppStrings.clipboardHistoryEnabled,
      AppStrings.clipboardSuggestionEnabled,
      AppStrings.clipboardReturnAfterPaste,
      AppStrings.clipboardHideSensitive,
    ]) {
      expect(toggle(label).onChanged, isNotNull);
      expect(toggle(label).hint, isNull);
    }
    expect(slider(AppStrings.clipboardHistoryLimit).onChanged, isNotNull);
    expect(
      slider(AppStrings.clipboardSuggestionTimeoutSec).onChanged,
      isNotNull,
    );

    toggle(AppStrings.clipboardHistoryEnabled).onChanged!(false);
    await tester.pumpAndSettle();
    expect(toggle(AppStrings.clipboardHistoryEnabled).onChanged, isNotNull);
    expect(toggle(AppStrings.clipboardSuggestionEnabled).onChanged, isNull);
    expect(toggle(AppStrings.clipboardReturnAfterPaste).onChanged, isNull);
    expect(toggle(AppStrings.clipboardHideSensitive).onChanged, isNull);
    expect(slider(AppStrings.clipboardHistoryLimit).onChanged, isNull);
    expect(slider(AppStrings.clipboardSuggestionTimeoutSec).onChanged, isNull);
    expect(c.androidSettings!.clipboardHideSensitive, isTrue);

    toggle(AppStrings.clipboardHistoryEnabled).onChanged!(true);
    await tester.pumpAndSettle();
    toggle(AppStrings.clipboardSuggestionEnabled).onChanged!(false);
    await tester.pumpAndSettle();
    expect(slider(AppStrings.clipboardSuggestionTimeoutSec).onChanged, isNull);
    expect(slider(AppStrings.clipboardHistoryLimit).onChanged, isNotNull);
    expect(toggle(AppStrings.clipboardReturnAfterPaste).onChanged, isNotNull);
    expect(toggle(AppStrings.clipboardHideSensitive).onChanged, isNotNull);
    expect(c.androidSettings!.clipboardSuggestionTimeoutSec, 30);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  }, variant: TargetPlatformVariant.only(TargetPlatform.android));
}
