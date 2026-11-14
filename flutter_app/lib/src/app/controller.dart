import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:file_selector/file_selector.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import '../platform/android_host.dart';
import '../platform/ios_host.dart';
import '../rust/api/core.dart' as core;
import '../rust/api/types.dart';
import '../settings/android_settings.dart';
import 'options.dart'
    show
        AppPage,
        defaultDownloadSource,
        schemeAssets,
        schemes,
        xmjdVariantSchemes;
import 'strings.dart';

enum OperationKind { install, deploy, addon, wanxiang, customDirectory }

enum LogLineKind { normal, error, warning, deploy, merged }

enum DeployStepState { neutral, ok, failed }

enum ImeBadgeState {
  checking,
  componentMissing,
  schemeMissing,
  pendingDeploy,
  ready,
}

class DeployStep {
  const DeployStep(this.message, this.state);
  final String message;
  final DeployStepState state;
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

class AppController extends ChangeNotifier {
  AppController({required this.info, this.android = const AndroidHost()});
  final BridgeInfo info;
  final AndroidHost android;
  bool get isAndroid => info.platform == BridgePlatform.android;
  bool get isWindows => info.platform == BridgePlatform.windows;
  bool get isLinux => info.platform == BridgePlatform.linux;
  bool get isIos => info.platform == BridgePlatform.ios;
  bool get isMobile => isAndroid || isIos;
  bool get hasOnboarding => isMobile || info.platform == BridgePlatform.macOs;
  bool get showsEmbeddedComposition =>
      isWindows || info.platform == BridgePlatform.macOs;
  bool _busy = false;
  bool get busy =>
      _busy || windowsRegistrationBusy || _pendingUserWordDeletes.isNotEmpty;
  bool get busyWithoutPendingUserWordDeletes =>
      _busy || windowsRegistrationBusy;
  set busy(bool value) => _busy = value;
  bool _platformStarted = false;
  bool _windowsActionRequested = false;
  bool _windowsActionInFlight = false;
  bool _managingWindowsIme = false;
  static const _windowsChannel = MethodChannel('keytao/windows');
  bool _disposed = false;
  bool _refreshWhenIdle = false;
  bool _manuallySelectedScheme = false;
  bool _startupStarted = false;
  bool _logsWhenIdle = false;
  int _releaseRequest = 0;
  int _schemeUpdatesRequest = 0;
  AppPage activePage = AppPage.input;
  OperationKind? operation;
  bool checkingStorage = false;
  bool releaseLoading = false;
  bool logsLoading = false;
  bool filesLoading = false;
  bool isOpeningCustomDirectory = false;
  bool onboardingCompleted = false;
  AppUpdateDto? appUpdate;
  List<InstalledPackageDto> installedPackages = [];
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
  String? addonError;
  String? wanxiangError;
  String? macosImeError;
  String? windowsImeError;
  String? linuxImeError;
  String? imeUiError;
  String? debugError;
  String? debugNotice;
  InstallProgressDto? installProgress;
  String? installingSchemeKey;
  InstallProgressDto? addonProgress;
  InstallProgressDto? wanxiangProgress;
  InstallProgressDto? customProgress;
  final List<DeployStep> deploySteps = [];
  core.AddonStatusDto? wanxiang;
  String? _selectedWanxiangVariant;
  String? wanxiangLatestVersion;
  String? addonLatestVersion;
  final _addonUpdateChecks = <String, Object>{};
  DebugLogsDto? systemLogs;
  String? selectedDirectory;
  String? selectedTreeUri;
  List<FileItemDto> customFiles = [];
  LocalSchemasDto? customSchemas;
  LocalSchemaDto? customLocal;
  InstallResultDto? customResult;
  String? customError;
  String? error;
  String progress = '';
  double? installFraction;
  final List<String> operationLogs = [];
  List<VerifyEntryDto> verification = [];
  LocalSchemaDto? local;
  String? _localSchemaError;
  core.AddonStatusDto? addon;
  Map<String, dynamic>? storage;
  Map<String, dynamic>? ime;
  MacosImeStatusDto? macosIme;
  WindowsImeStatusDto? windowsIme;
  LinuxImeStatusDto? linuxIme;
  AndroidImeInputSettingsDto? androidSettings;
  ImeUiSettingsDto? uiSettings;
  List<SchemaSwitchDto> schemaSwitches = [];
  EnglishModeDto desktopEnglishMode = EnglishModeDto.ascii;
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

  bool get canInstall =>
      !busy && !releaseLoading && downloadUrl?.isNotEmpty == true;
  InstalledPackageDto? installedPackageFor(String key) {
    for (final package in installedPackages) {
      if (package.schemeKey == key) return package;
    }
    return null;
  }

  bool isLocalScheme(String key) => installedPackageFor(key)?.source == 'local';

