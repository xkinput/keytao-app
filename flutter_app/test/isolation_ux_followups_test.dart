import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/app/app.dart';
import 'package:keytao/src/app/controller.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/rust/frb_generated.dart';
import 'package:keytao/src/ui/input_page.dart';
import 'package:keytao/src/ui/package_orphans.dart';
import 'package:keytao/src/ui/scheme_page.dart';
import 'package:keytao/src/ui/theme.dart';
import 'package:keytao/src/ui/widgets.dart';

import 'controller_parity_test.dart' show FakeCore, deployedLocal;
import 'view_fixture.dart';

class _FakeLibjdRelease implements LibjdReleaseDto {
  const _FakeLibjdRelease(this.version);

  @override
  final String version;

  @override
  int get size => 1024;

  @override
  String get sourceUrl => 'https://github.com/hronro/ime-jd/releases/download/$version/libjd.tar.xz';

  @override
  bool get isDisposed => false;

  @override
  void dispose() {}
}

class _FollowupsCore extends FakeCore {
  _FollowupsCore() {
    local = deployedLocal;
  }

  bool migrationPending = false;
  String? migrationFailure;
  String? backupNoticeDir;
  bool dismissedBackupNotice = false;
  LibjdStatusDto libjd = const LibjdStatusDto(
    available: true,
    installedVersion: 'v0.1.0',
  );
  String latestLibjdVersion = 'v0.1.0';
  String keytaoEngine = 'rime';
  int libjdDownloads = 0;

  @override
  Future<bool> crateApiCorePackageMigrationPending() async => migrationPending;

  @override
  Future<void> crateApiCoreMigrateToIsolated() async {
    if (migrationFailure case final error?) {
      throw StateError(error);
    }
  }

  @override
  Future<String?> crateApiCorePendingMigrationBackupDir() async =>
      dismissedBackupNotice ? null : backupNoticeDir;

  @override
  Future<void> crateApiCoreDismissMigrationBackupNotice() async {
    dismissedBackupNotice = true;
    calls.add('dismissBackupNotice');
  }

  List<InstalledPackageDto>? customInstalledPackages;
  final List<Map<String, Object?>> gitImports = [];

  @override
  Future<List<InstalledPackageDto>> crateApiCoreListInstalledPackages() async =>
      customInstalledPackages ??
      [
        InstalledPackageDto(
          schemeKey: 'keytao',
          version: '1.0.0',
          active: true,
          hasPackageZip: true,
          engine: keytaoEngine,
        ),
      ];

  @override
  Future<String> crateApiCoreImportGitScheme({
    required String repoUrl,
    required String mode,
    String? branch,
    String? assetName,
    required String displayName,
    String? schemeKey,
  }) async {
    gitImports.add({
      'repoUrl': repoUrl,
      'mode': mode,
      'branch': branch,
      'assetName': assetName,
      'displayName': displayName,
      'schemeKey': schemeKey,
    });
    final key = schemeKey ?? 'local-git-demo';
    customInstalledPackages = [
      InstalledPackageDto(
        schemeKey: 'keytao',
        version: '1.0.0',
        active: false,
        hasPackageZip: true,
        engine: keytaoEngine,
      ),
      InstalledPackageDto(
        schemeKey: key,
        version: 'v1.0.0',
        active: true,
        hasPackageZip: true,
        source: 'local',
        displayName: displayName.isEmpty ? 'Git 示例方案' : displayName,
        git: GitSchemeConfigDto(
          repoUrl: repoUrl,
          mode: mode,
          branch: branch,
          assetName: assetName,
        ),
      ),
    ];
    return key;
  }

  @override
  Future<void> crateApiCoreSwitchPackage({
    required String packageKey,
    String? schemaId,
    required FutureOr<String> Function() androidDeploy,
  }) async {
    calls.add('switchPackage:$packageKey:${schemaId ?? ''}');
  }

  @override
  Future<LibjdStatusDto> crateApiCoreLibjdStatus() async => libjd;

