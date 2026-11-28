import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/app/controller.dart';
import 'package:keytao/src/rust/api/core.dart' show AddonStatusDto;
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/rust/frb_generated.dart';
import 'package:keytao/src/ui/scheme_page.dart';
import 'package:keytao/src/ui/theme.dart';
import 'package:keytao/src/ui/widgets.dart';

import 'view_fixture.dart';

class _VisibilityCore implements RustLibApi {
  Map<String, dynamic> settings = {};
  int writes = 0;

  @override
  dynamic noSuchMethod(Invocation invocation) {
    final method = invocation.memberName.toString();
    if (method.contains('GetSchemeSyncState') ||
        method.contains('SetSchemeSyncPreferences')) {
      if (invocation.namedArguments[#settingsJson] case final String value) {
        settings = jsonDecode(value) as Map<String, dynamic>;
        writes++;
      }
      return Future<String>.value(
        jsonEncode({
          'settings': settings,
          'follow_account_sync': false,
          'synced_schemes': <String>[],
        }),
      );
    }
    throw StateError('Unexpected bridge call: $method');
  }
}

class _VisibilityController extends ViewController {
  _VisibilityController() : super(BridgePlatform.macOs);

  final installs = <String>[];

  @override
  Future<bool> installSchemePackage(String key) async {
    installs.add(key);
    return true;
  }

