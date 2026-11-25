import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/ui/candidate_panel_preview.dart';
import 'package:keytao/src/ui/system_font_picker.dart';
import 'package:keytao/src/ui/theme.dart';
import 'package:keytao/src/ui/theme_editor.dart';
import 'package:keytao/src/ui/widgets.dart';

import 'view_fixture.dart';

const _fontChannel = MethodChannel('ink.rea.keytao/window');

class _FontController extends ViewController {
  _FontController([super.platform = BridgePlatform.macOs]);

  final listing = Completer<List<SystemFontFamilyDto>>();
  final bytes = Completer<Uint8List>();
  int listingCalls = 0;
  final loads = <String>[];
  final pathRequests = <String>[];
  final fontPaths = <String, String>{};
  final writes = <(String, Object?)>[];
  final themeSource = <String, dynamic>{
    'version': 3,
    'meta': {'id': 'my.fixture', 'name': 'Fixture'},
    'common': {
      'font': {'family': '', 'weight': 'bold'},
    },
  };

  @override
  Future<List<SystemFontFamilyDto>> listSystemFontFamilies() {
    listingCalls++;
    return listing.future;
  }

  @override
  Future<Uint8List> loadSystemFontBytes({required String family}) {
    loads.add(family);
    return bytes.future;
  }

  @override
  Future<String?> systemFontPath({required String family}) async {
    pathRequests.add(family);
    return fontPaths[family];
  }

  @override
  Map<String, dynamic> resolveThemeEditorPreview({
    required Map<String, dynamic> source,
    required bool dark,
  }) {
    final preview = super.resolveThemeEditorPreview(source: source, dark: dark);
    preview['font'] = {
      ...preview['font'] as Map<String, dynamic>,
      ...?source['common']?['font'] as Map<String, dynamic>?,
      ...?source[dark ? 'dark' : 'light']?['common']?['font']
          as Map<String, dynamic>?,
    };
    final family = preview['font']['family'] as String?;
    preview['font']['effectiveFamily'] = family?.isEmpty ?? true
        ? null
        : family;
    return preview;
  }

  @override
  Future<Map<String, dynamic>> getThemeEditor() async {
    final snapshot = await super.getThemeEditor();
    snapshot['source'] = jsonDecode(jsonEncode(themeSource));
    for (final dark in [false, true]) {
      snapshot[dark ? 'resolvedDark' : 'resolvedLight'] =
          resolveThemeEditorPreview(source: themeSource, dark: dark);
    }
    return snapshot;
  }

  @override
  Future<Map<String, dynamic>> updateThemeEditor({
    required String target,
    required String path,
    Object? value,
  }) async {
    writes.add((path, value));
    var node = themeSource.putIfAbsent(
      target,
      () => <String, dynamic>{},
    ) as Map<String, dynamic>;
    final parts = path.split('.');
    for (final part in parts.take(parts.length - 1)) {
      node = node.putIfAbsent(
        part,
        () => <String, dynamic>{},
      ) as Map<String, dynamic>;
    }
    node[parts.last] = value;
    return getThemeEditor();
  }
}

