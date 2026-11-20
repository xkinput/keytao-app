import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/ui/system_font_picker.dart';
import 'package:keytao/src/ui/theme.dart';
import 'package:keytao/src/ui/theme_editor.dart';
import 'package:keytao/src/ui/widgets.dart';

import 'view_fixture.dart';

class _FontController extends ViewController {
  _FontController() : super(BridgePlatform.macOs);

  final listing = Completer<List<SystemFontFamilyDto>>();
  final bytes = Completer<Uint8List>();
  int listingCalls = 0;
  final loads = <String>[];
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
    'listing is lazy, rows are virtual, and font loads are deduplicated',
    (tester) async {
      final controller = _FontController();
      addTearDown(controller.dispose);
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
            family: 'Font $index',
            displayName: '字体 $index',
            weights: const ['regular'],
          ),
      ]);
      await tester.pump();
      await tester.pump();
      expect(find.text('默认'), findsOneWidget);
      expect(controller.loads, isNotEmpty);
      expect(controller.loads.length, lessThan(8));
      expect(controller.loads, isNot(contains('Font 99')));
      expect(identical(catalog.list(), catalog.list()), isTrue);
      expect(identical(catalog.load('Font 0'), catalog.load('Font 0')), isTrue);
      expect(controller.loads.where((font) => font == 'Font 0').length, 1);
      await tester.drag(find.byType(ListView), const Offset(0, -1200));
      await tester.pump();
      expect(controller.loads.length, lessThan(20));
      expect(controller.loads, isNot(contains('Font 99')));
      final fontBytes = await rootBundle.load(
        'packages/forui/assets/fonts/inter/Inter.ttf',
      );
      controller.bytes.complete(fontBytes.buffer.asUint8List());
      await tester.pumpAndSettle();
      expect(catalog.alias('Font 0'), isNotNull);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('picking a family writes its nearest supported weight', (
    tester,
  ) async {
    final controller = _FontController();
    addTearDown(controller.dispose);
    controller.listing.complete(const [
      SystemFontFamilyDto(
        family: 'Fixture Sans',
        displayName: '测试字体',
        weights: ['regular', 'medium'],
      ),
    ]);
    final fontBytes = await rootBundle.load(
      'packages/forui/assets/fonts/inter/Inter.ttf',
    );
    controller.bytes.complete(fontBytes.buffer.asUint8List());
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
    await tester.tap(find.text('默认').first);
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
    expect(tester.takeException(), isNull);
  });
}
