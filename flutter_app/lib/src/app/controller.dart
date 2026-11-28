import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:ui' show Locale, PlatformDispatcher;

import 'package:file_selector/file_selector.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import '../platform/android_host.dart';
import '../platform/ios_host.dart';
import '../rust/api/core.dart' as core;
import '../rust/api/timing.dart' show AppOperationName;
import '../rust/api/types.dart';
import '../settings/android_settings.dart';
import 'operation_timing.dart';
import 'options.dart'
    show
        AppPage,
        defaultDownloadSource,
        keytaoWebsiteOrigins,
        schemeAssets,
        schemeDisplayName,
        schemeRepositoryUrls,
        schemes,
        xmjdVariantSchemes;
import 'strings.dart';

enum OperationKind {
  install,
  migrate,
  libjd,
  deploy,
  addon,
  wanxiang,
  wanxiangGrammar,
  rimeIce,
  rimeMint,
  rimeFrost,
  rimeLssp,
}

class RimeAddonSlot {
  RimeAddonSlot._(
    this.operation,
    this.title,
    this.tooltip,
    this._installedMessage,
    this._removedMessage,
    this._readStatus,
    this._manage,
  );

  final OperationKind operation;
  final String title;
  final String tooltip;
  final String _installedMessage;
  final String _removedMessage;
  final Future<core.AddonStatusDto> Function() _readStatus;
  final Future<core.AddonStatusDto> Function({
    required bool installed,
    required FutureOr<String> Function() androidDeploy,
  })
  _manage;
  core.AddonStatusDto? status;
  String? error;
  InstallProgressDto? progress;
  String? latestVersion;

  bool get installed => status?.installed == true;
  bool get hasUpdate =>
      installed &&
      latestVersion != null &&
      AppController._normalizedVersion(latestVersion!) !=
          AppController._normalizedVersion(status!.version);
  bool get showInstall => !installed || status?.deployed != true || hasUpdate;
  String get buttonLabel => hasUpdate
      ? AppStrings.update
      : installed
      ? AppStrings.deploy
      : AppStrings.download;
}

enum LogLineKind { normal, error, warning, deploy, merged }

enum DeployStepState { neutral, ok, failed }

enum AppUpdateAction { download, openPackage, restart, openLink }

enum ImeBadgeState {
  checking,
  componentMissing,
  schemeMissing,
  pendingDeploy,
  ready,
}

class DeployStep {
  const DeployStep(this.message, this.state, {this.packageKey, this.error});
  final String message;
  final DeployStepState state;
  final String? packageKey;
  final String? error;
}

LogLineKind operationLogKind(String line) {
  if (line.contains('[DEPLOY ERROR]') || line.contains('[ERROR]')) {
    return LogLineKind.error;
  }
  if (line.contains('[WARN]')) return LogLineKind.warning;
  if (line.contains('[DEPLOY]')) return LogLineKind.deploy;
  if (line.contains('[MERGED]') || line.contains('[RENAMED]')) {
    return LogLineKind.merged;
  }
  return LogLineKind.normal;
}

({String? time, String message}) parseOperationLog(String line) {
  final match = RegExp(
    r'^\[((?:[01]\d|2[0-3]):[0-5]\d)(?::[0-5]\d)?\]\s+\[[^\]\r\n]+\]\s+(.+)$',
  ).firstMatch(line);
  return match == null
      ? (time: null, message: line)
      : (time: match[1], message: match[2]!);
}

String formatLogBytes(BigInt bytes) {
  if (bytes < BigInt.from(1024)) return '$bytes B';
  if (bytes < BigInt.from(1024 * 1024)) {
    return '${(bytes.toDouble() / 1024).toStringAsFixed(1)} KB';
  }
  return '${(bytes.toDouble() / (1024 * 1024)).toStringAsFixed(1)} MB';
}

String? schemeKeyFromSchemas(List<String> schemas) {
  for (final schema in schemas) {
    final name = schema.trim().toLowerCase();
    for (final entry in const {
      'xmjd6': 'xmjd',
      'txjx': 'txjx',
      'keydo': 'keydo',
      'snow_jiandao': 'snow_jiandao',
      'keytao': 'keytao',
    }.entries) {
      if (name.startsWith(entry.key)) return entry.value;
    }
  }
  return null;
}

String? installedSchemeKey(List<String> schemas, String? installedScheme) {
  if (installedScheme?.startsWith('local-') == true) return installedScheme;
  final family = schemeKeyFromSchemas(schemas);
  return family == 'xmjd' && installedScheme == 'xmjd-feiding'
      ? 'xmjd-feiding'
      : family;
}

String schemeForFamily(String family, String? installed) =>
    installed == 'xmjd-feiding' && family == 'xmjd' ? 'xmjd-feiding' : family;

String? displayedSchemeVersion(
  List<String> schemas,
  String? fileVersion,
  String? installedScheme,
  String? installedVersion,
) {
  if (installedScheme?.startsWith('local-') == true) return installedVersion;
  final family = schemeKeyFromSchemas(schemas);
  if (family == 'keytao') return fileVersion;
  final installedFamily = installedScheme == 'xmjd-feiding'
      ? 'xmjd'
      : installedScheme;
  return family != null && installedFamily == family ? installedVersion : null;
}

String errorText(Object error) => error is PlatformException
    ? error.message ?? AppStrings.systemFailure
    : error is StateError
    ? error.message
    : '$error';

String formatWindowTitle(String pageTitle, Locale locale) =>
    '$pageTitle · KeyTao ${locale.languageCode.startsWith('zh') ? '输入法' : 'Input Method'}';

typedef PendingSchemaSelectionPrompt = ({
  String packageKey,
  bool activateOnConfirm,
  bool convert,
});

class AppController extends ChangeNotifier {
  AppController({
    required this.info,
    this.android = const AndroidHost(),
    this.pickDesktopDirectory = getDirectoryPath,
    this.pickDesktopArchive = _pickDesktopZip,
    this.runDesktopOpen = Process.run,
  });
  final BridgeInfo info;
  final AndroidHost android;
  final Future<String?> Function() pickDesktopDirectory;
  final Future<String?> Function() pickDesktopArchive;
  final Future<ProcessResult> Function(String, List<String>) runDesktopOpen;

  static Future<String?> _pickDesktopZip() async {
    final file = await openFile(
      acceptedTypeGroups: const [
        XTypeGroup(
          label: 'Rime 方案压缩包',
          extensions: ['zip'],
          uniformTypeIdentifiers: ['public.zip-archive'],
          mimeTypes: ['application/zip', 'application/x-zip-compressed'],
        ),
      ],
    );
    return file?.path;
  }

  bool get isAndroid => info.platform == BridgePlatform.android;
  bool get isWindows => info.platform == BridgePlatform.windows;
  bool get isLinux => info.platform == BridgePlatform.linux;
  bool get isIos => info.platform == BridgePlatform.ios;
  bool get isMobile => isAndroid || isIos;
  bool get hasOnboarding => isMobile || info.platform == BridgePlatform.macOs;
  bool get showsEmbeddedComposition => !isMobile;
  bool _busy = false;
  bool _installingAppUpdate = false;
  bool _switchingInput = false;
  bool get isSwitchingInput => _switchingInput;
  bool get busy =>
      busyWithoutPendingUserWordDeletes || _pendingUserWordDeletes.isNotEmpty;
  bool get busyWithoutPendingUserWordDeletes =>
      _busy ||
      windowsRegistrationBusy ||
      _installingAppUpdate ||
      (isMobile && savingTheme);
  set busy(bool value) => _busy = value;
  bool _platformStarted = false;
  bool _windowsActionRequested = false;
  bool _windowsActionInFlight = false;
  bool _managingWindowsIme = false;
  static const _windowsChannel = MethodChannel('keytao/windows');
  bool _disposed = false;
  bool _refreshSchemasWhenIdle = false;
  bool _resumeRefreshInFlight = false;
  bool _refreshOnResumeWhenIdle = false;
  bool _manuallySelectedScheme = false;
  bool _startupStarted = false;
  bool _packageMigrationFailed = false;
  bool get packageMigrationFailed => _packageMigrationFailed;
  bool _migrationDuringOperation = false;
  String? migrationBackupDir;
  String? _pendingMigrationFailureToast;
  bool _migrationFailureToastShown = false;
  bool get hasPendingMigrationFailureToast =>
      _pendingMigrationFailureToast != null;
  String? takeMigrationFailureToast() {
    final message = _pendingMigrationFailureToast;
    _pendingMigrationFailureToast = null;
    return message;
  }

  static const libjdRepositoryUrl = 'https://github.com/hronro/ime-jd';
  String? _pendingToast;
  bool get hasPendingToast => _pendingToast != null;
  String? takePendingToast() {
    final message = _pendingToast;
    _pendingToast = null;
    return message;
  }

  void showToast(String message) {
    _pendingToast = message;
    _changed();
  }

  PendingSchemaSelectionPrompt? _pendingSchemaSelectionPrompt;
  bool get hasPendingSchemaSelectionPrompt =>
      _pendingSchemaSelectionPrompt != null;
  PendingSchemaSelectionPrompt? takePendingSchemaSelectionPrompt() {
    final prompt = _pendingSchemaSelectionPrompt;
    _pendingSchemaSelectionPrompt = null;
    return prompt;
  }

  void requestSchemaSelectionPrompt(
    String packageKey, {
    bool activateOnConfirm = false,
    bool convert = false,
  }) {
    _pendingSchemaSelectionPrompt = (
      packageKey: packageKey,
      activateOnConfirm: activateOnConfirm,
      convert: convert,
    );
    _changed();
  }

  Future<bool> _deferDeployForSchemaSelection(
    String key, {
    required bool activateOnConfirm,
    bool convert = false,
  }) async {
    final schemas = await packageSchemas(key);
    if (schemas.length <= 2) return false;
    _pendingSchemaSelectionPrompt = (
      packageKey: key,
      activateOnConfirm: activateOnConfirm,
      convert: convert,
    );
    return true;
  }

  Future<void> _finishAddonDownload(
    String key,
    core.AddonStatusDto status,
  ) async {
    if (!status.installed) return;
    if (prefersLibjd) {
      if (!await _deferDeployForSchemaSelection(
        key,
        activateOnConfirm: false,
        convert: true,
      )) {
        await _convertSchemePackage(key);
      }
    } else if (!status.deployed) {
      await _deferDeployForSchemaSelection(key, activateOnConfirm: false);
    }
  }

  void _recordMigrationFailure(String message) {
    _packageMigrationFailed = true;
    if (_migrationFailureToastShown) return;
    _migrationFailureToastShown = true;
    _pendingMigrationFailureToast = message;
  }

  final Set<String> _iosUpgradeDeploymentAttempts = {};
  bool _editingLogs = false;
  bool get canEditLogs => !logsLoading && !_editingLogs;
  bool _savingWebsiteSettings = false;
  bool get canEditWebsiteSettings => !_savingWebsiteSettings;
  bool savingTheme = false;
  bool get canSaveTheme =>
      !savingTheme &&
      !_applyingCloudProfile &&
      !_installingAppUpdate &&
      (!isMobile || !busy);
  int _releaseRequest = 0;
  int _schemeUpdatesRequest = 0;
  AppPage activePage = AppPage.input;
  late String websiteOrigin = keytaoWebsiteOrigins.contains(info.websiteOrigin)
      ? info.websiteOrigin!
      : keytaoWebsiteOrigins.first;
  late final _websiteHidden = ValueNotifier(info.websiteHidden);
  ValueListenable<bool> get websiteHiddenChanges => _websiteHidden;
  bool get websiteHidden => _websiteHidden.value;
  String? _pendingWebsitePath;
  int websiteNavigationRevision = 0;

  String? takePendingWebsitePath() {
    final path = _pendingWebsitePath;
    _pendingWebsitePath = null;
    return path;
  }

  Future<void> openWebsitePath(String path) async {
    if (_disposed) return;
    final normalized = path.startsWith('/') ? path : '/$path';
    if (websiteHidden) {
      final unhidden = await saveWebsiteSettings(hidden: false);
      if (_disposed) return;
      if (!unhidden || websiteHidden) {
        open(Uri.parse(websiteOrigin).replace(path: normalized).toString());
        return;
      }
    }
    _pendingWebsitePath = normalized;
    websiteNavigationRevision++;
    if (activePage == AppPage.website) {
      _changed();
    } else {
      selectPage(AppPage.website);
    }
  }

  late final _reduceMotion = ValueNotifier(info.reduceMotion);
  ValueListenable<bool> get reduceMotionChanges => _reduceMotion;
  bool get reduceMotion => _reduceMotion.value;
  bool _savingReduceMotion = false;
  bool get canSaveReduceMotion => !_savingReduceMotion;
  List<AppPage> get visiblePages => [
    for (final page in AppPage.values)
      if (!(info.platform == BridgePlatform.macOs && page == AppPage.scheme) &&
          !(websiteHidden && page == AppPage.website))
        page,
  ];
  OperationKind? operation;
  bool checkingStorage = false;
  bool releaseLoading = false;
  bool logsLoading = false;
  bool onboardingCompleted = false;
  late bool _appUpdatePrereleases = info.appUpdatePrereleases;
  bool get appUpdatePrereleases => _appUpdatePrereleases;
  AppUpdateDto? appUpdate;
  bool checkingAppUpdate = false;
  String? appUpdateError;
  bool appUpdateBusy = false;
  InstallProgressDto? appUpdateProgress;
  String? appUpdateActionError;
  String? _appUpdateDownloadedVersion;
  String? _appUpdateMissingPackageVersion;
  String _androidAppUpdateSource = 'link';

  AppUpdateAction get appUpdateAction {
    final update = appUpdate;
    if (update == null ||
        (update.installMode != 'package' && update.installMode != 'restart') ||
        (isAndroid && _androidAppUpdateSource != 'package') ||
        _appUpdateMissingPackageVersion == update.latestVersion) {
      return AppUpdateAction.openLink;
    }
    if (_appUpdateDownloadedVersion != update.latestVersion) {
      return AppUpdateAction.download;
    }
    return update.installMode == 'restart'
        ? AppUpdateAction.restart
        : AppUpdateAction.openPackage;
  }

  bool get canChangeAppUpdateChannel => !appUpdateBusy && !checkingAppUpdate;
  bool get canDownloadAppUpdate => canChangeAppUpdateChannel;
  bool get canRunAppUpdate =>
      canChangeAppUpdateChannel &&
      ((!busy && !savingTheme) ||
          appUpdateAction == AppUpdateAction.download ||
          appUpdateAction == AppUpdateAction.openLink);

  List<InstalledPackageDto> installedPackages = [];
  List<IsolatedPackageDto> isolatedPackages = [];
  int _isolatedPackagesRequest = 0;
  List<SchemeUpdateDto> schemeUpdates = [];
  Map<String, String> dismissedSchemeUpdates = {};
  bool schemeUpdatesLoading = false;
  final Set<String> userWordsPendingDeploy = {};
  int userWordsRevision = 0;
  final Set<String> _pendingUserWordDeletes = {};
  String? userWordsReportScheme;
  UserWordsReportDto? userWordsReport;
  String? userWordsReportError;
  String? packageError;
  String? releaseError;
  String? installError;
  String? deployWarning;
  String? addonError;
  String? wanxiangError;
  String? wanxiangGrammarError;
  String? macosImeError;
  String? windowsImeError;
  String? linuxImeError;
  String? imeUiError;
  String? debugError;
  String? debugNotice;
  RimeSyncSettingsDto? rimeSyncSettings;
  bool rimeSyncLoading = false;
  bool rimeSyncing = false;
  String? rimeSyncError;
  InstallProgressDto? installProgress;
  String? installingSchemeKey;
  InstallProgressDto? addonProgress;
  InstallProgressDto? wanxiangProgress;
  InstallProgressDto? wanxiangGrammarProgress;
  final List<DeployStep> deploySteps = [];
  final _packageDeployProgress = <String, DeployProgressDto?>{};
  LibjdStatusDto? libjd;
  String inputEngine = 'rime';
  bool inputEngineEnabled = true;
  bool? inputEngineTransition;
  core.AddonStatusDto? wanxiang;
  core.WanxiangGrammarStatusDto? wanxiangGrammar;
  String? _selectedWanxiangVariant;
  String? wanxiangLatestVersion;
  final rimeAddons = Map<String, RimeAddonSlot>.unmodifiable({
    'rime_ice': RimeAddonSlot._(
      OperationKind.rimeIce,
      AppStrings.rimeIce,
      AppStrings.installRimeIceTooltip,
      AppStrings.rimeIceInstalled,
      AppStrings.rimeIceRemoved,
      core.rimeIceStatus,
      core.manageRimeIce,
    ),
    'rime_mint': RimeAddonSlot._(
      OperationKind.rimeMint,
      AppStrings.rimeMint,
      AppStrings.installRimeMintTooltip,
      AppStrings.rimeMintInstalled,
      AppStrings.rimeMintRemoved,
      core.rimeMintStatus,
      core.manageRimeMint,
    ),
    'rime_frost': RimeAddonSlot._(
      OperationKind.rimeFrost,
      AppStrings.rimeFrost,
      AppStrings.installRimeFrostTooltip,
      AppStrings.rimeFrostInstalled,
      AppStrings.rimeFrostRemoved,
      core.rimeFrostStatus,
      core.manageRimeFrost,
    ),
    'triple_pinyin_lssp': RimeAddonSlot._(
      OperationKind.rimeLssp,
      AppStrings.rimeLssp,
      AppStrings.installRimeLsspTooltip,
      AppStrings.rimeLsspInstalled,
      AppStrings.rimeLsspRemoved,
      core.rimeLsspStatus,
      core.manageRimeLssp,
    ),
  });
  String? addonLatestVersion;
  final _addonUpdateChecks = <String, Object>{};
  DebugLogsDto? systemLogs;
  String? error;
  String progress = '';
  double? installFraction;
  final List<String> operationLogs = [];
  List<VerifyEntryDto> verification = [];
  LocalSchemaDto? local;
  String? activeSchemaLogoPath;
  String? _localSchemaError;
  core.AddonStatusDto? addon;
  Map<String, dynamic>? storage;
  Map<String, dynamic>? ime;
  bool iosKeyboardEnabled = false;
  bool? iosKeyboardFullAccess;
  MacosImeStatusDto? macosIme;
  WindowsImeStatusDto? windowsIme;
  LinuxImeStatusDto? linuxIme;
  AndroidImeInputSettingsDto? androidSettings;
  List<KeyboardSummaryDto> keyboards = [];
  KeyboardSummaryDto? recommendedKeyboard;
  KeyboardSummaryDto? pendingRecommendedKeyboard;
  ImeUiSettingsDto? uiSettings;
  int? candidatePageSize;
  List<({String text, String comment})> candidatePreviewCandidates = [];
  int? candidatePreviewPageSize;
  int _candidatePreviewRequest = 0;
  Future<void>? _candidatePreviewRefresh;
  (String, String, String?)? _candidatePreviewTarget;
  List<SchemaSwitchDto> schemaSwitches = [];
  String? schemaSwitchError;
  int _schemaSwitchesRequest = 0;
  List<SchemaDto> schemas = [];
  String? currentSchemaId;
  String? deployingSchemeKey;

  bool get hasPackageDeployProgress => _packageDeployProgress.isNotEmpty;
  bool get hasUsableBuild =>
      local?.deployed == true || local?.buildAvailable == true;
  String? _attemptedAutoDeployKey;
  String? _attemptedAutoConvertKey;
  bool get _activeBuildHasChanges =>
      !prefersLibjd &&
      !packageMigrationFailed &&
      _localSchemaError == null &&
      local?.installed == true &&
      local?.buildAvailable == true &&
      local?.deployed == false;
  bool get _activeLibjdNeedsAutoConvert =>
      prefersLibjd &&
      !isUsingLibjd &&
      !packageMigrationFailed &&
      _localSchemaError == null &&
      (installedPackageFor(activeSchemeKey) != null ||
          isolatedPackages.any((p) => p.packageKey == activeSchemeKey));
  bool get autoDeployQueued =>
      _activeBuildHasChanges && _attemptedAutoDeployKey != activeSchemeKey;

  void _deployChangedScheme() {
    if (_disposed || busy || _resumeRefreshInFlight) return;
    if (_activeLibjdNeedsAutoConvert &&
        _attemptedAutoConvertKey != activeSchemeKey) {
      final key = activeSchemeKey;
      _attemptedAutoConvertKey = key;
      unawaited(
        run(() async {
          packageError = null;
          await _convertSchemePackage(key);
        }),
      );
      return;
    }
    if (!_activeLibjdNeedsAutoConvert) {
      _attemptedAutoConvertKey = null;
    }
    if (!_activeBuildHasChanges) {
      _attemptedAutoDeployKey = null;
      return;
    }
    if (!autoDeployQueued || hasPendingSchemaSelectionPrompt) return;
    unawaited(deploy(schemeKey: activeSchemeKey));
  }

  bool get activeSchemeDownloaded =>
      local?.installed == true ||
      installedPackageFor(activeSchemeKey)?.hasPackageZip == true ||
      isolatedPackages.any((package) => package.active);

  String? packageDeployLabel(String key) {
    final packageKey = _packageDeployKey(key);
    if (!busy || !_packageDeployProgress.containsKey(packageKey)) return null;
    return switch (_packageDeployProgress[packageKey]?.step) {
      null => AppStrings.packageDeployQueued,
      DeployStepDto.materialize ||
      DeployStepDto.compile ||
      DeployStepDto.validate ||
      DeployStepDto.prune => AppStrings.packageDeploying,
      DeployStepDto.skip || DeployStepDto.done || DeployStepDto.failed => null,
    };
  }

  String? packageDeployError(String key) {
    final package =
        deploySteps.where((step) => step.packageKey == key).firstOrNull ??
        deploySteps
            .where((step) => step.packageKey == 'builtin/$key')
            .firstOrNull;
    return package?.error;
  }

  String _packageDeployKey(String key) =>
      _packageDeployProgress.containsKey(key) ? key : 'builtin/$key';

  String? get currentSchemaName =>
      schemas.where((schema) => schema.id == currentSchemaId).firstOrNull?.name;

  static const Duration remoteLogoCacheTtl = Duration(days: 2);
  static const Duration remoteLogoFailureCooldown = Duration(minutes: 5);
  final Map<String, String?> _remoteSchemeLogos = {};
  final Map<String, DateTime> _remoteSchemeLogoFetchedAt = {};
  final Map<String, DateTime> _remoteSchemeLogoFailedAt = {};
  final Map<String, bool> _addonLogoInstalledState = {};
  final Set<String> _remoteSchemeLogoInFlight = {};
  final _schemaLogoVersions = <String, (DateTime, DateTime, int)>{};