void main() {
  test(
    'weight selection keeps supported values and chooses nearest weight',
    () {
      expect(closestThemeFontWeight('bold', ['regular', 'bold']), 'bold');
      expect(closestThemeFontWeight('bold', ['regular', 'medium']), 'medium');
      expect(
        closestThemeFontWeight('medium', ['regular', 'semibold']),
        'regular',
      );
      expect(closestThemeFontWeight('ultralight', ['light', 'black']), 'light');
    },
  );

  testWidgets(
    'macOS listing is lazy and rows preview by name without loading bytes',
    (tester) async {
      final controller = _FontController();
      addTearDown(controller.dispose);
      controller.fontPaths['Font 99'] = '/fixtures/font99.ttf';
      final messenger =
          TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
      messenger.setMockMethodCallHandler(_fontChannel, (call) async {
        expect(call.method, 'resolveSystemFontFamily');
        final family = call.arguments['family'];
        if (family == '宋体-简') return 'Songti SC';
        if (family == 'Font 98' ||
            (family == 'Font 99' &&
                call.arguments['path'] != controller.fontPaths[family])) {
          return null;
        }
        return family;
      });
      addTearDown(() => messenger.setMockMethodCallHandler(_fontChannel, null));
      final catalog = SystemFontCatalog.forController(controller);
      expect(controller.listingCalls, 0);
      await tester.pumpWidget(
        KeyTaoTheme(
          platform: BridgePlatform.macOs,
          home: FScaffold(
            child: Center(
              child: SizedBox(
                width: 400,
                height: 280,
                child: SystemFontPicker(catalog: catalog, selected: ''),
              ),
            ),
          ),
        ),
      );
      expect(controller.listingCalls, 1);
      expect(find.byType(FProgress), findsOneWidget);
      controller.listing.complete([
        for (var index = 0; index < 100; index++)
          SystemFontFamilyDto(
            family: index == 0 ? '宋体-简' : 'Font $index',
            displayName: '字体 $index',
            weights: const ['regular'],
          ),
      ]);
      await tester.pump();
      await tester.pump();
      expect(find.text('默认'), findsOneWidget);
      expect(
        tester.widget<Text>(find.text('字体 0')).style?.fontFamily,
        'Songti SC',
      );
      expect(find.text('字体 99'), findsNothing);
      expect(identical(catalog.list(), catalog.list()), isTrue);
      await tester.drag(find.byType(ListView), const Offset(0, -1200));
      await tester.pumpAndSettle();
      expect(find.text('字体 0'), findsNothing);
      await tester.enterText(find.byType(EditableText), 'Font 99');
      await tester.pumpAndSettle();
      expect(
        tester.widget<Text>(find.text('字体 99')).style?.fontFamily,
        'Font 99',
      );
      expect(controller.pathRequests, ['Font 99']);
      await tester.enterText(find.byType(EditableText), 'Font 98');
      await tester.pumpAndSettle();
      expect(tester.widget<Text>(find.text('字体 98')).style?.fontFamily, isNull);
      expect(controller.pathRequests, ['Font 99', 'Font 98']);
      expect(controller.loads, isEmpty);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('mobile font loads are deduplicated and registered', (
    tester,
  ) async {
    final controller = _FontController(BridgePlatform.android);
    addTearDown(controller.dispose);
    final catalog = SystemFontCatalog.forController(controller);
    await tester.runAsync(() async {
      final fontBytes = await rootBundle.load(
        'packages/forui/assets/fonts/inter/Inter.ttf',
      );
      controller.bytes.complete(Uint8List.sublistView(fontBytes));
      final load = catalog.load('Font 0')!;
      expect(catalog.usesFontLoader, isTrue);
      expect(catalog.load('Font 0'), same(load));
      final alias = await load;
      expect(catalog.alias('Font 0'), alias);
      expect(controller.loads, ['Font 0']);
    });
    expect(tester.takeException(), isNull);
  });

  testWidgets('picking a family writes its nearest supported weight', (
    tester,
  ) async {
    final controller = _FontController();
    addTearDown(controller.dispose);
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(_fontChannel, (call) async {
      expect(call.method, 'resolveSystemFontFamily');
      return call.arguments['family'];
    });
    addTearDown(() => messenger.setMockMethodCallHandler(_fontChannel, null));
    controller.listing.complete(const [
      SystemFontFamilyDto(
        family: 'Fixture Sans',
        displayName: '测试字体',
        weights: ['regular', 'medium'],
      ),
    ]);
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(1150, 1100);
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(
      KeyTaoTheme(
        platform: BridgePlatform.macOs,
        home: FScaffold(child: ThemeEditorSection(controller: controller)),
      ),
    );
    await tester.pumpAndSettle();
    expect(controller.listingCalls, 0);
    await tester.tap(find.text('字体'));
    await tester.pumpAndSettle();
    final field = find.byKey(
      const ValueKey(('theme-control', 'common.font.family')),
    );
    await tester.tap(find.descendant(of: field, matching: find.text('默认')));
    await tester.pumpAndSettle();
    expect(controller.listingCalls, 1);
    await tester.tap(find.text('测试字体'));
    await tester.pumpAndSettle();
    expect(controller.writes, hasLength(1));
    expect(controller.writes.single.$1, 'common.font');
    expect(controller.writes.single.$2, {
      'family': 'Fixture Sans',
      'weight': 'medium',
    });
    final bold = tester.widget<ActionButton>(
      find.byWidgetPredicate(
        (widget) => widget is ActionButton && widget.label == '粗',
      ),
    );
    expect(bold.onPress, isNull);
    expect(bold.primary, isFalse);
    final medium = tester.widget<ActionButton>(
      find.byWidgetPredicate(
        (widget) => widget is ActionButton && widget.label == '中等',
      ),
    );
    expect(medium.onPress, isNotNull);
    expect(medium.primary, isTrue);
    final preview = find.byType(CandidatePanelPreview);
    final hint = find.byWidgetPredicate(
      (widget) => widget is Semantics && widget.properties.label == '模式提示预览',
    );
    for (final area in [preview, hint]) {
      expect(
        find.descendant(
          of: area,
          matching: find.byWidgetPredicate(
            (widget) =>
                widget is Text && widget.style?.fontFamily == 'Fixture Sans',
          ),
        ),
        findsWidgets,
      );
    }
    expect(controller.loads, isEmpty);
    expect(tester.takeException(), isNull);
  });
}
