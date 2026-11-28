import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/ui/input_page.dart';
import 'package:keytao/src/ui/theme.dart';
import 'package:keytao/src/ui/widgets.dart';

import 'view_fixture.dart';

class _ActionsController extends ViewController {
  _ActionsController() : super(BridgePlatform.macOs);

  final calls = <String>[];

  void update(VoidCallback change) {
    change();
    notifyListeners();
  }

  @override
  Future<bool> deploy({int? imeActionId, String? schemeKey}) async {
    calls.add('deploy');
    return true;
  }

  @override
  Future<bool> syncRimeUserData() async {
    calls.add('sync');
    return true;
  }

  @override
  Future<bool> openActiveSchemeDirectory() async {
    calls.add('open-directory');
    return true;
  }

  @override
  Future<bool> refreshState() async {
    calls.add('check-local');
    return true;
  }

  @override
  Future<void> ensureRemoteSchemeLogo(
    String key, {
    String? repoUrl,
    String? branch,
    bool? addonInstalled,
  }) async {}
}

Future<_ActionsController> _show(WidgetTester tester) async {
  tester.view.devicePixelRatio = 1;
  tester.view.physicalSize = const Size(1000, 800);
  addTearDown(tester.view.resetDevicePixelRatio);
  addTearDown(tester.view.resetPhysicalSize);
  final c = _ActionsController();
  addTearDown(c.dispose);
  await tester.pumpWidget(
    KeyTaoTheme(
      platform: c.info.platform,
      home: FScaffold(
        child: ListenableBuilder(
          listenable: c,
          builder: (_, _) => InputPage(controller: c),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
  return c;
}

void main() {
  testWidgets(
    'input actions keep their order and invoke the original handlers',
    (tester) async {
      final c = await _show(tester);
      final actions = find.byKey(const ValueKey('current-scheme-actions'));
      final buttons = tester.widgetList<ActionButton>(
        find.descendant(of: actions, matching: find.byType(ActionButton)),
      );
      expect(buttons.map((button) => button.label), ['部署', '同步', '打开目录']);
      final schema = tester.getRect(
        find.byKey(const ValueKey('current-schema-selector')),
      );
      expect(tester.getRect(actions).top, greaterThan(schema.bottom));
      expect(find.widgetWithText(ActionButton, '重新部署'), findsNothing);

      await tester.tap(find.byKey(const ValueKey('input-deploy-button')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('rime-sync-button')));
      await tester.pumpAndSettle();
      await tester.tap(
        find.byKey(const ValueKey('input-open-directory-button')),
      );
      await tester.pumpAndSettle();
      final checkLocal = find.byKey(const ValueKey('input-check-local-button'));
      await tester.ensureVisible(checkLocal);
      await tester.pumpAndSettle();
      final cleanup = tester.getRect(
        find.widgetWithText(ActionButton, '清理残留文件'),
      );
      final check = tester.getRect(checkLocal);
      expect(check.left, greaterThan(cleanup.right));
      expect(check.top, closeTo(cleanup.top, .1));
      await tester.tap(checkLocal);
      await tester.pumpAndSettle();
      expect(c.calls, ['deploy', 'sync', 'open-directory', 'check-local']);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('moved input actions preserve busy and deployment guards', (
    tester,
  ) async {
    final c = await _show(tester);
    ActionButton button(String key) =>
        tester.widget<ActionButton>(find.byKey(ValueKey(key)));
    c.update(() => c.busy = true);
    await tester.pumpAndSettle();
    for (final key in [
      'input-deploy-button',
      'rime-sync-button',
      'input-open-directory-button',
      'input-check-local-button',
    ]) {
      expect(button(key).onPress, isNull, reason: key);
    }
    expect(
      tester
          .widget<FButton>(
            find.byKey(const ValueKey('rime-sync-settings-button')),
          )
          .onPress,
      isNull,
    );
    c.update(() {
      c.busy = false;
      c.rimeSyncLoading = true;
    });
    await tester.pumpAndSettle();
    expect(button('rime-sync-button').onPress, isNull);
    expect(
      tester
          .widget<FButton>(
            find.byKey(const ValueKey('rime-sync-settings-button')),
          )
          .onPress,
      isNull,
    );
    c.update(() {
      c.rimeSyncLoading = false;
      c.inputEngine = 'libjd_engine';
    });
    await tester.pumpAndSettle();
    expect(button('input-deploy-button').onPress, isNull);
    expect(button('rime-sync-button').onPress, isNotNull);
    expect(button('input-open-directory-button').onPress, isNotNull);
    expect(button('input-check-local-button').onPress, isNotNull);
    c.update(() {
      c.inputEngine = 'rime';
      c.local = const LocalSchemaDto(
        installed: false,
        deployed: false,
        schemas: [],
      );
    });
    await tester.pumpAndSettle();
    expect(button('input-deploy-button').onPress, isNull);
    expect(c.calls, isEmpty);
    expect(tester.takeException(), isNull);
  });
}
