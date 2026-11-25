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

final _directory =
    Platform.environment['SCREENSHOTS_DIR'] ?? '../target/ui-shots';
const _cjkFamily = 'SettingsScreenshotCjk';
const _captureKey = ValueKey('settings-screenshot');
final _schemaLogoPath = '$_directory/schema-logo.png';
final _catLogoPath = '$_directory/cat-logo.png';
final _tianLogoPath = '$_directory/tian-logo.png';
const _conflict = UserWordComparisonDto(
  entry: UserWordDto(id: 1, word: '测试词', code: 'csci'),
  status: 'code_taken',
  words: ['测试'],
  codes: [],
);
const _themes = [
  ThemeSummaryDto(
    id: 'keytao',
    name: '键道',
    builtIn: true,
    current: true,
    light: ThemePreviewColorsDto(
      background: '#FFFFFF',
      foreground: '#202124',
      accent: '#3B73D9',
    ),
    dark: ThemePreviewColorsDto(
      background: '#202124',
      foreground: '#EEEEEE',
      accent: '#89ACF0',
    ),
  ),
];

class _ScreenshotCore implements RustLibApi {
  @override
  dynamic noSuchMethod(Invocation invocation) =>
      switch (invocation.memberName) {
        #crateApiCoreGetThemeLibrary => Future.value(_themes),
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
      InstalledPackageDto(
        schemeKey: 'xmjd',
        active: false,
        hasPackageZip: true,
        logoPath: _catLogoPath,
      ),
      InstalledPackageDto(
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
    double? cornerRadius,
    String backgroundColor = '',
    required double fontSize,
    required bool systemDark,
  }) {
    final theme = super.resolveCandidatePanelPreview(
      colorScheme: colorScheme,
      accentColor: accentColor,
      cornerRadius: cornerRadius,
      backgroundColor: backgroundColor,
      fontSize: fontSize,
      systemDark: systemDark,
    );
    (theme['font'] as Map<String, dynamic>)['family'] = _cjkFamily;
    return theme;
  }
}

