import '../rust/api/types.dart';
import 'bootstrap.dart' show flutterVersion;
import 'controller.dart';

const schemes = {
  'keytao': '键道6',
  'xmjd': '星猫键道',
  'txjx': '天行键',
  'keydo': '键道·我流',
};
const repositoryUrl = 'https://github.com/xkinput/keytao-app';
const downloadSources = {'github': 'GitHub', 'gitee': 'Gitee'};
const colorSchemes = {
  UiColorSchemeDto.auto: '跟随系统',
  UiColorSchemeDto.light: '浅色',
  UiColorSchemeDto.dark: '深色',
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
  RuntimeLogLevelDto.info: '常规',
  RuntimeLogLevelDto.verbose: '详细',
};
const candidateFontMin = 10.0;
const candidateFontMax = 36.0;
const candidateFontDivisions = 26;

enum AppPage {
  input('输入法'),
  about('关于'),
  debug('调试');

  const AppPage(this.title);
  final String title;

  void onSelected(AppController controller) {
    if (this == debug && controller.canLoadInitialLogs) {
      controller.refreshLogs();
    }
  }
}

enum SetupStep {
  storage('文件访问权限'),
  migration('数据迁移'),
  component('输入法组件'),
  install('安装方案'),
  deploy('部署方案'),
  enable('启用 KeyTao'),
  select('切换到 KeyTao'),
  finish('完成');

  const SetupStep(this.title);
  final String title;

  bool isComplete(AppController c) => switch (this) {
    storage => c.storageReady,
    migration => c.migrationReady,
    component => c.macosIme?.installed == true,
    install => c.local?.installed == true,
    deploy => c.local?.deployed == true,
    enable => c.ime?['enabled'] == true,
    select => c.ime?['selected'] == true,
    finish => c.setupReady,
  };

  bool canContinue(AppController c) => !c.busy && isComplete(c);

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
    if (isAndroid) ...[
      SetupStep.storage,
      SetupStep.migration,
    ] else
      SetupStep.component,
    SetupStep.install,
    SetupStep.deploy,
    if (isAndroid) ...[SetupStep.enable, SetupStep.select],
    SetupStep.finish,
  ];

  String get englishSchemaLabel => englishReady ? '英文方案' : '英文方案（未就绪）';
  Map<EnglishModeDto, String> get englishModes => {
    EnglishModeDto.ascii: 'ASCII',
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
  bool get canInstallDuringSetup => !busy && (!isAndroid || migrationReady);
  bool get canDeploy => !busy && local?.installed == true;
  bool get canOpenTheme =>
      !busy && uiSettings?.themeExists == true && uiSettings?.themePath != null;
  bool get canEditLogs => !busy && logSettings != null;
  bool get canLoadInitialLogs => !busy && logSettings == null;
  bool get hasOperationLog =>
      operationLogs.isNotEmpty || verification.isNotEmpty;
  String get installButtonTitle => local?.installed == true ? '重新安装' : '安装方案';
  String get localStatus => local == null
      ? '未读取'
      : '${local!.installed ? '已安装' : '未安装'} · ${local!.deployed ? '已部署' : '未部署'}';
  List<String> get logLines => runtimeLog?.lines ?? const [];
  String? get logLineCount =>
      runtimeLog?.truncated == true ? '最近 ${logLines.length} 行' : null;
  Map<String, String> get aboutValues => {
    'KeyTao': versions?.appVersion ?? info.appVersion,
    'Flutter': flutterVersion,
    'librime': versions?.librimeVersion ?? '—',
    'OpenCC': versions?.openccVersion ?? '—',
    '平台': isAndroid ? 'Android' : 'macOS',
    'KeyTao 目录': versions?.dataDir ?? info.userRoot,
  };
}
