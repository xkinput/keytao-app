import 'dart:convert';
import 'dart:io';

import 'package:keytao/src/app/controller.dart';
import 'package:keytao/src/rust/api/core.dart' show AddonStatusDto;
import 'package:keytao/src/rust/api/types.dart';

WindowsImeStatusDto windowsStatus({
  String state = 'registered',
  bool busy = false,
  bool registered = true,
  String? error,
}) => WindowsImeStatusDto(
  supported: true,
  packaged: true,
  registered: registered,
  registeredDll: registered,
  profileEnabled: registered,
  registrationBusy: busy,
  registrationState: state,
  registrationError: error,
  runtimeDir: r'C:\ProgramData\KeyTao\runtime',
  registeredPath: r'C:\ProgramData\KeyTao\runtime\keytao_windows_ime.dll',
  profileStatus: 'enabled',
  sharedDataSource: 'packaged',
  message: 'Windows fixture',
);

const linuxStatus = LinuxImeStatusDto(
  supported: true,
  kdeSession: true,
  kdeConfigured: true,
  running: true,
  managedPid: 123,
  command: '/usr/lib/KeyTao/keytao-ime',
  processes: [],
  kdeNativeProcesses: 1,
  fallbackProcesses: 2,
  sharedDataSource: 'packaged',
  message: 'Linux fixture',
);

// Captured from the Rust preview resolver at 20pt with the default accent.
final _candidatePanelThemes = jsonDecode(
  File('test/fixtures/candidate_panel_themes.json').readAsStringSync(),
) as Map<String, dynamic>;

