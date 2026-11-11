import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/app/app.dart';
import 'package:keytao/src/app/controller.dart'
    show DeployStep, DeployStepState;
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/ui/scheme_page.dart';
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
  _PackageController() : super(BridgePlatform.macOs) {
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

List<String> _actionLabels(WidgetTester tester) => tester
    .widgetList<ActionButton>(find.byType(ActionButton))
    .map((button) => button.label)
    .where((label) => const {'下载', '重新下载', '更新', '启用', '删除'}.contains(label))
    .toList();

void main() {
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
      c.deploySteps.add(
        const DeployStep('Missing english.schema.yaml', DeployStepState.failed),
      );
      await _pumpRow(tester, c, 'keytao');
      expect(find.text('使用中'), findsNothing);
      expect(find.text('部署失败'), findsOneWidget);
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
        labels: <String>[],
        calls: <String>[],
      ),
      (
        package: const InstalledPackageDto(
          schemeKey: 'keytao',
          version: 'v1',
          active: true,
          hasPackageZip: false,
        ),
        update: false,
        labels: ['重新下载'],
        calls: ['install:keytao'],
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
        labels: ['更新'],
        calls: ['update:keytao'],
      ),
      (
        package: const InstalledPackageDto(
          schemeKey: 'keytao',
          version: 'v1',
          active: true,
          hasPackageZip: false,
        ),
        update: true,
        labels: ['更新'],
        calls: ['update:keytao'],
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
      expect(tester.getTopLeft(title).dx, tester.getTopLeft(row).dx);
      expect(
        find.descendant(of: row, matching: find.byType(ActionRow)),
        findsOneWidget,
      );
      expect(_actionLabels(tester), entry.labels);
      expect(
        find.text('使用中'),
        entry.package?.active == true ? findsOneWidget : findsNothing,
      );
      if (entry.package != null) {
        expect(find.text('v1'), findsOneWidget);
        expect(
          tester.getTopLeft(find.text('v1')).dy,
          greaterThan(tester.getBottomLeft(title).dy),
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
        await tester.tap(find.widgetWithText(ActionButton, label));
        await tester.pump();
      }
      expect(c.actions, entry.calls);
      c.busy = true;
      await _pumpRow(tester, c, 'keytao');
      expect(
        tester.widgetList<ActionButton>(find.byType(ActionButton)),
        everyElement(
          isA<ActionButton>().having((b) => b.onPress, 'onPress', null),
        ),
      );
      expect(tester.takeException(), isNull);
    }
    await tester.pumpWidget(const SizedBox.shrink());
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
    expect(find.text('星猫键道 · 顶功'), findsOneWidget);
    expect(find.text('星猫键道 · 非顶'), findsOneWidget);
    expect(find.text('20261104'), findsOneWidget);
    expect(find.text('20261103'), findsOneWidget);
    expect(
      find.descendant(
        of: find.byKey(const ValueKey('scheme-package-xmjd')),
        matching: find.byType(ChoiceSetting<String>),
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
        matching: find.byType(ChoiceSetting<String>),
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
