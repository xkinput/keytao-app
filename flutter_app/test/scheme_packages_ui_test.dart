import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/app/app.dart';
import 'package:keytao/src/app/controller.dart'
    show DeployStep, DeployStepState;
import 'package:keytao/src/app/options.dart';
import 'package:keytao/src/rust/api/core.dart' show AddonStatusDto;
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/ui/input_page.dart';
import 'package:keytao/src/ui/login.dart' show showAccountDialog;
import 'package:keytao/src/ui/onboarding.dart';
import 'package:keytao/src/ui/scheme_page.dart';
import 'package:keytao/src/ui/settings.dart' show showCloudPackageTransfer;
import 'package:keytao/src/ui/theme.dart';
import 'package:keytao/src/ui/widgets.dart';

import 'view_fixture.dart';

const _update = SchemeUpdateDto(
  schemeKey: 'keytao',
  installedVersion: 'v1',
  latestVersion: 'v2',
  hasUpdate: true,
);

class _PackageController extends ViewController {
  _PackageController([super.platform = BridgePlatform.macOs]) {
    appUpdate = null;
  }

  final actions = <String>[];

  void rebuild() => notifyListeners();

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
  Future<bool> manageRimeMint(bool installed) async {
    actions.add('mint:$installed');
    return true;
  }

  @override
  Future<bool> manageRimeFrost(bool installed) async {
    actions.add('frost:$installed');
    return true;
  }

