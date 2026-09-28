import 'dart:async';
import 'dart:io';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:keytao/src/app/bootstrap.dart';
import 'package:keytao/src/app/controller.dart';
import 'package:keytao/src/app/options.dart';
import 'package:keytao/src/platform/ios_host.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/rust/frb_generated.dart';

import 'controller_parity_test.dart'
    show FakeCore, installedLocal, deployedLocal, result, settle;
import 'view_fixture.dart';

class PlatformCore extends FakeCore {
  PlatformCore(this.mobileSettings);
  AndroidImeInputSettingsDto mobileSettings;
  final args = <List<String>>[];
  int? pending;
  bool failDeploy = false;
  bool failSearch = false;
  bool failLocal = false;
  bool failStart = false;
  WindowsImeStatusDto windows = windowsStatus();
  Completer<void>? registrationGate;

  @override
  Future<LocalSchemaDto> crateApiCoreCheckLocalSchema() async {
    if (failLocal) throw StateError('offline local check');
    return local;
  }

  @override
  Future<void> crateApiCoreHandleAppArgs({required List<String> args}) async {
    this.args.add(args);
    if (args.contains('keytao://ime/redeploy')) pending ??= 7;
    if (args.any(
      (arg) => arg == 'keytao://ime/redeploy' || arg == 'keytao://ime/open',
    )) {
      events.add(const BridgeEvent(kind: BridgeEventKind.windowsImeAction));
    }
  }

  @override
  Future<WindowsImeStatusDto> crateApiCoreWindowsImeEnsureRegistered() async {
    calls.add('register');
    await registrationGate?.future;
    return windows;
  }

  @override
  Future<void> crateApiCoreWindowsPrepareSearchSchemas() async {
    calls.add('search');
    if (failSearch) throw StateError('offline search');
  }

  @override
  Future<WindowsImeStatusDto> crateApiCoreWindowsImeStatus() async {
    calls.add('windowsStatus');
    return windows;
  }

  @override
  Future<int?> crateApiCoreWindowsPendingImeAction() async {
    calls.add('pending');
    return pending;
  }

  @override
  Future<void> crateApiCoreWindowsDismissImeAction({required int id}) async {
    calls.add('dismiss:$id');
    if (pending == id) pending = null;
  }

  @override
  Future<DeployResultDto> crateApiCoreWindowsRedeployImeAction({
    required int id,
  }) async {
    calls.add('redeploy:$id');
    pending = null;
    if (failDeploy) throw StateError('offline redeploy');
    return crateApiCoreDeployDefault();
  }

  @override
  Future<LinuxImeStatusDto> crateApiCoreLinuxStartIme({
    required bool restart,
  }) async {
    calls.add('linuxStart:$restart');
    if (failStart) throw StateError('offline Linux start');
    return linuxStatus;
  }

  @override
  Future<LinuxImeStatusDto> crateApiCoreLinuxImeStatus() async {
    calls.add('linuxStatus');
    return linuxStatus;
  }

  @override
  Future<AndroidImeInputSettingsDto>
  crateApiCoreGetAndroidImeInputSettings() async {
    calls.add('mobileSettings');
    return mobileSettings;
  }

  @override
  Future<AndroidImeInputSettingsDto> crateApiCoreSetAndroidImeInputSettings({
    required AndroidImeInputSettingsDto settings,
  }) async {
    calls.add('saveMobile');
    return mobileSettings = settings;
  }

  @override
  Future<InstallResultDto> crateApiCoreInstallSchemeFromUrl({
    required String url,
  }) async {
    calls.add('install:$url');
    local = installedLocal;
    return result;
  }
}

