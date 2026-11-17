import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:keytao/src/app/controller.dart';
import 'package:keytao/src/platform/android_host.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/rust/frb_generated.dart';

class SyncCore implements RustLibApi {
  final calls = <String>[];
  bool enabled = false;
  int accountId = 1;
  Map<String, dynamic>? cloudProfile;
  Map<String, dynamic>? savedSettings;
  String? accentOverride;
  String? writtenAccent;
  double? radius;
  String background = '';
  ImeUiSettingsDto get ui => ImeUiSettingsDto(
    colorScheme: UiColorSchemeDto.auto,
    effectiveColorScheme: EffectiveColorSchemeDto.dark,
    orientation: PanelOrientationDto.vertical,
    embeddedComposition: true,
    hideCandidatePanel: true,
    showTrayModeIndicator: true,
    effectiveEmbeddedComposition: true,
    accentColor: accentOverride ?? '#4A8DF6',
    accentColorOverride: accentOverride,
    fontSize: 24,
    cornerRadius: radius,
    backgroundColor: background,
    themeExists: true,
    message: '',
  );
  final profile = <String, dynamic>{
    'version': 1,
    'active_scheme': 'keytao',
    'packages': <dynamic>[],
    'settings': <String, dynamic>{},
  };

  @override
  dynamic noSuchMethod(Invocation invocation) {
    final method = invocation.memberName.toString();
    calls.add(method);
    if (method.contains('GetImeUiSettings')) {
      return Future<ImeUiSettingsDto>.value(ui);
    }
    if (method.contains('SetImeUiSettings')) {
      writtenAccent = invocation.namedArguments[#accentColor] as String?;
      accentOverride = writtenAccent ?? accentOverride;
      radius = invocation.namedArguments[#cornerRadius] as double?;
      background = invocation.namedArguments[#backgroundColor] as String;
      return Future<ImeUiSettingsDto>.value(ui);
    }
    if (method.contains('GetSchemeSyncState') ||
        method.contains('SetSchemeSyncPreferences')) {
      if (invocation.namedArguments[#settingsJson] case final String json) {
        savedSettings = jsonDecode(json) as Map<String, dynamic>;
      }
      return Future<String>.value(
        jsonEncode({
          'settings': savedSettings ?? <String, dynamic>{},
          'follow_account_sync': enabled,
          'synced_schemes': ['keytao'],
        }),
      );
    }
    if (method.contains('AccountSession')) {
      return Future<AccountUserDto?>.value(
        AccountUserDto(id: accountId, name: 'User'),
      );
    }
    if (method.contains('FetchSchemeProfile')) {
      return Future<String>.value(jsonEncode({'profile': cloudProfile}));
    }
    if (method.contains('ExportSchemeProfile')) {
      return Future<String>.value(jsonEncode(profile));
    }
    if (method.contains('ApplySchemeProfile')) {
      return Future<String>.value(
        invocation.namedArguments[#profileJson] as String,
      );
    }
    if (method.contains('ListInstalledPackages')) {
      return Future<List<InstalledPackageDto>>.value([]);
    }
    if (method.contains('ListCloudPackages')) {
      return Future<String>.value('[]');
    }
    if (method.contains('UploadCloudPackage')) {
      return Future<String>.value('{}');
    }
    if (method.contains('UploadSchemeProfile')) {
      return Future<String>.value(
        jsonEncode({'profile': profile, 'updateAt': '2026-11-11T08:00:00Z'}),
      );
    }
    throw StateError('Unexpected bridge call: $method');
  }
}

class SyncAndroidHost extends AndroidHost {
  @override
  Future<Map<String, dynamic>> storagePermissionStatus() async => {
    'granted': true,
    'writable': true,
  };
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  late SyncCore api;
  late AppController controller;

  setUp(() {
    api = SyncCore();
    RustLib.initMock(api: api);
    controller = AppController(
      info: const BridgeInfo(
        platform: BridgePlatform.macOs,
        appVersion: 'test',
        userRoot: '/local/rime',
      ),
    );
  });

  tearDown(() {
    controller.dispose();
    RustLib.dispose();
  });

  for (final platform in [BridgePlatform.android, BridgePlatform.ios]) {
    testWidgets(
      '$platform saves and syncs appearance without forcing an accent',
      (tester) async {
        controller.dispose();
        controller = AppController(
          info: BridgeInfo(
            platform: platform,
            appVersion: 'test',
            userRoot: '/local/rime',
          ),
          android: SyncAndroidHost(),
        );
        expect(await controller.saveUi(cornerRadius: 24), isTrue);
        expect(api.writtenAccent, isNull);
        expect(api.savedSettings!['ime_ui'], {
          'color_scheme': 'auto',
          'corner_radius': 24.0,
          'background_color': '',
        });
        expect(await controller.saveUi(accentColor: '#123456'), isTrue);
        expect(api.writtenAccent, '#123456');
        expect(await controller.saveUi(backgroundColor: '#654321'), isTrue);
        expect(api.writtenAccent, isNull);
        expect(api.savedSettings!['ime_ui']['accent_color'], '#123456');
        api.accentOverride = null;
        expect(await controller.saveUi(backgroundColor: '#ABCDEF'), isTrue);
        expect(api.writtenAccent, isNull);
        expect(
          api.savedSettings!['ime_ui'].containsKey('accent_color'),
          isFalse,
        );
        controller.pendingCloudProfile = {
          ...api.profile,
          'settings': {
            'ime_ui': {'corner_radius': 8},
          },
        };
        expect(await controller.applyCloudProfile(), isTrue);
        expect(api.writtenAccent, isNull);
        expect(api.accentOverride, isNull);
        expect(api.radius, 8);
      },
    );
  }

  testWidgets('desktop export keeps the complete IME UI settings', (
    tester,
  ) async {
    expect(await controller.saveUi(cornerRadius: 24), isTrue);
    expect(api.savedSettings!['ime_ui'], {
      'color_scheme': 'auto',
      'orientation': 'vertical',
      'corner_radius': 24.0,
      'background_color': '',
      'font_size': 24.0,
      'embedded_composition': true,
      'hide_candidate_panel': true,
    });
  });

  testWidgets('account sync stays off until enabled', (tester) async {
    await controller.loadSchemeSyncState();
    await controller.reloadAccountSession();
    controller.scheduleSchemeProfileSync(packageKey: 'keytao');
    await tester.pump(const Duration(seconds: 2));
    expect(api.calls.where((call) => call.contains('Upload')), isEmpty);
  });

  testWidgets('integer JSON settings match doubles and allow automatic sync', (
    tester,
  ) async {
    api.enabled = true;
    api.profile['settings'] = {
      'android': {'candidate_font_scale': 1.0, 'swipe_threshold_dp': 34.0},
      'ime_ui': {'font_size': 14.0},
    };
    api.cloudProfile = {
      ...api.profile,
      'settings': {
        'android': {'candidate_font_scale': 1, 'swipe_threshold_dp': 34},
        'ime_ui': {'font_size': 14},
      },
    };
    await controller.loadSchemeSyncState();
    await controller.reloadAccountSession();
    expect(controller.pendingCloudProfile, isNull);
    controller.scheduleSchemeProfileSync();
    await tester.pump(const Duration(seconds: 1));
    await tester.pump();
    expect(
      api.calls.where((call) => call.contains('UploadSchemeProfile')),
      hasLength(1),
    );
  });

  testWidgets('different numeric settings still require a cloud decision', (
    tester,
  ) async {
    api.enabled = true;
    api.profile['settings'] = {
      'android': {'swipe_threshold_dp': 34.0},
    };
    api.cloudProfile = {
      ...api.profile,
      'settings': {
        'android': {'swipe_threshold_dp': 34.5},
      },
    };
    await controller.loadSchemeSyncState();
    await controller.reloadAccountSession();
    expect(controller.pendingCloudProfile, api.cloudProfile);
    controller.scheduleSchemeProfileSync();
    await tester.pump(const Duration(seconds: 2));
    expect(api.calls.where((call) => call.contains('Upload')), isEmpty);
  });

  testWidgets('different cloud profile waits for one user decision', (
    tester,
  ) async {
    api.enabled = true;
    api.cloudProfile = {...api.profile, 'active_scheme': 'xmjd'};
    await controller.loadSchemeSyncState();
    await controller.reloadAccountSession();
    controller.scheduleSchemeProfileSync(packageKey: 'keytao');
    await tester.pump(const Duration(seconds: 2));
    expect(controller.pendingCloudProfile?['active_scheme'], 'xmjd');
    expect(api.calls.where((call) => call.contains('Upload')), isEmpty);
    controller.dismissCloudProfileOffer();
    await controller.reloadAccountSession();
    expect(controller.pendingCloudProfile, isNull);
    expect(
      api.calls.where((call) => call.contains('FetchSchemeProfile')),
      hasLength(1),
    );
    await tester.pump(const Duration(seconds: 1));
    await tester.pump();
  });

  testWidgets('marking a package does not upload before successful deploy', (
    tester,
  ) async {
    api.enabled = true;
    controller.installedPackages = const [
      InstalledPackageDto(
        schemeKey: 'keytao',
        version: '1',
        active: true,
        hasPackageZip: true,
      ),
    ];
    await controller.loadSchemeSyncState();
    await controller.reloadAccountSession();
    controller.scheduleSchemeProfileSync(packageKey: 'keytao');
    controller.scheduleSchemeProfileSync(packageKey: 'keytao');
    await tester.pump(const Duration(seconds: 1));
    await tester.pump();
    expect(api.calls.where((call) => call.contains('Upload')).toList(), [
      'Symbol("crateApiCoreUploadSchemeProfile")',
    ]);
    await tester.pump(const Duration(seconds: 2));
    expect(
      api.calls.where((call) => call.contains('UploadSchemeProfile')),
      hasLength(1),
    );
  });

  testWidgets('queued upload cannot cross into an unchecked account', (
    tester,
  ) async {
    api.enabled = true;
    await controller.loadSchemeSyncState();
    await controller.reloadAccountSession();
    controller.scheduleSchemeProfileSync();
    api.accountId = 2;
    await controller.reloadAccountSession(checkSchemeProfile: false);
    await tester.pump(const Duration(seconds: 2));
    expect(api.calls.where((call) => call.contains('Upload')), isEmpty);
  });

  testWidgets('successful deploy queues only the active marked package once', (
    tester,
  ) async {
    api.enabled = true;
    controller.installedPackages = const [
      InstalledPackageDto(
        schemeKey: 'keytao',
        version: '1',
        active: true,
        hasPackageZip: true,
      ),
    ];
    controller.local = const LocalSchemaDto(
      installed: true,
      deployed: true,
      schemas: ['keytao'],
    );
    await controller.loadSchemeSyncState();
    await controller.reloadAccountSession();
    controller.scheduleSchemeProfileSync(
      packageKey: 'keytao',
      afterSuccessfulDeploy: true,
    );
    controller.scheduleSchemeProfileSync(
      packageKey: 'keytao',
      afterSuccessfulDeploy: true,
    );
    await tester.pump(const Duration(seconds: 1));
    await tester.pump();
    expect(api.calls.where((call) => call.contains('Upload')).toList(), [
      'Symbol("crateApiCoreUploadCloudPackage")',
      'Symbol("crateApiCoreUploadSchemeProfile")',
    ]);
    await tester.pump(const Duration(seconds: 2));
    expect(
      api.calls.where((call) => call.contains('UploadCloudPackage')),
      hasLength(1),
    );
  });
}
