import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:keytao/src/app/app.dart';
import 'package:keytao/src/app/controller.dart';
import 'package:keytao/src/app/options.dart' show AppPage, AppViewOptions;
import 'package:keytao/src/rust/api/core.dart' show AddonStatusDto;
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/rust/frb_generated.dart';
import 'package:keytao/src/ui/input_page.dart';
import 'package:keytao/src/ui/package_orphans.dart';
import 'package:keytao/src/ui/scheme_page.dart';
import 'package:keytao/src/ui/theme.dart';
import 'package:keytao/src/ui/widgets.dart';

import 'controller_parity_test.dart' show FakeCore, deployedLocal;
import 'view_fixture.dart';

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
    installedVersion: 'v0.14.0',
  );
  String inputEngine = 'rime';
  Completer<void>? engineSwitchGate;
  Completer<int>? libjdImportGate;
  Completer<String>? previewGate;
  String previewWord = '键道';
  List<IsolatedPackageDto> isolatedPackages = [];
  int simplificationValue = 0;

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
        ),
      ];

  @override
  Future<void> crateApiCoreDownloadSchemePackage({
    required String schemeKey,
    required String source,
  }) async {
    calls.add('downloadSchemePackage:$schemeKey');
    await _storeDownloadedPackage(schemeKey);
  }

  @override
  Future<void> crateApiCoreDownloadCloudPackage({required String id}) async {
    calls.add('downloadCloudPackage:$id');
    await _storeDownloadedPackage('local-cloud-fixture');
  }

  Future<void> _storeDownloadedPackage(String key) async {
    final packages = await crateApiCoreListInstalledPackages();
    customInstalledPackages = [
      for (final package in packages)
        if (package.schemeKey != key) package,
      InstalledPackageDto(
        schemeKey: key,
        version: 'v1.0.0',
        active: false,
        hasPackageZip: true,
        source: key.startsWith('local-') ? 'local' : 'official',
      ),
    ];
  }

  @override
  Future<AddonStatusDto> crateApiCoreManageRimeIce({
    required bool installed,
    required FutureOr<String> Function() androidDeploy,
  }) async {
    if (!installed || inputEngine != 'libjd_engine') {
      return super.crateApiCoreManageRimeIce(
        installed: installed,
        androidDeploy: androidDeploy,
      );
    }
    calls.add('rimeIce:$installed');
    return rimeIceStatus = const AddonStatusDto(
      installed: true,
      deployed: false,
      version: 'v1.0.0',
      englishAvailable: false,
    );
  }

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
    final packages = await crateApiCoreListInstalledPackages();
    customInstalledPackages = [
      for (final package in packages)
        if (package.schemeKey != key) package,
      InstalledPackageDto(
        schemeKey: key,
        version: 'v1.0.0',
        active: inputEngine == 'rime',
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
    if (inputEngine == 'libjd_engine') {
      _activate(packageKey, schemaId ?? packageKey);
    }
  }

  @override
  Future<LibjdStatusDto> crateApiCoreLibjdStatus() async => libjd;

  @override
  Future<String> crateApiCoreInputEngine() async => inputEngine;

  @override
  Future<void> crateApiCoreSetInputEngine({required String engine}) async {
    calls.add('setInputEngine:$engine');
    await engineSwitchGate?.future;
    inputEngine = engine;
  }

  @override
  Future<List<PackageSchemaDto>> crateApiCorePackageSchemas({
    required String schemeKey,
  }) => schemeKey == 'xmjd'
      ? Future.value(const [
          PackageSchemaDto(id: 'xmjd6', name: '星猫键道', selected: true),
          PackageSchemaDto(id: 'xmjd_alt', name: '星猫备用方案', selected: false),
        ])
      : super.crateApiCorePackageSchemas(schemeKey: schemeKey);

  @override
  Future<int> crateApiCoreImportLibjdPackage({
    required String key,
    String? schemaId,
  }) async {
    calls.add('importLibjd:$key');
    if (schemaId != null) calls.add('convertSchema:$key:$schemaId');
    events.add(
      const BridgeEvent(
        kind: BridgeEventKind.installProgress,
        installProgress: InstallProgressDto(
          stage: 'converting',
          percent: 50,
          message: '正在转换词库 1/2',
        ),
      ),
    );
    final skipped = await libjdImportGate?.future ?? 0;
    _activate(key, schemaId ?? key);
    return skipped;
  }

  void _activate(String key, String schemaId) {
    local = LocalSchemaDto(
      installed: true,
      deployed: false,
      installedScheme: key,
      schemas: [schemaId],
    );
    isolatedPackages = [
      for (final entry in isolatedPackages)
        if (entry.packageKey != key)
          IsolatedPackageDto(
            packageKey: entry.packageKey,
            displayName: entry.displayName,
            active: false,
            ready: entry.ready,
            engine: entry.engine,
            schemas: entry.schemas,
          ),
      IsolatedPackageDto(
        packageKey: key,
        displayName: key,
        active: true,
        ready: true,
        engine: 'libjd_engine',
        schemas: [SchemaDto(id: schemaId, name: schemaId)],
      ),
    ];
  }

  @override
  Future<List<IsolatedPackageDto>> crateApiCoreIsolatedPackages() async =>
      isolatedPackages;

  @override
  Future<List<SchemaSwitchDto>> crateApiCoreGetSchemaSwitches() async => [
    SchemaSwitchDto(
      schemaId: 'keytao',
      name: 'simplification',
      states: ['简体', '繁体'],
      value: simplificationValue,
      options: [],
    ),
  ];

  @override
  Future<String> crateApiCorePreviewCandidates({int? size}) async {
    final gate = previewGate;
    previewGate = null;
    return gate?.future ??
        '{"page_size":${size ?? 5},"candidates":[{"text":"$previewWord","comment":"jd"}]}';
  }

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
  test('live IME marker refreshes schema switches in an open App', () async {
    final dir = await Directory.systemTemp.createTemp('keytao-switch-watch-');
    final api = _FollowupsCore();
    RustLib.initMock(api: api);
    final c = AppController(
      info: BridgeInfo(
        platform: BridgePlatform.macOs,
        appVersion: '1.2.18-alpha.4',
        userRoot: dir.path,
      ),
    );
    addTearDown(() async {
      c.dispose();
      await api.events.close();
      RustLib.dispose();
      await dir.delete(recursive: true);
    });
    expect(await c.refreshState(), isTrue);
    expect(c.schemaSwitches.single.value, 0);
    final changed = Completer<void>();
    c.addListener(() {
      if (!changed.isCompleted && c.schemaSwitches.singleOrNull?.value == 1) {
        changed.complete();
      }
    });
    api.simplificationValue = 1;
    final temp = await File('${dir.path}/.switches.tmp').writeAsString('{}');
    await temp.rename('${dir.path}/.keytao-live-switches.json');
    await changed.future.timeout(const Duration(seconds: 5));
  });

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
    'engine preference preserves converted schemes and Rime deployment',
    () async {
      final api = _FollowupsCore()
        ..isolatedPackages = const [
          IsolatedPackageDto(
            packageKey: 'keytao',
            displayName: '键道6',
            active: true,
            ready: true,
            engine: 'libjd_engine',
          ),
        ];
      RustLib.initMock(api: api);
      final c =
          AppController(
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

      expect(await c.setInputEngine('libjd_engine'), isTrue);
      expect(api.calls, contains('setInputEngine:libjd_engine'));
      expect(c.usesLibjd('keytao'), isTrue);
      // Rime schema switches are cleared and hidden while using ime-jd.
      expect(c.schemaSwitches, isEmpty);

      api.calls.clear();
      expect(await c.saveCandidatePageSize(8), isTrue);
      expect(c.candidatePageSize, 8);
      expect(api.calls.where((call) => call.contains('deploy')), isEmpty);

      // Switching back to Rime restores Rime schema switches and candidate count triggers Rime deploy.
      expect(await c.setInputEngine('rime'), isTrue);
      expect(c.usesLibjd('keytao'), isFalse);
      expect(c.usesImportedLibjd('keytao'), isTrue);
      expect(api.calls.where((call) => call.contains('deploy')), isEmpty);
      expect(c.schemaSwitches, hasLength(1));
      api.calls.clear();
      expect(await c.saveCandidatePageSize(6), isTrue);
      expect(api.calls, contains('deployDefault'));
    },
  );

  testWidgets(
    'ime-jd import shows dictionary progress only on its card and then shows active state',
    (tester) async {
      const target = 'xmjd';
      const other = 'txjx';
      final api = _FollowupsCore()
        ..inputEngine = 'libjd_engine'
        ..libjdImportGate = Completer<int>()
        ..customInstalledPackages = const [
          InstalledPackageDto(
            schemeKey: 'keytao',
            active: true,
            hasPackageZip: true,
            engine: 'libjd_engine',
          ),
          InstalledPackageDto(
            schemeKey: target,
            active: false,
            hasPackageZip: true,
          ),
          InstalledPackageDto(
            schemeKey: other,
            active: false,
            hasPackageZip: true,
          ),
        ];
      RustLib.initMock(api: api);
      final c =
          AppController(
              info: const BridgeInfo(
                platform: BridgePlatform.macOs,
                appVersion: '1.2.16-alpha.3',
                userRoot: '/rime',
              ),
            )
            ..local = deployedLocal
            ..libjd = api.libjd
            ..inputEngine = api.inputEngine
            ..installedPackages = api.customInstalledPackages!;
      addTearDown(() async {
        c.dispose();
        await api.events.close();
        RustLib.dispose();
      });
      final ready = c.connectEvents();
      await tester.pump();
      await ready;

      await tester.pumpWidget(
        KeyTaoTheme(
          platform: c.info.platform,
          home: FScaffold(
            child: ListenableBuilder(
              listenable: c,
              builder: (_, _) => Column(
                children: [
                  for (final key in [target, other])
                    SchemePackageRow(
                      key: ValueKey(key),
                      controller: c,
                      schemeKey: key,
                    ),
                ],
              ),
            ),
          ),
        ),
      );
      Finder inCard(String key, Finder child) =>
          find.descendant(of: find.byKey(ValueKey(key)), matching: child);
      await tester.tap(inCard(target, find.text('使用')));
      await tester.pump();
      expect(api.calls, contains('importLibjd:$target'));
      final progress = find.text('正在转换词库 1/2');
      expect(inCard(target, progress), findsOneWidget);
      expect(inCard(other, progress), findsNothing);
      expect(inCard(target, find.byType(FDeterminateProgress)), findsOneWidget);
      expect(find.text('使用中'), findsNothing);

      api.libjdImportGate!.complete(0);
      await tester.pumpAndSettle();
      expect(inCard(target, find.text('使用中')), findsOneWidget);
      expect(inCard(target, find.text('切回 Rime')), findsNothing);
      expect(inCard(other, find.text('使用')), findsOneWidget);
      expect(progress, findsNothing);
      expect(c.activeSchemeKey, target);
    },
  );

  testWidgets(
    'home selects engines and ime-jd automatically converts downloaded schemes',
    (tester) async {
      const packages = [
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
      ];
      final api = _FollowupsCore()..isolatedPackages = packages;
      RustLib.initMock(api: api);
      final c =
          AppController(
              info: const BridgeInfo(
                platform: BridgePlatform.macOs,
                appVersion: '1.2.15-alpha.7',
                userRoot: '/rime',
              ),
            )
            ..local = deployedLocal
            ..libjd = api.libjd
            ..macosIme = const MacosImeStatusDto(installed: true, message: '')
            ..schemas = const [SchemaDto(id: 'keytao', name: '键道6')]
            ..currentSchemaId = 'keytao'
            ..isolatedPackages = packages
            ..installedPackages = const [
              InstalledPackageDto(
                schemeKey: 'keytao',
                version: '1.0.0',
                active: true,
                hasPackageZip: true,
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
      final engineSelector = find.byKey(
        const ValueKey('current-engine-selector'),
      );
      final schemeSelector = find.byKey(
        const ValueKey('current-schema-selector'),
      );
      expect(find.text('macOS 系统输入法 ·'), findsOneWidget);
      expect(find.descendant(of: engineSelector, matching: find.text('Rime')),
          findsOneWidget);
      expect(find.text('已部署'), findsNothing);
      expect(find.widgetWithText(ActionButton, '重新部署'), findsOneWidget);
      expect(find.widgetWithText(SwitchSetting, '算法引擎'), findsOneWidget);
      expect(find.text('切换引擎'), findsNothing);
      expect(find.text('切换方案'), findsNothing);
      await c.refreshCandidatePreview();
      expect(c.candidatePreviewCandidates.single.text, '键道');
      final macosReady = find.descendant(
        of: find.byType(MacosImeCard),
        matching: find.text('可使用'),
      );
      expect(macosReady, findsOneWidget);
      api.local = const LocalSchemaDto(
        installed: true,
        deployed: false,
        buildAvailable: true,
        schemas: ['keytao'],
      );
      await tester.tap(engineSelector);
      await tester.pumpAndSettle();
      api.engineSwitchGate = Completer<void>();
      await tester.tap(find.widgetWithText(FItem, 'ime-jd'));
      await tester.pump();
      expect(find.text('切换中'), findsOneWidget);
      api.previewWord = 'ime-jd预览';
      expect(c.candidatePreviewCandidates.single.text, '键道');
      api.engineSwitchGate!.complete();
      await tester.pumpAndSettle();
      expect(c.prefersLibjd, isTrue);
      expect(c.isUsingLibjd, isFalse);
      expect(c.hasUsableBuild, isTrue);
      expect(find.descendant(of: engineSelector, matching: find.text('ime-jd')),
          findsOneWidget);
      expect(c.candidatePreviewCandidates.single.text, 'ime-jd预览');
      expect(find.text('未转换'), findsOneWidget);
      expect(find.text('已部署'), findsNothing);
      expect(find.text('未部署'), findsNothing);
      expect(find.text('改动待部署'), findsNothing);
      expect(macosReady, findsNothing);
      expect(find.textContaining('请前往方案页下载并转换你的方案'), findsOneWidget);
      expect(find.text('了解 ime-jd'), findsOneWidget);

      final stalePreview = Completer<String>();
      api.previewGate = stalePreview;
      final firstPreview = c.refreshCandidatePreview();
      await tester.tap(schemeSelector);
      await tester.pumpAndSettle();
      await tester.tap(find.widgetWithText(FItem, '星猫键道'));
      await tester.pumpAndSettle();
      expect(api.calls, contains('convertSchema:xmjd:xmjd6'));
      expect(find.descendant(of: engineSelector, matching: find.text('ime-jd')),
          findsOneWidget);
      api.previewWord = '星猫预览';
      stalePreview.complete(
        '{"page_size":5,"candidates":[{"text":"过期待选","comment":""}]}',
      );
      await firstPreview;
      expect(c.candidatePreviewCandidates.single.text, '星猫预览');
      expect(find.text('已转换'), findsNothing);
      expect(macosReady, findsOneWidget);
      expect(find.textContaining('请前往方案页下载并转换你的方案'), findsNothing);

      expect(await c.downloadSchemePackage('keytao'), isTrue);
      expect(await c.downloadSchemePackage('xmjd'), isTrue);
      expect(await c.installSchemePackage('txjx'), isTrue);
      expect(
        await c.importGitScheme(
          repoUrl: 'https://github.com/acme/demo',
          mode: 'release',
        ),
        isTrue,
      );
      c.cloudPackages = [
        {'id': 'fixture', 'schemeKey': 'local-cloud-fixture'},
      ];
      expect(await c.downloadCloudPackage('fixture'), isTrue);
      expect(await c.manageRimeIce(true), isTrue);
      expect(c.activeSchemeKey, 'rime_ice');
      expect(c.hasPendingSchemaSelectionPrompt, isFalse);
      expect(c.installedPackageFor('txjx')?.hasPackageZip, isTrue);
      expect(c.installedPackageFor('local-git-demo')?.hasPackageZip, isTrue);
      expect(
        c.installedPackageFor('local-cloud-fixture')?.hasPackageZip,
        isTrue,
      );
      expect(c.rimeIce?.installed, isTrue);
      expect(c.rimeIce?.deployed, isFalse);
      expect(c.rimeIceStatusText, contains('使用中'));
      expect(
        api.calls.where((call) => call.startsWith('switchPackage:')),
        isEmpty,
      );
      expect(
        api.calls,
        containsAllInOrder([
          'importLibjd:keytao',
          'importLibjd:xmjd',
          'importLibjd:txjx',
          'importLibjd:local-git-demo',
          'importLibjd:local-cloud-fixture',
          'importLibjd:rime_ice',
        ]),
      );
      expect(api.gitImports, hasLength(1));
      api.calls.clear();
      expect(await c.switchPackage('xmjd'), isTrue);
      expect(api.calls, contains('switchPackage:xmjd:'));
      expect(api.calls, isNot(contains('importLibjd:xmjd')));
      c.selectPage(AppPage.scheme);
      await tester.pumpAndSettle();
      final xmjdMenu = find.descendant(
        of: find.byWidgetPredicate(
          (widget) => widget is SchemePackageRow && widget.schemeKey == 'xmjd',
        ),
        matching: find.byIcon(FLucideIcons.ellipsis),
      );
      await tester.ensureVisible(xmjdMenu);
      await tester.tap(xmjdMenu);
      await tester.pumpAndSettle();
      await tester.tap(find.widgetWithText(FItem, '选择转换方案'));
      await tester.pumpAndSettle();
      expect(find.text('星猫备用方案'), findsOneWidget);
      await tester.tap(find.text('星猫备用方案'));
      await tester.pump();
      await tester.tap(find.text('转换词库'));
      await tester.pumpAndSettle();
      expect(api.calls, contains('convertSchema:xmjd:xmjd_alt'));
      expect(c.activeSchemeKey, 'xmjd');
      expect(
        c.isolatedPackages
            .singleWhere((package) => package.active)
            .schemas
            .single
            .id,
        'xmjd_alt',
      );
      c.selectPage(AppPage.input);
      await tester.pumpAndSettle();
      api.local = const LocalSchemaDto(
        installed: false,
        deployed: false,
        schemas: [],
      );
      expect(await c.setInputEngine('rime'), isTrue);
      await tester.pumpAndSettle();
      expect(find.descendant(of: engineSelector, matching: find.text('Rime')),
          findsOneWidget);
      expect(find.text('未部署'), findsOneWidget);
      expect(find.text('请先安装输入方案。'), findsNothing);
      expect(c.canDeploy, isTrue);
      expect(c.usesImportedLibjd('xmjd'), isTrue);
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

  test('only a changed logo file invalidates its cached image', () async {
    final directory = Directory.systemTemp.createTempSync('keytao-test-logos-');
    addTearDown(() => directory.deleteSync(recursive: true));
    final logo = File('${directory.path}/logo.png')..writeAsStringSync('old');
    final api = _FollowupsCore()..remoteLogos['rime_ice'] = logo.path;
    RustLib.initMock(api: api);
    final c = AppController(
      info: const BridgeInfo(
        platform: BridgePlatform.windows,
        appVersion: '1.2.15',
        userRoot: '/rime',
      ),
    );
    addTearDown(() async {
      c.dispose();
      await api.events.close();
      RustLib.dispose();
    });

    var updates = 0;
    c.addListener(() => updates++);
    await c.ensureRemoteSchemeLogo('rime_ice', addonInstalled: false);
    final version = c.schemaLogoVersion(logo.path);
    expect(version, isNotNull);
    expect(updates, 1);

    await c.ensureRemoteSchemeLogo('rime_ice', addonInstalled: true);
    expect(c.schemaLogoVersion(logo.path), version);
    expect(updates, 1);

    logo.writeAsStringSync('new content');
    await c.ensureRemoteSchemeLogo('rime_ice', addonInstalled: false);
    expect(c.schemaLogoVersion(logo.path), isNot(version));
    expect(updates, 2);
  });

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

      // The logo lookup now includes async filesystem I/O.
      await tester.runAsync(() => c.ensureRemoteSchemeLogo('txjx'));
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
