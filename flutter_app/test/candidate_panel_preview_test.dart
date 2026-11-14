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
import 'package:keytao/src/ui/settings.dart';
import 'package:keytao/src/ui/theme.dart';
import 'package:keytao/src/ui/widgets.dart';

import 'view_fixture.dart';

class _PreviewController extends ViewController {
  _PreviewController() : super(BridgePlatform.macOs);

  final savedFonts = <double>[];
  Completer<bool>? saveGate;

  void updateSettings({
    UiColorSchemeDto? colorScheme,
    PanelOrientationDto? orientation,
    String? accentColor,
    double? fontSize,
  }) {
    final current = uiSettings!;
    uiSettings = ImeUiSettingsDto(
      colorScheme: colorScheme ?? current.colorScheme,
      effectiveColorScheme: current.effectiveColorScheme,
      orientation: orientation ?? current.orientation,
      embeddedComposition: current.embeddedComposition,
      accentColor: accentColor ?? current.accentColor,
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
    String? accentColor,
    double? fontSize,
  }) async {
    if (fontSize != null) savedFonts.add(fontSize);
    if (saveGate != null) await saveGate!.future;
    updateSettings(
      colorScheme: colorScheme,
      orientation: orientation,
      accentColor: accentColor,
      fontSize: fontSize,
    );
    return true;
  }
}

final _panel = find.byKey(const ValueKey('candidate-panel-preview'));
final _candidates = find.byKey(const ValueKey('candidate-preview-candidates'));
final _firstText = find.byKey(const ValueKey('candidate-preview-text-0'));

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
      platform: BridgePlatform.macOs,
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
      expect(tester.widget<Flex>(_candidates).direction, Axis.horizontal);
      expect(controller.uiSettings!.orientation, PanelOrientationDto.vertical);
      controller.saveGate!.complete(true);
      await tester.pumpAndSettle();
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
      expect(controller.previewAccent, '#0F9F8F');
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

  testWidgets('automatic preview colors follow system brightness', (
    tester,
  ) async {
    tester.platformDispatcher.platformBrightnessTestValue = Brightness.light;
    addTearDown(tester.platformDispatcher.clearPlatformBrightnessTestValue);
    final controller = _PreviewController();
    await _pumpSettings(tester, controller);
    final light = _panelColor(tester);
    tester.platformDispatcher.platformBrightnessTestValue = Brightness.dark;
    await tester.pumpAndSettle();
    expect(_panelColor(tester), isNot(light));
    controller.updateSettings(colorScheme: UiColorSchemeDto.light);
    await tester.pumpAndSettle();
    expect(_panelColor(tester), light);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

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
        expect(rect.width, 160);
      } else {
        await _capture(tester, sample.name);
      }
      await tester.pumpWidget(const SizedBox.shrink());
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));
  }
}
