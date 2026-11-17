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
const schemeRepositoryUrls = {
  'keytao': 'https://github.com/xkinput/KeyTao',
  'xmjd': 'https://github.com/hugh7007/xmjd6-rere',
  'xmjd-feiding': 'https://github.com/hugh7007/xmjd6-rere',
  'txjx': 'https://github.com/wzxmer/rime-txjx',
  'keydo': 'https://github.com/pingshunhuangalex/rime-keydo',
};
// 星猫键道在同一个 release 里发两个包：顶功版 xmjd6.zip、非顶版 feiding-xmjd6.zip。
// 选中「星猫键道」后再列一次具体文件；键名直接作为后端 scheme 参数，值即 asset 名。
const xmjdVariantSchemes = {
  'xmjd': 'xmjd6.zip',
  'xmjd-feiding': 'feiding-xmjd6.zip',
};
String schemeDisplayName(String key) => switch (key) {
  'xmjd' => '星猫键道 · 顶功',
  'xmjd-feiding' => '星猫键道 · 非顶',
  'easy_en' => 'Easy English',
  'wanxiang' => AppStrings.wanxiang,
  _ => schemes[key] ?? key,
};
const logoAsset = 'assets/logo.png';
const schemeAssets = {
  'keytao': '键道6',
  'xmjd': 'xmjd6.zip',
  'xmjd-feiding': 'feiding-xmjd6.zip',
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
  '#4F46E5': '靛',
  '#0284C7': '天蓝',
  '#16A34A': '草绿',
  '#E5484D': '红',
  '#DB2777': '玫红',
  '#475569': '石墨',
};
const themeBackgrounds = {
  '#1C1D21': '石墨',
  '#1D2230': '夜蓝',
  '#1E2924': '墨绿',
  '#272230': '暗紫',
  '#F6F5F2': '米白',
  '#F1ECE4': '暖灰',
  '#EDF1F6': '雾蓝',
  '#EDF3EE': '浅绿',
};
const logLevels = {
  RuntimeLogLevelDto.info: 'info',
  RuntimeLogLevelDto.verbose: 'verbose',
};
const candidateFontMin = 10.0;
const candidateFontMax = 36.0;
const candidateFontDivisions = 26;

enum AppPage {
  input('ime', AppStrings.ime),
  scheme('scheme', AppStrings.scheme),
  extension('extension', AppStrings.extension),
  about('about', AppStrings.about),
  debug('debug', AppStrings.debug);

  const AppPage(this.id, this.title);
  final String id;
  final String title;
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
    // iOS reads the keyboard list and Full Access recorded by the extension.
    enable => c.isIos ? c.iosKeyboardEnabled : c.ime?['enabled'] == true,
    fullAccess => c.isIos && c.iosKeyboardFullAccess == true,
    select => c.ime?['selected'] == true,
    finish => c.setupReady,
  };
}

// Shared presentation decisions; platform views only arrange these values.
extension AppViewOptions on AppController {
  List<SetupStep> get setupSteps => [
    if (isIos) ...[
      SetupStep.enable,
      SetupStep.fullAccess,
      SetupStep.install,
      SetupStep.deploy,
    ] else if (isAndroid) ...[
      SetupStep.enable,
      SetupStep.select,
      SetupStep.storage,
      SetupStep.install,
      SetupStep.deploy,
    ] else ...[
      SetupStep.component,
      SetupStep.install,
      SetupStep.deploy,
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
