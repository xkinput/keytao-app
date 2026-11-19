import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/app/options.dart';
import 'package:keytao/src/rust/api/core.dart'
    show AddonStatusDto, WanxiangGrammarStatusDto;
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/ui/shell.dart';
import 'package:keytao/src/ui/theme.dart';
import 'package:keytao/src/ui/widgets.dart';

import '../view_fixture.dart';

const _directory =
    '/private/tmp/claude-501/-Users-rea-code-keytao-org-keytao-app/'
    '348229fe-6d80-4d02-b020-e4d903e04588/scratchpad/snowfix/scheme-page';
const _cjkFamily = 'SchemeScreenshotCjk';
const _captureKey = ValueKey('scheme-page-screenshot');

class _SchemeScreenshotController extends ViewController {
  _SchemeScreenshotController(super.platform) {
    activePage = AppPage.scheme;
    appUpdate = null;
    activeSchemaLogoPath = File('assets/logo.png').absolute.path;
    installedPackages = [
      InstalledPackageDto(
        schemeKey: 'keytao',
        version: local!.version,
        active: true,
        hasPackageZip: true,
        logoPath: activeSchemaLogoPath,
      ),
      const InstalledPackageDto(
        schemeKey: 'xmjd',
        version: '2026.09.26',
        active: false,
        hasPackageZip: true,
      ),
      const InstalledPackageDto(
        schemeKey: 'local-custom',
        source: 'local',
        displayName: '小鹤双拼',
        active: false,
        hasPackageZip: true,
      ),
    ];
    schemeUpdates = const [
      SchemeUpdateDto(
        schemeKey: 'xmjd',
        installedVersion: '2026.09.26',
        latestVersion: '2026.09.28',
        hasUpdate: true,
      ),
    ];
    dismissedSchemeUpdates = {'xmjd': '2026.09.28'};
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
    wanxiang = const AddonStatusDto(
      installed: true,
      deployed: true,
      version: '2026.09.26',
      variant: 'base',
      englishAvailable: false,
    );
    wanxiangGrammar = WanxiangGrammarStatusDto(
      available: true,
      installed: false,
      version: '',
      size: BigInt.zero,
    );
    rimeMint = rimeFrost = rimeLssp = rimeIce;
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
      // Try the next system font when this collection cannot be loaded.
    }
  }
  throw StateError('No macOS CJK font could be loaded for screenshots.');
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
      await Directory(_directory).create(recursive: true);
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
  final screenshots = Platform.environment['SCREENSHOTS'] == '1';
  setUpAll(() async {
    if (screenshots) await _loadFonts();
  });

  for (final device in [
    (
      name: 'desktop-dark-1440x900',
      size: const Size(1440, 900),
      platform: BridgePlatform.macOs,
      brightness: Brightness.dark,
    ),
    (
      name: 'phone-light-390x844',
      size: const Size(390, 844),
      platform: BridgePlatform.android,
      brightness: Brightness.light,
    ),
  ]) {
    testWidgets('scheme page fits ${device.name}', (tester) async {
      final controller = _SchemeScreenshotController(device.platform);
      addTearDown(controller.dispose);
      tester.view.devicePixelRatio = 1;
      tester.view.physicalSize = device.size;
      tester.platformDispatcher.platformBrightnessTestValue = device.brightness;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      addTearDown(tester.platformDispatcher.clearPlatformBrightnessTestValue);
      await tester.pumpWidget(const SizedBox(key: _captureKey));
      await tester.runAsync(
        () => precacheImage(
          FileImage(File(controller.activeSchemaLogoPath!)),
          tester.element(find.byKey(_captureKey)),
          onError: (error, stack) => Error.throwWithStackTrace(error, stack!),
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
                final fallback = screenshots ? const [_cjkFamily] : null;
                return FTheme(
                  platform: context.platformVariant,
                  data: FThemeData(
                    colors: theme.colors,
                    touch: context.platformVariant.touch,
                    style: theme.style,
                    typography: screenshots
                        ? FTypography(
                            display: _withScreenshotFont(
                              theme.typography.display,
                            ),
                            body: _withScreenshotFont(theme.typography.body),
                          )
                        : theme.typography,
                  ),
                  child: Theme(
                    data: Theme.of(context).copyWith(
                      textTheme: Theme.of(context).textTheme
                          .apply(fontFamilyFallback: fallback),
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
      expect(tester.getSize(find.byKey(_captureKey)), device.size);
      expect(find.text('方案列表'), findsOneWidget);
      expect(find.text('键道方案'), findsNothing);
      expect(
        find.byKey(const ValueKey('addon-schemes-button')),
        findsOneWidget,
      );
      expect(find.bySemanticsLabel('展开全部方案'), findsNothing);
      final cards = [
        for (final key in ['keytao', 'xmjd', 'txjx', 'keydo', 'snow_jiandao'])
          find
              .ancestor(
                of: find.byKey(ValueKey('scheme-package-$key')),
                matching: find.byType(HairlineGridCell),
              )
              .first,
        find.byKey(const ValueKey('addon-rime-ice')),
        find.byKey(const ValueKey('addon-easy-en')),
      ];
      expect(
        tester.getTopLeft(cards.first).dy -
            tester.getBottomLeft(find.text('方案列表')).dy,
        inInclusiveRange(0, 40),
      );
      for (final card in cards) {
        expect(tester.getSize(card).width, tester.getSize(cards.first).width);
      }
      if (device.platform == BridgePlatform.macOs) {
        expect(tester.getTopLeft(cards[0]).dy, tester.getTopLeft(cards[1]).dy);
        expect(tester.getTopLeft(cards[4]).dy, tester.getTopLeft(cards[5]).dy);
        for (final (card, title) in [
          (cards[5], AppStrings.rimeIce),
          (cards[6], schemeDisplayName('easy_en')),
        ]) {
          if (screenshots) {
            final paragraph = tester.renderObject<RenderParagraph>(
              find.descendant(of: card, matching: find.text(title)),
            );
            expect(paragraph.didExceedMaxLines, isFalse, reason: title);
          }
        }
        expect(
          tester.getTopLeft(cards[1]).dx,
          greaterThan(tester.getTopLeft(cards[0]).dx),
        );
      } else {
        for (var index = 1; index < cards.length; index++) {
          expect(
            tester.getTopLeft(cards[index]).dx,
            tester.getTopLeft(cards[0]).dx,
          );
          expect(
            tester.getTopLeft(cards[index]).dy,
            greaterThanOrEqualTo(tester.getBottomLeft(cards[index - 1]).dy),
          );
        }
      }
      expect(tester.takeException(), isNull);
      if (screenshots) await _capture(tester, device.name);
      final scrollable = find
          .descendant(
            of: find.byKey(const PageStorageKey('scheme')),
            matching: find.byType(Scrollable),
          )
          .first;
      final position = tester.state<ScrollableState>(scrollable).position;
      position.jumpTo(position.maxScrollExtent);
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('wanxiang-grammar')), findsNothing);
      expect(tester.takeException(), isNull);
      if (screenshots && device.platform == BridgePlatform.macOs) {
        await _capture(tester, '${device.name}-scrolled');
      }
      final addons = find.byKey(const ValueKey('addon-schemes-button'));
      await tester.ensureVisible(addons);
      await tester.tap(addons);
      await tester.pumpAndSettle();
      expect(find.byType(AppDialog), findsOneWidget);
      for (final key in ['wanxiang', 'rime-mint', 'rime-frost', 'rime-lssp']) {
        expect(
          find.descendant(
            of: find.byType(AppDialog),
            matching: find.byKey(ValueKey('addon-$key')),
          ),
          findsOneWidget,
        );
      }
      for (final key in ['rime-ice', 'easy-en']) {
        expect(
          find.descendant(
            of: find.byType(AppDialog),
            matching: find.byKey(ValueKey('addon-$key')),
          ),
          findsNothing,
        );
      }
      expect(find.byKey(const ValueKey('wanxiang-grammar')), findsOneWidget);
      expect(tester.takeException(), isNull);
      if (screenshots) await _capture(tester, '${device.name}-addons');
      await tester.pumpWidget(const SizedBox.shrink());
    });
  }
}
