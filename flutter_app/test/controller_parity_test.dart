import 'dart:async';
import 'dart:convert';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:keytao/src/app/controller.dart';
import 'package:keytao/src/app/options.dart';
import 'package:keytao/src/platform/android_host.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/rust/frb_generated.dart';

const installedLocal = LocalSchemaDto(
  installed: true,
  deployed: false,
  version: 'local',
  schemas: ['keytao'],
);
const deployedLocal = LocalSchemaDto(
  installed: true,
  deployed: true,
  version: 'local',
  schemas: ['keytao'],
);
const release = ReleaseInfoDto(
  version: 'new',
  name: 'Release',
  publishedAt: '',
  body: 'Release notes',
  github: PlatformReleaseDto(
    version: 'gh',
    downloadUrls: DownloadUrlsDto(android: 'offline-gh', macos: 'offline-gh'),
  ),
  gitee: PlatformReleaseDto(
    version: 'ge',
    downloadUrls: DownloadUrlsDto(android: 'offline-ge', macos: 'offline-ge'),
  ),
);
const extracted = <String, Object>{
  'mergedSchemas': ['custom'],
  'logs': ['[MERGED] default.custom.yaml'],
  'verify': [
    {'path': 'custom.schema.yaml', 'ok': false, 'note': 'missing'},
  ],
};
const result = InstallResultDto(
  mergedSchemas: ['custom'],
  logs: ['[MERGED] default.custom.yaml'],
  verify: [
    VerifyEntryDto(path: 'custom.schema.yaml', ok: false, note: 'missing'),
  ],
);
const emptyLog = DebugLogFileDto(lines: [], truncated: false);

// Only the external contract is mocked: no native libraries, HTTP or live roots.
class FakeCore implements RustLibApi {
  final calls = <String>[];
  final events = StreamController<BridgeEvent>.broadcast();
  LocalSchemaDto local = installedLocal;
  final latestRequests = <Completer<ReleaseInfoDto>>[];
  final schemeRequests = <String, Completer<SchemeReleaseDto>>{};
  bool delayReleases = false;
  bool updateFails = false;
  final logFailures = <String>{};
  Completer<void>? deployGate;
  EnglishModeDto english = EnglishModeDto.schema;
  int onboardingWrites = 0;

  @override
  dynamic noSuchMethod(Invocation invocation) =>
      throw StateError('Unexpected bridge call: ${invocation.memberName}');
  @override
  Stream<BridgeEvent> crateApiCoreCoreEvents() async* {
    yield const BridgeEvent(kind: BridgeEventKind.ready);
    yield* events.stream;
  }

  @override
  Future<LocalSchemaDto> crateApiCoreCheckLocalSchema() async => local;
  @override
  Future<AddonSchemaStatusDto> crateApiCoreAddonSchemaStatus({
    required String id,
  }) async => const AddonSchemaStatusDto(
    englishAvailable: false,
    installed: false,
    deployed: false,
    version: '1',
  );
  @override
  Future<WanxiangStatusDto> crateApiCoreWanxiangStatus() async =>
      const WanxiangStatusDto(installed: false, deployed: false, version: '1');
  @override
  Future<MacosImeStatusDto> crateApiCoreMacosImeStatus() async =>
      const MacosImeStatusDto(
        installed: true,
        sharedDataSource: '',
        message: '',
      );
  @override
  Future<ComponentVersionsDto> crateApiCoreGetComponentVersions() async =>
      const ComponentVersionsDto(appVersion: 'test');
  @override
  Future<ImeUiSettingsDto> crateApiCoreGetImeUiSettings() async =>
      const ImeUiSettingsDto(
        colorScheme: UiColorSchemeDto.auto,
        effectiveColorScheme: EffectiveColorSchemeDto.light,
        orientation: PanelOrientationDto.horizontal,
        embeddedComposition: false,
        accentColor: '#3B73D9',
        fontSize: 20,
        themeExists: false,
        message: '',
      );
  @override
  Future<EnglishModeDto> crateApiCoreGetDesktopEnglishMode() async {
    calls.add('englishMode');
    return english;
  }

