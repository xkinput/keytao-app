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
  message: 'Windows fixture',
);

const linuxStatus = LinuxImeStatusDto(
  kdeSession: true,
  kdeConfigured: true,
  running: true,
  managedPid: 123,
  command: '/usr/lib/KeyTao/keytao-ime',
  kdeNativeProcesses: 1,
  fallbackProcesses: 2,
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
    rimeIce = const AddonStatusDto(
      installed: false,
      deployed: false,
      version: '',
      englishAvailable: false,
    );
    macosIme = const MacosImeStatusDto(
      installed: true,
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
      hideCandidatePanel: false,
      showTrayModeIndicator: true,
      effectiveEmbeddedComposition: false,
      accentColor: '#3B73D9',
      cornerRadius: null,
      backgroundColor: '',
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
      symbolAutoReturn: false,
      phrases: const ['谢谢', '好的', '收到', '稍等', '我在路上'],
      clipboardHistoryEnabled: true,
      clipboardHistoryLimit: 50,
      clipboardSuggestionEnabled: true,
      clipboardSuggestionTimeoutSec: 30,
      clipboardReturnAfterPaste: true,
      clipboardHideSensitive: true,
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
    double? cornerRadius,
    String backgroundColor = '',
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
    final accent = int.parse(accentColor.substring(1, 7), radix: 16);
    theme['ui']['accentColor'] = {
      'red': (accent >> 16) & 255,
      'green': (accent >> 8) & 255,
      'blue': accent & 255,
      'alpha': 255,
    };
    if (cornerRadius != null) {
      final radius = cornerRadius.clamp(0, 28);
      for (final entry in {
        'panel': radius,
        'candidate': radius * 11 / 16,
        'navigation': radius * 10 / 16,
        'modeHint': radius * 14 / 16,
      }.entries) {
        (theme[entry.key] as Map<String, dynamic>)['cornerRadius'] =
            entry.value;
      }
    }
    if (backgroundColor.isNotEmpty) {
      final rgb = int.parse(backgroundColor.substring(1, 7), radix: 16);
      final background = theme['panel']['background'] as Map<String, dynamic>;
      background['red'] = (rgb >> 16) & 255;
      background['green'] = (rgb >> 8) & 255;
      background['blue'] = rgb & 255;
    }
    final mobileEdits =
        _candidatePanelThemes['mobileEdits'][dark ? 'dark' : 'light']
            as Map<String, dynamic>;
    if (backgroundColor == '#102030') {
      theme['mobile'] = jsonDecode(jsonEncode(mobileEdits['background']));
    }
    if (cornerRadius != null) {
      final radius = mobileEdits['radii']['${cornerRadius.round()}'];
      if (radius != null) {
        theme['mobile']['keyboard']['key']['cornerRadius'] = radius;
      }
    }
    final scale = fontSize / 20;
    final font = theme['font'] as Map<String, dynamic>;
    font['size'] = fontSize;
    for (final field in ['labelSize', 'commentSize', 'preeditSize']) {
      font[field] = (font[field] as num) * scale;
    }
    for (final entry in {
      'panel': [
        'paddingX',
        'paddingY',
        'paddingTop',
        'paddingRight',
        'paddingBottom',
        'paddingLeft',
        'gap',
        'preeditGap',
      ],
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
  Future<Map<String, dynamic>> getThemeEditor() async {
    final platform = switch (info.platform) {
      BridgePlatform.macOs => 'macos',
      BridgePlatform.windows => 'windows',
      BridgePlatform.linux => 'linux',
      BridgePlatform.android => 'android',
      BridgePlatform.ios => 'ios',
    };
    final settings = uiSettings;
    final source = <String, dynamic>{
      'version': 3,
      'meta': {'id': 'my.fixture', 'name': 'Fixture'},
      'common': {
        'accentColor': settings?.accentColor ?? '#3B73D9',
        'cornerRadius': ?settings?.cornerRadius,
        if (settings?.backgroundColor case final color? when color.isNotEmpty)
          'backgroundColor': color,
      },
      'pc': {
        'font': {'size': settings?.fontSize ?? 20},
        'panel': {'orientation': settings?.orientation.name ?? 'horizontal'},
      },
    };
    return {
      'source': source,
      'resolvedLight': resolveThemeEditorPreview(source: source, dark: false),
      'resolvedDark': resolveThemeEditorPreview(source: source, dark: true),
      'colorScheme': settings?.colorScheme.name ?? 'auto',
      'capabilities': _candidatePanelThemes['editorCapabilities'][platform],
    };
  }

  @override
  Map<String, dynamic> resolveThemeEditorPreview({
    required Map<String, dynamic> source,
    required bool dark,
  }) {
    final variant = source[dark ? 'dark' : 'light'] as Map<String, dynamic>?;
    Object? field(String section, String name) =>
        (variant?[section] as Map<String, dynamic>?)?[name] ??
        (source[section] as Map<String, dynamic>?)?[name];
    Object? pcField(String section, String name) =>
        variant?['pc']?[section]?[name] ?? source['pc']?[section]?[name];
    final theme = resolveCandidatePanelPreview(
      colorScheme: dark ? UiColorSchemeDto.dark : UiColorSchemeDto.light,
      accentColor: field('common', 'accentColor') as String? ?? '#3B73D9',
      cornerRadius: (field('common', 'cornerRadius') as num?)?.toDouble(),
      backgroundColor: field('common', 'backgroundColor') as String? ?? '',
      fontSize: (pcField('font', 'size') as num?)?.toDouble() ?? 20,
      systemDark: dark,
    );
    theme['panel']['orientation'] =
        pcField('panel', 'orientation') ?? 'horizontal';
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