  Object? schemaLogoVersion(String path) => _schemaLogoVersions[path];

  String? effectiveSchemeLogoPath(String key) {
    final package = installedPackageFor(key);
    return package?.logoPath ??
        (package?.hasPackageZip == true ? null : _remoteSchemeLogos[key]);
  }

  bool shouldFetchRemoteSchemeLogo(
    String key, {
    String? repoUrl,
    bool? addonInstalled,
  }) {
    if (_disposed || _remoteSchemeLogoInFlight.contains(key)) return false;
    final package = installedPackageFor(key);
    if (package?.hasPackageZip == true) return false;
    final url =
        repoUrl ??
        package?.git?.repoUrl ??
        schemeRepositoryUrls[key == 'triple_pinyin_lssp' ? 'rime_lssp' : key];
    if (url == null || url.isEmpty) return false;
    if (addonInstalled != null &&
        _addonLogoInstalledState[key] != addonInstalled) {
      return true;
    }
    final now = DateTime.now();
    final fetchedAt = _remoteSchemeLogoFetchedAt[key];
    if (fetchedAt != null && now.difference(fetchedAt) < remoteLogoCacheTtl) {
      return false;
    }
    final failedAt = _remoteSchemeLogoFailedAt[key];
    if (failedAt != null &&
        now.difference(failedAt) < remoteLogoFailureCooldown) {
      return false;
    }
    return true;
  }

  Future<bool> _readSchemaLogoVersion(String path) async {
    final stat = await File(path).stat();
    if (_disposed || stat.type != FileSystemEntityType.file) return false;
    final version = (stat.modified, stat.changed, stat.size);
    if (version == _schemaLogoVersions[path]) return false;
    _schemaLogoVersions[path] = version;
    return true;
  }

  Future<void> ensureRemoteSchemeLogo(
    String key, {
    String? repoUrl,
    String? branch,
    bool? addonInstalled,
  }) async {
    if (_disposed) return;
    final package = installedPackageFor(key);
    if (package?.hasPackageZip == true) {
      if (package?.logoPath != null) {
        _remoteSchemeLogos.remove(key);
        _remoteSchemeLogoFetchedAt.remove(key);
        _remoteSchemeLogoFailedAt.remove(key);
      }
      return;
    }
    if (addonInstalled != null &&
        _addonLogoInstalledState[key] != addonInstalled) {
      _addonLogoInstalledState[key] = addonInstalled;
      _remoteSchemeLogoFetchedAt.remove(key);
      _remoteSchemeLogoFailedAt.remove(key);
    }
    final now = DateTime.now();
    final fetchedAt = _remoteSchemeLogoFetchedAt[key];
    if (fetchedAt != null && now.difference(fetchedAt) < remoteLogoCacheTtl) {
      return;
    }
    final failedAt = _remoteSchemeLogoFailedAt[key];
    if (failedAt != null &&
        now.difference(failedAt) < remoteLogoFailureCooldown) {
      return;
    }
    final url =
        repoUrl ??
        package?.git?.repoUrl ??
        schemeRepositoryUrls[key == 'triple_pinyin_lssp' ? 'rime_lssp' : key];
    if (url == null || url.isEmpty) return;
    if (!_remoteSchemeLogoInFlight.add(key)) return;
    try {
      final path = await core.remoteSchemeLogo(
        schemeKey: key,
        repoUrl: url,
        branch: branch ?? package?.git?.branch,
      );
      if (_disposed) return;
      final imageChanged = path != null && await _readSchemaLogoVersion(path);
      if (_disposed) return;
      _remoteSchemeLogoFailedAt.remove(key);
      _remoteSchemeLogoFetchedAt[key] = DateTime.now();
      final latestPkg = installedPackageFor(key);
      if (latestPkg?.hasPackageZip == true && latestPkg?.logoPath != null) {
        _remoteSchemeLogos.remove(key);
        return;
      }
      final previous = _remoteSchemeLogos[key];
      _remoteSchemeLogos[key] = path;
      if (previous != path || imageChanged) {
        _changed();
      }
    } catch (_) {
      // Uninstalled preview icon lookup is best-effort; cool down before retry
      // so offline/rate-limited rebuilds do not hammer the bridge or flicker UI.
      if (!_disposed) _remoteSchemeLogoFailedAt[key] = DateTime.now();
    } finally {
      _remoteSchemeLogoInFlight.remove(key);
    }
  }

  EnglishModeDto desktopEnglishMode = EnglishModeDto.ascii;
  String desktopEnglishSwitchKey = 'schema';
  List<String> desktopSchemaEnglishSwitchKeys = const [];
  ComponentVersionsDto? versions;
  RuntimeLogSettingsDto? logSettings;
  DebugLogFileDto? runtimeLog;
  String? sharedLogPath;
  String scheme = 'keytao';
  String source = defaultDownloadSource;
  ReleaseInfoDto? latestRelease;
  SchemeReleaseDto? schemeRelease;
  AccountUserDto? accountUser;
  bool loggingIn = false;
  bool syncingDictionary = false;
  UserDictionarySyncDto? pendingDictionarySync;
  Object? _pendingDictionarySyncAccountId;
  final Map<Object, Set<String>> _cancelledDictionarySyncDeletions = {};
  Timer? _dictionarySyncDebounce;
  Timer? _dictionaryPullTimer;
  Timer? _dictionaryPullCooldown;
  Timer? _userWordsDeployDebounce;
  bool _foreground = true;
  String? accountError;
  String? accountMessage;
  bool followAccountSync = false;
  final Set<String> syncedSchemes = {};
  String? lastSchemeSyncAt;
  List<Map<String, dynamic>> cloudPackages = [];
  bool schemeSyncBusy = false;
  String? schemeSyncError;
  String? cloudTransferError;
  CloudTransferProgressDto? cloudTransferProgress;
  Map<String, dynamic>? pendingCloudProfile;
  String? pendingCloudActiveScheme;
  Timer? _schemeSyncDebounce;
  final Set<String> _pendingCloudPackageUploads = {};
  final Set<Object> _checkedSchemeSyncAccounts = {};
  Object? _readySchemeSyncAccount;
  Object? _checkingSchemeSyncAccount;
  bool _applyingCloudProfile = false;
  bool _schemeProfileDirty = false;
  Future<void>? _schemeSettingsWrite;
  StreamSubscription<BridgeEvent>? _events;
  StreamSubscription<InstallProgressDto>? _androidProgress;
  StreamSubscription<FileSystemEvent>? _currentSchemaEvents;
  Timer? _schemaSwitchPoll;

  bool get canInstall =>
      !busy && !releaseLoading && downloadUrl?.isNotEmpty == true;
  InstalledPackageDto? installedPackageFor(String key) {
    for (final package in installedPackages) {
      if (package.schemeKey == key) return package;
    }
    return null;
  }

  String packageDisplayName(String key) {
    final normalized = key.startsWith('builtin/')
        ? key.substring('builtin/'.length)
        : key;
    final installedName = installedPackageFor(normalized)?.displayName?.trim();
    if (installedName != null && installedName.isNotEmpty) return installedName;
    final isolatedName = isolatedPackages
        .where((p) => p.packageKey == key || p.packageKey == normalized)
        .map((p) => p.displayName.trim())
        .where((name) => name.isNotEmpty && name != key && name != normalized)
        .firstOrNull;
    if (isolatedName != null) return isolatedName;
    final cloudName = cloudPackages
        .where((p) => p['schemeKey'] == normalized)
        .map((p) => (p['name'] as String?)?.trim())
        .whereType<String>()
        .where((name) => name.isNotEmpty)
        .firstOrNull;
    if (cloudName != null) return cloudName;
    return schemeDisplayName(normalized);
  }

  bool isLocalScheme(String key) {
    final package = installedPackageFor(key);
    return package?.source == 'local' && package?.git == null;
  }

  List<InstalledPackageDto> get rememberedGitSchemes {
    final configs = <String, GitSchemeConfigDto>{};
    final names = <String, String?>{};
    final stored = _schemeSettings['custom_git_schemes'];
    if (stored is List) {
      for (final value in stored.whereType<Map>()) {
        final key = value['scheme_key'];
        final repoUrl = value['repo_url'];
        final mode = value['mode'];
        if (key is! String ||
            !key.startsWith('local-') ||
            repoUrl is! String ||
            mode is! String) {
          continue;
        }
        configs[key] = GitSchemeConfigDto(
          repoUrl: repoUrl,
          mode: mode,
          branch: value['branch'] as String?,
          assetName: value['asset_name'] as String?,
        );
        names[key] = value['display_name'] as String?;
      }
    }
    for (final package in installedPackages) {
      if (package.git case final git?) {
        configs[package.schemeKey] = git;
        names[package.schemeKey] =
            package.displayName ?? names[package.schemeKey];
      }
    }
    return [
      for (final key in configs.keys)
        _rememberedGitPackage(
          key,
          configs[key]!,
          names[key],
          installedPackageFor(key),
        ),
    ];
  }

  InstalledPackageDto _rememberedGitPackage(
    String key,
    GitSchemeConfigDto git,
    String? displayName,
    InstalledPackageDto? installed,
  ) => InstalledPackageDto(
    schemeKey: key,
    version: installed?.version,
    active: installed?.active ?? false,
    inUse: installed?.inUse ?? false,
    hasPackageZip: installed?.hasPackageZip ?? false,
    supportsUserWords: installed?.supportsUserWords ?? true,
    source: installed?.source ?? 'git',
    displayName: installed?.displayName ?? displayName,
    logoPath: installed?.logoPath,
    engine: installed?.engine ?? 'rime',
    git: installed?.git ?? git,
    buildCacheBytes: installed?.buildCacheBytes,
    imeJdCacheBytes: installed?.imeJdCacheBytes,
  );

  bool _rememberGitScheme({
    required String schemeKey,
    required String repoUrl,
    required String mode,
    required String? branch,
    required String? assetName,
    required String displayName,
  }) {
    final package = installedPackageFor(schemeKey);
    final git = package?.git;
    final url = git?.repoUrl ?? repoUrl;
    final savedMode = git?.mode ?? mode;
    final savedBranch = git?.branch ?? branch;
    final savedAsset = git?.assetName ?? assetName;
    final savedName = package?.displayName ?? displayName;
    final current = _schemeSettings['custom_git_schemes'];
    final entries = current is List
        ? current
              .whereType<Map>()
              .map((entry) => Map<String, dynamic>.from(entry))
              .toList()
        : <Map<String, dynamic>>[];
    final old = entries
        .where((entry) => entry['scheme_key'] == schemeKey)
        .firstOrNull;
    if (old != null &&
        old['repo_url'] == url &&
        old['mode'] == savedMode &&
        old['branch'] == savedBranch &&
        old['asset_name'] == savedAsset &&
        old['display_name'] == savedName) {
      return false;
    }
    entries.removeWhere((entry) => entry['scheme_key'] == schemeKey);
    entries.add({
      'scheme_key': schemeKey,
      'repo_url': url,
      'mode': savedMode,
      'branch': savedBranch,
      'asset_name': savedAsset,
      'display_name': savedName,
    });
    _schemeSettings['custom_git_schemes'] = entries;
    return true;
  }

  bool _rememberInstalledGitSchemes() {
    var changed = false;
    for (final package in installedPackages) {
      final git = package.git;
      if (git == null) continue;
      changed =
          _rememberGitScheme(
            schemeKey: package.schemeKey,
            repoUrl: git.repoUrl,
            mode: git.mode,
            branch: git.branch,
            assetName: git.assetName,
            displayName: package.displayName ?? '',
          ) ||
          changed;
    }
    return changed;
  }

  static String _schemeVisibilityKey(String key) {
    final normalized = key.startsWith('builtin/')
        ? key.substring('builtin/'.length)
        : key;
    return normalized == 'xmjd-feiding' ? 'xmjd' : normalized;
  }

  Set<String> get hiddenSchemes => {
    for (final key in _schemeSettings['hidden_schemes'] as List? ?? const [])
      if (key is String && _schemeVisibilityKey(key) != 'keytao')
        _schemeVisibilityKey(key),
  };

  bool isSchemeDeployed(String key) {
    final family = _schemeVisibilityKey(key);
    bool matches(String candidate) => _schemeVisibilityKey(candidate) == family;
    if (isolatedPackages.any(
      (package) => matches(package.packageKey) && package.ready,
    )) {
      return true;
    }
    if (local?.deployed == true && matches(activeSchemeKey)) return true;
    if (family == 'easy_en') return addon?.deployed == true;
    if (family == 'wanxiang') return wanxiang?.deployed == true;
    return rimeAddons.entries.any(
      (entry) => matches(entry.key) && entry.value.status?.deployed == true,
    );
  }

  bool canHideScheme(String key) =>
      !busy &&
      _schemeVisibilityKey(key) != 'keytao' &&
      !isLocalScheme(key) &&
      !isSchemeDeployed(key);

  bool isSchemeHidden(String key) =>
      hiddenSchemes.contains(_schemeVisibilityKey(key)) &&
      !isSchemeDeployed(key);

  Future<void> setSchemeHidden(String key, bool hidden) async {
    if ((hidden && !canHideScheme(key)) ||
        _schemeVisibilityKey(key) == 'keytao' ||
        isLocalScheme(key)) {
      return;
    }
    final keys = hiddenSchemes;
    final family = _schemeVisibilityKey(key);
    if (!(hidden ? keys.add(family) : keys.remove(family))) return;
    _schemeSettings['hidden_schemes'] = keys.toList()..sort();
    _changed();
    await _persistSchemeSettings();
  }

  String get activeSchemeKey {
    final installed = local?.installedScheme;
    if (installed != null) {
      return installed.startsWith('builtin/')
          ? installed.substring('builtin/'.length)
          : installed;
    }
    final isolated = isolatedPackages
        .where((package) => package.active)
        .firstOrNull;
    if (isolated != null) return isolated.packageKey;
    for (final package in installedPackages) {
      if (package.active) return package.schemeKey;
    }
    return local == null
        ? scheme
        : installedSchemeKey(local!.schemas, local!.installedScheme) ??
              switch (local!.schemas.firstOrNull) {
                'easy_en' => 'easy_en',
                'wanxiang' => 'wanxiang',
                'rime_ice' => 'rime_ice',
                'rime_mint' => 'rime_mint',
                'rime_frost' => 'rime_frost',
                'triple_pinyin_lssp' => 'triple_pinyin_lssp',
                _ => scheme,
              };
  }

  void markUserWordsChanged(String key) {
    userWordsRevision++;
    if (isKeytaoScheme(key)) {
      _scheduleUserWordsDeploy();
      scheduleUserDictionarySync();
    } else if (userWordsAffectActiveScheme(key)) {
      userWordsPendingDeploy.add(activeSchemeKey);
    }
    _changed();
  }

  void _scheduleUserWordsDeploy() {
    _userWordsDeployDebounce?.cancel();
    if (_disposed ||
        !isKeytaoScheme(activeSchemeKey) ||
        local?.installed != true) {
      return;
    }
    _userWordsDeployDebounce = Timer(const Duration(milliseconds: 800), () {
      if (_disposed || !isKeytaoScheme(activeSchemeKey)) return;
      if (busy) {
        _scheduleUserWordsDeploy();
      } else if (local?.installed == true) {
        unawaited(deploy());
      }
    });
  }

  bool isKeytaoScheme(String key) =>
      const {'keytao', 'keytao-dz', 'keytao-bj'}.contains(key);

  bool userWordsAffectActiveScheme(String key) =>
      key == activeSchemeKey ||
      (isKeytaoScheme(key) && isKeytaoScheme(activeSchemeKey)) ||
      (const {'xmjd', 'xmjd-feiding'}.contains(key) &&
          const {'xmjd', 'xmjd-feiding'}.contains(activeSchemeKey));

  void setUserWordDeletePending(String key, bool pending) {
    if (pending) {
      _pendingUserWordDeletes.add(key);
    } else {
      _pendingUserWordDeletes.remove(key);
      _resumeDeferredActions();
    }
    _changed();
  }

  void dismissUserWordsReport() {
    userWordsReport = null;
    userWordsReportScheme = null;
    userWordsReportError = null;
    _changed();
  }

  Future<void> _readUserWordsReport(String key) async {
    try {
      final report = await core.takeUserWordsReport(schemeKey: key);
      if (report.removed.isNotEmpty || report.conflicts.isNotEmpty) {
        userWordsReportScheme = key;
        userWordsReport = report;
      } else {
        userWordsReportScheme = null;
        userWordsReport = null;
      }
      userWordsReportError = null;
      if (report.removed.isNotEmpty && isKeytaoScheme(key)) {
        scheduleUserDictionarySync();
      }
    } catch (failure) {
      userWordsReportScheme = key;
      userWordsReport = null;
      userWordsReportError = errorText(failure);
      addOperationLogs(['[WARN] $userWordsReportError']);
    }
  }

  SchemeUpdateDto? schemeUpdateFor(String key) {
    for (final update in schemeUpdates) {
      if (update.schemeKey == key && update.hasUpdate) return update;
    }
    return null;
  }

  bool usesImportedLibjd(String key) {
    final isolated = isolatedPackages
        .where((package) => package.packageKey == key)
        .firstOrNull;
    return (isolated?.engine ?? installedPackageFor(key)?.engine) ==
        'libjd_engine';
  }

  bool get prefersLibjd => inputEngine == 'libjd_engine';
  bool get hasConvertedSchemes =>
      isolatedPackages.any(
        (package) => usesImportedLibjd(package.packageKey),
      ) ||
      installedPackages.any((package) => usesImportedLibjd(package.schemeKey));

  bool usesLibjd(String key) => prefersLibjd && usesImportedLibjd(key);

  bool get isUsingLibjd => usesLibjd(activeSchemeKey);

  bool requireRimeEngine() {
    if (!isUsingLibjd) return true;
    const message = '这项功能仅适用于 Rime';
    packageError = message;
    showToast(message);
    return false;
  }

  bool get hasSchemeUpdates => schemeUpdates.any((update) => update.hasUpdate);
  List<SchemeUpdateDto> get visibleSchemeUpdates => [
    for (final update in schemeUpdates)
      if (update.hasUpdate &&
          _normalizedVersion(dismissedSchemeUpdates[update.schemeKey] ?? '') !=
              _normalizedVersion(update.latestVersion))
        update,
  ];
  static String _normalizedVersion(String version) =>
      version.startsWith('v') ? version.substring(1) : version;
  bool get isInstalling => operation == OperationKind.install;
  bool get isManagingLibjd => operation == OperationKind.libjd;
  bool get isImportingLibjd => isManagingLibjd && installingSchemeKey != null;
  bool get canCancelDownload =>
      (isInstalling || isManagingLibjd) &&
      installProgress?.stage == 'downloading';
  bool get isDeploying => operation == OperationKind.deploy;
  bool get isManagingAddon => operation == OperationKind.addon;
  bool get isManagingWanxiang => operation == OperationKind.wanxiang;
  bool get isManagingWanxiangGrammar =>
      operation == OperationKind.wanxiangGrammar;
  bool get showWanxiangGrammar =>
      !isIos &&
      wanxiang?.installed == true &&
      wanxiangGrammar?.available == true;
  bool get canManageWanxiangGrammar => !busy && showWanxiangGrammar;
  bool get canCancelWanxiangGrammarDownload =>
      isManagingWanxiangGrammar &&
      wanxiangGrammarProgress?.stage.startsWith('wanxiang-grammar-download') ==
          true;
  String get wanxiangGrammarSize =>
      wanxiangGrammar != null && wanxiangGrammar!.size > BigInt.zero
      ? formatLogBytes(wanxiangGrammar!.size)
      : '407 MB';
  RimeAddonSlot? get _managingRimeAddon {
    for (final slot in rimeAddons.values) {
      if (operation == slot.operation) return slot;
    }
    return null;
  }

  bool get canInstallAddon => !busy;
  bool get canInstallWanxiang => canInstallAddon;
  bool get canUninstallAddon => !busy && addon?.installed == true;
  bool get canUninstallWanxiang => !busy && wanxiang?.installed == true;
  String get installedWanxiangVariant =>
      wanxiang?.installed == true ? wanxiang?.variant ?? 'base' : 'base';
  String get selectedWanxiangVariant =>
      _selectedWanxiangVariant ?? installedWanxiangVariant;
  bool get addonHasUpdate =>
      addon?.installed == true &&
      (addon!.version.isEmpty ||
          (addonLatestVersion != null && addonLatestVersion != addon!.version));
  bool get wanxiangHasUpdate =>
      wanxiang?.installed == true &&
      wanxiangLatestVersion != null &&
      _normalizedVersion(wanxiangLatestVersion!) !=
          _normalizedVersion(wanxiang!.version);
  bool get showAddonInstall =>
      addon?.installed != true || addon?.deployed != true || addonHasUpdate;
  bool get showWanxiangInstall =>
      wanxiang?.installed != true ||
      wanxiang?.deployed != true ||
      wanxiangHasUpdate ||
      selectedWanxiangVariant != installedWanxiangVariant;
  int get operationLogCount => operationLogs.length;
  String get installButtonLabel => checkingStorage
      ? AppStrings.checkingPermission
      : isInstalling
      ? AppStrings.installing
      : isDeploying
      ? AppStrings.deploying
      : installedPackageFor(scheme) != null ||
            (local?.installed == true &&
                installedSchemeKey(local!.schemas, local!.installedScheme) ==
                    scheme)
      ? AppStrings.updateScheme
      : AppStrings.installScheme;
  String get deployTooltip => activeSchemeDownloaded
      ? AppStrings.deployCurrent
      : AppStrings.installFirst;
  String get addonTooltip => AppStrings.installEnglishTooltip;
  String get wanxiangTooltip => AppStrings.installWanxiangTooltip;
  String get addonButtonLabel => addonHasUpdate
      ? AppStrings.update
      : addon?.installed == true
      ? AppStrings.deploy
      : AppStrings.download;
  String get wanxiangButtonLabel =>
      wanxiang?.installed == true &&
          selectedWanxiangVariant != installedWanxiangVariant
      ? AppStrings.download
      : wanxiangHasUpdate
      ? AppStrings.update
      : wanxiang?.installed == true
      ? AppStrings.deploy
      : AppStrings.download;
  String get addonStatusText => _addonStatus('easy_en', addon, addonHasUpdate);
  String rimeAddonStatusText(String id) {
    final slot = rimeAddons[id]!;
    return _addonStatus(id, slot.status, slot.hasUpdate);
  }