  @override
  Future<AppUpdateDto> crateApiCoreCheckAppUpdate() async {
    calls.add('update');
    if (updateFails) throw StateError('offline update');
    return const AppUpdateDto(
      hasUpdate: true,
      latestVersion: '2',
      currentVersion: '1',
      releaseUrl: 'offline-update',
    );
  }

  @override
  Future<ReleaseInfoDto> crateApiCoreFetchLatestRelease() {
    calls.add('latest');
    if (!delayReleases) return Future.value(release);
    final request = Completer<ReleaseInfoDto>();
    latestRequests.add(request);
    return request.future;
  }

  @override
  Future<SchemeReleaseDto> crateApiCoreFetchSchemeRelease({
    required String scheme,
  }) {
    calls.add('release:$scheme');
    return (schemeRequests[scheme] = Completer<SchemeReleaseDto>()).future;
  }

  @override
  Future<String> crateApiCoreDownloadSchemeArchive({
    required String url,
  }) async {
    calls.add('download:$url');
    return '/cache/archive.zip';
  }

  @override
  Future<void> crateApiCoreRemoveDownloadedArchive({
    required String path,
  }) async {
    calls.add('remove:$path');
  }

  @override
  Future<InstallResultDto> crateApiCoreFinishAndroidInstall({
    required String resultJson,
  }) async {
    calls.add('finish');
    expect(jsonDecode(resultJson), extracted);
    return result;
  }

  @override
  Future<InstallResultDto> crateApiCoreInstallSchemeToDir({
    required String url,
    required String dir,
  }) async {
    calls.add('installTo:$url:$dir');
    return result;
  }

  @override
  Future<List<FileItemDto>> crateApiCoreListDir({required String dir}) async =>
      [const FileItemDto(name: 'default.custom.yaml', isDir: false)];
  @override
  Future<LocalSchemasDto> crateApiCoreReadLocalSchemas({
    required String dir,
  }) async =>
      const LocalSchemasDto(hasDefaultCustom: true, schemas: ['custom']);
  @override
  Future<AddonSchemaStatusDto> crateApiCoreAddonSchemaInstall({
    required String id,
    required FutureOr<String> Function() androidDeploy,
  }) async {
    calls.add('addonInstall:$id');
    final failure = await androidDeploy();
    calls.add('callback:$failure');
    if (failure.isNotEmpty) throw StateError(failure);
    return const AddonSchemaStatusDto(
      englishAvailable: true,
      installed: true,
      deployed: true,
      version: '1',
    );
  }

  @override
  Future<AddonSchemaStatusDto> crateApiCoreAddonSchemaUninstall({
    required String id,
    required FutureOr<String> Function() androidDeploy,
  }) async {
    calls.add('addonUninstall:$id');
    english = EnglishModeDto.ascii;
    return crateApiCoreAddonSchemaStatus(id: id);
  }

  @override
  Future<WanxiangStatusDto> crateApiCoreManageWanxiang({
    required bool installed,
    required FutureOr<String> Function() androidDeploy,
  }) async {
    calls.add('wanxiang:$installed');
    final failure = await androidDeploy();
    calls.add('callback:$failure');
    if (failure.isNotEmpty) throw StateError(failure);
    return WanxiangStatusDto(
      installed: installed,
      deployed: installed,
      version: '1',
    );
  }

  @override
  Future<DeployResultDto> crateApiCoreDeployDefault() async {
    await deployGate?.future;
    local = deployedLocal;
    return const DeployResultDto(success: true, message: 'done');
  }

  @override
  Future<void> crateApiCoreCompleteOnboarding() async {
    onboardingWrites++;
  }

  @override
  Future<RuntimeLogSettingsDto> crateApiCoreGetRuntimeLogSettings() async {
    calls.add('settings');
    if (logFailures.contains('settings')) throw StateError('settings');
    return RuntimeLogSettingsDto(
      enabled: true,
      level: RuntimeLogLevelDto.info,
      logDir: '/logs',
      files: [RuntimeLogFileInfoDto(name: 'test.log', size: BigInt.from(2048))],
      fileCount: 1,
      totalBytes: BigInt.from(2048),
    );
  }

