import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/ui/candidate_panel_preview.dart';
import 'package:keytao/src/ui/mobile_keyboard_preview.dart';
import 'package:keytao/src/ui/theme.dart';
import 'package:keytao/src/ui/theme_editor.dart';

import 'view_fixture.dart';

class _ThemeEditorController extends ViewController {
  _ThemeEditorController(this.accent) : super(BridgePlatform.macOs);

  final Object? accent;

  @override
  Future<Map<String, dynamic>> getThemeEditor() async {
    final snapshot = await super.getThemeEditor();
    snapshot['source'] = {
      'version': 3,
      'meta': {'id': 'keytao.default', 'name': 'Default'},
    };
    for (final variant in ['resolvedLight', 'resolvedDark']) {
      snapshot[variant]['ui']['accentColor'] = accent;
    }
    return snapshot;
  }
}

class _DragThemeEditorController extends ViewController {
  _DragThemeEditorController() : super(BridgePlatform.macOs);

  final writes = <(String, Object?)>[];
  Completer<void>? saveGate;
  int loads = 0;

  @override
  Future<Map<String, dynamic>> getThemeEditor() {
    loads++;
    return super.getThemeEditor();
  }

  @override
  Future<Map<String, dynamic>> updateThemeEditor({
    required String target,
    required String path,
    Object? value,
  }) async {
    writes.add((path, value));
    await saveGate?.future;
    return getThemeEditor();
  }
}

void main() {
  testWidgets('groups survive dragging, saves, reloads, layout and tabs', (
    tester,
  ) async {
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(1100, 1100);
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final controller = _DragThemeEditorController();
    addTearDown(controller.dispose);
    await tester.pumpWidget(
      KeyTaoTheme(
        platform: BridgePlatform.macOs,
        home: FScaffold(child: ThemeEditorSection(controller: controller)),
      ),
    );
    await tester.pumpAndSettle();
    final fontGroup = find.ancestor(
      of: find.text('字体'),
      matching: find.byType(FAccordion),
    );
    final slider = find.descendant(
      of: find.descendant(
        of: fontGroup,
        matching: find.byKey(const ValueKey(('theme-control', 'pc.font.size'))),
      ),
      matching: find.byType(FSlider),
    );
    void expectExpanded() {
      expect(
        tester
            .widget<FCollapsible>(
              find.descendant(
                of: fontGroup,
                matching: find.byType(FCollapsible),
              ),
            )
            .value,
        1,
      );
    }

    await tester.tap(find.text('字体'));
    await tester.pumpAndSettle();
    expectExpanded();
    await tester.ensureVisible(slider);
    final element = tester.element(slider);
    var bounds = tester.getRect(slider);
    final drag = await tester.startGesture(
      Offset(bounds.left + bounds.width * 8 / 26, bounds.center.dy),
    );
    await drag.moveBy(const Offset(24, 0));
    await tester.pumpAndSettle();
    expectExpanded();
    expect(tester.element(slider), same(element));
    tester.view.physicalSize = const Size(850, 1100);
    await tester.pumpAndSettle();
    expectExpanded();
    expect(tester.element(slider), same(element));
    bounds = tester.getRect(slider);
    await drag.moveTo(Offset(bounds.right - 1, bounds.center.dy));
    await tester.pumpAndSettle();
    expectExpanded();
    expect(controller.writes, isEmpty);
    controller.saveGate = Completer<void>();
    await drag.up();
    await tester.pump(const Duration(milliseconds: 300));
    expectExpanded();
    expect(controller.writes, [('pc.font.size', 36.0)]);
    controller.saveGate!.complete();
    await tester.pumpAndSettle();
    expectExpanded();

    final loads = controller.loads;
    controller.uiSettings = null;
    controller.notifyListeners();
    await tester.pumpAndSettle();
    expect(controller.loads, greaterThan(loads));
    expectExpanded();
    for (final width in [950.0, 850.0, 1100.0]) {
      tester.view.physicalSize = Size(width, 1100);
      await tester.pumpAndSettle();
      expectExpanded();
    }
    await tester.tap(find.text('移动端'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('PC'));
    await tester.pumpAndSettle();
    expectExpanded();
    await tester.tap(find.text('字体'));
    await tester.pumpAndSettle();
    controller.notifyListeners();
    await tester.pumpAndSettle();
    expect(
      tester
          .widget<FCollapsible>(
            find.descendant(of: fontGroup, matching: find.byType(FCollapsible)),
          )
          .value,
      0,
    );
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  for (final accent in <Object?>[null, 'invalid']) {
    testWidgets('unset or invalid accent stays unset: $accent', (tester) async {
      final controller = _ThemeEditorController(accent);
      addTearDown(controller.dispose);
      await tester.pumpWidget(
        KeyTaoTheme(
          platform: BridgePlatform.macOs,
          home: FScaffold(child: ThemeEditorSection(controller: controller)),
        ),
      );
      await tester.pumpAndSettle();

      expect(tester.takeException(), isNull);
      expect(
        tester
            .widget<Semantics>(
              find.byWidgetPredicate(
                (widget) =>
                    widget is Semantics && widget.properties.label == '选择强调色',
              ),
            )
            .properties
            .value,
        '',
      );
      expect(find.byType(CandidatePanelPreview), findsOneWidget);
      expect(find.byType(MobileKeyboardPreview), findsNothing);
      await tester.pumpWidget(const SizedBox.shrink());
    });
  }
}
