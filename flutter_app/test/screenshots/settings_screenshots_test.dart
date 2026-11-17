import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/app/options.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/ui/shell.dart';
import 'package:keytao/src/ui/theme.dart';
import 'package:keytao/src/ui/widgets.dart';

import '../view_fixture.dart';

const _directory =
    '/private/tmp/claude-501/-Users-rea-code-keytao-org-keytao-app/'
    '348229fe-6d80-4d02-b020-e4d903e04588/scratchpad/redesign-shots';
const _cjkFamily = 'SettingsScreenshotCjk';
const _captureKey = ValueKey('settings-screenshot');
const _schemaLogoPath = '$_directory/schema-logo.png';

class _ScreenshotController extends ViewController {
  _ScreenshotController(super.platform) {
    activeSchemaLogoPath = _schemaLogoPath;
    candidatePageSize = 6;
    appUpdate = null;
    installedPackages = [
      InstalledPackageDto(
        schemeKey: 'keytao',
        version: local!.version,
        active: true,
        hasPackageZip: true,
      ),
    ];
  }

  @override
  Map<String, dynamic> resolveCandidatePanelPreview({
    required UiColorSchemeDto colorScheme,
    required String accentColor,
    required double fontSize,
    required bool systemDark,
  }) {
    final theme = super.resolveCandidatePanelPreview(
      colorScheme: colorScheme,
      accentColor: accentColor,
      fontSize: fontSize,
      systemDark: systemDark,
    );
    (theme['font'] as Map<String, dynamic>)['family'] = _cjkFamily;
    return theme;
  }
}

Future<void> _loadFonts() async {
  final manifest =
      jsonDecode(await rootBundle.loadString('FontManifest.json')) as List;
  for (final entry in manifest) {
    final loader = FontLoader(entry['family'] as String);
    for (final font in entry['fonts'] as List) {
      loader.addFont(rootBundle.load(font['asset'] as String));
    }
    await loader.load();
  }
  for (final path in [
    '/System/Library/Fonts/PingFang.ttc',
    '/System/Library/Fonts/STHeiti Medium.ttc',
    '/System/Library/Fonts/Supplemental/Songti.ttc',
  ]) {
    final file = File(path);
    if (!file.existsSync()) continue;
    try {
      await (FontLoader(
        _cjkFamily,
      )..addFont(file.readAsBytes().then(ByteData.sublistView))).load();
      return;
    } on Object catch (_) {
      // Try the next system font when this font collection cannot be loaded.
    }
  }
  throw StateError('No macOS CJK font could be loaded for screenshots.');
}

