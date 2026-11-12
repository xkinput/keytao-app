import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:keytao/src/app/controller.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/rust/frb_generated.dart';

import 'controller_parity_test.dart' show FakeAndroid, FakeCore, settle;

const successfulInstall = InstallResultDto(
  mergedSchemas: [],
  logs: [],
  verify: [],
);
const keytaoPackage = InstalledPackageDto(
  schemeKey: 'keytao',
  version: 'old',
  active: true,
  hasPackageZip: true,
);
const xmjdPackage = InstalledPackageDto(
  schemeKey: 'xmjd',
  version: '20261101',
  active: false,
  hasPackageZip: true,
);
const xmjdUpdate = SchemeUpdateDto(
  schemeKey: 'xmjd',
  installedVersion: '20261101',
  latestVersion: 'v20261104',
  hasUpdate: true,
);
const packageRelease = ReleaseInfoDto(
  version: 'new',
  name: 'Release',
  publishedAt: '',
  body: '',
  github: PlatformReleaseDto(
    version: 'new',
    downloadUrls: DownloadUrlsDto(android: 'offline-gh', macos: 'offline-gh'),
  ),
  gitee: PlatformReleaseDto(
    version: 'older',
    downloadUrls: DownloadUrlsDto(android: 'offline-ge', macos: 'offline-ge'),
  ),
);

class PackageCore extends FakeCore {
  List<InstalledPackageDto> packages = [keytaoPackage, xmjdPackage];
  List<SchemeUpdateDto> updates = [xmjdUpdate];
  final dismissed = <String, String>{};
  final downloadedSources = <String>[];
  Completer<void>? activationGate;
  Completer<List<SchemeUpdateDto>>? updateGate;

  @override
  Future<List<InstalledPackageDto>> crateApiCoreListInstalledPackages() async =>
      List.of(packages);

  @override
  Future<List<SchemeUpdateDto>> crateApiCoreCheckSchemeUpdates() async =>
      updateGate == null ? List.of(updates) : await updateGate!.future;

  @override
  Future<ReleaseInfoDto> crateApiCoreFetchLatestRelease() {
    calls.add('latest');
    if (!delayReleases) return Future.value(packageRelease);
    final request = Completer<ReleaseInfoDto>();
    latestRequests.add(request);
    return request.future;
  }

  @override
  Future<Map<String, String>> crateApiCoreGetDismissedSchemeUpdates() async =>
      Map.of(dismissed);

  @override
  Future<void> crateApiCoreDismissSchemeUpdate({
    required String schemeKey,
    required String version,
  }) async {
    dismissed[schemeKey] = version;
  }

  void replacePackage(String key, {String? version, bool? active}) {
    packages = [
      for (final package in packages)
        InstalledPackageDto(
          schemeKey: package.schemeKey,
          version: package.schemeKey == key
              ? version ?? package.version
              : package.version,
          active: active == true ? package.schemeKey == key : package.active,
          hasPackageZip: package.schemeKey == key || package.hasPackageZip,
        ),
    ];
    if (active == true) readActiveLocal(deployed: false);
  }

  void readActiveLocal({required bool deployed}) {
    final active = packages.singleWhere((package) => package.active);
    local = LocalSchemaDto(
      installed: true,
      deployed: deployed,
      version: active.version,
      schemas: [active.schemeKey == 'xmjd' ? 'xmjd6' : active.schemeKey],
      installedScheme: active.schemeKey,
      installedSchemeVersion: active.version,
    );
  }

  @override
  Future<void> crateApiCoreDownloadSchemePackage({
    required String schemeKey,
    required String source,
  }) async {
    calls.add('packageDownload:$schemeKey');
    downloadedSources.add(source);
    replacePackage(schemeKey, version: '20261104');
  }

  @override
  Future<InstallResultDto> crateApiCoreActivateScheme({
    required String schemeKey,
    required FutureOr<String> Function(String) androidExtract,
  }) async {
    calls.add('activate:$schemeKey');
    await activationGate?.future;
    replacePackage(schemeKey, active: true);
    return successfulInstall;
  }

