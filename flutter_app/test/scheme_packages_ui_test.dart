import 'dart:async';
import 'dart:io';

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/app/app.dart';
import 'package:keytao/src/app/controller.dart'
    show AppController, DeployStep, DeployStepState, OperationKind;
import 'package:keytao/src/app/options.dart';
import 'package:keytao/src/rust/api/core.dart'
    show AddonStatusDto, WanxiangGrammarStatusDto;
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/rust/frb_generated.dart';
import 'package:keytao/src/ui/input_page.dart';
import 'package:keytao/src/ui/login.dart' show showAccountDialog;
import 'package:keytao/src/ui/onboarding.dart';
import 'package:keytao/src/ui/package_schema_selection.dart'
    show showLocalImportSchemaSelection;
import 'package:keytao/src/ui/scheme_page.dart';
import 'package:keytao/src/ui/settings.dart' show showCloudPackageTransfer;
import 'package:keytao/src/ui/theme.dart';
import 'package:keytao/src/ui/widgets.dart';

import 'controller_parity_test.dart' show FakeCore;
import 'view_fixture.dart';

const _update = SchemeUpdateDto(
  schemeKey: 'keytao',
  installedVersion: 'v1',
  latestVersion: 'v2',
  hasUpdate: true,
);

class _SchemaSwitchCore extends FakeCore {
  @override
  Future<List<SchemaSwitchDto>> crateApiCoreGetSchemaSwitches() async => const [
    SchemaSwitchDto(
      schemaId: 'JD6',
      name: 'jffh',
      states: ['简体', '繁體'],
      value: 0,
      options: [],
    ),
  ];
}

class _PackageController extends ViewController {
  _PackageController([super.platform = BridgePlatform.macOs]) {
    appUpdate = null;
  }

  final actions = <String>[];
  Completer<bool>? schemaSave;

  List<PackageSchemaDto> schemasResult = const [
    PackageSchemaDto(id: 'alpha', name: 'Alpha Name', selected: true),
    PackageSchemaDto(id: 'beta', name: 'Beta Name', selected: false),
  ];

  @override
  Future<List<PackageSchemaDto>> packageSchemas(String key) async =>
      schemasResult;

  @override
  Future<bool> setPackageSchemas(
    String key,
    List<String> ids, {
    bool activate = false,
  }) async {
    actions.add(
      activate
          ? 'schemas:$key:${ids.join(',')}:activate'
          : 'schemas:$key:${ids.join(',')}',
    );
    return await schemaSave?.future ?? true;
  }

  void rebuild() => notifyListeners();

  @override
  Future<bool> open(String value) async {
    actions.add('open:$value');
    return true;
  }

  @override
  void selectScheme(String value) {
    scheme = value;
    notifyListeners();
  }

  @override
  Future<bool> installSchemePackage(String key) async {
    actions.add('install:$key');
    return true;
  }

  @override
  Future<bool> activateSchemePackage(String key) async {
    actions.add('activate:$key');
    return true;
  }

  @override
  Future<bool> switchPackage(String key, {String? schemaId}) =>
      activateSchemePackage(schemaId == null ? key : '$key:$schemaId');

  @override
  Future<bool> setPackageEasyEnglish(String key, bool enabled) async {
    actions.add('overlay:$key:$enabled');
    return true;
  }

  @override
  Future<bool> updateSchemePackage(String key) async {
    actions.add('update:$key');
    return true;
  }

  @override
  Future<bool> downloadSchemePackage(String key) async {
    actions.add('download:$key');
    return true;
  }

  @override
  Future<bool> removeSchemePackage(String key) async {
    actions.add('remove:$key');
    return true;
  }

  @override
  Future<bool> releasePackageCache(String key, {required bool imeJd}) async {
    actions.add('release:$key:$imeJd');
    return true;
  }

  @override
  Future<bool> installAddon() async {
    actions.add('easy:true');
    return true;
  }

  @override
  Future<bool> uninstallAddon() async {
    actions.add('easy:false');
    return true;
  }

  @override
  Future<bool> manageWanxiang(bool installed) async {
    actions.add('wanxiang:$installed');
    return true;
  }

  @override
  Future<bool> manageWanxiangGrammar(bool installed) async {
    actions.add('grammar:$installed');
    return true;
  }

  @override
  Future<bool> manageRimeAddon(String id, bool installed) async {
    actions.add('$id:$installed');
    return true;
  }

  @override
  Future<bool> importLocalScheme({
    String? schemeKey,
    bool fromArchive = false,
    Future<List<String>?> Function(LocalImportInspectionDto inspection)?
    selectSchemas,
  }) async {
    actions.add('import:$schemeKey:${fromArchive ? 'zip' : 'folder'}');
    return true;
  }

  @override
  Future<bool> setSchemeAccountSync(String key, bool enabled) async {
    actions.add('sync:$key:$enabled');
    if (enabled) {
      syncedSchemes.add(key);
    } else {
      syncedSchemes.remove(key);
    }
    notifyListeners();
    return true;
  }

  @override
  Future<Map<String, dynamic>?> previewCloudPackage(String key) async => {
    'files': ['keytao.schema.yaml'],
    'archive_size': 1024,
    'sha256': 'preview',
  };

  @override
  Future<bool> uploadCloudPackage(String key, {String? expectedSha256}) async =>
      true;

  @override
  Future<bool> downloadCloudPackage(String id) async => true;

  @override
  Future<void> reloadAccountSession({bool checkSchemeProfile = true}) async {}

  @override
  Future<bool> dismissSchemeUpdate(SchemeUpdateDto update) async {
    actions.add('dismiss:${update.schemeKey}');
    dismissedSchemeUpdates[update.schemeKey] = update.latestVersion;
    notifyListeners();
    return true;
  }
}

void _size(WidgetTester tester, Size size) {
  tester.view.devicePixelRatio = 1;
  tester.view.physicalSize = size;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
}

