import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/app/controller.dart';
import 'package:keytao/src/rust/api/core.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/ui/scheme_page.dart';
import 'package:keytao/src/ui/theme.dart';
import 'package:keytao/src/ui/widgets.dart';

import 'view_fixture.dart';

class _GrammarController extends ViewController {
  _GrammarController(super.platform) {
    wanxiang = const AddonStatusDto(
      installed: true,
      deployed: true,
      version: '1',
      englishAvailable: false,
    );
    wanxiangGrammar = WanxiangGrammarStatusDto(
      available: true,
      installed: false,
      version: '',
      size: BigInt.zero,
      enabledPackages: const [],
    );
  }

  final actions = <String>[];

  @override
  Future<bool> manageWanxiangGrammar(bool installed) async {
    actions.add('grammar:$installed');
    return true;
  }

  @override
  Future<bool> setPackageGrammarModel(String key, String? modelId) async {
    actions.add('pkg-model:$key:${modelId ?? 'off'}');
    return true;
  }

  @override
  Future<bool> manageGrammarModel(String modelId, bool installed) async {
    if (modelId == AppController.wanxiangHansModelId) {
      return manageWanxiangGrammar(installed);
    }
    actions.add('model:$modelId:$installed');
    return true;
  }

  @override
  Future<void> cancelWanxiangGrammarDownload() async {
    actions.add('cancel');
  }

  @override
  Future<void> cancelDownload() async {
    actions.add('cancel-scheme-download');
  }
}

Future<void> _pump(WidgetTester tester, _GrammarController controller) async {
  await tester.pumpWidget(
    KeyTaoTheme(
      platform: controller.info.platform,
      home: FScaffold(
        child: SingleChildScrollView(child: AddonCard(controller: controller)),
      ),
    ),
  );
  await tester.pump();
}