  String get activeSchemeKey {
    for (final package in installedPackages) {
      if (package.active) return package.schemeKey;
    }
    return local == null
        ? scheme
        : installedSchemeKey(local!.schemas, local!.installedScheme) ?? scheme;
  }

  void markUserWordsChanged(String key) {
    userWordsRevision++;
    if (userWordsAffectActiveScheme(key)) {
      userWordsPendingDeploy.add(activeSchemeKey);
    }
    _changed();
    if (isKeytaoScheme(key)) scheduleUserDictionarySync();
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
      if (_refreshWhenIdle && !_disposed) {
        _refreshWhenIdle = false;
        unawaited(refreshState());
      } else if (_logsWhenIdle && !_disposed) {
        _logsWhenIdle = false;
        if (activePage == AppPage.debug) unawaited(refreshLogs());
      }
      unawaited(_handleWindowsAction());
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
  bool get canCancelDownload =>
      isInstalling && installProgress?.stage == 'downloading';
  bool get isDeploying => operation == OperationKind.deploy;
  bool get isManagingAddon => operation == OperationKind.addon;
  bool get isManagingWanxiang => operation == OperationKind.wanxiang;
  bool get isInstallingCustom => operation == OperationKind.customDirectory;
  bool get canInstallAddon => !busy && local?.installed == true;
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
  bool get showAddonInstall => addon?.deployed != true || addonHasUpdate;
  bool get canInstallCustom => canInstall && selectedDirectory != null;
  int get operationLogCount => operationLogs.length;
  int get customVerifyFailureCount =>
      customResult?.verify.where((v) => !v.ok).length ?? 0;
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
  String get deployTooltip => local?.installed == true
      ? AppStrings.deployCurrent
      : AppStrings.installFirst;
  String get addonTooltip => local?.installed == true
      ? AppStrings.installEnglishTooltip
      : AppStrings.installKeytaoFirst;
  String get wanxiangTooltip => local?.installed == true
      ? AppStrings.installWanxiangTooltip
      : AppStrings.installMainFirst;
  String get addonButtonLabel => addonHasUpdate
      ? AppStrings.update
      : addon?.installed == true
      ? AppStrings.redeploy
      : AppStrings.install;
  String get wanxiangButtonLabel =>
      wanxiang?.installed == true &&
          selectedWanxiangVariant != installedWanxiangVariant
      ? AppStrings.replace
      : wanxiangHasUpdate
      ? AppStrings.update
      : wanxiang?.installed == true
      ? AppStrings.redeploy
      : AppStrings.install;
  String get addonStatusText =>
      _addonStatus(addon?.installed, addon?.deployed, addon?.version);
  String get wanxiangStatusText {
    final status = _addonStatus(
      wanxiang?.installed,
      wanxiang?.deployed,
      wanxiang?.version,
    );
    return wanxiang?.installed == true
        ? '$status · ${AppStrings.wanxiangVariants[installedWanxiangVariant] ?? installedWanxiangVariant}'
        : status;
  }

  String _addonStatus(bool? installed, bool? deployed, String? version) =>
      installed == null
      ? AppStrings.checking
      : !installed
      ? AppStrings.notInstalled
      : '${deployed == true ? AppStrings.deployed : AppStrings.installedNotDeployed} · ${version == null || version.isEmpty ? AppStrings.unknownVersion : version}';
  String? get localStatusLine => local == null
      ? null
      : AppStrings.localStatus(
          installed: local!.installed,
          deployed: local!.deployed,
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
      ? schemeAssets[scheme]!
      : schemeRelease?.assetName ?? schemeAssets[scheme] ?? '';
  String get schemeSelectionText =>
      '${AppStrings.selection}${schemes[schemeFamily] ?? scheme}'
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
      : local?.installed != true
      ? ImeBadgeState.schemeMissing
      : local?.deployed != true
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
  String get dictionaryManagerUrl => Uri(
    scheme: 'rime-dict',
    host: 'open',
    queryParameters: {'dir': defaultDir},
  ).toString();
  bool get canOpenDictionaryManager =>
      !isMobile && !busy && defaultDir.isNotEmpty;
  String get englishModeLabel =>
      (isMobile
          ? androidSettings?.englishMode == 'schema'
          : desktopEnglishMode == EnglishModeDto.schema)
      ? AppStrings.englishSchema
      : AppStrings.englishAscii;
  String? get englishModeHint => englishReady ? null : AppStrings.englishHint;
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
    releaseError,
    packageError,
    installError,
    addonError,
    wanxiangError,
    customError,
    macosImeError,
    windowsImeError,
    linuxImeError,
    imeUiError,
    debugError,
    schemeSyncError,
    cloudTransferError,
    ...deploySteps.map((step) => step.message),
  ].contains(message);
  String? get unhandledError => _isInlineError(error) ? null : error;

  bool get storageReady =>
      storage?['granted'] == true && storage?['writable'] == true;
  bool get englishReady =>
      addon?.englishAvailable == true ||
      (addon?.installed == true && addon?.deployed == true) ||
      (local?.schemas.contains('english') ?? false);
  bool get setupReady {
    if (local?.installed != true || local?.deployed != true) return false;
    if (isAndroid) {
      return storageReady &&
          ime?['enabled'] == true &&
          ime?['selected'] == true;
    }
    return isIos || !hasOnboarding || macosIme?.installed == true;
  }

  PlatformReleaseDto? get selectedRelease =>
      source == 'github' ? latestRelease?.github : latestRelease?.gitee;
  String? get releaseVersion =>
      scheme == 'keytao' ? selectedRelease?.version : schemeRelease?.version;
  String? get downloadUrl => scheme == 'keytao'
      ? _platformDownloadUrl(selectedRelease)
      : schemeRelease?.downloadUrl;

  String? _platformDownloadUrl(PlatformReleaseDto? release) =>
      switch (info.platform) {
        BridgePlatform.android => release?.downloadUrls.android,
        BridgePlatform.ios => release?.downloadUrls.ios,
        BridgePlatform.windows => release?.downloadUrls.windows,
        BridgePlatform.linux => release?.downloadUrls.linux,
        BridgePlatform.macOs => release?.downloadUrls.macos,
      };

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
          cloudTransferProgress = value;
          _changed();
          return;
        }
        if (event.deployProgress case final value? when isDeploying) {
          progress = value;
          _deployStep(value, DeployStepState.neutral);
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

  void _receiveInstallProgress(InstallProgressDto value) {
    installProgress = value;
    progress = value.message;
    installFraction = value.percent.clamp(0, 100) / 100;
    if (isInstallingCustom) customProgress = value;
    if (isManagingAddon) addonProgress = value;
    if (isManagingWanxiang) wanxiangProgress = value;
    _changed();
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
    if (activePage == page) return;
    activePage = page;
    if (info.platform == BridgePlatform.macOs) unawaited(setWindowTitle());
    if (page == AppPage.debug) {
      sharedLogPath = null;
      if (busy) {
        _logsWhenIdle = true;
      } else {
        unawaited(refreshLogs());
      }
    }
    if (page == AppPage.scheme) unawaited(refreshSchemeUpdates());
    _changed();
  }

  Future<bool> run(Future<void> Function() action) async {
    if (busy || _disposed) return false;
    busy = true;
    error = null;
    _changed();
    try {
      await action();
      return true;
    } catch (failure) {
      final message = errorText(failure);
      error = _isInlineError(message) ? null : message;
      return false;
    } finally {
      busy = false;
      _changed();
      if (_refreshWhenIdle && !_disposed) {
        _refreshWhenIdle = false;
        unawaited(refreshState());
      } else if (_logsWhenIdle && !_disposed) {
        _logsWhenIdle = false;
        if (activePage == AppPage.debug) unawaited(refreshLogs());
      }
      unawaited(_handleWindowsAction());
    }
  }

  Future<void> _readLocal() async {
    _localSchemaError = null;
    try {
      local = await core.checkLocalSchema();
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
    await _readSchemaSwitches();
  }

  Future<void> _readSchemaSwitches() async {
    schemaSwitches = [];
    if (local?.deployed == true) {
      schemaSwitches = await core.getSchemaSwitches();
    }
  }

  Future<void> _readState() async {
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
    } else if (isWindows) {
      await read(_readWindowsIme);
    } else if (isLinux) {
      await read(() async {
        try {
          linuxIme = await core.linuxImeStatus();
          linuxImeError = linuxIme!.error;
        } catch (failure) {
          linuxImeError = errorText(failure);
          rethrow;
        }
      });
    } else if (!isIos) {
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
    local = null;
    addon = null;
    await Future.wait([
      read(_readLocal),
      read(_readInstalledPackages),
      read(() async {
        try {
          wanxiang = await core.wanxiangStatus();
          wanxiangError = null;
        } catch (failure) {
          wanxiangError = errorText(failure);
        }
      }),
      read(() async {
        versions = await core.getComponentVersions();
      }),
      if (isIos || (isAndroid && storageReady))
        read(() async {
          androidSettings = await core.getAndroidImeInputSettings();
        }),
      if (!isMobile) ...[
        read(() async {
          uiSettings = await core.getImeUiSettings();
        }),
        read(() async {
          desktopEnglishMode = await core.getDesktopEnglishMode();
        }),
      ],
    ]);
    if (isAndroid && !storageReady) androidSettings = null;
    if (errors.isNotEmpty) error = errors.toSet().join('\n');
    if (isAndroid && setupReady && !onboardingCompleted) {
      await core.completeOnboarding();
      onboardingCompleted = true;
    }
  }

  Future<bool> refreshState() async {
    final result = await run(() async {
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
            linuxIme = await core.linuxStartIme(restart: false);
            linuxImeError = linuxIme!.error;
          } catch (failure) {
            startupImeError = errorText(failure);
          }
        }
      }
      await _readState();
      if (startupImeError != null) {
        if (isWindows) windowsImeError = startupImeError;
        if (isLinux) linuxImeError = startupImeError;
      }
    });
    if (!_startupStarted && !_disposed) {
      _startupStarted = true;
      unawaited(_loadAccount());
      unawaited(_checkAppUpdate());
      unawaited(refreshSchemeUpdates());
      unawaited(refreshRelease());
    }
    if (result && !_disposed) unawaited(_checkAddonUpdates());
    return result;
  }