Future<void> _pumpRow(
  WidgetTester tester,
  _PackageController controller,
  String key,
) async {
  await tester.pumpWidget(
    KeyTaoTheme(
      platform: controller.info.platform,
      home: FScaffold(
        child: SchemePackageRow(controller: controller, schemeKey: key),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

Future<void> _pumpAddons(
  WidgetTester tester,
  _PackageController controller,
) async {
  await tester.pumpWidget(
    KeyTaoTheme(
      platform: controller.info.platform,
      home: FScaffold(
        child: SingleChildScrollView(child: AddonCard(controller: controller)),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

Future<void> _pumpSchemePage(
  WidgetTester tester,
  AppController controller, {
  bool settle = true,
}) async {
  await tester.pumpWidget(
    KeyTaoTheme(
      platform: controller.info.platform,
      home: FScaffold(
        child: ListenableBuilder(
          listenable: controller,
          builder: (_, _) => SchemePage(controller: controller),
        ),
      ),
    ),
  );
  if (settle) await tester.pumpAndSettle();
}

Future<void> _openAddons(WidgetTester tester) async {
  final button = find.byKey(const ValueKey('addon-schemes-button'));
  await tester.ensureVisible(button);
  await tester.tap(button);
  await tester.pumpAndSettle();
  expect(find.byType(AppDialog), findsOneWidget);
}

List<String> _actionLabels(WidgetTester tester) => tester
    .widgetList<ActionButton>(find.byType(ActionButton))
    .map((button) => button.label)
    .where((label) => const {'下载', '重新下载', '更新', '使用', '卸载'}.contains(label))
    .toList();

void main() {
  testWidgets('package cache menu shows sizes and requires confirmation', (
    tester,
  ) async {
    const key = 'local-custom';
    final c = _PackageController();
    addTearDown(c.dispose);
    _size(tester, const Size(390, 844));
    InstalledPackageDto package({bool active = false}) => InstalledPackageDto(
      schemeKey: key,
      source: 'local',
      displayName: 'Custom scheme',
      active: active,
      hasPackageZip: true,
      buildCacheBytes: BigInt.from(2 * 1024 * 1024),
      imeJdCacheBytes: BigInt.from(1024 * 1024),
    );
    final more = find.byIcon(FLucideIcons.ellipsis);
    final buildCache = find.widgetWithText(FItem, '释放编译缓存');
    final imeJdCache = find.widgetWithText(FItem, '释放 ime-jd 词典');

    c.installedPackages = [package()];
    await _pumpRow(tester, c, key);
    expect(find.text('可释放 3.0 MB'), findsOneWidget);
    await tester.tap(more);
    await tester.pumpAndSettle();
    expect(
      find.descendant(of: buildCache, matching: find.text('2.0 MB')),
      findsOneWidget,
    );
    expect(
      find.descendant(of: imeJdCache, matching: find.text('1.0 MB')),
      findsOneWidget,
    );

    await tester.tap(buildCache);
    await tester.pumpAndSettle();
    expect(find.byType(AppDialog), findsOneWidget);
    expect(find.textContaining('可释放 2.0 MB'), findsOneWidget);
    expect(c.actions, isEmpty);
    await tester.tap(find.widgetWithText(ActionButton, AppStrings.cancel));
    await tester.pumpAndSettle();
    expect(c.actions, isEmpty);

    await tester.tap(more);
    await tester.pumpAndSettle();
    await tester.tap(buildCache);
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(ActionButton, '确认释放'));
    await tester.pumpAndSettle();
    expect(c.actions, ['release:$key:false']);

    c.installedPackages = [package(active: true)];
    await _pumpRow(tester, c, key);
    await tester.tap(more);
    await tester.pumpAndSettle();
    expect(buildCache, findsNothing);
    await tester.tap(imeJdCache);
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(ActionButton, '确认释放'));
    await tester.pumpAndSettle();
    expect(c.actions, ['release:$key:false', 'release:$key:true']);

    c.inputEngine = 'libjd_engine';
    await _pumpRow(tester, c, key);
    await tester.tap(more);
    await tester.pumpAndSettle();
    expect(imeJdCache, findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'every installed card selects named schemas and confirms deployment',
    (tester) async {
      final c = _PackageController();
      addTearDown(c.dispose);
      _size(tester, const Size(1100, 1200));
      Future<void> select(
        String key,
        Finder menu, {
        bool inDialog = false,
      }) async {
        await tester.ensureVisible(menu);
        await tester.tap(menu);
        await tester.pumpAndSettle();
        final item = find.widgetWithText(FItem, AppStrings.selectSchemas);
        expect(item, findsOneWidget, reason: key);
        await tester.tap(item);
        await tester.pumpAndSettle();
        expect(find.text('Alpha Name'), findsOneWidget);
        expect(find.text('alpha'), findsOneWidget);
        expect(find.text('Beta Name'), findsOneWidget);
        final boxes = find.byType(FCheckbox);
        expect(tester.widget<FCheckbox>(boxes.at(0)).value, isTrue);
        expect(tester.widget<FCheckbox>(boxes.at(0)).enabled, isFalse);
        expect(tester.widget<FCheckbox>(boxes.at(1)).value, isFalse);
        await tester.tap(boxes.at(1));
        await tester.pumpAndSettle();
        await tester.tap(boxes.at(0));
        await tester.pumpAndSettle();
        expect(tester.widget<FCheckbox>(boxes.at(1)).enabled, isFalse);
        await tester.tap(
          find.descendant(
            of: find.byType(AppDialog),
            matching: find.widgetWithText(ActionButton, AppStrings.deploy),
          ),
        );
        await tester.pumpAndSettle();
        expect(c.actions.last, 'schemas:$key:beta');
        expect(
          find.byType(AppDialog),
          inDialog ? findsOneWidget : findsNothing,
        );
        expect(tester.takeException(), isNull);
      }

      for (final key in [
        'keytao',
        'xmjd',
        'txjx',
        'keydo',
        'snow_jiandao',
        'local-demo',
      ]) {
        c.installedPackages = [
          InstalledPackageDto(
            schemeKey: key,
            active: true,
            hasPackageZip: true,
            source: key.startsWith('local-') ? 'local' : 'official',
          ),
        ];
        await _pumpRow(tester, c, key);
        await select(key, find.byIcon(FLucideIcons.ellipsis));
      }
      const installed = AddonStatusDto(
        installed: true,
        deployed: true,
        version: 'v1',
        englishAvailable: false,
      );
      c.addon = c.wanxiang = installed;
      for (final slot in c.rimeAddons.values) {
        slot.status = installed;
      }
      await _pumpAddons(tester, c);
      await select(
        'rime_ice',
        find.descendant(
          of: find.byKey(const ValueKey('addon-rime-ice')),
          matching: find.byIcon(FLucideIcons.ellipsis),
        ),
      );
      await _pumpSchemePage(tester, c);
      await _openAddons(tester);
      for (final (card, key) in [
        ('addon-wanxiang', 'wanxiang'),
        ('addon-rime-mint', 'rime_mint'),
        ('addon-rime-frost', 'rime_frost'),
        ('addon-rime-lssp', 'triple_pinyin_lssp'),
      ]) {
        await select(
          key,
          find.descendant(
            of: find.byKey(ValueKey(card)),
            matching: find.byIcon(FLucideIcons.ellipsis),
          ),
          inDialog: true,
        );
      }
    },
  );

  for (final platform in [
    BridgePlatform.windows,
    BridgePlatform.macOs,
    BridgePlatform.linux,
  ]) {
    testWidgets('${platform.name} missing scheme prompt opens scheme page', (
      tester,
    ) async {
      final c = _PackageController(platform)
        ..installedPackages = []
        ..local = const LocalSchemaDto(
          installed: false,
          deployed: false,
          schemas: [],
        );
      addTearDown(c.dispose);
      _size(tester, const Size(1200, 1000));
      await tester.pumpWidget(
        KeyTaoApp(
          controller: c,
          initialOnboarding: const OnboardingDto(completed: true),
        ),
      );
      await tester.pumpAndSettle();
      if (platform == BridgePlatform.windows) {
        expect(c.activePage, AppPage.scheme);
        expect(find.byType(SchemePage), findsOneWidget);
        await tester.tap(find.byKey(const ValueKey('nav-ime')));
        await tester.pumpAndSettle();
      }
      expect(
        find.descendant(
          of: find.byKey(const ValueKey('current-schema-selector')),
          matching: find.text(AppStrings.schemeMissing),
        ),
        findsOneWidget,
      );
      final button = find.widgetWithText(ActionButton, '前往安装方案');
      await tester.ensureVisible(button);
      await tester.tap(button);
      await tester.pumpAndSettle();
      expect(c.activePage, AppPage.scheme);
      expect(find.byType(SchemePage), findsOneWidget);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    });
  }

  for (final (installed, deployed, latest, label) in [
    (false, false, '2', '下载'),
    (true, true, '1', null),
    (true, false, '1', null),
    (true, true, '2', AppStrings.update),
    (true, false, '2', AppStrings.update),
  ]) {
    testWidgets(
      'dialog actions for installed=$installed deployed=$deployed latest=$latest',
      (tester) async {
        final c = _PackageController();
        addTearDown(c.dispose);
        _size(tester, const Size(1200, 1200));
        final status = AddonStatusDto(
          installed: installed,
          deployed: deployed,
          version: '1',
          englishAvailable: false,
        );
        c.addon = c.wanxiang = status;
        c.addonLatestVersion = c.wanxiangLatestVersion = latest;
        for (final slot in c.rimeAddons.values) {
          slot.status = status;
          slot.latestVersion = latest;
        }
        await _pumpSchemePage(tester, c);
        await _openAddons(tester);
        final grid = find.byKey(const ValueKey('addon-schemes'));
        final buttons = tester.widgetList<ActionButton>(
          find.descendant(of: grid, matching: find.byType(ActionButton)),
        );
        expect(
          buttons
              .where((button) => button.primary)
              .map((button) => button.label),
          !installed
              ? isEmpty
              : [
                  for (var i = 0; i < 4; i++) ...[?label, '使用'],
                ],
        );
        expect(
          buttons.where((button) => button.label == AppStrings.uninstall),
          isEmpty,
        );
        for (final (key, name) in [
          ('addon-wanxiang', 'wanxiang'),
          ('addon-rime-mint', 'rime_mint'),
          ('addon-rime-frost', 'rime_frost'),
          ('addon-rime-lssp', 'triple_pinyin_lssp'),
        ]) {
          final card = find.byKey(ValueKey(key));
          if (label != null) {
            final install = find.descendant(
              of: card,
              matching: find.widgetWithText(ActionButton, label),
            );
            await tester.ensureVisible(install);
            await tester.tap(install);
            await tester.pump(const Duration(seconds: 1));
            expect(c.actions.last, '$name:true');
          }
          if (installed) {
            final more = find.descendant(
              of: card,
              matching: find.byIcon(FLucideIcons.ellipsis),
            );
            await tester.ensureVisible(more);
            await tester.tap(more);
            await tester.pumpAndSettle();
            await tester.tap(find.widgetWithText(FItem, AppStrings.uninstall));
            await tester.pumpAndSettle();
            expect(c.actions.last, '$name:false');
          }
        }
        c.busy = true;
        c.rebuild();
        await tester.pumpAndSettle();
        expect(
          tester.widgetList<ActionButton>(
            find.descendant(of: grid, matching: find.byType(ActionButton)),
          ),
          everyElement(
            isA<ActionButton>().having(
              (button) => button.onPress,
              'onPress',
              null,
            ),
          ),
        );
        expect(
          tester
              .widget<FSelect<String>>(
                find.byWidgetPredicate((widget) => widget is FSelect<String>),
              )
              .enabled,
          isFalse,
        );
        expect(tester.takeException(), isNull);
        await tester.pumpWidget(const SizedBox.shrink());
        await tester.pumpAndSettle();
      },
    );
  }

  testWidgets('add-on repository menus open by button and secondary tap', (
    tester,
  ) async {
    final c = _PackageController();
    addTearDown(c.dispose);
    _size(tester, const Size(1000, 1200));
    for (final busy in [false, true]) {
      c.busy = busy;
      await _pumpSchemePage(tester, c);
      await _openAddons(tester);
      for (final (key, title, url) in const [
        ('addon-wanxiang', '万象拼音', 'https://github.com/amzxyz/rime-wanxiang'),
        ('addon-rime-mint', '薄荷拼音', 'https://github.com/Mintimate/oh-my-rime'),
        ('addon-rime-frost', '白霜拼音', 'https://github.com/gaboolic/rime-frost'),
        (
          'addon-rime-lssp',
          AppStrings.rimeLssp,
          'https://github.com/arsenali/rime-triple-pinyin-lssp',
        ),
      ]) {
        final card = find.byKey(ValueKey(key));
        final more = find.descendant(
          of: card,
          matching: find.byIcon(FLucideIcons.ellipsis),
        );
        expect(more, findsOneWidget);
        await tester.ensureVisible(more);
        c.actions.clear();
        await tester.tap(more);
        await tester.pumpAndSettle();
        final repository = find.widgetWithText(FItem, '仓库地址');
        expect(repository, findsOneWidget, reason: '$key busy=$busy');
        expect(tester.widget<FItem>(repository).onPress, isNotNull);
        await tester.tap(repository);
        await tester.pumpAndSettle();
        expect(c.actions, ['open:$url']);

        c.actions.clear();
        final heading = find.descendant(of: card, matching: find.text(title));
        await tester.ensureVisible(heading);
        await tester.tap(
          heading,
          kind: PointerDeviceKind.mouse,
          buttons: kSecondaryMouseButton,
        );
        await tester.pumpAndSettle();
        expect(repository, findsOneWidget);
        expect(c.actions, isEmpty);
        await tester.tap(repository);
        await tester.pumpAndSettle();
        expect(c.actions, ['open:$url']);
      }
      Navigator.of(tester.element(find.byType(AppDialog))).pop();
      await tester.pumpAndSettle();
    }
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets('scheme card secondary tap opens its menu without activation', (
    tester,
  ) async {
    final c = _PackageController()
      ..installedPackages = const [
        InstalledPackageDto(
          schemeKey: 'local-custom',
          source: 'local',
          displayName: 'Custom scheme',
          active: false,
          hasPackageZip: true,
        ),
      ];
    addTearDown(c.dispose);
    _size(tester, const Size(1000, 1200));
    for (final busy in [false, true]) {
      c.busy = busy;
      for (final key in {...schemes.keys, 'xmjd-feiding', 'local-custom'}) {
        await _pumpRow(tester, c, key);
        final selected = c.scheme;
        final title = key == 'local-custom'
            ? 'Custom scheme'
            : key.startsWith('xmjd')
            ? schemes['xmjd']!
            : schemeDisplayName(key);
        await tester.tap(
          find.text(title),
          kind: PointerDeviceKind.mouse,
          buttons: kSecondaryMouseButton,
        );
        await tester.pumpAndSettle();
        expect(find.byType(FItem), findsWidgets, reason: '$key busy=$busy');
        expect(
          find.widgetWithText(FItem, '仓库地址'),
          key == 'local-custom' ? findsNothing : findsOneWidget,
        );
        expect(c.scheme, selected);
        expect(c.actions, isEmpty);
        await tester.tap(find.byIcon(FLucideIcons.ellipsis));
        await tester.pumpAndSettle();
        expect(find.byType(FItem), findsNothing);
      }
    }
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  for (final latest in ['1', '2']) {
    testWidgets('Wanxiang variant replacement with latest=$latest', (
      tester,
    ) async {
      final c = _PackageController()
        ..wanxiang = const AddonStatusDto(
          installed: true,
          deployed: true,
          version: '1',
          variant: 'base',
          englishAvailable: false,
        )
        ..wanxiangLatestVersion = latest;
      addTearDown(c.dispose);
      _size(tester, const Size(1000, 1200));
      c.selectWanxiangVariant('pure');
      await _pumpAddons(tester, c);
      expect(
        find.descendant(
          of: find.byKey(const ValueKey('addon-wanxiang')),
          matching: find.widgetWithText(ActionButton, '下载'),
        ),
        findsOneWidget,
      );
      expect(
        find.widgetWithText(ActionButton, AppStrings.update),
        findsNothing,
      );
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets('add-on install controls work without a main scheme', (
    tester,
  ) async {
    final c = _PackageController()
      ..local = const LocalSchemaDto(
        installed: false,
        deployed: false,
        schemas: [],
      );
    c.addon = c.wanxiang;
    addTearDown(c.dispose);
    await _pumpAddons(tester, c);
    expect(find.text(AppStrings.rimeMint), findsOneWidget);
    expect(find.text(AppStrings.rimeFrost), findsOneWidget);
    expect(find.text(AppStrings.rimeSnow), findsNothing);
    expect(find.text(AppStrings.rimeLssp), findsOneWidget);
    final installs = tester.widgetList<ActionButton>(
      find.widgetWithText(ActionButton, '下载'),
    );
    expect(installs, hasLength(6));
    expect(installs.every((button) => button.onPress != null), isTrue);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'input hero identifies built-in packages and standalone add-ons',
    (tester) async {
      final c = _PackageController();
      addTearDown(c.dispose);
      _size(tester, const Size(1200, 1000));
      for (final (schema, name) in [
        ('easy_en', 'Easy English'),
        ('wanxiang', AppStrings.wanxiang),
        ('snow_jiandao', AppStrings.rimeSnow),
        ('triple_pinyin_lssp', AppStrings.rimeLssp),
      ]) {
        c.local = LocalSchemaDto(
          installed: true,
          deployed: true,
          schemas: [schema],
        );
        c.installedPackages = [
          if (schema == 'snow_jiandao')
            InstalledPackageDto(
              schemeKey: schema,
              active: true,
              hasPackageZip: true,
              version: 'standalone',
            ),
        ];
        const status = AddonStatusDto(
          installed: true,
          deployed: true,
          englishAvailable: true,
          version: 'standalone',
        );
        c.addon = status;
        c.wanxiang = status;
        c.rimeAddons['triple_pinyin_lssp']!.status = status;
        await tester.pumpWidget(
          KeyTaoTheme(
            platform: c.info.platform,
            home: FScaffold(child: InputPage(controller: c)),
          ),
        );
        await tester.pumpAndSettle();
        expect(c.activeSchemeKey, schema);
        expect(find.text(name), findsOneWidget);
        expect(find.text('vstandalone'), findsOneWidget);
        expect(tester.takeException(), isNull);
      }
      c.local = const LocalSchemaDto(
        installed: true,
        deployed: true,
        schemas: ['wanxiang', 'keytao', 'easy_en'],
      );
      c.installedPackages = [];
      expect(c.activeSchemeKey, 'keytao');
    },
  );

  testWidgets(
    'empty schemes render the app and first-run setup on all platforms',
    (tester) async {
      _size(tester, const Size(1200, 1000));
      for (final platform in [
        BridgePlatform.macOs,
        BridgePlatform.android,
        BridgePlatform.ios,
      ]) {
        final c = _PackageController(platform)
          ..installedPackages = []
          ..local = const LocalSchemaDto(
            installed: false,
            deployed: false,
            schemas: [],
          );
        c.addon = c.wanxiang;
        if (c.isAndroid) c.storage = {'granted': false, 'writable': false};
        addTearDown(c.dispose);
        for (final completed in [true, false]) {
          await tester.pumpWidget(
            KeyTaoApp(
              key: ValueKey('${platform.name}-$completed'),
              controller: c,
              initialOnboarding: OnboardingDto(completed: completed),
            ),
          );
          await tester.pumpAndSettle();
          expect(c.setupReady, isFalse);
          expect(
            find.byType(OnboardingPage),
            completed ? findsNothing : findsOneWidget,
          );
          if (completed) {
            expect(
              find.descendant(
                of: find.byKey(const ValueKey('current-schema-selector')),
                matching: find.text(AppStrings.schemeMissing),
              ),
              findsOneWidget,
            );
          }
          expect(tester.takeException(), isNull);
          await tester.pumpWidget(const SizedBox.shrink());
          await tester.pumpAndSettle();
        }
      }
    },
  );

  testWidgets('scheme page avoids overflow above a tall landscape keyboard', (
    tester,
  ) async {
    final c = _PackageController(BridgePlatform.ios)
      ..activePage = AppPage.scheme;
    addTearDown(c.dispose);
    _size(tester, const Size(667, 375));
    addTearDown(tester.view.resetViewInsets);
    await tester.pumpWidget(
      KeyTaoApp(
        controller: c,
        initialOnboarding: const OnboardingDto(completed: true),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.byIcon(FLucideIcons.user), findsOneWidget);
    final input = find.byType(EditableText).last;
    await tester.ensureVisible(input);
    await tester.showKeyboard(input);
    tester.view.viewInsets = const FakeViewPadding(bottom: 320);
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
    expect(find.byIcon(FLucideIcons.user), findsNothing);
    expect(tester.widget<EditableText>(input).focusNode.hasFocus, isTrue);
    final viewport = tester.getRect(find.byKey(const PageStorageKey('scheme')));
    expect(viewport.height, greaterThan(0));
    expect(viewport.bottom, lessThanOrEqualTo(55));
    tester.view.resetViewInsets();
    await tester.pumpAndSettle();
    expect(find.byIcon(FLucideIcons.user), findsOneWidget);
    expect(tester.takeException(), isNull);
  }, variant: TargetPlatformVariant.only(TargetPlatform.iOS));

  testWidgets('scheme busy transitions preserve the page scroll offset', (
    tester,
  ) async {
    final c = _PackageController(BridgePlatform.android);
    addTearDown(c.dispose);
    _size(tester, const Size(360, 800));
    await tester.pumpWidget(
      KeyTaoTheme(
        platform: c.info.platform,
        home: FScaffold(
          child: ListenableBuilder(
            listenable: c,
            builder: (_, _) => SchemePage(controller: c),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    final scrollable = find
        .descendant(
          of: find.byKey(const PageStorageKey('scheme')),
          matching: find.byType(Scrollable),
        )
        .first;
    final position = tester.state<ScrollableState>(scrollable).position;
    final source = find.byKey(const ValueKey('scheme-source'));
    final sourceButton = find.descendant(
      of: source,
      matching: find.byType(FButton),
    );
    final label = find
        .descendant(of: sourceButton, matching: find.byType(Text))
        .first;
    Focus.of(tester.element(label)).requestFocus();
    await tester.pumpAndSettle();
    position.jumpTo(60);
    await tester.pumpAndSettle();
    for (final busy in [true, false, true, false]) {
      c.busy = busy;
      c.schemeSyncError = busy ? '上传失败' : null;
      c.rebuild();
      await tester.pumpAndSettle();
      expect(position.pixels, closeTo(60, 0.01));
    }
    final input = find.byType(EditableText).last;
    final inputFocus = tester.widget<EditableText>(input).focusNode;
    await tester.ensureVisible(input);
    await tester.showKeyboard(input);
    tester.view.viewInsets = const FakeViewPadding(bottom: 300);
    addTearDown(tester.view.resetViewInsets);
    await tester.pumpAndSettle();
    position.jumpTo(60);
    await tester.pumpAndSettle();
    expect(inputFocus.hasFocus, isTrue);
    for (final busy in [true, false]) {
      c.busy = busy;
      c.rebuild();
      await tester.pumpAndSettle();
      expect(position.pixels, closeTo(60, 0.01));
    }
    final transfer = showCloudPackageTransfer(
      tester.element(find.byType(SchemePage)),
      c,
      schemeKey: 'keytao',
    );
    await tester.pumpAndSettle();
    expect(position.pixels, closeTo(60, 0.01));
    expect(inputFocus.hasFocus, isFalse);
    await tester.tap(find.widgetWithText(ActionButton, '确认上传'));
    await tester.pump();
    await tester.pump(const Duration(seconds: 1));
    await tester.pumpAndSettle();
    await transfer;
    expect(inputFocus.hasFocus, isFalse);
    expect(position.pixels, closeTo(60, 0.01));
    await tester.showKeyboard(input);
    await tester.pumpAndSettle();
    position.jumpTo(60);
    await tester.pumpAndSettle();
    final account = showAccountDialog(
      tester.element(find.byType(SchemePage)),
      c,
    );
    await tester.pumpAndSettle();
    final download = showCloudPackageTransfer(
      tester.element(find.byType(AppDialog)),
      c,
      schemeKey: 'keytao',
      packageId: 'cloud-keytao',
      totalBytes: 1024,
    );
    await tester.pump();
    await tester.pump(const Duration(seconds: 1));
    await tester.pumpAndSettle();
    await download;
    Navigator.of(tester.element(find.byType(AppDialog))).pop();
    await tester.pumpAndSettle();
    await account;
    expect(inputFocus.hasFocus, isFalse);
    expect(position.pixels, closeTo(60, 0.01));
    c.installedPackages = const [
      InstalledPackageDto(
        schemeKey: 'keytao',
        active: false,
        hasPackageZip: true,
      ),
    ];
    c.rebuild();
    await tester.pumpAndSettle();
    await tester.showKeyboard(input);
    await tester.pumpAndSettle();
    position.jumpTo(60);
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(ActionButton, '使用').first);
    await tester.pumpAndSettle();
    expect(inputFocus.hasFocus, isFalse);
    expect(position.pixels, closeTo(60, 0.01));
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'an active package with an incomplete build is pending deployment',
    (tester) async {
      final c = _PackageController()
        ..installedPackages = const [
          InstalledPackageDto(
            schemeKey: 'keytao',
            version: 'v1',
            active: true,
            hasPackageZip: true,
          ),
        ]
        ..local = const LocalSchemaDto(
          installed: true,
          deployed: false,
          schemas: ['keytao'],
        );
      addTearDown(c.dispose);
      _size(tester, const Size(400, 860));
      await _pumpRow(tester, c, 'keytao');
      expect(find.text('使用中'), findsNothing);
      expect(find.text('待部署'), findsOneWidget);
      expect(
        find.widgetWithText(ActionButton, AppStrings.deploy),
        findsOneWidget,
      );
      const deployError =
          '方案 english：缺少构建产物 build/english.schema.yaml: No such file or directory';
      c.deploySteps.add(const DeployStep(deployError, DeployStepState.failed));
      await _pumpRow(tester, c, 'keytao');
      expect(find.text('使用中'), findsNothing);
      expect(find.text('部署失败'), findsOneWidget);
      expect(find.text(deployError), findsOneWidget);
    },
    variant: TargetPlatformVariant.only(TargetPlatform.macOS),
  );

  testWidgets(
    'a Git scheme keeps its usable build while deployment is queued',
    (tester) async {
      const key = 'local-evo';
      final c = _PackageController(BridgePlatform.windows)
        ..installedPackages = const [
          InstalledPackageDto(
            schemeKey: key,
            active: true,
            hasPackageZip: true,
            source: 'local',
            version: '90fea1d09811',
            displayName: '进化键道',
            git: GitSchemeConfigDto(
              repoUrl: 'https://example.org/evo',
              mode: 'branch',
            ),
          ),
        ]
        ..local = const LocalSchemaDto(
          installed: true,
          deployed: false,
          buildAvailable: true,
          installedScheme: key,
          schemas: ['evo'],
        );
      addTearDown(c.dispose);
      _size(tester, const Size(1200, 1000));
      await _pumpRow(tester, c, key);
      expect(find.widgetWithText(ActionButton, '使用中'), findsOneWidget);
      expect(find.text(AppStrings.pendingDeploy), findsNothing);
      expect(find.text(AppStrings.packageDeployQueued), findsOneWidget);

      c.local = const LocalSchemaDto(
        installed: true,
        deployed: true,
        installedScheme: key,
        schemas: ['evo'],
      );
      c.deploySteps.addAll(const [
        DeployStep(
          'Another package failed',
          DeployStepState.failed,
          packageKey: 'keytao',
        ),
        DeployStep('Deployment failed', DeployStepState.failed),
      ]);
      await _pumpRow(tester, c, key);
      expect(find.widgetWithText(ActionButton, '使用中'), findsOneWidget);
      expect(find.text(AppStrings.deployFailed), findsNothing);

      c.local = const LocalSchemaDto(
        installed: true,
        deployed: false,
        buildAvailable: true,
        installedScheme: key,
        schemas: ['evo'],
      );
      await tester.pumpWidget(
        KeyTaoTheme(
          platform: c.info.platform,
          home: FScaffold(child: InputPage(controller: c)),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.widgetWithText(AppBadge, AppStrings.deployed), findsNothing);
      expect(
        find.descendant(
          of: find.byKey(const ValueKey('current-schema-selector')),
          matching: find.text(AppStrings.packageDeployQueued),
        ),
        findsOneWidget,
      );
    },
  );

  testWidgets(
    'a dirty build automatically deploys and keeps switches available',
    (tester) async {
      final core = _SchemaSwitchCore()
        ..deployGate = Completer<void>()
        ..local = const LocalSchemaDto(
          installed: true,
          deployed: false,
          buildAvailable: true,
          installedScheme: 'local-jd-v3-6-6',
          schemas: ['JD6'],
        );
      RustLib.initMock(api: core);
      final c = AppController(
        info: const BridgeInfo(
          platform: BridgePlatform.macOs,
          appVersion: 'test',
          userRoot: '/isolated/packages',
        ),
      );
      addTearDown(() async {
        c.dispose();
        await core.events.close();
        RustLib.dispose();
      });

      await c.refreshState();
      await core.deployStarted.future;
      expect(c.isDeploying, isTrue);
      expect(c.local?.deployed, isFalse);
      expect(c.schemaSwitches.map((entry) => entry.name), ['jffh']);
      _size(tester, const Size(1000, 900));
      await _pumpSchemePage(tester, c, settle: false);
      expect(find.text('方案开关'), findsOneWidget);
      final setting = tester.widget<SwitchSetting>(
        find.widgetWithText(SwitchSetting, '繁體'),
      );
      expect(setting.onChanged, isNull);
      core.deployGate!.complete();
      await tester.pumpAndSettle();
      expect(c.local?.deployed, isTrue);
      expect(c.isDeploying, isFalse);
      expect(core.calls.where((call) => call == 'deployDefault'), hasLength(1));
      expect(
        tester
            .widget<SwitchSetting>(find.widgetWithText(SwitchSetting, '繁體'))
            .onChanged,
        isNotNull,
      );
    },
  );

  testWidgets('package cards keep their own logo throughout deployment', (
    tester,
  ) async {
    final c = _PackageController()
      ..local = const LocalSchemaDto(
        installed: true,
        deployed: true,
        schemas: ['xmjd6'],
        installedScheme: 'xmjd',
      )
      ..activeSchemaLogoPath = 'assets/logo.png';
    addTearDown(c.dispose);
    _size(tester, const Size(1200, 800));
    for (final logoPath in [File('assets/logo.png').absolute.path, null]) {
      c.installedPackages = [
        InstalledPackageDto(
          schemeKey: 'xmjd',
          active: true,
          hasPackageZip: true,
          logoPath: logoPath,
        ),
      ];
      for (final operation in [null, OperationKind.deploy, null]) {
        c.operation = operation;
        c.deployingSchemeKey = operation == OperationKind.deploy
            ? 'xmjd'
            : null;
        c.busy = operation != null;
        c.progress = AppStrings.deployingLibrime;
        await tester.pumpWidget(
          KeyTaoTheme(
            platform: c.info.platform,
            home: FScaffold(
              child: SchemePackageRow(controller: c, schemeKey: 'xmjd'),
            ),
          ),
        );
        final button = tester.widget<ActionButton>(
          find.widgetWithText(
            ActionButton,
            operation == OperationKind.deploy ? AppStrings.deploying : '使用中',
          ),
        );
        expect(button.onPress, isNull);
        expect(button.success, operation != OperationKind.deploy);
        expect(find.byType(AppBadge), findsNothing);
        expect(
          find.text(AppStrings.deployingLibrime),
          operation == OperationKind.deploy ? findsOneWidget : findsNothing,
        );
        c.deployingSchemeKey = 'keytao';
        await tester.pumpWidget(
          KeyTaoTheme(
            platform: c.info.platform,
            home: FScaffold(
              child: SchemePackageRow(controller: c, schemeKey: 'xmjd'),
            ),
          ),
        );
        expect(find.widgetWithText(ActionButton, '使用中'), findsOneWidget);
        expect(
          find.widgetWithText(ActionButton, AppStrings.deploying),
          findsNothing,
        );
        expect(find.text(AppStrings.deployingLibrime), findsNothing);
        final logo = find.byType(SchemaLogo);
        expect(logo, logoPath == null ? findsNothing : findsOneWidget);
        if (logoPath != null) {
          final image = tester.widget<Image>(
            find.descendant(of: logo, matching: find.byType(Image)),
          );
          expect((image.image as FileImage).file.path, logoPath);
        }
      }
    }
  });

  testWidgets('schema logos use the global logo only without a package key', (
    tester,
  ) async {
    final c = _PackageController()
      ..local = const LocalSchemaDto(
        installed: true,
        deployed: true,
        installedScheme: 'local-moran',
        schemas: ['moran'],
      )
      ..activeSchemaLogoPath = 'assets/logo.png'
      ..installedPackages = const [
        InstalledPackageDto(
          schemeKey: 'local-moran',
          active: true,
          hasPackageZip: true,
        ),
      ];
    addTearDown(c.dispose);
    for (final schemeKey in ['local-moran', 'missing', null]) {
      await tester.pumpWidget(
        KeyTaoTheme(
          platform: c.info.platform,
          home: SchemaLogo(
            controller: c,
            size: 32,
            schemeKey: schemeKey,
            appLogoFallback: false,
          ),
        ),
      );
      expect(
        find.byType(Image),
        schemeKey == null ? findsOneWidget : findsNothing,
      );
      if (schemeKey == null) {
        final image = tester.widget<Image>(find.byType(Image));
        expect((image.image as FileImage).file.path, c.activeSchemaLogoPath);
      }
    }
  });

  testWidgets('a logo replacement keeps its image element mounted', (
    tester,
  ) async {
    final c = _PackageController(BridgePlatform.windows)
      ..activeSchemaLogoPath = 'assets/logo.png';
    addTearDown(c.dispose);
    Future<void> pumpLogo() => tester.pumpWidget(
      KeyTaoTheme(
        platform: c.info.platform,
        home: SchemaLogo(controller: c, size: 32),
      ),
    );
    await pumpLogo();
    final logo = find.byType(Image);
    final element = tester.element(logo);
    c.activeSchemaLogoPath = File('assets/logo.png').absolute.path;
    await pumpLogo();

    expect(tester.element(logo), same(element));
    expect(tester.widget<Image>(logo).gaplessPlayback, isTrue);
  });

  testWidgets('uninstalled scheme status occupies the version row', (
    tester,
  ) async {
    final c = _PackageController();
    addTearDown(c.dispose);
    await tester.pumpWidget(
      KeyTaoTheme(
        platform: c.info.platform,
        home: FScaffold(
          child: SchemePackageRow(controller: c, schemeKey: 'txjx'),
        ),
      ),
    );
    expect(
      tester.getCenter(find.text('未安装')).dy,
      closeTo(tester.getCenter(find.text('云端')).dy, 1),
    );
  });

  testWidgets('package rows expose only applicable actions', (tester) async {
    final c = _PackageController();
    addTearDown(c.dispose);
    _size(tester, const Size(360, 800));
    final cases = [
      (package: null, update: false, labels: ['下载'], calls: ['install:keytao']),
      (
        package: const InstalledPackageDto(
          schemeKey: 'keytao',
          version: 'v1',
          active: true,
          inUse: false,
          hasPackageZip: true,
        ),
        update: false,
        labels: ['使用', '卸载'],
        calls: ['activate:keytao', 'remove:keytao'],
      ),
      (
        package: const InstalledPackageDto(
          schemeKey: 'keytao',
          version: 'v1',
          active: true,
          hasPackageZip: true,
        ),
        update: false,
        labels: ['卸载'],
        calls: ['remove:keytao'],
      ),
      (
        package: const InstalledPackageDto(
          schemeKey: 'keytao',
          version: 'v1',
          active: true,
          hasPackageZip: false,
        ),
        update: false,
        labels: ['卸载'],
        calls: ['remove:keytao'],
      ),
      (
        package: const InstalledPackageDto(
          schemeKey: 'keytao',
          version: 'v1',
          active: false,
          hasPackageZip: true,
        ),
        update: false,
        labels: ['使用', '卸载'],
        calls: ['activate:keytao', 'remove:keytao'],
      ),
      (
        package: const InstalledPackageDto(
          schemeKey: 'keytao',
          version: 'v1',
          active: false,
          hasPackageZip: false,
        ),
        update: false,
        labels: ['下载', '卸载'],
        calls: ['download:keytao', 'remove:keytao'],
      ),
      (
        package: const InstalledPackageDto(
          schemeKey: 'keytao',
          version: 'v1',
          active: false,
          hasPackageZip: true,
        ),
        update: true,
        labels: ['更新', '使用', '卸载'],
        calls: ['update:keytao', 'activate:keytao', 'remove:keytao'],
      ),
      (
        package: const InstalledPackageDto(
          schemeKey: 'keytao',
          version: 'v1',
          active: true,
          hasPackageZip: true,
        ),
        update: true,
        labels: ['更新', '卸载'],
        calls: ['update:keytao', 'remove:keytao'],
      ),
      (
        package: const InstalledPackageDto(
          schemeKey: 'keytao',
          version: 'v1',
          active: true,
          hasPackageZip: false,
        ),
        update: true,
        labels: ['更新', '卸载'],
        calls: ['update:keytao', 'remove:keytao'],
      ),
    ];
    for (final entry in cases) {
      c
        ..busy = false
        ..installedPackages = [if (entry.package != null) entry.package!]
        ..schemeUpdates = [if (entry.update) _update];
      c.actions.clear();
      await _pumpRow(tester, c, 'keytao');
      final row = find.byType(SchemePackageRow);
      final title = find.text('键道6');
      expect(
        tester
            .widget<FButton>(
              find.ancestor(of: title, matching: find.byType(FButton)),
            )
            .variant,
        FButtonVariant.ghost,
      );
      expect(
        find.descendant(of: row, matching: find.byType(SchemaLogo)),
        findsNothing,
      );
      expect(tester.getTopLeft(title).dx, tester.getTopLeft(row).dx);
      expect(
        find.descendant(of: row, matching: find.byType(ActionRow)),
        findsOneWidget,
      );
      expect(
        _actionLabels(tester),
        entry.labels.where((label) => label != '卸载'),
      );
      for (final button in tester.widgetList<ActionButton>(
        find.byType(ActionButton),
      )) {
        if (button.label == '下载') expect(button.primary, isFalse);
        if (button.label == '使用中') {
          expect(button.success, isTrue);
          expect(button.onPress, isNull);
        }
      }
      expect(find.text('自造词'), findsNothing);
      expect(find.text('卸载'), findsNothing);
      expect(
        find.text('使用中'),
        entry.package?.active == true && entry.package!.inUse
            ? findsOneWidget
            : findsNothing,
      );
      if (entry.package != null) {
        expect(find.text('v1'), findsOneWidget);
        expect(
          tester.getBottomLeft(find.text('v1')).dy,
          lessThan(tester.getTopLeft(title).dy),
        );
        expect(
          tester.getTopLeft(find.byType(ActionRow)).dy,
          greaterThan(tester.getBottomLeft(find.text('v1')).dy),
        );
      }
      if (entry.update) {
        expect(find.text('→ v2'), findsOneWidget);
        expect(
          tester
              .widget<ActionButton>(find.widgetWithText(ActionButton, '更新'))
              .primary,
          isTrue,
        );
      }
      for (final label in entry.labels) {
        if (label == '卸载') {
          await tester.tap(find.byIcon(FLucideIcons.ellipsis));
          await tester.pumpAndSettle();
          await tester.tap(find.widgetWithText(FItem, label));
        } else {
          await tester.tap(find.widgetWithText(ActionButton, label));
        }
        await tester.pumpAndSettle();
      }
      expect(c.actions, entry.calls);
      if (entry.package?.active == true) {
        await tester.tap(find.byIcon(FLucideIcons.ellipsis));
        await tester.pumpAndSettle();
        expect(find.text('自造词'), findsOneWidget);
        expect(find.text('卸载'), findsOneWidget);
        expect(find.widgetWithText(FItem, '重新部署'), findsOneWidget);
        if (!entry.package!.hasPackageZip && !entry.update) {
          expect(find.widgetWithText(FItem, '下载'), findsOneWidget);
          await tester.tap(find.widgetWithText(FItem, '下载'));
          await tester.pumpAndSettle();
          expect(c.actions.last, 'download:keytao');
        } else {
          await tester.tap(find.byIcon(FLucideIcons.ellipsis));
        }
        await tester.pumpAndSettle();
      }
      c.busy = true;
      await _pumpRow(tester, c, 'keytao');
      expect(
        tester.widgetList<ActionButton>(find.byType(ActionButton)),
        everyElement(
          isA<ActionButton>().having((b) => b.onPress, 'onPress', null),
        ),
      );
      if (entry.package != null) {
        await tester.tap(find.byIcon(FLucideIcons.ellipsis));
        await tester.pumpAndSettle();
        expect(tester.widgetList<FItem>(find.byType(FItem)), isNotEmpty);
        final repository = tester.widget<FItem>(
          find.widgetWithText(FItem, '仓库地址'),
        );
        final directory = tester.widget<FItem>(
          find.widgetWithText(FItem, '打开方案目录'),
        );
        expect(repository.onPress, isNotNull);
        expect(directory.onPress, isNotNull);
        final changelog = tester
            .widgetList<FItem>(find.widgetWithText(FItem, AppStrings.changelog))
            .firstOrNull;
        if (changelog != null) expect(changelog.onPress, isNotNull);
        expect(
          tester
              .widgetList<FItem>(find.byType(FItem))
              .where(
                (item) =>
                    item != repository &&
                    item != directory &&
                    item != changelog,
              ),
          everyElement(
            isA<FItem>().having((item) => item.onPress, 'onPress', isNull),
          ),
        );
        await tester.tap(find.byIcon(FLucideIcons.ellipsis));
        await tester.pumpAndSettle();
      }
      expect(tester.takeException(), isNull);
    }
    await tester.pumpWidget(const SizedBox.shrink());
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets(
    'package menu opens when busy changes between pointer down and up',
    (tester) async {
      final c = _PackageController(BridgePlatform.windows)
        ..accountUser = const AccountUserDto(id: 1)
        ..installedPackages = const [
          InstalledPackageDto(
            schemeKey: 'local-custom',
            source: 'local',
            active: false,
            hasPackageZip: true,
            supportsUserWords: false,
          ),
        ];
      addTearDown(c.dispose);
      _size(tester, const Size(360, 800));
      await tester.pumpWidget(
        KeyTaoTheme(
          platform: c.info.platform,
          home: FScaffold(
            child: ListenableBuilder(
              listenable: c,
              builder: (_, _) =>
                  SchemePackageRow(controller: c, schemeKey: 'local-custom'),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.byType(FItem), findsNothing);
      final gesture = await tester.startGesture(
        tester.getCenter(find.byIcon(FLucideIcons.ellipsis)),
      );
      c.busy = true;
      c.rebuild();
      await tester.pump();
      await gesture.up();
      await tester.pumpAndSettle();
      expect(find.byType(FItem), findsNWidgets(5));
      expect(find.text('打开方案目录'), findsOneWidget);
      final directory = tester.widget<FItem>(
        find.widgetWithText(FItem, '打开方案目录'),
      );
      expect(directory.onPress, isNotNull);
      expect(
        tester
            .widgetList<FItem>(find.byType(FItem))
            .where((item) => item != directory),
        everyElement(
          isA<FItem>().having((item) => item.onPress, 'onPress', isNull),
        ),
      );
      expect(c.actions, isEmpty);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
    variant: TargetPlatformVariant.only(TargetPlatform.windows),
  );

  testWidgets('custom words action follows package support', (tester) async {
    final c = _PackageController();
    addTearDown(c.dispose);
    _size(tester, const Size(360, 800));
    for (final active in [false, true]) {
      for (final supportsUserWords in [false, true]) {
        c.installedPackages = [
          InstalledPackageDto(
            schemeKey: 'keydo',
            active: active,
            hasPackageZip: true,
            supportsUserWords: supportsUserWords,
          ),
        ];
        await _pumpRow(tester, c, 'keydo');
        await tester.tap(find.byIcon(FLucideIcons.ellipsis));
        await tester.pumpAndSettle();
        expect(find.text('仓库地址'), findsOneWidget);
        expect(
          find.text('自造词'),
          supportsUserWords ? findsOneWidget : findsNothing,
        );
        await tester.tap(find.byIcon(FLucideIcons.ellipsis));
        await tester.pumpAndSettle();
        expect(tester.takeException(), isNull);
      }
    }
    await tester.pumpWidget(const SizedBox.shrink());
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  for (final (width, platform) in [
    (1200.0, BridgePlatform.macOs),
    (760.0, BridgePlatform.macOs),
    (360.0, BridgePlatform.macOs),
    (360.0, BridgePlatform.android),
  ]) {
    testWidgets(
      'add-on dialog header and responsive grid at $width on ${platform.name}',
      (tester) async {
        final c = _PackageController(platform);
        c.addon = c.rimeAddons['rime_ice']!.status;
        addTearDown(c.dispose);
        _size(tester, Size(width, 1200));
        await _pumpSchemePage(tester, c);
        final source = find.byKey(const ValueKey('scheme-source'));
        final button = find.byKey(const ValueKey('addon-schemes-button'));
        final refresh = find.byWidgetPredicate(
          (widget) =>
              widget is Icon && widget.semanticLabel == AppStrings.checkUpdate,
        );
        expect(tester.widget<ActionButton>(button).label, '附加方案');
        expect(
          tester.getRect(source).right,
          lessThanOrEqualTo(tester.getRect(button).left),
        );
        expect(
          tester.getRect(button).left -
              tester
                  .getRect(
                    find.descendant(
                      of: source,
                      matching: find.textContaining('来源 ·'),
                    ),
                  )
                  .right,
          inInclusiveRange(8, 32),
        );
        expect(
          tester.getRect(button).right,
          lessThanOrEqualTo(tester.getRect(refresh).left),
        );
        expect(tester.getCenter(button).dy, tester.getCenter(refresh).dy);
        expect(tester.getCenter(source).dy, tester.getCenter(button).dy);
        if (width == 1200) {
          for (final key in [
            'keytao',
            'xmjd',
            'txjx',
            'keydo',
            'snow_jiandao',
          ]) {
            expect(
              find.descendant(
                of: find.byKey(ValueKey('scheme-package-$key')),
                matching: find.widgetWithText(ActionButton, '下载'),
              ),
              findsOneWidget,
            );
          }
          for (final key in ['rime-ice']) {
            expect(
              find.descendant(
                of: find.byKey(ValueKey('addon-$key')),
                matching: find.widgetWithText(ActionButton, '下载'),
              ),
              findsOneWidget,
            );
          }
          for (final button in tester.widgetList<ActionButton>(
            find.widgetWithText(ActionButton, '下载'),
          )) {
            expect(button.primary, isFalse);
          }
        }
        await _openAddons(tester);
        final grid = find.byKey(const ValueKey('addon-schemes'));
        final cards = [
          for (final key in [
            'wanxiang',
            'rime-mint',
            'rime-frost',
            'rime-lssp',
          ])
            find.descendant(
              of: grid,
              matching: find.byKey(ValueKey('addon-$key')),
            ),
        ];
        expect(tester.widget<HairlineGridSection>(grid).children, hasLength(4));
        for (final card in cards) {
          expect(
            find.descendant(
              of: card,
              matching: find.widgetWithText(ActionButton, '下载'),
            ),
            findsOneWidget,
          );
        }
        for (final key in ['rime-ice', 'easy-en']) {
          expect(
            find.descendant(
              of: grid,
              matching: find.byKey(ValueKey('addon-$key')),
            ),
            findsNothing,
          );
        }
        final tops = cards.map(tester.getTopLeft).toList();
        final columns = tops.map((top) => top.dx).toSet();
        final rows = tops.map((top) => top.dy).toSet();
        if (width == 1200) {
          expect(rows, hasLength(1));
          expect(columns, hasLength(4));
          for (var index = 1; index < cards.length; index++) {
            expect(
              tester.getTopLeft(cards[index]).dy,
              tester.getTopLeft(cards.first).dy,
            );
            expect(
              tester.getTopLeft(cards[index]).dx,
              greaterThan(tester.getTopLeft(cards[index - 1]).dx),
            );
          }
        } else {
          expect(rows.length, greaterThan(1));
          expect(columns.length, lessThan(5));
        }
        expect(tester.takeException(), isNull);
        await tester.pumpWidget(const SizedBox.shrink());
      },
    );
  }

  for (final (key, action, operation) in [
    ('rime-ice', 'rime_ice', OperationKind.rimeIce),
    ('easy-en', 'easy', OperationKind.addon),
  ]) {
    testWidgets('home $key card uses package badges and retains operations', (
      tester,
    ) async {
      final c = _PackageController();
      addTearDown(c.dispose);
      _size(tester, const Size(1400, 1000));
      final card = find.byKey(ValueKey('addon-$key'));
      final scheme = key == 'easy-en' ? 'easy_en' : 'rime_ice';
      for (final (installed, deployed, active, latest, status) in [
        (false, false, false, '1', '未安装'),
        (true, true, false, '1', null),
        (true, false, false, '1', AppStrings.pendingDeploy),
        (true, true, false, '2', '可更新'),
        (true, false, true, '2', '可更新'),
        (true, true, true, '2', '使用中'),
      ]) {
        final info = AddonStatusDto(
          installed: installed,
          deployed: deployed,
          version: installed ? '1' : '',
          englishAvailable: false,
        );
        if (key == 'easy-en') {
          c.addon = info;
          c.addonLatestVersion = latest;
        } else {
          c.rimeAddons['rime_ice']!
            ..status = info
            ..latestVersion = latest;
        }
        final overlay = key == 'easy-en';
        c.isolatedPackages = [
          if (installed && !overlay)
            IsolatedPackageDto(
              packageKey: scheme,
              displayName: scheme,
              active: active,
              ready: deployed,
            ),
        ];
        c.local = LocalSchemaDto(
          installed: true,
          deployed: true,
          schemas: [active ? scheme : 'keytao'],
          installedScheme: overlay ? 'keytao' : null,
          easyEnglishEnabled: overlay ? active : null,
        );
        await _pumpSchemePage(tester, c);
        expect(
          find.descendant(
            of: card,
            matching: find.text(!overlay && active && deployed ? '在用' : '云端'),
          ),
          findsOneWidget,
        );
        final inUse = !overlay && active && deployed;
        if (!installed) {
          final source = find.descendant(of: card, matching: find.text('云端'));
          final status = find.descendant(of: card, matching: find.text('未安装'));
          expect(
            tester.getCenter(status).dy,
            closeTo(tester.getCenter(source).dy, 1),
          );
        }
        expect(
          find.descendant(of: card, matching: find.byType(AppBadge)),
          findsNothing,
        );
        final badge = find.descendant(
          of: card,
          matching: find.byType(StatusPill),
        );
        if (inUse || status == null) {
          expect(badge, findsNothing);
        } else {
          expect(
            find.descendant(
              of: badge,
              matching: find.text(overlay && status == '使用中' ? '可更新' : status),
            ),
            findsOneWidget,
          );
        }
        if (!installed) {
          final button = tester.widget<ActionButton>(
            find.descendant(
              of: card,
              matching: find.widgetWithText(ActionButton, AppStrings.download),
            ),
          );
          expect(button.primary, isFalse);
          expect(button.icon, FLucideIcons.download);
          button.onPress!();
          expect(c.actions.last, '$action:true');
        } else if (overlay) {
          final button = tester.widget<ActionButton>(
            find.descendant(
              of: card,
              matching: find.widgetWithText(
                ActionButton,
                active ? '从当前方案移除' : '加入当前方案',
              ),
            ),
          );
          button.onPress!();
          expect(c.actions.last, 'overlay:keytao:${!active}');
        } else {
          final button = tester.widget<ActionButton>(
            find.descendant(
              of: card,
              matching: find.widgetWithText(ActionButton, inUse ? '使用中' : '使用'),
            ),
          );
          expect(button.success, inUse);
          expect(button.onPress == null, inUse);
          if (!inUse) {
            button.onPress!();
            expect(c.actions.last, 'activate:$scheme');
          }
        }
        expect(
          find.descendant(
            of: card,
            matching: find.widgetWithText(ActionButton, '部署'),
          ),
          findsNothing,
        );
        if (status != null) {
          expect(
            find.descendant(of: card, matching: find.text('$status · 1')),
            findsNothing,
          );
        }
        expect(
          find.descendant(of: card, matching: find.text('1')),
          installed ? findsOneWidget : findsNothing,
        );
      }
      c.actions.clear();
      await tester.ensureVisible(card);
      await tester.tap(
        find.descendant(
          of: card,
          matching: find.widgetWithText(ActionButton, '更新'),
        ),
      );
      if (key == 'easy-en') {
        await tester.tap(
          find.descendant(
            of: card,
            matching: find.widgetWithText(ActionButton, '从当前方案移除'),
          ),
        );
      } else {
        expect(find.text(AppStrings.uninstall), findsNothing);
        await tester.tap(
          find.descendant(
            of: card,
            matching: find.byIcon(FLucideIcons.ellipsis),
          ),
        );
        await tester.pumpAndSettle();
        await tester.tap(find.widgetWithText(FItem, AppStrings.uninstall));
        await tester.pumpAndSettle();
      }
      expect(c.actions, [
        '$action:true',
        key == 'easy-en' ? 'overlay:keytao:false' : '$action:false',
      ]);
      c
        ..busy = true
        ..operation = operation;
      const progress = InstallProgressDto(
        stage: 'download',
        percent: 40,
        message: 'Downloading 40%',
      );
      if (key == 'easy-en') {
        c.addonProgress = progress;
        c.addonError = 'Download failed';
      } else {
        c.rimeAddons['rime_ice']!
          ..progress = progress
          ..error = 'Download failed';
      }
      c.rebuild();
      await tester.pumpAndSettle();
      expect(
        find.descendant(of: card, matching: find.byType(FDeterminateProgress)),
        findsOneWidget,
      );
      expect(
        find.descendant(of: card, matching: find.text('Download failed')),
        findsOneWidget,
      );
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets('future cloud add-ons appear only in the dialog', (tester) async {
    final c = _PackageController()
      ..installedPackages = const [
        InstalledPackageDto(
          schemeKey: 'future-addon',
          source: 'cloud',
          displayName: 'Future add-on',
          active: false,
          hasPackageZip: true,
        ),
        InstalledPackageDto(
          schemeKey: 'future-addon-2',
          source: 'cloud',
          displayName: 'Future add-on 2',
          active: false,
          hasPackageZip: true,
        ),
      ];
    addTearDown(c.dispose);
    _size(tester, const Size(1200, 1000));
    await _pumpSchemePage(tester, c);
    expect(
      find.byKey(const ValueKey('scheme-package-future-addon')),
      findsNothing,
    );
    expect(
      tester
          .widget<HairlineGridSection>(
            find.byKey(const ValueKey('cloud-schemes')),
          )
          .children,
      hasLength(7),
    );
    await _openAddons(tester);
    final cells = find.descendant(
      of: find.byKey(const ValueKey('addon-schemes')),
      matching: find.byType(HairlineGridCell),
    );
    final tops = [
      for (final cell in tester.widgetList<HairlineGridCell>(cells))
        tester.getTopLeft(find.byWidget(cell)),
    ];
    final columns = tops.map((top) => top.dx).toSet();
    final rows = tops.map((top) => top.dy).toSet();
    expect(rows, hasLength(2));
    expect(columns, hasLength(5));
    expect(
      find.byKey(const ValueKey('scheme-package-future-addon-2')),
      findsOneWidget,
    );
    final card = find.byKey(const ValueKey('scheme-package-future-addon'));
    expect(card, findsOneWidget);
    await tester.ensureVisible(card);
    await tester.tap(find.descendant(of: card, matching: find.text('使用')));
    await tester.pumpAndSettle();
    expect(c.actions, ['activate:future-addon']);
    expect(tester.takeException(), isNull);
  });

  testWidgets('dialog retains variants, grammar, progress and errors', (
    tester,
  ) async {
    final c = _PackageController()
      ..wanxiang = const AddonStatusDto(
        installed: true,
        deployed: true,
        version: '1',
        variant: 'base',
        englishAvailable: false,
      )
      ..wanxiangGrammar = WanxiangGrammarStatusDto(
        available: true,
        installed: false,
        version: '',
        size: BigInt.from(1024),
      );
    addTearDown(c.dispose);
    _size(tester, const Size(1200, 1000));
    await _pumpSchemePage(tester, c);
    await _openAddons(tester);
    final variant = find.byWidgetPredicate(
      (widget) => widget is FSelect<String>,
    );
    expect(find.text('基础版'), findsOneWidget);
    await tester.tap(variant);
    await tester.pumpAndSettle();
    await tester.tap(find.text('纯净').last);
    await tester.pumpAndSettle();
    expect(c.selectedWanxiangVariant, 'pure');
    final grammar = find.byKey(const ValueKey('wanxiang-grammar'));
    await tester.tap(find.descendant(of: grammar, matching: find.text('下载')));
    await tester.pumpAndSettle();
    expect(c.actions, ['grammar:true']);
    const progress = InstallProgressDto(
      stage: 'download',
      percent: 40,
      message: 'Downloading 40%',
    );
    c
      ..busy = true
      ..wanxiangProgress = progress
      ..wanxiangError = 'Download failed';
    for (final id in ['rime_mint', 'rime_frost', 'triple_pinyin_lssp']) {
      c.rimeAddons[id]!
        ..progress = progress
        ..error = 'Download failed';
    }
    for (final (operation, key) in [
      (OperationKind.wanxiang, 'wanxiang'),
      (OperationKind.rimeMint, 'rime-mint'),
      (OperationKind.rimeFrost, 'rime-frost'),
      (OperationKind.rimeLssp, 'rime-lssp'),
    ]) {
      c.operation = operation;
      c.rebuild();
      await tester.pumpAndSettle();
      final card = find.byKey(ValueKey('addon-$key'));
      expect(
        find.descendant(of: card, matching: find.byType(FDeterminateProgress)),
        findsOneWidget,
      );
      expect(
        find.descendant(of: card, matching: find.text('Download failed')),
        findsOneWidget,
      );
    }
    expect(tester.takeException(), isNull);
  });

  for (final layout in [
    (width: 1400.0, columns: 5),
    (width: 1200.0, columns: 4),
    (width: 900.0, columns: 3),
    (width: 360.0, columns: 1),
  ]) {
    testWidgets('home scheme grid fits or collapses at ${layout.width}', (
      tester,
    ) async {
      final c = _PackageController();
      addTearDown(c.dispose);
      _size(tester, Size(layout.width, 1000));
      await _pumpSchemePage(tester, c);
      final catalogue = find.byKey(const ValueKey('cloud-schemes'));
      final cells = find.descendant(
        of: catalogue,
        matching: find.byType(HairlineGridCell),
      );
      Set<double> rowTops() => {
        for (final cell in tester.widgetList<HairlineGridCell>(cells))
          tester.getTopLeft(find.byWidget(cell)).dy,
      };

      final columns = {
        for (final cell in tester.widgetList<HairlineGridCell>(cells))
          tester.getTopLeft(find.byWidget(cell)).dx,
      };
      expect(columns, hasLength(layout.columns));
      final expand = find.bySemanticsLabel('展开全部方案');
      final collapsible = layout.columns > 1 && 7 > layout.columns * 2;
      if (!collapsible) {
        expect(
          tester.widget<HairlineGridSection>(catalogue).children,
          hasLength(7),
        );
        expect(
          rowTops(),
          hasLength((7 / layout.columns).ceil()),
        );
        expect(expand, findsNothing);
      } else {
        final visibleCount = layout.columns * 2;
        expect(
          tester.widget<HairlineGridSection>(catalogue).children,
          hasLength(visibleCount),
        );
        expect(rowTops(), hasLength(2));
        expect(find.text('+${7 - visibleCount + 1}'), findsOneWidget);
        expect(find.byKey(const ValueKey('addon-rime-lssp')), findsNothing);
        expect(
          tester.getSize(expand).width,
          closeTo(tester.getSize(catalogue).width / layout.columns, .01),
        );
        await tester.ensureVisible(expand);
        await tester.tap(expand);
        await tester.pumpAndSettle();
        expect(
          tester.widget<HairlineGridSection>(catalogue).children,
          hasLength(8),
        );
        expect(
          rowTops(),
          hasLength((8 / layout.columns).ceil()),
        );
      }
      expect(
        find.descendant(of: catalogue, matching: find.byType(SchemePackageRow)),
        findsNWidgets(5),
      );
      for (final key in ['wanxiang', 'rime-mint', 'rime-frost', 'rime-lssp']) {
        expect(find.byKey(ValueKey('addon-$key')), findsNothing);
      }
      expect(find.byKey(const ValueKey('addon-rime-ice')), findsOneWidget);
      expect(find.byKey(const ValueKey('addon-easy-en')), findsOneWidget);
      expect(
        tester
            .widgetList<SchemePackageRow>(
              find.descendant(
                of: catalogue,
                matching: find.byType(SchemePackageRow),
              ),
            )
            .map((row) => row.schemeKey),
        ['keytao', 'xmjd', 'txjx', 'keydo', 'snow_jiandao'],
      );
      if (collapsible) {
        final collapse = find.bySemanticsLabel('收起');
        expect(collapse, findsOneWidget);
        await tester.ensureVisible(collapse);
        await tester.tap(collapse);
        await tester.pumpAndSettle();
        expect(rowTops(), hasLength(2));
      }
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    });
  }

  testWidgets('collapsed scheme grid keeps the active hidden package visible', (
    tester,
  ) async {
    final c = _PackageController()
      ..installedPackages = const [
        InstalledPackageDto(
          schemeKey: 'snow_jiandao',
          active: true,
          hasPackageZip: true,
        ),
      ];
    addTearDown(c.dispose);
    _size(tester, const Size(720, 1000));
    await _pumpSchemePage(tester, c);
    final catalogue = find.byKey(const ValueKey('cloud-schemes'));
    final rows = find.descendant(
      of: catalogue,
      matching: find.byType(SchemePackageRow),
    );
    expect(
      tester.widgetList<SchemePackageRow>(rows).map((row) => row.schemeKey),
      ['keytao', 'xmjd', 'snow_jiandao'],
    );
    expect(find.byKey(const ValueKey('scheme-package-txjx')), findsNothing);
    expect(find.text('+4'), findsOneWidget);
    final expand = find.bySemanticsLabel('展开全部方案');
    await tester.ensureVisible(expand);
    await tester.tap(expand);
    await tester.pumpAndSettle();
    expect(
      tester.widgetList<SchemePackageRow>(rows).map((row) => row.schemeKey),
      ['keytao', 'xmjd', 'txjx', 'keydo', 'snow_jiandao'],
    );
    final collapse = find.bySemanticsLabel('收起');
    expect(tester.getSize(collapse).height, greaterThan(0));
    await tester.ensureVisible(collapse);
    await tester.tap(collapse);
    await tester.pumpAndSettle();
    expect(
      find.byKey(const ValueKey('scheme-package-snow_jiandao')),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('scheme-package-txjx')), findsNothing);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  for (final (width, hiddenCount) in [(900.0, 2), (720.0, 4)]) {
    for (final (scheme, key) in [
      ('rime_ice', 'rime-ice'),
      ('easy_en', 'easy-en'),
    ]) {
      testWidgets('collapsed grid keeps active $scheme visible at $width', (
        tester,
      ) async {
        final c = _PackageController()
          ..local = LocalSchemaDto(
            installed: true,
            deployed: true,
            schemas: [scheme],
          );
        addTearDown(c.dispose);
        _size(tester, Size(width, 1000));
        await _pumpSchemePage(tester, c);
        final card = find.byKey(ValueKey('addon-$key'));
        expect(card, findsOneWidget);
        expect(find.text('+$hiddenCount'), findsOneWidget);
        final expand = find.bySemanticsLabel('展开全部方案');
        await tester.ensureVisible(expand);
        await tester.tap(expand);
        await tester.pumpAndSettle();
        expect(find.byKey(const ValueKey('addon-rime-ice')), findsOneWidget);
        expect(find.byKey(const ValueKey('addon-easy-en')), findsOneWidget);
        final collapse = find.bySemanticsLabel('收起');
        await tester.ensureVisible(collapse);
        await tester.tap(collapse);
        await tester.pumpAndSettle();
        expect(card, findsOneWidget);
        expect(find.text('+$hiddenCount'), findsOneWidget);
        expect(tester.takeException(), isNull);
      });
    }
  }

  testWidgets('active add-ons stay in their assigned home or dialog grid', (
    tester,
  ) async {
    final c = _PackageController();
    addTearDown(c.dispose);
    _size(tester, const Size(1400, 1000));
    for (final (schema, addon) in [
      ('easy_en', 'easy-en'),
      ('wanxiang', 'wanxiang'),
      ('rime_ice', 'rime-ice'),
      ('rime_mint', 'rime-mint'),
      ('rime_frost', 'rime-frost'),
      ('triple_pinyin_lssp', 'rime-lssp'),
    ]) {
      c.local = LocalSchemaDto(
        installed: true,
        deployed: true,
        schemas: [schema],
      );
      await _pumpSchemePage(tester, c);
      expect(c.activeSchemeKey, schema);
      final grid = tester.widget<HairlineGridSection>(
        find.byKey(const ValueKey('cloud-schemes')),
      );
      expect(grid.children, hasLength(7));
      expect(
        find.byKey(ValueKey('addon-$addon')),
        ['rime-ice', 'easy-en'].contains(addon) ? findsOneWidget : findsNothing,
      );
      expect(find.bySemanticsLabel('展开全部方案'), findsNothing);
      await _openAddons(tester);
      expect(
        find.descendant(
          of: find.byKey(const ValueKey('addon-schemes')),
          matching: find.byKey(ValueKey('addon-$addon')),
        ),
        ['rime-ice', 'easy-en'].contains(addon) ? findsNothing : findsOneWidget,
      );
      Navigator.of(tester.element(find.byType(AppDialog))).pop();
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull);
    }
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets('mobile scheme cards and switches fit two narrow columns', (
    tester,
  ) async {
    final c = _PackageController(BridgePlatform.android)
      ..addon = const AddonStatusDto(
        installed: false,
        deployed: false,
        version: '',
        englishAvailable: false,
      )
      ..installedPackages = const [
        InstalledPackageDto(
          schemeKey: 'keytao',
          source: 'cloud',
          active: true,
          inUse: true,
          hasPackageZip: true,
        ),
        InstalledPackageDto(
          schemeKey: 'local-custom',
          source: 'local',
          displayName: 'Very long local scheme display name',
          active: false,
          hasPackageZip: true,
        ),
        InstalledPackageDto(
          schemeKey: 'local-second',
          source: 'local',
          displayName: 'Second local scheme',
          active: false,
          hasPackageZip: true,
        ),
      ]
      ..schemaSwitches = const [
        SchemaSwitchDto(
          schemaId: 'keytao',
          name: 'example',
          states: ['关闭', '较长的方案开关名字'],
          value: 0,
          options: [],
        ),
        SchemaSwitchDto(
          schemaId: 'keytao',
          name: 'choice',
          states: ['较长的第一项选择', '较长的第二项选择'],
          value: 0,
          options: ['first', 'second'],
        ),
      ];
    c.rimeAddons['rime_ice']!.status = c.addon;
    addTearDown(c.dispose);
    _size(tester, const Size(320, 850));
    await _pumpSchemePage(tester, c);

    Finder card(String key) => find.byKey(ValueKey('scheme-package-$key'));
    Finder cellFor(String key) => find
        .ancestor(of: card(key), matching: find.byType(HairlineGridCell))
        .first;
    double titleOffset(Finder cell, String name) =>
        tester
            .getCenter(find.descendant(of: cell, matching: find.text(name)))
            .dy -
        tester.getTopLeft(cell).dy;
    double actionOffset(Finder cell) =>
        tester
            .getTopLeft(
              find.descendant(of: cell, matching: find.byType(ActionRow)),
            )
            .dy -
        tester.getTopLeft(cell).dy;
    void expectSameRow(Finder left, Finder right) {
      final first = tester.getRect(left);
      final second = tester.getRect(right);
      expect(first.top, closeTo(second.top, .1));
      expect(first.right, lessThanOrEqualTo(second.left));
    }

    final active = card('keytao');
    expectSameRow(active, card('xmjd'));
    expectSameRow(card('local-custom'), card('local-second'));
    final ice = find.byKey(const ValueKey('addon-rime-ice'));
    expectSameRow(cellFor('snow_jiandao'), ice);
    expectSameRow(find.byType(SwitchSetting), find.byType(ChoiceSetting<int>));
    final activeCell = cellFor('keytao');
    final expectedTitleOffset = titleOffset(activeCell, '键道6');
    final expectedActionOffset = actionOffset(activeCell);
    for (final (cell, name) in [
      (cellFor('xmjd'), '星猫键道'),
      (cellFor('local-custom'), 'Very long local scheme display name'),
      (cellFor('local-second'), 'Second local scheme'),
      (cellFor('snow_jiandao'), AppStrings.rimeSnow),
      (ice, AppStrings.rimeIce),
      (find.byKey(const ValueKey('addon-easy-en')), 'Easy English'),
    ]) {
      expect(
        titleOffset(cell, name),
        closeTo(expectedTitleOffset, .5),
        reason: name,
      );
      expect(
        actionOffset(cell),
        closeTo(expectedActionOffset, .5),
        reason: name,
      );
      expect(
        tester.getSize(cell).height,
        closeTo(tester.getSize(activeCell).height, .5),
        reason: name,
      );
    }
    expect(tester.widget<HairlineGridCell>(activeCell).color, isNull);
    final activeLabel = find.descendant(of: active, matching: find.text('使用中'));
    expect(activeLabel, findsOneWidget);
    expect(tester.getSize(activeLabel).width, lessThan(60));
    expect(find.widgetWithText(ActionButton, '使用中'), findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets('scheme sections separate local packages and deployment', (
    tester,
  ) async {
    final c = _PackageController()
      ..installedPackages = const [
        InstalledPackageDto(
          schemeKey: 'keytao',
          source: 'cloud',
          active: false,
          hasPackageZip: true,
        ),
        InstalledPackageDto(
          schemeKey: 'local-custom',
          source: 'local',
          displayName: 'Custom scheme',
          active: true,
          hasPackageZip: true,
        ),
      ];
    addTearDown(c.dispose);
    _size(tester, const Size(360, 800));
    await tester.pumpWidget(
      KeyTaoTheme(
        platform: c.info.platform,
        home: FScaffold(
          child: ListenableBuilder(
            listenable: c,
            builder: (_, _) => SchemePage(controller: c),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    final catalogue = find.byKey(const ValueKey('cloud-schemes'));
    final local = find.byKey(const ValueKey('local-schemes'));
    final cleanup = find.widgetWithText(ActionButton, '清理残留文件');
    final active = find.byKey(const ValueKey('scheme-package-local-custom'));
    final deployment = find.byWidgetPredicate(
      (widget) => widget is Section && widget.title == '部署',
    );
    expect(find.text('方案列表'), findsOneWidget);
    expect(find.text('键道方案'), findsNothing);
    expect(find.byKey(const ValueKey('addon-schemes-button')), findsOneWidget);
    expect(find.text('云端方案'), findsNothing);
    expect(
      find.descendant(of: catalogue, matching: find.byType(SchemePackageRow)),
      findsNWidgets(5),
    );
    expect(
      tester
          .widgetList<SchemePackageRow>(
            find.descendant(
              of: catalogue,
              matching: find.byType(SchemePackageRow),
            ),
          )
          .map((row) => row.schemeKey),
      ['keytao', 'xmjd', 'txjx', 'keydo', 'snow_jiandao'],
    );
    expect(find.byKey(const ValueKey('addon-rime-snow')), findsNothing);
    expect(
      tester.widget<HairlineGridSection>(catalogue).children,
      hasLength(7),
    );
    for (final key in [
      'addon-wanxiang',
      'addon-rime-mint',
      'addon-rime-frost',
      'addon-rime-lssp',
    ]) {
      expect(
        find.descendant(of: catalogue, matching: find.byKey(ValueKey(key))),
        findsNothing,
      );
    }
    expect(
      find.descendant(
        of: catalogue,
        matching: find.byKey(const ValueKey('addon-rime-ice')),
      ),
      findsOneWidget,
    );
    expect(
      find.descendant(
        of: catalogue,
        matching: find.byKey(const ValueKey('addon-easy-en')),
      ),
      findsOneWidget,
    );
    expect(
      find.descendant(of: active, matching: find.text('Custom scheme')),
      findsOneWidget,
    );
    expect(
      find.descendant(of: active, matching: find.text('使用中')),
      findsOneWidget,
    );
    expect(
      find.descendant(of: deployment, matching: find.byType(VersionPicker)),
      findsNothing,
    );
    expect(find.text('Custom scheme'), findsOneWidget);
    expect(find.descendant(of: local, matching: active), findsOneWidget);
    expect(find.descendant(of: local, matching: cleanup), findsOneWidget);
    expect(tester.widget<ActionButton>(cleanup).onPress, isNotNull);
    expect(
      find.descendant(of: active, matching: find.text('在用')),
      findsOneWidget,
    );
    expect(
      find.descendant(of: deployment, matching: find.text('检查本地')),
      findsOneWidget,
    );
    tester.view.physicalSize = const Size(1440, 1000);
    await tester.pumpAndSettle();
    expect(
      tester
          .widget<SchemePackageRow>(
            find.ancestor(of: active, matching: find.byType(SchemePackageRow)),
          )
          .compact,
      isFalse,
    );
    expect(
      find.descendant(of: active, matching: find.byType(SchemaLogo)),
      findsNothing,
    );
    expect(tester.takeException(), isNull);
    final add = find.widgetWithText(ActionButton, '从目录导入');
    expect(find.descendant(of: local, matching: add), findsOneWidget);
    expect(
      find.descendant(
        of: local,
        matching: find.textContaining('请导入纯 Rime 文件夹或 Rime 方案 .zip 压缩包'),
      ),
      findsOneWidget,
    );
    expect(
      find.descendant(of: local, matching: find.textContaining('512 MB')),
      findsOneWidget,
    );
    await tester.ensureVisible(add);
    await tester.tap(add);
    await tester.pumpAndSettle();
    expect(find.textContaining('自动过滤 build'), findsAtLeastNWidgets(1));
    expect(find.textContaining('512 MB'), findsAtLeastNWidgets(1));
    expect(find.byKey(const ValueKey('import-local-folder-button')), findsOneWidget);
    expect(find.byKey(const ValueKey('import-local-zip-button')), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('import-local-folder-button')));
    await tester.pumpAndSettle();
    expect(c.actions, ['import:null:folder']);
    await tester.tap(add);
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('import-local-zip-button')));
    await tester.pumpAndSettle();
    expect(c.actions, ['import:null:folder', 'import:null:zip']);
    c.installedPackages = [];
    c.rebuild();
    await tester.pumpAndSettle();
    expect(
      find.descendant(of: local, matching: find.byType(SchemePackageRow)),
      findsNothing,
    );
    expect(find.descendant(of: local, matching: add), findsOneWidget);
    expect(find.descendant(of: local, matching: cleanup), findsOneWidget);
    c.busy = true;
    c.rebuild();
    await tester.pumpAndSettle();
    expect(tester.widget<ActionButton>(cleanup).onPress, isNull);
    expect(tester.takeException(), isNull);
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets('local package menu allows deleting active packages', (
    tester,
  ) async {
    final c = _PackageController()..accountUser = const AccountUserDto(id: 1);
    addTearDown(c.dispose);
    _size(tester, const Size(360, 800));
    for (final active in [true, false]) {
      c.installedPackages = [
        InstalledPackageDto(
          schemeKey: 'local-custom',
          source: 'local',
          active: active,
          hasPackageZip: true,
        ),
      ];
      c.actions.clear();
      c.syncedSchemes.clear();
      await _pumpRow(tester, c, 'local-custom');
      expect(find.text('重新导入'), findsNothing);
      expect(find.text('随账号同步'), findsNothing);
      expect(find.text('卸载'), findsNothing);
      final more = find.byIcon(FLucideIcons.ellipsis);
      await tester.tap(more);
      await tester.pumpAndSettle();
      expect(find.text('上传'), active ? findsOneWidget : findsNothing);
      expect(find.text('卸载'), findsOneWidget);
      expect(
        tester
            .widget<FItem>(find.widgetWithText(FItem, '随账号同步'))
            .semanticsChecked,
        isFalse,
      );
      await tester.tap(find.widgetWithText(FItem, '随账号同步'));
      await tester.pumpAndSettle();
      expect(c.actions, ['sync:local-custom:true']);
      await tester.tap(more);
      await tester.pumpAndSettle();
      expect(
        tester
            .widget<FItem>(find.widgetWithText(FItem, '随账号同步'))
            .semanticsChecked,
        isTrue,
      );
      await tester.tap(find.widgetWithText(FItem, '重新导入'));
      await tester.pumpAndSettle();
      expect(find.text('重新导入本地方案'), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('import-local-zip-button')));
      await tester.pumpAndSettle();
      expect(c.actions.last, 'import:local-custom:zip');
      await tester.tap(more);
      await tester.pumpAndSettle();
      await tester.tap(find.widgetWithText(FItem, '卸载'));
      await tester.pumpAndSettle();
      expect(c.actions.last, 'remove:local-custom');
      expect(tester.takeException(), isNull);
    }
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets('installed xmjd variants share a switchable home card', (
    tester,
  ) async {
    final c = _PackageController()
      ..installedPackages = const [
        InstalledPackageDto(
          schemeKey: 'xmjd',
          version: '20261104',
          active: true,
          hasPackageZip: true,
        ),
        InstalledPackageDto(
          schemeKey: 'xmjd-feiding',
          version: '20261103',
          active: false,
          hasPackageZip: true,
        ),
      ];
    addTearDown(c.dispose);
    _size(tester, const Size(1200, 900));
    await tester.pumpWidget(
      KeyTaoTheme(
        platform: c.info.platform,
        home: FScaffold(
          child: ListenableBuilder(
            listenable: c,
            builder: (_, _) => SchemePage(controller: c),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    final catalogue = find.byKey(const ValueKey('cloud-schemes'));
    expect(
      find.descendant(
        of: catalogue,
        matching: find.byWidgetPredicate(
          (widget) => widget is Text && widget.semanticsLabel == '星猫键道 · 顶功',
        ),
      ),
      findsOneWidget,
    );
    expect(
      find.descendant(
        of: catalogue,
        matching: find.byWidgetPredicate(
          (widget) => widget is Text && widget.semanticsLabel == '星猫键道 · 非顶',
        ),
      ),
      findsNothing,
    );
    expect(find.text('20261104'), findsOneWidget);
    expect(find.text('20261103'), findsNothing);
    expect(find.text('非顶'), findsNothing);
    await tester.ensureVisible(find.text('星猫键道'));
    await tester.tap(find.text('星猫键道'));
    await tester.pumpAndSettle();
    expect(find.text('顶功'), findsOneWidget);
    await tester.tap(find.text('非顶'));
    await tester.pumpAndSettle();
    expect(c.scheme, 'xmjd-feiding');
    expect(find.byKey(const ValueKey('scheme-package-xmjd')), findsNothing);
    expect(
      find.byKey(const ValueKey('scheme-package-xmjd-feiding')),
      findsOneWidget,
    );
    expect(find.text('20261104'), findsNothing);
    expect(find.text('20261103'), findsOneWidget);
    expect(
      find.descendant(
        of: find.byKey(const ValueKey('scheme-package-xmjd-feiding')),
        matching: find.byIcon(FLucideIcons.chevronDown),
      ),
      findsOneWidget,
    );
    expect(find.text('非顶'), findsNothing);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  for (final size in [const Size(360, 800), const Size(1000, 760)]) {
    testWidgets('update banner and scheme tab indicator at $size', (
      tester,
    ) async {
      final c = _PackageController()..schemeUpdates = [_update];
      addTearDown(c.dispose);
      _size(tester, size);
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
      final banner = find.byType(SchemeUpdateBanner);
      final update = find.descendant(
        of: banner,
        matching: find.widgetWithText(ActionButton, '更新'),
      );
      final close = find.descendant(
        of: banner,
        matching: find.byIcon(FLucideIcons.x),
      );
      expect(banner, findsOneWidget);
      expect(
        find.descendant(of: banner, matching: find.text('键道6')),
        findsOneWidget,
      );
      expect(
        find.descendant(of: banner, matching: find.text('→ v2')),
        findsOneWidget,
      );
      expect(
        find.descendant(of: banner, matching: find.byType(FBadge)),
        findsNothing,
      );
      expect(
        find.byKey(const ValueKey('scheme-update-indicator')),
        findsOneWidget,
      );
      await tester.tap(update);
      await tester.pump();
      expect(c.actions, ['update:keytao']);
      c.busy = true;
      c.rebuild();
      await tester.pumpAndSettle();
      expect(tester.widget<ActionButton>(update).onPress, isNull);
      final closeButton = find.ancestor(
        of: close,
        matching: find.byType(FButton),
      );
      expect(tester.widget<FButton>(closeButton).onPress, isNull);
      c.busy = false;
      c.rebuild();
      await tester.pumpAndSettle();
      await tester.tap(close);
      await tester.pumpAndSettle();
      expect(banner, findsNothing);
      expect(c.actions.last, 'dismiss:keytao');
      expect(
        find.byKey(const ValueKey('scheme-update-indicator')),
        findsOneWidget,
      );
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));
  }

  testWidgets(
    'confirming post-download schema selection closes before deployment ends',
    (tester) async {
      final schemaSave = Completer<bool>();
      final c = _PackageController()
        ..schemaSave = schemaSave
        ..schemasResult = const [
          PackageSchemaDto(id: 's1', name: '冰雪键道', selected: true),
          PackageSchemaDto(id: 's2', name: '冰雪双拼', selected: true),
          PackageSchemaDto(id: 's3', name: '冰雪三拼', selected: false),
          PackageSchemaDto(id: 's4', name: '冰雪四拼', selected: false),
        ];
      addTearDown(c.dispose);
      _size(tester, const Size(1000, 760));
      await _pumpSchemePage(tester, c);

      c.requestSchemaSelectionPrompt('snow_jiandao', activateOnConfirm: true);
      await tester.pumpAndSettle();

      expect(find.text('选择方案'), findsOneWidget);
      expect(find.text('冰雪键道'), findsWidgets);
      expect(find.text('冰雪四拼'), findsOneWidget);

      await tester.tap(
        find.descendant(
          of: find.byType(AppDialog),
          matching: find.widgetWithText(ActionButton, '部署'),
        ),
      );
      await tester.pumpAndSettle();

      expect(c.actions, ['schemas:snow_jiandao:s1,s2:activate']);
      expect(find.text('选择方案'), findsNothing);
      expect(schemaSave.isCompleted, isFalse);
      schemaSave.complete(true);
      await tester.pumpAndSettle();
      await tester.pumpWidget(const SizedBox.shrink());
    },
    variant: TargetPlatformVariant.only(TargetPlatform.macOS),
  );

  testWidgets(
    'scheme page displays active isolated scheme directory instead of flat root',
    (tester) async {
      final c = _PackageController()
        ..installedPackages = const [
          InstalledPackageDto(
            schemeKey: 'rime_ice',
            version: '2026.06.30',
            active: true,
            inUse: true,
            hasPackageZip: true,
          ),
        ]
        ..local = const LocalSchemaDto(
          installed: true,
          deployed: true,
          schemas: ['rime_ice'],
          installedScheme: 'rime_ice',
        );
      addTearDown(c.dispose);
      _size(tester, const Size(1000, 760));
      await _pumpSchemePage(tester, c);

      expect(c.activeSchemeDir, '${c.defaultDir}/runtime/rime_ice');
      expect(find.text('${c.defaultDir}/runtime/rime_ice'), findsOneWidget);
      await tester.pumpWidget(const SizedBox.shrink());
    },
    variant: TargetPlatformVariant.only(TargetPlatform.macOS),
  );

  testWidgets(
    'local import selection dialog groups same-family schemas and returns selected IDs',
    (tester) async {
      _size(tester, const Size(1000, 760));
      List<String>? picked;
      const inspection = LocalImportInspectionDto(
        groups: [
          LocalImportGroupDto(
            primarySchemaId: 'keytao',
            displayName: '键道6',
            schemas: [
              PackageSchemaDto(id: 'keytao', name: '键道6', selected: true),
              PackageSchemaDto(id: 'keytao-bj', name: '键道6并击', selected: true),
              PackageSchemaDto(id: 'keytao-dz', name: '键道6单字', selected: true),
            ],
          ),
          LocalImportGroupDto(
            primarySchemaId: 'rime_ice',
            displayName: '雾凇拼音',
            schemas: [
              PackageSchemaDto(id: 'rime_ice', name: '雾凇拼音', selected: true),
            ],
          ),
        ],
        totalSchemas: 4,
      );
      await tester.pumpWidget(
        KeyTaoTheme(
          platform: BridgePlatform.macOs,
          home: FScaffold(
            child: Builder(
              builder: (context) => Center(
                child: ActionButton(
                  '打开导入选择',
                  onPress: () async {
                    picked = await showLocalImportSchemaSelection(
                      context,
                      inspection,
                    );
                  },
                ),
              ),
            ),
          ),
        ),
      );
      await tester.tap(find.text('打开导入选择'));
      await tester.pumpAndSettle();

      expect(find.text('选择要导入的方案'), findsOneWidget);
      expect(find.text('3 个子方案'), findsOneWidget);
      expect(find.text('键道6并击'), findsOneWidget);
      expect(find.text('键道6单字'), findsOneWidget);
      expect(find.text('导入 (4)'), findsOneWidget);

      await tester.tap(find.text('键道6单字'));
      await tester.pumpAndSettle();
      expect(find.text('导入 (3)'), findsOneWidget);

      await tester.tap(find.text('导入 (3)'));
      await tester.pumpAndSettle();

      expect(picked, ['keytao', 'keytao-bj', 'rime_ice']);
      await tester.pumpWidget(const SizedBox.shrink());
    },
    variant: TargetPlatformVariant.only(TargetPlatform.macOS),
  );
}