  @override
  Future<DebugLogFileDto> crateApiCoreReadRuntimeLog({int? maxLines}) async {
    calls.add('runtime');
    expect(maxLines, isNull);
    if (logFailures.contains('runtime')) throw StateError('runtime');
    return const DebugLogFileDto(lines: ['runtime'], truncated: true);
  }

  @override
  Future<DebugLogsDto> crateApiCoreReadDebugLogs() async {
    calls.add('system');
    if (logFailures.contains('system')) throw StateError('system');
    return const DebugLogsDto(
      ime: DebugLogFileDto(lines: ['ime'], truncated: true),
      app: emptyLog,
    );
  }
}

class FakeAndroid extends AndroidHost {
  FakeAndroid(this.calls);
  final List<String> calls;
  final progress = StreamController<InstallProgressDto>.broadcast();
  Map<String, dynamic> permission = {'granted': true, 'writable': true};
  Completer<Map<String, dynamic>>? extraction;
  bool failDeploy = false;
  bool deployed = true;
  @override
  Stream<InstallProgressDto> get installProgress => progress.stream;
  @override
  Future<Map<String, dynamic>> storagePermissionStatus() async {
    calls.add('permission');
    return permission;
  }

  @override
  Future<Map<String, dynamic>> imeStatus() async => {
    'enabled': true,
    'selected': true,
  };
  @override
  Future<void> openStoragePermissionSettings() async {
    calls.add('permissionSettings');
  }

  @override
  Future<String?> pickDirectory() async {
    calls.add('pick');
    return 'content://tree/tree/primary%3ACustom';
  }

  @override
  Future<Map<String, dynamic>> listFiles(String treeUri) async => {
    'files': [
      {'name': 'default.custom.yaml', 'isDir': false},
    ],
  };
  @override
  Future<Map<String, dynamic>> readLocalSchemas(String treeUri) async => {
    'schemas': ['custom'],
    'installed': true,
    'deployed': false,
  };
  @override
  Future<Map<String, dynamic>> smartExtractZip(String zipPath, String treeUri) {
    calls.add('extract:$zipPath:$treeUri');
    return extraction?.future ?? Future.value(extracted);
  }

  @override
  Future<Map<String, dynamic>> copyAddonSchemaAssets(String id) async {
    calls.add('copy:$id');
    return {};
  }

  @override
  Future<Map<String, dynamic>> deployImeData() async {
    calls.add('deployImeData');
    if (failDeploy) {
      throw PlatformException(code: 'deploy', message: 'deploy failed');
    }
    return {'deployed': deployed, 'message': ''};
  }
}

