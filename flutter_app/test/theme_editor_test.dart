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

class _PaddingThemeEditorController extends ViewController {
  _PaddingThemeEditorController() : super(BridgePlatform.macOs);

  static const initial = {
    'paddingTop': 2.0,
    'paddingBottom': 7.0,
    'paddingLeft': 31.0,
    'paddingRight': 12.0,
  };
  final padding = Map.of(initial);
  final writes = <(String, Object?)>[];

  @override
  Future<Map<String, dynamic>> getThemeEditor() async {
    final snapshot = await super.getThemeEditor();
    final source = snapshot['source'] as Map<String, dynamic>;
    final pc = source['pc'] as Map<String, dynamic>;
    source['pc'] = <String, dynamic>{
      ...pc,
      'panel': {...pc['panel'] as Map<String, dynamic>, ...padding},
    };
    for (final dark in [false, true]) {
      snapshot[dark ? 'resolvedDark' : 'resolvedLight'] =
          resolveThemeEditorPreview(source: source, dark: dark);
    }
    return snapshot;
  }

  @override
  Map<String, dynamic> resolveThemeEditorPreview({
    required Map<String, dynamic> source,
    required bool dark,
  }) {
    final theme = super.resolveThemeEditorPreview(source: source, dark: dark);
    final panel = theme['panel'] as Map<String, dynamic>;
    for (final side in initial.keys) {
      final value =
          source[dark ? 'dark' : 'light']?['pc']?['panel']?[side] ??
          source['pc']?['panel']?[side];
      if (value != null) panel[side] = value;
    }
    return theme;
  }

  @override
  Future<Map<String, dynamic>> updateThemeEditor({
    required String target,
    required String path,
    Object? value,
  }) async {
    writes.add((path, value));
    final edits = path == 'pc.panel.padding'
        ? value as Map<String, Object?>
        : {path.split('.').last: value};
    for (final entry in edits.entries) {
      if (entry.value case final num value) {
        padding[entry.key] = value.toDouble();
      } else {
        padding.remove(entry.key);
      }
    }
    return getThemeEditor();
  }
}

