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
    );
  }

  final actions = <String>[];

  @override
  Future<bool> manageWanxiangGrammar(bool installed) async {
    actions.add('grammar:$installed');
    return true;
  }

  @override
  Future<void> cancelWanxiangGrammarDownload() async {
    actions.add('cancel');
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
    'grammar row requires Wanxiang and runtime capability, never iOS',
    (tester) async {
      for (final platform in BridgePlatform.values) {
        final c = _GrammarController(platform);
        addTearDown(c.dispose);
        await _pump(tester, c);
        expect(
          find.byKey(rowKey),
          platform == BridgePlatform.ios ? findsNothing : findsOneWidget,
        );
        c.wanxiangGrammar = WanxiangGrammarStatusDto(
          available: false,
          installed: false,
          version: '',
          size: BigInt.zero,
        );
        await _pump(tester, c);
        expect(find.byKey(rowKey), findsNothing);
      }
      final c = _GrammarController(BridgePlatform.macOs)..wanxiang = null;
      addTearDown(c.dispose);
      await _pump(tester, c);
      expect(find.byKey(rowKey), findsNothing);
    },
  );

  testWidgets('grammar row downloads, deletes and only cancels the download', (
    tester,
  ) async {
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
    );
    await _pump(tester, c);
    expect(action('1.0 KB'), findsOneWidget);
    await tester.tap(action('卸载'));
    expect(c.actions.last, 'grammar:false');

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
      expect(button.label, '取消');
      expect(button.onPress != null, stage == 'wanxiang-grammar-download');
      expect(
        find.descendant(of: row, matching: find.byType(FDeterminateProgress)),
        findsOneWidget,
      );
      if (button.onPress != null) await tester.tap(action('取消'));
    }
    expect(c.actions.last, 'cancel');
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
  });
}
