import 'dart:async';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
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

class _PreviewController extends ViewController {
  _PreviewController([super.platform = BridgePlatform.macOs]);

  final savedFonts = <double>[];
  Completer<bool>? saveGate;

  void updateSettings({
    UiColorSchemeDto? colorScheme,
    PanelOrientationDto? orientation,
    String? accentColor,
    double? cornerRadius,
    bool resetCornerRadius = false,
    String? backgroundColor,
    double? fontSize,
  }) {
    final current = uiSettings!;
    uiSettings = ImeUiSettingsDto(
      colorScheme: colorScheme ?? current.colorScheme,
      effectiveColorScheme: current.effectiveColorScheme,
      orientation: orientation ?? current.orientation,
      embeddedComposition: current.embeddedComposition,
      hideCandidatePanel: current.hideCandidatePanel,
      showTrayModeIndicator: current.showTrayModeIndicator,
      effectiveEmbeddedComposition: current.effectiveEmbeddedComposition,
      accentColor: accentColor ?? current.accentColor,
      cornerRadius: resetCornerRadius
          ? null
          : cornerRadius ?? current.cornerRadius,
      backgroundColor: backgroundColor ?? current.backgroundColor,
      fontSize: fontSize ?? current.fontSize,
      themeExists: current.themeExists,
      themePath: current.themePath,
      message: current.message,
    );
    notifyListeners();
  }

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
    if (fontSize != null) savedFonts.add(fontSize);
    if (saveGate != null) await saveGate!.future;
    updateSettings(
      colorScheme: colorScheme,
      orientation: orientation,
      accentColor: accentColor,
      cornerRadius: cornerRadius,
      resetCornerRadius: resetCornerRadius,
      backgroundColor: backgroundColor,
      fontSize: fontSize,
    );
    return true;
  }

  @override
  Future<Map<String, dynamic>> updateThemeEditor({
    required String target,
    required String path,
    Object? value,
  }) async {
    if (path == 'pc.font.size' && value is num) {
      savedFonts.add(value.toDouble());
    }
    if (saveGate != null) await saveGate!.future;
    switch (path) {
      case 'common.accentColor':
        updateSettings(accentColor: value as String? ?? '#3B73D9');
      case 'common.backgroundColor':
        updateSettings(backgroundColor: value as String? ?? '');
      case 'common.cornerRadius':
        updateSettings(
          cornerRadius: (value as num?)?.toDouble(),
          resetCornerRadius: value == null,
        );
      case 'pc.font.size':
        updateSettings(fontSize: (value as num?)?.toDouble() ?? 20);
      case 'pc.panel.orientation':
        updateSettings(
          orientation: PanelOrientationDto.values.byName(value as String),
        );
    }
    return getThemeEditor();
  }
}

final _panel = find.byKey(const ValueKey('candidate-panel-preview'));
final _candidates = find.byKey(const ValueKey('candidate-preview-candidates'));
final _firstText = find.byKey(const ValueKey('candidate-preview-text-0'));
Finder _pcText(String text) => find.descendant(
  of: find.byType(CandidatePanelPreview),
  matching: find.text(text),
);

void _narrowScreen(WidgetTester tester) {
  tester.view.devicePixelRatio = 1;
  tester.view.physicalSize = const Size(400, 860);
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
}