Future<void> _loadFonts() async {
  await (FontLoader('monospace')..addFont(
        File('/System/Library/Fonts/Menlo.ttc')
            .readAsBytes()
            .then(ByteData.sublistView),
      ))
      .load();
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

FTypeface _withScreenshotFont(FTypeface typeface) => typeface.copyWith(
  xs3: typeface.xs3.copyWith(fontFamilyFallback: const [_cjkFamily]),
  xs2: typeface.xs2.copyWith(fontFamilyFallback: const [_cjkFamily]),
  xs: typeface.xs.copyWith(fontFamilyFallback: const [_cjkFamily]),
  sm: typeface.sm.copyWith(fontFamilyFallback: const [_cjkFamily]),
  md: typeface.md.copyWith(fontFamilyFallback: const [_cjkFamily]),
  lg: typeface.lg.copyWith(fontFamilyFallback: const [_cjkFamily]),
  xl: typeface.xl.copyWith(fontFamilyFallback: const [_cjkFamily]),
  xl2: typeface.xl2.copyWith(fontFamilyFallback: const [_cjkFamily]),
  xl3: typeface.xl3.copyWith(fontFamilyFallback: const [_cjkFamily]),
  xl4: typeface.xl4.copyWith(fontFamilyFallback: const [_cjkFamily]),
  xl5: typeface.xl5.copyWith(fontFamilyFallback: const [_cjkFamily]),
  xl6: typeface.xl6.copyWith(fontFamilyFallback: const [_cjkFamily]),
  xl7: typeface.xl7.copyWith(fontFamilyFallback: const [_cjkFamily]),
  xl8: typeface.xl8.copyWith(fontFamilyFallback: const [_cjkFamily]),
);

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
      name: 'desktop-960',
      size: const Size(960, 860),
      platform: BridgePlatform.macOs,
    ),
    (
      name: 'desktop-1440',
      size: const Size(1440, 900),
      platform: BridgePlatform.macOs,
    ),
    (
      name: 'phone',
      size: const Size(390, 844),
      platform: BridgePlatform.android,
    ),
  ]) {
    for (final page in AppPage.values) {
      for (final brightness in Brightness.values) {
        final name = '${device.name}-${page.id}-${brightness.name}';
        testWidgets(
          'capture settings $name',
          (tester) async {
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
            const webViewChannel = MethodChannel('com.infradise.webview_core');
            final messenger = tester.binding.defaultBinaryMessenger;
            int? webViewId;
            if (page == AppPage.website) {
              messenger.setMockMethodCallHandler(webViewChannel, (call) async {
                if (call.method == 'create') {
                  webViewId = (call.arguments as Map)['controllerId'] as int;
                }
                return null;
              });
              addTearDown(
                () => messenger.setMockMethodCallHandler(webViewChannel, null),
              );
            }

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
                      return FTheme(
                        platform: context.platformVariant,
                        data: FThemeData(
                          colors: theme.colors,
                          touch: context.platformVariant.touch,
                          style: theme.style,
                          typography: FTypography(
                            display: _withScreenshotFont(
                              theme.typography.display,
                            ),
                            body: _withScreenshotFont(theme.typography.body),
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
            if (page == AppPage.website) {
              await tester.pump();
              expect(webViewId, isNotNull);
              await messenger.handlePlatformMessage(
                webViewChannel.name,
                webViewChannel.codec.encodeMethodCall(
                  MethodCall('onPageFinished', {
                    'controllerId': webViewId,
                    'url': controller.websiteOrigin,
                  }),
                ),
                null,
              );
            }
            await tester.pumpAndSettle();
            if (page == AppPage.input || page == AppPage.scheme) {
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
            }
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
                greaterThanOrEqualTo(controller.isMobile ? 16 : 24),
              );
              expect(find.text('方案开关'), findsOneWidget);
              for (final label in ['西文', '全角', '繁體']) {
                expect(
                  tester.getTopLeft(find.text(label)).dx,
                  greaterThanOrEqualTo(controller.isMobile ? 16 : 24),
                );
              }
            }

            expect(tester.takeException(), isNull);
            await _capture(tester, name);
            if (page != AppPage.website) {
              final scrollable = tester.state<ScrollableState>(
                find
                    .descendant(
                      of: find.byKey(PageStorageKey(page.id)),
                      matching: find.byType(Scrollable),
                    )
                    .first,
              );
              final extent = scrollable.position.maxScrollExtent.ceil();
              if (extent > 0) {
                tester.view.physicalSize = Size(
                  device.size.width,
                  device.size.height + extent,
                );
                await tester.pumpAndSettle();
                expect(
                  scrollable.position.maxScrollExtent,
                  lessThanOrEqualTo(1),
                );
                expect(tester.takeException(), isNull);
                await _capture(tester, '$name-full');
                tester.view.physicalSize = device.size;
                await tester.pumpAndSettle();
              }
            }
            if (page == AppPage.scheme) {
              for (final (key, title) in [
                ('rime-ice', AppStrings.rimeIce),
                ('easy-en', schemeDisplayName('easy_en')),
              ]) {
                final paragraph = tester.renderObject<RenderParagraph>(
                  find.descendant(
                    of: find.byKey(ValueKey('addon-$key')),
                    matching: find.text(title),
                  ),
                );
                expect(paragraph.didExceedMaxLines, isFalse, reason: title);
              }
              final addons = find.byKey(const ValueKey('addon-schemes-button'));
              await tester.ensureVisible(addons);
              await tester.tap(addons);
              await tester.pumpAndSettle();
              expect(find.byType(AppDialog), findsOneWidget);
              await _capture(
                tester,
                '${device.name}-addons-${brightness.name}',
              );
              Navigator.of(tester.element(find.byType(AppDialog))).pop();
              await tester.pumpAndSettle();
              final conflicts = find.text('查看冲突');
              await tester.ensureVisible(conflicts);
              await tester.tap(conflicts);
              await tester.pumpAndSettle();
              expect(find.text('测试词'), findsOneWidget);
              expect(tester.takeException(), isNull);
              await _capture(
                tester,
                '${device.name}-user-words-${brightness.name}',
              );
            }
            await tester.pumpWidget(const SizedBox.shrink());
          },
          skip: !enabled,
          variant: page == AppPage.website
              ? TargetPlatformVariant.only(TargetPlatform.linux)
              : const DefaultTestVariant(),
        );
      }
    }
  }
}