Future<Object?> appArgs(Object args) async {
  const codec = StandardMethodCodec();
  final reply = Completer<Object?>();
  TestWidgetsFlutterBinding.instance.channelBuffers.push(
    'keytao/windows',
    codec.encodeMethodCall(MethodCall('appArgs', args)),
    (data) {
      try {
        reply.complete(data == null ? null : codec.decodeEnvelope(data));
      } catch (error, stack) {
        reply.completeError(error, stack);
      }
    },
  );
  return reply.future;
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  late PlatformCore core;
  AppController? controller;
  final iosCalls = <String>[];
  final androidCalls = <String>[];

  AppController create(BridgePlatform platform) => controller = AppController(
    info: BridgeInfo(
      platform: platform,
      appVersion: 'test',
      userRoot: '/isolated/rime',
    ),
  );

  setUp(() {
    final fixture = ViewController(BridgePlatform.ios);
    core = PlatformCore(fixture.androidSettings!);
    fixture.dispose();
    RustLib.initMock(api: core);
    iosCalls.clear();
    androidCalls.clear();
    messenger.setMockMethodCallHandler(IosHost.channel, (call) async {
      iosCalls.add(call.method);
      if (call.method == 'openUrl') {
        expect(call.arguments, {'url': 'https://example.com/keytao'});
        return null;
      }
      return switch (call.method) {
        'getPaths' => {
          'dataDir': '/ios/data',
          'cacheDir': '/ios/cache',
          'userRoot': '/ios/group/keytao',
        },
        'openSettings' => null,
        _ => throw StateError('Unexpected iOS method: ${call.method}'),
      };
    });
    messenger.setMockMethodCallHandler(const MethodChannel('keytao/android'), (
      call,
    ) async {
      androidCalls.add(call.method);
      throw StateError('Android host must not be used');
    });
  });

  tearDown(() async {
    controller?.dispose();
    controller = null;
    await core.events.close();
    RustLib.dispose();
    messenger.setMockMethodCallHandler(IosHost.channel, null);
    messenger.setMockMethodCallHandler(
      const MethodChannel('keytao/android'),
      null,
    );
  });

  test(
    'BridgeConfig preserves Tauri Windows paths and iOS App Group paths',
    () async {
      final windows = await bridgeConfig(
        BridgePlatform.windows,
        environment: {'LOCALAPPDATA': r'C:\Users\Test\AppData\Local'},
        resolvedExecutable: r'C:\Program Files\KeyTao\keytao-app.exe',
      );
      expect(
        windows.dataDir,
        r'C:\Users\Test\AppData\Local\ink.rea.keytao-app',
      );
      expect(windows.cacheDir, windows.dataDir);
      expect(windows.resourceDir, r'C:\Program Files\KeyTao');
      final ios = await bridgeConfig(BridgePlatform.ios);
      expect(ios.dataDir, '/ios/data');
      expect(ios.resourceDir, ios.dataDir);
      expect(ios.cacheDir, '/ios/cache');
      expect(ios.userRootOverride, '/ios/group/keytao');
      expect(iosCalls, ['getPaths']);
    },
  );

  test(
    'Linux XDG roots and resolved executable retain packaged resources',
    () async {
      for (final xdg in [true, false]) {
        final config = await bridgeConfig(
          BridgePlatform.linux,
          environment: {
            'HOME': '/home/test',
            'XDG_DATA_HOME': xdg ? '/data' : 'relative',
            'XDG_CACHE_HOME': xdg ? '/cache' : '',
          },
          resolvedExecutable: '/usr/lib/KeyTao/keytao-app',
        );
        expect(
          config.dataDir,
          '${xdg ? '/data' : '/home/test/.local/share'}/ink.rea.keytao-app',
        );
        expect(
          config.cacheDir,
          '${xdg ? '/cache' : '/home/test/.cache'}/ink.rea.keytao-app',
        );
        expect(config.resourceDir, '/usr/lib/KeyTao');
      }
      // Exercise an actual symlink, supplying the OS-resolved executable as Dart does.
      final root = await Directory.systemTemp.createTemp('keytao-b3-path-');
      addTearDown(() => root.delete(recursive: true));
      final executable = await File('${root.path}/lib/KeyTao/keytao-app')
          .create(recursive: true);
      final link = await Link('${root.path}/keytao-app')
          .create(executable.path);
      final config = await bridgeConfig(
        BridgePlatform.linux,
        environment: {'HOME': root.path},
        resolvedExecutable: await link.resolveSymbolicLinks(),
      );
      expect(
        config.resourceDir,
        await executable.parent.resolveSymbolicLinks(),
      );
    },
  );

  test('Windows first and later args open input and serialize redeploy after registration', () async {
    final c = create(BridgePlatform.windows)..activePage = AppPage.about;
    core.registrationGate = Completer<void>();
    await c.connectEvents(args: ['keytao-app.exe', 'keytao://ime/redeploy']);
    final refreshing = c.refreshState();
    await settle();
    expect(core.calls, ['register']);
    expect(c.busy, isTrue);
    expect(await c.deploy(), isFalse);
    await appArgs(['keytao://ime/open']);
    await settle();
    expect(c.activePage, AppPage.input);
    core.registrationGate!.complete();
    await refreshing;
    await settle();
    await settle();
    expect(core.args, [
      ['keytao-app.exe', 'keytao://ime/redeploy'],
      ['keytao://ime/open'],
    ]);
    expect(core.calls.where((call) => call == 'redeploy:7'), hasLength(1));
    expect(core.calls, contains('dismiss:7'));
    expect(c.local!.deployed, isTrue);
    expect(c.error, isNull);
    await c.refreshState();
    expect(core.calls.where((call) => call == 'register'), hasLength(1));
    expect(core.calls.where((call) => call == 'search'), hasLength(1));
    await expectLater(
      appArgs(['keytao://ime/open', 12]),
      throwsA(isA<PlatformException>()),
    );
    expect(core.args, hasLength(2));
  });

  test('Windows action waits through another operation and failure is acknowledged once', () async {
    final c = create(BridgePlatform.windows);
    await c.connectEvents();
    await c.refreshState();
    await settle();
    final gate = Completer<void>();
    final running = c.run(() => gate.future);
    core.failDeploy = true;
    await appArgs(['keytao://ime/redeploy']);
    await settle();
    expect(core.calls.where((call) => call.startsWith('redeploy')), isEmpty);
    gate.complete();
    await running;
    await settle();
    await settle();
    expect(core.calls.where((call) => call == 'redeploy:7'), hasLength(1));
    expect(core.calls.where((call) => call == 'dismiss:7'), hasLength(1));
    expect(c.deploySteps.last.state, DeployStepState.failed);
    expect(c.deploySteps.last.message, 'offline redeploy');
    expect(c.unhandledError, isNull);
  });

  test(
    'Windows reports and acknowledges actions when the local check fails',
    () async {
      final c = create(BridgePlatform.windows);
      core.failLocal = true;
      await c.connectEvents(args: ['keytao://ime/redeploy']);
      await c.refreshState();
      await settle();
      await settle();
      expect(core.calls, contains('dismiss:7'));
      expect(core.calls.where((call) => call.startsWith('redeploy')), isEmpty);
      expect(c.deploySteps.last.message, '检查本地方案失败：offline local check');
    },
  );

  test(
    'Linux startup failure remains visible after a successful status read',
    () async {
      final c = create(BridgePlatform.linux);
      core.failStart = true;
      await c.refreshState();
      expect(c.linuxImeError, 'offline Linux start');
      await c.refreshState();
      expect(c.linuxImeError, isNull);
      expect(core.calls.where((call) => call.startsWith('linuxStart')), [
        'linuxStart:false',
      ]);
    },
  );

  test(
    'Windows status events gate mutations and preserve registration errors',
    () async {
      final c = create(BridgePlatform.windows);
      core.failSearch = true;
      await c.connectEvents();
      await c.refreshState();
      expect(c.imeUiError, AppStrings.windowsSearchFailure('offline search'));
      for (final entry in {
        'registering': AppStrings.registering,
        'repairing': AppStrings.repairing,
        'checking': AppStrings.checkingRegistration,
      }.entries) {
        core.events.add(
          BridgeEvent(
            kind: BridgeEventKind.windowsImeStatus,
            windowsImeStatus: windowsStatus(
              state: entry.key,
              busy: true,
              registered: false,
            ),
          ),
        );
        await settle();
        expect(c.windowsRegistrationLabel, entry.value);
        expect(c.busy, isTrue);
        expect(await c.run(() async => fail('must not run')), isFalse);
      }
      core.windows = windowsStatus(
        state: 'repair_failed',
        registered: false,
        error: 'repair error',
      );
      core.events.add(
        BridgeEvent(
          kind: BridgeEventKind.windowsImeStatus,
          windowsImeStatus: core.windows,
        ),
      );
      await settle();
      expect(c.busy, isFalse);
      expect(c.windowsRegistrationLabel, AppStrings.repairFailed);
      expect(c.windowsImeError, 'repair error');
      await c.refreshWindowsIme();
      expect(core.calls.last, 'windowsStatus');
      expect(c.windowsImeError, 'repair error');
    },
  );

  test('Linux starts once without restart, refreshes status and selects Linux download', () async {
    final c = create(BridgePlatform.linux);
    await c.connectEvents();
    await c.refreshState();
    await c.refreshState();
    c.onResume();
    await settle();
    expect(core.calls.where((call) => call.startsWith('linuxStart')), [
      'linuxStart:false',
    ]);
    expect(core.calls.where((call) => call == 'linuxStatus'), hasLength(3));
    expect(c.linuxIme, linuxStatus);
    expect(c.error, isNull);
    expect(c.hasOnboarding, isFalse);
  });

  test('iOS installs, deploys and saves mobile settings without Android host calls', () async {
    final c = create(BridgePlatform.ios);
    await c.connectEvents();
    await c.refreshState();
    expect(c.androidSettings, core.mobileSettings);
    expect(c.error, isNull);
    expect(core.calls, isNot(contains('englishMode')));
    expect(await c.saveAndroid({'longPressDelayMs': 350}), isTrue);
    expect(core.mobileSettings.longPressDelayMs, 350);
    expect(await c.openInputMethodSettings(), isTrue);
    expect(await c.open('https://example.com/keytao'), isTrue);
    expect(iosCalls, ['openSettings', 'openUrl']);
    const urls = DownloadUrlsDto(windows: 'win', linux: 'linux', ios: 'ios');
    c.latestRelease = const ReleaseInfoDto(
      version: '1',
      name: '',
      publishedAt: '',
      body: '',
      gitee: PlatformReleaseDto(version: '1', downloadUrls: urls),
    );
    expect(c.downloadUrl, 'ios');
    expect(await c.install(), isTrue);
    expect(core.calls, contains('install:ios'));
    expect(await c.deploy(), isTrue);
    expect(c.local, deployedLocal);
    expect(c.setupReady, isTrue);
    expect(await c.finishOnboarding(), isTrue);
    expect(core.onboardingWrites, 1);
    expect(androidCalls, isEmpty);
    for (final platform in [BridgePlatform.windows, BridgePlatform.linux]) {
      final other = AppController(
        info: BridgeInfo(
          platform: platform,
          appVersion: 'test',
          userRoot: '/isolated',
        ),
      )..latestRelease = c.latestRelease;
      expect(
        other.downloadUrl,
        platform == BridgePlatform.windows ? 'win' : 'linux',
      );
      other.dispose();
    }
  });
}
