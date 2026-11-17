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
import 'package:keytao/src/rust/frb_generated.dart';
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
const _catLogoPath = '$_directory/cat-logo.png';
const _tianLogoPath = '$_directory/tian-logo.png';
const _conflict = UserWordComparisonDto(
  entry: UserWordDto(id: 1, word: '测试词', code: 'csci'),
  status: 'code_taken',
  words: ['测试'],
  codes: [],
);

class _ScreenshotCore implements RustLibApi {
  @override
  dynamic noSuchMethod(Invocation invocation) =>
      switch (invocation.memberName) {
        #crateApiCoreListUserWords => Future.value([_conflict.entry]),
        #crateApiCoreCompareUserWords => Future.value([_conflict]),
        _ => throw StateError(
          'Unexpected screenshot bridge call: ${invocation.memberName}',
        ),
      };
}

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
        logoPath: _schemaLogoPath,
      ),
      const InstalledPackageDto(
        schemeKey: 'xmjd',
        active: false,
        hasPackageZip: true,
        logoPath: _catLogoPath,
      ),
      const InstalledPackageDto(
        schemeKey: 'txjx',
        active: false,
        hasPackageZip: true,
        logoPath: _tianLogoPath,
      ),
    ];
    schemaSwitches = [
      for (final (name, off, on) in [
        ('ascii_mode', '中文', '西文'),
        ('full_shape', '半角', '全角'),
        ('traditional', '简体', '繁體'),
      ])
        SchemaSwitchDto(
          schemaId: 'keytao',
          name: name,
          states: [off, on],
          options: const [],
          value: 0,
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

Future<void> _writeSchemaLogo(String path, String label, Color color) async {
  final recorder = ui.PictureRecorder();
  final canvas = Canvas(recorder);
  final text = TextPainter(
    text: TextSpan(
      text: label,
      style: TextStyle(
        fontFamily: _cjkFamily,
        fontSize: 82,
        fontWeight: FontWeight.w700,
        color: color,
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
    await File(path).writeAsBytes(png.buffer.asUint8List());
  } finally {
    image.dispose();
    picture.dispose();
  }
}

Future<void> _capture(WidgetTester tester, String name) async {
  final boundary = tester.renderObject<RenderRepaintBoundary>(
    find.byKey(_captureKey),
  );
  await tester.runAsync(() async {
    final image = await boundary.toImage();
    try {
      final png = (await image.toByteData(format: ui.ImageByteFormat.png))!;
      expect(png.lengthInBytes, greaterThan(10000));
      await File('$_directory/$name.png').writeAsBytes(
        png.buffer.asUint8List(png.offsetInBytes, png.lengthInBytes),
      );
    } finally {
      image.dispose();
    }
  });
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  final enabled = Platform.environment['SCREENSHOTS'] == '1';
  setUpAll(() async {
    if (enabled) {
      await _loadFonts();
      await _writeSchemaLogo(_schemaLogoPath, '键道', const Color(0xFF3B73D9));
      await _writeSchemaLogo(_catLogoPath, '星猫', const Color(0xFF9964CC));
      await _writeSchemaLogo(_tianLogoPath, '天行', const Color(0xFF188574));
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
          if (page == AppPage.scheme) {
            controller.userWordsReportScheme = 'keytao';
            controller.userWordsReport = const UserWordsReportDto(
              removed: [],
              conflicts: [_conflict],
            );
          }
          RustLib.initMock(api: _ScreenshotCore());
          addTearDown(RustLib.dispose);
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
          for (final path in [_schemaLogoPath, _catLogoPath, _tianLogoPath]) {
            await tester.runAsync(
              () => precacheImage(
                FileImage(File(path)),
                tester.element(find.byKey(_captureKey)),
                onError: (error, stack) =>
                    Error.throwWithStackTrace(error, stack!),
              ),
            );
          }
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
                      child: Theme(
                        data: Theme.of(context).copyWith(
                          textTheme: Theme.of(context).textTheme
                              .apply(fontFamilyFallback: const [_cjkFamily]),
                        ),
                        child: Navigator(
                          onGenerateRoute: (_) => MaterialPageRoute<void>(
                            builder: (_) => AppShell(controller: controller),
                          ),
                        ),
                      ),
                    );
                  },
                ),
              ),
            ),
          );
          await tester.pumpAndSettle();
          final logos = find.byType(SchemaLogo);
          expect(logos, findsWidgets);
          final logo = logos.first;
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
          if (page == AppPage.scheme) {
            for (final (key, path) in [
              ('keytao', _schemaLogoPath),
              ('xmjd', _catLogoPath),
              ('txjx', _tianLogoPath),
            ]) {
              final card = find.byKey(ValueKey('scheme-package-$key'));
              final image = tester.widget<Image>(
                find.descendant(of: card, matching: find.byType(Image)).first,
              );
              expect((image.image as FileImage).file.path, path);
            }
            final notice = find.text('键道6 · 自造词');
            expect(
              tester.getTopLeft(notice).dx,
              greaterThanOrEqualTo(device.name == 'desktop' ? 32 : 16),
            );
          } else {
            expect(find.text('方案开关'), findsOneWidget);
            for (final label in ['西文', '全角', '繁體']) {
              expect(
                tester.getTopLeft(find.text(label)).dx,
                greaterThanOrEqualTo(device.name == 'desktop' ? 32 : 16),
              );
            }
          }

          if (device.name == 'phone' && page == AppPage.input) {
            await _capture(tester, 'phone-keyboard-${brightness.name}');
          }

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

          await _capture(tester, name);
          if (page == AppPage.scheme) {
            tester.view.physicalSize = device.size;
            await tester.pumpAndSettle();
            await tester.tap(find.text('查看冲突'));
            await tester.pumpAndSettle();
            expect(find.text('测试词'), findsOneWidget);
            expect(tester.takeException(), isNull);
            await _capture(
              tester,
              '${device.name}-user-words-${brightness.name}',
            );
          }
          await tester.pumpWidget(const SizedBox.shrink());
        }, skip: !enabled);
      }
    }
  }
}
