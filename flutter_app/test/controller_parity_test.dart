import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:keytao/src/app/controller.dart';
import 'package:keytao/src/app/options.dart';
import 'package:keytao/src/platform/android_host.dart';
import 'package:keytao/src/platform/ios_host.dart';
import 'package:keytao/src/rust/api/core.dart'
    show AddonStatusDto, WanxiangGrammarStatusDto;
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/rust/frb_generated.dart';

DeployProgressDto _legacyDeployProgress(String message) => DeployProgressDto(
  packageKey: 'keytao',
  packageName: '键道',
  index: 1,
  total: 1,
  step: DeployStepDto.compile,
  legacyMessage: message,
  queuedPackageKeys: const [],
);

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
const missingAddon = AddonStatusDto(
  englishAvailable: false,
  installed: false,
  deployed: false,
  version: '1',
);
const installedAddon = AddonStatusDto(
  englishAvailable: true,
  installed: true,
  deployed: true,
  version: '1',
);
const addonUpdate = AddonStatusDto(
  englishAvailable: true,
  installed: true,
  deployed: true,
  version: '1',
  latestVersion: '2',
);

// Only the external contract is mocked: no native libraries, HTTP or live roots.
class FakeCore implements RustLibApi {
  final calls = <String>[];
  final events = StreamController<BridgeEvent>.broadcast();
  LocalSchemaDto local = installedLocal;
  final latestRequests = <Completer<ReleaseInfoDto>>[];
  final schemeRequests = <String, Completer<SchemeReleaseDto>>{};
  Completer<String>? schemeSyncState;
  final schemeSettings = <String, dynamic>{};
  final addonUpdateRequests = <String, Completer<AddonStatusDto>>{};
  AddonStatusDto addonStatus = missingAddon;
  AddonStatusDto wanxiangStatus = missingAddon;
  WanxiangGrammarStatusDto wanxiangGrammarStatus = WanxiangGrammarStatusDto(
    available: true,
    installed: false,
    version: '',
    size: BigInt.zero,
  );
  AddonStatusDto rimeIceStatus = missingAddon;
  AddonStatusDto rimeMintStatus = missingAddon;
  AddonStatusDto rimeFrostStatus = missingAddon;
  AddonStatusDto rimeLsspStatus = missingAddon;
  bool delayAddonUpdates = false;
  bool addonUpdateFails = false;
  bool delayReleases = false;
  bool updateFails = false;
  bool addonDeployProgress = false;
  String? addonPublicWarning;
  String? installFailure;
  final logFailures = <String>{};
  Completer<void>? deployGate;
  final deployStarted = Completer<void>();
  String? deployFailure;
  String? deployWarning;
  EnglishModeDto english = EnglishModeDto.schema;
  bool inputEngineEnabled = true;
  int onboardingWrites = 0;

