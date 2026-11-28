import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/app/controller.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/rust/frb_generated.dart';
import 'package:keytao/src/ui/input_page.dart';
import 'package:keytao/src/ui/library_page.dart';
import 'package:keytao/src/ui/settings.dart' show AccountCard;
import 'package:keytao/src/ui/theme.dart';
import 'package:keytao/src/ui/widgets.dart';

final _initial = RimeSyncSettingsDto(
  installationId: 'desktop',
  syncDir: '',
  effectiveSyncDir: '/isolated/rime-sync/sync',
  defaultSyncDir: '/isolated/rime-sync/runtime/keytao/sync',
  packageSyncDir: '/isolated/rime-sync/runtime/keytao/sync',
  lastSyncAt: BigInt.zero,
);

const _newId = 'f906573a-85d0-4691-b08e-75b76ea4cd09';

class _SyncCore implements RustLibApi {
  final saves = <({String installationId, String syncDir})>[];
  Completer<void>? sync;
  int syncCalls = 0;
  String? saveFailure;
  String? lastSyncError;
  BigInt lastSyncAt = BigInt.zero;
  String _installationId = _initial.installationId;
  String _syncDir = '';
  String _defaultSyncDir = _initial.defaultSyncDir;

  RimeSyncSettingsDto get _settings => RimeSyncSettingsDto(
    installationId: _installationId,
    syncDir: _syncDir,
    effectiveSyncDir: _syncDir.isEmpty ? _initial.effectiveSyncDir : _syncDir,
    defaultSyncDir: _defaultSyncDir,
    packageSyncDir: _syncDir.isEmpty
        ? _defaultSyncDir
        : '$_syncDir/keytao-packages/keytao',
    lastSyncAt: lastSyncAt,
    lastSyncError: lastSyncError,
  );

  @override
  Future<RimeSyncSettingsDto> crateApiCoreGetRimeSyncSettings() async =>
      _settings;

  @override
  Future<RimeSyncSettingsDto> crateApiCoreSaveRimeSyncSettings({
    required String installationId,
    required String syncDir,
  }) async {
    saves.add((installationId: installationId, syncDir: syncDir));
    if (saveFailure case final failure?) throw StateError(failure);
    _installationId = installationId.trim();
    _syncDir = syncDir;
    return _settings;
  }

  @override
  Future<void> crateApiCoreSyncRimeUserData() {
    syncCalls++;
    return sync!.future;
  }

  @override
  Future<String> crateApiCoreNewRimeInstallationId() async => _newId;

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
      ) {
    local = const LocalSchemaDto(
      installed: true,
      deployed: true,
      installedScheme: 'keytao',
      schemas: ['keytao'],
    );
    schemas = const [SchemaDto(id: 'keytao', name: '键道6')];
    currentSchemaId = 'keytao';
    macosIme = const MacosImeStatusDto(
      installed: true,
      appPath: '/isolated/rime-sync/KeyTao.app',
      message: '',
    );
  }

  int opens = 0;

  @override
  Future<void> refreshSchemas() async {}

  @override
  bool shouldFetchRemoteSchemeLogo(
    String key, {
    String? repoUrl,
    bool? addonInstalled,
  }) => false;

  @override
  Future<bool> openRimeSyncDirectory({Locale? locale}) async {
    opens++;
    return true;
  }
}