void main() {
  const rowKey = ValueKey('wanxiang-grammar');

  testWidgets('grammar size and action fit a narrow macOS card', (
    tester,
  ) async {
    final c = _GrammarController(BridgePlatform.macOs);
    addTearDown(c.dispose);
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(220, 1200);
    tester.platformDispatcher.textScaleFactorTestValue = 2;
    addTearDown(tester.view.resetDevicePixelRatio);
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    for (final installed in [false, true]) {
      c.wanxiangGrammar = WanxiangGrammarStatusDto(
        available: true,
        installed: installed,
        version: '',
        size: BigInt.from(426770496),
        enabledPackages: const [],
      );
      await _pump(tester, c);
      final row = find.byKey(rowKey);
      final size = find.descendant(
        of: row,
        matching: find.text(c.wanxiangGrammarSize),
      );
      final action = find.descendant(
        of: row,
        matching: find.widgetWithText(ActionButton, installed ? '卸载' : '下载'),
      );
      expect(size, findsOneWidget);
      expect(action, findsOneWidget);
      expect(
        tester.getRect(size).intersect(tester.getRect(row)),
        tester.getRect(size),
      );
      expect(
        tester.getRect(action).intersect(tester.getRect(row)),
        tester.getRect(action),
      );
      expect(tester.takeException(), isNull);
      await tester.ensureVisible(action);
      await tester.tap(action);
      expect(c.actions.last, 'grammar:${!installed}');
      await tester.pumpAndSettle();
    }
  });

  testWidgets(
    'models section shows standalone Wanxiang model outside Wanxiang card, never iOS',
    (tester) async {
      for (final platform in BridgePlatform.values) {
        final c = _GrammarController(platform);
        addTearDown(c.dispose);
        await _pump(tester, c);
        expect(
          find.byKey(rowKey),
          platform == BridgePlatform.ios ? findsNothing : findsOneWidget,
        );
        if (platform != BridgePlatform.ios) {
          // Lives in the dedicated Models section, not inside Wanxiang card.
          expect(
            find.descendant(
              of: find.byKey(const ValueKey('addon-models')),
              matching: find.byKey(rowKey),
            ),
            findsOneWidget,
          );
          expect(
            find.descendant(
              of: find.byKey(const ValueKey('addon-wanxiang')),
              matching: find.byKey(rowKey),
            ),
            findsNothing,
          );
        }
        c.wanxiangGrammar = WanxiangGrammarStatusDto(
          available: false,
          installed: false,
          version: '',
          size: BigInt.zero,
          enabledPackages: const [],
        );
        await _pump(tester, c);
        expect(find.byKey(rowKey), findsNothing);
      }
      // Still visible when Wanxiang Pinyin itself is not installed, listing all 4 models.
      final c = _GrammarController(BridgePlatform.macOs)..wanxiang = null;
      addTearDown(c.dispose);
      await _pump(tester, c);
      expect(find.byKey(rowKey), findsOneWidget);
      expect(
        find.byKey(const ValueKey('grammar-model-wanxiang-lts-zh-hant')),
        findsOneWidget,
      );
      expect(
        find.byKey(const ValueKey('grammar-model-zh-hans-t-essay-bgw')),
        findsOneWidget,
      );
      expect(
        find.byKey(const ValueKey('grammar-model-zh-hant-t-essay-bgw')),
        findsOneWidget,
      );
    },
  );

  testWidgets('grammar row downloads, deletes and only cancels the download', (
    tester,
  ) async {
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(900, 1200);
    addTearDown(tester.view.resetDevicePixelRatio);
    addTearDown(tester.view.resetPhysicalSize);
    final c = _GrammarController(BridgePlatform.macOs);
    addTearDown(c.dispose);
    final row = find.byKey(rowKey);
    Finder action(String text) =>
        find.descendant(of: row, matching: find.text(text));
    await _pump(tester, c);
    expect(action('407 MB'), findsOneWidget);
    await tester.tap(action('下载'));
    expect(c.actions, ['grammar:true']);

    c.wanxiangGrammar = WanxiangGrammarStatusDto(
      available: true,
      installed: true,
      version: 'LTS',
      size: BigInt.from(1024),
      enabledPackages: const [],
    );
    await _pump(tester, c);
    expect(action('1.0 KB'), findsOneWidget);
    await tester.tap(action('卸载'));
    expect(c.actions.last, 'grammar:false');

    final idleHeight = tester.getSize(row).height;
    c.busy = true;
    c.operation = OperationKind.wanxiangGrammar;
    for (final stage in [
      'wanxiang-grammar-download',
      'wanxiang-grammar-deploy',
    ]) {
      c.wanxiangGrammarProgress = InstallProgressDto(
        stage: stage,
        percent: 40,
        message: '40%',
      );
      await _pump(tester, c);
      final button = tester.widget<ActionButton>(
        find.descendant(of: row, matching: find.byType(ActionButton)),
      );
      expect(
        button.label,
        stage == 'wanxiang-grammar-download' ? '取消 40%' : '部署中… 40%',
      );
      expect(button.onPress != null, stage == 'wanxiang-grammar-download');
      // Inline button progress never increases card height.
      expect(tester.getSize(row).height, idleHeight);
      if (button.onPress != null) await tester.tap(find.byWidget(button));
    }
    expect(c.actions.last, 'cancel');
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'cloud and local scheme overflow menus toggle Wanxiang model in Rime mode only',
    (tester) async {
      final c = _GrammarController(BridgePlatform.macOs)
        ..wanxiangGrammar = WanxiangGrammarStatusDto(
          available: true,
          installed: true,
          version: 'LTS',
          size: BigInt.from(1024),
          enabledPackages: const ['keytao'],
        )
        ..installedPackages = [
          const InstalledPackageDto(
            schemeKey: 'keytao',
            version: '1.0.0',
            active: true,
            inUse: true,
            hasPackageZip: true,
            supportsUserWords: true,
            engine: 'rime',
          ),
          const InstalledPackageDto(
            schemeKey: 'local-demo',
            version: '1.0.0',
            active: false,
            inUse: false,
            hasPackageZip: true,
            supportsUserWords: true,
            displayName: '本地演示',
            engine: 'rime',
          ),
        ];
      addTearDown(c.dispose);
      tester.view.devicePixelRatio = 1;
      tester.view.physicalSize = const Size(1200, 1200);
      addTearDown(tester.view.resetDevicePixelRatio);
      addTearDown(tester.view.resetPhysicalSize);

      Future<void> pumpSchemes() async {
        await tester.pumpWidget(
          KeyTaoTheme(
            platform: c.info.platform,
            home: FScaffold(
              child: SingleChildScrollView(
                child: Column(
                  children: [
                    SchemePackageRow(controller: c, schemeKey: 'keytao'),
                    SchemePackageRow(controller: c, schemeKey: 'local-demo'),
                  ],
                ),
              ),
            ),
          ),
        );
        await tester.pumpAndSettle();
      }

      await pumpSchemes();
      final moreButtons = find.byIcon(FLucideIcons.ellipsis);
      expect(moreButtons, findsNWidgets(2));

      // Cloud scheme (keytao): open nested grammar model submenu and turn off.
      await tester.tap(moreButtons.first);
      await tester.pumpAndSettle();
      final cloudItem = find.byKey(
        const ValueKey('enable-wanxiang-grammar-keytao'),
      );
      expect(cloudItem, findsOneWidget);
      await tester.tap(cloudItem);
      await tester.pumpAndSettle();
      await tester.tap(
        find.byKey(const ValueKey('grammar-model-option-keytao-off')),
      );
      await tester.pumpAndSettle();
      expect(c.actions.last, 'pkg-model:keytao:off');

      // Local scheme (local-demo): open nested grammar model submenu and select Wanxiang Hans.
      await tester.tap(moreButtons.last);
      await tester.pumpAndSettle();
      final localItem = find.byKey(
        const ValueKey('enable-wanxiang-grammar-local-demo'),
      );
      expect(localItem, findsOneWidget);
      await tester.tap(localItem);
      await tester.pumpAndSettle();
      await tester.tap(
        find.byKey(
          const ValueKey(
            'grammar-model-option-local-demo-wanxiang-lts-zh-hans',
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(c.actions.last, 'pkg-model:local-demo:wanxiang-lts-zh-hans');

      // In Libjd mode, the switch is hidden.
      c.inputEngine = 'libjd_engine';
      await pumpSchemes();
      await tester.tap(moreButtons.first);
      await tester.pumpAndSettle();
      expect(
        find.byKey(const ValueKey('enable-wanxiang-grammar-keytao')),
        findsNothing,
      );
    },
  );

  testWidgets('collapse/expand toggle cell matches adjacent card height', (
    tester,
  ) async {
    final c = _GrammarController(BridgePlatform.macOs);
    addTearDown(c.dispose);
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(900, 1200);
    addTearDown(tester.view.resetDevicePixelRatio);
    addTearDown(tester.view.resetPhysicalSize);

    await tester.pumpWidget(
      KeyTaoTheme(
        platform: c.info.platform,
        home: FScaffold(
          child: SingleChildScrollView(child: SchemePage(controller: c)),
        ),
      ),
    );
    await tester.pumpAndSettle();

    final toggleCollapsed = find.bySemanticsLabel('展开全部方案');
    expect(toggleCollapsed, findsOneWidget);
    final siblingCard = find.ancestor(
      of: find.byKey(const ValueKey('scheme-package-keydo')),
      matching: find.byType(HairlineGridCell),
    );
    expect(
      tester.getSize(toggleCollapsed).height,
      closeTo(tester.getSize(siblingCard).height, 1.0),
    );

    await tester.tap(toggleCollapsed);
    await tester.pumpAndSettle();
    final toggleExpanded = find.bySemanticsLabel('收起');
    expect(toggleExpanded, findsOneWidget);
    final easyEnCell = find.byKey(const ValueKey('addon-easy-en'));
    expect(
      tester.getSize(toggleExpanded).height,
      closeTo(tester.getSize(easyEnCell).height, 1.0),
    );
  });

  testWidgets(
    'downloading a cloud scheme keeps card height unchanged and puts cancel on button',
    (tester) async {
      final c = _GrammarController(BridgePlatform.macOs);
      addTearDown(c.dispose);
      tester.view.devicePixelRatio = 1;
      tester.view.physicalSize = const Size(900, 1200);
      addTearDown(tester.view.resetDevicePixelRatio);
      addTearDown(tester.view.resetPhysicalSize);

      Future<void> pumpCard() async {
        await tester.pumpWidget(
          KeyTaoTheme(
            platform: c.info.platform,
            home: FScaffold(
              child: HairlineGridSection(
                columnCount: 2,
                children: [
                  HairlineGridCell(
                    child: SchemePackageRow(
                      controller: c,
                      schemeKey: 'keydo',
                    ),
                  ),
                  HairlineGridCell(
                    child: SchemePackageRow(
                      controller: c,
                      schemeKey: 'snow_jiandao',
                    ),
                  ),
                ],
              ),
            ),
          ),
        );
        await tester.pumpAndSettle();
      }

      await pumpCard();
      final card = find.byKey(const ValueKey('scheme-package-keydo'));
      final idleHeight = tester.getSize(card).height;

      c
        ..busy = true
        ..operation = OperationKind.install
        ..installingSchemeKey = 'keydo'
        ..installProgress = const InstallProgressDto(
          stage: 'downloading',
          percent: 35,
          message: '正在下载…',
        );
      await pumpCard();

      expect(tester.getSize(card).height, closeTo(idleHeight, 1.0));
      final cancelBtn = find.descendant(
        of: card,
        matching: find.widgetWithText(ActionButton, '取消 35%'),
      );
      expect(cancelBtn, findsOneWidget);
      await tester.tap(cancelBtn);
      await tester.pumpAndSettle();
      expect(c.actions.last, 'cancel-scheme-download');
    },
  );
}
