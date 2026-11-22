import 'dart:async';
import 'dart:ui' show PointerDeviceKind;

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
  int previewResolutions = 0;

  @override
  Map<String, dynamic> resolveThemeEditorPreview({
    required Map<String, dynamic> source,
    required bool dark,
  }) {
    previewResolutions++;
    return super.resolveThemeEditorPreview(source: source, dark: dark);
  }

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
  testWidgets('phone editor pins its preview and reveals the tapped field', (
    tester,
  ) async {
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(390, 800);
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final controller = ViewController(BridgePlatform.android);
    addTearDown(controller.dispose);
    await tester.pumpWidget(
      KeyTaoTheme(
        platform: BridgePlatform.android,
        home: FScaffold(
          child: ThemeEditorSection(
            controller: controller,
            header: const Text('主题库'),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    final scroll = find.byKey(const ValueKey('theme-editor-scroll'));
    final preview = find.byKey(const ValueKey('theme-sticky-preview'));
    Finder expectFocused(String path) {
      final field = find.byKey(ValueKey(('theme-control', path)));
      final focus = find.descendant(
        of: field,
        matching: find.byKey(const ValueKey('theme-field-focus')),
      );
      expect(Focus.of(tester.element(focus)).hasPrimaryFocus, isTrue);
      expect(
        tester.getTopLeft(field).dy,
        greaterThanOrEqualTo(tester.getBottomLeft(preview).dy),
      );
      return field;
    }

    final mobileHeight = tester.getSize(preview).height;
    await tester.tap(find.byKey(const ValueKey('keyboard-preview-key-空格')));
    await tester.pumpAndSettle();
    expectFocused('mobile.keyboard.keyTypes.space.foreground');
    await tester.drag(scroll, const Offset(0, 800));
    await tester.pumpAndSettle();
    await tester.tap(find.text('PC'));
    await tester.pumpAndSettle();
    expect(tester.getSize(preview).height, lessThan(mobileHeight));

    await tester.drag(scroll, const Offset(0, -310));
    await tester.pumpAndSettle();
    expect(find.text('PC').hitTestable(), findsNothing);
    expect(find.text('主题库').hitTestable(), findsNothing);
    expect(tester.getTopLeft(preview).dy, tester.getTopLeft(scroll).dy);

    const path = 'pc.font.commentSize';
    final comment = find.byKey(const ValueKey(('theme-preview-edit', path)));
    await tester.tap(comment.first);
    await tester.pumpAndSettle();
    expectFocused(path);
    expect(
      tester
          .widget<CandidatePanelPreview>(find.byType(CandidatePanelPreview))
          .highlightPath,
      path,
    );
    expect(tester.takeException(), isNull);
  });

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
    final sliderField = find.descendant(
      of: fontGroup,
      matching: find.byKey(const ValueKey(('theme-control', 'pc.font.size'))),
    );
    final slider = find.descendant(
      of: sliderField,
      matching: find.byType(FSlider),
    );
    String? sliderLabel() => tester
        .widgetList<Text>(
          find.descendant(of: sliderField, matching: find.byType(Text)),
        )
        .last
        .data;
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
    String? highlightedPath() => tester
        .widget<CandidatePanelPreview>(
          find.byWidgetPredicate(
            (widget) => widget is CandidatePanelPreview && !widget.keyboard,
          ),
        )
        .highlightPath;
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: tester.getCenter(slider));
    await tester.pumpAndSettle();
    expect(highlightedPath(), 'pc.font.size');
    await mouse.moveTo(Offset.zero);
    await mouse.removePointer();
    await tester.pumpAndSettle();
    expect(highlightedPath(), isNull);
    var bounds = tester.getRect(slider);
    final drag = await tester.startGesture(
      Offset(bounds.left + bounds.width * 8 / 26, bounds.center.dy),
    );
    final beforeDrag = controller.previewResolutions;
    final initialLabel = sliderLabel();
    await drag.moveBy(const Offset(24, 0));
    await tester.pump(const Duration(milliseconds: 35));
    await drag.moveBy(const Offset(10, 0));
    await tester.pump(const Duration(milliseconds: 60));
    expect(sliderLabel(), isNot(initialLabel));
    expect(controller.previewResolutions, beforeDrag);
    await tester.pump(const Duration(milliseconds: 20));
    expect(controller.previewResolutions, beforeDrag + 1);
    await tester.pumpAndSettle();
    expect(highlightedPath(), 'pc.font.size');
    expectExpanded();
    expect(tester.element(slider), same(element));
    tester.view.physicalSize = const Size(850, 1100);
    await tester.pumpAndSettle();
    expectExpanded();
    expect(tester.element(slider), same(element));
    bounds = tester.getRect(slider);
    final beforeRelease = controller.previewResolutions;
    await drag.moveTo(Offset(bounds.right - 1, bounds.center.dy));
    expectExpanded();
    expect(controller.writes, isEmpty);
    controller.saveGate = Completer<void>();
    await drag.up();
    await tester.pump(const Duration(milliseconds: 300));
    expectExpanded();
    expect(controller.writes, [('pc.font.size', 36.0)]);
    expect(controller.previewResolutions, beforeRelease + 1);
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