  @override
  Future<LibjdReleaseDto> crateApiCoreLibjdRelease() async =>
      _FakeLibjdRelease(latestLibjdVersion);

  @override
  Future<LibjdStatusDto> crateApiCoreInstallOrUpdateLibjd({
    required LibjdReleaseDto release,
  }) async {
    libjdDownloads++;
    keytaoEngine = 'libjd';
    return libjd = LibjdStatusDto(
      available: true,
      installedVersion: release.version,
    );
  }

  @override
  Future<void> crateApiCoreSetPackageEngine({
    required String key,
    required String engine,
  }) async {
    calls.add('setEngine:$key:$engine');
    keytaoEngine = engine;
  }

  @override
  Future<List<SchemaSwitchDto>> crateApiCoreGetSchemaSwitches() async => const [
    SchemaSwitchDto(
      schemaId: 'keytao',
      name: 'simplification',
      states: ['简体', '繁体'],
      value: 0,
      options: [],
    ),
  ];

  @override
  Future<String> crateApiCorePreviewCandidates({int? size}) async =>
      '{"page_size":${size ?? 5},"candidates":[{"text":"键道","comment":"jd"}]}';

  final Map<String, String?> remoteLogos = {};

  @override
  Future<String?> crateApiCoreRemoteSchemeLogo({
    required String schemeKey,
    required String repoUrl,
    String? branch,
  }) async {
    calls.add('remoteLogo:$schemeKey:$repoUrl');
    return remoteLogos[schemeKey];
  }

  @override
  Future<String?> crateApiCoreRepairSchemePackage({
    required String packageKey,
    required FutureOr<String> Function() androidDeploy,
  }) async {
    calls.add('repair:$packageKey');
    return packageKey.startsWith('local-')
        ? null
        : '/rime/migrations/repair-$packageKey-1';
  }
}

class _OrphanController extends ViewController {
  _OrphanController() : super(BridgePlatform.macOs);

  PackageOrphanReportDto report = PackageOrphanReportDto(
    files: [
      PackageOrphanFileDto(
        path: 'JD6_userdata/a.txt',
        sha256: 'sha-a',
        size: BigInt.from(100),
      ),
      PackageOrphanFileDto(
        path: '.DS_Store',
        sha256: 'sha-ds',
        size: BigInt.from(64),
      ),
    ],
    keptBackups: const [
      PackageKeptBackupDto(
        path: 'JD6_userdata/JD6_overrides.tsv',
        backup: '/store/backups/abc/JD6_userdata/JD6_overrides.tsv',
      ),
      PackageKeptBackupDto(
        path: 'rime.lua',
        backup: '/rime/.keytao-rime-user-0.lua',
      ),
    ],
  );

  List<String> trashedFiles = [];
  List<String> trashedBackups = [];

  @override
  Future<PackageOrphanReportDto> packageOrphanReport() async => report;

  @override
  Future<bool> trashPackageOrphans(
    List<PackageOrphanFileDto> files, [
    List<PackageKeptBackupDto> keptBackups = const [],
  ]) async {
    trashedFiles = files.map((f) => f.path).toList();
    trashedBackups = keptBackups.map((b) => b.backup).toList();
    report = PackageOrphanReportDto(
      files: report.files
          .where((f) => !trashedFiles.contains(f.path))
          .toList(),
      keptBackups: report.keptBackups
          .where((b) => !trashedBackups.contains(b.backup))
          .toList(),
    );
    return true;
  }
}

class _AppTestController extends ViewController {
  _AppTestController() : super(BridgePlatform.macOs);

  final openedPaths = <String>[];
  int dismissedCount = 0;

  @override
  Future<bool> open(String value) async {
    openedPaths.add(value);
    return true;
  }

  @override
  Future<void> dismissMigrationBackupNotice() async {
    migrationBackupDir = null;
    dismissedCount++;
    notifyListeners();
  }
}

