import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/ui/candidate_panel_preview.dart';
import 'package:keytao/src/ui/input_page.dart';
import 'package:keytao/src/ui/settings.dart';
import 'package:keytao/src/ui/theme.dart';

import 'view_fixture.dart';

void main() {
  testWidgets(
    'macOS input keeps a two-column header and separate scheme rows',
    (tester) async {
      final controller = ViewController(BridgePlatform.macOs)
        ..appUpdate = null
        ..schemaSwitches = const [
          SchemaSwitchDto(
            schemaId: 'keytao',
            name: 'ascii_mode',
            states: ['中文', '西文'],
            options: [],
            value: 0,
          ),
        ]
        ..installedPackages = const [
          InstalledPackageDto(
            schemeKey: 'keytao',
            active: true,
            hasPackageZip: true,
          ),
          InstalledPackageDto(
            schemeKey: 'local-custom',
            source: 'local',
            displayName: '本地双拼',
            active: false,
            hasPackageZip: true,
          ),
        ];
      addTearDown(controller.dispose);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetDevicePixelRatio);
      addTearDown(tester.view.resetPhysicalSize);

      for (final width in [390.0, 840.0, 1120.0, 1440.0]) {
        tester.view.physicalSize = Size(width, 1400);
        await tester.pumpWidget(
          KeyTaoTheme(
            platform: controller.info.platform,
            home: FScaffold(child: InputPage(controller: controller)),
          ),
        );
        await tester.pumpAndSettle();

        expect(find.text('当前方案 · Rime'), findsOneWidget);
        expect(find.text('远程方案'), findsOneWidget);
        expect(find.byType(CandidatePanelPreview), findsNothing);
        final switches = find.byType(SchemaSwitchesSection);
        final header = find
            .ancestor(of: switches, matching: find.byType(Row))
            .first;
        expect(
          find.descendant(
            of: header,
            matching: find.byKey(const ValueKey('current-schema-selector')),
          ),
          findsOneWidget,
        );
        final columns = tester.widget<Row>(header).children;
        expect(columns, hasLength(2));
        expect(columns, everyElement(isA<Expanded>()));
        final left = tester.getRect(find.byWidget(columns[0]));
        final right = tester.getRect(find.byWidget(columns[1]));
        expect(left.width, closeTo(right.width, 0.5));
        expect(left.right, closeTo(right.left, 0.5));

        final remote = find.byKey(const ValueKey('cloud-schemes'));
        final local = find.byKey(const ValueKey('local-schemes'));
        expect(
          tester.getTopLeft(remote).dy,
          greaterThanOrEqualTo(tester.getBottomLeft(header).dy),
        );
        expect(
          tester.getTopLeft(local).dy,
          greaterThanOrEqualTo(tester.getBottomLeft(remote).dy),
        );
        expect(tester.takeException(), isNull, reason: 'width $width');
      }
    },
  );
}
