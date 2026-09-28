import 'package:keytao/src/app/controller.dart';
import 'package:keytao/src/rust/api/types.dart';

// View-only fixture: no bootstrap, event subscription, FFI, or external IO.
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
    addon = const AddonSchemaStatusDto(
      installed: true,
      deployed: true,
      version: '2026.09',
      englishAvailable: true,
    );
    wanxiang = const WanxiangStatusDto(
      installed: false,
      deployed: false,
      version: '',
    );
    macosIme = const MacosImeStatusDto(
      installed: true,
      sharedDataSource:
          '/Library/Input Methods/KeyTao.app/Contents/SharedSupport',
      appPath: '/Library/Input Methods/KeyTao.app',
      message: '',
    );
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
      downloadUrls: DownloadUrlsDto(macos: 'offline', android: 'offline'),
    );
    const gitee = PlatformReleaseDto(
      version: '2026.09.26',
      downloadUrls: DownloadUrlsDto(macos: 'offline', android: 'offline'),
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
  @override
  Future<bool> refreshLogs() async {
    logRefreshes++;
    return true;
  }

  @override
  void onResume() {}
}