  @override
  Future<bool> manageRimeAddon(String key, bool installed) async {
    installs.add(key);
    return true;
  }
}

Future<void> _pumpLibrary(WidgetTester tester, AppController controller) async {
  tester.view.devicePixelRatio = 1;
  tester.view.physicalSize = const Size(1200, 1000);
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  await tester.pumpWidget(
    KeyTaoTheme(
      platform: controller.info.platform,
      home: FScaffold(
        child: SingleChildScrollView(
          child: ListenableBuilder(
            listenable: controller,
            builder: (_, _) => SchemeLibrarySections(controller: controller),
          ),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  late _VisibilityCore api;
  late AppController controller;

  AppController createController() => AppController(
    info: const BridgeInfo(
      platform: BridgePlatform.macOs,
      appVersion: 'test',
      userRoot: '/local/rime',
    ),
  );

  setUp(() {
    api = _VisibilityCore();
    RustLib.initMock(api: api);
    controller = createController();
  });

  tearDown(() {
    controller.dispose();
    RustLib.dispose();
  });

  test(
    'hidden schemes persist and can be restored after controller restart',
    () async {
      api.settings = {
        'download_source': 'github',
        'desktop_english_mode': 'ascii',
      };
      await controller.loadSchemeSyncState();
      await controller.setSchemeHidden('txjx', true);
      await controller.setSchemeHidden('rime_mint', true);
      await controller.setSchemeHidden('xmjd-feiding', true);
      expect(api.settings['hidden_schemes'], ['rime_mint', 'txjx', 'xmjd']);
      expect(api.settings['desktop_english_mode'], 'ascii');
      controller.dispose();
      controller = createController();
      await controller.loadSchemeSyncState();
      expect(controller.isSchemeHidden('txjx'), isTrue);
      expect(controller.isSchemeHidden('rime_mint'), isTrue);
      expect(controller.isSchemeHidden('xmjd'), isTrue);
      expect(controller.isSchemeHidden('xmjd-feiding'), isTrue);
      await controller.setSchemeHidden('xmjd', false);
      expect(controller.isSchemeHidden('xmjd-feiding'), isFalse);
      expect(api.settings['hidden_schemes'], ['rime_mint', 'txjx']);
    },
  );

  test(
    'all deployed packages and xmjd variants are protected from hiding',
    () async {
      controller.isolatedPackages = const [
        IsolatedPackageDto(
          packageKey: 'builtin/txjx',
          displayName: '天行键',
          active: false,
          ready: true,
        ),
        IsolatedPackageDto(
          packageKey: 'xmjd-feiding',
          displayName: '星猫键道（非顶功）',
          active: false,
          ready: true,
          engine: 'libjd_engine',
        ),
      ];
      controller.addon = const AddonStatusDto(
        installed: true,
        deployed: true,
        version: 'v1',
        englishAvailable: true,
      );
      controller.rimeAddons['rime_mint']!.status = const AddonStatusDto(
        installed: true,
        deployed: true,
        version: 'v1',
        englishAvailable: false,
      );
      controller.installedPackages = const [
        InstalledPackageDto(
          schemeKey: 'local-test',
          source: 'local',
          active: false,
          hasPackageZip: true,
        ),
      ];
      for (final key in [
        'keytao',
        'txjx',
        'xmjd',
        'xmjd-feiding',
        'easy_en',
        'rime_mint',
        'local-test',
      ]) {
        expect(controller.canHideScheme(key), isFalse, reason: key);
        await controller.setSchemeHidden(key, true);
        expect(controller.isSchemeHidden(key), isFalse, reason: key);
      }
      expect(api.writes, 0);
    },
  );

  test(
    'deployed schemes remain visible even with a saved hidden preference',
    () async {
      api.settings = {
        'hidden_schemes': ['keytao', 'txjx', 'xmjd-feiding'],
      };
      controller.isolatedPackages = const [
        IsolatedPackageDto(
          packageKey: 'txjx',
          displayName: '天行键',
          active: false,
          ready: true,
        ),
        IsolatedPackageDto(
          packageKey: 'xmjd',
          displayName: '星猫键道',
          active: false,
          ready: true,
        ),
      ];
      await controller.loadSchemeSyncState();
      expect(controller.hiddenSchemes, isNot(contains('keytao')));
      expect(controller.isSchemeHidden('txjx'), isFalse);
      expect(controller.isSchemeHidden('xmjd-feiding'), isFalse);
    },
  );

  testWidgets('one remote list hides and restores schemes without installing', (
    tester,
  ) async {
    controller.dispose();
    final c = _VisibilityController();
    controller = c;
    await _pumpLibrary(tester, c);
    final grid = find.byKey(const ValueKey('cloud-schemes'));
    expect(tester.widget<HairlineGridSection>(grid).children, hasLength(11));
    for (final key in [
      'rime-ice',
      'easy-en',
      'wanxiang',
      'rime-mint',
      'rime-frost',
      'rime-lssp',
    ]) {
      expect(
        find.descendant(of: grid, matching: find.byKey(ValueKey('addon-$key'))),
        findsOneWidget,
      );
    }
    expect(find.bySemanticsLabel('展开全部方案'), findsNothing);
    expect(find.byKey(const ValueKey('addon-schemes-button')), findsNothing);
    expect(find.byKey(const ValueKey('hide-scheme-keytao')), findsNothing);
    expect(
      tester
          .widget<FButton>(find.byKey(const ValueKey('hide-scheme-easy_en')))
          .onPress,
      isNull,
    );
    for (final key in ['txjx', 'rime_mint']) {
      final hide = find.byKey(ValueKey('hide-scheme-$key'));
      await tester.ensureVisible(hide);
      await tester.tap(hide);
      await tester.pumpAndSettle();
    }
    expect(find.byKey(const ValueKey('scheme-package-txjx')), findsNothing);
    expect(find.byKey(const ValueKey('addon-rime-mint')), findsNothing);
    final add = find.byKey(const ValueKey('add-scheme-button'));
    expect(
      tester.getBottomLeft(add).dy,
      lessThanOrEqualTo(tester.getTopLeft(grid).dy),
    );
    await tester.ensureVisible(add);
    await tester.tap(add);
    await tester.pumpAndSettle();
    final restore = find.byKey(const ValueKey('add-hidden-scheme-txjx'));
    expect(restore, findsOneWidget);
    expect(
      find.byKey(const ValueKey('add-hidden-scheme-rime_mint')),
      findsOneWidget,
    );
    expect(
      tester.getTopLeft(find.byKey(const ValueKey('add-custom-scheme'))).dy,
      greaterThan(tester.getTopLeft(restore).dy),
    );
    await tester.tap(restore);
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('scheme-package-txjx')), findsOneWidget);
    expect(c.installs, isEmpty);
    expect(find.byKey(const ValueKey('add-custom-scheme')), findsNothing);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets(
    'add menu ends with the existing Git dialog and keeps local import',
    (tester) async {
      controller.dispose();
      controller = _VisibilityController();
      await _pumpLibrary(tester, controller);
      expect(find.widgetWithText(ActionButton, '从目录导入'), findsOneWidget);
      expect(find.widgetWithText(ActionButton, '从 Git 添加'), findsNothing);
      final add = find.byKey(const ValueKey('add-scheme-button'));
      await tester.ensureVisible(add);
      await tester.tap(add);
      await tester.pumpAndSettle();
      final items = tester.widgetList<FItem>(find.byType(FItem));
      expect(items.map((item) => item.key), [
        const ValueKey('add-custom-scheme'),
      ]);
      await tester.tap(find.byKey(const ValueKey('add-custom-scheme')));
      await tester.pumpAndSettle();
      expect(find.text('从 Git 添加方案'), findsOneWidget);
      expect(find.byKey(const ValueKey('git-scheme-url')), findsOneWidget);
      expect(find.byKey(const ValueKey('add-custom-scheme')), findsNothing);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );
}