// View-only fixture: local theme data, no bootstrap, events, network, or FFI.
class ViewController extends AppController {
  ViewController(BridgePlatform platform)
    : super(
        info: BridgeInfo(
          platform: platform,
          appVersion: '1.2.1-alpha.42',
          userRoot: platform == BridgePlatform.android
              ? '/storage/emulated/0/rime'
              : '/Users/rea/Library/Rime',
        ),
      ) {
    local = const LocalSchemaDto(
      installed: true,
      deployed: true,
      version: '2026.09.26',
      schemas: ['keytao', 'english'],
    );
    addon = const AddonStatusDto(
      installed: true,
      deployed: true,
      version: '2026.09',
      englishAvailable: true,
    );
    wanxiang = const AddonStatusDto(
      installed: false,
      deployed: false,
      version: '',
      englishAvailable: false,
    );
    macosIme = const MacosImeStatusDto(
      installed: true,
      sharedDataSource:
          '/Library/Input Methods/KeyTao.app/Contents/SharedSupport',
      appPath: '/Library/Input Methods/KeyTao.app',
      message: '',
    );
    windowsIme = windowsStatus();
    linuxIme = linuxStatus;
    ime = {'enabled': true, 'selected': true, 'canShowPicker': true};
    storage = {
      'granted': true,
      'writable': true,
      'canOpenSettings': true,
      'path': defaultDir,
    };
    uiSettings = ImeUiSettingsDto(
      colorScheme: UiColorSchemeDto.auto,
      effectiveColorScheme: EffectiveColorSchemeDto.light,
      orientation: PanelOrientationDto.horizontal,
      embeddedComposition: false,
      accentColor: '#3B73D9',
      fontSize: 18,
      themeExists: true,
      themePath: '$defaultDir/keytao/theme.yaml',
      message: '',
    );
    androidSettings = AndroidImeInputSettingsDto(
      hapticsEnabled: true,
      hapticIntensity: 42,
      enterKeyBehavior: 'system',
      keyPreviewEnabled: true,
      longPressDelayMs: 300,
      keyboardHeightScale: 100,
      deleteSpeed: 'standard',
      englishMode: 'ascii',
      backspaceGestureMode: 'immediate',
      keyboardHeightDp: 266,
      candidateBarHeightDp: 52,
      swipeThresholdDp: 34,
      keySoundEnabled: true,
      keySoundVolume: 100,
      keyHintVisible: true,
      flickKeysEnabled: true,
      numberRowEnabled: false,
      candidateFontScale: 1,
      doubleSpacePeriodEnabled: true,
      floatingPortraitEnabled: false,
      floatingPortraitScale: 88,
      floatingLandscapeEnabled: true,
      floatingLandscapeScale: 72,
      configPath: '$defaultDir/keytao/keyboard.yaml',
      message: '',
    );
    const github = PlatformReleaseDto(
      version: '2026.09.28',
      downloadUrls: DownloadUrlsDto(
        macos: 'offline',
        android: 'offline',
        windows: 'offline',
        linux: 'offline',
        ios: 'offline',
      ),
    );
    const gitee = PlatformReleaseDto(
      version: '2026.09.26',
      downloadUrls: DownloadUrlsDto(
        macos: 'offline',
        android: 'offline',
        windows: 'offline',
        linux: 'offline',
        ios: 'offline',
      ),
    );
    latestRelease = const ReleaseInfoDto(
      version: '2026.09.28',
      name: '键道6 2026.09.28',
      publishedAt: '',
      body: '更新词库\n修复候选词排序\n改善英文方案兼容性',
      github: github,
      gitee: gitee,
    );
    appUpdate = const AppUpdateDto(
      hasUpdate: true,
      latestVersion: '1.2.1-alpha.43',
      currentVersion: '1.2.1-alpha.42',
      releaseUrl: 'https://example.invalid/release',
    );
    versions = ComponentVersionsDto(
      appVersion: info.appVersion,
      librimeVersion: '1.14.0',
      openccVersion: '1.1.9',
      dataDir: defaultDir,
    );
    selectedDirectory = isAndroid ? '主存储空间/Rime' : '/Users/rea/Documents/Rime';
    selectedTreeUri = isAndroid
        ? 'content://com.android.externalstorage.documents/tree/primary%3ARime'
        : null;
    customSchemas = const LocalSchemasDto(
      hasDefaultCustom: true,
      schemas: ['keytao', 'english'],
    );
    customFiles = const [
      FileItemDto(name: 'build', isDir: true),
      FileItemDto(name: 'default.custom.yaml', isDir: false),
      FileItemDto(name: 'keytao.schema.yaml', isDir: false),
      FileItemDto(name: 'keytao.dict.yaml', isDir: false),
    ];
    customResult = const InstallResultDto(
      mergedSchemas: ['keytao'],
      logs: ['[MERGED] default.custom.yaml', '安装完成'],
      verify: [],
    );
    operationLogs.addAll([
      '[14:21:03] [MERGED] default.custom.yaml',
      '[14:21:05] [DEPLOY] 部署完成',
    ]);
    deploySteps.add(const DeployStep('部署完成', DeployStepState.ok));
    logSettings = RuntimeLogSettingsDto(
      enabled: true,
      level: RuntimeLogLevelDto.info,
      logDir: '$defaultDir/keytao/logs',
      files: [
        RuntimeLogFileInfoDto(
          name: 'keytao-runtime.log',
          size: BigInt.from(24576),
        ),
        RuntimeLogFileInfoDto(name: 'keytao-ime.log', size: BigInt.from(8192)),
      ],
      fileCount: 2,
      totalBytes: BigInt.from(32768),
    );
    runtimeLog = const DebugLogFileDto(
      lines: [
        '2026-09-28 14:21:01 [INFO] 输入法已启动',
        '2026-09-28 14:21:03 [INFO] 方案 keytao 已加载',
        '2026-09-28 14:21:05 [INFO] 部署完成',
      ],
      truncated: false,
    );
    systemLogs = const DebugLogsDto(
      ime: DebugLogFileDto(
        lines: ['[INFO] KeyTao input method ready'],
        truncated: false,
      ),
      app: DebugLogFileDto(lines: ['[INFO] Settings loaded'], truncated: false),
    );
  }
  int logRefreshes = 0;
  String? previewAccent;

  @override
  Map<String, dynamic> resolveCandidatePanelPreview({
    required UiColorSchemeDto colorScheme,
    required String accentColor,
    required double fontSize,
    required bool systemDark,
  }) {
    previewAccent = accentColor;
    final dark =
        colorScheme == UiColorSchemeDto.dark ||
        (colorScheme == UiColorSchemeDto.auto && systemDark);
    final theme = jsonDecode(
      jsonEncode(_candidatePanelThemes[dark ? 'dark' : 'light']),
    ) as Map<String, dynamic>;
    (theme['ui'] as Map<String, dynamic>)['colorScheme'] = colorScheme.name;
    final scale = fontSize / 20;
    final font = theme['font'] as Map<String, dynamic>;
    font['size'] = fontSize;
    for (final field in ['labelSize', 'commentSize', 'preeditSize']) {
      font[field] = (font[field] as num) * scale;
    }
    for (final entry in {
      'panel': ['paddingX', 'paddingY', 'gap', 'preeditGap'],
      'candidate': ['paddingX', 'paddingY', 'inlineGap', 'minHeight'],
      'navigation': ['buttonSize'],
    }.entries) {
      final section = theme[entry.key] as Map<String, dynamic>;
      for (final field in entry.value) {
        section[field] = (section[field] as num) * scale;
      }
    }
    return theme;
  }

  @override
  Future<bool> refreshLogs() async {
    logRefreshes++;
    return true;
  }

  @override
  void onResume() {}
}