Future<void> _show(
  WidgetTester tester,
  AppController controller, {
  bool library = false,
}) async {
  await tester.pumpWidget(
    KeyTaoTheme(
      platform: controller.info.platform,
      home: FScaffold(
        child: ListenableBuilder(
          listenable: controller,
          builder: (_, _) =>
              controller.info.platform == BridgePlatform.macOs && !library
              ? InputPage(controller: controller)
              : LibraryPage(controller: controller),
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
        if (platform == BridgePlatform.macOs) {
          expect(
            find.descendant(of: find.byType(InputPage), matching: button),
            findsOneWidget,
          );
          expect(
            find.descendant(
              of: find.byKey(const ValueKey('current-scheme-actions')),
              matching: button,
            ),
            findsOneWidget,
          );
        } else {
          expect(
            find.descendant(of: find.byType(AccountCard), matching: button),
            findsOneWidget,
          );
        }
        final settingsButton = platform == BridgePlatform.macOs
            ? find.byKey(const ValueKey('rime-sync-settings-button'))
            : button;
        await tester.tap(settingsButton);
        await tester.pumpAndSettle();
        expect(core.syncCalls, 0);
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

  testWidgets('macOS sync runs immediately without opening settings', (
    tester,
  ) async {
    core.sync = Completer<void>();
    await _show(tester, controller);
    final sync = find.byKey(const ValueKey('rime-sync-button'));
    final settings = find.byKey(const ValueKey('rime-sync-settings-button'));
    expect(controller.rimeSyncSettings, isNull);
    expect(
      tester.getRect(settings).left,
      greaterThan(tester.getRect(sync).right),
    );
    expect(tester.getRect(settings).center.dy, tester.getRect(sync).center.dy);
    await tester.tap(sync);
    await tester.pump();
    expect(core.syncCalls, 1);
    expect(core.saves, isEmpty);
    expect(controller.rimeSyncing, isTrue);
    expect(find.byType(AppDialog), findsNothing);
    expect(tester.widget<ActionButton>(sync).onPress, isNull);
    expect(tester.widget<FButton>(settings).onPress, isNull);
    core.sync!.complete();
    await tester.pumpAndSettle();
    expect(controller.rimeSyncing, isFalse);
    expect(controller.rimeSyncSettings, _initial);
    expect(tester.widget<ActionButton>(sync).onPress, isNotNull);
    expect(tester.widget<FButton>(settings).onPress, isNotNull);
    core.sync = Completer<void>();
    await tester.tap(sync);
    await tester.pump();
    core.sync!.completeError(StateError('IME is not running'));
    await tester.pumpAndSettle();
    expect(core.syncCalls, 2);
    expect(
      find.widgetWithText(ErrorMessage, 'IME is not running'),
      findsOneWidget,
    );
    expect(find.byType(AppDialog), findsNothing);
    await tester.tap(settings);
    await tester.pumpAndSettle();
    expect(find.byType(AppDialog), findsOneWidget);
    expect(core.syncCalls, 2);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets('macOS library no longer shows the sync entry', (tester) async {
    await _show(tester, controller, library: true);

    expect(find.byType(LibraryPage), findsOneWidget);
    expect(find.byType(AccountCard), findsNothing);
    expect(find.text('账号与同步'), findsNothing);
    expect(find.byKey(const ValueKey('rime-sync-button')), findsNothing);
    expect(find.byType(AppDialog), findsNothing);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  for (final savedPath in [
    '',
    '/isolated/saved-sync',
    _initial.defaultSyncDir,
  ]) {
    testWidgets('ID reset keeps saved path "$savedPath" and path draft', (
      tester,
    ) async {
      core._syncDir = savedPath;
      await _show(tester, controller);
      await tester.tap(find.byKey(const ValueKey('rime-sync-settings-button')));
      await tester.pumpAndSettle();

      final path = find.byKey(const ValueKey('rime-sync-directory'));
      const draft = '/isolated/unsaved-sync';
      await tester.enterText(path, draft);
      await tester.tap(
        find.descendant(
          of: find.widgetWithText(SettingRow, '同步文件夹名称'),
          matching: find.text('重置'),
        ),
      );
      await tester.pumpAndSettle();

      expect(core.saves, [(installationId: _newId, syncDir: savedPath)]);
      expect(controller.rimeSyncSettings!.syncDir, savedPath);
      expect(find.text(_newId), findsOneWidget);
      expect(find.text(draft), findsOneWidget);
      await tester.pumpWidget(const SizedBox.shrink());
    });
  }

  for (final choose in [true, false]) {
    final action = choose ? '选择' : '重置';
    final savedPath = choose ? '/isolated/picked-sync' : '';
    testWidgets(
      'path ${choose ? 'choose' : 'reset'} keeps saved ID and draft',
      (tester) async {
        core._syncDir = '/isolated/saved-sync';
        await _show(tester, controller);
        await tester.tap(
          find.byKey(const ValueKey('rime-sync-settings-button')),
        );
        await tester.pumpAndSettle();

        const draft = 'unsaved-desktop';
        final id = find.byKey(const ValueKey('rime-sync-installation-id'));
        await tester.enterText(id, draft);
        await tester.tap(
          find.descendant(
            of: find.widgetWithText(SettingRow, '同步路径'),
            matching: find.text(action),
          ),
        );
        await tester.pumpAndSettle();

        expect(core.saves, [
          (installationId: _initial.installationId, syncDir: savedPath),
        ]);
        expect(
          controller.rimeSyncSettings!.installationId,
          _initial.installationId,
        );
        expect(find.text(draft), findsOneWidget);
        expect(
          find.text(choose ? savedPath : _initial.defaultSyncDir),
          findsOneWidget,
        );
        await tester.pumpWidget(const SizedBox.shrink());
      },
    );
  }

  testWidgets('pinned legacy path displays and saves as the default', (
    tester,
  ) async {
    const pinnedPath = '/isolated/rime-sync/sync/keytao-packages/keytao';
    core._defaultSyncDir = pinnedPath;
    await _show(tester, controller);
    await tester.tap(find.byKey(const ValueKey('rime-sync-settings-button')));
    await tester.pumpAndSettle();

    expect(find.text(pinnedPath), findsOneWidget);
    expect(find.text('默认'), findsOneWidget);
    await tester.enterText(
      find.byKey(const ValueKey('rime-sync-installation-id')),
      'updated-desktop',
    );
    await tester.pump();
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();

    expect(core.saves.single.syncDir, isEmpty);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  for (final (label, custom, legacy) in [
    ('custom', true, false),
    ('default', false, false),
    ('pinned legacy', false, true),
  ]) {
    test('open $label sync directory', () async {
      final root = await Directory.systemTemp.createTemp('keytao-rime-sync-');
      addTearDown(() => root.delete(recursive: true));
      final opened = <String>[];
      final openController = AppController(
        info: controller.info,
        runDesktopOpen: (_, arguments) async {
          opened.add(arguments.single);
          return ProcessResult(0, 0, '', '');
        },
      );
      addTearDown(openController.dispose);

      final runtime = '${root.path}/runtime/keytao';
      final defaultPath = '$runtime/sync';
      final customPath = '${root.path}/custom';
      final packagePath = custom
          ? '$customPath/keytao-packages/keytao'
          : legacy
          ? '${root.path}/sync/keytao-packages/keytao'
          : defaultPath;
      final directory = Directory(packagePath);
      final parent = custom ? Directory(customPath) : directory.parent;
      openController.rimeSyncSettings = RimeSyncSettingsDto(
        installationId: _initial.installationId,
        syncDir: custom ? customPath : '',
        effectiveSyncDir: custom ? customPath : '${root.path}/sync',
        defaultSyncDir: custom ? defaultPath : packagePath,
        packageSyncDir: packagePath,
        lastSyncAt: BigInt.zero,
      );

      for (final (locale, message) in [
        (const Locale('zh'), '同步文件夹不可用：${parent.path}'),
        (const Locale('en'), 'Sync folder unavailable: ${parent.path}'),
      ]) {
        expect(
          await openController.openRimeSyncDirectory(locale: locale),
          isFalse,
        );
        expect(openController.rimeSyncError, message);
        expect(await parent.exists(), isFalse);
        expect(await directory.exists(), isFalse);
        expect(opened, isEmpty);
      }

      await parent.create(recursive: true);
      expect(await openController.openRimeSyncDirectory(), isTrue);
      expect(await directory.exists(), isTrue);
      expect(openController.rimeSyncError, isNull);
      expect(opened, [packagePath]);
    });
  }

  testWidgets('sync settings read error leaves path editable', (tester) async {
    const error = 'Could not read sync result';
    core.lastSyncError = error;
    await _show(tester, controller);
    await tester.tap(find.byKey(const ValueKey('rime-sync-settings-button')));
    await tester.pumpAndSettle();

    expect(
      find.descendant(
        of: find.byType(AppDialog),
        matching: find.widgetWithText(ErrorMessage, error),
      ),
      findsOneWidget,
    );
    final path = find.byKey(const ValueKey('rime-sync-directory'));
    await tester.enterText(path, '/isolated/replacement-sync');
    await tester.pump();
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();
    expect(core.saves.single.syncDir, '/isolated/replacement-sync');
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets(
    'sync controls save, choose, reset, open and sync through controller',
    (tester) async {
      await _show(tester, controller);
      await tester.tap(find.byKey(const ValueKey('rime-sync-settings-button')));
      await tester.pumpAndSettle();
      final id = find.byKey(const ValueKey('rime-sync-installation-id'));
      final path = find.byKey(const ValueKey('rime-sync-directory'));
      String pathText() => tester
          .widget<EditableText>(
            find.descendant(of: path, matching: find.byType(EditableText)),
          )
          .controller
          .text;
      expect(pathText(), _initial.defaultSyncDir);

      await tester.enterText(id, 'new-desktop');
      await tester.pump();
      await tester.tap(find.text('保存'));
      await tester.pumpAndSettle();
      expect(core.saves.last.installationId, 'new-desktop');

      await tester.tap(find.text('选择'));
      await tester.pumpAndSettle();
      expect(core.saves.last.syncDir, '/isolated/picked-sync');
      expect(pathText(), '/isolated/picked-sync');

      await tester.tap(
        find.descendant(
          of: find.widgetWithText(SettingRow, '同步路径'),
          matching: find.text('重置'),
        ),
      );
      await tester.pumpAndSettle();
      expect(core.saves.last.syncDir, isEmpty);
      expect(pathText(), _initial.defaultSyncDir);

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
      expect(
        find.descendant(of: find.byType(AppDialog), matching: find.text('同步中')),
        findsOneWidget,
      );
      expect(find.byType(FProgress), findsWidgets);
      expect(controller.busy, isTrue);
      core.lastSyncAt = BigInt.from(
        DateTime(2026, 11, 25, 10, 20).millisecondsSinceEpoch,
      );
      core.sync!.complete();
      await tester.pumpAndSettle();
      expect(
        find.descendant(
          of: find.byType(AppDialog),
          matching: find.text('上次同步 2026-11-25 10:20'),
        ),
        findsOneWidget,
      );

      core.sync = Completer<void>();
      await tester.tap(find.text('立即同步'));
      await tester.pump();
      core.lastSyncError = 'IME is not running';
      core.sync!.completeError(StateError('IME is not running'));
      await tester.pumpAndSettle();
      expect(
        find.descendant(
          of: find.byType(AppDialog),
          matching: find.widgetWithText(ErrorMessage, 'IME is not running'),
        ),
        findsOneWidget,
      );
      expect(controller.unhandledError, isNull);

      core.saveFailure = 'Invalid installation_id';
      await tester.enterText(id, '../invalid');
      await tester.pump();
      await tester.tap(find.text('保存'));
      await tester.pumpAndSettle();
      expect(
        find.descendant(
          of: find.byType(AppDialog),
          matching: find.widgetWithText(
            ErrorMessage,
            'Invalid installation_id',
          ),
        ),
        findsOneWidget,
      );
      expect(controller.rimeSyncSettings!.installationId, 'new-desktop');
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );
}