  Future<void> _checkAddonUpdates() async {
    await Future.wait([
      if (addon?.installed == true)
        _checkAddonUpdate('easy_en', addon!.version),
      if (wanxiang?.installed == true)
        _checkAddonUpdate('wanxiang', wanxiang!.version),
    ]);
  }

  Future<void> _checkAddonUpdate(String id, String version) async {
    if (_addonUpdateChecks.containsKey(id)) return;
    final check = Object();
    _addonUpdateChecks[id] = check;
    try {
      final status = await core.checkAddonUpdate(id: id);
      final current = id == 'easy_en' ? addon : wanxiang;
      if (_disposed ||
          !identical(_addonUpdateChecks[id], check) ||
          current?.installed != true ||
          current!.version != version) {
        return;
      }
      if (id == 'easy_en') {
        addonLatestVersion = status.latestVersion;
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

  Future<void> _checkAppUpdate() async {
    try {
      final update = await core.checkAppUpdate();
      if (!_disposed) appUpdate = update.hasUpdate ? update : null;
    } catch (_) {
      // Startup update failures are intentionally silent, as in App.tsx.
    }
    _changed();
  }

  Future<void> _readInstalledPackages() async {
    final request = _schemeUpdatesRequest;
    final packages = await core.listInstalledPackages();
    if (_disposed || request != _schemeUpdatesRequest) return;
    installedPackages = packages;
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
    if (busy) {
      _refreshWhenIdle = true;
    } else {
      unawaited(refreshState());
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

  Future<void> _requireStorage({
    bool redirect = false,
    bool customDirectory = false,
  }) async {
    checkingStorage = true;
    _changed();
    try {
      await _readStorage();
      if (storage?['granted'] != true || (!customDirectory && !storageReady)) {
        final message = storage?['message'] as String?;
        final failure = customDirectory || message == null || message.isEmpty
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

  Future<bool> installAndDeploy() async {
    if (!canInstall) return false;
    final result = await run(() async {
      await _install();
      // Deploying clears the verification list, so keep failures on screen.
      if (verification.every((entry) => entry.ok)) await _deploy();
    });
    return result && verification.every((entry) => entry.ok);
  }

  Future<bool> install() {
    if (!canInstall) return Future.value(false);
    return run(_install);
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
      await core.recordInstalledScheme(
        scheme: installing,
        version: installingVersion,
      );
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
        if (verification.every((entry) => entry.ok)) await _deploy();
      } finally {
        installingSchemeKey = null;
      }
    }).then((result) => result && verification.every((entry) => entry.ok));
  }

  Future<bool> updateSchemePackage(String key) {
    final package = installedPackageFor(key);
    if (package == null) return Future.value(false);
    return package.active
        ? _installSchemePackage(key, latest: true)
        : downloadSchemePackage(key);
  }

  Future<bool> downloadSchemePackage(String key) => run(() async {
    installingSchemeKey = key;
    packageError = null;
    _invalidateSchemeUpdates();
    _startOperation(OperationKind.install, AppStrings.installing);
    try {
      await core.downloadSchemePackage(schemeKey: key, source: source);
      _invalidateSchemeUpdates();
      await _readInstalledPackages();
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

  Future<bool> importLocalScheme({String? schemeKey}) async {
    final wasActive = installedPackageFor(schemeKey ?? '')?.active == true;
    String? importedKey;
    final result = await run(() async {
      packageError = null;
      String? temporaryDirectory;
      try {
        final String directoryPath;
        final String displayName;
        if (isMobile) {
          final selected = await MethodChannel(
            isAndroid ? 'keytao/android' : 'keytao/ios',
          ).invokeMapMethod<String, dynamic>('importLocalSchemeDirectory');
          if (selected == null) return;
          temporaryDirectory =
              selected['temporaryDirectory'] as String? ??
              selected['path'] as String;
          directoryPath = selected['path'] as String;
          displayName = selected['name'] as String;
        } else {
          final selected = await getDirectoryPath();
          if (selected == null) return;
          directoryPath = selected;
          final segments = selected
              .split(RegExp(r'[/\\]'))
              .where((segment) => segment.isNotEmpty);
          displayName = segments.isEmpty ? 'local-scheme' : segments.last;
        }
        final key = await core.importLocalScheme(
          directoryPath: directoryPath,
          displayName: displayName,
          schemeKey: schemeKey,
        );
        importedKey = key;
        _pendingCloudPackageUploads.remove(key);
        _invalidateSchemeUpdates();
        await _readInstalledPackages();
        await _readLocal();
        _manuallySelectedScheme = true;
        scheme = key;
        await refreshRelease();
        await _persistSchemeSettings(schedule: false);
        if (wasActive) {
          await _activateSchemePackage(key);
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
        (!wasActive || verification.every((entry) => entry.ok));
  }

  Future<bool> activateSchemePackage(String key) {
    final package = installedPackageFor(key);
    if (package == null || package.active || !package.hasPackageZip) {
      return Future.value(false);
    }
    return run(() => _activateSchemePackage(key))
        .then((result) => result && verification.every((entry) => entry.ok));
  }

  Future<void> _activateSchemePackage(String key) async {
    packageError = null;
    _invalidateSchemeUpdates();
    _startOperation(OperationKind.install, AppStrings.installing);
    try {
      if (isAndroid) await _requireStorage(redirect: true);
      final result = await core.activateScheme(
        schemeKey: key,
        androidExtract: (archive) async {
          try {
            return jsonEncode(await android.smartExtractZipToPrivate(archive));
          } catch (failure) {
            return jsonEncode({'error': errorText(failure)});
          }
        },
      );
      _recordInstallResult(result);
      verification = result.verify;
      _manuallySelectedScheme = true;
      scheme = key;
      latestRelease = null;
      schemeRelease = null;
      _invalidateSchemeUpdates();
      await _readInstalledPackages();
      scheduleSchemeProfileSync();
      await _readLocal();
      await _readUserWordsReport(key);
      await refreshRelease();
    } catch (failure) {
      packageError = errorText(failure);
      rethrow;
    } finally {
      operation = null;
    }
    if (verification.every((entry) => entry.ok)) await _deploy();
  }

  Future<bool> removeSchemePackage(String key) {
    final package = installedPackageFor(key);
    if (package == null || package.active) return Future.value(false);
    return run(() async {
      packageError = null;
      _invalidateSchemeUpdates();
      try {
        await core.removeSchemePackage(schemeKey: key);
        _invalidateSchemeUpdates();
        await _readInstalledPackages();
        if (package.source == 'local' && scheme == key) {
          _manuallySelectedScheme = true;
          scheme =
              installedPackages
                  .where((package) => package.active)
                  .firstOrNull
                  ?.schemeKey ??
              'keytao';
          await refreshRelease();
          await _persistSchemeSettings(schedule: false);
        }
        scheduleSchemeProfileSync();
      } catch (failure) {
        packageError = errorText(failure);
        rethrow;
      }
    });
  }

  void _recordInstallResult(InstallResultDto result) {
    addOperationLogs(result.logs);
    addOperationLogs(
      result.verify
          .where((entry) => !entry.ok)
          .map((entry) => '[VERIFY FAIL] ${entry.path}: ${entry.note}'),
    );
  }

  Future<bool> deploy({int? imeActionId}) =>
      run(() => _deploy(imeActionId: imeActionId));

  Future<void> _deploy({int? imeActionId}) async {
    _pendingCloudPackageUploads.remove(activeSchemeKey);
    _startOperation(OperationKind.deploy, AppStrings.deployingLibrime);
    deploySteps.clear();
    try {
      if (local?.installed != true) {
        throw StateError(
          _localSchemaError == null
              ? AppStrings.noSchemeForDeploy
              : AppStrings.localCheckFailure(_localSchemaError!),
        );
      }
      _deployStep(AppStrings.deployingLibrime, DeployStepState.neutral);
      final String message;
      if (isAndroid) {
        await _requireStorage();
        await core.recordAndroidDeploy(success: false);
        final result = await android.deployImeData();
        if (result['deployed'] != true) {
          throw StateError(AppStrings.deployIncomplete);
        }
        await core.recordAndroidDeploy(success: true);
        message = result['message'] as String? ?? AppStrings.deployDone;
      } else {
        final result = imeActionId == null
            ? await core.deployDefault()
            : await core.windowsRedeployImeAction(id: imeActionId);
        if (!result.success) throw StateError(result.message);
        message = result.message;
      }
      await _readState();
      if (local?.deployed != true) {
        throw StateError(AppStrings.deployCheckFailed);
      }
      _deployStep(message, DeployStepState.ok);
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
      operation = null;
    }
  }

  String? get accountName =>
      accountUser?.nickname ?? accountUser?.name ?? accountUser?.email;

  void _setAccountUser(AccountUserDto? value) {
    final previous = accountUser?.id;
    accountUser = value;
    if (previous == value?.id) return;
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
      scheme = value;
      _manuallySelectedScheme = true;
    }
  }

  Map<String, dynamic> _schemeSettings = {};

  Future<void> loadSchemeSyncState() async {
    try {
      _readSchemeSyncState(await core.getSchemeSyncState());
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
          'flick_keys_enabled': input.flickKeysEnabled,
          'number_row_enabled': input.numberRowEnabled,
          'candidate_font_scale': input.candidateFontScale,
          'double_space_period_enabled': input.doubleSpacePeriodEnabled,
          'floating_portrait_enabled': input.floatingPortraitEnabled,
          'floating_portrait_scale': input.floatingPortraitScale,
          'floating_landscape_enabled': input.floatingLandscapeEnabled,
          'floating_landscape_scale': input.floatingLandscapeScale,
        },
      if (ui != null)
        'ime_ui': {
          'color_scheme': ui.colorScheme.name,
          'orientation': ui.orientation.name,
          'accent_color': ui.accentColor,
          'font_size': ui.fontSize,
          'embedded_composition': ui.embeddedComposition,
        },
      'desktop_english_mode': desktopEnglishMode.name,
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
      final envelope = jsonDecode(await core.fetchSchemeProfile()) as Map;
      final profile = envelope['profile'];
      await _persistSchemeSettings(schedule: false);
      final local = jsonDecode(await core.exportSchemeProfile());
      if (accountUser?.id != account || _disposed) return;
      _readySchemeSyncAccount = account;
      _checkedSchemeSyncAccounts.add(account);
      if (envelope['updateAt'] case final String time) lastSchemeSyncAt = time;
      if (profile is Map &&
          _comparableProfile(profile) != _comparableProfile(local)) {
        pendingCloudProfile = Map<String, dynamic>.from(profile);
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
        packageKey == activeSchemeKey &&
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
          if (key != activeSchemeKey || local?.deployed != true) {
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
      scheme = value;
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
    if (!isMobile && settings['ime_ui'] is Map) {
      final ui = settings['ime_ui'] as Map;
      final current = await core.getImeUiSettings();
      uiSettings = await core.setImeUiSettings(
        colorScheme: UiColorSchemeDto.values.firstWhere(
          (value) => value.name == ui['color_scheme'],
          orElse: () => current.colorScheme,
        ),
        orientation: PanelOrientationDto.values.firstWhere(
          (value) => value.name == ui['orientation'],
          orElse: () => current.orientation,
        ),
        accentColor: ui['accent_color'] as String? ?? current.accentColor,
        fontSize: (ui['font_size'] as num?)?.toDouble() ?? current.fontSize,
      );
      if (ui['embedded_composition'] case final bool value) {
        uiSettings = await core.setImeEmbeddedComposition(embedded: value);
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
    _changed();
  }

  Future<bool> applyCloudProfile() {
    final profile = pendingCloudProfile;
    if (profile == null) return Future.value(false);
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
    await core.downloadCloudPackage(id: id);
    await _readInstalledPackages();
    scheduleSchemeProfileSync();
  }, modal: true);

  Future<bool> deleteCloudPackage(String id) => _schemeCloudAction(() async {
    await core.deleteCloudPackage(id: id);
    cloudPackages.removeWhere((package) => package['id'] == id);
    scheduleSchemeProfileSync();
  });

  void scheduleUserDictionarySync() {
    _dictionarySyncDebounce?.cancel();
    if (_disposed || accountUser == null || pendingDictionarySync != null) {
      return;
    }
    _dictionarySyncDebounce = Timer(const Duration(milliseconds: 800), () {
      if (_disposed || accountUser == null || pendingDictionarySync != null) {
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
          .map((entry) => jsonEncode([entry.word, entry.code]))
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
    busy = true;
    syncingDictionary = true;
    accountError = null;
    accountMessage = null;
    _changed();
    try {
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
        if (isKeytaoScheme(activeSchemeKey)) {
          userWordsPendingDeploy.add(activeSchemeKey);
        }
      }
      return true;
    } catch (failure) {
      accountError = errorText(failure);
      return false;
    } finally {
      await reloadAccountSession();
      syncingDictionary = false;
      busy = false;
      _changed();
    }
  }

  // The bridge holds its install lock while invoking this callback. Never
  // re-enter a bridge install/deploy call here, including during rollback.
  Future<String> _androidDeploy() async {
    try {
      final result = await android.deployImeData();
      if (result['deployed'] == true) return '';
      final message = result['message'] as String?;
      return message == null || message.isEmpty
          ? AppStrings.deployIncomplete
          : message;
    } catch (failure) {
      return errorText(failure);
    }
  }

  Future<bool> installAddon() => _manageAddon(true);
  Future<bool> uninstallAddon() => _manageAddon(false);
  Future<bool> _manageAddon(bool installed) => run(() async {
    _addonUpdateChecks['easy_en'] = Object();
    _startOperation(
      OperationKind.addon,
      installed ? AppStrings.installing : AppStrings.uninstall,
    );
    addonError = null;
    addonProgress = null;
    try {
      if (installed && local?.installed != true) {
        throw StateError(AppStrings.installKeytaoFirst);
      }
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
      if (installed && local?.installed != true) {
        throw StateError(AppStrings.installMainFirst);
      }
      if (isAndroid) await _requireStorage();
      wanxiang = await core.manageWanxiang(
        installed: installed,
        variant: selectedWanxiangVariant,
        androidDeploy: _androidDeploy,
      );
      if (installed) wanxiangLatestVersion = wanxiang!.version;
      await _readLocal();
      addOperationLogs([
        installed ? AppStrings.wanxiangInstalled : AppStrings.wanxiangRemoved,
      ]);
    } catch (failure) {
      wanxiangError = errorText(failure);
      try {
        wanxiang = await core.wanxiangStatus();
      } catch (_) {}
      try {
        await _readLocal();
      } catch (_) {}
      rethrow;
    } finally {
      operation = null;
    }
  });

  Future<bool> pickCustomDirectory() => run(() async {
    customError = null;
    try {
      final selected = isAndroid
          ? await android.pickDirectory()
          : await const MethodChannel('ink.rea.keytao/window')
                .invokeMethod<String>('pickDirectory');
      if (selected == null || selected.isEmpty) return;
      selectedTreeUri = isAndroid ? selected : null;
      selectedDirectory = isAndroid ? _safDisplayPath(selected) : selected;
      customResult = null;
      customSchemas = null;
      customLocal = null;
      customFiles = [];
      await _readCustomDirectory();
    } catch (failure) {
      customError = errorText(failure);
      rethrow;
    }
  });

  String _safDisplayPath(String uri) {
    try {
      final tree = Uri.decodeComponent(uri.split('/tree/')[1]);
      return '/${tree.replaceFirst('primary:', 'sdcard/')}';
    } catch (_) {
      return uri;
    }
  }

  Future<void> _readCustomDirectory() async {
    if (selectedDirectory == null) return;
    filesLoading = true;
    _changed();
    final failures = <String>[];
    Future<void> read(Future<void> Function() action) async {
      try {
        await action();
      } catch (failure) {
        failures.add(errorText(failure));
      }
    }

    try {
      if (isAndroid) {
        await Future.wait([
          read(() async {
            final result = await android.listFiles(selectedTreeUri!);
            customFiles = [
              for (final item in result['files'] as List)
                FileItemDto(
                  name: item['name'] as String,
                  isDir: item['isDir'] as bool,
                ),
            ];
          }),
          read(() async {
            final result = await android.readLocalSchemas(selectedTreeUri!);
            final schemas = List<String>.from(result['schemas'] as List);
            customLocal = LocalSchemaDto(
              installed: result['installed'] == true,
              deployed: result['deployed'] == true,
              version: result['version'] as String?,
              schemas: schemas,
            );
          }),
        ]);
        customSchemas = customLocal == null
            ? null
            : LocalSchemasDto(
                hasDefaultCustom: customFiles.any(
                  (file) =>
                      !file.isDir &&
                      (file.name == 'default.custom.yaml' ||
                          file.name == 'default-custom.yaml'),
                ),
                schemas: customLocal!.schemas,
              );
      } else {
        await Future.wait([
          read(() async {
            customFiles = await core.listDir(dir: selectedDirectory!);
          }),
          read(() async {
            customSchemas = await core.readLocalSchemas(
              dir: selectedDirectory!,
            );
          }),
        ]);
      }
      if (failures.isNotEmpty) throw StateError(failures.join('；'));
    } finally {
      filesLoading = false;
      _changed();
    }
  }

  Future<bool> refreshCustomDirectory() => run(() async {
    customError = null;
    try {
      await _readCustomDirectory();
    } catch (failure) {
      customError = errorText(failure);
      rethrow;
    }
  });

  Future<bool> installCustomDirectory() {
    if (!canInstallCustom) return Future.value(false);
    final url = downloadUrl!;
    final installing = scheme;
    final installingVersion = releaseVersion;
    return run(() async {
      _startOperation(OperationKind.customDirectory, AppStrings.installing);
      customResult = null;
      customProgress = null;
      customError = null;
      try {
        if (isAndroid) {
          await _requireStorage(customDirectory: true);
          final archive = await core.downloadSchemeArchive(url: url);
          try {
            _receiveInstallProgress(
              const InstallProgressDto(
                stage: 'extracting',
                percent: 61,
                message: '正在解压...',
              ),
            );
            final result = await android.smartExtractZip(
              archive,
              selectedTreeUri!,
            );
            customResult = await core.finishAndroidInstall(
              resultJson: jsonEncode(result),
              schemeKey: installing,
              version: installingVersion,
            );
          } finally {
            await core.removeDownloadedArchive(path: archive);
          }
        } else {
          customResult = await core.installSchemeToDir(
            url: url,
            dir: selectedDirectory!,
            schemeKey: installing,
            version: installingVersion,
          );
        }
        addOperationLogs(customResult!.logs);
        progress = AppStrings.customInstallDone;
        await _readCustomDirectory();
      } catch (failure) {
        customError = errorText(failure);
        rethrow;
      } finally {
        operation = null;
      }
    });
  }

  Future<bool> finishOnboarding() => run(() async {
    await _readState();
    if (!setupReady) throw StateError(AppStrings.finishSetupFirst);
    await core.completeOnboarding();
    onboardingCompleted = true;
  });

  Future<bool> saveAndroid(Map<String, Object> patch) => _saveAndroid(patch);

  Future<bool> _saveAndroid(
    Map<String, Object> patch, {
    bool resetKeyboardLayout = false,
  }) => run(() async {
    if (isAndroid) await _requireStorage();
    if (isAndroid && resetKeyboardLayout) await android.resetKeyboardLayout();
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
        schemaSwitches = await core.setSchemaSwitch(
          schemaId: entry.schemaId,
          name: entry.name,
          value: value,
        );
      });

  Future<bool> saveUi({
    UiColorSchemeDto? colorScheme,
    PanelOrientationDto? orientation,
    String? accentColor,
    double? fontSize,
  }) {
    final normalizedAccent = accentColor?.trim().toUpperCase();
    if (normalizedAccent != null &&
        !RegExp(r'^#[0-9A-F]{6}$').hasMatch(normalizedAccent)) {
      return Future.value(false);
    }
    return run(() async {
      final current = await core.getImeUiSettings();
      uiSettings = await core.setImeUiSettings(
        colorScheme: colorScheme ?? current.colorScheme,
        orientation: orientation ?? current.orientation,
        accentColor: normalizedAccent ?? current.accentColor,
        fontSize: (fontSize ?? current.fontSize).clamp(10, 36),
      );
      await _persistSchemeSettings();
    });
  }

  Future<bool> saveEmbedded(bool value) => run(() async {
    uiSettings = await core.setImeEmbeddedComposition(embedded: value);
    await _persistSchemeSettings();
  });
  Future<bool> saveDesktopEnglish(EnglishModeDto value) => run(() async {
    await _readLocal();
    if (value == EnglishModeDto.schema && !englishReady) {
      throw StateError(AppStrings.englishNotReady);
    }
    desktopEnglishMode = await core.setDesktopEnglishMode(mode: value);
    await _persistSchemeSettings();
  });

  Future<bool> open(String value) => run(() => _open(value));
  Future<void> _open(String value) async {
    if (isAndroid) {
      await android.openUrl(value);
    } else if (isIos) {
      await const IosHost().openUrl(value);
    } else {
      final result = await Process.run(
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
  Future<bool> openCustomDirectory() async {
    if (selectedDirectory == null || isOpeningCustomDirectory) return false;
    isOpeningCustomDirectory = true;
    _changed();
    try {
      await _open(selectedTreeUri ?? selectedDirectory!);
      return true;
    } catch (failure) {
      addOperationLogs(['[OPEN DIR ERROR] ${errorText(failure)}']);
      return false;
    } finally {
      isOpeningCustomDirectory = false;
      _changed();
    }
  }

  Future<bool> openTheme() => uiSettings?.themePath == null
      ? Future.value(false)
      : open(uiSettings!.themePath!);
  Future<bool> openAppUpdate() async {
    if (appUpdate == null) return false;
    try {
      await _open(appUpdate!.releaseUrl);
      return true;
    } catch (failure) {
      error = errorText(failure);
      _changed();
      return false;
    }
  }

  // A title change must not toggle `busy`; a missing macOS channel is ignored.
  Future<void> setWindowTitle([String? title]) async {
    if (info.platform != BridgePlatform.macOs) return;
    try {
      await const MethodChannel('ink.rea.keytao/window')
          .invokeMethod<void>('setTitle', title ?? activePage.title);
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
    if (_disposed || logsLoading) return false;
    await _readLogs();
    return true;
  }

  Future<bool> _logAction(
    String failurePrefix,
    Future<void> Function() action, {
    String? notice,
  }) => run(() async {
    debugError = null;
    debugNotice = null;
    try {
      await action();
      debugNotice = notice;
    } catch (failure) {
      debugError = AppStrings.failure(failurePrefix, errorText(failure));
      throw StateError(debugError!);
    }
  });

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
    _dictionarySyncDebounce?.cancel();
    _schemeSyncDebounce?.cancel();
    if (isWindows && _events != null) {
      _windowsChannel.setMethodCallHandler(null);
    }
    unawaited(_events?.cancel());
    unawaited(_androidProgress?.cancel());
    super.dispose();
  }
}