  @override
  Future<bool> importLocalScheme({String? schemeKey}) async {
    actions.add('import:$schemeKey');
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

List<String> _actionLabels(WidgetTester tester) => tester
    .widgetList<ActionButton>(find.byType(ActionButton))
    .map((button) => button.label)
    .where((label) => const {'下载', '重新下载', '更新', '启用', '删除'}.contains(label))
    .toList();

void main() {
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
      expect(find.text('请先安装输入方案。'), findsOneWidget);
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
    (false, false, '2', AppStrings.install),
    (true, true, '1', null),
    (true, false, '1', AppStrings.deploy),
    (true, true, '2', AppStrings.update),
    (true, false, '2', AppStrings.update),
  ]) {
    testWidgets(
      'add-on actions for installed=$installed deployed=$deployed latest=$latest',
      (tester) async {
        final c = _PackageController();
        addTearDown(c.dispose);
        _size(tester, const Size(1000, 1200));
        final status = AddonStatusDto(
          installed: installed,
          deployed: deployed,
          version: '1',
          englishAvailable: false,
        );
        c.addon = c.wanxiang = c.rimeIce = c.rimeMint = c.rimeFrost = status;
        c.addonLatestVersion = c.wanxiangLatestVersion =
            c.rimeIceLatestVersion = c.rimeMintLatestVersion =
                c.rimeFrostLatestVersion = latest;
        await _pumpAddons(tester, c);
        final buttons = tester.widgetList<ActionButton>(
          find.byType(ActionButton),
        );
        expect(
          buttons
              .where((button) => button.primary)
              .map((button) => button.label),
          label == null ? isEmpty : List.filled(5, label),
        );
        expect(
          buttons.where((button) => button.label == AppStrings.uninstall),
          hasLength(installed ? 5 : 0),
        );
        for (final (key, name) in [
          ('addon-rime-mint', 'mint'),
          ('addon-rime-frost', 'frost'),
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
            final uninstall = find.descendant(
              of: card,
              matching: find.widgetWithText(ActionButton, AppStrings.uninstall),
            );
            await tester.ensureVisible(uninstall);
            await tester.tap(uninstall);
            await tester.pump(const Duration(seconds: 1));
            expect(c.actions.last, '$name:false');
          }
        }
        c.busy = true;
        await _pumpAddons(tester, c);
        expect(
          tester.widgetList<ActionButton>(find.byType(ActionButton)),
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
        find.widgetWithText(ActionButton, AppStrings.replace),
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
    final installs = tester.widgetList<ActionButton>(
      find.widgetWithText(ActionButton, AppStrings.install),
    );
    expect(installs, hasLength(5));
    expect(installs.every((button) => button.onPress != null), isTrue);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'input hero identifies standalone add-ons and keeps main priority',
    (tester) async {
      final c = _PackageController();
      addTearDown(c.dispose);
      _size(tester, const Size(1200, 1000));
      for (final (schema, name) in [
        ('easy_en', 'Easy English'),
        ('wanxiang', AppStrings.wanxiang),
      ]) {
        c.local = LocalSchemaDto(
          installed: true,
          deployed: true,
          schemas: [schema],
        );
        const status = AddonStatusDto(
          installed: true,
          deployed: true,
          englishAvailable: true,
          version: 'standalone',
        );
        c.addon = status;
        c.wanxiang = status;
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
            expect(find.text(AppStrings.notInstalled), findsWidgets);
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
    await tester.tap(find.widgetWithText(ActionButton, '启用').first);
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
          hasPackageZip: true,
        ),
        update: false,
        labels: ['删除'],
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
        labels: ['重新下载', '删除'],
        calls: ['install:keytao', 'remove:keytao'],
      ),
      (
        package: const InstalledPackageDto(
          schemeKey: 'keytao',
          version: 'v1',
          active: false,
          hasPackageZip: true,
        ),
        update: false,
        labels: ['启用', '删除'],
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
        labels: ['重新下载', '删除'],
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
        labels: ['更新', '启用', '删除'],
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
        labels: ['更新', '删除'],
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
        labels: ['更新', '删除'],
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
        entry.labels.where((label) => label != '删除'),
      );
      expect(find.text('自造词'), findsNothing);
      expect(find.text('删除'), findsNothing);
      expect(
        find.text('使用中'),
        entry.package?.active == true ? findsOneWidget : findsNothing,
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
        if (label == '删除') {
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
        expect(find.text('删除'), findsOneWidget);
        await tester.tap(find.byIcon(FLucideIcons.ellipsis));
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
        expect(
          tester
              .widget<FButton>(
                find.widgetWithIcon(FButton, FLucideIcons.ellipsis),
              )
              .onPress,
          isNull,
        );
      }
      expect(tester.takeException(), isNull);
    }
    await tester.pumpWidget(const SizedBox.shrink());
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

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
    final active = find.byKey(const ValueKey('scheme-package-local-custom'));
    final deployment = find.byWidgetPredicate(
      (widget) => widget is Section && widget.title == '部署',
    );
    for (final heading in ['方案列表', '键道方案', '附加方案']) {
      expect(find.text(heading), findsOneWidget);
    }
    expect(find.text('云端方案'), findsNothing);
    expect(
      find.descendant(of: catalogue, matching: find.byType(SchemePackageRow)),
      findsNWidgets(4),
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
    final add = find.widgetWithText(ActionButton, '添加本地方案');
    expect(find.descendant(of: local, matching: add), findsOneWidget);
    await tester.ensureVisible(add);
    await tester.tap(add);
    expect(c.actions, ['import:null']);
    c.installedPackages = [];
    c.rebuild();
    await tester.pumpAndSettle();
    expect(
      find.descendant(of: local, matching: find.byType(SchemePackageRow)),
      findsNothing,
    );
    expect(find.descendant(of: local, matching: add), findsOneWidget);
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
      expect(find.text('删除'), findsNothing);
      final more = find.byIcon(FLucideIcons.ellipsis);
      await tester.tap(more);
      await tester.pumpAndSettle();
      expect(find.text('上传'), active ? findsOneWidget : findsNothing);
      expect(find.text('删除'), findsOneWidget);
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
      expect(c.actions.last, 'import:local-custom');
      await tester.tap(more);
      await tester.pumpAndSettle();
      await tester.tap(find.widgetWithText(FItem, '删除'));
      await tester.pumpAndSettle();
      expect(c.actions.last, 'remove:local-custom');
      expect(tester.takeException(), isNull);
    }
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets('both installed xmjd variants remain distinct in one list', (
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
    _size(tester, const Size(1000, 900));
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
      findsOneWidget,
    );
    expect(find.text('20261104'), findsOneWidget);
    expect(find.text('20261103'), findsOneWidget);
    expect(
      find.descendant(
        of: find.byKey(const ValueKey('scheme-package-xmjd')),
        matching: find.byType(BigTextRadioGroup<String>),
      ),
      findsOneWidget,
    );
    await tester.tap(find.text('非顶'));
    await tester.pumpAndSettle();
    expect(c.scheme, 'xmjd-feiding');
    expect(find.byKey(const ValueKey('scheme-package-xmjd')), findsOneWidget);
    expect(
      find.byKey(const ValueKey('scheme-package-xmjd-feiding')),
      findsOneWidget,
    );
    expect(
      find.descendant(
        of: find.byKey(const ValueKey('scheme-package-xmjd-feiding')),
        matching: find.byType(BigTextRadioGroup<String>),
      ),
      findsOneWidget,
    );
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
}