Future<void> settle() => Future<void>.delayed(Duration.zero);

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  late FakeCore core;
  late FakeAndroid android;
  late AppController controller;
  AppController create({bool mobile = true}) => AppController(
    info: BridgeInfo(
      platform: mobile ? BridgePlatform.android : BridgePlatform.macOs,
      appVersion: 'test',
      userRoot: '/isolated/key tao',
    ),
    android: android,
  );

  setUp(() {
    core = FakeCore();
    android = FakeAndroid(core.calls);
    RustLib.initMock(api: core);
    controller = create();
  });
  tearDown(() async {
    controller.dispose();
    await core.events.close();
    await android.progress.close();
    RustLib.dispose();
  });

  test('startup checks once and stale source/scheme releases cannot enable install', () async {
    controller.dispose();
    controller = create(mobile: false);
    core.delayReleases = true;
    core.updateFails = true;
    expect(await controller.install(), isFalse);
    await controller.refreshState();
    await settle();
    expect(controller.appUpdate, isNull);
    expect(controller.error, isNull);
    expect(controller.releaseLoading, isTrue);
    controller.selectSource('github');
    controller.selectScheme('xmjd');
    core.latestRequests[0].complete(release);
    core.latestRequests[1].completeError(StateError('stale'));
    core.schemeRequests['xmjd']!.complete(
      const SchemeReleaseDto(
        scheme: 'xmjd',
        label: 'xmjd',
        version: '3',
        name: '',
        downloadUrl: 'offline-xmjd',
        assetName: 'xmjd6.zip',
      ),
    );
    await settle();
    expect(controller.downloadUrl, 'offline-xmjd');
    expect(controller.releaseError, isNull);
    expect(controller.canInstall, isTrue);
    await controller.refreshState();
    expect(core.calls.where((c) => c == 'update').length, 1);
    final request = controller.refreshRelease();
    core.schemeRequests['xmjd']!.completeError(StateError('offline'));
    expect(await request, isFalse);
    expect(controller.releaseError, '获取版本信息失败：offline');
    expect(controller.canInstall, isFalse);
  });

  test('SAF export is serialized, preserves live state, finishes and always cleans up', () async {
    controller.local = installedLocal;
    controller.latestRelease = release;
    await controller.connectEvents();
    expect(await controller.pickCustomDirectory(), isTrue);
    expect(controller.local, same(installedLocal));
    expect(controller.customSchemas!.schemas, ['custom']);
    android.extraction = Completer<Map<String, dynamic>>();
    final install = controller.installCustomDirectory();
    await settle();
    expect(await controller.installAddon(), isFalse);
    expect(await controller.install(), isFalse);
    android.progress.add(
      const InstallProgressDto(
        stage: 'extracting',
        percent: 80,
        message: 'file',
      ),
    );
    await settle();
    expect(controller.customProgress!.stage, 'extracting');
    expect(controller.installFraction, .8);
    android.extraction!.complete(extracted);
    expect(await install, isTrue);
    expect(controller.customVerifyFailureCount, 1);
    expect(controller.local, same(installedLocal));
    expect(
      core.calls,
      containsAllInOrder([
        'download:offline-ge',
        'extract:/cache/archive.zip:content://tree/tree/primary%3ACustom',
        'finish',
        'remove:/cache/archive.zip',
      ]),
    );
    expect(core.calls, isNot(contains('deployImeData')));
    final logCount = controller.operationLogCount;
    android.extraction = Completer<Map<String, dynamic>>();
    final failed = controller.installCustomDirectory();
    await settle();
    android.extraction!.completeError(StateError('extraction failed'));
    expect(await failed, isFalse);
    expect(core.calls.last, 'remove:/cache/archive.zip');
    expect(controller.customError, 'extraction failed');
    expect(controller.operationLogCount, greaterThan(logCount));
    expect(controller.operationLogs.first, matches(r'^\[\d\d:\d\d:\d\d\] '));
    controller.clearOperationLogs();
    expect(controller.operationLogCount, 0);
  });

  test('macOS export uses the selected release and does not deploy', () async {
    controller.dispose();
    controller = create(mobile: false)..latestRelease = release;
    const channel = MethodChannel('ink.rea.keytao/window');
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(channel, (call) async {
      expect(call.method, 'pickDirectory');
      return '/isolated/custom';
    });
    addTearDown(() => messenger.setMockMethodCallHandler(channel, null));
    await controller.pickCustomDirectory();
    expect(await controller.installCustomDirectory(), isTrue);
    expect(core.calls, contains('installTo:offline-ge:/isolated/custom'));
    expect(core.calls, isNot(contains('deployImeData')));
  });

  test(
    'permission redirect precedes any download and migration error logs once',
    () async {
      controller.latestRelease = release;
      android.permission = {'granted': false, 'writable': false, 'message': ''};
      expect(await controller.install(), isFalse);
      expect(core.calls, ['permission', 'permissionSettings']);
      expect(controller.installError, '需要文件访问权限');
      expect(
        controller.operationLogs.first,
        contains('[ANDROID PERMISSION] 需要文件访问权限'),
      );
      android.permission = {
        'granted': true,
        'writable': true,
        'migrationError': 'migration failed',
        'deployError': 'migration deploy failed',
      };
      await controller.install();
      await controller.install();
      expect(
        controller.deploySteps
            .where((step) => step.message == 'migration deploy failed')
            .length,
        lessThanOrEqualTo(1),
      );
      expect(
        controller.operationLogs
            .where(
              (line) => line.contains('[DEPLOY ERROR] migration deploy failed'),
            )
            .length,
        1,
      );
    },
  );

  test('add-ons gate main scheme, copy before bridge, and return callback failures as text', () async {
    expect(await controller.installAddon(), isFalse);
    expect(controller.addonError, '请先安装键道方案');
    expect(await controller.manageWanxiang(true), isFalse);
    expect(controller.wanxiangError, '请先安装主方案');
    controller.local = installedLocal;
    core.calls.clear();
    expect(await controller.installAddon(), isTrue);
    expect(
      core.calls,
      containsAllInOrder([
        'permission',
        'copy:easy_en',
        'addonInstall:easy_en',
        'deployImeData',
        'callback:',
      ]),
    );
    android.failDeploy = true;
    expect(await controller.manageWanxiang(true), isFalse);
    expect(core.calls, contains('callback:deploy failed'));
    expect(controller.wanxiangError, 'deploy failed');
    android.failDeploy = false;
    android.deployed = false;
    expect(await controller.manageWanxiang(true), isFalse);
    expect(core.calls, contains('callback:部署未完成'));
    controller.dispose();
    controller = create(mobile: false)
      ..desktopEnglishMode = EnglishModeDto.schema;
    expect(await controller.uninstallAddon(), isTrue);
    expect(controller.desktopEnglishMode, EnglishModeDto.ascii);
    expect(
      core.calls,
      containsAllInOrder(['addonUninstall:easy_en', 'englishMode']),
    );
  });

  test(
    'deploy progress retains ordered steps and persistent operation logs',
    () async {
      controller.dispose();
      controller = create(mobile: false)..local = installedLocal;
      await controller.connectEvents();
      await settle();
      core.deployGate = Completer<void>();
      controller.addOperationLogs(['earlier']);
      final deploy = controller.deploy();
      core.events.add(
        const BridgeEvent(
          kind: BridgeEventKind.deployProgress,
          deployProgress: 'compile',
        ),
      );
      core.events.add(
        const BridgeEvent(
          kind: BridgeEventKind.deployProgress,
          deployProgress: 'reload',
        ),
      );
      await settle();
      expect(controller.deploySteps.map((step) => step.message), [
        '正在部署 librime...',
        'compile',
        'reload',
      ]);
      expect(controller.deploySteps.last.state, DeployStepState.running);
      core.deployGate!.complete();
      expect(await deploy, isTrue);
      expect(
        controller.deploySteps.every(
          (step) => step.state == DeployStepState.ok,
        ),
        isTrue,
      );
      expect(controller.operationLogs.first, contains('earlier'));
    },
  );

  test(
    'each debug read survives failures in the other sources on every visit',
    () async {
      for (final failure in ['settings', 'runtime', 'system']) {
        core.logFailures
          ..clear()
          ..add(failure);
        core.calls.clear();
        controller.logSettings = null;
        controller.runtimeLog = null;
        controller.systemLogs = null;
        controller.selectPage(AppPage.input);
        controller.selectPage(AppPage.debug);
        await settle();
        expect(core.calls, containsAll(['settings', 'runtime', 'system']));
        expect(controller.logSettings != null, failure != 'settings');
        expect(controller.runtimeLog != null, failure != 'runtime');
        expect(controller.systemLogs != null, failure != 'system');
        expect(controller.debugError, contains(failure));
      }
      final gate = Completer<void>();
      controller.selectPage(AppPage.input);
      final running = controller.run(() => gate.future);
      controller.selectPage(AppPage.debug);
      core.logFailures.clear();
      core.calls.clear();
      gate.complete();
      await running;
      await settle();
      expect(core.calls, containsAll(['settings', 'runtime', 'system']));
      expect(controller.debugError, isNull);
    },
  );

  test('AndroidHost forwards native progress through a cancellable broadcast stream', () async {
    const host = AndroidHost();
    final messages = <InstallProgressDto>[];
    final subscription = host.installProgress.listen(messages.add);
    const codec = StandardMethodCodec();
    final complete = Completer<void>();
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .handlePlatformMessage(
          'keytao/android',
          codec.encodeMethodCall(
            const MethodCall('installProgress', {
              'stage': 'extracting',
              'percent': 99,
              'message': 'last file',
            }),
          ),
          (_) => complete.complete(),
        );
    await complete.future;
    await settle();
    expect(messages.single.percent, 99);
    expect(messages.single.message, 'last file');
    await subscription.cancel();
  });
}