  String get wanxiangStatusText {
    final status = _addonStatus('wanxiang', wanxiang, wanxiangHasUpdate);
    return wanxiang?.installed == true
        ? '$status · ${AppStrings.wanxiangVariants[installedWanxiangVariant] ?? installedWanxiangVariant}'
        : status;
  }

  String _addonStatus(String key, core.AddonStatusDto? addon, bool hasUpdate) {
    if (addon == null) return AppStrings.checking;
    if (!addon.installed) return AppStrings.notInstalled;
    if (prefersLibjd && key != 'easy_en') {
      final status = activeSchemeKey == key && isUsingLibjd
          ? '使用中'
          : hasUpdate
          ? '可更新'
          : usesImportedLibjd(key)
          ? '已转换'
          : '待转换';
      return '$status · ${addon.version.isEmpty ? AppStrings.unknownVersion : addon.version}';
    }
    final state = activeSchemeKey == key
        ? addon.deployed
              ? '使用中'
              : AppStrings.pendingDeploy
        : hasUpdate
        ? '可更新'
        : addon.deployed
        ? '已安装'
        : AppStrings.pendingDeploy;
    return '$state · ${addon.version.isEmpty ? AppStrings.unknownVersion : addon.version}';
  }

  String? get localStatusLine => local == null
      ? null
      : AppStrings.localStatus(
          installed: local!.installed,
          deployed: hasUsableBuild,
          version: displayedSchemeVersion(
            local!.schemas,
            local!.version,
            local!.installedScheme,
            local!.installedSchemeVersion,
          ),
          label: schemes[schemeKeyFromSchemas(local!.schemas)],
        );
  String? get localSchemaIds =>
      local?.installed == true && local!.schemas.isNotEmpty
      ? '(${local!.schemas.join(', ')})'
      : null;

  /// 星猫键道有两个发行包（顶功 / 非顶功），它们同属一个方案族。
  String get schemeFamily => scheme == 'xmjd-feiding' ? 'xmjd' : scheme;
  String get selectedSchemeAsset => scheme == 'keytao'
      ? Uri.tryParse(downloadUrl ?? '')?.pathSegments.lastOrNull ?? ''
      : schemeRelease?.assetName ?? schemeAssets[scheme] ?? '';
  String get schemeSelectionText =>
      '${AppStrings.selection} ${schemes[schemeFamily] ?? scheme}'
      '${releaseVersion == null ? '' : ' $releaseVersion'} · $selectedSchemeAsset';
  Map<String, String> get sourceVersions => {
    if (latestRelease?.github case final release?) 'github': release.version,
    if (latestRelease?.gitee case final release?) 'gitee': release.version,
  };
  String? get changelogBody =>
      scheme == 'keytao' && latestRelease?.body.isNotEmpty == true
      ? latestRelease!.body
      : null;
  String? get changelogTitle => changelogBody == null
      ? null
      : AppStrings.changelogTitle(
          latestRelease!.name.isEmpty
              ? latestRelease!.version
              : latestRelease!.name,
        );
  ImeBadgeState get macosImeBadge => macosIme == null
      ? ImeBadgeState.checking
      : !macosIme!.installed
      ? ImeBadgeState.componentMissing
      : !activeSchemeDownloaded
      ? ImeBadgeState.schemeMissing
      : !hasUsableBuild && !isUsingLibjd
      ? ImeBadgeState.pendingDeploy
      : ImeBadgeState.ready;
  String get macosImeBadgeText => switch (macosImeBadge) {
    ImeBadgeState.checking => AppStrings.checking,
    ImeBadgeState.componentMissing => AppStrings.componentMissing,
    ImeBadgeState.schemeMissing => AppStrings.schemeMissing,
    ImeBadgeState.pendingDeploy => AppStrings.pendingDeploy,
    ImeBadgeState.ready => AppStrings.usable,
  };
  bool get windowsRegistrationBusy =>
      isWindows &&
      (_managingWindowsIme || windowsIme?.registrationBusy == true);
  String get windowsRegistrationLabel {
    final state = windowsIme?.registrationState;
    if (windowsRegistrationBusy) {
      return switch (state) {
        'registering' => AppStrings.registering,
        'repairing' => AppStrings.repairing,
        _ => AppStrings.checkingRegistration,
      };
    }
    if (state == 'repair_failed') return AppStrings.repairFailed;
    if (state == 'failed') return AppStrings.registrationFailed;
    if (windowsIme?.registered == true) return AppStrings.registered;
    return switch (state) {
      'partial' => AppStrings.partiallyRegistered,
      'missing_runtime' => AppStrings.runtimeMissing,
      _ => windowsIme == null ? AppStrings.checking : AppStrings.notRegistered,
    };
  }

  String get defaultDir => info.userRoot;

  String? get activeIsolatedPackageKey {
    final activeIsolated = isolatedPackages
        .where((pkg) => pkg.active)
        .firstOrNull;
    if (activeIsolated != null && activeIsolated.packageKey.isNotEmpty) {
      return activeIsolated.packageKey;
    }
    final installed = local?.installedScheme;
    if (installed != null && installed.isNotEmpty) return installed;
    for (final package in installedPackages) {
      if (package.active && package.schemeKey.isNotEmpty) {
        return package.schemeKey;
      }
    }
    return null;
  }