void main() {
  testWidgets(
    'orphan dialog supports one-click select all and deleting kept backups',
    (tester) async {
      final c = _OrphanController();
      addTearDown(c.dispose);
      tester.view.devicePixelRatio = 1;
      tester.view.physicalSize = const Size(1200, 900);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);

      await tester.pumpWidget(
        KeyTaoTheme(
          platform: BridgePlatform.macOs,
          home: Builder(
            builder: (context) => FScaffold(
              child: ActionButton(
                '打开清理',
                onPress: () => showPackageOrphans(context, c),
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();

      await tester.tap(find.text('打开清理'));
      await tester.pumpAndSettle();

      expect(find.text('已保留的文件备份'), findsOneWidget);
      expect(find.text('移到废纸篓 (0)'), findsOneWidget);
      expect(find.text('全选'), findsOneWidget);

      await tester.tap(find.text('全选'));
      await tester.pumpAndSettle();

      expect(find.text('取消全选'), findsOneWidget);
      expect(find.text('移到废纸篓 (4)'), findsOneWidget);

      await tester.tap(find.text('移到废纸篓 (4)'));
      await tester.pumpAndSettle();

      expect(c.trashedFiles, ['JD6_userdata/a.txt', '.DS_Store']);
      expect(c.trashedBackups, [
        '/store/backups/abc/JD6_userdata/JD6_overrides.tsv',
        '/rime/.keytao-rime-user-0.lua',
      ]);
      expect(find.text('没有发现残留文件。'), findsOneWidget);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets(
    'migration failure shows 5s toast once without top bar and backup notice opens directory',
    (tester) async {
      final c = _AppTestController();
      addTearDown(c.dispose);
      tester.view.devicePixelRatio = 1;
      tester.view.physicalSize = const Size(1200, 900);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);

      await tester.pumpWidget(
        KeyTaoApp(
          controller: c,
          initialOnboarding: const OnboardingDto(completed: true),
        ),
      );
      await tester.pumpAndSettle();

      expect(find.text('迁移失败，重试'), findsNothing);

      final api = _FollowupsCore()..migrationFailure = '方案目录迁移失败：权限不足';
      RustLib.initMock(api: api);
      addTearDown(() async {
        await api.events.close();
        RustLib.dispose();
      });

      final real = AppController(
        info: const BridgeInfo(
          platform: BridgePlatform.macOs,
          appVersion: '1.2.15-alpha.7',
          userRoot: '/rime',
        ),
      );
      addTearDown(real.dispose);

      await real.startPackageMigration();
      expect(real.takeMigrationFailureToast(), contains('方案目录迁移失败'));
      // Second failure does not emit another toast.
      await real.startPackageMigration();
      expect(real.takeMigrationFailureToast(), isNull);

      c.migrationBackupDir = '/rime/migrations/1.2.15-alpha.7';
      c.notifyListeners();
      await tester.pumpAndSettle();

      expect(find.text('方案目录已更新'), findsOneWidget);
      expect(find.text('/rime/migrations/1.2.15-alpha.7'), findsOneWidget);
      expect(find.text('打开备份文件目录'), findsOneWidget);

      await tester.tap(find.text('打开备份文件目录'));
      await tester.pumpAndSettle();

      expect(c.openedPaths, ['/rime/migrations/1.2.15-alpha.7']);
      expect(c.dismissedCount, 1);
      expect(find.text('方案目录已更新'), findsNothing);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  test(
    'switching back to installed ime-jd skips download when up to date and candidate count skips Rime deploy on ime-jd',
    () async {
      final api = _FollowupsCore();
      RustLib.initMock(api: api);
      final c = AppController(
        info: const BridgeInfo(
          platform: BridgePlatform.macOs,
          appVersion: '1.2.15-alpha.7',
          userRoot: '/rime',
        ),
      )
        ..local = deployedLocal
        ..libjd = api.libjd
        ..schemaSwitches = const [
          SchemaSwitchDto(
            schemaId: 'keytao',
            name: 'simplification',
            states: ['简体', '繁体'],
            value: 0,
            options: [],
          ),
        ]
        ..installedPackages = [
          const InstalledPackageDto(
            schemeKey: 'keytao',
            version: '1.0.0',
            active: true,
            hasPackageZip: true,
            engine: 'rime',
          ),
        ];
      addTearDown(() async {
        c.dispose();
        await api.events.close();
        RustLib.dispose();
      });

      var confirmPrompts = 0;
      final notice = await c.downloadLibjd((_) async {
        confirmPrompts++;
        return true;
      });
      expect(notice, isNull);
      expect(confirmPrompts, 0);
      expect(api.libjdDownloads, 0);
      expect(api.calls, contains('setEngine:keytao:libjd'));
      expect(c.usesLibjd('keytao'), isTrue);
      // Rime schema switches are cleared and hidden while using ime-jd.
      expect(c.schemaSwitches, isEmpty);

      api.calls.clear();
      expect(await c.saveCandidatePageSize(8), isTrue);
      expect(c.candidatePageSize, 8);
      expect(api.calls.where((call) => call.contains('deploy')), isEmpty);

      // Switching back to Rime restores Rime schema switches and candidate count triggers Rime deploy.
      await c.revertLibjd();
      expect(c.usesLibjd('keytao'), isFalse);
      expect(c.schemaSwitches, hasLength(1));
      api.calls.clear();
      expect(await c.saveCandidatePageSize(6), isTrue);
      expect(api.calls, contains('deployDefault'));
    },
  );

  testWidgets(
    'using ime-jd blocks downloading and switching other schemes with toast prompt until switched back to Rime',
    (tester) async {
      final api = _FollowupsCore()..keytaoEngine = 'libjd';
      RustLib.initMock(api: api);
      final c = AppController(
        info: const BridgeInfo(
          platform: BridgePlatform.macOs,
          appVersion: '1.2.15-alpha.7',
          userRoot: '/rime',
        ),
      )
        ..local = deployedLocal
        ..libjd = api.libjd
        ..schemas = const [SchemaDto(id: 'keytao', name: '键道6')]
        ..currentSchemaId = 'keytao'
        ..isolatedPackages = const [
          IsolatedPackageDto(
            packageKey: 'keytao',
            displayName: '键道6',
            active: true,
            ready: true,
            schemas: [SchemaDto(id: 'keytao', name: '键道6')],
          ),
          IsolatedPackageDto(
            packageKey: 'xmjd',
            displayName: '星猫键道',
            active: false,
            ready: true,
            schemas: [SchemaDto(id: 'xmjd6', name: '星猫键道')],
          ),
        ]
        ..installedPackages = const [
          InstalledPackageDto(
            schemeKey: 'keytao',
            version: '1.0.0',
            active: true,
            hasPackageZip: true,
            engine: 'libjd',
          ),
          InstalledPackageDto(
            schemeKey: 'xmjd',
            version: '20261101',
            active: false,
            hasPackageZip: true,
          ),
        ];
      addTearDown(() async {
        c.dispose();
        await api.events.close();
        RustLib.dispose();
      });

      await tester.binding.setSurfaceSize(const Size(1200, 900));
      addTearDown(() => tester.binding.setSurfaceSize(null));

      await tester.pumpWidget(
        KeyTaoApp(
          controller: c,
          initialOnboarding: const OnboardingDto(completed: true),
        ),
      );
      await tester.pump();
      expect(find.byType(InputPage), findsOneWidget);

      // Tapping current-schema-selector on Home while on ime-jd prompts to switch back to Rime.
      await tester.tap(find.byKey(const ValueKey('current-schema-selector')));
      await tester.pump();
      expect(find.text('请先切换回 Rime 才能使用'), findsWidgets);

      // Controller actions for downloading or switching other schemes are all blocked.
      expect(await c.activateSchemePackage('xmjd'), isFalse);
      expect(await c.downloadSchemePackage('xmjd'), isFalse);
      expect(await c.installSchemePackage('txjx'), isFalse);
      expect(await c.importLocalScheme(), isFalse);
      expect(
        await c.importGitScheme(
          repoUrl: 'https://github.com/acme/demo',
          mode: 'release',
        ),
        isFalse,
      );
      expect(await c.installAddon(), isFalse);
      expect(await c.manageWanxiang(true), isFalse);
      expect(await c.manageRimeIce(true), isFalse);
      expect(c.packageError, '请先切换回 Rime 才能使用');
      expect(api.calls.where((call) => call.startsWith('switchPackage:')), isEmpty);
      expect(api.gitImports, isEmpty);

      // Let toast auto-dismiss timer finish cleanly.
      await tester.pump(const Duration(seconds: 5));
      await tester.pumpAndSettle();
    },
  );

  testWidgets(
    'Git scheme dialog configures release fixed filename and auto-detects branch tree URLs',
    (tester) async {
      final api = _FollowupsCore();
      RustLib.initMock(api: api);
      final c = AppController(
        info: const BridgeInfo(
          platform: BridgePlatform.macOs,
          appVersion: '1.2.15-alpha.7',
          userRoot: '/rime',
        ),
      )
        ..local = deployedLocal
        ..installedPackages = const [
          InstalledPackageDto(
            schemeKey: 'keytao',
            version: '1.0.0',
            active: true,
            hasPackageZip: true,
          ),
        ];
      addTearDown(() async {
        c.dispose();
        await api.events.close();
        RustLib.dispose();
      });

      await tester.binding.setSurfaceSize(const Size(1280, 1000));
      addTearDown(() => tester.binding.setSurfaceSize(null));

      await tester.pumpWidget(
        KeyTaoTheme(
          platform: BridgePlatform.macOs,
          home: FScaffold(
            child: ListenableBuilder(
              listenable: c,
              builder: (_, _) => SchemePage(controller: c),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();

      final addGit = find.byKey(const ValueKey('add-git-scheme-button'));
      await tester.ensureVisible(addGit);
      await tester.tap(addGit);
      await tester.pumpAndSettle();
      expect(find.text('从 Git 添加方案'), findsOneWidget);

      // 1. Pasting a /tree/dev URL automatically selects branch mode and fills 'dev'.
      await tester.enterText(
        find.byKey(const ValueKey('git-scheme-url')),
        'https://github.com/acme/rime-custom/tree/dev',
      );
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('git-scheme-branch')), findsOneWidget);
      expect(find.text('dev'), findsOneWidget);

      // 2. Switch to Release mode and configure a fixed latest filename.
      await tester.tap(find.text('Release 发布包'));
      await tester.pumpAndSettle();
      await tester.enterText(
        find.byKey(const ValueKey('git-scheme-url')),
        'https://github.com/acme/rime-custom',
      );
      await tester.enterText(
        find.byKey(const ValueKey('git-scheme-asset')),
        'full.zip',
      );
      await tester.enterText(
        find.byKey(const ValueKey('git-scheme-name')),
        '自定义 Git 方案',
      );
      await tester.pump();

      await tester.tap(find.byKey(const ValueKey('git-scheme-submit')));
      await tester.pumpAndSettle();

      expect(api.gitImports, hasLength(1));
      expect(api.gitImports.single, {
        'repoUrl': 'https://github.com/acme/rime-custom',
        'mode': 'release',
        'branch': null,
        'assetName': 'full.zip',
        'displayName': '自定义 Git 方案',
        'schemeKey': null,
      });
      expect(find.text('从 Git 添加方案'), findsNothing);
      expect(find.text('自定义 Git 方案'), findsOneWidget);
    },
  );

  testWidgets(
    'repairSchemePackage and auto-repair progress emit logs and toast notices',
    (tester) async {
      final api = _FollowupsCore();
      RustLib.initMock(api: api);
      final c = AppController(
        info: const BridgeInfo(
          platform: BridgePlatform.macOs,
          appVersion: '1.2.15-alpha.7',
          userRoot: '/rime',
        ),
      )
        ..local = deployedLocal
        ..installedPackages = const [
          InstalledPackageDto(
            schemeKey: 'keytao',
            version: '1.0.0',
            active: true,
            hasPackageZip: true,
          ),
        ];
      addTearDown(() async {
        c.dispose();
        await api.events.close();
        RustLib.dispose();
      });

      final connecting = tester.runAsync(() async {
        final fut = c.connectEvents();
        api.events.add(const BridgeEvent(kind: BridgeEventKind.ready));
        await fut;
      });
      await connecting;
      await tester.runAsync(() async {
        api.events.add(
          const BridgeEvent(
            kind: BridgeEventKind.installProgress,
            installProgress: InstallProgressDto(
              stage: 'repairing',
              percent: 10,
              message: '检测到 键道6 部署异常，正在自动备份并修复…',
            ),
          ),
        );
        await Future<void>.delayed(Duration.zero);
      });
      await tester.pump();
      expect(c.takePendingToast(), '检测到 键道6 部署异常，正在自动备份并修复…');
      expect(
        c.operationLogs.any((line) => line.contains('[REPAIR] 检测到 键道6 部署异常')),
        isTrue,
      );

      expect(await c.repairSchemePackage('keytao'), isTrue);
      expect(api.calls, contains('repair:keytao'));
      expect(c.takePendingToast(), '已备份异常文件并重新下载修复方案');
    },
  );

  testWidgets(
    'uninstalled scheme fetches and caches remote .ico logo and automatically switches to real logo once downloaded',
    (tester) async {
      final api = _FollowupsCore()
        ..remoteLogos['txjx'] = '/cache/remote-scheme-logos-v1/txjx/logo.png';
      RustLib.initMock(api: api);
      final c = AppController(
        info: const BridgeInfo(
          platform: BridgePlatform.macOs,
          appVersion: '1.2.15-alpha.7',
          userRoot: '/rime',
        ),
      )
        ..local = deployedLocal
        ..installedPackages = const [
          InstalledPackageDto(
            schemeKey: 'keytao',
            version: '1.0.0',
            active: true,
            hasPackageZip: true,
          ),
        ];
      addTearDown(() async {
        c.dispose();
        await api.events.close();
        RustLib.dispose();
      });

      await tester.binding.setSurfaceSize(const Size(1280, 1000));
      addTearDown(() => tester.binding.setSurfaceSize(null));

      await tester.pumpWidget(
        KeyTaoTheme(
          platform: BridgePlatform.macOs,
          home: FScaffold(
            child: ListenableBuilder(
              listenable: c,
              builder: (_, _) => SchemePackageRow(
                controller: c,
                schemeKey: 'txjx',
              ),
            ),
          ),
        ),
      );
      await tester.pump();
      await tester.pump();

      expect(
        api.calls,
        contains('remoteLogo:txjx:https://github.com/wzxmer/rime-txjx'),
      );
      expect(
        c.effectiveSchemeLogoPath('txjx'),
        '/cache/remote-scheme-logos-v1/txjx/logo.png',
      );
      expect(find.byType(SchemaLogo), findsOneWidget);

      // Subsequent rebuilds within 2 days reuse cache without calling bridge again.
      final callsBefore = api.calls.where((x) => x.startsWith('remoteLogo:txjx')).length;
      await c.ensureRemoteSchemeLogo('txjx');
      expect(
        api.calls.where((x) => x.startsWith('remoteLogo:txjx')).length,
        callsBefore,
      );

      // Once txjx is downloaded and has its real logoPath, real file replaces remote preview!
      c.installedPackages = const [
        InstalledPackageDto(
          schemeKey: 'txjx',
          version: '1.0.0',
          active: false,
          hasPackageZip: true,
          logoPath: '/cache/scheme-logos-v2/real-sha/.keytao-logo/txjx.png',
        ),
      ];
      expect(
        c.effectiveSchemeLogoPath('txjx'),
        '/cache/scheme-logos-v2/real-sha/.keytao-logo/txjx.png',
      );
    },
  );
}