  @override
  Future<void> crateApiCoreRemoveSchemePackage({
    required String schemeKey,
  }) async {
    calls.add('packageRemove:$schemeKey');
    packages.removeWhere((package) => package.schemeKey == schemeKey);
  }

  @override
  Future<InstallResultDto> crateApiCoreInstallSchemeFromUrl({
    required String url,
    required String schemeKey,
    String? version,
  }) async {
    calls.add('install:$url');
    return successfulInstall;
  }

  @override
  Future<void> crateApiCoreRecordInstalledScheme({
    required String scheme,
    String? version,
  }) async {
    calls.add('record:$scheme:$version');
    replacePackage(scheme, version: version, active: true);
  }

  @override
  Future<DeployResultDto> crateApiCoreDeployDefault() async {
    calls.add('deployDefault');
    await deployGate?.future;
    readActiveLocal(deployed: true);
    return const DeployResultDto(success: true, message: 'done');
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  late PackageCore core;
  late FakeAndroid android;
  late AppController controller;

  AppController create({bool mobile = false}) => AppController(
    info: BridgeInfo(
      platform: mobile ? BridgePlatform.android : BridgePlatform.macOs,
      appVersion: 'test',
      userRoot: '/isolated/packages',
    ),
    android: android,
  );

  setUp(() {
    core = PackageCore()..readActiveLocal(deployed: true);
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

  test(
    'inactive updates bypass Android permission and all live writes',
    () async {
      controller.dispose();
      controller = create(mobile: true);
      android.permission = {'granted': false, 'writable': false};
      await controller.refreshSchemeUpdates();
      final activeLocal = core.local;

      expect(await controller.updateSchemePackage('xmjd'), isTrue);

      expect(core.calls, ['packageDownload:xmjd']);
      expect(core.downloadedSources, ['gitee']);
      expect(core.local, same(activeLocal));
      expect(controller.installedPackageFor('keytao')!.active, isTrue);
      expect(controller.installedPackageFor('xmjd')!.version, '20261104');
      expect(controller.busy, isFalse);
    },
  );

  test(
    'legacy redownload preserves the active scheme without storage access',
    () async {
      core.packages[1] = const InstalledPackageDto(
        schemeKey: 'xmjd',
        version: '20261101',
        active: false,
        hasPackageZip: false,
      );
      controller.dispose();
      controller = create(mobile: true);
      controller.selectSource('github');
      android.permission = {'granted': false, 'writable': false};
      await controller.refreshSchemeUpdates();
      final activeLocal = core.local;

      expect(await controller.downloadSchemePackage('xmjd'), isTrue);

      expect(core.calls, ['packageDownload:xmjd']);
      expect(core.downloadedSources, ['github']);
      expect(core.local, same(activeLocal));
      expect(controller.installedPackageFor('xmjd')!.hasPackageZip, isTrue);
      expect(controller.installedPackageFor('xmjd')!.active, isFalse);
    },
  );

  test(
    'active updates install then deploy while release fetching stays busy',
    () async {
      await controller.refreshSchemeUpdates();
      core.delayReleases = true;
      final updating = controller.updateSchemePackage('keytao');
      await settle();

      expect(controller.busy, isTrue);
      expect(await controller.updateSchemePackage('xmjd'), isFalse);
      expect(await controller.removeSchemePackage('xmjd'), isFalse);
      expect(core.calls, ['latest']);

      core.latestRequests.single.complete(packageRelease);
      expect(await updating, isTrue);
      expect(
        core.calls,
        containsAllInOrder([
          'latest',
          'install:offline-gh',
          'record:keytao:new',
          'deployDefault',
        ]),
      );
      expect(
        core.calls.any((call) => call.startsWith('packageDownload:')),
        isFalse,
      );
      expect(controller.installedPackageFor('keytao')!.version, 'new');
      expect(controller.local!.deployed, isTrue);
      expect(controller.busy, isFalse);
    },
  );

  test('activation and its deployment hold one busy lifecycle', () async {
    await controller.refreshSchemeUpdates();
    core.activationGate = Completer<void>();
    core.deployGate = Completer<void>();
    final activating = controller.activateSchemePackage('xmjd');
    await settle();

    expect(controller.busy, isTrue);
    expect(await controller.downloadSchemePackage('keytao'), isFalse);
    expect(await controller.removeSchemePackage('xmjd'), isFalse);
    expect(core.calls, ['activate:xmjd']);

    core.activationGate!.complete();
    await settle();
    expect(core.calls, ['activate:xmjd', 'release:xmjd']);
    expect(controller.busy, isTrue);
    core.schemeRequests['xmjd']!.complete(
      const SchemeReleaseDto(
        scheme: 'xmjd',
        label: 'xmjd',
        version: '20261104',
        name: '',
        downloadUrl: 'offline-xmjd',
        assetName: 'xmjd6.zip',
      ),
    );
    await settle();
    expect(core.calls, ['activate:xmjd', 'release:xmjd', 'deployDefault']);
    expect(controller.releaseVersion, '20261104');
    expect(controller.busy, isTrue);
    expect(await controller.updateSchemePackage('keytao'), isFalse);

    core.deployGate!.complete();
    expect(await activating, isTrue);
    expect(controller.installedPackageFor('xmjd')!.active, isTrue);
    expect(controller.installedPackageFor('keytao')!.active, isFalse);
    expect(controller.local!.deployed, isTrue);
    expect(controller.busy, isFalse);
  });

  test('active removal is refused before entering the bridge', () async {
    await controller.refreshSchemeUpdates();

    expect(await controller.removeSchemePackage('keytao'), isFalse);
    expect(core.calls, isEmpty);
    expect(controller.installedPackageFor('keytao')!.active, isTrue);
    expect(await controller.removeSchemePackage('xmjd'), isTrue);
    expect(core.calls, ['packageRemove:xmjd']);
    expect(controller.installedPackageFor('xmjd'), isNull);
  });

  test(
    'stale update checks cannot undo dismissal or a package write',
    () async {
      await controller.refreshSchemeUpdates();
      core.updateGate = Completer<List<SchemeUpdateDto>>();
      final beforeDismissal = controller.refreshSchemeUpdates();
      await settle();

      expect(await controller.dismissSchemeUpdate(xmjdUpdate), isTrue);
      core.updateGate!.complete([xmjdUpdate]);
      expect(await beforeDismissal, isFalse);
      expect(controller.visibleSchemeUpdates, isEmpty);

      core.updateGate = Completer<List<SchemeUpdateDto>>();
      final beforeDownload = controller.refreshSchemeUpdates();
      await settle();
      expect(await controller.downloadSchemePackage('xmjd'), isTrue);
      core.updateGate!.complete([xmjdUpdate]);
      expect(await beforeDownload, isFalse);
      expect(controller.schemeUpdateFor('xmjd'), isNull);
      expect(controller.installedPackageFor('xmjd')!.version, '20261104');
    },
  );

  test(
    'dismissal survives recreation and only a newer version reappears',
    () async {
      await controller.refreshSchemeUpdates();
      expect(controller.visibleSchemeUpdates, [xmjdUpdate]);
      expect(await controller.dismissSchemeUpdate(xmjdUpdate), isTrue);
      expect(controller.visibleSchemeUpdates, isEmpty);
      expect(controller.hasSchemeUpdates, isTrue);

      core.updates = [
        const SchemeUpdateDto(
          schemeKey: 'xmjd',
          installedVersion: '20261101',
          latestVersion: '20261104',
          hasUpdate: true,
        ),
      ];
      controller.dispose();
      controller = create();
      await controller.refreshSchemeUpdates();
      expect(controller.visibleSchemeUpdates, isEmpty);

      core.updates = [
        const SchemeUpdateDto(
          schemeKey: 'xmjd',
          installedVersion: '20261101',
          latestVersion: '20261105',
          hasUpdate: true,
        ),
      ];
      await controller.refreshSchemeUpdates();
      expect(controller.visibleSchemeUpdates.single.latestVersion, '20261105');
    },
  );
}