Future<void> _pumpSettings(
  WidgetTester tester,
  _PreviewController controller,
) async {
  _narrowScreen(tester);
  addTearDown(controller.dispose);
  await tester.pumpWidget(
    KeyTaoTheme(
      platform: controller.info.platform,
      home: FScaffold(
        child: SingleChildScrollView(
          child: Padding(
            padding: const EdgeInsets.all(16),
            child: ListenableBuilder(
              listenable: controller,
              builder: (_, _) => SettingsCard(controller: controller),
            ),
          ),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

Color? _panelColor(WidgetTester tester) =>
    (tester.widget<Container>(_panel).decoration as BoxDecoration).color;

Future<void> _capture(WidgetTester tester, String name) async {
  const directory = String.fromEnvironment('CANDIDATE_PREVIEW_SCREENSHOT_DIR');
  if (directory.isEmpty) return;
  final boundary = tester.renderObject<RenderRepaintBoundary>(
    find.byKey(const ValueKey('candidate-preview-image')),
  );
  await tester.runAsync(() async {
    final image = await boundary.toImage(pixelRatio: 2);
    try {
      final bytes = await image.toByteData(format: ui.ImageByteFormat.png);
      await Directory(directory).create(recursive: true);
      await File('$directory/$name.png').writeAsBytes(
        bytes!.buffer.asUint8List(bytes.offsetInBytes, bytes.lengthInBytes),
      );
    } finally {
      image.dispose();
    }
  });
}

void main() {
  var screenshotFontAvailable = false;
  setUpAll(() async {
    const directory = String.fromEnvironment(
      'CANDIDATE_PREVIEW_SCREENSHOT_DIR',
    );
    if (directory.isEmpty) return;
    for (final path in [
      '/System/Library/Fonts/Supplemental/Arial Unicode.ttf',
      '/System/Library/Fonts/PingFang.ttc',
      '/System/Library/Fonts/Hiragino Sans GB.ttc',
    ]) {
      final file = File(path);
      if (!file.existsSync()) continue;
      await (FontLoader(
        'CandidatePreviewScreenshot',
      )..addFont(file.readAsBytes().then(ByteData.sublistView))).load();
      screenshotFontAvailable = true;
      break;
    }
  });

  test('accent parsing falls back safely for invalid settings', () {
    expect(parseAccentColor('#ab12ef'), const Color(0xFFAB12EF));
    for (final value in ['', '#', '#GGGGGG', 'invalid']) {
      expect(parseAccentColor(value), const Color(0xFF3B73D9));
    }
  });

  testWidgets('phone preview renders keyboard rows without desktop settings', (
    tester,
  ) async {
    _narrowScreen(tester);
    final controller = ViewController(BridgePlatform.android)
      ..uiSettings = null
      ..candidatePageSize = 3;
    addTearDown(controller.dispose);
    await tester.pumpWidget(
      KeyTaoTheme(
        platform: BridgePlatform.android,
        home: FScaffold(
          child: SingleChildScrollView(
            child: SettingsCard(controller: controller),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    for (final key in 'qwertyuiopasdfghjkl'.split('')) {
      expect(find.byKey(ValueKey('keyboard-preview-key-$key')), findsOneWidget);
    }
    // The preview keeps two letter rows and the edited key types visible.
    for (final key in 'zxcvbnm'.split('')) {
      expect(find.text(key), findsNothing);
    }
    for (final label in ['⇧', '⌫', '，', '键道6', '。']) {
      expect(find.byKey(ValueKey('keyboard-preview-key-$label')), findsNothing);
    }
    for (final label in ['123', '符', '空格', '↵']) {
      expect(
        find.byKey(ValueKey('keyboard-preview-key-$label')),
        findsOneWidget,
      );
    }
    final keyboard = find.byKey(const ValueKey('mobile-keyboard-preview'));
    expect(
      find.descendant(of: keyboard, matching: find.text('1')),
      findsOneWidget,
    );
    expect(
      find.descendant(of: keyboard, matching: find.text('@')),
      findsOneWidget,
    );
    expect(
      find.byKey(const ValueKey('mobile-candidate-bar-preview')),
      findsOneWidget,
    );
    expect(find.bySemanticsLabel('KeyTao 键盘预览，键道6，静态展示'), findsOneWidget);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  }, variant: TargetPlatformVariant.only(TargetPlatform.android));

  for (final orientation in PanelOrientationDto.values) {
    testWidgets('real candidates and page size render in ${orientation.name}', (
      tester,
    ) async {
      final controller = _PreviewController()
        ..candidatePreviewPageSize = 4
        ..candidatePreviewCandidates = const [
          (text: '方案首选', comment: '~real'),
          (text: '第二词', comment: '词典注释'),
          (text: '第三词', comment: ''),
          (text: '第四词', comment: '🙂'),
          (text: '第五词', comment: 'fifth'),
        ]
        ..updateSettings(orientation: orientation);
      await _pumpSettings(tester, controller);
      expect(_pcText('方案首选'), findsOneWidget);
      expect(_pcText('~real'), findsOneWidget);
      expect(_pcText('词典注释'), findsOneWidget);
      expect(_pcText('🙂'), findsOneWidget);
      expect(_pcText('这'), findsNothing);
      expect(
        find.byKey(const ValueKey('candidate-preview-text-3')),
        findsOneWidget,
      );
      expect(
        find.byKey(const ValueKey('candidate-preview-text-4')),
        findsNothing,
      );
      expect(
        tester.widget<Flex>(_candidates).direction,
        orientation == PanelOrientationDto.vertical
            ? Axis.vertical
            : Axis.horizontal,
      );

      controller.candidatePageSize = 2;
      controller.updateSettings();
      await tester.pumpAndSettle();
      expect(_pcText('第二词'), findsOneWidget);
      expect(_pcText('第三词'), findsNothing);
      expect(
        find.byKey(const ValueKey('candidate-preview-text-2')),
        findsNothing,
      );

      controller.candidatePageSize = null;
      controller.candidatePreviewPageSize = 12;
      controller.candidatePreviewCandidates = controller
          .candidatePreviewCandidates
          .take(2)
          .toList();
      controller.updateSettings();
      await tester.pumpAndSettle();
      expect(
        find.byKey(const ValueKey('candidate-preview-text-3')),
        findsOneWidget,
      );
      expect(
        find.byKey(const ValueKey('candidate-preview-text-11')),
        findsOneWidget,
      );
      expect(
        find.byKey(const ValueKey('candidate-preview-text-12')),
        findsNothing,
      );
      expect(
        tester
            .widget<Text>(
              find.byKey(const ValueKey('candidate-preview-text-3')),
            )
            .data,
        '',
      );
      expect(_pcText('支'), findsNothing);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets('candidate count updates live in ${orientation.name}', (
      tester,
    ) async {
      final controller = _PreviewController()
        ..updateSettings(orientation: orientation);
      await _pumpSettings(tester, controller);
      for (final count in <int?>[
        null,
        ...List.generate(10, (i) => i + 1),
        null,
      ]) {
        controller.candidatePageSize = count;
        controller.updateSettings();
        await tester.pumpAndSettle();
        final expected = count ?? 6;
        for (var index = 0; index < 10; index++) {
          expect(
            find.byKey(ValueKey('candidate-preview-text-$index')),
            index < expected ? findsOneWidget : findsNothing,
          );
        }
        expect(find.text('›'), findsOneWidget);
        expect(tester.takeException(), isNull);
      }
      expect(
        [
          for (var index = 0; index < 6; index++)
            tester
                .widget<Text>(
                  find.byKey(ValueKey('candidate-preview-text-$index')),
                )
                .data,
        ],
        ['这', '之', '支', '只能', '服从', '中国'],
      );
      await tester.pumpWidget(const SizedBox.shrink());
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));
  }

  testWidgets(
    'settings preview follows a font drag before saving and changes direction',
    (tester) async {
      final controller = _PreviewController()
        ..updateSettings(
          fontSize: 24,
          orientation: PanelOrientationDto.vertical,
        );
      await _pumpSettings(tester, controller);
      expect(tester.widget<Flex>(_candidates).direction, Axis.vertical);
      expect(tester.widget<Text>(_firstText).style!.fontSize, 24);
      final originalHeight = tester.getSize(_panel).height;

      final fontRow = find.widgetWithText(
        SliderSetting,
        AppStrings.candidateFontSize,
      );
      await tester.ensureVisible(fontRow);
      await tester.pumpAndSettle();
      final slider = find.descendant(
        of: fontRow,
        matching: find.byType(FSlider),
      );
      final thumb = find.descendant(
        of: slider,
        matching: find.byWidgetPredicate(
          (widget) =>
              widget is GestureDetector &&
              widget.onHorizontalDragUpdate != null,
        ),
      );
      await tester.ensureVisible(thumb);
      await tester.pumpAndSettle();
      final rect = tester.getRect(slider);
      final gesture = await tester.startGesture(tester.getCenter(thumb));
      // The first move starts the drag; the next move changes its value.
      await gesture.moveBy(const Offset(-24, 0));
      await tester.pump();
      await gesture.moveTo(Offset(rect.left + rect.width * .1, rect.center.dy));
      await tester.pump();
      final previewFont = tester.widget<Text>(_firstText).style!.fontSize!;
      expect(previewFont, lessThan(24));
      expect(tester.getSize(_panel).height, lessThan(originalHeight));
      expect(controller.savedFonts, isEmpty);
      await gesture.up();
      await tester.pumpAndSettle();
      expect(controller.savedFonts, [previewFont]);

      controller.saveGate = Completer<bool>();
      final horizontal = find.text('横排');
      await tester.ensureVisible(horizontal);
      await tester.tap(horizontal);
      await tester.pump();
      expect(tester.widget<Flex>(_candidates).direction, Axis.vertical);
      expect(controller.uiSettings!.orientation, PanelOrientationDto.vertical);
      controller.saveGate!.complete(true);
      await tester.pumpAndSettle();
      expect(tester.widget<Flex>(_candidates).direction, Axis.horizontal);
      expect(tester.getSize(_panel).width, lessThanOrEqualTo(368));
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
    variant: TargetPlatformVariant.only(TargetPlatform.macOS),
  );

  testWidgets('accent selection updates the preview before saving finishes', (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();
    try {
      final controller = _PreviewController()..saveGate = Completer<bool>();
      await _pumpSettings(tester, controller);
      final green = find.bySemanticsLabel(AppStrings.useColor('#0F9F8F'));
      await tester.ensureVisible(green);
      await tester.tap(green);
      await tester.pump();
      expect(
        tester
            .widget<CandidatePanelPreview>(find.byType(CandidatePanelPreview))
            .theme['ui']['accentColor'],
        {'red': 15, 'green': 159, 'blue': 143, 'alpha': 255},
      );
      expect(controller.uiSettings!.accentColor, '#3B73D9');
      controller.saveGate!.complete(true);
      await tester.pumpAndSettle();
      expect(controller.uiSettings!.accentColor, '#0F9F8F');
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    } finally {
      semantics.dispose();
    }
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets(
    'editing target controls preview colors independently of appearance',
    (tester) async {
      tester.platformDispatcher.platformBrightnessTestValue = Brightness.light;
      addTearDown(tester.platformDispatcher.clearPlatformBrightnessTestValue);
      final controller = _PreviewController();
      await _pumpSettings(tester, controller);
      final light = _panelColor(tester);
      tester.platformDispatcher.platformBrightnessTestValue = Brightness.dark;
      await tester.pumpAndSettle();
      expect(_panelColor(tester), light);
      ChoiceSetting<String> editTarget() =>
          tester.widget<ChoiceSetting<String>>(
            find.byWidgetPredicate(
              (widget) =>
                  widget is ChoiceSetting<String> && widget.label == '编辑目标',
            ),
          );
      editTarget().onChanged!('dark');
      await tester.pumpAndSettle();
      final dark = _panelColor(tester);
      expect(dark, isNot(light));
      controller.updateSettings(colorScheme: UiColorSchemeDto.light);
      await tester.pumpAndSettle();
      expect(_panelColor(tester), dark);
      editTarget().onChanged!('light');
      await tester.pumpAndSettle();
      expect(_panelColor(tester), light);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
    variant: TargetPlatformVariant.only(TargetPlatform.macOS),
  );

  for (final entry in {
    BridgePlatform.macOs: TargetPlatform.macOS,
    BridgePlatform.android: TargetPlatform.android,
  }.entries) {
    testWidgets('${entry.key.name} appearance edits update the live preview', (
      tester,
    ) async {
      final controller = _PreviewController(entry.key);
      await _pumpSettings(tester, controller);
      final panel = entry.key == BridgePlatform.android
          ? find.byKey(const ValueKey('mobile-keyboard-preview'))
          : _panel;
      BoxDecoration decoration(Finder finder) =>
          tester.widget<Container>(finder).decoration! as BoxDecoration;
      final originalColor = decoration(panel).color;
      final radius = tester.widget<SliderSetting>(
        find.widgetWithText(SliderSetting, '圆角'),
      );
      radius.onPreview!(24);
      await tester.pump();
      if (!controller.isMobile) {
        expect(decoration(panel).borderRadius, BorderRadius.circular(24));
      }
      final preview = tester.widget<CandidatePanelPreview>(
        find.byType(CandidatePanelPreview),
      );
      expect(preview.theme['candidate']['cornerRadius'], 16.5);
      if (controller.isMobile) {
        expect(
          decoration(find.byKey(const ValueKey('keyboard-preview-key-q')))
              .borderRadius,
          BorderRadius.circular(15),
        );
      }
      radius.onPreview!(0);
      await tester.pump();
      if (!controller.isMobile) {
        expect(decoration(panel).borderRadius, BorderRadius.zero);
      }
      controller.saveGate = Completer<bool>();
      radius.onChanged!(0);
      await tester.pump();
      expect(controller.uiSettings!.cornerRadius, isNull);
      controller.saveGate!.complete(true);
      await tester.pumpAndSettle();
      expect(controller.uiSettings!.cornerRadius, 0);

      final picker = find.byType(BackgroundPicker);
      final custom = find.descendant(
        of: picker,
        matching: find.byIcon(FLucideIcons.pipette),
      );
      Future<void> editBackground() async {
        await tester.ensureVisible(custom);
        await tester.tap(custom);
        await tester.pumpAndSettle();
        await tester.enterText(
          find.descendant(
            of: find.byType(AppDialog),
            matching: find.byType(EditableText),
          ),
          '#102030',
        );
        await tester.pump();
      }

      await editBackground();
      expect(decoration(panel).color, isNot(originalColor));
      expect(decoration(panel).color!.a, originalColor!.a);
      expect(controller.uiSettings!.backgroundColor, isEmpty);
      if (controller.isMobile) {
        expect(
          decoration(find.byKey(const ValueKey('keyboard-preview-key-q')))
              .color,
          const Color(0xD2FFFFFF),
        );
      }
      await tester.tap(find.text(AppStrings.cancel));
      await tester.pumpAndSettle();
      expect(decoration(panel).color, originalColor);

      await editBackground();
      controller.saveGate = Completer<bool>();
      await tester.tap(find.text(AppStrings.apply));
      await tester.pump(const Duration(milliseconds: 250));
      expect(controller.uiSettings!.backgroundColor, isEmpty);
      expect(decoration(panel).color, isNot(originalColor));
      controller.saveGate!.complete(true);
      await tester.pumpAndSettle();
      expect(controller.uiSettings!.backgroundColor, '#102030');
      if (!controller.isMobile) {
        expect(decoration(panel).borderRadius, BorderRadius.zero);
      }

      final reset = find.descendant(
        of: picker,
        matching: find.byIcon(FLucideIcons.rotateCcw),
      );
      await tester.ensureVisible(reset);
      await tester.tap(reset);
      await tester.pumpAndSettle();
      expect(controller.uiSettings!.backgroundColor, isEmpty);
      expect(decoration(panel).color, originalColor);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    }, variant: TargetPlatformVariant.only(entry.value));
  }

  for (final sample in [
    (
      name: 'dark-vertical-15',
      dark: true,
      vertical: true,
      font: 15.0,
      custom: false,
    ),
    (
      name: 'light-horizontal-20',
      dark: false,
      vertical: false,
      font: 20.0,
      custom: false,
    ),
    (
      name: 'dark-vertical-24',
      dark: true,
      vertical: true,
      font: 24.0,
      custom: false,
    ),
    (
      name: 'custom narrow candidates and inverted panel bounds',
      dark: false,
      vertical: true,
      font: 36.0,
      custom: true,
    ),
  ]) {
    testWidgets('${sample.name} fits a 400px viewport', (tester) async {
      _narrowScreen(tester);
      final controller = ViewController(BridgePlatform.macOs);
      addTearDown(controller.dispose);
      final theme = controller.resolveCandidatePanelPreview(
        colorScheme: sample.dark
            ? UiColorSchemeDto.dark
            : UiColorSchemeDto.light,
        accentColor: '#3B73D9',
        fontSize: sample.font,
        systemDark: sample.dark,
      );
      if (sample.custom) {
        final candidate = theme['candidate'] as Map<String, dynamic>;
        candidate['maxWidth'] = 72;
        candidate['paddingX'] = 50.4;
        final panel = theme['panel'] as Map<String, dynamic>;
        panel['minWidth'] = 480;
        panel['maxWidth'] = 160;
      }
      if (screenshotFontAvailable) {
        (theme['font'] as Map<String, dynamic>)['family'] =
            'CandidatePreviewScreenshot';
      }
      await tester.pumpWidget(
        KeyTaoTheme(
          platform: BridgePlatform.macOs,
          home: FScaffold(
            child: SingleChildScrollView(
              child: Padding(
                padding: const EdgeInsets.all(16),
                child: CandidatePanelPreview(
                  theme: theme,
                  orientation: sample.vertical
                      ? PanelOrientationDto.vertical
                      : PanelOrientationDto.horizontal,
                  showPreedit: true,
                ),
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(tester.widget<Text>(_firstText).style!.fontSize, sample.font);
      expect(
        tester.widget<Flex>(_candidates).direction,
        sample.vertical ? Axis.vertical : Axis.horizontal,
      );
      final rect = tester.getRect(_panel);
      expect(rect.left, greaterThanOrEqualTo(0));
      expect(rect.right, lessThanOrEqualTo(400));
      expect(tester.takeException(), isNull);
      if (sample.custom) {
        expect(rect.width, greaterThan(320));
      } else {
        await _capture(tester, sample.name);
      }
      await tester.pumpWidget(const SizedBox.shrink());
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));
  }
}
