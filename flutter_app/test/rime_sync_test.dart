import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/app/controller.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/rust/frb_generated.dart';
import 'package:keytao/src/ui/library_page.dart';
import 'package:keytao/src/ui/settings.dart' show AccountCard;
import 'package:keytao/src/ui/theme.dart';
import 'package:keytao/src/ui/widgets.dart';

const _initial = RimeSyncSettingsDto(
  installationId: 'desktop',
  syncDir: '',
  effectiveSyncDir: '/isolated/rime-sync/sync',
);

class _SyncCore implements RustLibApi {
  final saves = <({String installationId, String syncDir})>[];
  Completer<void>? sync;
  String? saveFailure;

  @override
  Future<RimeSyncSettingsDto> crateApiCoreGetRimeSyncSettings() async =>
      _initial;

  @override
  Future<RimeSyncSettingsDto> crateApiCoreSaveRimeSyncSettings({
    required String installationId,
    required String syncDir,
  }) async {
    saves.add((installationId: installationId, syncDir: syncDir));
    if (saveFailure case final failure?) throw StateError(failure);
    return RimeSyncSettingsDto(
      installationId: installationId.trim(),
      syncDir: syncDir,
      effectiveSyncDir: syncDir.isEmpty ? _initial.effectiveSyncDir : syncDir,
    );
  }

  @override
  Future<void> crateApiCoreSyncRimeUserData() => sync!.future;

  @override
  dynamic noSuchMethod(Invocation invocation) =>
      throw StateError('Unexpected bridge call: ${invocation.memberName}');
}

class _SyncController extends AppController {
  _SyncController(BridgePlatform platform)
    : super(
        info: BridgeInfo(
          platform: platform,
          appVersion: 'test',
          userRoot: '/isolated/rime-sync',
        ),
        pickDesktopDirectory: () async => '/isolated/picked-sync',
      );

  int opens = 0;

  @override
  Future<bool> openRimeSyncDirectory() async {
    opens++;
    return true;
  }
}

Future<void> _show(WidgetTester tester, AppController controller) async {
  await tester.pumpWidget(
    KeyTaoTheme(
      platform: controller.info.platform,
      home: FScaffold(
        child: ListenableBuilder(
          listenable: controller,
          builder: (_, _) => LibraryPage(controller: controller),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

void main() {
  late _SyncCore core;
  late _SyncController controller;

  setUp(() {
    core = _SyncCore();
    RustLib.initMock(api: core);
    controller = _SyncController(BridgePlatform.macOs);
  });

  tearDown(() {
    controller.dispose();
    RustLib.dispose();
  });

  for (final platform in [
    BridgePlatform.macOs,
    BridgePlatform.windows,
    BridgePlatform.linux,
    BridgePlatform.android,
    BridgePlatform.ios,
  ]) {
    testWidgets('sync dialog visibility on $platform', (tester) async {
      controller.dispose();
      controller = _SyncController(platform);
      await _show(tester, controller);
      final mobile =
          platform == BridgePlatform.android || platform == BridgePlatform.ios;
      final button = find.byKey(const ValueKey('rime-sync-button'));
      expect(button, mobile ? findsNothing : findsOneWidget);
      expect(find.text('同步文件夹名称'), findsNothing);
      expect(find.text('同步路径'), findsNothing);
      if (!mobile) {
        expect(
          find.descendant(of: find.byType(AccountCard), matching: button),
          findsOneWidget,
        );
        await tester.tap(button);
        await tester.pumpAndSettle();
        expect(find.byType(AppDialog), findsOneWidget);
        expect(find.text('同步文件夹名称'), findsOneWidget);
        expect(find.text('同步路径'), findsOneWidget);
        expect(controller.rimeSyncSettings, _initial);
        await tester.tap(find.text('关闭'));
        await tester.pumpAndSettle();
        expect(find.byType(AppDialog), findsNothing);
      }
      await tester.pumpWidget(const SizedBox.shrink());
    });
  }

  testWidgets(
    'sync controls save, choose, reset, open and sync through controller',
    (tester) async {
      await _show(tester, controller);
      await tester.tap(find.byKey(const ValueKey('rime-sync-button')));
      await tester.pumpAndSettle();
      final id = find.byKey(const ValueKey('rime-sync-installation-id'));
      final path = find.byKey(const ValueKey('rime-sync-directory'));
      expect(tester.widget<FTextField>(path).hint, _initial.effectiveSyncDir);

      await tester.enterText(id, 'new-desktop');
      await tester.tap(find.text('保存'));
      await tester.pumpAndSettle();
      expect(core.saves.last.installationId, 'new-desktop');

      await tester.tap(find.text('选择'));
      await tester.pumpAndSettle();
      expect(core.saves.last.syncDir, '/isolated/picked-sync');
      expect(
        tester
            .widget<EditableText>(
              find.descendant(of: path, matching: find.byType(EditableText)),
            )
            .controller
            .text,
        '/isolated/picked-sync',
      );

      await tester.tap(find.text('重置'));
      await tester.pumpAndSettle();
      expect(core.saves.last.syncDir, isEmpty);
      expect(tester.widget<FTextField>(path).hint, _initial.effectiveSyncDir);

      await tester.enterText(path, '/isolated/manual-sync');
      await tester.testTextInput.receiveAction(TextInputAction.done);
      await tester.pumpAndSettle();
      expect(core.saves.last.syncDir, '/isolated/manual-sync');

      await tester.enterText(path, '/isolated/open-sync');
      await tester.tap(find.text('打开'));
      await tester.pumpAndSettle();
      expect(core.saves.last.syncDir, '/isolated/open-sync');
      expect(controller.opens, 1);

      await tester.enterText(path, '/isolated/run-sync');
      core.sync = Completer<void>();
      await tester.tap(find.text('立即同步'));
      await tester.pump();
      expect(core.saves.last.syncDir, '/isolated/run-sync');
      expect(find.text('同步中'), findsOneWidget);
      expect(find.byType(FProgress), findsWidgets);
      expect(controller.busy, isTrue);
      core.sync!.complete();
      await tester.pumpAndSettle();
      expect(find.text('完成'), findsOneWidget);

      core.sync = Completer<void>();
      await tester.tap(find.text('完成'));
      await tester.pump();
      core.sync!.completeError(StateError('IME is not running'));
      await tester.pumpAndSettle();
      expect(find.text('失败'), findsOneWidget);
      expect(
        find.widgetWithText(ErrorMessage, 'IME is not running'),
        findsOneWidget,
      );
      expect(controller.unhandledError, isNull);

      core.saveFailure = 'Invalid installation_id';
      await tester.enterText(id, '../invalid');
      await tester.tap(find.text('保存'));
      await tester.pumpAndSettle();
      expect(
        find.widgetWithText(ErrorMessage, 'Invalid installation_id'),
        findsOneWidget,
      );
      expect(controller.rimeSyncSettings!.installationId, 'new-desktop');
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );
}
