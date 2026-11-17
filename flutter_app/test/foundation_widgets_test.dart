import 'package:flutter/material.dart';
import 'package:flutter/semantics.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/app/controller.dart';
import 'package:keytao/src/ui/color_picker.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/ui/theme.dart';
import 'package:keytao/src/ui/widgets.dart';

import 'view_fixture.dart';

final _tappables = find.byWidgetPredicate((widget) => widget is FTappable);

Future<void> _pump(WidgetTester tester, Widget child) => tester.pumpWidget(
  KeyTaoTheme(
    platform: BridgePlatform.macOs,
    home: Scaffold(
      body: Align(alignment: Alignment.topLeft, child: child),
    ),
  ),
);

Future<void> _finish(WidgetTester tester) async {
  await tester.pumpAndSettle();
  await tester.pumpWidget(const SizedBox.shrink());
  await tester.pumpAndSettle();
}

void _setViewport(WidgetTester tester, double width) {
  tester.view.devicePixelRatio = 1;
  tester.view.physicalSize = Size(width, 900);
}

void main() {
  test('operation log summaries preserve messages and malformed lines', () {
    expect(parseOperationLog('[14:21:05] [DEPLOY] 部署完成'), (
      time: '14:21',
      message: '部署完成',
    ));
    expect(parseOperationLog('[09:02] [DEPLOY ERROR] Failed [retry]'), (
      time: '09:02',
      message: 'Failed [retry]',
    ));
    for (final line in [
      '[25:21:05] [DEPLOY] bad time',
      '[14:61:05] [DEPLOY ERROR] bad minute',
      '[14:21:99] [DEPLOY] bad second',
      '[DEPLOY ERROR] missing timestamp',
      '[14:21:05] [DEPLOY]',
      'raw message',
    ]) {
      expect(parseOperationLog(line), (time: null, message: line));
    }
  });

  testWidgets('accent swatches fit desktop and balance narrow rows', (
    tester,
  ) async {
    final controller = ViewController(BridgePlatform.macOs);
    addTearDown(controller.dispose);
    for (final layout in [
      (width: 356.0, rows: [11]),
      (width: 300.0, rows: [6, 5]),
      (width: 180.0, rows: [4, 4, 3]),
    ]) {
      await _pump(
        tester,
        SizedBox(
          width: layout.width,
          child: AccentPicker(controller: controller),
        ),
      );
      final rowCounts = <double, int>{};
      for (final element in _tappables.evaluate()) {
        final y = tester
            .getTopLeft(find.byElementPredicate((e) => e == element))
            .dy;
        rowCounts.update(y, (count) => count + 1, ifAbsent: () => 1);
      }
      expect(rowCounts.values, layout.rows);
      expect(tester.takeException(), isNull);
    }
    await _finish(tester);
  });

  testWidgets('text radios expose exclusive checked and disabled semantics', (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();
    try {
      final changes = <int>[];
      await _pump(
        tester,
        BigTextRadioGroup<int>(
          label: '候选模式',
          value: 0,
          options: const {0: '横排', 1: '竖排', 2: '隐藏'},
          disabled: const {1},
          onChanged: changes.add,
        ),
      );
      expect(
        tester.getSemantics(find.byType(RadioGroup<int>)),
        isSemantics(role: SemanticsRole.radioGroup),
      );
      for (final label in ['横排', '竖排', '隐藏']) {
        expect(
          tester.getSemantics(find.bySemanticsLabel(label)),
          isSemantics(
            label: label,
            hasCheckedState: true,
            isChecked: label == '横排',
            isInMutuallyExclusiveGroup: true,
            isEnabled: label != '竖排',
            isButton: false,
          ),
        );
      }
      await tester.tap(find.text('竖排'));
      await tester.tap(find.text('横排'));
      expect(changes, isEmpty);
      await tester.tap(find.text('隐藏'));
      expect(changes, [2]);
    } finally {
      await _finish(tester);
      semantics.dispose();
    }
  });

  testWidgets('text radios support arrow wrapping and Space selection', (
    tester,
  ) async {
    var value = 0;
    final changes = <int>[];
    await _pump(
      tester,
      StatefulBuilder(
        builder: (context, setState) => BigTextRadioGroup<int>(
          label: '候选模式',
          value: value,
          options: const {0: '横排', 1: '竖排', 2: '隐藏'},
          disabled: const {1},
          onChanged: (selected) => setState(() {
            value = selected;
            changes.add(selected);
          }),
        ),
      ),
    );

    await tester.sendKeyEvent(LogicalKeyboardKey.tab);
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
    await tester.pump();
    expect(value, 2);
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
    await tester.pump();
    expect(value, 0);
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowLeft);
    await tester.pump();
    expect(value, 2);

    final firstRadio = tester.widget<FTappable>(
      find.ancestor(of: find.text('横排'), matching: _tappables),
    );
    firstRadio.focusNode!.requestFocus();
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.space);
    await tester.pump();
    expect(value, 0);
    await tester.sendKeyEvent(LogicalKeyboardKey.space);
    await tester.pump();
    expect(changes, [2, 0, 2, 0]);
    await _finish(tester);
  });

  testWidgets('text radios retain nullable options and disabled groups', (
    tester,
  ) async {
    final changes = <String?>[];
    await _pump(
      tester,
      BigTextRadioGroup<String?>(
        label: '配色',
        value: 'day',
        options: const {null: '自动', 'day': '白天'},
        onChanged: changes.add,
      ),
    );
    await tester.tap(find.text('自动'));
    expect(changes, [null]);

    await _pump(
      tester,
      const BigTextRadioGroup<String?>(
        label: '配色',
        value: 'day',
        options: {null: '自动', 'day': '白天'},
      ),
    );
    await tester.tap(find.text('自动'));
    await tester.sendKeyEvent(LogicalKeyboardKey.tab);
    await tester.sendKeyEvent(LogicalKeyboardKey.space);
    expect(changes, [null]);
    await _finish(tester);
  });

  testWidgets('text radio type size changes at the 720 layout boundary', (
    tester,
  ) async {
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    for (final width in [719.0, 720.0]) {
      _setViewport(tester, width);
      await _pump(
        tester,
        BigTextRadioGroup<int>(
          label: '候选模式',
          value: 0,
          options: const {0: '横排', 1: '竖排'},
          onChanged: (_) {},
        ),
      );
      expect(
        tester.widget<Text>(find.text('横排')).style!.fontSize,
        width < 720 ? 24 : 36,
      );
      expect(tester.getSize(_tappables.first).height, greaterThanOrEqualTo(44));
    }
    await _finish(tester);
  });

  testWidgets(
    'hairline grids reflow at cell widths and stay stacked on phone',
    (tester) async {
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      for (final layout in [
        (viewport: 1000.0, width: 639.0, min: 320.0, columns: 1),
        (viewport: 1000.0, width: 640.0, min: 320.0, columns: 2),
        (viewport: 1000.0, width: 960.0, min: 320.0, columns: 3),
        (viewport: 1000.0, width: 879.0, min: 440.0, columns: 1),
        (viewport: 1000.0, width: 880.0, min: 440.0, columns: 2),
        (viewport: 700.0, width: 700.0, min: 320.0, columns: 1),
        (viewport: 390.0, width: 390.0, min: 440.0, columns: 1),
      ]) {
        _setViewport(tester, layout.viewport);
        await _pump(
          tester,
          SizedBox(
            width: layout.width,
            child: HairlineGridSection(
              minCellWidth: layout.min,
              children: [
                for (var i = 0; i < 3; i++)
                  HairlineGridCell(
                    key: ValueKey('cell-$i'),
                    child: const SizedBox(height: 24),
                  ),
              ],
            ),
          ),
        );
        final first = tester.getTopLeft(find.byKey(const ValueKey('cell-0')));
        for (var i = 0; i < 3; i++) {
          final cell = find.byKey(ValueKey('cell-$i'));
          final position = tester.getTopLeft(cell);
          expect(
            tester.getSize(cell).width,
            closeTo(layout.width / layout.columns, .01),
          );
          expect(
            position.dx,
            closeTo(
              first.dx + (i % layout.columns) * layout.width / layout.columns,
              .01,
            ),
          );
          expect(
            position.dy,
            i < layout.columns ? first.dy : greaterThan(first.dy),
          );
        }
        expect(tester.takeException(), isNull);
      }
      await _finish(tester);
    },
  );

  testWidgets('dialogs retain the user accent and action callbacks', (
    tester,
  ) async {
    const accent = Color(0xFF0F9F8F);
    Color? dialogAccent;
    Future<bool?>? result;
    var confirmations = 0;
    await _pump(
      tester,
      ChromeAccent(
        color: accent,
        child: Builder(
          builder: (context) => ActionButton(
            'Open',
            onPress: () {
              result = showFDialog<bool>(
                context: context,
                builder: (context, _, animation) {
                  dialogAccent = ChromeAccent.of(context);
                  return AppDialog(
                    title: 'Dialog',
                    animation: animation,
                    actions: [
                      ActionButton(
                        'Confirm',
                        onPress: () {
                          confirmations++;
                          Navigator.of(context).pop(true);
                        },
                      ),
                    ],
                    child: const Text('Content'),
                  );
                },
              );
            },
          ),
        ),
      ),
    );
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
    expect(dialogAccent, accent);
    await tester.tap(find.text('Confirm'));
    await tester.pumpAndSettle();
    expect(confirmations, 1);
    expect(await result, isTrue);
    expect(find.text('Confirm'), findsNothing);
    await _finish(tester);
  });

  for (final brightness in Brightness.values) {
    testWidgets('palette follows $brightness and preserves the user accent', (
      tester,
    ) async {
      addTearDown(tester.platformDispatcher.clearPlatformBrightnessTestValue);
      const accent = Color(0xFF0F9F8F);
      tester.platformDispatcher.platformBrightnessTestValue = brightness;
      final palette = brightness == Brightness.dark
          ? AppPalette.dark
          : AppPalette.light;
      await _pump(
        tester,
        ChromeAccent(
          color: accent,
          child: Builder(
            builder: (context) {
              expect(AppPalette.of(context), same(palette));
              expect(ChromeAccent.of(context), accent);
              expect(Theme.of(context).scaffoldBackgroundColor, palette.ground);
              expect(Theme.of(context).colorScheme.surface, palette.surface);
              expect(context.theme.colors.brightness, brightness);
              expect(context.theme.colors.background, palette.ground);
              expect(context.theme.colors.card, palette.surface);
              return const SizedBox.shrink();
            },
          ),
        ),
      );
      await _finish(tester);
    });
  }
}