  Future<void> emitAddonDeployProgress() async {
    if (!addonDeployProgress) return;
    for (final message in ['正在部署 librime...', '部署完成', '已通知系统输入法重载']) {
      events.add(
        BridgeEvent(
          kind: BridgeEventKind.deployProgress,
          deployProgress: _legacyDeployProgress(message),
        ),
      );
    }
    if (addonPublicWarning case final warning?) {
      events.add(
        BridgeEvent(
          kind: BridgeEventKind.deployProgress,
          deployProgress: _legacyDeployProgress('系统搜索框方案未更新：$warning'),
        ),
      );
    }
    await settle();
  }

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
  Future<String> crateApiCoreInputEngine() async => 'rime';
  @override
  Future<bool> crateApiCoreInputEngineEnabled() async => inputEngineEnabled;
  @override
  Future<void> crateApiCoreSetInputEngineEnabled({required bool enabled}) async {
    inputEngineEnabled = enabled;
  }
  @override
  Future<LibjdStatusDto> crateApiCoreLibjdStatus() async =>
      const LibjdStatusDto(available: false);
  @override
  Future<String?> crateApiCoreActiveSchemaLogoPath() async => null;
  @override
  Future<List<SchemaSwitchDto>> crateApiCoreGetSchemaSwitches() async => [];
  @override
  Future<SchemaListDto> crateApiCoreGetSchemas() async =>
      const SchemaListDto(current: null, schemas: []);
  @override
  Future<int?> crateApiCoreGetCandidatePageSize() async => null;
  @override
  Future<int?> crateApiCoreSetCandidatePageSize({int? size}) async => size;
  @override
  Future<List<InstalledPackageDto>> crateApiCoreListInstalledPackages() async =>
      [];
  @override
  Future<List<IsolatedPackageDto>> crateApiCoreIsolatedPackages() async => [];
  @override
  Future<List<SchemeUpdateDto>> crateApiCoreCheckSchemeUpdates() async => [];
  @override
  Future<Map<String, String>> crateApiCoreGetDismissedSchemeUpdates() async =>
      {};
  @override
  Future<AddonStatusDto> crateApiCoreAddonSchemaStatus({
    required String id,
  }) async => addonStatus;
  @override
  Future<AddonStatusDto> crateApiCoreWanxiangStatus() async =>
      wanxiangStatus;
  @override
  Future<WanxiangGrammarStatusDto> crateApiCoreWanxiangGrammarStatus() async =>
      wanxiangGrammarStatus;
  @override
  Future<WanxiangGrammarStatusDto> crateApiCoreManageWanxiangGrammar({
    required bool installed,
    required FutureOr<String> Function() androidDeploy,
  }) async {
    calls.add('wanxiangGrammar:$installed');
    await emitAddonDeployProgress();
    final failure = await androidDeploy();
    calls.add('callback:$failure');
    if (failure.isNotEmpty) throw StateError(failure);
    return wanxiangGrammarStatus = WanxiangGrammarStatusDto(
      available: true,
      installed: installed,
      version: installed ? 'LTS' : '',
      size: BigInt.zero,
    );
  }
  @override
  Future<void> crateApiCoreCancelWanxiangGrammarDownload() async {
    calls.add('cancelWanxiangGrammar');
  }
  @override
  Future<AddonStatusDto> crateApiCoreRimeIceStatus() async => rimeIceStatus;
  @override
  Future<AddonStatusDto> crateApiCoreRimeMintStatus() async => rimeMintStatus;
  @override
  Future<AddonStatusDto> crateApiCoreRimeFrostStatus() async => rimeFrostStatus;
  @override
  Future<AddonStatusDto> crateApiCoreRimeLsspStatus() async => rimeLsspStatus;
  @override
  Future<AddonStatusDto> crateApiCoreCheckAddonUpdate({required String id}) async {
    calls.add('addonUpdate:$id');
    if (addonUpdateFails) throw StateError('offline add-on update');
    if (delayAddonUpdates) {
      final request = Completer<AddonStatusDto>();
      addonUpdateRequests[id] = request;
      return request.future;
    }
    return addonUpdate;
  }
  @override
  Future<MacosImeStatusDto> crateApiCoreMacosImeStatus() async =>
      const MacosImeStatusDto(
        installed: true,
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
        hideCandidatePanel: false,
        showTrayModeIndicator: true,
        effectiveEmbeddedComposition: false,
        accentColor: '#3B73D9',
        cornerRadius: null,
        backgroundColor: '',
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
  Future<String> crateApiCoreGetSchemeSyncState() =>
      schemeSyncState?.future ??
      Future.value(jsonEncode({'settings': schemeSettings}));

  @override
  Future<String> crateApiCoreApplySchemeProfile({
    required String profileJson,
  }) async => profileJson;

  @override
  Future<List<PackageSchemaDto>> crateApiCorePackageSchemas({
    required String schemeKey,
  }) async => const [
    PackageSchemaDto(id: 'default', name: 'Default', selected: true),
  ];

  @override
  Future<void> crateApiCoreRemoveDownloadedArchive({
    required String path,
  }) async {
    calls.add('remove:$path');
  }

  @override
  Future<InstallResultDto> crateApiCoreFinishAndroidInstall({
    required String resultJson,
    required String schemeKey,
    String? version,
  }) async {
    calls.add('finish');
    expect(jsonDecode(resultJson), extracted);
    return result;
  }

  @override
  Future<InstallResultDto> crateApiCoreInstallSchemeFromUrl({
    required String url,
    required String schemeKey,
    String? version,
  }) async {
    calls.add('install:$url');
    if (installFailure != null) throw StateError(installFailure!);
    return result;
  }

  @override
  Future<AddonStatusDto> crateApiCoreAddonSchemaInstall({
    required String id,
    required FutureOr<String> Function() androidDeploy,
  }) async {
    calls.add('addonInstall:$id');
    await emitAddonDeployProgress();
    final failure = await androidDeploy();
    calls.add('callback:$failure');
    if (failure.isNotEmpty) throw StateError(failure);
    return addonStatus = installedAddon;
  }

  @override
  Future<AddonStatusDto> crateApiCoreAddonSchemaUninstall({
    required String id,
    required FutureOr<String> Function() androidDeploy,
  }) async {
    calls.add('addonUninstall:$id');
    await emitAddonDeployProgress();
    english = EnglishModeDto.ascii;
    return addonStatus = missingAddon;
  }

  @override
  Future<AddonStatusDto> crateApiCoreManageWanxiang({
    required bool installed,
    required String variant,
    required FutureOr<String> Function() androidDeploy,
  }) async {
    calls.add('wanxiang:$installed');
    await emitAddonDeployProgress();
    final failure = await androidDeploy();
    calls.add('callback:$failure');
    if (failure.isNotEmpty) throw StateError(failure);
    return AddonStatusDto(
      installed: installed,
      deployed: installed,
      version: '1',
      englishAvailable: false,
      variant: installed ? variant : null,
    );
  }

  @override
  Future<AddonStatusDto> crateApiCoreManageRimeIce({
    required bool installed,
    required FutureOr<String> Function() androidDeploy,
  }) async {
    calls.add('rimeIce:$installed');
    await emitAddonDeployProgress();
    final failure = await androidDeploy();
    calls.add('callback:$failure');
    if (failure.isNotEmpty) throw StateError(failure);
    return rimeIceStatus = installed ? installedAddon : missingAddon;
  }

  @override
  Future<AddonStatusDto> crateApiCoreManageRimeMint({
    required bool installed,
    required FutureOr<String> Function() androidDeploy,
  }) async {
    calls.add('rimeMint:$installed');
    await emitAddonDeployProgress();
    final failure = await androidDeploy();
    calls.add('callback:$failure');
    if (failure.isNotEmpty) throw StateError(failure);
    return rimeMintStatus = installed ? installedAddon : missingAddon;
  }

  @override
  Future<AddonStatusDto> crateApiCoreManageRimeFrost({
    required bool installed,
    required FutureOr<String> Function() androidDeploy,
  }) async {
    calls.add('rimeFrost:$installed');
    await emitAddonDeployProgress();
    final failure = await androidDeploy();
    calls.add('callback:$failure');
    if (failure.isNotEmpty) throw StateError(failure);
    return rimeFrostStatus = installed ? installedAddon : missingAddon;
  }

  @override
  Future<AddonStatusDto> crateApiCoreManageRimeLssp({
    required bool installed,
    required FutureOr<String> Function() androidDeploy,
  }) async {
    calls.add('rimeLssp:$installed');
    await emitAddonDeployProgress();
    final failure = await androidDeploy();
    calls.add('callback:$failure');
    if (failure.isNotEmpty) throw StateError(failure);
    return rimeLsspStatus = installed ? installedAddon : missingAddon;
  }

  @override
  Future<void> crateApiCoreDeployAndroid({
    required FutureOr<String> Function() androidDeploy,
  }) async {
    final failure = await androidDeploy();
    if (failure.isNotEmpty) throw StateError(failure);
  }

  @override
  Future<DeployResultDto> crateApiCoreDeployDefault() async {
    calls.add('deployDefault');
    if (!deployStarted.isCompleted) deployStarted.complete();
    await deployGate?.future;
    if (deployFailure != null) throw StateError(deployFailure!);
    local = deployedLocal;
    return DeployResultDto(
      success: true,
      message: 'done',
      warning: deployWarning,
    );
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

  test('algorithm toggle preserves the preferred engine and clears preview', () async {
    controller.dispose();
    controller = create(mobile: false)
      ..inputEngine = 'libjd_engine'
      ..candidatePreviewCandidates = [(text: '候选', comment: '')];
    expect(await controller.setInputEngineEnabled(false), isTrue);
    expect(core.inputEngineEnabled, isFalse);
    expect(controller.candidatePreviewCandidates, isEmpty);
    expect(controller.prefersLibjd, isTrue);
    expect(await controller.setInputEngineEnabled(true), isTrue);
    expect(core.inputEngineEnabled, isTrue);
  });

  test('every managed scheme uses the download action label', () {
    controller.addon = controller.wanxiang = controller.rimeIce =
        controller.rimeMint = controller.rimeFrost = controller.rimeLssp =
            missingAddon;
    expect([
      controller.installButtonLabel,
      controller.addonButtonLabel,
      controller.wanxiangButtonLabel,
      controller.rimeIceButtonLabel,
      controller.rimeMintButtonLabel,
      controller.rimeFrostButtonLabel,
      controller.rimeLsspButtonLabel,
    ], everyElement(AppStrings.download));
    controller.wanxiang = installedAddon;
    controller.selectWanxiangVariant('pure');
    expect(controller.wanxiangButtonLabel, AppStrings.download);
  });

  test('add-on status uses the same installed and active vocabulary', () {
    expect(controller.rimeIceStatusText, AppStrings.checking);
    controller.rimeIce = missingAddon;
    expect(controller.rimeIceStatusText, AppStrings.notInstalled);
    controller.rimeIce = installedAddon;
    controller.local = deployedLocal;
    expect(controller.rimeIceStatusText, '已安装 · 1');
    controller.rimeIceLatestVersion = '2';
    expect(controller.rimeIceStatusText, '可更新 · 1');
    controller.local = const LocalSchemaDto(
      installed: true,
      deployed: true,
      schemas: ['rime_ice'],
    );
    expect(controller.rimeIceStatusText, '使用中 · 1');
    controller.rimeIce = const AddonStatusDto(
      installed: true,
      deployed: false,
      version: '1',
      englishAvailable: false,
    );
    expect(controller.rimeIceStatusText, '待部署 · 1');
    controller.local = deployedLocal;
    controller.rimeIceLatestVersion = null;
    expect(controller.rimeIceStatusText, '待部署 · 1');
  });

  Future<File> prepareIosUpgrade() async {
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(IosHost.channel, (call) async {
      expect(call.method, 'keyboardStatus');
      return {'enabled': true, 'fullAccess': true};
    });
    addTearDown(
      () => messenger.setMockMethodCallHandler(IosHost.channel, null),
    );
    final root = await Directory.systemTemp.createTemp('keytao-ios-upgrade-');
    addTearDown(() => root.delete(recursive: true));
    final shared = await Directory('${root.path}/rime-data').create();
    final stamp = File('${shared.path}/.bundled-version');
    await stamp.writeAsString('2/20');
    controller.dispose();
    controller = AppController(
      info: BridgeInfo(
        platform: BridgePlatform.ios,
        appVersion: '2',
        userRoot: root.path,
      ),
    );
    return stamp;
  }

  test(
    'iOS startup deploys stale data once without concurrent user deploy',
    () async {
      await prepareIosUpgrade();
      core.deployGate = Completer<void>();
      final startup = controller.refreshState();
      await core.deployStarted.future.timeout(const Duration(seconds: 2));
      expect(controller.busy, isTrue);
      expect(await controller.deploy(), isFalse);
      expect(await controller.refreshState(), isFalse);
      controller.onResume();
      core.deployGate!.complete();
      expect(await startup, isTrue);
      await settle();
      expect(controller.local?.deployed, isTrue);
      expect(await controller.refreshState(), isTrue);
      expect(core.calls.where((call) => call == 'deployDefault'), hasLength(1));
    },
  );

  test('iOS resume waits for a user deploy and does not repeat it', () async {
    await prepareIosUpgrade();
    core.deployGate = Completer<void>();
    controller.local = installedLocal;
    final deploying = controller.deploy();
    await core.deployStarted.future;
    controller.onResume();
    expect(await controller.refreshState(), isFalse);
    core.deployGate!.complete();
    expect(await deploying, isTrue);
    await settle();
    expect(core.calls.where((call) => call == 'deployDefault'), hasLength(1));
  });

  test('iOS failed upgrade retries only for a new bundle stamp', () async {
    final stamp = await prepareIosUpgrade();
    core.deployFailure = 'deploy failed';
    expect(await controller.refreshState(), isFalse);
    expect(core.calls.where((call) => call == 'deployDefault'), hasLength(1));
    expect(await controller.refreshState(), isTrue);
    expect(await controller.refreshState(), isTrue);
    expect(core.calls.where((call) => call == 'deployDefault'), hasLength(1));
    await stamp.writeAsString('3/30');
    core.deployFailure = null;
    expect(await controller.refreshState(), isTrue);
    expect(core.calls.where((call) => call == 'deployDefault'), hasLength(2));
    expect(controller.local?.deployed, isTrue);
  });

  test('source switching keeps fetched releases and never refetches', () async {
    for (final mobile in [true, false]) {
      controller.dispose();
      controller = create(mobile: mobile)..latestRelease = release;
      core.calls.clear();
      core.delayReleases = true;
      var notifiedSource = controller.source;
      controller.addListener(() => notifiedSource = controller.source);
      for (final source in ['github', 'gitee']) {
        controller.selectSource(source);
        await settle();
        expect(notifiedSource, source);
        expect(core.calls, isEmpty);
        expect(controller.latestRelease, same(release));
        expect(controller.releaseLoading, isFalse);
        expect(
          controller.downloadUrl,
          source == 'github' ? 'offline-gh' : 'offline-ge',
        );
        expect(controller.canInstall, isTrue);
        expect(controller.changelogBody, 'Release notes');
      }
      controller.busy = true;
      controller.selectSource('github');
      expect(controller.downloadUrl, 'offline-gh');
      expect(controller.latestRelease, same(release));
      expect(core.calls, isEmpty);
    }
  });

  test('a failed automatic deployment keeps the build and stops retrying', () async {
    controller.dispose();
    controller = create(mobile: false);
    core.local = const LocalSchemaDto(
      installed: true,
      deployed: false,
      buildAvailable: true,
      installedScheme: 'keytao',
      schemas: ['keytao'],
    );
    core.deployFailure = 'invalid YAML';
    await controller.refreshSchemas();
    await core.deployStarted.future;
    await settle();
    expect(controller.hasUsableBuild, isTrue);
    expect(controller.isDeploying, isFalse);
    expect(controller.deploySteps.last.message, contains('invalid YAML'));
    await controller.refreshSchemas();
    expect(core.calls.where((call) => call == 'deployDefault'), hasLength(1));
    core.deployFailure = null;
    expect(await controller.deploy(), isTrue);
    expect(controller.local?.deployed, isTrue);
  });

  test('desktop theme saves share one slot during deployment', () async {
    controller.dispose();
    controller = create(mobile: false);
    final deployGate = Completer<void>();
    final deployment = controller.run(() => deployGate.future);
    final themeGate = Completer<void>();
    final themeWrite = controller.themeLibraryAction(() => themeGate.future);
    try {
      expect(controller.busy, isTrue);
      expect(controller.savingTheme, isTrue);
      expect(controller.canSaveTheme, isFalse);
      await expectLater(
        controller.themeLibraryAction(() async => fail('overlapping write')),
        throwsStateError,
      );
    } finally {
      themeGate.complete();
      deployGate.complete();
      await themeWrite;
      await deployment;
    }
    expect(controller.canSaveTheme, isTrue);
  });

  test(
    'only explicit deploy operations receive deploy progress steps',
    () async {
      controller.dispose();
      controller = create(mobile: false)..local = installedLocal;
      await controller.connectEvents();
      await settle();
      core.addonDeployProgress = true;
      for (final previousDeploy in [false, true]) {
        controller.deploySteps.clear();
        if (previousDeploy) expect(await controller.deploy(), isTrue);
        final previousSteps = List<DeployStep>.of(controller.deploySteps);
        for (final action in [
          controller.installAddon,
          controller.uninstallAddon,
          () => controller.manageWanxiang(true),
          () => controller.manageWanxiang(false),
          () => controller.manageRimeIce(true),
          () => controller.manageRimeIce(false),
          () => controller.manageRimeMint(true),
          () => controller.manageRimeMint(false),
          () => controller.manageRimeFrost(true),
          () => controller.manageRimeFrost(false),
          () => controller.manageRimeLssp(true),
          () => controller.manageRimeLssp(false),
        ]) {
          controller.clearOperationLogs();
          expect(await action(), isTrue);
          expect(controller.deploySteps, previousSteps);
          expect(controller.operationLogs, everyElement(contains('[ADDON]')));
        }
        core.events.add(
          BridgeEvent(
            kind: BridgeEventKind.deployProgress,
            deployProgress: _legacyDeployProgress('late event'),
          ),
        );
        await settle();
        expect(controller.deploySteps, previousSteps);
      }
    },
  );

  test('implicit deployments retain nonblocking public schema warnings', () async {
    controller.dispose();
    controller = create(mobile: false)..local = installedLocal;
    await controller.connectEvents();
    await settle();
    core.addonDeployProgress = true;
    core.addonPublicWarning = 'publish public schemas: access denied';
    const warningMessage =
        '系统搜索框方案未更新：publish public schemas: access denied';
    for (final action in [
      controller.installAddon,
      controller.uninstallAddon,
      () => controller.manageWanxiang(true),
      () => controller.manageWanxiangGrammar(true),
      () => controller.manageWanxiangGrammar(false),
      () => controller.manageWanxiang(false),
      () => controller.manageRimeIce(true),
      () => controller.manageRimeIce(false),
      () => controller.manageRimeMint(true),
      () => controller.manageRimeMint(false),
      () => controller.manageRimeFrost(true),
      () => controller.manageRimeFrost(false),
      () => controller.manageRimeLssp(true),
      () => controller.manageRimeLssp(false),
    ]) {
      controller.deploySteps.clear();
      controller.clearOperationLogs();
      expect(await action(), isTrue);
      expect(controller.error, isNull);
      expect(controller.deploySteps, hasLength(1));
      expect(controller.deploySteps.single.message, warningMessage);
      expect(controller.deploySteps.single.state, DeployStepState.neutral);
      expect(
        controller.operationLogs.where((line) => line.contains('[WARN]')),
        [contains(warningMessage)],
      );
    }
  });

  test(
    'install progress updates state without appending operation logs',
    () async {
      await controller.connectEvents();
      await settle();
      controller.addOperationLogs(['[MERGED] existing result']);
      final logs = List<String>.of(controller.operationLogs);
      for (final operation in [
        OperationKind.install,
        OperationKind.addon,
        OperationKind.wanxiang,
        OperationKind.wanxiangGrammar,
        OperationKind.rimeIce,
        OperationKind.rimeMint,
        OperationKind.rimeFrost,
        OperationKind.rimeLssp,
      ]) {
        controller.operation = operation;
        const progress = InstallProgressDto(
          stage: 'downloading',
          percent: 42,
          message: 'download tick',
        );
        core.events.add(
          const BridgeEvent(
            kind: BridgeEventKind.installProgress,
            installProgress: progress,
          ),
        );
        await settle();
        expect(controller.installProgress, same(progress));
        expect(controller.installFraction, .42);
        expect(controller.operationLogs, logs);
        if (operation == OperationKind.addon) {
          expect(controller.addonProgress, same(progress));
        }
        if (operation == OperationKind.wanxiang) {
          expect(controller.wanxiangProgress, same(progress));
        }
        if (operation == OperationKind.wanxiangGrammar) {
          expect(controller.wanxiangGrammarProgress, same(progress));
        }
        if (operation == OperationKind.rimeIce) {
          expect(controller.rimeIceProgress, same(progress));
        }
        if (operation == OperationKind.rimeMint) {
          expect(controller.rimeMintProgress, same(progress));
        }
        if (operation == OperationKind.rimeFrost) {
          expect(controller.rimeFrostProgress, same(progress));
        }
        if (operation == OperationKind.rimeLssp) {
          expect(controller.rimeLsspProgress, same(progress));
        }
      }
      var changes = 0;
      controller.addListener(() => changes++);
      android.progress.add(
        const InstallProgressDto(
          stage: 'extracting',
          percent: 80,
          message: 'extract tick',
        ),
      );
      await settle();
      expect(controller.installFraction, .8);
      expect(changes, 1);
      expect(controller.operationLogs, logs);
    },
  );

  test('grammar download cancels only during its download stage', () async {
    await controller.connectEvents();
    await settle();
    controller.operation = OperationKind.wanxiangGrammar;
    controller.busy = true;
    for (final stage in [
      'wanxiang-grammar-download',
      'wanxiang-grammar-deploy',
    ]) {
      core.events.add(
        BridgeEvent(
          kind: BridgeEventKind.installProgress,
          installProgress: InstallProgressDto(
            stage: stage,
            percent: 40,
            message: stage,
          ),
        ),
      );
      await settle();
      expect(
        controller.canCancelWanxiangGrammarDownload,
        stage == 'wanxiang-grammar-download',
      );
      await controller.cancelWanxiangGrammarDownload();
    }
    expect(core.calls.where((call) => call == 'cancelWanxiangGrammar'), [
      'cancelWanxiangGrammar',
    ]);
    expect(await controller.manageWanxiangGrammar(true), isFalse);
  });

  test('obsolete last-page settings leave startup on the input page', () async {
    controller.dispose();
    controller = create(mobile: false);
    core.schemeSettings['last_page'] = 'extension';
    expect(await controller.refreshState(), isTrue);
    await settle();
    expect(controller.activePage, AppPage.input);
    expect(controller.error, isNull);
    expect(controller.schemeSyncError, isNull);
  });

  test(
    'startup checks once and stale scheme releases cannot enable install',
    () async {
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
    },
  );

  test(
    'startup restores the selected scheme release and rejects the old request',
    () async {
      controller.dispose();
      controller = create(mobile: false);
      core.delayReleases = true;
      core.schemeSyncState = Completer<String>();
      expect(await controller.refreshState(), isTrue);
      await settle();
      expect(core.latestRequests, hasLength(1));

      core.schemeSyncState!.complete(
        jsonEncode({
          'settings': {'selected_scheme': 'txjx'},
        }),
      );
      await settle();
      expect(controller.scheme, 'txjx');
      expect(core.schemeRequests, contains('txjx'));
      expect(controller.latestRelease, isNull);

      core.schemeRequests['txjx']!.complete(
        const SchemeReleaseDto(
          scheme: 'txjx',
          label: 'txjx',
          version: '3',
          name: '',
          downloadUrl: 'offline-txjx',
          assetName: 'txjx.zip',
        ),
      );
      core.latestRequests.single.complete(release);
      await settle();
      expect(controller.latestRelease, isNull);
      expect(controller.releaseVersion, '3');
      expect(controller.downloadUrl, 'offline-txjx');
      expect(controller.canInstall, isTrue);
    },
  );

  test('cloud settings refresh a changed scheme release only once', () async {
    controller.dispose();
    controller = create(mobile: false);
    expect(await controller.refreshState(), isTrue);
    await settle();
    expect(controller.latestRelease, same(release));
    controller.pendingCloudProfile = {
      'settings': {'selected_scheme': 'txjx'},
    };
    expect(await controller.applyCloudProfile(), isTrue);
    expect(controller.scheme, 'txjx');
    expect(controller.latestRelease, isNull);
    expect(core.schemeRequests, contains('txjx'));

    core.schemeRequests['txjx']!.complete(
      const SchemeReleaseDto(
        scheme: 'txjx',
        label: 'txjx',
        version: '3',
        name: '',
        downloadUrl: 'offline-txjx',
        assetName: 'txjx.zip',
      ),
    );
    await settle();
    final selectedRelease = controller.schemeRelease;
    controller.pendingCloudProfile = {
      'settings': {'selected_scheme': 'txjx'},
    };
    expect(await controller.applyCloudProfile(), isTrue);
    core.schemeSettings['selected_scheme'] = 'txjx';
    await controller.loadSchemeSyncState();
    expect(controller.schemeRelease, same(selectedRelease));
    expect(core.calls.where((call) => call == 'release:txjx'), hasLength(1));
    expect(controller.canInstall, isTrue);
  });

  test('add-on update checks skip uninstalled schemes', () async {
    await controller.refreshState();
    await controller.refreshState();
    await settle();
    expect(core.calls.where((call) => call.startsWith('addonUpdate:')), isEmpty);
  });

  test('add-on update checks run once per installed scheme per session', () async {
    core.addonStatus = installedAddon;
    core.delayAddonUpdates = true;
    await controller.refreshState();
    final request = core.addonUpdateRequests['easy_en']!;
    await controller.refreshState();
    expect(
      core.calls.where((call) => call.startsWith('addonUpdate:')),
      ['addonUpdate:easy_en'],
    );
    request.complete(addonUpdate);
    await settle();
    expect(controller.addonLatestVersion, '2');
    core.wanxiangStatus = installedAddon;
    await controller.refreshState();
    core.addonUpdateRequests['wanxiang']!.complete(addonUpdate);
    await settle();
    await controller.refreshState();
    expect(
      core.calls.where((call) => call.startsWith('addonUpdate:')),
      ['addonUpdate:easy_en', 'addonUpdate:wanxiang'],
    );
    expect(controller.wanxiangLatestVersion, '2');
    core.rimeIceStatus = installedAddon;
    await controller.refreshState();
    core.addonUpdateRequests['rime_ice']!.complete(addonUpdate);
    await settle();
    await controller.refreshState();
    expect(
      core.calls.where((call) => call == 'addonUpdate:rime_ice'),
      hasLength(1),
    );
    expect(controller.rimeIceLatestVersion, '2');
    expect(controller.rimeIceHasUpdate, isTrue);
    core.rimeMintStatus = installedAddon;
    core.rimeFrostStatus = installedAddon;
    core.rimeLsspStatus = installedAddon;
    await controller.refreshState();
    core.addonUpdateRequests['rime_mint']!.complete(addonUpdate);
    core.addonUpdateRequests['rime_frost']!.complete(addonUpdate);
    core.addonUpdateRequests['triple_pinyin_lssp']!.complete(addonUpdate);
    await settle();
    await controller.refreshState();
    for (final id in [
      'rime_mint',
      'rime_frost',
      'triple_pinyin_lssp',
    ]) {
      expect(
        core.calls.where((call) => call == 'addonUpdate:$id'),
        hasLength(1),
      );
    }
    expect(controller.rimeMintLatestVersion, '2');
    expect(controller.rimeMintHasUpdate, isTrue);
    expect(controller.rimeFrostLatestVersion, '2');
    expect(controller.rimeFrostHasUpdate, isTrue);
    expect(controller.rimeLsspLatestVersion, '2');
    expect(controller.rimeLsspHasUpdate, isTrue);
  });

  test('add-on update check failures remain nonblocking for the session', () async {
    controller.dispose();
    controller = create(mobile: false);
    core.addonStatus = installedAddon;
    core.wanxiangStatus = installedAddon;
    core.rimeIceStatus = installedAddon;
    core.rimeMintStatus = installedAddon;
    core.rimeFrostStatus = installedAddon;
    core.addonUpdateFails = true;
    expect(await controller.refreshState(), isTrue);
    expect(await controller.refreshState(), isTrue);
    await settle();
    expect(
      core.calls.where((call) => call.startsWith('addonUpdate:')),
      [
        'addonUpdate:easy_en',
        'addonUpdate:wanxiang',
        'addonUpdate:rime_ice',
        'addonUpdate:rime_mint',
        'addonUpdate:rime_frost',
      ],
    );
    expect(controller.error, isNull);
    expect(controller.addonError, isNull);
    expect(controller.wanxiangError, isNull);
    expect(controller.rimeIceError, isNull);
    expect(controller.rimeMintError, isNull);
    expect(controller.rimeFrostError, isNull);
    expect(controller.addonLatestVersion, isNull);
    expect(controller.wanxiangLatestVersion, isNull);
    expect(controller.rimeIceLatestVersion, isNull);
    expect(controller.rimeMintLatestVersion, isNull);
    expect(controller.rimeFrostLatestVersion, isNull);
  });

  test('add-on update checks discard results across install and uninstall', () async {
    for (final installed in [true, false]) {
      controller.dispose();
      controller = create(mobile: false);
      core.addonStatus = installedAddon;
      core.wanxiangStatus = installedAddon;
      core.rimeIceStatus = installedAddon;
      core.rimeMintStatus = installedAddon;
      core.rimeFrostStatus = installedAddon;
      core.rimeLsspStatus = installedAddon;
      core.delayAddonUpdates = true;
      await controller.refreshState();
      final easyEnRequest = core.addonUpdateRequests['easy_en']!;
      final wanxiangRequest = core.addonUpdateRequests['wanxiang']!;
      final rimeIceRequest = core.addonUpdateRequests['rime_ice']!;
      final rimeMintRequest = core.addonUpdateRequests['rime_mint']!;
      final rimeFrostRequest = core.addonUpdateRequests['rime_frost']!;
      final rimeLsspRequest = core.addonUpdateRequests['triple_pinyin_lssp']!;
      expect(
        await (installed
            ? controller.installAddon()
            : controller.uninstallAddon()),
        isTrue,
      );
      expect(await controller.manageRimeIce(installed), isTrue);
      expect(await controller.manageRimeMint(installed), isTrue);
      expect(await controller.manageRimeFrost(installed), isTrue);
      expect(await controller.manageRimeLssp(installed), isTrue);
      easyEnRequest.complete(addonUpdate);
      wanxiangRequest.complete(addonUpdate);
      rimeIceRequest.complete(addonUpdate);
      rimeMintRequest.complete(addonUpdate);
      rimeFrostRequest.complete(addonUpdate);
      rimeLsspRequest.complete(addonUpdate);
      await settle();
      expect(controller.addonLatestVersion, installed ? '1' : null);
      expect(controller.wanxiangLatestVersion, '2');
      expect(controller.rimeIceLatestVersion, installed ? '1' : null);
      expect(controller.rimeMintLatestVersion, installed ? '1' : null);
      expect(controller.rimeFrostLatestVersion, installed ? '1' : null);
      expect(controller.rimeLsspLatestVersion, installed ? '1' : null);
    }
  });

  test('cancelled installs stay inline and never deploy, allowing retry', () async {
    controller.dispose();
    controller = create(mobile: false)
      ..latestRelease = release
      ..local = installedLocal;
    core.installFailure = AppStrings.downloadCancelled;
    for (final install in [controller.install, controller.installAndDeploy]) {
      expect(await install(), isFalse);
      expect(controller.installError, isNull);
      expect(controller.error, isNull);
      expect(controller.progress, AppStrings.downloadCancelled);
      expect(controller.canInstall, isTrue);
      expect(controller.busy, isFalse);
      expect(controller.operation, isNull);
      expect(
        controller.operationLogs.any((line) => line.contains('[ERROR]')),
        isFalse,
      );
    }
    expect(core.calls.where((call) => call.startsWith('install:')), hasLength(2));
    expect(core.calls, isNot(contains('deployDefault')));
  });

  test(
    'storage gates downloads and permits deployment once granted and writable',
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
      android.permission = {'granted': true, 'writable': false};
      core.calls.clear();
      expect(await controller.install(), isFalse);
      expect(core.calls, ['permission', 'permissionSettings']);
      expect(SetupStep.storage.isComplete(controller), isFalse);

      android.permission = {'granted': true, 'writable': true};
      controller.local = installedLocal;
      core.local = deployedLocal;
      core.calls.clear();
      expect(await controller.deploy(), isTrue);
      expect(
        core.calls,
        containsAllInOrder(['permission', 'deployImeData']),
      );
      expect(SetupStep.storage.isComplete(controller), isTrue);
      expect(controller.setupReady, isTrue);
      expect(
        controller.deploySteps.any((step) => step.state == DeployStepState.failed),
        isFalse,
      );
    },
  );

  test('Android completes setup without a scheme after a queued resume check', () async {
    core.local = const LocalSchemaDto(
      installed: false,
      deployed: false,
      schemas: [],
    );
    final gate = Completer<void>();
    final action = controller.run(() => gate.future);
    controller.onResume();
    expect(controller.setupReady, isFalse);
    gate.complete();
    expect(await action, isTrue);
    await settle();

    expect(controller.setupReady, isTrue);
    expect(controller.local!.installed, isFalse);
    expect(core.onboardingWrites, 0);
    expect(await controller.finishOnboarding(), isTrue);
    expect(core.onboardingWrites, 1);
    expect(core.calls, isNot(contains('deployImeData')));
  });

  test('add-ons install without a main scheme and report callback failures', () async {
    core.local = const LocalSchemaDto(
      installed: false,
      deployed: false,
      schemas: [],
    );
    expect(controller.local, isNull);
    expect(controller.canInstallAddon, isTrue);
    expect(controller.canInstallWanxiang, isTrue);
    expect(controller.canInstallRimeIce, isTrue);
    expect(controller.canInstallRimeMint, isTrue);
    expect(controller.canInstallRimeFrost, isTrue);
    expect(controller.canInstallRimeLssp, isTrue);
    expect(await controller.manageRimeIce(true), isTrue);
    expect(await controller.manageRimeMint(true), isTrue);
    expect(await controller.manageRimeFrost(true), isTrue);
    expect(await controller.manageRimeLssp(true), isTrue);
    expect(await controller.manageWanxiang(true), isTrue);
    core.calls.clear();
    expect(await controller.installAddon(), isTrue);
    expect(
      core.calls,
      containsAllInOrder([
        'permission',
        'addonInstall:easy_en',
        'deployImeData',
        'callback:',
      ]),
    );
    android.failDeploy = true;
    expect(await controller.manageWanxiang(true), isFalse);
    expect(core.calls, contains('callback:deploy failed'));
    expect(controller.wanxiangError, 'deploy failed');
    expect(await controller.manageRimeIce(true), isFalse);
    expect(controller.rimeIceError, 'deploy failed');
    expect(await controller.manageRimeMint(true), isFalse);
    expect(controller.rimeMintError, 'deploy failed');
    expect(await controller.manageRimeFrost(true), isFalse);
    expect(controller.rimeFrostError, 'deploy failed');
    expect(await controller.manageRimeLssp(true), isFalse);
    expect(controller.rimeLsspError, 'deploy failed');
    android.failDeploy = false;
    android.deployed = false;
    expect(await controller.manageWanxiang(true), isFalse);
    expect(core.calls, contains('callback:部署未完成'));
    expect(await controller.manageRimeIce(true), isFalse);
    expect(controller.rimeIceError, '部署未完成');
    expect(await controller.manageRimeMint(true), isFalse);
    expect(controller.rimeMintError, '部署未完成');
    expect(await controller.manageRimeFrost(true), isFalse);
    expect(controller.rimeFrostError, '部署未完成');
    expect(await controller.manageRimeLssp(true), isFalse);
    expect(controller.rimeLsspError, '部署未完成');
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
        BridgeEvent(
          kind: BridgeEventKind.deployProgress,
          deployProgress: _legacyDeployProgress('compile'),
        ),
      );
      core.events.add(
        BridgeEvent(
          kind: BridgeEventKind.deployProgress,
          deployProgress: _legacyDeployProgress('reload'),
        ),
      );
      await settle();
      expect(controller.deploySteps.map((step) => step.message), [
        '正在部署 librime…',
        'compile',
        'reload',
      ]);
      expect(controller.deploySteps.last.state, DeployStepState.neutral);
      core.deployGate!.complete();
      expect(await deploy, isTrue);
      expect(
        controller.deploySteps
            .take(controller.deploySteps.length - 1)
            .every((step) => step.state == DeployStepState.neutral),
        isTrue,
      );
      expect(controller.deploySteps.last.state, DeployStepState.ok);
      expect(controller.operationLogs.first, contains('earlier'));
    },
  );

