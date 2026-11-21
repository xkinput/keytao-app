import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:keytao/src/app/controller.dart';
import 'package:keytao/src/rust/api/core.dart' show AddonStatusDto;
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/rust/frb_generated.dart';

import 'controller_parity_test.dart'
    show FakeAndroid, FakeCore, settle;

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
const xmjdRelease = SchemeReleaseDto(
  scheme: 'xmjd',
  label: 'xmjd',
  version: '20261104',
  name: '',
  downloadUrl: 'offline-xmjd',
  assetName: 'xmjd6.zip',
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
  String? currentPackage;
  Completer<void>? activationGate;
  Completer<void>? addonDeployGate;
  Completer<List<SchemeUpdateDto>>? updateGate;
  bool androidRemoval = false;
  String? removalFailure;
  List<String> keptFiles = [];
  SchemaListDto schemaList = const SchemaListDto(current: null, schemas: []);
  LocalSchemaDto localAfterRemoval = const LocalSchemaDto(
    installed: false,
    deployed: false,
    schemas: [],
  );

  @override
  Future<void> emitAddonDeployProgress() async {
    await super.emitAddonDeployProgress();
    await addonDeployGate?.future;
  }

  @override
  Future<List<PackageSchemaDto>> crateApiCorePackageSchemas({
    required String schemeKey,
  }) async => const [
    PackageSchemaDto(id: 'alpha', name: 'Alpha Name', selected: true),
    PackageSchemaDto(id: 'beta', name: 'Beta Name', selected: false),
  ];

  @override
  Future<bool> crateApiCoreSetPackageSchemas({
    required String schemeKey,
    required List<String> ids,
  }) async {
    calls.add('schemas:$schemeKey:${ids.join(',')}');
    return packages.any(
          (package) => package.schemeKey == schemeKey && package.active,
        ) ||
        schemeKey == 'rime_mint';
  }

  @override
  Future<String?> crateApiCoreActiveSchemaLogoPath() async =>
      local.deployed ? '/isolated/logos/${local.schemas.first}.png' : null;

  @override
  Future<void> crateApiCoreSelectSchema({required String schemaId}) async {
    calls.add('select:$schemaId');
    schemaList = SchemaListDto(current: schemaId, schemas: schemaList.schemas);
  }

  @override
  Future<SchemaListDto> crateApiCoreGetSchemas() async => schemaList;

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
          inUse: active == true ? package.schemeKey == key : package.inUse,
          hasPackageZip: package.schemeKey == key || package.hasPackageZip,
        ),
    ];
    if (active == true) {
      currentPackage = key;
      readActiveLocal(deployed: false);
    }
  }

  void readActiveLocal({required bool deployed}) {
    final active = packages.firstWhere(
      (package) => package.active && package.schemeKey == currentPackage,
      orElse: () => packages.firstWhere((package) => package.active),
    );
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
  Future<void> crateApiCoreDownloadCloudPackage({required String id}) async {
    calls.add('cloudDownload:$id');
    replacePackage('xmjd', version: '20261104');
  }

  @override
  Future<void> crateApiCoreSwitchPackage({
    required String packageKey,
    String? schemaId,
    required FutureOr<String> Function() androidDeploy,
  }) async {
    calls.add(schemaId == null ? 'switch:$packageKey' : 'switch:$packageKey:$schemaId');
    await activationGate?.future;
    await deployGate?.future;
    if (packages.any((package) => package.schemeKey == packageKey)) {
      replacePackage(packageKey, active: true);
      readActiveLocal(deployed: true);
    } else {
      local = LocalSchemaDto(
        installed: true,
        deployed: true,
        schemas: [packageKey],
        installedScheme: packageKey,
      );
    }
  }

  @override
  Future<List<String>> crateApiCoreRemoveSchemePackage({
    required String schemeKey,
    required FutureOr<String> Function() androidDeploy,
  }) async {
    calls.add('packageRemove:$schemeKey');
    final active = packages.any(
      (package) => package.schemeKey == schemeKey && package.active,
    );
    packages.removeWhere((package) => package.schemeKey == schemeKey);
    if (active) {
      if (androidRemoval) expect(await androidDeploy(), isEmpty);
      local = localAfterRemoval;
    }
    if (removalFailure case final failure?) throw StateError(failure);
    return keptFiles;
  }

  @override
  Future<InstallResultDto> crateApiCoreInstallSchemeFromUrl({
    required String url,
    required String schemeKey,
    String? version,
  }) async {
    calls.add('install:$url');
    replacePackage(schemeKey, version: version);
    return successfulInstall;
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
    'schema changes refresh package usage without changing activation',
    () async {
      for (final select in [false, true]) {
        core.packages = [keytaoPackage];
        await controller.refreshSchemeUpdates();
        core.packages = [
          const InstalledPackageDto(
            schemeKey: 'keytao',
            active: true,
            inUse: false,
            hasPackageZip: true,
          ),
          const InstalledPackageDto(
            schemeKey: 'xmjd',
            active: true,
            inUse: true,
            hasPackageZip: true,
          ),
        ];
        if (select) {
          expect(await controller.selectSchema('xmjd6'), isTrue);
        } else {
          await controller.refreshSchemas();
        }
        expect(controller.installedPackageFor('keytao')!.active, isTrue);
        expect(controller.installedPackageFor('keytao')!.inUse, isFalse);
        expect(controller.installedPackageFor('xmjd')!.inUse, isTrue);
      }
    },
  );

  test('inactive updates require Android storage before downloading', () async {
    controller.dispose();
    controller = create(mobile: true);
    android.permission = {'granted': false, 'writable': false};
    await controller.refreshSchemeUpdates();
    final activeLocal = core.local;

    expect(await controller.updateSchemePackage('xmjd'), isFalse);

    expect(core.calls, ['permission', 'permissionSettings']);
    expect(core.downloadedSources, isEmpty);
    expect(core.local, same(activeLocal));
    expect(controller.installedPackageFor('keytao')!.active, isTrue);
    expect(controller.installedPackageFor('xmjd')!.version, '20261101');
    expect(controller.busy, isFalse);
  });

  test('using a deployed package switches its runtime', () async {
    core.packages[1] = const InstalledPackageDto(
      schemeKey: 'xmjd',
      active: true,
      inUse: false,
      hasPackageZip: true,
    );
    core.schemaList = const SchemaListDto(
      current: 'keytao',
      schemas: [
        SchemaDto(id: 'keytao', name: 'KeyTao'),
        SchemaDto(id: 'alpha', name: 'Alpha'),
        SchemaDto(id: 'beta', name: 'Beta'),
      ],
    );
    controller.local = core.local;
    await controller.refreshSchemeUpdates();
    await controller.refreshSchemas();
    core.calls.clear();

    expect(await controller.activateSchemePackage('xmjd'), isTrue);
    expect(core.calls, ['switch:xmjd']);
    expect(controller.busy, isFalse);
    expect(controller.deployingSchemeKey, isNull);
    expect(await controller.activateSchemePackage('xmjd'), isFalse);
    expect(core.calls, ['switch:xmjd']);
  });

  test(
    'using an independently installed add-on switches its runtime',
    () async {
      for (final deployed in [true, false]) {
        const key = 'rime_ice';
        final status = AddonStatusDto(
          installed: true,
          deployed: deployed,
          version: '1',
          englishAvailable: false,
        );
        core.rimeIceStatus = status;
        controller.rimeIce = core.rimeIceStatus;
        controller.local = core.local;
        core.schemaList = const SchemaListDto(
          current: 'keytao',
          schemas: [
            SchemaDto(id: 'keytao', name: 'KeyTao'),
            SchemaDto(id: 'alpha', name: 'Alpha'),
          ],
        );
        await controller.refreshSchemeUpdates();
        core.calls.clear();

        expect(await controller.activateSchemePackage(key), isTrue);
        expect(core.calls, ['switch:rime_ice']);
        expect(controller.activeSchemeKey, key);
      }
    },
  );

  test('implicit addon deployments track only the managed package', () async {
    controller.local = core.local;
    await controller.connectEvents();
    await settle();
    core.addonDeployProgress = true;
    for (final key in ['easy_en', 'rime_ice']) {
      core.addonDeployGate = Completer<void>();
      final managing = key == 'easy_en'
          ? controller.installAddon()
          : controller.manageRimeIce(true);
      await settle();
      expect(controller.deployingSchemeKey, key == 'easy_en' ? isNull : key);
      core.addonDeployGate!.complete();
      expect(await managing, isTrue);
      expect(controller.deployingSchemeKey, isNull);
    }
  });

  test(
    'using an active legacy package downloads the missing archive',
    () async {
      core.packages[1] = const InstalledPackageDto(
        schemeKey: 'xmjd',
        active: true,
        inUse: false,
        hasPackageZip: false,
      );
      core.schemaList = const SchemaListDto(
        current: 'keytao',
        schemas: [
          SchemaDto(id: 'keytao', name: 'KeyTao'),
          SchemaDto(id: 'alpha', name: 'Alpha'),
        ],
      );
      await controller.refreshSchemeUpdates();
      final activating = controller.activateSchemePackage('xmjd');
      await settle();
      expect(core.calls, contains('packageDownload:xmjd'));
      core.schemeRequests['keytao']?.complete(xmjdRelease);
      core.schemeRequests['xmjd']?.complete(xmjdRelease);
      expect(await activating, isTrue);
      expect(controller.installedPackageFor('xmjd')!.hasPackageZip, isTrue);
      expect(
        core.calls,
        containsAllInOrder(['packageDownload:xmjd', 'switch:xmjd']),
      );
    },
  );

  test('redownload switches to the restored package runtime', () async {
    core.packages[1] = const InstalledPackageDto(
      schemeKey: 'xmjd',
      version: '20261101',
      active: false,
      hasPackageZip: false,
    );
    controller.dispose();
    controller = create();
    controller.selectSource('github');
    await controller.refreshSchemeUpdates();
      expect(await controller.downloadSchemePackage('xmjd'), isTrue);

    expect(
      core.calls,
      containsAllInOrder(['packageDownload:xmjd', 'switch:xmjd']),
    );
    expect(core.downloadedSources, ['github']);
    expect(controller.installedPackageFor('xmjd')!.hasPackageZip, isTrue);
    expect(controller.installedPackageFor('xmjd')!.active, isTrue);
    expect(controller.installedPackageFor('keytao')!.active, isFalse);
    expect(controller.activeSchemeKey, 'xmjd');
  });

  test('updating a stored package switches to its updated runtime', () async {
    core.packages[1] = const InstalledPackageDto(
      schemeKey: 'xmjd',
      version: '20261101',
      active: true,
      hasPackageZip: true,
    );
    await controller.refreshSchemeUpdates();
    final updating = controller.updateSchemePackage('xmjd');
    await settle();
    core.schemeRequests['xmjd']?.complete(xmjdRelease);
    expect(await updating, isTrue);
    expect(
      core.calls,
      containsAllInOrder(['packageDownload:xmjd', 'switch:xmjd']),
    );
    expect(controller.scheme, 'xmjd');
    expect(controller.activeSchemeKey, 'xmjd');
  });

  test(
    'cloud redownload reactivates and deploys the installed package',
    () async {
      await controller.refreshSchemeUpdates();
      controller.cloudPackages = [
        {'id': '21', 'schemeKey': 'xmjd'},
      ];
      final downloading = controller.downloadCloudPackage('21');
      await settle();
      core.schemeRequests['xmjd']?.complete(xmjdRelease);
      expect(await downloading, isTrue);
      expect(
        core.calls,
        containsAllInOrder(['cloudDownload:21', 'switch:xmjd']),
      );
    },
  );

  test(
    'active updates download and deploy the same package while staying busy',
    () async {
      await controller.refreshSchemeUpdates();
      core.deployGate = Completer<void>();
      final updating = controller.updateSchemePackage('keytao');
      await settle();

      expect(controller.busy, isTrue);
      expect(await controller.updateSchemePackage('xmjd'), isFalse);
      expect(await controller.removeSchemePackage('xmjd'), isFalse);
      expect(core.calls, ['packageDownload:keytao', 'switch:keytao']);

      core.deployGate!.complete();
      expect(await updating, isTrue);
      expect(
        core.calls,
        containsAllInOrder(['packageDownload:keytao', 'switch:keytao']),
      );
      expect(
        core.calls.any((call) => call.startsWith('packageDownload:')),
        isTrue,
      );
      expect(controller.installedPackageFor('keytao')!.version, '20261104');
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
    expect(controller.deployingSchemeKey, 'xmjd');
    expect(await controller.downloadSchemePackage('keytao'), isFalse);
    expect(await controller.removeSchemePackage('xmjd'), isFalse);
    expect(core.calls, ['switch:xmjd']);

    core.activationGate!.complete();
    await settle();
    expect(core.calls, ['switch:xmjd']);
    expect(controller.busy, isTrue);
    expect(await controller.updateSchemePackage('keytao'), isFalse);

    core.deployGate!.complete();
    expect(await activating, isTrue);
    expect(controller.installedPackageFor('xmjd')!.active, isTrue);
    expect(controller.installedPackageFor('keytao')!.active, isFalse);
    expect(controller.local!.deployed, isTrue);
    expect(controller.activeSchemaLogoPath, '/isolated/logos/xmjd6.png');
    expect(controller.activeSchemaLogoRevision, 1);
    expect(controller.busy, isFalse);
    expect(controller.deployingSchemeKey, isNull);
  });

  test('a global redeploy tracks the in-use package', () async {
    controller.local = core.local;
    for (final key in <String?>[null, 'keytao']) {
      await controller.refreshSchemeUpdates();
      core.deployGate = Completer<void>();
      final deploying = controller.deploy(schemeKey: key);
      await settle();
      expect(controller.isDeploying, isTrue);
      expect(
        controller.deployingSchemeKey,
        key ??
            controller.installedPackages
                .where((package) => package.inUse)
                .map((package) => package.schemeKey)
                .firstOrNull,
      );

      core.deployGate!.completeError(StateError('deploy failed'));
      expect(await deploying, isFalse);
      expect(controller.deployingSchemeKey, isNull);
      expect(controller.isDeploying, isFalse);
    }
  });

  test(
    'schema confirmation deploys the active package and installed addons',
    () async {
      await controller.refreshSchemeUpdates();
      expect(
        (await controller.packageSchemas('keytao')).first.name,
        'Alpha Name',
      );
      for (final key in ['keytao', 'rime_mint']) {
        core.calls.clear();
        expect(await controller.setPackageSchemas(key, ['beta']), isTrue);
        expect(
          core.calls,
          containsAllInOrder(['schemas:$key:beta', 'deployDefault']),
        );
        expect(controller.busy, isFalse);
      }
    },
  );

  test(
    'inactive schema selection only saves without Android permission or deploy',
    () async {
      controller.dispose();
      controller = create(mobile: true);
      android.permission = {'granted': false, 'writable': false};
      await controller.refreshSchemeUpdates();
      expect(await controller.setPackageSchemas('xmjd', ['beta']), isTrue);
      expect(core.calls, ['schemas:xmjd:beta']);
    },
  );

  test('inactive removal preserves the active package and selection', () async {
    await controller.refreshSchemeUpdates();

    expect(await controller.removeSchemePackage('xmjd'), isTrue);
    expect(core.calls, ['packageRemove:xmjd']);
    expect(controller.installedPackageFor('xmjd'), isNull);
    expect(controller.installedPackageFor('keytao')!.active, isTrue);
    expect(controller.scheme, 'keytao');
  });

  test('removal reports preserved user changes without failing', () async {
    core
      ..packages = [keytaoPackage]
      ..keptFiles = ['keytao.custom.yaml', 'lua/user.lua'];
    await controller.refreshSchemeUpdates();

    expect(await controller.removeSchemePackage('keytao'), isTrue);

    expect(
      controller.packageError,
      '已保留您修改过的文件：\nkeytao.custom.yaml\nlua/user.lua',
    );
    expect(
      controller.operationLogs,
      containsAll([
        endsWith('[KEPT] User-modified file: keytao.custom.yaml'),
        endsWith('[KEPT] User-modified file: lua/user.lua'),
      ]),
    );
  });

  test(
    'active removal selects a remaining cached package and refreshes state',
    () async {
      core.packages = [
        const InstalledPackageDto(
          schemeKey: 'xmjd',
          active: true,
          hasPackageZip: true,
        ),
        const InstalledPackageDto(
          schemeKey: 'keytao',
          active: false,
          hasPackageZip: true,
        ),
      ];
      core.readActiveLocal(deployed: true);
      controller.scheme = 'xmjd';
      await controller.refreshSchemeUpdates();

      expect(await controller.removeSchemePackage('xmjd'), isTrue);

      expect(controller.installedPackageFor('xmjd'), isNull);
      expect(controller.installedPackageFor('keytao')!.active, isFalse);
      expect(controller.scheme, 'keytao');
      expect(controller.local!.installed, isFalse);
      expect(controller.local!.deployed, isFalse);
      expect(controller.local!.schemas, isEmpty);
      expect(controller.activeSchemaLogoPath, isNull);
      expect(controller.schemaSwitches, isEmpty);
      expect(controller.setupReady, isFalse);
      expect(core.calls, ['packageRemove:xmjd', 'englishMode', 'latest']);
    },
  );

  for (final addonsRemain in [false, true]) {
    test(
      'active removal restores ${addonsRemain ? 'add-on-only' : 'fresh install'} state',
      () async {
        core.packages = [keytaoPackage];
        if (addonsRemain) {
          core.localAfterRemoval = const LocalSchemaDto(
            installed: true,
            deployed: true,
            schemas: ['easy_en', 'wanxiang'],
          );
        }
        await controller.refreshSchemeUpdates();

        expect(await controller.removeSchemePackage('keytao'), isTrue);

        expect(controller.installedPackages, isEmpty);
        expect(controller.scheme, 'keytao');
        expect(controller.activeSchemeKey, addonsRemain ? 'easy_en' : 'keytao');
        expect(controller.local!.installed, addonsRemain);
        expect(controller.local!.deployed, addonsRemain);
        expect(controller.setupReady, addonsRemain);
        expect(core.calls, ['packageRemove:keytao', 'englishMode', 'latest']);
      },
    );
  }

  test(
    'Android active removal uses the deployment callback and refreshes state',
    () async {
      controller.dispose();
      controller = create(mobile: true);
      android.permission = {'granted': false, 'writable': false};
      core
        ..packages = [keytaoPackage]
        ..androidRemoval = true
        ..localAfterRemoval = const LocalSchemaDto(
          installed: true,
          deployed: true,
          schemas: ['easy_en'],
        );
      await controller.refreshSchemeUpdates();

      expect(await controller.removeSchemePackage('keytao'), isTrue);

      expect(core.calls, [
        'packageRemove:keytao',
        'deployImeData',
        'permission',
        'latest',
      ]);
      expect(controller.installedPackages, isEmpty);
      expect(controller.activeSchemeKey, 'easy_en');
      expect(controller.local!.deployed, isTrue);
      expect(controller.ime?['enabled'], isTrue);
    },
  );

  test(
    'active removal refreshes actual state after a later bridge failure',
    () async {
      core
        ..packages = [keytaoPackage]
        ..removalFailure = 'package store failed';
      await controller.refreshSchemeUpdates();

      expect(await controller.removeSchemePackage('keytao'), isFalse);

      expect(controller.packageError, 'package store failed');
      expect(controller.installedPackages, isEmpty);
      expect(controller.local!.installed, isFalse);
      expect(controller.local!.deployed, isFalse);
      expect(controller.setupReady, isFalse);
    },
  );

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

  test(
    'persisted skipped-schema warning loads on startup and packageDisplayName resolves names',
    () async {
      const warningText = '方案 broken 编译失败，已跳过：missing dict\n已从切换列表移除';
      core
        ..packages = [
          const InstalledPackageDto(
            schemeKey: 'local-custom',
            version: '1.0',
            active: true,
            source: 'local',
            displayName: '我的本地双拼',
            hasPackageZip: true,
          ),
        ]
        ..local = const LocalSchemaDto(
          installed: true,
          deployed: true,
          schemas: ['keytao'],
          installedScheme: 'local-custom',
          deployWarning: warningText,
        );
      controller.dispose();
      controller = create();
      await controller.refreshState();

      expect(controller.deployWarning, warningText);
      expect(controller.packageDisplayName('local-custom'), '我的本地双拼');
      expect(controller.packageDisplayName('rime_ice'), '雾凇拼音');

      core.local = const LocalSchemaDto(
        installed: true,
        deployed: true,
        schemas: ['keytao'],
        installedScheme: 'local-custom',
      );
      await controller.refreshSchemas();
      expect(controller.deployWarning, isNull);
    },
  );
}