void main() {
  testWidgets('padding locks keep existing values and save linked moves once', (
    tester,
  ) async {
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(1150, 1100);
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final controller = _PaddingThemeEditorController();
    addTearDown(controller.dispose);
    await tester.pumpWidget(
      KeyTaoTheme(
        platform: BridgePlatform.macOs,
        home: FScaffold(child: ThemeEditorSection(controller: controller)),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('面板'));
    await tester.pumpAndSettle();

    Finder lock(String axis) =>
        find.byKey(ValueKey('theme-padding-lock-$axis'));
    Finder field(String side) =>
        find.byKey(ValueKey(('theme-control', 'pc.panel.$side')));
    double displayed(String side) => double.parse(
      tester
          .widgetList<Text>(
            find.descendant(of: field(side), matching: find.byType(Text)),
          )
          .last
          .data!,
    );
    Map<String, double> displayedAll() => {
      for (final side in _PaddingThemeEditorController.initial.keys)
        side: displayed(side),
    };
    Future<TestGesture> move(String side, double limit) async {
      final slider = find.descendant(
        of: field(side),
        matching: find.byType(FSlider),
      );
      await tester.ensureVisible(slider);
      await tester.pumpAndSettle();
      expect(slider.hitTestable(), findsOneWidget);
      final bounds = tester.getRect(slider);
      final thumb = tester
          .element(slider)
          .theme
          .sliderStyles
          .horizontal
          .thumbSize;
      Offset point(double fraction) => Offset(
        bounds.left + thumb / 2 + (bounds.width - thumb) * fraction,
        bounds.center.dy,
      );
      final start = displayed(side).clamp(0.0, limit) / limit;
      final end = start < .5 ? .93 : .07;
      final drag = await tester.startGesture(point(start));
      await drag.moveTo(point((start + end) / 2));
      await tester.pump();
      final preview = find.byType(CandidatePanelPreview);
      final previewBeforeMove = tester.widget<CandidatePanelPreview>(preview);
      await drag.moveTo(point(end));
      await tester.pump();
      expect(
        tester.widget<CandidatePanelPreview>(preview),
        same(previewBeforeMove),
      );
      return drag;
    }

    expect(displayedAll(), _PaddingThemeEditorController.initial);
    await tester.tap(lock('vertical'));
    await tester.pumpAndSettle();
    await tester.tap(lock('all'));
    await tester.pumpAndSettle();
    expect(displayedAll(), _PaddingThemeEditorController.initial);
    expect(controller.writes, isEmpty);
    expect(tester.widget<FButton>(lock('vertical')).onPress, isNull);
    await tester.tap(lock('all'));
    await tester.pumpAndSettle();
    expect(
      find.descendant(
        of: lock('vertical'),
        matching: find.byIcon(FLucideIcons.lockOpen),
      ),
      findsOneWidget,
    );
    expect(
      find.descendant(
        of: lock('horizontal'),
        matching: find.byIcon(FLucideIcons.lock),
      ),
      findsOneWidget,
    );

    final horizontal = await move('paddingRight', 32);
    final right = displayed('paddingRight');
    expect(right, isNot(12));
    expect(displayed('paddingLeft'), right);
    expect(displayed('paddingTop'), 2);
    expect(controller.writes, isEmpty);
    await tester.pump(const Duration(milliseconds: 90));
    final panel = tester
        .widget<CandidatePanelPreview>(find.byType(CandidatePanelPreview))
        .theme['panel'];
    expect(panel['paddingLeft'], right);
    expect(panel['paddingRight'], right);
    await horizontal.up();
    await tester.pumpAndSettle();
    expect(controller.writes.single.$1, 'pc.panel.padding');
    expect(controller.writes.single.$2, {
      'paddingLeft': right,
      'paddingRight': right,
    });

    final vertical = await move('paddingTop', 28);
    final top = displayed('paddingTop');
    expect(displayed('paddingBottom'), 7);
    await vertical.up();
    await tester.pumpAndSettle();
    expect(controller.writes.last, ('pc.panel.paddingTop', top));

    await tester.tap(lock('all'));
    await tester.pumpAndSettle();
    final all = await move('paddingLeft', 28);
    final value = displayed('paddingLeft');
    expect(value, lessThanOrEqualTo(28));
    expect(displayedAll().values, everyElement(value));
    await all.up();
    await tester.pumpAndSettle();
    expect(controller.writes, hasLength(3));
    expect(controller.writes.last.$1, 'pc.panel.padding');
    expect(controller.writes.last.$2, displayedAll());

    await tester.tap(
      find.descendant(
        of: field('paddingTop'),
        matching: find.byIcon(FLucideIcons.rotateCcw),
      ),
    );
    await tester.pumpAndSettle();
    expect(controller.writes, hasLength(4));
    expect(controller.writes.last.$2, {
      for (final side in _PaddingThemeEditorController.initial.keys) side: null,
    });
    expect(tester.takeException(), isNull);
  });

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

  testWidgets(
    'verticalFullWidth stretches vertical highlight box to widest candidate and toggles in ThemeEditor',
    (tester) async {
      final controller = _DragThemeEditorController();
      addTearDown(controller.dispose);
      final baseTheme = controller.resolveCandidatePanelPreview(
        colorScheme: UiColorSchemeDto.light,
        accentColor: '#3B73D9',
        cornerRadius: null,
        backgroundColor: '',
        fontSize: 20,
        systemDark: false,
      );
      const sampleCandidates = [
        (text: '这', comment: ''),
        (text: '之', comment: ''),
        (text: '只能服从中国', comment: '~a'),
      ];

      // Default verticalFullWidth: false -> first candidate is narrower than longest candidate.
      await tester.pumpWidget(
        KeyTaoTheme(
          platform: BridgePlatform.macOs,
          home: FScaffold(
            child: CandidatePanelPreview(
              theme: baseTheme,
              orientation: PanelOrientationDto.vertical,
              showPreedit: false,
              candidates: sampleCandidates,
              candidateCount: 3,
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      final shortWidthOff = tester
          .getSize(find.byKey(const ValueKey('candidate-preview-item-0')))
          .width;
      final longWidthOff = tester
          .getSize(find.byKey(const ValueKey('candidate-preview-item-2')))
          .width;
      expect(shortWidthOff, lessThan(longWidthOff));

      // Enabled verticalFullWidth: true -> highlight box matches widest candidate.
      final fullWidthTheme = Map<String, dynamic>.from(baseTheme);
      fullWidthTheme['candidate'] = {
        ...(baseTheme['candidate'] as Map<String, dynamic>),
        'verticalFullWidth': true,
      };
      await tester.pumpWidget(
        KeyTaoTheme(
          platform: BridgePlatform.macOs,
          home: FScaffold(
            child: CandidatePanelPreview(
              theme: fullWidthTheme,
              orientation: PanelOrientationDto.vertical,
              showPreedit: false,
              candidates: sampleCandidates,
              candidateCount: 3,
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      final shortWidthOn = tester
          .getSize(find.byKey(const ValueKey('candidate-preview-item-0')))
          .width;
      final longWidthOn = tester
          .getSize(find.byKey(const ValueKey('candidate-preview-item-2')))
          .width;
      expect(shortWidthOn, closeTo(longWidthOn, 0.5));

      // ThemeEditorSection exposes the toggle and writes pc.candidate.verticalFullWidth.
      await tester.binding.setSurfaceSize(const Size(900, 1200));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      await tester.pumpWidget(
        KeyTaoTheme(
          platform: BridgePlatform.macOs,
          home: FScaffold(child: ThemeEditorSection(controller: controller)),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('候选'));
      await tester.pumpAndSettle();
      final toggleSwitch = find.byWidgetPredicate(
        (w) => w is FSwitch && w.semanticsLabel == '竖排高亮等宽',
      );
      await tester.ensureVisible(toggleSwitch);
      await tester.pumpAndSettle();
      await tester.tap(toggleSwitch);
      await tester.pumpAndSettle();
      expect(controller.writes, contains(('pc.candidate.verticalFullWidth', true)));
      expect(tester.takeException(), isNull);
    },
  );
}