  String schemeDirectoryFor(String packageKey) {
    final root = defaultDir;
    if (root.isEmpty || packageKey.isEmpty) return root;
    final sep = isWindows ? r'\' : '/';
    final normalizedKey = isWindows
        ? packageKey.replaceAll('/', r'\')
        : packageKey;
    final trimmedRoot = root.endsWith(sep)
        ? root.substring(0, root.length - 1)
        : root;
    return '$trimmedRoot${sep}runtime$sep$normalizedKey';
  }

  String get activeSchemeDir {
    final key = activeIsolatedPackageKey;
    if (key == null) return defaultDir;
    return schemeDirectoryFor(key);
  }

  String get dictionaryManagerUrl => Uri(
    scheme: 'rime-dict',
    host: 'open',
    queryParameters: {'dir': activeSchemeDir},
  ).toString();
  bool get canOpenDictionaryManager =>
      !isMobile && !busy && defaultDir.isNotEmpty;
  String? get englishModeHint => englishReady
      ? null
      : info.platform == BridgePlatform.macOs
      ? '请在上方方案列表安装附加方案 English'
      : AppStrings.englishHint;
  bool get canOpenStorageSettings =>
      !busy && storage?['canOpenSettings'] != false;
  String get permissionDescription =>
      AppStrings.permissionDescription(storage?['path'] as String?);
  String? get imeMessage => ime?['message'] as String?;
  String? get logDirectory => logSettings?.logDir;
  int get logFileCount => logSettings?.fileCount ?? 0;
  String get logTotalSize =>
      formatLogBytes(logSettings?.totalBytes ?? BigInt.zero);
  String? get logStatsText => logSettings == null
      ? null
      : AppStrings.logStats(logFileCount, logTotalSize);
  List<String> get logFileLines => [
    for (final file in logSettings?.files ?? <RuntimeLogFileInfoDto>[])
      '${file.name} · ${formatLogBytes(file.size)}',
  ];
  String? get sharedLogResultText =>
      sharedLogPath == null ? null : AppStrings.savedLog(sharedLogPath!);
  bool _isInlineError(String? message) => [
    AppStrings.downloadCancelled,
    AppStrings.retryPackageMigration,
    releaseError,
    packageError,
    installError,
    addonError,
    wanxiangError,
    wanxiangGrammarError,
    ...rimeAddons.values.map((slot) => slot.error),
    macosImeError,
    windowsImeError,
    linuxImeError,
    imeUiError,
    schemaSwitchError,
    debugError,
    rimeSyncError,
    schemeSyncError,
    cloudTransferError,
    ...deploySteps.map((step) => step.message),
  ].contains(message);
  String? get unhandledError => _isInlineError(error) ? null : error;

  bool get storageReady =>
      storage?['granted'] == true && storage?['writable'] == true;
  bool get englishReady => schemas.any(
    (schema) =>
        const {'easy_en', 'english'}.contains(schema.id) ||
        const {'easy english', 'english'}.contains(schema.name.toLowerCase()),
  );
  bool get setupReady {
    if (isAndroid) {
      return storageReady &&
          ime?['enabled'] == true &&
          ime?['selected'] == true;
    }
    if (isIos) return iosKeyboardEnabled && iosKeyboardFullAccess == true;
    return local?.installed == true &&
        hasUsableBuild &&
        (!hasOnboarding || macosIme?.installed == true);
  }

  PlatformReleaseDto? get selectedRelease =>
      source == 'github' ? latestRelease?.github : latestRelease?.gitee;
  String? get releaseVersion =>
      scheme == 'keytao' ? selectedRelease?.version : schemeRelease?.version;
  String? get downloadUrl => scheme == 'keytao'
      ? _platformDownloadUrl(selectedRelease)
      : schemeRelease?.downloadUrl;

  String? _platformDownloadUrl(PlatformReleaseDto? release) {
    final platformUrl = switch (info.platform) {
      BridgePlatform.android => release?.downloadUrls.android,
      BridgePlatform.ios => release?.downloadUrls.ios,
      BridgePlatform.windows => release?.downloadUrls.windows,
      BridgePlatform.linux => release?.downloadUrls.linux,
      BridgePlatform.macOs => release?.downloadUrls.macos,
    };
    if (platformUrl != null && platformUrl.isNotEmpty) return platformUrl;
    final universal = release?.downloadUrl;
    return universal != null && universal.isNotEmpty ? universal : null;
  }

  void _changed() {
    if (!_disposed) notifyListeners();
  }

  Future<void> connectEvents({List<String> args = const []}) async {
    if (_events != null) return;
    if (isAndroid) {
      _androidProgress = android.installProgress.listen(
        _receiveInstallProgress,
      );
    }
    final ready = Completer<void>();
    if (isWindows) {
      _windowsChannel.setMethodCallHandler((call) async {
        if (call.method != 'appArgs') throw MissingPluginException();
        final value = call.arguments;
        if (value is! List || value.any((arg) => arg is! String)) {
          throw PlatformException(code: 'invalid_args');
        }
        await ready.future;
        await core.handleAppArgs(args: value.cast<String>());
      });
    }
    _events = core.coreEvents().listen(
      (event) {
        if (event.kind == BridgeEventKind.ready && !ready.isCompleted) {
          ready.complete();
        }
        if (event.installProgress case final value?) {
          _receiveInstallProgress(value);
          return;
        }
        if (event.cloudTransferProgress case final value?) {
          final previous = cloudTransferProgress;
          cloudTransferProgress = value;
          if (previous == null ||
              previous.direction != value.direction ||
              previous.schemeKey != value.schemeKey ||
              _cloudTransferPercent(previous) != _cloudTransferPercent(value)) {
            _changed();
          }
          return;
        }
        if (event.deployProgress case final value?) {
          _receiveDeployProgress(value);
        }
        if (isWindows) {
          if (event.windowsImeStatus case final status?) {
            windowsIme = status;
            windowsImeError = status.registrationError;
          }
          if (event.kind == BridgeEventKind.windowsImeAction) {
            selectPage(AppPage.input);
            _windowsActionRequested = true;
          }
          unawaited(_handleWindowsAction());
        }
        _changed();
      },
      onError: (Object failure) {
        if (!ready.isCompleted) ready.completeError(failure);
        error = errorText(failure);
        _changed();
      },
    );
    await ready.future.timeout(const Duration(seconds: 10));
    if (isWindows) {
      await core.handleAppArgs(args: args);
      _windowsActionRequested = true;
    }
  }

  Future<void> _handleWindowsAction() async {
    if (!isWindows ||
        _disposed ||
        !_platformStarted ||
        (local == null && _localSchemaError == null) ||
        busy ||
        _windowsActionInFlight ||
        !_windowsActionRequested) {
      return;
    }
    _windowsActionInFlight = true;
    _windowsActionRequested = false;
    try {
      final id = await core.windowsPendingImeAction();
      if (_disposed || id == null) return;
      if (busy) {
        _windowsActionRequested = true;
        return;
      }
      selectPage(AppPage.input);
      await deploy(imeActionId: id);
      // The old app acknowledges attempted deployments, including errors and
      // missing schemes; retry requires a new IME request.
      await core.windowsDismissImeAction(id: id);
      _windowsActionRequested = true;
    } catch (failure) {
      _deployStep(errorText(failure), DeployStepState.failed);
    } finally {
      _windowsActionInFlight = false;
      if (!_disposed && !busy && _windowsActionRequested) {
        unawaited(_handleWindowsAction());
      }
    }
  }

  static BigInt? _cloudTransferPercent(CloudTransferProgressDto value) =>
      value.totalBytes == BigInt.zero
      ? null
      : value.bytesTransferred * BigInt.from(100) ~/ value.totalBytes;

  void _receiveInstallProgress(InstallProgressDto value) {
    bool changedFrom(InstallProgressDto? previous) =>
        previous?.stage != value.stage || previous?.percent != value.percent;

    if (value.stage == 'app_update_downloading' ||
        value.stage == 'app_update_verifying') {
      if (appUpdateBusy && appUpdateAction == AppUpdateAction.download) {
        final changed = changedFrom(appUpdateProgress);
        appUpdateProgress = value;
        if (changed) _changed();
      }
      return;
    }
    final changed = changedFrom(installProgress);
    installProgress = value;
    progress = value.message;
    installFraction = value.percent.clamp(0, 100) / 100;
    if (isManagingAddon) addonProgress = value;
    if (value.stage.contains('deploy') && operation == OperationKind.rimeIce) {
      deployingSchemeKey = 'rime_ice';
    }
    if (isManagingWanxiang) wanxiangProgress = value;
    if (isManagingWanxiangGrammar) wanxiangGrammarProgress = value;
    _managingRimeAddon?.progress = value;
    if (value.stage == 'repairing') {
      addOperationLogs(['[REPAIR] ${value.message}']);
      showToast(value.message);
    } else if (changed) {
      _changed();
    }
  }

  void _receiveDeployProgress(DeployProgressDto value) {
    if (value.legacyMessage case final message?) {
      if (message == AppStrings.migratingPackageDirs) {
        if (busy && operation != OperationKind.migrate) {
          _migrationDuringOperation = true;
          _changed();
        }
        return;
      }
      if (message == AppStrings.retryPackageMigration ||
          (_migrationDuringOperation && message.isEmpty)) {
        _migrationDuringOperation = false;
        if (message.isNotEmpty) {
          _recordMigrationFailure(message);
        } else {
          _packageMigrationFailed = false;
        }
        _changed();
        return;
      }
      if (message.isEmpty) return;
      if (isManagingAddon || operation == OperationKind.rimeIce) {
        if (operation == OperationKind.rimeIce) deployingSchemeKey = 'rime_ice';
        progress = message;
      }
      if (message == AppStrings.deployDone) deployWarning = null;
      final compileWarning = message.contains('编译失败，已跳过：');
      final publicWarning = message.startsWith('系统搜索框方案未更新：');
      if (compileWarning) deployWarning = message;
      if (publicWarning) addOperationLogs(['[WARN] $message']);
      if (isDeploying ||
          ((isManagingAddon ||
                  isManagingWanxiang ||
                  isManagingWanxiangGrammar ||
                  _managingRimeAddon != null) &&
              (publicWarning || compileWarning))) {
        progress = message;
        _deployStep(message, DeployStepState.neutral);
        if (!isDeploying && !publicWarning) {
          addOperationLogs(['[WARN] $message']);
        }
      }
      return;
    }

    if (value.index == 1 && value.step == DeployStepDto.materialize) {
      _packageDeployProgress
        ..clear()
        ..[value.packageKey] = value;
      for (final key in value.queuedPackageKeys) {
        _packageDeployProgress.putIfAbsent(key, () => null);
      }
      deploySteps.clear();
      deployWarning = null;
    }
    _packageDeployProgress[value.packageKey] = value;
    final heading = '正在部署 ${value.packageName}（${value.index}/${value.total}）';
    final step = switch (value.step) {
      DeployStepDto.skip || DeployStepDto.done => DeployStep(
        value.packageName,
        DeployStepState.ok,
        packageKey: value.packageKey,
      ),
      DeployStepDto.failed => DeployStep(
        '${value.packageName}：${value.error ?? AppStrings.deployFailed}',
        DeployStepState.failed,
        packageKey: value.packageKey,
        error: value.error,
      ),
      _ => DeployStep(
        heading,
        DeployStepState.neutral,
        packageKey: value.packageKey,
      ),
    };
    if (step.state == DeployStepState.neutral) progress = heading;
    final row = deploySteps.indexWhere(
      (step) => step.packageKey == value.packageKey,
    );
    if (row == -1) {
      deploySteps.add(step);
    } else {
      deploySteps[row] = step;
    }
  }

  void addOperationLogs(Iterable<String> lines) {
    final now = DateTime.now().toLocal();
    String part(int value) => value.toString().padLeft(2, '0');
    final time = '${part(now.hour)}:${part(now.minute)}:${part(now.second)}';
    for (final line in lines) {
      for (final part in line.split('\n')) {
        operationLogs.add('[$time] $part');
      }
    }
    _changed();
  }

  void clearOperationLogs() {
    operationLogs.clear();
    _changed();
  }

  void _deployStep(String message, DeployStepState state) {
    deploySteps.add(DeployStep(message, state));
    _changed();
  }

  void selectPage(AppPage page) {
    if (info.platform == BridgePlatform.macOs && page == AppPage.scheme) {
      page = AppPage.input;
    }
    if (activePage == page || (page == AppPage.website && websiteHidden)) {
      return;
    }
    activePage = page;
    unawaited(setWindowTitle());
    if (page == AppPage.about) {
      sharedLogPath = null;
      unawaited(refreshLogs());
    }
    if (page == AppPage.scheme ||
        (page == AppPage.input && info.platform == BridgePlatform.macOs)) {
      unawaited(refreshSchemeUpdates());
      unawaited(_refreshLiveSchemaSwitches());
    }
    if (page == AppPage.input) {
      unawaited(refreshSchemas());
    }
    _changed();
  }

  Future<bool> saveWebsiteSettings({String? origin, bool? hidden}) async {
    if (_disposed || !canEditWebsiteSettings) return false;
    _savingWebsiteSettings = true;
    _changed();
    try {
      final nextOrigin = origin ?? websiteOrigin;
      final nextHidden = hidden ?? websiteHidden;
      await core.setWebsiteSettings(origin: nextOrigin, hidden: nextHidden);
      if (_disposed) return false;
      websiteOrigin = nextOrigin;
      _websiteHidden.value = nextHidden;
      if (websiteHidden && activePage == AppPage.website) {
        selectPage(AppPage.input);
      }
      return true;
    } catch (failure) {
      if (!_disposed) showToast(errorText(failure));
      return false;
    } finally {
      _savingWebsiteSettings = false;
      _changed();
    }
  }

  Future<bool> saveReduceMotion(bool enabled) async {
    if (_disposed || !canSaveReduceMotion) return false;
    if (enabled == reduceMotion) return true;
    _savingReduceMotion = true;
    _changed();
    try {
      await core.setReduceMotion(enabled: enabled);
      if (_disposed) return false;
      _reduceMotion.value = enabled;
      return true;
    } catch (failure) {
      if (!_disposed) showToast(errorText(failure));
      return false;
    } finally {
      _savingReduceMotion = false;
      _changed();
    }
  }

  Future<bool> run(
    Future<void> Function() action, {
    bool switchingInput = false,
  }) async {
    if (busy || _disposed) return false;
    busy = true;
    _switchingInput = switchingInput;
    error = null;
    _changed();
    try {
      await action();
      return true;
    } catch (failure) {
      final message = errorText(failure);
      final migrationError =
          message == AppStrings.retryPackageMigration ||
          message.startsWith('方案目录迁移失败');
      if (migrationError) {
        _recordMigrationFailure(message);
      }
      error = migrationError || _isInlineError(message) ? null : message;
      return false;
    } finally {
      _migrationDuringOperation = false;
      _packageDeployProgress.clear();
      _switchingInput = false;
      busy = false;
      _changed();
      _resumeDeferredActions();
    }
  }

  void _resumeDeferredActions() {
    if (busy || _disposed) return;
    if (_refreshOnResumeWhenIdle) {
      _refreshOnResumeWhenIdle = false;
      unawaited(_refreshOnResume());
    }
    if (_refreshSchemasWhenIdle) {
      _refreshSchemasWhenIdle = false;
      unawaited(refreshSchemas());
    }
    unawaited(_handleWindowsAction());
    _deployChangedScheme();
  }

  Future<void> _readLocal() async {
    _localSchemaError = null;
    try {
      inputEngine = await core.inputEngine();
    } catch (failure) {
      debugPrint('Input engine refresh failed: ${errorText(failure)}');
    }
    await _readInputEngineEnabled();
    try {
      local = await core.checkLocalSchema();
      deployWarning = local?.deployWarning;
    } catch (failure) {
      _localSchemaError = errorText(failure);
      rethrow;
    }
    if (!_manuallySelectedScheme) {
      final detected = installedSchemeKey(
        local!.schemas,
        local!.installedScheme,
      );
      if (detected != null && detected != scheme) {
        scheme = detected;
        schemeRelease = null;
        latestRelease = null;
        if (_startupStarted) unawaited(refreshRelease());
      }
    }
    try {
      addon = await core.addonSchemaStatus(id: 'easy_en');
      addonError = null;
    } catch (failure) {
      addonError = errorText(failure);
    }
    await _readRimeAddon(rimeAddons['rime_ice']!);
    await refreshIsolatedPackages(notify: false);
    _watchCurrentSchema();
    await _readSchemas();
    await _readSchemaSwitches();
    final logoPath = await core.activeSchemaLogoPath();
    if (logoPath != null) await _readSchemaLogoVersion(logoPath);
    activeSchemaLogoPath = logoPath;
  }

  Future<void> _readRimeAddon(RimeAddonSlot slot) async {
    try {
      slot.status = await slot._readStatus();
      if (slot.operation != OperationKind.rimeIce ||
          operation != slot.operation) {
        slot.error = null;
      }
    } catch (failure) {
      if (slot.operation != OperationKind.rimeIce || slot.error == null) {
        slot.error = errorText(failure);
      }
    }
  }

  void _watchCurrentSchema() {
    if (_disposed ||
        _currentSchemaEvents != null ||
        _schemaSwitchPoll != null) {
      return;
    }
    if (!FileSystemEntity.isWatchSupported) {
      _pollSchemaSwitches();
      return;
    }
    try {
      _currentSchemaEvents = Directory(info.userRoot)
          .watch(
            events:
                FileSystemEvent.create |
                FileSystemEvent.modify |
                FileSystemEvent.move,
          )
          .listen(
            (event) {
              if (!_foreground) return;
              final separator = Platform.pathSeparator;
              bool schemaChanged(String? path) =>
                  path?.endsWith('$separator.keytao-current-schema') == true ||
                  path?.endsWith('${separator}keytao-ime.reload') == true;
              final destination = event is FileSystemMoveEvent
                  ? event.destination
                  : null;
              if (schemaChanged(event.path) || schemaChanged(destination)) {
                unawaited(refreshSchemas());
              } else if (event.path.endsWith(_liveSwitchesPath) ||
                  destination?.endsWith(_liveSwitchesPath) == true) {
                unawaited(_refreshLiveSchemaSwitches());
              }
            },
            onError: (Object _) {
              _currentSchemaEvents = null;
              _pollSchemaSwitches();
            },
            cancelOnError: true,
          );
    } on FileSystemException {
      _pollSchemaSwitches();
    } on UnsupportedError {
      _pollSchemaSwitches();
    }
  }

  String get _liveSwitchesPath =>
      '${Platform.pathSeparator}.keytao-live-switches.json';

  void _pollSchemaSwitches() {
    if (_disposed || _schemaSwitchPoll != null) return;
    final marker = File('${info.userRoot}$_liveSwitchesPath');
    DateTime? lastModified;
    _schemaSwitchPoll = Timer.periodic(const Duration(seconds: 1), (_) async {
      final watchSwitches =
          activePage == AppPage.scheme ||
          (activePage == AppPage.input &&
              info.platform == BridgePlatform.macOs);
      if (!_foreground || !watchSwitches) return;
      final stat = await marker.stat();
      if (_disposed || !_foreground || stat.type != FileSystemEntityType.file) {
        return;
      }
      if (lastModified != stat.modified) {
        lastModified = stat.modified;
        await _refreshLiveSchemaSwitches();
      }
    });
  }

  Future<void> _refreshLiveSchemaSwitches() async {
    if (_disposed || !_foreground) return;
    if (busy) {
      _refreshSchemasWhenIdle = true;
      return;
    }
    final span = beginAppOperation(AppOperationName.refreshLiveSchemaSwitches);
    try {
      final previous = _activePreviewTarget;
      await _readSchemas();
      await _readSchemaSwitches();
      if (previous != _activePreviewTarget) {
        unawaited(refreshCandidatePreview());
      }
      _changed();
    } catch (failure) {
      debugPrint('Live schema refresh failed: ${errorText(failure)}');
    } finally {
      span?.dispose();
    }
  }

  Future<void> _readSchemas() async {
    if (!hasUsableBuild && !isUsingLibjd) {
      schemas = [];
      currentSchemaId = null;
      return;
    }
    final result = await core.getSchemas();
    schemas = result.schemas;
    currentSchemaId = result.current;
  }

  Future<void> refreshSchemas() async {
    if (_disposed) return;
    if (busy) {
      _refreshSchemasWhenIdle = true;
      return;
    }
    try {
      final previous = _activePreviewTarget;
      final previousEngineEnabled = inputEngineEnabled;
      await _readInputEngineEnabled();
      final snapshot = await core.checkLocalSchema();
      if (_disposed) return;
      if (busy) {
        _refreshSchemasWhenIdle = true;
        return;
      }
      local = snapshot;
      _localSchemaError = null;
      deployWarning = local?.deployWarning;
      await _readInstalledPackages();
      await refreshIsolatedPackages(notify: false);
      await _readSchemas();
      await _readSchemaSwitches();
      if (previous != _activePreviewTarget ||
          previousEngineEnabled != inputEngineEnabled) {
        unawaited(refreshCandidatePreview());
      }
      _changed();
      _deployChangedScheme();
    } catch (_) {
      // Keep the last schema visible if a foreground refresh is unavailable.
    }
  }

  Future<void> refreshIsolatedPackages({bool notify = true}) async {
    final request = ++_isolatedPackagesRequest;
    try {
      final packages = await core.isolatedPackages();
      if (_disposed || request != _isolatedPackagesRequest) return;
      isolatedPackages = packages;
      if (notify) _changed();
    } catch (failure) {
      debugPrint('Package selector refresh failed: ${errorText(failure)}');
    }
  }

  Future<void> _readSchemaSwitches() async {
    final request = ++_schemaSwitchesRequest;
    List<SchemaSwitchDto> switches = [];
    String? loadError;
    if (hasUsableBuild && !usesLibjd(activeSchemeKey)) {
      try {
        switches = await core.getSchemaSwitches();
      } catch (failure) {
        loadError = AppStrings.schemaSwitchUnavailable;
        debugPrint('Schema switch refresh failed: ${errorText(failure)}');
      }
    }
    if (_disposed || request != _schemaSwitchesRequest) return;
    schemaSwitches = switches;
    schemaSwitchError = loadError;
    if (!isMobile) {
      try {
        final switchKey = await core.getDesktopEnglishSwitchKey();
        if (!_disposed && request == _schemaSwitchesRequest) {
          desktopEnglishSwitchKey = switchKey.key;
          desktopSchemaEnglishSwitchKeys = switchKey.schemaKeys;
        }
      } catch (_) {}
    }
  }

  Future<void> refreshIosKeyboardStatus() async {
    if (!isIos || busy || _resumeRefreshInFlight || _disposed || !_foreground) {
      return;
    }
    try {
      if (await _readIosKeyboardStatus()) _changed();
    } catch (failure) {
      debugPrint('Keyboard status refresh failed: ${errorText(failure)}');
    }
  }

  Future<bool> _readIosKeyboardStatus() async {
    final status = await const IosHost().keyboardStatus();
    final enabled = status['enabled'] == true;
    final fullAccess = enabled ? status['fullAccess'] == true : null;
    if (iosKeyboardEnabled == enabled && iosKeyboardFullAccess == fullAccess) {
      return false;
    }
    iosKeyboardEnabled = enabled;
    iosKeyboardFullAccess = fullAccess;
    return true;
  }

  Future<void> _readState({bool quiet = false, bool refreshPreview = true}) =>
      measureAppOperation(
        AppOperationName.readState,
        () => _readStateSnapshot(quiet: quiet, refreshPreview: refreshPreview),
      );

  Future<void> _readStateSnapshot({
    required bool quiet,
    required bool refreshPreview,
  }) async {
    final errors = <String>[];
    Future<void> read(Future<void> Function() action) async {
      try {
        await action();
      } catch (failure) {
        errors.add(errorText(failure));
      }
    }

    if (isAndroid) {
      storage = null;
      ime = null;
      await Future.wait([
        read(() async {
          await _readStorage();
        }),
        read(() async {
          ime = await android.imeStatus();
        }),
      ]);
    } else if (isIos) {
      iosKeyboardEnabled = false;
      iosKeyboardFullAccess = null;
      await read(_readIosKeyboardStatus);
    } else if (isWindows) {
      await read(_readWindowsIme);
    } else if (isLinux) {
      await read(() async {
        try {
          linuxIme = await core.linuxImeStatus();
          linuxImeError = null;
        } catch (failure) {
          linuxImeError = errorText(failure);
          rethrow;
        }
      });
    } else {
      macosIme = null;
      await read(() async {
        try {
          macosIme = await core.macosImeStatus();
          macosImeError = null;
        } catch (failure) {
          macosImeError = errorText(failure);
          rethrow;
        }
      });
    }
    await Future.wait([
      read(_readLocal),
      read(_readInstalledPackages),
      read(() async {
        try {
          wanxiang = await core.wanxiangStatus();
          wanxiangGrammarError = null;
          await _readWanxiangGrammar();
          wanxiangError = null;
        } catch (failure) {
          wanxiangError = errorText(failure);
        }
      }),
      for (final slot in rimeAddons.values)
        if (slot.operation != OperationKind.rimeIce)
          read(() => _readRimeAddon(slot)),
      read(() async {
        versions = await core.getComponentVersions();
      }),
      if (!isAndroid || storageReady)
        read(() async {
          candidatePageSize = await core.getCandidatePageSize();
        }),
      if (isIos || (isAndroid && storageReady))
        read(() async {
          androidSettings = await core.getAndroidImeInputSettings();
        }),
      if (isIos || (isAndroid && storageReady))
        read(() async {
          await _refreshKeyboardLibrary();
        }),
      if (!isAndroid || storageReady)
        read(() async {
          uiSettings = await core.getImeUiSettings();
        }),
      if (!isMobile) ...[
        read(() async {
          desktopEnglishMode = await core.getDesktopEnglishMode();
        }),
        read(() async {
          final switchKey = await core.getDesktopEnglishSwitchKey();
          desktopEnglishSwitchKey = switchKey.key;
          desktopSchemaEnglishSwitchKeys = switchKey.schemaKeys;
        }),
      ],
    ]);
    if (isAndroid && !storageReady) {
      androidSettings = null;
      uiSettings = null;
    }
    if (errors.isNotEmpty) {
      final message = errors.toSet().join('\n');
      if (quiet) {
        debugPrint('Resume refresh failed: $message');
      } else {
        error = message;
      }
    }
    if (refreshPreview) unawaited(refreshCandidatePreview());
  }

  Future<bool> refreshState() async {
    final result = await run(() async {
      _attemptedAutoDeployKey = null;
      String? startupImeError;
      if (!_platformStarted) {
        _platformStarted = true;
        if (isWindows) {
          try {
            windowsIme = await core.windowsImeEnsureRegistered();
            windowsImeError = windowsIme!.registrationError;
          } catch (failure) {
            startupImeError = errorText(failure);
          }
          try {
            await core.windowsPrepareSearchSchemas();
          } catch (failure) {
            imeUiError = AppStrings.windowsSearchFailure(errorText(failure));
            addOperationLogs(['[WINDOWS SEARCH] $imeUiError']);
          }
        } else if (isLinux) {
          try {
            linuxIme = await core.linuxStartIme();
            linuxImeError = null;
          } catch (failure) {
            startupImeError = errorText(failure);
          }
        }
      }
      await _readState();
      await _deployIosUpgradeIfNeeded();
      if (startupImeError != null) {
        if (isWindows) windowsImeError = startupImeError;
        if (isLinux) linuxImeError = startupImeError;
      }
    });
    if (!_startupStarted && !_disposed) {
      _startupStarted = true;
      unawaited(_loadAccount());
      unawaited(_checkAppUpdate(silent: true));
      unawaited(refreshSchemeUpdates());
      unawaited(refreshRelease());
    }
    if (result && !_disposed) unawaited(_checkAddonUpdates());
    return result;
  }

  Future<void> _deployIosUpgradeIfNeeded() async {
    if (!isIos ||
        prefersLibjd ||
        packageMigrationFailed ||
        _activeBuildHasChanges ||
        local?.installed != true ||
        local?.deployed == true) {
      return;
    }
    final String stamp;
    try {
      stamp = await File('${info.userRoot}/rime-data/.bundled-version')
          .readAsString();
    } on FileSystemException {
      return;
    }
    if (_disposed ||
        stamp.isEmpty ||
        !_iosUpgradeDeploymentAttempts.add(stamp)) {
      return;
    }
    // Keep run's busy guard held; core replaces an interrupted deploy sentinel.
    await _deploy();
  }

  Future<void> _checkAddonUpdates() async {
    await Future.wait([
      if (addon?.installed == true)
        _checkAddonUpdate('easy_en', addon!.version),
      if (wanxiang?.installed == true)
        _checkAddonUpdate('wanxiang', wanxiang!.version),
      for (final entry in rimeAddons.entries)
        if (entry.value.installed)
          _checkAddonUpdate(entry.key, entry.value.status!.version),
    ]);
  }

  Future<void> _checkAddonUpdate(String id, String version) async {
    if (_addonUpdateChecks.containsKey(id)) return;
    final check = Object();
    _addonUpdateChecks[id] = check;
    try {
      final status = await core.checkAddonUpdate(id: id);
      final slot = rimeAddons[id];
      final current = switch (id) {
        'easy_en' => addon,
        'wanxiang' => wanxiang,
        _ => slot?.status,
      };
      if (_disposed ||
          !identical(_addonUpdateChecks[id], check) ||
          current?.installed != true ||
          current!.version != version) {
        return;
      }
      if (id == 'easy_en') {
        addonLatestVersion = status.latestVersion;
      } else if (slot != null) {
        slot.latestVersion = status.latestVersion;
      } else {
        wanxiangLatestVersion = status.latestVersion;
      }
      _changed();
    } catch (_) {}
  }

  Future<void> _readWindowsIme() async {
    try {
      windowsIme = await core.windowsImeStatus();
      windowsImeError = windowsIme!.registrationError;
    } catch (failure) {
      windowsImeError = errorText(failure);
      rethrow;
    }
  }

  Future<void> refreshWindowsIme() async {
    if (windowsRegistrationBusy) return;
    _managingWindowsIme = true;
    windowsImeError = null;
    _changed();
    try {
      await _readWindowsIme();
      addOperationLogs(['[WINDOWS IME] ${windowsIme!.message}']);
    } catch (failure) {
      addOperationLogs(['[WINDOWS IME ERROR] ${errorText(failure)}']);
    } finally {
      _managingWindowsIme = false;
      _changed();
      unawaited(_handleWindowsAction());
    }
  }

  Future<bool> setAppUpdatePrereleases(bool enabled) async {
    if (_disposed ||
        !canChangeAppUpdateChannel ||
        appUpdatePrereleases == enabled) {
      return false;
    }
    appUpdateBusy = true;
    _changed();
    try {
      await core.setAppUpdatePrereleases(enabled: enabled);
      if (_disposed) return false;
      _appUpdatePrereleases = enabled;
      appUpdate = null;
      appUpdateError = null;
      appUpdateActionError = null;
      _appUpdateDownloadedVersion = null;
      _appUpdateMissingPackageVersion = null;
      return true;
    } catch (failure) {
      if (!_disposed) {
        showToast(
          AppStrings.appUpdateChannelSaveFailure(
            PlatformDispatcher.instance.locale,
            errorText(failure),
          ),
        );
      }
      return false;
    } finally {
      appUpdateBusy = false;
      _changed();
    }
  }

  Future<AppUpdateDto?> checkAppUpdateNow() => _checkAppUpdate();

  Future<AppUpdateDto?> _checkAppUpdate({bool silent = false}) async {
    if (_disposed || checkingAppUpdate || appUpdateBusy) return null;
    checkingAppUpdate = true;
    appUpdateError = null;
    _changed();
    try {
      final update = await core.checkAppUpdate();
      if (isAndroid && update.hasUpdate) {
        try {
          _androidAppUpdateSource = await android.appUpdateSource();
        } on PlatformException {
          _androidAppUpdateSource = 'link';
        } on MissingPluginException {
          _androidAppUpdateSource = 'link';
        }
      }
      if (_disposed) return null;
      if (appUpdate?.latestVersion != update.latestVersion) {
        appUpdateActionError = null;
      }
      _appUpdateMissingPackageVersion = null;
      appUpdate = silent && !update.hasUpdate ? null : update;
      return update;
    } catch (failure) {
      if (!_disposed && !silent) appUpdateError = errorText(failure);
      return null;
    } finally {
      checkingAppUpdate = false;
      _changed();
    }
  }

  Future<void> _readInstalledPackages() async {
    final request = _schemeUpdatesRequest;
    final packages = await core.listInstalledPackages();
    if (_disposed || request != _schemeUpdatesRequest) return;
    LibjdStatusDto? libjdStatus;
    try {
      libjdStatus = await core.libjdStatus();
    } catch (failure) {
      debugPrint('ime-jd status refresh failed: ${errorText(failure)}');
    }
    if (_disposed || request != _schemeUpdatesRequest) return;
    installedPackages = packages;
    final logoVersionReads = <Future<bool>>[];
    for (final package in packages) {
      final logoPath = package.logoPath;
      if (logoPath != null) {
        _remoteSchemeLogos.remove(package.schemeKey);
        _remoteSchemeLogoFetchedAt.remove(package.schemeKey);
        _remoteSchemeLogoFailedAt.remove(package.schemeKey);
        logoVersionReads.add(_readSchemaLogoVersion(logoPath));
      }
    }
    if (logoVersionReads.isNotEmpty) {
      await Future.wait(logoVersionReads);
      if (_disposed || request != _schemeUpdatesRequest) return;
    }
    libjd = libjdStatus;
    schemeUpdates = [
      for (final update in schemeUpdates)
        if (packages.any(
          (package) =>
              package.schemeKey == update.schemeKey &&
              (package.version ?? '') == update.installedVersion,
        ))
          update,
    ];
  }

  Future<bool> refreshSchemeUpdates() async {
    if (_disposed || schemeUpdatesLoading) return false;
    final request = ++_schemeUpdatesRequest;
    schemeUpdatesLoading = true;
    _changed();
    try {
      await _readInstalledPackages();
      final dismissed = await core.getDismissedSchemeUpdates();
      final updates = await core.checkSchemeUpdates();
      if (_disposed || request != _schemeUpdatesRequest) return false;
      dismissedSchemeUpdates = dismissed;
      schemeUpdates = updates;
      return true;
    } catch (_) {
      // Update checks are optional and must not interrupt local package actions.
      return false;
    } finally {
      if (request == _schemeUpdatesRequest) {
        schemeUpdatesLoading = false;
        _changed();
      }
    }
  }

  void _invalidateSchemeUpdates() {
    _schemeUpdatesRequest++;
    schemeUpdatesLoading = false;
  }

  Future<bool> dismissSchemeUpdate(SchemeUpdateDto update) => run(() async {
    await core.dismissSchemeUpdate(
      schemeKey: update.schemeKey,
      version: update.latestVersion,
    );
    _invalidateSchemeUpdates();
    dismissedSchemeUpdates[update.schemeKey] = update.latestVersion;
  });

  void onResume() {
    _foreground = true;
    _startUserDictionaryPull(resumed: true);
    if (isMobile && busy) {
      _refreshOnResumeWhenIdle = true;
    } else {
      unawaited(_refreshOnResume());
    }
  }

  Future<void> _refreshOnResume() async {
    if (busy || _resumeRefreshInFlight || _disposed) return;
    final span = beginAppOperation(AppOperationName.refreshOnResume);
    _resumeRefreshInFlight = true;
    try {
      final previousSize = candidatePageSize;
      final previousEngineEnabled = inputEngineEnabled;
      await _readState(quiet: true, refreshPreview: false);
      if (_candidatePreviewTarget != _activePreviewTarget ||
          candidatePreviewPageSize == null ||
          previousSize != candidatePageSize ||
          previousEngineEnabled != inputEngineEnabled) {
        unawaited(refreshCandidatePreview());
      }
    } catch (failure) {
      debugPrint('Resume refresh failed: ${errorText(failure)}');
    } finally {
      span?.dispose();
      _resumeRefreshInFlight = false;
      _changed();
      _deployChangedScheme();
    }
  }

  void onPause() {
    _foreground = false;
    _dictionaryPullTimer?.cancel();
    _dictionarySyncDebounce?.cancel();
  }

  void _startUserDictionaryPull({bool resumed = false}) {
    _dictionaryPullTimer?.cancel();
    if (_disposed || !_foreground || accountUser == null) return;
    if (!resumed || _dictionaryPullCooldown?.isActive != true) {
      _scheduleUserDictionaryPull();
    }
    _dictionaryPullTimer = Timer.periodic(
      const Duration(minutes: 5),
      (_) => _scheduleUserDictionaryPull(),
    );
  }

  void _scheduleUserDictionaryPull() {
    if (installedPackages.any((package) => isKeytaoScheme(package.schemeKey))) {
      scheduleUserDictionarySync();
    }
  }

  void selectSchemeFamily(String family) {
    final installed = local == null
        ? null
        : installedSchemeKey(local!.schemas, local!.installedScheme);
    selectScheme(
      schemeFamily == family ? scheme : schemeForFamily(family, installed),
    );
  }

  void selectScheme(String value) {
    final known =
        schemes.containsKey(value) ||
        xmjdVariantSchemes.containsKey(value) ||
        installedPackageFor(value) != null;
    if (busy || !known) return;
    _manuallySelectedScheme = true;
    if (value == scheme) return;
    scheme = value;
    schemeRelease = null;
    latestRelease = null;
    error = null;
    _changed();
    unawaited(_persistSchemeSettings());
    unawaited(refreshRelease());
  }

  void selectSource(String value) {
    if (value == source || (value != 'github' && value != 'gitee')) {
      return;
    }
    source = value;
    _changed();
    unawaited(_persistSchemeSettings());
  }

  void selectWanxiangVariant(String? value) {
    if (busy ||
        value == null ||
        value == selectedWanxiangVariant ||
        !AppStrings.wanxiangVariants.containsKey(value)) {
      return;
    }
    _selectedWanxiangVariant = value;
    _changed();
  }

  Future<bool> refreshRelease() async {
    if (_disposed) return false;
    final request = ++_releaseRequest;
    final selected = scheme;
    releaseLoading = true;
    releaseError = null;
    latestRelease = null;
    schemeRelease = null;
    if (isLocalScheme(selected)) {
      releaseLoading = false;
      _changed();
      return true;
    }
    _changed();
    try {
      if (selected == 'keytao') {
        final release = await core.fetchLatestRelease();
        if (request == _releaseRequest && !_disposed) latestRelease = release;
      } else {
        final release = await core.fetchSchemeRelease(scheme: selected);
        if (request == _releaseRequest && !_disposed) schemeRelease = release;
      }
      return request == _releaseRequest && !_disposed;
    } catch (failure) {
      if (request == _releaseRequest && !_disposed) {
        releaseError = '${AppStrings.releaseFailure}${errorText(failure)}';
      }
      return false;
    } finally {
      if (request == _releaseRequest && !_disposed) {
        releaseLoading = false;
        _changed();
      }
    }
  }

  Future<void> _readStorage() async {
    storage = await android.storagePermissionStatus();
  }

  Future<void> _requireStorage({bool redirect = false}) async {
    checkingStorage = true;
    _changed();
    try {
      await _readStorage();
      if (!storageReady) {
        final message = storage?['message'] as String?;
        final failure = message == null || message.isEmpty
            ? AppStrings.storagePermission
            : message;
        if (redirect) {
          installError = failure;
          addOperationLogs(['[ANDROID PERMISSION] $failure']);
          try {
            await android.openStoragePermissionSettings();
          } catch (settingsFailure) {
            addOperationLogs([
              '[ANDROID PERMISSION] ${errorText(settingsFailure)}',
            ]);
          }
        }
        throw StateError(failure);
      }
    } finally {
      checkingStorage = false;
      _changed();
    }
  }

  void _startOperation(OperationKind kind, String message) {
    operation = kind;
    verification = [];
    installProgress = null;
    installFraction = null;
    progress = message;
    _changed();
  }

  Future<void> cancelDownload() async {
    if (canCancelDownload) await core.cancelSchemeDownload();
  }

  Future<bool> setInputEngine(String engine) => run(() async {
    packageError = null;
    try {
      if (isAndroid) await _requireStorage(redirect: true);
      await core.setInputEngine(engine: engine);
      if (engine == 'libjd_engine') _pendingSchemaSelectionPrompt = null;
      await _readInstalledPackages();
      await _readLocal();
      final key = activeSchemeKey;
      final hasInstalledPackage =
          installedPackageFor(key) != null ||
          isolatedPackages.any((p) => p.packageKey == key);
      if (engine == 'libjd_engine' && !isUsingLibjd && hasInstalledPackage) {
        _attemptedAutoConvertKey = key;
        await _convertSchemePackage(key);
        return;
      }
      if (engine == 'rime' && !hasUsableBuild && hasInstalledPackage) {
        _attemptedAutoDeployKey = key;
        await _deploy();
        return;
      }
      unawaited(refreshCandidatePreview());
    } catch (failure) {
      packageError = errorText(failure);
      rethrow;
    }
  }, switchingInput: true);

  Future<bool> convertSchemePackage(String key, String schemaId) =>
      run(() async {
        packageError = null;
        await _convertSchemePackage(key, schemaId: schemaId);
      });

  Future<void> _convertSchemePackage(String key, {String? schemaId}) async {
    final previousOperation = operation;
    final previousKey = installingSchemeKey;
    installingSchemeKey = key;
    _startOperation(OperationKind.libjd, '正在准备转换词库…');
    try {
      if (isAndroid) await _requireStorage(redirect: true);
      final skipped = await core.importLibjdPackage(
        key: key,
        schemaId: schemaId,
      );
      await _refreshAfterPackageSwitch(key);
      if (skipped > 0) showToast('已转换，跳过 $skipped 条不兼容词条');
    } catch (failure) {
      packageError = errorText(failure);
      // The downloaded source stays installed even if it cannot be converted.
      try {
        await _readInstalledPackages();
        await _readLocal();
      } catch (refreshFailure) {
        debugPrint('Scheme refresh failed: ${errorText(refreshFailure)}');
      }
      rethrow;
    } finally {
      installingSchemeKey = previousKey;
      operation = previousOperation;
    }
  }

  Future<void> cancelWanxiangGrammarDownload() async {
    if (canCancelWanxiangGrammarDownload) {
      await core.cancelWanxiangGrammarDownload();
    }
  }

  Future<bool> install() {
    if (prefersLibjd) return downloadSchemePackage(scheme);
    if (!canInstall || !requireRimeEngine()) return Future.value(false);
    return run(() async {
      final installing = scheme;
      await _install();
      if (verification.every((entry) => entry.ok)) {
        await _deferDeployForSchemaSelection(
          installing,
          activateOnConfirm: true,
        );
      }
    });
  }

  Future<void> _install() async {
    _invalidateSchemeUpdates();
    final url = downloadUrl!;
    final installing = scheme;
    final installingVersion = releaseVersion;
    _startOperation(OperationKind.install, AppStrings.installing);
    installError = null;
    deploySteps.clear();
    try {
      if (isAndroid) await _requireStorage(redirect: true);
      final InstallResultDto result;
      if (isAndroid) {
        final zipPath = await core.prepareAndroidInstall(url: url);
        try {
          final extracted = await android.smartExtractZipToPrivate(zipPath);
          result = await core.finishAndroidInstall(
            resultJson: jsonEncode(extracted),
            schemeKey: installing,
            version: installingVersion,
          );
        } finally {
          await core.removeDownloadedArchive(path: zipPath);
        }
      } else {
        result = await core.installSchemeFromUrl(
          url: url,
          schemeKey: installing,
          version: installingVersion,
        );
      }
      _recordInstallResult(result);
      verification = result.verify;
      _invalidateSchemeUpdates();
      await _readInstalledPackages();
      await _readLocal();
      await _readUserWordsReport(installing);
      scheduleSchemeProfileSync(packageKey: installing);
      progress = AppStrings.installDone;
      final broken = [
        for (final line in result.logs.expand((log) => log.split('\n')))
          if (line.startsWith('[BROKEN] ')) line.substring('[BROKEN] '.length),
      ];
      if (broken.isNotEmpty) installError = broken.join('\n');
    } catch (failure) {
      final message = errorText(failure);
      if (message == AppStrings.downloadCancelled) {
        progress = message;
      } else {
        progress = AppStrings.installFailed;
        installError = message;
        addOperationLogs(['[ERROR] $installError']);
      }
      rethrow;
    } finally {
      operation = null;
    }
  }

  Future<bool> installSchemePackage(String key) => _installSchemePackage(
    key,
    latest: installedPackageFor(key)?.active == true,
  );

  Future<bool> _installSchemePackage(String key, {required bool latest}) {
    if (!schemeAssets.containsKey(key)) return Future.value(false);
    if (prefersLibjd) return downloadSchemePackage(key);
    return run(() async {
      installingSchemeKey = key;
      try {
        packageError = null;
        _manuallySelectedScheme = true;
        scheme = key;
        if (!await refreshRelease()) {
          throw StateError(releaseError ?? AppStrings.installFailed);
        }
        if (latest && key == 'keytao') {
          final version = _normalizedVersion(latestRelease!.version);
          final matching = sourceVersions.entries.where(
            (entry) =>
                _normalizedVersion(entry.value) == version &&
                _platformDownloadUrl(
                      entry.key == 'github'
                          ? latestRelease!.github
                          : latestRelease!.gitee,
                    )?.isNotEmpty ==
                    true,
          );
          if (matching.isEmpty) throw StateError(AppStrings.installFailed);
          if (!matching.any((entry) => entry.key == source)) {
            source = matching.first.key;
          }
        }
        if (downloadUrl?.isNotEmpty != true) {
          throw StateError(AppStrings.installFailed);
        }
        await _install();
        if (verification.every((entry) => entry.ok)) {
          if (!await _deferDeployForSchemaSelection(
            key,
            activateOnConfirm: true,
          )) {
            await _switchSchemePackage(key);
          }
        }
      } finally {
        installingSchemeKey = null;
      }
    }).then((result) => result && verification.every((entry) => entry.ok));
  }

  Future<bool> updateSchemePackage(String key) {
    final package = installedPackageFor(key);
    if (package == null) return Future.value(false);
    return downloadSchemePackage(key);
  }

  Future<bool> downloadSchemePackage(String key) {
    return run(() async {
      installingSchemeKey = key;
      packageError = null;
      _invalidateSchemeUpdates();
      _startOperation(OperationKind.install, AppStrings.installing);
      try {
        if (isAndroid) await _requireStorage(redirect: true);
        await core.downloadSchemePackage(schemeKey: key, source: source);
        if (prefersLibjd) {
          await _readInstalledPackages();
          final defer = await _deferDeployForSchemaSelection(
            key,
            activateOnConfirm: true,
            convert: true,
          );
          if (!defer) {
            await _convertSchemePackage(key);
          } else {
            await _readLocal();
          }
        } else {
          final defer = await _deferDeployForSchemaSelection(
            key,
            activateOnConfirm: true,
          );
          if (!defer) await _switchSchemePackage(key);
          await _readInstalledPackages();
          if (defer) await _readLocal();
        }
        _invalidateSchemeUpdates();
        await _readUserWordsReport(key);
        scheduleSchemeProfileSync(packageKey: key);
        progress = AppStrings.installDone;
      } catch (failure) {
        final message = errorText(failure);
        if (message == AppStrings.downloadCancelled) {
          progress = message;
        } else {
          packageError = message;
          progress = AppStrings.installFailed;
        }
        rethrow;
      } finally {
        installingSchemeKey = null;
        operation = null;
      }
    });
  }

  Future<bool> importLocalScheme({
    String? schemeKey,
    bool fromArchive = false,
    Future<List<String>?> Function(LocalImportInspectionDto inspection)?
    selectSchemas,
  }) async {
    final wasActive = installedPackageFor(schemeKey ?? '')?.active == true;
    String? importedKey;
    var deferred = false;
    final result = await run(() async {
      packageError = null;
      String? temporaryDirectory;
      try {
        final String sourcePath;
        final String displayName;
        if (isMobile) {
          final selected = fromArchive
              ? (isAndroid
                    ? await android.importLocalSchemeArchive()
                    : await const IosHost().importLocalSchemeArchive())
              : (isAndroid
                    ? await android.importLocalSchemeDirectory()
                    : await const IosHost().importLocalSchemeDirectory());
          if (selected == null) return;
          temporaryDirectory =
              selected['temporaryDirectory'] as String? ??
              selected['path'] as String;
          sourcePath = selected['path'] as String;
          displayName = selected['name'] as String;
        } else if (fromArchive) {
          final selected = await pickDesktopArchive();
          if (selected == null) return;
          sourcePath = selected;
          final segments = selected
              .split(RegExp(r'[/\\]'))
              .where((segment) => segment.isNotEmpty);
          final rawName = segments.isEmpty ? 'local-scheme' : segments.last;
          displayName =
              rawName.toLowerCase().endsWith('.zip') && rawName.length > 4
              ? rawName.substring(0, rawName.length - 4)
              : rawName;
        } else {
          final selected = await pickDesktopDirectory();
          if (selected == null) return;
          sourcePath = selected;
          final segments = selected
              .split(RegExp(r'[/\\]'))
              .where((segment) => segment.isNotEmpty);
          displayName = segments.isEmpty ? 'local-scheme' : segments.last;
        }
        List<String>? selectedSchemas;
        var userSelectedBeforeImport = false;
        if (schemeKey == null && selectSchemas != null) {
          final inspection = await core.inspectLocalScheme(
            directoryPath: sourcePath,
          );
          if (inspection.totalSchemas > 2) {
            final chosen = await selectSchemas(inspection);
            if (chosen == null || chosen.isEmpty) return;
            selectedSchemas = chosen;
            userSelectedBeforeImport = true;
          }
        }
        final key = await core.importLocalScheme(
          directoryPath: sourcePath,
          displayName: displayName,
          schemeKey: schemeKey,
          selectedSchemas: selectedSchemas,
        );
        importedKey = key;
        _pendingCloudPackageUploads.remove(key);
        _invalidateSchemeUpdates();
        if (prefersLibjd) {
          await _readInstalledPackages();
          if (!userSelectedBeforeImport) {
            deferred = await _deferDeployForSchemaSelection(
              key,
              activateOnConfirm: true,
              convert: true,
            );
          }
          if (!deferred) {
            await _convertSchemePackage(key);
          } else {
            await _readLocal();
          }
          scheduleSchemeProfileSync();
          return;
        }
        await _readInstalledPackages();
        await _readLocal();
        _manuallySelectedScheme = true;
        scheme = key;
        await refreshRelease();
        await _persistSchemeSettings(schedule: false);
        if (!userSelectedBeforeImport) {
          deferred = await _deferDeployForSchemaSelection(
            key,
            activateOnConfirm: true,
          );
        }
        if (!deferred && wasActive) {
          await _switchSchemePackage(key);
        } else {
          scheduleSchemeProfileSync();
        }
      } catch (failure) {
        packageError = errorText(failure);
        rethrow;
      } finally {
        if (temporaryDirectory != null) {
          try {
            await Directory(temporaryDirectory).delete(recursive: true);
          } on FileSystemException catch (failure) {
            addOperationLogs(['[WARN] ${errorText(failure)}']);
          }
        }
      }
    });
    return result &&
        importedKey != null &&
        (prefersLibjd ||
            !wasActive ||
            deferred ||
            verification.every((entry) => entry.ok));
  }

  Future<bool> importGitScheme({
    required String repoUrl,
    required String mode,
    String? branch,
    String? assetName,
    String displayName = '',
    String? schemeKey,
    bool activate = true,
  }) async {
    final existingActive = installedPackageFor(schemeKey ?? '')?.active == true;
    final shouldActivate =
        !prefersLibjd && (schemeKey == null ? activate : existingActive);
    String? importedKey;
    var deferred = false;
    final result = await run(() async {
      packageError = null;
      if (schemeKey != null) installingSchemeKey = schemeKey;
      _startOperation(OperationKind.install, '正在从 Git 下载方案…');
      try {
        if (isAndroid) await _requireStorage(redirect: true);
        final key = await core.importGitScheme(
          repoUrl: repoUrl,
          mode: mode,
          branch: branch,
          assetName: assetName,
          displayName: displayName,
          schemeKey: schemeKey,
        );
        importedKey = key;
        _pendingCloudPackageUploads.remove(key);
        _invalidateSchemeUpdates();
        await _readInstalledPackages();
        if (_rememberGitScheme(
          schemeKey: key,
          repoUrl: repoUrl,
          mode: mode,
          branch: branch,
          assetName: assetName,
          displayName: displayName,
        )) {
          await _persistSchemeSettings(schedule: false);
        }
        if (prefersLibjd) {
          deferred = await _deferDeployForSchemaSelection(
            key,
            activateOnConfirm: true,
            convert: true,
          );
          if (!deferred) {
            await _convertSchemePackage(key);
          } else {
            await _readLocal();
          }
        } else {
          await _readLocal();
          _manuallySelectedScheme = true;
          scheme = key;
          await refreshRelease();
          await _persistSchemeSettings(schedule: false);
          deferred = await _deferDeployForSchemaSelection(
            key,
            activateOnConfirm: shouldActivate,
          );
        }
        if (!deferred && shouldActivate) {
          await _switchSchemePackage(key);
        } else {
          scheduleSchemeProfileSync();
        }
        progress = AppStrings.installDone;
      } catch (failure) {
        final message = errorText(failure);
        if (message == AppStrings.downloadCancelled) {
          progress = message;
        } else {
          packageError = message;
          progress = AppStrings.installFailed;
        }
        rethrow;
      } finally {
        installingSchemeKey = null;
        operation = null;
      }
    });
    return result &&
        importedKey != null &&
        (!shouldActivate ||
            deferred ||
            verification.every((entry) => entry.ok));
  }

  Future<bool> activateSchemePackage(String key) async {
    var package = installedPackageFor(key);
    final addonStatus = switch (key) {
      'easy_en' => addon,
      'rime_ice' => rimeAddons[key]!.status,
      _ => null,
    };
    if (prefersLibjd &&
        (package?.hasPackageZip == true ||
            addonStatus?.installed == true ||
            usesImportedLibjd(key))) {
      return (activeSchemeKey != key || !isUsingLibjd) &&
          await switchPackage(key);
    }
    if (package?.active == true && package!.inUse) return false;
    if (addonStatus?.installed != true && package?.hasPackageZip != true) {
      if (package == null || isLocalScheme(key)) return false;
      if (!await downloadSchemePackage(key)) return false;
      package = installedPackageFor(key);
      if (package?.hasPackageZip != true) return false;
      if (package!.inUse || hasPendingSchemaSelectionPrompt) return true;
    }
    return switchPackage(key);
  }

  Future<List<PackageSchemaDto>> packageSchemas(String key) =>
      core.packageSchemas(schemeKey: key);

  Future<bool> setPackageSchemas(
    String key,
    List<String> ids, {
    bool activate = false,
  }) => run(() async {
    packageError = null;
    _startOperation(OperationKind.install, AppStrings.installing);
    try {
      if (isAndroid &&
          (activate || (installedPackageFor(key)?.active ?? true))) {
        await _requireStorage(redirect: true);
      }
      final shouldDeploy = await core.setPackageSchemas(
        schemeKey: key,
        ids: ids,
      );
      await _readInstalledPackages();
      if (activate) {
        await _switchSchemePackage(key);
      } else if (shouldDeploy) {
        await _readLocal();
        if (local?.installed != true) {
          await _switchSchemePackage(key);
        } else {
          await _deploy(schemeKey: key);
        }
      }
    } catch (failure) {
      packageError = errorText(failure);
      rethrow;
    } finally {
      operation = null;
    }
  });

  Future<void> _switchSchemePackage(String key, {String? schemaId}) async {
    packageError = null;
    if (prefersLibjd && !usesImportedLibjd(key)) {
      await _convertSchemePackage(key, schemaId: schemaId);
      return;
    }
    deployingSchemeKey = key;
    _startOperation(
      OperationKind.deploy,
      usesLibjd(key) ? '正在切换方案…' : AppStrings.deployingLibrime,
    );
    try {
      if (isAndroid) await _requireStorage(redirect: true);
      await core.switchPackage(
        packageKey: key,
        schemaId: prefersLibjd ? null : schemaId,
        androidDeploy: _androidDeploy,
      );
      await _refreshAfterPackageSwitch(key);
    } catch (failure) {
      packageError = errorText(failure);
      try {
        await Future.wait([_readInstalledPackages(), _readLocal()]);
      } catch (refreshFailure) {
        debugPrint(
          'Package switch state refresh failed: ${errorText(refreshFailure)}',
        );
      }
      rethrow;
    } finally {
      deployingSchemeKey = null;
      operation = null;
    }
  }

  Future<bool> switchPackage(String key, {String? schemaId}) => run(
    () => _switchSchemePackage(key, schemaId: schemaId),
    switchingInput: true,
  );

  Future<bool> selectPackageSchema(String packageKey, String schemaId) {
    if (prefersLibjd) {
      return switchPackage(packageKey, schemaId: schemaId);
    }
    final active = isolatedPackages.where((pkg) => pkg.active).firstOrNull;
    if (active != null && active.packageKey == packageKey && active.ready) {
      return selectSchema(schemaId);
    }
    return switchPackage(packageKey, schemaId: schemaId);
  }

  Future<void> _refreshAfterPackageSwitch(String key) async {
    verification = [];
    if (schemeAssets.containsKey(key) || isLocalScheme(key)) {
      _manuallySelectedScheme = true;
      scheme = key;
      latestRelease = null;
      schemeRelease = null;
    }
    _invalidateSchemeUpdates();
    await _readInstalledPackages();
    await _readLocal();
    if (isMobile) {
      await _checkRecommendedKeyboardAfterSchemaSwitch(currentSchemaId ?? key);
    }
    if (key == 'wanxiang') {
      wanxiang = await core.wanxiangStatus();
      await _readWanxiangGrammar();
    } else if (rimeAddons[key] case final slot? when key != 'rime_ice') {
      slot.status = await slot._readStatus();
    }
    scheduleSchemeProfileSync(
      packageKey: key,
      afterSuccessfulDeploy: !prefersLibjd,
    );
    unawaited(refreshCandidatePreview());
  }

  Future<void> startPackageMigration({VoidCallback? onPending}) async {
    _packageMigrationFailed = false;
    try {
      if (await core.packageMigrationPending()) {
        onPending?.call();
        if (isAndroid) await android.prepareRimeData();
      }
      await core.migrateToIsolated();
      try {
        migrationBackupDir = await core.pendingMigrationBackupDir();
      } on UnimplementedError catch (_) {}
      _changed();
    } catch (failure) {
      final text = errorText(failure);
      _recordMigrationFailure(text);
      addOperationLogs(['[ERROR] $text']);
      _changed();
    }
  }

  Future<void> dismissMigrationBackupNotice() async {
    migrationBackupDir = null;
    _changed();
    try {
      await core.dismissMigrationBackupNotice();
    } catch (failure) {
      debugPrint(
        'Dismiss migration backup notice failed: ${errorText(failure)}',
      );
    }
  }

  Future<bool> retryPackageMigration() => run(() async {
    _startOperation(OperationKind.migrate, AppStrings.migratingPackageDirs);
    try {
      await startPackageMigration();
      if (!packageMigrationFailed) await _readState();
    } finally {
      operation = null;
    }
  }).then((ran) => ran && !packageMigrationFailed);

  Future<bool> repairSchemePackage(String key) {
    if (!requireRimeEngine()) return Future.value(false);
    return run(() async {
      packageError = null;
      installingSchemeKey = key;
      _invalidateSchemeUpdates();
      _startOperation(
        OperationKind.install,
        isLocalScheme(key) ? '正在修复方案…' : '正在备份并修复方案…',
      );
      try {
        if (isAndroid) await _requireStorage(redirect: true);
        final backup = await core.repairSchemePackage(
          packageKey: key,
          androidDeploy: _androidDeploy,
        );
        if (backup != null && backup.isNotEmpty) {
          addOperationLogs(['[REPAIR] 已备份异常文件至 $backup 并重新下载修复']);
          showToast('已备份异常文件并重新下载修复方案');
        } else {
          addOperationLogs(['[REPAIR] 已重新部署方案 $key']);
          showToast('已重新部署方案');
        }
        await _readState();
      } catch (failure) {
        packageError = errorText(failure);
        rethrow;
      } finally {
        installingSchemeKey = null;
        operation = null;
      }
    });
  }

  Future<bool> removeSchemePackage(String key) {
    final package = installedPackageFor(key);
    if (package == null) return Future.value(false);
    return run(() async {
      packageError = null;
      _invalidateSchemeUpdates();
      try {
        final kept = await core.removeSchemePackage(
          schemeKey: key,
          androidDeploy: _androidDeploy,
        );
        addOperationLogs(
          kept.map((path) => '[KEPT] User-modified file: $path'),
        );
        _reportKeptPackageFiles(kept);
        _invalidateSchemeUpdates();
        if (package.active) {
          await _readState();
        } else {
          await _readInstalledPackages();
        }
        if (package.active || scheme == key) {
          _manuallySelectedScheme = true;
          scheme =
              installedPackages
                  .where((package) => package.active)
                  .firstOrNull
                  ?.schemeKey ??
              (package.active
                  ? installedPackages.firstOrNull?.schemeKey
                  : null) ??
              'keytao';
          await refreshRelease();
          await _persistSchemeSettings(schedule: false);
        }
        scheduleSchemeProfileSync();
      } catch (failure) {
        packageError = errorText(failure);
        if (package.active) {
          try {
            await _readState();
          } catch (_) {}
        }
        rethrow;
      }
    });
  }

  Future<bool> releasePackageCache(String key, {required bool imeJd}) =>
      run(() async {
        packageError = null;
        try {
          await core.releasePackageCache(schemeKey: key, imeJd: imeJd);
          _invalidateSchemeUpdates();
          await _readInstalledPackages();
          await refreshIsolatedPackages(notify: false);
          if (key == activeSchemeKey) {
            await _readLocal();
          }
        } catch (failure) {
          packageError = errorText(failure);
          rethrow;
        }
      });

  Future<PackageOrphanReportDto> packageOrphanReport() =>
      core.packageOrphanReport();

  Future<bool> trashPackageOrphans(
    List<PackageOrphanFileDto> files, [
    List<PackageKeptBackupDto> keptBackups = const [],
  ]) => run(() async {
    final moved = await core.trashPackageOrphans(
      files: files,
      keptBackups: keptBackups,
    );
    addOperationLogs(moved.map((path) => '[TRASH] $path'));
  });

  void _recordInstallResult(InstallResultDto result) {
    addOperationLogs(result.logs);
    const keptPrefix = '[KEPT] User-modified file: ';
    _reportKeptPackageFiles([
      for (final line in result.logs.expand((log) => log.split('\n')))
        if (line.startsWith(keptPrefix)) line.substring(keptPrefix.length),
    ]);
    addOperationLogs(
      result.verify
          .where((entry) => !entry.ok)
          .map((entry) => '[VERIFY FAIL] ${entry.path}: ${entry.note}'),
    );
  }

  void _reportKeptPackageFiles(Iterable<String> paths) {
    final files = paths.join('\n');
    if (files.isNotEmpty) packageError = '已保留您修改过的文件：\n$files';
  }

  Future<bool> deploy({int? imeActionId, String? schemeKey}) =>
      run(() => _deploy(imeActionId: imeActionId, schemeKey: schemeKey));

  Future<void> _readInputEngineEnabled() async {
    try {
      inputEngineEnabled = await core.inputEngineEnabled();
    } catch (failure) {
      debugPrint('Input engine state refresh failed: ${errorText(failure)}');
    }
  }

  Future<bool> setInputEngineEnabled(bool enabled) => run(() async {
    inputEngineTransition = enabled;
    _changed();
    try {
      if (isAndroid) await _requireStorage();
      await core.setInputEngineEnabled(enabled: enabled);
      inputEngineEnabled = enabled;
    } catch (_) {
      await _readInputEngineEnabled();
      rethrow;
    } finally {
      inputEngineTransition = null;
      unawaited(refreshCandidatePreview());
    }
  });

  Future<void> _deploy({int? imeActionId, String? schemeKey}) async {
    _pendingCloudPackageUploads.clear();
    _packageDeployProgress.clear();
    deployingSchemeKey =
        schemeKey ??
        installedPackages
            .where((package) => package.inUse)
            .map((package) => package.schemeKey)
            .firstOrNull ??
        (activeSchemeDownloaded ? activeSchemeKey : null);
    if (_activeBuildHasChanges && deployingSchemeKey == activeSchemeKey) {
      _attemptedAutoDeployKey = activeSchemeKey;
    }
    _userWordsDeployDebounce?.cancel();
    final deploying = prefersLibjd
        ? AppStrings.deployingLibjd
        : AppStrings.deployingLibrime;
    _startOperation(OperationKind.deploy, deploying);
    deploySteps.clear();
    try {
      if (!activeSchemeDownloaded) {
        throw StateError(
          _localSchemaError == null
              ? AppStrings.noSchemeForDeploy
              : AppStrings.localCheckFailure(_localSchemaError!),
        );
      }
      _deployStep(deploying, DeployStepState.neutral);
      final String message;
      String? warning;
      if (isAndroid) await _requireStorage();
      if (isAndroid && !prefersLibjd) {
        Map<String, dynamic>? result;
        await core.deployAndroid(
          androidDeploy: () async {
            try {
              final response = await android.deployImeData();
              result = response;
              if (response['deployed'] == true) return '';
              final text = response['message'] as String?;
              return text != null && text.isNotEmpty
                  ? text
                  : AppStrings.deployIncomplete;
            } catch (failure) {
              return errorText(failure);
            }
          },
        );
        message = result?['message'] as String? ?? AppStrings.deployDone;
        warning = result?['warning'] as String?;
      } else {
        final result = imeActionId == null || prefersLibjd
            ? await core.deployDefault()
            : await core.windowsRedeployImeAction(id: imeActionId);
        if (!result.success) throw StateError(result.message);
        message = result.message;
        warning = result.warning;
      }
      await _readState();
      if (!prefersLibjd &&
          !hasPackageDeployProgress &&
          local?.deployed != true) {
        throw StateError(AppStrings.deployCheckFailed);
      }
      final effectiveWarning = warning?.isNotEmpty == true
          ? warning
          : local?.deployWarning;
      deployWarning = effectiveWarning;
      _deployStep(message, DeployStepState.ok);
      if (effectiveWarning != null && effectiveWarning.isNotEmpty) {
        if (!deploySteps.any((step) => step.message == effectiveWarning)) {
          _deployStep(effectiveWarning, DeployStepState.neutral);
        }
        addOperationLogs(['[WARN] $effectiveWarning']);
      }
      userWordsPendingDeploy.removeWhere(userWordsAffectActiveScheme);
      addOperationLogs(['[DEPLOY] $message']);
      progress = message;
      scheduleSchemeProfileSync(
        packageKey: activeSchemeKey,
        afterSuccessfulDeploy: true,
      );
    } catch (failure) {
      progress = AppStrings.deployFailed;
      _deployStep(errorText(failure), DeployStepState.failed);
      addOperationLogs(['[DEPLOY ERROR] ${errorText(failure)}']);
      rethrow;
    } finally {
      _packageDeployProgress.clear();
      deployingSchemeKey = null;
      operation = null;
    }
  }

  String? get accountName =>
      accountUser?.nickname ?? accountUser?.name ?? accountUser?.email;

  void _setAccountUser(AccountUserDto? value) {
    final previous = accountUser?.id;
    accountUser = value;
    if (previous == value?.id) return;
    _dictionaryPullCooldown?.cancel();
    _schemeSyncDebounce?.cancel();
    _readySchemeSyncAccount = null;
    _checkingSchemeSyncAccount = null;
    _checkedSchemeSyncAccounts.clear();
    pendingCloudProfile = null;
    pendingCloudActiveScheme = null;
    cloudPackages = [];
    if (previous != null) {
      _pendingCloudPackageUploads.clear();
      _schemeProfileDirty = false;
    }
  }

  Future<void> _loadAccount() async {
    await loadSchemeSyncState();
    try {
      _setAccountUser(await core.accountSession());
      _changed();
      await core.accountRefresh();
    } catch (failure) {
      debugPrint('Account refresh failed: ${errorText(failure)}');
    } finally {
      await reloadAccountSession();
      _startUserDictionaryPull();
    }
  }

  Future<void> reloadAccountSession({bool checkSchemeProfile = true}) async {
    try {
      _setAccountUser(await core.accountSession());
      if (_pendingDictionarySyncAccountId != accountUser?.id) {
        pendingDictionarySync = null;
        _pendingDictionarySyncAccountId = null;
      }
      _changed();
      if (checkSchemeProfile) await checkCloudProfileAfterLogin();
    } catch (failure) {
      debugPrint('Account session failed: ${errorText(failure)}');
    }
  }

  Future<bool> login(String name, String password) async {
    if (loggingIn) return false;
    loggingIn = true;
    accountError = null;
    accountMessage = null;
    _changed();
    try {
      _setAccountUser(await core.accountLogin(name: name, password: password));
      pendingDictionarySync = null;
      _pendingDictionarySyncAccountId = null;
      _startUserDictionaryPull();
      await loadSchemeSyncState();
      await checkCloudProfileAfterLogin();
      return true;
    } catch (failure) {
      accountError = errorText(failure);
      return false;
    } finally {
      loggingIn = false;
      _changed();
    }
  }

  Future<void> logout() async {
    _dictionarySyncDebounce?.cancel();
    _dictionaryPullTimer?.cancel();
    _schemeSyncDebounce?.cancel();
    accountError = null;
    accountMessage = null;
    try {
      await core.accountLogout();
      _setAccountUser(null);
      pendingDictionarySync = null;
      _pendingDictionarySyncAccountId = null;
      pendingCloudProfile = null;
      pendingCloudActiveScheme = null;
      _readySchemeSyncAccount = null;
      cloudPackages = [];
    } catch (failure) {
      accountError = errorText(failure);
    }
    _changed();
  }

  void _readSchemeSyncState(String value) {
    final state = jsonDecode(value) as Map<String, dynamic>;
    followAccountSync = state['follow_account_sync'] == true;
    syncedSchemes
      ..clear()
      ..addAll((state['synced_schemes'] as List? ?? []).cast<String>());
    lastSchemeSyncAt = state['last_sync_at'] as String?;
    _schemeSettings = Map<String, dynamic>.from(
      state['settings'] as Map? ?? const {},
    );
    if (_schemeSettings['download_source'] case final String value) {
      source = value;
    }
    if (_schemeSettings['selected_scheme'] case final String value) {
      if (value != scheme) {
        scheme = value;
        schemeRelease = null;
        latestRelease = null;
        if (_startupStarted) unawaited(refreshRelease());
      }
      _manuallySelectedScheme = true;
    }
  }

  Map<String, dynamic> _schemeSettings = {};

  Future<void> loadSchemeSyncState() async {
    try {
      _readSchemeSyncState(await core.getSchemeSyncState());
      if (_rememberInstalledGitSchemes()) {
        await _persistSchemeSettings(schedule: false);
      }
      _changed();
    } catch (failure) {
      schemeSyncError = errorText(failure);
      _changed();
    }
  }

  Map<String, dynamic> _schemeSettingsSnapshot() {
    final input = androidSettings;
    final ui = uiSettings;
    return {
      ..._schemeSettings,
      'download_source': source,
      'selected_scheme': scheme,
      if (input != null)
        'android': {
          'haptics_enabled': input.hapticsEnabled,
          'haptic_intensity': input.hapticIntensity,
          'enter_key_behavior': input.enterKeyBehavior,
          'key_preview_enabled': input.keyPreviewEnabled,
          'long_press_delay_ms': input.longPressDelayMs,
          'keyboard_height_scale': input.keyboardHeightScale,
          'delete_speed': input.deleteSpeed,
          'english_mode': input.englishMode,
          'backspace_gesture_mode': input.backspaceGestureMode,
          'keyboard_height_dp': input.keyboardHeightDp,
          'candidate_bar_height_dp': input.candidateBarHeightDp,
          'swipe_threshold_dp': input.swipeThresholdDp,
          'key_sound_enabled': input.keySoundEnabled,
          'key_sound_volume': input.keySoundVolume,
          'key_hint_visible': input.keyHintVisible,
          'mnemonic_hints_enabled': input.mnemonicHintsEnabled,
          'flick_keys_enabled': input.flickKeysEnabled,
          'number_row_enabled': input.numberRowEnabled,
          'candidate_font_scale': input.candidateFontScale,
          'double_space_period_enabled': input.doubleSpacePeriodEnabled,
          'symbol_auto_return': input.symbolAutoReturn,
          'phrases': input.phrases,
          'clipboard_history_enabled': input.clipboardHistoryEnabled,
          'clipboard_history_limit': input.clipboardHistoryLimit,
          'clipboard_suggestion_enabled': input.clipboardSuggestionEnabled,
          'clipboard_suggestion_timeout_sec':
              input.clipboardSuggestionTimeoutSec,
          'clipboard_return_after_paste': input.clipboardReturnAfterPaste,
          'clipboard_hide_sensitive': input.clipboardHideSensitive,
          'floating_portrait_enabled': input.floatingPortraitEnabled,
          'floating_portrait_scale': input.floatingPortraitScale,
          'floating_landscape_enabled': input.floatingLandscapeEnabled,
          'floating_landscape_scale': input.floatingLandscapeScale,
        },
      if (ui != null)
        'ime_ui': {
          'color_scheme': ui.colorScheme.name,
          if (ui.accentColorOverride != null)
            'accent_color': ui.accentColorOverride,
          'corner_radius': ui.cornerRadius,
          'background_color': ui.backgroundColor,
          if (!isMobile) ...{
            'orientation': ui.orientation.name,
            'font_size': ui.fontSize,
            'embedded_composition': ui.embeddedComposition,
            'hide_candidate_panel': ui.hideCandidatePanel,
          },
          if (isWindows) 'show_tray_mode_indicator': ui.showTrayModeIndicator,
        },
      'desktop_english_mode': desktopEnglishMode.name,
      if (!isMobile) 'desktop_english_switch_key': desktopEnglishSwitchKey,
    };
  }

  Future<void> _persistSchemeSettings({bool schedule = true}) {
    if (_applyingCloudProfile || _disposed) return Future.value();
    final snapshot = _schemeSettingsSnapshot();
    final write = (_schemeSettingsWrite ?? Future<void>.value()).then((
      _,
    ) async {
      try {
        _readSchemeSyncState(
          await core.setSchemeSyncPreferences(
            settingsJson: jsonEncode(snapshot),
          ),
        );
        if (schedule) scheduleSchemeProfileSync();
      } catch (failure) {
        schemeSyncError = errorText(failure);
      }
      _changed();
    });
    _schemeSettingsWrite = write;
    return write;
  }

  Future<bool> setFollowAccountSync(bool enabled) async {
    final result = await _schemeCloudAction(() async {
      _readSchemeSyncState(
        await core.setSchemeSyncPreferences(followAccountSync: enabled),
      );
    });
    if (!enabled) _schemeSyncDebounce?.cancel();
    if (result && enabled) {
      await checkCloudProfileAfterLogin();
      scheduleSchemeProfileSync();
    }
    return result;
  }

  Future<bool> setSchemeAccountSync(String key, bool enabled) async {
    final result = await _schemeCloudAction(() async {
      _readSchemeSyncState(
        await core.setSchemeSyncPreferences(schemeKey: key, sync_: enabled),
      );
    });
    if (result) {
      if (!enabled) _pendingCloudPackageUploads.remove(key);
      scheduleSchemeProfileSync(packageKey: enabled ? key : null);
    }
    return result;
  }

  String _comparableProfile(Object? value) {
    Object? sorted(Object? item) {
      if (item is num) return item.toDouble();
      if (item is Map) {
        final keys = item.keys.cast<String>().toList()..sort();
        return {for (final key in keys) key: sorted(item[key])};
      }
      if (item is List) {
        final result = item.map(sorted).toList();
        if (result.every(
          (entry) => entry is Map && entry['scheme_key'] != null,
        )) {
          result.sort(
            (a, b) => '${(a as Map)['scheme_key']}'.compareTo(
              '${(b as Map)['scheme_key']}',
            ),
          );
        }
        return result;
      }
      return item;
    }

    return jsonEncode(sorted(value));
  }

  bool _cloudProfileDiffers(Map cloud, Map local) {
    Map<String, dynamic> normalize(Map raw, Map other) {
      final copy = jsonDecode(jsonEncode(raw)) as Map<String, dynamic>;
      final settings = copy['settings'];
      final otherSettings = other['settings'];
      if (settings is Map<String, dynamic> && otherSettings is Map) {
        if (!isMobile && !otherSettings.containsKey('android')) {
          settings.remove('android');
        }
        if (isMobile) {
          for (final key in const [
            'desktop_english_mode',
            'desktop_english_switch_key',
          ]) {
            if (!otherSettings.containsKey(key)) settings.remove(key);
          }
        } else if (settings['desktop_english_switch_key'] == 'schema' &&
            !otherSettings.containsKey('desktop_english_switch_key')) {
          settings.remove('desktop_english_switch_key');
        }
        final ui = settings['ime_ui'];
        final otherUi = otherSettings['ime_ui'];
        if (ui is Map<String, dynamic> && otherUi is Map) {
          if (isMobile) {
            for (final key in const [
              'orientation',
              'font_size',
              'embedded_composition',
              'hide_candidate_panel',
              'show_tray_mode_indicator',
            ]) {
              if (!otherUi.containsKey(key)) ui.remove(key);
            }
          } else if (!isWindows &&
              !otherUi.containsKey('show_tray_mode_indicator')) {
            ui.remove('show_tray_mode_indicator');
          }
        }
      }
      return copy;
    }

    final normalizedCloud = normalize(cloud, local);
    final normalizedLocal = normalize(local, cloud);
    return _comparableProfile(normalizedCloud) !=
        _comparableProfile(normalizedLocal);
  }

  Future<void> checkCloudProfileAfterLogin() async {
    final account = accountUser?.id;
    if (_disposed ||
        account == null ||
        _checkedSchemeSyncAccounts.contains(account) ||
        _checkingSchemeSyncAccount == account) {
      return;
    }
    _checkingSchemeSyncAccount = account;
    _schemeSyncDebounce?.cancel();
    try {
      if (followAccountSync) {
        final envelope = jsonDecode(await core.fetchSchemeProfile()) as Map;
        final profile = envelope['profile'];
        await _persistSchemeSettings(schedule: false);
        final local = jsonDecode(await core.exportSchemeProfile());
        if (accountUser?.id != account || _disposed) return;
        _readySchemeSyncAccount = account;
        _checkedSchemeSyncAccounts.add(account);
        if (envelope['updateAt'] case final String time) {
          lastSchemeSyncAt = time;
        }
        if (profile is Map &&
            local is Map &&
            _cloudProfileDiffers(profile, local)) {
          pendingCloudProfile = Map<String, dynamic>.from(profile);
        }
      }
      schemeSyncError = null;
      try {
        final packages = (jsonDecode(await core.listCloudPackages()) as List)
            .map((value) => Map<String, dynamic>.from(value as Map))
            .toList();
        if (accountUser?.id == account) cloudPackages = packages;
      } catch (failure) {
        if (accountUser?.id == account) schemeSyncError = errorText(failure);
      }
    } catch (failure) {
      schemeSyncError = errorText(failure);
    } finally {
      if (_checkingSchemeSyncAccount == account) {
        _checkingSchemeSyncAccount = null;
      }
      _changed();
      if (pendingCloudProfile == null && _schemeProfileDirty) {
        scheduleSchemeProfileSync();
      }
    }
  }

  void dismissCloudProfileOffer() {
    pendingCloudProfile = null;
    _changed();
    if (_schemeProfileDirty) scheduleSchemeProfileSync();
  }

  void dismissCloudActiveScheme() {
    pendingCloudActiveScheme = null;
    _changed();
  }

  void scheduleSchemeProfileSync({
    String? packageKey,
    bool afterSuccessfulDeploy = false,
  }) {
    _schemeProfileDirty = true;
    if (afterSuccessfulDeploy &&
        packageKey != null &&
        installedPackageFor(packageKey)?.active == true &&
        syncedSchemes.contains(packageKey)) {
      _pendingCloudPackageUploads.add(packageKey);
    }
    _schemeSyncDebounce?.cancel();
    if (_disposed ||
        !followAccountSync ||
        accountUser == null ||
        _readySchemeSyncAccount != accountUser?.id ||
        _checkingSchemeSyncAccount != null ||
        pendingCloudProfile != null ||
        _applyingCloudProfile) {
      return;
    }
    _schemeSyncDebounce = Timer(const Duration(milliseconds: 800), () async {
      if (busy || schemeSyncBusy) {
        scheduleSchemeProfileSync();
        return;
      }
      if (_disposed ||
          !followAccountSync ||
          accountUser == null ||
          _readySchemeSyncAccount != accountUser?.id ||
          _checkingSchemeSyncAccount != null ||
          pendingCloudProfile != null) {
        return;
      }
      await _schemeCloudAction(() async {
        Object? packageFailure;
        for (final key in _pendingCloudPackageUploads.toList()) {
          if (!syncedSchemes.contains(key)) {
            _pendingCloudPackageUploads.remove(key);
            continue;
          }
          if (installedPackageFor(key)?.active != true ||
              local?.deployed != true) {
            _pendingCloudPackageUploads.remove(key);
            continue;
          }
          if (installedPackageFor(key)?.hasPackageZip != true) continue;
          try {
            await core.uploadCloudPackage(schemeKey: key);
            _pendingCloudPackageUploads.remove(key);
          } catch (failure) {
            packageFailure ??= failure;
          }
        }
        await _uploadSchemeProfile();
        if (packageFailure != null) throw packageFailure;
      });
    });
  }

  Future<bool> _schemeCloudAction(
    Future<void> Function() action, {
    bool modal = false,
  }) async {
    if (schemeSyncBusy) return false;
    return run(() async {
      schemeSyncBusy = true;
      schemeSyncError = null;
      if (modal) {
        cloudTransferError = null;
        cloudTransferProgress = null;
      }
      try {
        await action();
      } catch (failure) {
        if (modal) {
          cloudTransferError = errorText(failure);
        } else {
          schemeSyncError = errorText(failure);
        }
        await reloadAccountSession(checkSchemeProfile: false);
        rethrow;
      } finally {
        schemeSyncBusy = false;
      }
    });
  }

  Future<void> _uploadSchemeProfile() async {
    await _persistSchemeSettings(schedule: false);
    final envelope = jsonDecode(await core.uploadSchemeProfile()) as Map;
    lastSchemeSyncAt = envelope['updateAt'] as String?;
    _schemeProfileDirty = false;
  }

  Future<bool> uploadSchemeProfile() =>
      _schemeCloudAction(_uploadSchemeProfile);

  Future<bool> fetchCloudProfile() => _schemeCloudAction(() async {
    final envelope = jsonDecode(await core.fetchSchemeProfile()) as Map;
    if (envelope['profile'] is! Map) throw StateError('云端暂无配置');
    pendingCloudProfile = Map<String, dynamic>.from(envelope['profile'] as Map);
    if (envelope['updateAt'] case final String time) lastSchemeSyncAt = time;
  });

  Future<void> _applyCloudSettings(Map<String, dynamic> settings) async {
    if (settings['download_source'] case final String value) source = value;
    if (settings['selected_scheme'] case final String value) {
      if (value != scheme) {
        scheme = value;
        schemeRelease = null;
        latestRelease = null;
        if (_startupStarted) unawaited(refreshRelease());
      }
      _manuallySelectedScheme = true;
    }
    if (isMobile && settings['android'] is Map) {
      if (isAndroid) await _requireStorage();
      final input = settings['android'] as Map;
      final patch = <String, Object>{};
      for (final key in androidDefaults.keys) {
        final snakeKey = key.replaceAllMapped(
          RegExp('[A-Z]'),
          (match) => '_${match[0]!.toLowerCase()}',
        );
        if (input[snakeKey] case final Object value) patch[key] = value;
      }
      androidSettings = await core.setAndroidImeInputSettings(
        settings: patchAndroidSettings(
          await core.getAndroidImeInputSettings(),
          patch,
        ),
      );
    }
    if (settings['ime_ui'] is Map) {
      if (isAndroid) await _requireStorage();
      final ui = settings['ime_ui'] as Map;
      final current = await core.getImeUiSettings();
      uiSettings = await core.setImeUiSettings(
        colorScheme: UiColorSchemeDto.values.firstWhere(
          (value) => value.name == ui['color_scheme'],
          orElse: () => current.colorScheme,
        ),
        orientation: isMobile
            ? current.orientation
            : PanelOrientationDto.values.firstWhere(
                (value) => value.name == ui['orientation'],
                orElse: () => current.orientation,
              ),
        accentColor: ui['accent_color'] as String?,
        cornerRadius: ui.containsKey('corner_radius')
            ? (ui['corner_radius'] as num?)?.toDouble()
            : current.cornerRadius,
        backgroundColor:
            ui['background_color'] as String? ?? current.backgroundColor,
        fontSize: isMobile
            ? current.fontSize
            : (ui['font_size'] as num?)?.toDouble() ?? current.fontSize,
      );
      if (ui['embedded_composition'] case final bool value when !isMobile) {
        uiSettings = await core.setImeEmbeddedComposition(embedded: value);
      }
      if (ui['hide_candidate_panel'] case final bool value when !isMobile) {
        uiSettings = await core.setImeHideCandidatePanel(hide_: value);
      }
      if (ui['show_tray_mode_indicator'] case final bool value when isWindows) {
        uiSettings = await core.setImeShowTrayModeIndicator(show_: value);
      }
    }
    if (settings['desktop_english_mode'] case final String mode
        when !isMobile) {
      desktopEnglishMode = await core.setDesktopEnglishMode(
        mode: EnglishModeDto.values.firstWhere(
          (value) => value.name == mode,
          orElse: () => desktopEnglishMode,
        ),
      );
    }
    if (settings['desktop_english_switch_key'] case final String key
        when !isMobile) {
      final saved = await core.setDesktopEnglishSwitchKey(key: key);
      desktopEnglishSwitchKey = saved.key;
      desktopSchemaEnglishSwitchKeys = saved.schemaKeys;
    }
    _changed();
  }

  Future<bool> applyCloudProfile() {
    final profile = pendingCloudProfile;
    if (profile == null || savingTheme) return Future.value(false);
    return _schemeCloudAction(() async {
      _applyingCloudProfile = true;
      _schemeSyncDebounce?.cancel();
      try {
        _schemeSettings = Map<String, dynamic>.from(
          profile['settings'] as Map? ?? const {},
        );
        await _applyCloudSettings(_schemeSettings);
        final result = jsonDecode(
          await core.applySchemeProfile(profileJson: jsonEncode(profile)),
        ) as Map;
        _schemeSettings = Map<String, dynamic>.from(
          result['settings'] as Map? ?? const {},
        );
        await _readInstalledPackages();
        final active = result['active_scheme'] as String?;
        pendingCloudActiveScheme = active != null && active != activeSchemeKey
            ? active
            : null;
        pendingCloudProfile = null;
        _pendingCloudPackageUploads.clear();
        _schemeProfileDirty = false;
      } finally {
        _applyingCloudProfile = false;
      }
    });
  }

  Future<bool> refreshCloudPackages() => _schemeCloudAction(() async {
    cloudPackages = (jsonDecode(await core.listCloudPackages()) as List)
        .map((value) => Map<String, dynamic>.from(value as Map))
        .toList();
  });

  Future<Map<String, dynamic>?> previewCloudPackage(String key) async {
    Map<String, dynamic>? preview;
    await _schemeCloudAction(() async {
      preview = Map<String, dynamic>.from(
        jsonDecode(await core.previewCloudPackage(schemeKey: key)) as Map,
      );
    }, modal: true);
    return preview;
  }

  Future<bool> uploadCloudPackage(String key, {String? expectedSha256}) =>
      _schemeCloudAction(() async {
        await core.uploadCloudPackage(
          schemeKey: key,
          expectedSha256: expectedSha256,
        );
        scheduleSchemeProfileSync();
        cloudPackages = (jsonDecode(await core.listCloudPackages()) as List)
            .map((value) => Map<String, dynamic>.from(value as Map))
            .toList();
      }, modal: true);

  Future<bool> downloadCloudPackage(String id) => _schemeCloudAction(() async {
    if (!cloudPackages.any((package) => package['id'] == id)) {
      cloudPackages = (jsonDecode(await core.listCloudPackages()) as List)
          .map((value) => Map<String, dynamic>.from(value as Map))
          .toList();
    }
    final package = cloudPackages.firstWhere((package) => package['id'] == id);
    final previous = installedPackages
        .map((package) => package.schemeKey)
        .toSet();
    if (isAndroid) await _requireStorage(redirect: true);
    await core.downloadCloudPackage(id: id);
    await _readInstalledPackages();
    final key =
        installedPackages
            .where((package) => !previous.contains(package.schemeKey))
            .firstOrNull
            ?.schemeKey ??
        package['schemeKey'] as String;
    if (prefersLibjd) {
      if (!await _deferDeployForSchemaSelection(
        key,
        activateOnConfirm: true,
        convert: true,
      )) {
        await _convertSchemePackage(key);
      }
    } else if (!await _deferDeployForSchemaSelection(
      key,
      activateOnConfirm: true,
    )) {
      await _switchSchemePackage(key);
    }
    scheduleSchemeProfileSync();
  }, modal: true);

  Future<bool> deleteCloudPackage(String id) => _schemeCloudAction(() async {
    await core.deleteCloudPackage(id: id);
    cloudPackages.removeWhere((package) => package['id'] == id);
    scheduleSchemeProfileSync();
  });

  void scheduleUserDictionarySync() {
    _dictionarySyncDebounce?.cancel();
    if (_disposed ||
        !_foreground ||
        accountUser == null ||
        pendingDictionarySync != null) {
      return;
    }
    _dictionarySyncDebounce = Timer(const Duration(milliseconds: 800), () {
      if (_disposed ||
          !_foreground ||
          accountUser == null ||
          pendingDictionarySync != null) {
        return;
      }
      if (busy || syncingDictionary) {
        scheduleUserDictionarySync();
      } else {
        unawaited(syncUserDictionary(automatic: true));
      }
    });
  }

  void dismissDictionarySyncConfirmation() {
    final pending = pendingDictionarySync;
    final account = accountUser?.id;
    if (pending != null &&
        account != null &&
        _pendingDictionarySyncAccountId == account) {
      (_cancelledDictionarySyncDeletions[account] ??= {}).add(
        _dictionarySyncDeletionKey(pending),
      );
    }
    pendingDictionarySync = null;
    _pendingDictionarySyncAccountId = null;
    _changed();
  }

  String _dictionarySyncDeletionKey(UserDictionarySyncDto sync) => jsonEncode([
    for (final entries in [sync.localDeletions, sync.serverDeletions])
      (entries
          .map((entry) => jsonEncode([entry.word, entry.code, entry.remove]))
          .toSet()
          .toList()
        ..sort()),
  ]);

  Future<bool> syncUserDictionary({
    UserDictionarySyncDto? confirmedDeletions,
    bool automatic = false,
  }) async {
    if (busy || syncingDictionary || accountUser == null) return false;
    final pending = pendingDictionarySync;
    if (confirmedDeletions != null &&
        (pending == null ||
            !identical(confirmedDeletions, pending) ||
            !pending.pending ||
            (pending.localDeletions.isEmpty &&
                pending.serverDeletions.isEmpty) ||
            _pendingDictionarySyncAccountId != accountUser?.id)) {
      return false;
    }
    final account = accountUser?.id;
    _dictionarySyncDebounce?.cancel();
    if (!automatic) busy = true;
    syncingDictionary = true;
    accountError = null;
    accountMessage = null;
    _changed();
    List<UserWordDto>? before;
    var synced = false;
    var syncCompleted = false;
    try {
      before = await core.listUserWords(schemeKey: 'keytao');
      final result = await core.accountSyncUserDictionary(
        confirmedDeletions: confirmedDeletions == null
            ? null
            : UserDictionaryDeletionConfirmationDto(
                localDeletions: List.of(confirmedDeletions.localDeletions),
                serverDeletions: List.of(confirmedDeletions.serverDeletions),
              ),
      );
      accountMessage = result.message;
      pendingDictionarySync =
          result.pending &&
              accountUser?.id == account &&
              !(automatic &&
                  (_cancelledDictionarySyncDeletions[account]?.contains(
                        _dictionarySyncDeletionKey(result),
                      ) ??
                      false))
          ? result
          : null;
      _pendingDictionarySyncAccountId = pendingDictionarySync != null
          ? account
          : null;
      if (!result.pending) {
        userWordsRevision++;
        syncCompleted = true;
      }
      synced = true;
    } catch (failure) {
      accountError = errorText(failure);
    } finally {
      if (before != null) {
        try {
          final after = await core.listUserWords(schemeKey: 'keytao');
          if (!listEquals(before, after)) {
            if (!synced) userWordsRevision++;
            _scheduleUserWordsDeploy();
          }
        } catch (failure) {
          accountError ??= errorText(failure);
          synced = false;
        }
      }
      await reloadAccountSession();
      syncingDictionary = false;
      if (!automatic) busy = false;
      if (automatic &&
          synced &&
          syncCompleted &&
          !_disposed &&
          accountUser?.id == account) {
        _dictionaryPullCooldown?.cancel();
        _dictionaryPullCooldown = Timer(const Duration(seconds: 60), () {});
      }
      _changed();
    }
    return synced;
  }

  // The bridge holds its install lock while invoking this callback. Never
  // re-enter a bridge install/deploy call here, including during rollback.
  Future<String> _androidDeploy() async {
    try {
      final result = await android.deployImeData();
      if (result['deployed'] == true) {
        final warning = result['warning'] as String?;
        if (warning != null && warning.isNotEmpty) {
          deployWarning = warning;
          _deployStep(warning, DeployStepState.neutral);
          addOperationLogs(['[WARN] $warning']);
        }
        return '';
      }
      final message = result['message'] as String?;
      return message == null || message.isEmpty
          ? AppStrings.deployIncomplete
          : message;
    } catch (failure) {
      return errorText(failure);
    }
  }

  Future<bool> installAddon() {
    if (!requireRimeEngine()) return Future.value(false);
    return _manageAddon(true);
  }

  Future<bool> uninstallAddon() => _manageAddon(false);

  Future<bool> setPackageEasyEnglish(String key, bool enabled) {
    if (!requireRimeEngine()) return Future.value(false);
    return run(() async {
      deployingSchemeKey = key;
      addonError = null;
      addonProgress = null;
      _startOperation(OperationKind.addon, AppStrings.deployingLibrime);
      try {
        if (isAndroid) await _requireStorage();
        await core.setPackageEasyEnglish(
          packageKey: key,
          enabled: enabled,
          androidDeploy: _androidDeploy,
        );
        await _readLocal();
      } catch (failure) {
        try {
          await _readLocal();
        } catch (_) {}
        addonError = errorText(failure);
        rethrow;
      } finally {
        deployingSchemeKey = null;
        operation = null;
      }
    });
  }

  Future<bool> _manageAddon(bool installed) => run(() async {
    _addonUpdateChecks['easy_en'] = Object();
    _startOperation(
      OperationKind.addon,
      installed ? AppStrings.installing : AppStrings.uninstall,
    );
    addonError = null;
    addonProgress = null;
    try {
      if (isAndroid) {
        await _requireStorage();
      }
      addon = installed
          ? await core.addonSchemaInstall(
              id: 'easy_en',
              androidDeploy: _androidDeploy,
            )
          : await core.addonSchemaUninstall(
              id: 'easy_en',
              androidDeploy: _androidDeploy,
            );
      if (installed) addonLatestVersion = addon!.version;
      addOperationLogs([
        installed
            ? AppStrings.easyEnglishInstalled(addon!.version)
            : AppStrings.easyEnglishRemoved,
      ]);
      await _readLocal();
      if (!installed) {
        if (isMobile) {
          androidSettings = await core.getAndroidImeInputSettings();
        } else {
          desktopEnglishMode = await core.getDesktopEnglishMode();
        }
      }
    } catch (failure) {
      addonError = errorText(failure);
      addOperationLogs(['[ADDON ERROR] $addonError']);
      try {
        addon = await core.addonSchemaStatus(id: 'easy_en');
      } catch (_) {}
      rethrow;
    } finally {
      deployingSchemeKey = null;
      operation = null;
    }
  });

  Future<bool> manageWanxiang(bool installed) => run(() async {
    _addonUpdateChecks['wanxiang'] = Object();
    _startOperation(
      OperationKind.wanxiang,
      installed ? AppStrings.installing : AppStrings.uninstall,
    );
    wanxiangError = null;
    wanxiangProgress = null;
    try {
      if (isAndroid) await _requireStorage();
      wanxiang = await core.manageWanxiang(
        installed: installed,
        variant: selectedWanxiangVariant,
        androidDeploy: _androidDeploy,
      );
      if (installed) {
        wanxiangLatestVersion = wanxiang!.version;
        await _finishAddonDownload('wanxiang', wanxiang!);
      }
      await _readWanxiangGrammar();
      await _readLocal();
      addOperationLogs([
        installed
            ? prefersLibjd
                  ? '[ADDON] ${AppStrings.wanxiang}已转换'
                  : wanxiang?.deployed == true
                  ? AppStrings.wanxiangInstalled
                  : '[ADDON] ${AppStrings.wanxiang}已下载'
            : AppStrings.wanxiangRemoved,
      ]);
    } catch (failure) {
      wanxiangError = errorText(failure);
      try {
        wanxiang = await core.wanxiangStatus();
        await _readWanxiangGrammar();
      } catch (_) {}
      try {
        await _readLocal();
      } catch (_) {}
      rethrow;
    } finally {
      operation = null;
    }
  });

  Future<void> _readWanxiangGrammar() async {
    try {
      wanxiangGrammar = isIos ? null : await core.wanxiangGrammarStatus();
    } catch (failure) {
      wanxiangGrammar = null;
      wanxiangGrammarError ??= errorText(failure);
    }
  }

  Future<bool> manageWanxiangGrammar(bool installed) {
    if (installed && !requireRimeEngine()) return Future.value(false);
    return run(() async {
      _startOperation(
        OperationKind.wanxiangGrammar,
        installed ? AppStrings.installing : AppStrings.uninstall,
      );
      wanxiangGrammarError = null;
      wanxiangGrammarProgress = null;
      try {
        if (!showWanxiangGrammar) {
          throw StateError(AppStrings.wanxiangGrammarUnavailable);
        }
        if (isAndroid) await _requireStorage();
        wanxiangGrammar = await core.manageWanxiangGrammar(
          installed: installed,
          androidDeploy: _androidDeploy,
        );
        await _readLocal();
        addOperationLogs([
          installed
              ? AppStrings.wanxiangGrammarInstalled
              : AppStrings.wanxiangGrammarRemoved,
        ]);
      } catch (failure) {
        wanxiangGrammarError = errorText(failure);
        try {
          await _readWanxiangGrammar();
          await _readLocal();
        } catch (_) {}
        rethrow;
      } finally {
        operation = null;
      }
    });
  }

  Future<bool> manageRimeAddon(String id, bool installed) => run(() async {
    final slot = rimeAddons[id]!;
    _addonUpdateChecks[id] = Object();
    _startOperation(
      slot.operation,
      installed ? AppStrings.installing : AppStrings.uninstall,
    );
    slot.error = null;
    slot.progress = null;
    try {
      if (isAndroid) await _requireStorage();
      final status = await slot._manage(
        installed: installed,
        androidDeploy: _androidDeploy,
      );
      slot.status = status;
      if (installed) {
        slot.latestVersion = status.version;
        await _finishAddonDownload(id, status);
      }
      await _readLocal();
      addOperationLogs([
        !installed
            ? slot._removedMessage
            : prefersLibjd
            ? '[ADDON] ${slot.title}已转换'
            : slot.status?.deployed == true
            ? slot._installedMessage
            : '[ADDON] ${slot.title}已下载',
      ]);
    } catch (failure) {
      slot.error = errorText(failure);
      try {
        slot.status = await slot._readStatus();
      } catch (_) {}
      try {
        await _readLocal();
      } catch (_) {}
      rethrow;
    } finally {
      if (id == 'rime_ice') deployingSchemeKey = null;
      operation = null;
    }
  });

  Future<bool> finishOnboarding() => run(() async {
    if (!isMobile) await _readState();
    if (!setupReady) throw StateError(AppStrings.finishSetupFirst);
    await core.completeOnboarding();
    onboardingCompleted = true;
  });

  Future<bool> saveAndroid(Map<String, Object> patch) => _saveAndroid(patch);

  Future<bool> setKeyboardFlushBottom(bool enabled) => run(() async {
    await android.setKeyboardFlushBottom(enabled);
    ime = {...?ime, 'keyboardFlushBottom': enabled};
  });

  Future<bool> _saveAndroid(
    Map<String, Object> patch, {
    bool resetKeyboardLayout = false,
  }) => run(() async {
    if (isAndroid) await _requireStorage();
    if (isAndroid && resetKeyboardLayout) {
      await android.resetKeyboardLayout();
      ime = {...?ime, 'keyboardFlushBottom': false};
    }
    final current = await core.getAndroidImeInputSettings();
    if (patch['englishMode'] == 'schema') {
      await _readLocal();
      if (!englishReady) throw StateError(AppStrings.englishNotReady);
    }
    androidSettings = await core.setAndroidImeInputSettings(
      settings: patchAndroidSettings(current, patch),
    );
    await _persistSchemeSettings();
  });

  Future<bool> saveSchemaSwitch(SchemaSwitchDto entry, int value) =>
      run(() async {
        if (isAndroid) await _requireStorage();
        ++_schemaSwitchesRequest;
        try {
          schemaSwitches = await core.setSchemaSwitch(
            schemaId: entry.schemaId,
            name: entry.name,
            value: value,
          );
          schemaSwitchError = null;
        } catch (failure) {
          schemaSwitchError = AppStrings.schemaSwitchUnavailable;
          debugPrint('Schema switch save failed: ${errorText(failure)}');
          throw StateError(schemaSwitchError!);
        }
        unawaited(refreshCandidatePreview());
      });

  Future<bool> selectSchema(String schemaId) {
    if (prefersLibjd) {
      return switchPackage(activeSchemeKey, schemaId: schemaId);
    }
    return run(() => _selectSchema(schemaId), switchingInput: true);
  }

  Future<void> _selectSchema(String schemaId) async {
    if (isAndroid) await _requireStorage();
    await core.selectSchema(schemaId: schemaId);
    await _readLocal();
    await _readInstalledPackages();
    if (isMobile) {
      await _checkRecommendedKeyboardAfterSchemaSwitch(schemaId);
    }
    unawaited(refreshCandidatePreview());
  }

  String get _recommendationSchemaKey => currentSchemaId ?? activeSchemeKey;

  bool get currentKeyboardHasMnemonics {
    final active = keyboards.where((k) => k.current).firstOrNull;
    if (active != null) return active.hasMnemonics;
    final raw = androidSettings?.keyboardLayoutJson;
    if (raw == null || raw.isEmpty) return false;
    try {
      final decoded = jsonDecode(raw);
      return decoded is Map<String, dynamic> && decoded['hasMnemonics'] == true;
    } catch (_) {
      return false;
    }
  }

  Future<void> _refreshKeyboardLibrary() async {
    try {
      keyboards = await core.getKeyboardLibrary();
      recommendedKeyboard = await core.getRecommendedKeyboardForSchema(
        schemaId: _recommendationSchemaKey,
      );
    } catch (failure) {
      debugPrint('Keyboard library refresh failed: ${errorText(failure)}');
    }
  }

  Future<void> _checkRecommendedKeyboardAfterSchemaSwitch(
    String schemaId,
  ) async {
    try {
      keyboards = await core.getKeyboardLibrary();
      final recommended = await core.getRecommendedKeyboardForSchema(
        schemaId: schemaId,
      );
      recommendedKeyboard = recommended;
      if (recommended != null && !recommended.current) {
        pendingRecommendedKeyboard = recommended;
      } else {
        pendingRecommendedKeyboard = null;
      }
      _changed();
    } catch (failure) {
      debugPrint('Recommended keyboard lookup failed: ${errorText(failure)}');
    }
  }

  void dismissRecommendedKeyboard() {
    if (pendingRecommendedKeyboard == null) return;
    pendingRecommendedKeyboard = null;
    _changed();
  }

  Future<bool> applyKeyboardProfile(String id) => run(() async {
    if (isAndroid) await _requireStorage();
    await core.applyKeyboard(id: id);
    pendingRecommendedKeyboard = null;
    if (isIos || (isAndroid && storageReady)) {
      androidSettings = await core.getAndroidImeInputSettings();
    }
    await _refreshKeyboardLibrary();
  });

  Future<bool> deleteKeyboardProfile(String id) => run(() async {
    if (isAndroid) await _requireStorage();
    await core.deleteKeyboard(id: id);
    if (isIos || (isAndroid && storageReady)) {
      androidSettings = await core.getAndroidImeInputSettings();
    }
    await _refreshKeyboardLibrary();
  });

  Future<KeyboardExportDto> exportKeyboardProfile(String id) =>
      core.exportKeyboard(id: id);

  Future<KeyboardImportPreviewDto> previewKeyboardImport(String text) =>
      core.previewKeyboardImport(text: text);

  Future<KeyboardImportResultDto> importKeyboardProfile(
    String text, {
    bool overwrite = false,
  }) async {
    final result = await core.importKeyboard(text: text, overwrite: overwrite);
    if (isIos || (isAndroid && storageReady)) {
      androidSettings = await core.getAndroidImeInputSettings();
    }
    await _refreshKeyboardLibrary();
    _changed();
    return result;
  }

  Future<KeyboardDocumentDto> getKeyboardDocument(String id) =>
      core.getKeyboardDocument(id: id);

  Future<KeyboardSummaryDto> saveKeyboardDocument(
    String id,
    String layoutJson, {
    bool apply = true,
  }) async {
    if (isAndroid) await _requireStorage();
    final saved = await core.saveKeyboardDocument(
      id: id,
      layoutJson: layoutJson,
      apply: apply,
    );
    if (saved.current && (isIos || (isAndroid && storageReady))) {
      androidSettings = await core.getAndroidImeInputSettings();
    }
    await _refreshKeyboardLibrary();
    _changed();
    return saved;
  }

  Future<KeyboardSummaryDto> duplicateKeyboardProfile(
    String id, {
    String? newName,
  }) async {
    if (isAndroid) await _requireStorage();
    final dup = await core.duplicateKeyboard(id: id, newName: newName);
    await _refreshKeyboardLibrary();
    _changed();
    return dup;
  }

  Future<KeyboardSummaryDto> renameKeyboardProfile(
    String id,
    String newName,
  ) async {
    if (isAndroid) await _requireStorage();
    final renamed = await core.renameKeyboard(id: id, newName: newName);
    if (renamed.current && (isIos || (isAndroid && storageReady))) {
      androidSettings = await core.getAndroidImeInputSettings();
    }
    await _refreshKeyboardLibrary();
    _changed();
    return renamed;
  }

  Future<void> shareKeyboardFile(
    KeyboardExportDto keyboard,
  ) => themeLibraryAction(() async {
    if (isAndroid) {
      await android.shareThemeFile(keyboard.fileName, keyboard.json);
    } else if (isIos) {
      await const IosHost().shareThemeFile(keyboard.fileName, keyboard.json);
    } else if (info.platform == BridgePlatform.macOs) {
      await const MethodChannel('ink.rea.keytao/window').invokeMethod<void>(
        'shareThemeFile',
        {'fileName': keyboard.fileName, 'json': keyboard.json},
      );
    } else {
      final location = await getSaveLocation(suggestedName: keyboard.fileName);
      if (location == null) return;
      await XFile.fromData(
        Uint8List.fromList(utf8.encode(keyboard.json)),
        mimeType: 'application/json',
        name: keyboard.fileName,
      ).saveTo(location.path);
    }
  });

  Future<bool> saveCandidatePageSize(int? size) => run(() async {
    if (isAndroid) await _requireStorage();
    candidatePageSize = await core.setCandidatePageSize(size: size);
    if (local?.installed == true && !prefersLibjd) {
      await _deploy();
    } else {
      unawaited(refreshCandidatePreview());
    }
  });

  (String, String, String?) get _activePreviewTarget =>
      (inputEngine, activeSchemeKey, currentSchemaId);

  Future<void> refreshCandidatePreview() {
    if (_disposed) return Future.value();
    _candidatePreviewRequest++;
    return _candidatePreviewRefresh ??= _readCandidatePreview().then<void>((
      request,
    ) {
      _candidatePreviewRefresh = null;
      if (!_disposed && request != _candidatePreviewRequest) {
        return refreshCandidatePreview();
      }
    });
  }

  Future<int> _readCandidatePreview() async {
    var request = 0;
    while (!_disposed && request != _candidatePreviewRequest) {
      request = _candidatePreviewRequest;
      if (!inputEngineEnabled) {
        candidatePreviewCandidates = [];
        candidatePreviewPageSize = null;
        _candidatePreviewTarget = null;
        _changed();
        break;
      }
      final size = candidatePageSize;
      final target = _activePreviewTarget;
      final span = beginAppOperation(AppOperationName.previewCandidates);
      try {
        final result =
            jsonDecode(await core.previewCandidates(size: size)) as Map;
        final pageSize = result['page_size'] as int;
        final candidates = [
          for (final candidate in result['candidates'] as List)
            (
              text: (candidate as Map)['text'] as String,
              comment: candidate['comment'] as String,
            ),
        ];
        if (pageSize < 1 || pageSize > 255) {
          throw const FormatException('Invalid candidate preview');
        }
        if (_disposed ||
            request != _candidatePreviewRequest ||
            size != candidatePageSize ||
            target != _activePreviewTarget) {
          continue;
        }
        candidatePreviewCandidates = candidates;
        candidatePreviewPageSize = pageSize;
        _candidatePreviewTarget = target;
        _changed();
      } catch (_) {
        // A temporary failure may keep the preview for the same scheme.
        if (!_disposed &&
            request == _candidatePreviewRequest &&
            target == _activePreviewTarget &&
            _candidatePreviewTarget != null &&
            _candidatePreviewTarget != target) {
          candidatePreviewCandidates = [];
          candidatePreviewPageSize = null;
          _candidatePreviewTarget = null;
          _changed();
        }
      } finally {
        span?.dispose();
      }
    }
    return request;
  }

  Map<String, dynamic> resolveCandidatePanelPreview({
    required UiColorSchemeDto colorScheme,
    required String accentColor,
    double? cornerRadius,
    String backgroundColor = '',
    required double fontSize,
    required bool systemDark,
  }) => jsonDecode(
    core.resolveCandidatePanelPreview(
      colorScheme: colorScheme,
      accentColor: accentColor,
      cornerRadius: cornerRadius,
      backgroundColor: backgroundColor,
      fontSize: fontSize,
      systemDark: systemDark,
    ),
  ) as Map<String, dynamic>;

  Future<T> themeLibraryAction<T>(
    Future<T> Function() action, {
    bool refresh = false,
  }) async {
    if (_disposed || !canSaveTheme) throw StateError('操作中');
    savingTheme = true;
    _changed();
    try {
      if (isAndroid) await _requireStorage();
      final result = await action();
      if (refresh) uiSettings = await core.getImeUiSettings();
      return result;
    } finally {
      savingTheme = false;
      _changed();
      if (isMobile) _resumeDeferredActions();
    }
  }

  Future<Map<String, dynamic>> getThemeEditor() async =>
      jsonDecode(await core.getThemeEditor()) as Map<String, dynamic>;

  Future<List<SystemFontFamilyDto>> listSystemFontFamilies() =>
      core.listSystemFontFamilies();

  Future<String?> systemFontPath({required String family}) =>
      core.systemFontPath(family: family);

  Future<Uint8List> loadSystemFontBytes({required String family}) async {
    late Uint8List bytes;
    await core.loadSystemFontBytes(
      family: family,
      receive: (value) {
        bytes = value;
      },
    );
    return bytes;
  }

  Map<String, dynamic> resolveThemeEditorPreview({
    required Map<String, dynamic> source,
    required bool dark,
  }) => jsonDecode(
    core.resolveThemeEditorPreview(sourceJson: jsonEncode(source), dark: dark),
  ) as Map<String, dynamic>;

  Future<Map<String, dynamic>> updateThemeEditor({
    required String target,
    required String path,
    Object? value,
  }) async => jsonDecode(
    await themeLibraryAction(
      () => core.updateThemeEditor(
        target: target,
        path: path,
        valueJson: value == null ? null : jsonEncode(value),
      ),
      refresh: true,
    ),
  ) as Map<String, dynamic>;

  Future<Map<String, dynamic>> setThemeEditorColorScheme(
    UiColorSchemeDto? scheme,
  ) async => jsonDecode(
    await themeLibraryAction(
      () => core.setThemeEditorColorScheme(colorScheme: scheme),
      refresh: true,
    ),
  ) as Map<String, dynamic>;

  Future<void> shareThemeFile(ThemeExportDto theme) =>
      themeLibraryAction(() async {
        if (isAndroid) {
          await android.shareThemeFile(theme.fileName, theme.json);
        } else if (isIos) {
          await const IosHost().shareThemeFile(theme.fileName, theme.json);
        } else if (info.platform == BridgePlatform.macOs) {
          await const MethodChannel('ink.rea.keytao/window').invokeMethod<void>(
            'shareThemeFile',
            {'fileName': theme.fileName, 'json': theme.json},
          );
        } else {
          final location = await getSaveLocation(suggestedName: theme.fileName);
          if (location == null) return;
          await XFile.fromData(
            Uint8List.fromList(utf8.encode(theme.json)),
            mimeType: 'application/json',
            name: theme.fileName,
          ).saveTo(location.path);
        }
      });

  Future<bool> saveUi({
    UiColorSchemeDto? colorScheme,
    PanelOrientationDto? orientation,
    bool? hideCandidatePanel,
    String? accentColor,
    double? cornerRadius,
    bool resetCornerRadius = false,
    String? backgroundColor,
    double? fontSize,
  }) async {
    final normalizedAccent = accentColor?.trim().toUpperCase();
    final normalizedBackground = backgroundColor?.trim().toUpperCase();
    // Match the Rust IME_UI_*_CHANGED flags.
    const radiusChanged = 1;
    const backgroundChanged = 2;
    const orientationChanged = 4;
    const fontSizeChanged = 8;
    final changedFields =
        (resetCornerRadius || cornerRadius != null ? radiusChanged : 0) |
        (backgroundColor != null ? backgroundChanged : 0) |
        (orientation != null ? orientationChanged : 0) |
        (fontSize != null ? fontSizeChanged : 0);
    if (normalizedAccent != null &&
        !RegExp(r'^#[0-9A-F]{6}$').hasMatch(normalizedAccent)) {
      return false;
    }
    if ((cornerRadius != null && !cornerRadius.isFinite) ||
        (normalizedBackground != null &&
            normalizedBackground.isNotEmpty &&
            !RegExp(r'^#[0-9A-F]{6}$').hasMatch(normalizedBackground))) {
      return false;
    }
    return _saveAppearance(() async {
      final current = await core.getImeUiSettings();
      uiSettings = await core.setImeUiSettings(
        colorScheme: colorScheme ?? current.colorScheme,
        orientation: orientation ?? current.orientation,
        accentColor: normalizedAccent,
        cornerRadius: resetCornerRadius
            ? null
            : (cornerRadius ?? current.cornerRadius)?.clamp(0, 28),
        backgroundColor: normalizedBackground ?? current.backgroundColor,
        fontSize: (fontSize ?? current.fontSize).clamp(10, 36),
        changedFields: changedFields,
      );
      if (hideCandidatePanel != null &&
          hideCandidatePanel != current.hideCandidatePanel) {
        uiSettings = await core.setImeHideCandidatePanel(
          hide_: hideCandidatePanel,
        );
      }
    });
  }

  Future<bool> _saveAppearance(Future<void> Function() save) async {
    if (_disposed || !canSaveTheme) return false;
    try {
      await themeLibraryAction(() async {
        await save();
        await _persistSchemeSettings();
      });
      return true;
    } catch (failure) {
      if (!_disposed) showToast(errorText(failure));
      return false;
    }
  }

  Future<bool> saveEmbedded(bool value) => _saveAppearance(() async {
    uiSettings = await core.setImeEmbeddedComposition(embedded: value);
  });
  Future<bool> saveHideCandidatePanel(bool value) => _saveAppearance(() async {
    uiSettings = await core.setImeHideCandidatePanel(hide_: value);
  });
  Future<bool> saveShowTrayModeIndicator(bool value) =>
      _saveAppearance(() async {
        uiSettings = await core.setImeShowTrayModeIndicator(show_: value);
      });
  Future<bool> saveDesktopEnglish(EnglishModeDto value) => run(() async {
    await _readLocal();
    if (value == EnglishModeDto.schema && !englishReady) {
      throw StateError(AppStrings.englishNotReady);
    }
    desktopEnglishMode = await core.setDesktopEnglishMode(mode: value);
    await _persistSchemeSettings();
  });
  Future<bool> saveDesktopEnglishSwitchKey(String value) => run(() async {
    final saved = await core.setDesktopEnglishSwitchKey(key: value);
    desktopEnglishSwitchKey = saved.key;
    desktopSchemaEnglishSwitchKeys = saved.schemaKeys;
    await _persistSchemeSettings();
  });

  Future<bool> open(String value) async {
    if (_disposed) return false;
    error = null;
    try {
      await _open(value);
      return true;
    } catch (failure) {
      final message = errorText(failure);
      error = _isInlineError(message) ? null : message;
      return false;
    } finally {
      _changed();
    }
  }

  Future<void> _open(String value) async {
    if (isAndroid) {
      await android.openUrl(value);
    } else if (isIos) {
      await const IosHost().openUrl(value);
    } else {
      final result = await runDesktopOpen(
        isWindows
            ? 'rundll32'
            : isLinux
            ? 'xdg-open'
            : 'open',
        [if (isWindows) 'url.dll,FileProtocolHandler', value],
      );
      if (result.exitCode != 0) {
        throw StateError('${result.stderr}');
      }
    }
  }

  Future<bool> openDictionaryManager() async {
    if (!canOpenDictionaryManager) return false;
    try {
      await _open(dictionaryManagerUrl);
      return true;
    } catch (failure) {
      addOperationLogs(['[OPEN RIME DICT ERROR] ${errorText(failure)}']);
      return false;
    }
  }

  Future<bool> openDefaultDirectory() => open(defaultDir);

  Future<bool> openSchemeDirectory(String packageKey) async {
    final target = schemeDirectoryFor(packageKey);
    if (target.isEmpty) return false;
    if (!isMobile && !Directory(target).existsSync()) {
      return open(defaultDir);
    }
    return open(target);
  }

  Future<bool> openActiveSchemeDirectory() async {
    final key = activeIsolatedPackageKey;
    if (key == null) return openDefaultDirectory();
    return openSchemeDirectory(key);
  }

  Future<bool> openTheme() => uiSettings?.themePath == null
      ? Future.value(false)
      : open(uiSettings!.themePath!);
  Future<bool> openAppUpdate({
    bool interactive = false,
    bool redownload = false,
  }) async {
    final update = appUpdate;
    if (_disposed ||
        update == null ||
        !update.hasUpdate ||
        !(redownload ? canDownloadAppUpdate : canRunAppUpdate)) {
      return false;
    }
    final version = update.latestVersion;
    if (redownload) _appUpdateDownloadedVersion = null;
    final action = interactive && appUpdateAction == AppUpdateAction.restart
        ? AppUpdateAction.openPackage
        : appUpdateAction;
    _installingAppUpdate =
        action == AppUpdateAction.openPackage ||
        action == AppUpdateAction.restart;
    appUpdateBusy = true;
    appUpdateProgress = null;
    appUpdateActionError = null;
    _changed();
    try {
      switch (action) {
        case AppUpdateAction.download:
          if (!await core.downloadAppUpdate(version: version)) {
            _appUpdateMissingPackageVersion = version;
            return false;
          }
          _appUpdateDownloadedVersion = version;
          return true;
        case AppUpdateAction.openPackage:
        case AppUpdateAction.restart:
          final restartLinux = isLinux && action == AppUpdateAction.restart;
          final executable = restartLinux ? Platform.resolvedExecutable : null;
          final path = await core.openAppUpdatePackage(
            version: version,
            restart: action == AppUpdateAction.restart,
          );
          if (executable != null) {
            try {
              await Process.start(
                executable,
                [],
                mode: ProcessStartMode.detached,
              );
            } on ProcessException {
              throw StateError(AppStrings.appUpdateReopenManually);
            }
            exit(0);
          }
          if (isAndroid) {
            if (await android.openAppUpdatePackage(path, version)) return true;
            showToast(AppStrings.allowAppUpdatePackage);
            return false;
          }
          if (!isWindows) await _open(path);
          return true;
        case AppUpdateAction.openLink:
          await _open(
            isAndroid &&
                    _androidAppUpdateSource == 'play' &&
                    !version.contains('-')
                ? 'https://play.google.com/store/apps/details?id=ink.rea.keytao_app'
                : update.releaseUrl,
          );
          return true;
      }
    } catch (failure) {
      appUpdateActionError = errorText(failure);
      return false;
    } finally {
      _installingAppUpdate = false;
      appUpdateBusy = false;
      appUpdateProgress = null;
      _changed();
      _resumeDeferredActions();
    }
  }

  // A title change must not toggle `busy`; a missing desktop channel is ignored.
  Future<void> setWindowTitle([String? title]) async {
    if (isMobile) return;
    try {
      await const MethodChannel('ink.rea.keytao/window').invokeMethod<void>(
        'setTitle',
        formatWindowTitle(
          title ?? activePage.title,
          PlatformDispatcher.instance.locale,
        ),
      );
    } on Object catch (_) {}
  }

  Future<bool> openStoragePermissionSettings() => run(() async {
    await android.openStoragePermissionSettings();
    await _readStorage();
  });
  Future<bool> openInputMethodSettings() => run(
    isIos ? const IosHost().openSettings : android.openInputMethodSettings,
  );
  Future<bool> showInputMethodPicker() => run(android.showInputMethodPicker);
  Future<bool> resetAndroidSettings() =>
      _saveAndroid(androidDefaults, resetKeyboardLayout: true);

  Future<void> refreshRimeSyncSettings() async {
    if (_disposed || isMobile || busy || rimeSyncLoading) return;
    rimeSyncLoading = true;
    rimeSyncError = null;
    _changed();
    try {
      rimeSyncSettings = await core.getRimeSyncSettings();
    } catch (failure) {
      rimeSyncError = errorText(failure);
    } finally {
      rimeSyncLoading = false;
      _changed();
    }
  }

  Future<bool> _rimeSyncAction(Future<void> Function() action) {
    if (isMobile || rimeSyncLoading) return Future.value(false);
    return run(() async {
      rimeSyncError = null;
      try {
        await action();
      } catch (failure) {
        rimeSyncError = errorText(failure);
        rethrow;
      }
    });
  }

  Future<void> _saveRimeSyncSettings(
    String installationId,
    String syncDir,
  ) async {
    rimeSyncSettings = await core.saveRimeSyncSettings(
      installationId: installationId,
      syncDir: syncDir == rimeSyncSettings?.defaultSyncDir ? '' : syncDir,
    );
  }

  Future<bool> saveRimeSyncSettings(String installationId, String syncDir) =>
      _rimeSyncAction(() => _saveRimeSyncSettings(installationId, syncDir));

  Future<bool> regenerateRimeInstallationId() {
    final settings = rimeSyncSettings;
    if (settings == null) return Future.value(false);
    return _rimeSyncAction(() async {
      rimeSyncSettings = await core.saveRimeSyncSettings(
        installationId: await core.newRimeInstallationId(),
        syncDir: settings.syncDir,
      );
    });
  }

  Future<bool> pickRimeSyncDirectory(String installationId) =>
      _rimeSyncAction(() async {
        final selected = await pickDesktopDirectory();
        if (selected == null || selected.isEmpty) return;
        await _saveRimeSyncSettings(installationId, selected);
      });

  Future<bool> resetRimeSyncDirectory(String installationId) =>
      saveRimeSyncSettings(installationId, '');

  Future<bool> openRimeSyncDirectory({Locale? locale}) => _rimeSyncAction(
    () async {
      if (rimeSyncSettings case final settings?
          when settings.packageSyncDir.isNotEmpty) {
        final directory = Directory(settings.packageSyncDir);
        if (!await directory.exists()) {
          final custom = settings.syncDir.isNotEmpty;
          final base = custom ? Directory(settings.syncDir) : directory.parent;
          final unavailable = StateError(
            AppStrings.syncFolderUnavailable(
              locale ?? PlatformDispatcher.instance.locale,
              base.path,
            ),
          );
          try {
            if (!await base.exists()) throw unavailable;
            if (directory.path != base.path) {
              // Create each level separately so the base is never recreated.
              if (custom) await directory.parent.create();
              await directory.create();
            }
          } on FileSystemException {
            throw unavailable;
          }
        }
        await _open(directory.path);
      }
    },
  );

  Future<bool> syncRimeUserData() => _rimeSyncAction(() async {
    rimeSyncing = true;
    _changed();
    try {
      await core.syncRimeUserData();
    } finally {
      try {
        rimeSyncSettings = await core.getRimeSyncSettings();
      } catch (failure) {
        rimeSyncError = errorText(failure);
      }
      rimeSyncing = false;
    }
  });

  Future<void> _readLogs() async {
    logsLoading = true;
    debugError = null;
    debugNotice = null;
    _changed();
    Future<String?> read(String prefix, Future<void> Function() action) async {
      try {
        await action();
        return null;
      } catch (failure) {
        return AppStrings.failure(prefix, errorText(failure));
      }
    }

    try {
      final errors = await Future.wait([
        read(AppStrings.readLogSettingsFailure, () async {
          logSettings = await core.getRuntimeLogSettings();
        }),
        read(AppStrings.readRuntimeLogFailure, () async {
          // Omit maxLines to keep Core's 20,000-line limit.
          runtimeLog = await core.readRuntimeLog();
        }),
        read(AppStrings.readSystemLogsFailure, () async {
          systemLogs = await core.readDebugLogs();
        }),
      ]);
      final messages = errors.whereType<String>().toList();
      debugError = messages.isEmpty ? null : messages.join('；');
    } finally {
      logsLoading = false;
      _changed();
    }
  }

  Future<bool> refreshLogs() async {
    if (_disposed || !canEditLogs) return false;
    await _readLogs();
    return true;
  }

  Future<bool> _logAction(
    String failurePrefix,
    Future<void> Function() action, {
    String? notice,
  }) async {
    if (_disposed || !canEditLogs) return false;
    _editingLogs = true;
    debugError = null;
    debugNotice = null;
    _changed();
    try {
      await action();
      debugNotice = notice;
      return true;
    } catch (failure) {
      debugError = AppStrings.failure(failurePrefix, errorText(failure));
      return false;
    } finally {
      _editingLogs = false;
      _changed();
    }
  }

  Future<bool> saveLogSettings(bool enabled, RuntimeLogLevelDto level) =>
      _logAction(AppStrings.updateLogSettingsFailure, () async {
        await core.setRuntimeLogSettings(enabled: enabled, level: level);
        logSettings = await core.getRuntimeLogSettings();
      });
  Future<bool> clearLogs() => _logAction(AppStrings.clearLogsFailure, () async {
    await core.clearRuntimeLog();
    await _readLogs();
  }, notice: AppStrings.logsCleared);
  Future<void> _shareLogs() async {
    sharedLogPath = null;
    if (!isAndroid) {
      // Desktop's old share_runtime_log action opens the fixed log directory.
      await _open('$defaultDir/log');
      return;
    }
    final result = await android.shareRuntimeLog();
    sharedLogPath = result?['path'] as String?;
    if (result?['shareError'] case final String failure
        when failure.isNotEmpty) {
      throw StateError(failure);
    }
  }

  Future<bool> shareLogs() => _logAction(AppStrings.shareFailure, _shareLogs);
  Future<bool> openLogDirectory() =>
      _logAction(AppStrings.openLogDirectoryFailure, () async {
        if (logDirectory == null) return;
        try {
          await _open(logDirectory!);
        } catch (_) {
          await _shareLogs();
        }
      });
  Future<bool> copyLogDirectory() =>
      _logAction(AppStrings.copyPathFailure, () async {
        if (logDirectory == null) return;
        await Clipboard.setData(ClipboardData(text: logDirectory!));
      }, notice: AppStrings.logPathCopied);

  @override
  void dispose() {
    _disposed = true;
    _websiteHidden.dispose();
    _reduceMotion.dispose();
    _dictionarySyncDebounce?.cancel();
    _dictionaryPullTimer?.cancel();
    _dictionaryPullCooldown?.cancel();
    _userWordsDeployDebounce?.cancel();
    _schemaSwitchPoll?.cancel();
    _schemeSyncDebounce?.cancel();
    if (isWindows && _events != null) {
      _windowsChannel.setMethodCallHandler(null);
    }
    unawaited(_events?.cancel());
    unawaited(_androidProgress?.cancel());
    unawaited(_currentSchemaEvents?.cancel());
    super.dispose();
  }
}
