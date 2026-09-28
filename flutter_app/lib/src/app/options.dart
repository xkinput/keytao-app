import 'package:flutter/cupertino.dart' show CupertinoIcons;
import 'package:flutter/material.dart' show IconData, Icons;

import '../rust/api/types.dart';
import 'bootstrap.dart' show flutterVersion;
import 'controller.dart';
import 'strings.dart';
export 'strings.dart';

const schemes = {
  'keytao': '键道6',
  'xmjd': '星猫键道',
  'txjx': '天行键',
  'keydo': '键道·我流',
};
const logoAsset = 'assets/logo.png';
const schemeAssets = {
  'keytao': 'keytao-linux',
  'xmjd': 'xmjd6.zip',
  'txjx': 'txjx.zip',
  'keydo': 'nightly zip',
};
const defaultDownloadSource = 'gitee';
const repositoryUrl = 'https://github.com/xkinput/keytao-app';
const downloadSources = {'github': 'GitHub', 'gitee': 'Gitee'};
const colorSchemes = {
  UiColorSchemeDto.auto: AppStrings.auto,
  UiColorSchemeDto.light: AppStrings.light,
  UiColorSchemeDto.dark: AppStrings.dark,
};
const orientations = {
  PanelOrientationDto.horizontal: '横排',
  PanelOrientationDto.vertical: '竖排',
};
const themeAccents = {
  '#3B73D9': '蓝',
  '#0F9F8F': '绿',
  '#D87A32': '橙',
  '#8B5CF6': '紫',
};
const logLevels = {
  RuntimeLogLevelDto.info: 'info',
  RuntimeLogLevelDto.verbose: 'verbose',
};
const candidateFontMin = 10.0;
const candidateFontMax = 36.0;
const candidateFontDivisions = 26;

enum AppPage {
  input(
    'ime',
    AppStrings.ime,
    CupertinoIcons.keyboard,
    Icons.keyboard_outlined,
  ),
  scheme(
    'scheme',
    AppStrings.scheme,
    CupertinoIcons.arrow_down_circle,
    Icons.download_outlined,
  ),
  extension(
    'extension',
    AppStrings.extension,
    CupertinoIcons.settings,
    Icons.settings_outlined,
  ),
  about(
    'about',
    AppStrings.about,
    CupertinoIcons.info_circle,
    Icons.info_outline,
  ),
  debug(
    'debug',
    AppStrings.debug,
    CupertinoIcons.doc_text,
    Icons.receipt_long_outlined,
  );

  const AppPage(this.id, this.title, this.cupertinoIcon, this.materialIcon);
  final String id;
  final String title;
  final IconData cupertinoIcon;
  final IconData materialIcon;
  int get shortcutDigit => index + 1;
}

enum SetupStep {
  storage('文件访问权限'),
  component('输入法组件'),
  install(AppStrings.installScheme),
  deploy(AppStrings.deployScheme),
  enable(AppStrings.enableKeytao),
  fullAccess(AppStrings.fullAccess),
  select('切换到 KeyTao'),
  finish('完成');

  const SetupStep(this.title);
  final String title;

  bool isComplete(AppController c) => switch (this) {
    storage => c.storageReady,
    component => c.macosIme?.installed == true,
    install => c.local?.installed == true,
    deploy => c.local?.deployed == true,
    // iOS exposes no keyboard/Full Access status query. These are guidance
    // pages, not detected system prerequisites; the UI does not mark them done.
    enable => c.isIos || c.ime?['enabled'] == true,
    fullAccess => c.isIos,
    select => c.ime?['selected'] == true,
    finish => c.setupReady,
  };

  Future<void> advance(
    AppController c, {
    required void Function() onNext,
    required void Function() onFinished,
  }) async {
    if (this == finish) {
      if (await c.finishOnboarding()) onFinished();
    } else {
      onNext();
    }
  }
}

// Shared presentation decisions; platform views only arrange these values.
extension AppViewOptions on AppController {
  List<SetupStep> get setupSteps => [
    if (isIos) ...[
      SetupStep.enable,
      SetupStep.fullAccess,
      SetupStep.install,
      SetupStep.deploy,
    ] else ...[
      if (isAndroid)
        SetupStep.storage
      else
        SetupStep.component,
      SetupStep.install,
      SetupStep.deploy,
      if (isAndroid) ...[SetupStep.enable, SetupStep.select],
      SetupStep.finish,
    ],
  ];

  String get englishSchemaLabel => AppStrings.englishSchema;
  Map<EnglishModeDto, String> get englishModes => {
    EnglishModeDto.ascii: AppStrings.englishAscii,
    EnglishModeDto.schema: englishSchemaLabel,
  };
  Map<String, String> get androidEnglishModes => {
    for (final entry in englishModes.entries) entry.key.name: entry.value,
  };
  Set<EnglishModeDto> get disabledEnglishModes =>
      englishReady ? const {} : const {EnglishModeDto.schema};
  Set<String> get disabledAndroidEnglishModes => {
    for (final mode in disabledEnglishModes) mode.name,
  };
  bool get canInstallDuringSetup => canInstall;
  bool get canDeploy => !busy && local?.installed == true;
  bool get canOpenTheme =>
      !busy && uiSettings?.themeExists == true && uiSettings?.themePath != null;
  bool get canEditLogs => !busy && logSettings != null;
  String get installButtonTitle => installButtonLabel;
  Map<String, String> get aboutValues => {
    'KeyTao': versions?.appVersion ?? info.appVersion,
    'Flutter': flutterVersion,
    'librime': versions?.librimeVersion ?? '—',
    'OpenCC': versions?.openccVersion ?? '—',
    '平台': switch (info.platform) {
      BridgePlatform.android => 'Android',
      BridgePlatform.ios => 'iOS',
      BridgePlatform.windows => 'Windows',
      BridgePlatform.linux => 'Linux',
      BridgePlatform.macOs => 'macOS',
    },
    'KeyTao 目录': versions?.dataDir ?? info.userRoot,
  };
}