  test('public schema warning preserves private deployment success', () async {
    const warningMessage =
        '系统搜索框方案未更新：compile public schema: invalid dictionary';
    for (final withProgress in [false, true]) {
      controller.dispose();
      controller = create(mobile: false)..local = installedLocal;
      core.deployWarning = warningMessage;
      core.deployGate = Completer<void>();
      await controller.connectEvents();
      await settle();
      final deploying = controller.deploy();
      if (withProgress) {
        core.events.add(
          BridgeEvent(
            kind: BridgeEventKind.deployProgress,
            deployProgress: _legacyDeployProgress(warningMessage),
          ),
        );
        await settle();
      }
      core.deployGate!.complete();

      expect(await deploying, isTrue);
      expect(controller.error, isNull);
      expect(controller.local?.deployed, isTrue);
      expect(
        controller.deploySteps.where((step) => step.state == DeployStepState.ok),
        hasLength(1),
      );
      final warnings = controller.deploySteps.where(
        (step) => step.message == warningMessage,
      );
      expect(warnings, hasLength(1));
      expect(warnings.single.state, DeployStepState.neutral);
      expect(controller.progress, 'done');
      expect(controller.operationLogs, contains(contains('[WARN] $warningMessage')));
    }
  });

  test(
    'package progress tracks the ordered queue and scoped failure',
    () async {
      controller.dispose();
      controller = create(mobile: false)..local = installedLocal;
      await controller.connectEvents();
      await settle();
      core.deployGate = Completer<void>();
      final deployment = controller.deploy();

      Future<void> step(
        String key,
        String name,
        int index,
        DeployStepDto step, {
        String? error,
        List<String> queued = const [],
      }) async {
        core.events.add(
          BridgeEvent(
            kind: BridgeEventKind.deployProgress,
            deployProgress: DeployProgressDto(
              packageKey: key,
              packageName: name,
              index: index,
              total: 2,
              step: step,
              error: error,
              queuedPackageKeys: queued,
            ),
          ),
        );
        await settle();
      }

      await step(
        'keytao',
        '键道',
        1,
        DeployStepDto.materialize,
        queued: ['rime_ice'],
      );
      expect(controller.packageDeployLabel('keytao'), '部署中');
      expect(controller.packageDeployLabel('rime_ice'), '等待部署');
      expect(controller.progress, '正在部署 键道（1/2）');

      await step('keytao', '键道', 1, DeployStepDto.done);
      await step('rime_ice', '雾凇', 2, DeployStepDto.compile);
      expect(controller.packageDeployLabel('keytao'), isNull);
      expect(controller.packageDeployLabel('rime_ice'), '部署中');
      expect(controller.progress, '正在部署 雾凇（2/2）');

      await step(
        'rime_ice',
        '雾凇',
        2,
        DeployStepDto.failed,
        error: 'invalid yaml',
      );
      expect(controller.packageDeployLabel('rime_ice'), isNull);
      expect(controller.packageDeployError('keytao'), isNull);
      expect(controller.packageDeployError('rime_ice'), 'invalid yaml');
      expect(controller.deploySteps.map((step) => step.packageKey), [
        'keytao',
        'rime_ice',
      ]);
      core.deployFailure = '雾凇：invalid yaml';
      core.deployGate!.complete();
      expect(await deployment, isFalse);
      expect(controller.packageDeployError('rime_ice'), 'invalid yaml');
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
      core.logFailures.clear();
      core.calls.clear();
      controller.selectPage(AppPage.debug);
      await settle();
      expect(controller.busy, isTrue);
      expect(core.calls, containsAll(['settings', 'runtime', 'system']));
      expect(controller.debugError, isNull);
      gate.complete();
      await running;
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
