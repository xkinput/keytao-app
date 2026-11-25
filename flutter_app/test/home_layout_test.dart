import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/ui/input_page.dart';
import 'package:keytao/src/ui/settings.dart';
import 'package:keytao/src/ui/theme.dart';
import 'package:keytao/src/ui/widgets.dart';

import 'view_fixture.dart';

void main() {
  testWidgets('desktop home cards flow row-major in a single grid', (
    tester,
  ) async {
    final controller = ViewController(BridgePlatform.macOs)..appUpdate = null;
    addTearDown(controller.dispose);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetDevicePixelRatio);
    addTearDown(tester.view.resetPhysicalSize);

    for (final (width, columns) in [(1120.0, 3), (840.0, 2)]) {
      tester.view.physicalSize = Size(width, 1400);
      await tester.pumpWidget(
        KeyTaoTheme(
          platform: controller.info.platform,
          home: FScaffold(child: InputPage(controller: controller)),
        ),
      );
      await tester.pumpAndSettle();

      final cells = [
        for (final content in [
          find.widgetWithText(EyebrowLabel, '候选模式'),
          find.widgetWithText(EyebrowLabel, '外观'),
          find.byType(MacosImeCard),
          find.text('应用设置'),
          find.byType(AccountCard),
          find.text('刚刚'),
        ])
          find.ancestor(of: content, matching: find.byType(HairlineGridCell)),
      ];
      final grid = find.ancestor(
        of: cells.first,
        matching: find.byType(HairlineGridSection),
      );
      expect(grid, findsOneWidget);
      final positions = cells.map(tester.getTopLeft).toList();
      for (var i = 0; i < cells.length; i++) {
        final column = i % columns;
        expect(find.descendant(of: grid, matching: cells[i]), findsOneWidget);
        expect(positions[i].dy, positions[i - column].dy);
        expect(positions[i].dx, positions[column].dx);
        if (i >= columns) {
          expect(positions[i].dy, greaterThan(positions[i - columns].dy));
        }
        if (column > 0) {
          expect(positions[i].dx, greaterThan(positions[i - 1].dx));
        }
      }
    }
  });
}
