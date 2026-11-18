import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/app/controller.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/rust/frb_generated.dart';
import 'package:keytao/src/ui/input_page.dart';
import 'package:keytao/src/ui/theme.dart';

import 'controller_parity_test.dart' show FakeCore, deployedLocal;
import 'view_fixture.dart';

const _schemas = [
  SchemaDto(id: 'keytao', name: '键道6'),
  SchemaDto(id: 'keytao_single', name: '键道6·单字'),
  SchemaDto(id: 'keytao_chord', name: '键道6·并击'),
  SchemaDto(id: 'wanxiang', name: '万象拼音'),
];

class _SchemaController extends ViewController {
  _SchemaController() : super(BridgePlatform.macOs) {
    appUpdate = null;
    schemas = _schemas;
    currentSchemaId = 'keytao_single';
  }

  final selections = <String>[];

  @override
  Future<void> refreshSchemas() async {}

  @override
  Future<bool> selectSchema(String schemaId) async {
    selections.add(schemaId);
    currentSchemaId = schemaId;
    notifyListeners();
    return true;
  }
}

class _SchemaCore extends FakeCore {
  _SchemaCore() {
    local = deployedLocal;
  }

  String current = 'keytao_single';
  final selections = <String>[];

  @override
  Future<SchemaListDto> crateApiCoreGetSchemas() async =>
      SchemaListDto(current: current, schemas: _schemas);

  @override
  Future<void> crateApiCoreSelectSchema({required String schemaId}) async {
    selections.add(schemaId);
    current = schemaId;
  }
}

void main() {
  testWidgets(
    'header uses current schema and lists only ordered schema names',
    (tester) async {
      final c = _SchemaController();
      addTearDown(c.dispose);
      tester.view.devicePixelRatio = 1;
      tester.view.physicalSize = const Size(1200, 1000);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      await tester.pumpWidget(
        KeyTaoTheme(
          platform: c.info.platform,
          home: ListenableBuilder(
            listenable: c,
            builder: (_, _) => FScaffold(child: InputPage(controller: c)),
          ),
        ),
      );
      await tester.pumpAndSettle();

      final selector = find.byKey(const ValueKey('current-schema-selector'));
      expect(
        find.descendant(of: selector, matching: find.text('键道6·单字')),
        findsOneWidget,
      );
      await tester.tap(selector);
      await tester.pumpAndSettle();
      expect(
        tester
            .widgetList<Text>(
              find.descendant(
                of: find.byType(FItem),
                matching: find.byType(Text),
              ),
            )
            .map((text) => text.data),
        _schemas.map((schema) => schema.name),
      );
      final chosen = find.descendant(
        of: find.byType(FItem),
        matching: find.text('键道6·并击'),
      );
      await tester.tap(chosen);
      await tester.pumpAndSettle();
      expect(c.selections, ['keytao_chord']);
      expect(
        find.descendant(of: selector, matching: find.text('键道6·并击')),
        findsOneWidget,
      );
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  test(
    'schema refresh follows IME changes and selection does not deploy',
    () async {
      final api = _SchemaCore();
      RustLib.initMock(api: api);
      final c = AppController(
        info: const BridgeInfo(
          platform: BridgePlatform.macOs,
          appVersion: 'test',
          userRoot: '/isolated/schema-selector',
        ),
      )..local = deployedLocal;
      addTearDown(() async {
        c.dispose();
        await api.events.close();
        RustLib.dispose();
      });

      await c.refreshSchemas();
      expect(c.currentSchemaName, '键道6·单字');
      api.current = 'wanxiang';
      await c.refreshSchemas();
      expect(c.currentSchemaName, '万象拼音');
      expect(await c.selectSchema('keytao_chord'), isTrue);
      expect(api.selections, ['keytao_chord']);
      expect(c.currentSchemaName, '键道6·并击');
      expect(api.calls.where((call) => call.contains('deploy')), isEmpty);
    },
  );
}