Future<void> _writeSchemaLogo() async {
  final recorder = ui.PictureRecorder();
  final canvas = Canvas(recorder);
  final text = TextPainter(
    text: const TextSpan(
      text: '键道',
      style: TextStyle(
        fontFamily: _cjkFamily,
        fontSize: 82,
        fontWeight: FontWeight.w700,
        color: Color(0xFF3B73D9),
      ),
    ),
    textDirection: TextDirection.ltr,
  )..layout();
  text.paint(canvas, Offset((192 - text.width) / 2, (192 - text.height) / 2));
  text.dispose();
  final picture = recorder.endRecording();
  final image = await picture.toImage(192, 192);
  try {
    final png = (await image.toByteData(format: ui.ImageByteFormat.png))!;
    await Directory(_directory).create(recursive: true);
    await File(_schemaLogoPath).writeAsBytes(png.buffer.asUint8List());
  } finally {
    image.dispose();
    picture.dispose();
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  final enabled = Platform.environment['SCREENSHOTS'] == '1';
  setUpAll(() async {
    if (enabled) {
      await _loadFonts();
      await _writeSchemaLogo();
    }
  });

  for (final device in [
    (
      name: 'desktop',
      size: const Size(1440, 1220),
      platform: BridgePlatform.macOs,
    ),
    (
      name: 'phone',
      size: const Size(390, 844),
      platform: BridgePlatform.android,
    ),
  ]) {
    for (final page in [AppPage.input, AppPage.scheme]) {
      for (final brightness in Brightness.values) {
        final name = '${device.name}-${page.id}-${brightness.name}';
        testWidgets('capture settings $name', (tester) async {
          final controller = _ScreenshotController(device.platform)
            ..activePage = page;
          addTearDown(controller.dispose);
          tester.view.devicePixelRatio = 1;
          tester.view.physicalSize = device.size;
          tester.platformDispatcher.platformBrightnessTestValue = brightness;
          addTearDown(tester.view.resetPhysicalSize);
          addTearDown(tester.view.resetDevicePixelRatio);
          addTearDown(
            tester.platformDispatcher.clearPlatformBrightnessTestValue,
          );

          await tester.pumpWidget(const SizedBox(key: _captureKey));
          await tester.runAsync(
            () => precacheImage(
              FileImage(File(_schemaLogoPath)),
              tester.element(find.byKey(_captureKey)),
              onError: (error, stack) =>
                  Error.throwWithStackTrace(error, stack!),
            ),
          );
          await tester.pumpWidget(
            RepaintBoundary(
              key: _captureKey,
              child: KeyTaoTheme(
                platform: device.platform,
                home: Builder(
                  builder: (context) {
                    final theme = context.theme;
                    final typeface = FTypeface.inherit(
                      colors: theme.colors,
                      touch: context.platformVariant.touch,
                      fontFamilyFallback: const [_cjkFamily],
                    );
                    return FTheme(
                      platform: context.platformVariant,
                      data: FThemeData(
                        colors: theme.colors,
                        touch: context.platformVariant.touch,
                        typography: FTypography(
                          display: typeface,
                          body: typeface,
                        ),
                      ),
                      child: AppShell(controller: controller),
                    );
                  },
                ),
              ),
            ),
          );
          await tester.pumpAndSettle();
          final logo = find.byType(SchemaLogo);
          expect(logo, findsOneWidget);
          final logoImage = tester.widget<Image>(
            find.descendant(of: logo, matching: find.byType(Image)),
          );
          expect(logoImage.image, isA<FileImage>());
          final renderedLogo = tester.widget<RawImage>(
            find.descendant(of: logo, matching: find.byType(RawImage)),
          );
          expect(renderedLogo.image?.width, 192);
          expect(renderedLogo.image?.height, 192);
          expect(tester.getSize(find.byKey(_captureKey)), device.size);

          if (device.name == 'phone') {
            final scrollable = tester.state<ScrollableState>(
              find
                  .descendant(
                    of: find.byKey(PageStorageKey(page.id)),
                    matching: find.byType(Scrollable),
                  )
                  .first,
            );
            // Keep the phone width while exposing every setting below the fold.
            tester.view.physicalSize = Size(
              device.size.width,
              device.size.height + scrollable.position.maxScrollExtent.ceil(),
            );
            await tester.pumpAndSettle();
            expect(scrollable.position.maxScrollExtent, lessThanOrEqualTo(1));
          }
          expect(tester.takeException(), isNull);

          final boundary = tester.renderObject<RenderRepaintBoundary>(
            find.byKey(_captureKey),
          );
          await tester.runAsync(() async {
            final image = await boundary.toImage();
            try {
              final png = await image.toByteData(
                format: ui.ImageByteFormat.png,
              );
              expect(png, isNotNull);
              expect(png!.lengthInBytes, greaterThan(10000));
              await Directory(_directory).create(recursive: true);
              await File('$_directory/$name.png').writeAsBytes(
                png.buffer.asUint8List(png.offsetInBytes, png.lengthInBytes),
              );
            } finally {
              image.dispose();
            }
          });
          await tester.pumpWidget(const SizedBox.shrink());
        }, skip: !enabled);
      }
    }
  }
}
